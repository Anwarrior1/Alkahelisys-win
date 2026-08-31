use crate::db::{default_commission_bps, insert_audit, new_id, now, Database, MONEY_SCALE};
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::{header, HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post, put},
    Json, Router,
};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use rand::RngCore;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    path::{Path as FsPath, PathBuf},
    sync::{Arc, Mutex},
};
use tower_http::{cors::CorsLayer, trace::TraceLayer};

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Mutex<Database>>,
    pub data_dir: PathBuf,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Principal {
    id: String,
    full_name: String,
    username: String,
    worker_id: Option<String>,
    role_code: String,
    role_name: String,
    theme: String,
    permissions: Vec<String>,
}

impl Principal {
    fn is_manager(&self) -> bool {
        self.role_code == "manager"
    }

    fn has_permission(&self, permission: &str) -> bool {
        self.is_manager() || self.permissions.iter().any(|value| value == permission)
    }
}

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
    fn bad(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }
    fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "انتهت الجلسة أو بيانات الدخول غير صحيحة",
        )
    }
    fn forbidden() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "لا تملك صلاحية الوصول إلى هذه البيانات",
        )
    }
    fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "السجل المطلوب غير موجود")
    }
    fn internal(error: impl std::fmt::Display) -> Self {
        eprintln!("API error: {error}");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "تعذر إتمام العملية بأمان",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({"error": self.message}))).into_response()
    }
}

type ApiResult = Result<Json<Value>, ApiError>;

fn ok(data: Value) -> Json<Value> {
    Json(json!({ "data": data }))
}

pub fn build_router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(tower_http::cors::Any)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]);

    Router::new()
        .route("/api/health", get(health))
        .route("/api/setup/status", get(setup_status))
        .route("/api/setup/initial-manager", post(initial_manager))
        .route("/api/auth/login", post(login))
        .route("/api/auth/logout", post(logout))
        .route("/api/auth/me", get(me))
        .route("/api/preferences/theme", put(update_theme))
        .route("/api/dashboard", get(dashboard))
        .route("/api/washes", get(list_washes).post(create_wash))
        .route("/api/washes/:id", patch(update_wash))
        .route("/api/washes/:id/paid", patch(set_wash_paid))
        .route("/api/washes/:id/void", post(void_wash))
        .route("/api/paid-cars", get(list_paid_cars))
        .route("/api/overnight-cars", get(list_overnight_cars))
        .route("/api/overnight-cars/:id", delete(delete_overnight_car))
        .route("/api/workers", get(list_workers).post(create_worker))
        .route(
            "/api/workers/:id",
            get(worker_detail)
                .patch(update_worker)
                .delete(delete_worker),
        )
        .route("/api/workers/:id/financial", get(worker_financial))
        .route(
            "/api/workers/:id/daily-value",
            put(update_worker_daily_value),
        )
        .route(
            "/api/workers/:id/withdrawals-returns",
            get(worker_withdrawal_returns).post(create_worker_withdrawal_return),
        )
        .route("/api/showrooms", get(list_showrooms).post(create_showroom))
        .route(
            "/api/showrooms/:id",
            get(showroom_detail)
                .patch(update_showroom)
                .delete(delete_showroom),
        )
        .route("/api/showrooms/:id/financial", get(showroom_financial))
        .route("/api/showrooms/:id/statistics", get(showroom_statistics))
        .route("/api/showroom-debts", get(list_showroom_debts))
        .route("/api/showroom-debts/:id", get(showroom_debt_detail))
        .route("/api/payroll", get(payroll_summary))
        .route("/api/payroll/employees", post(create_payroll_employee))
        .route("/api/payroll/workers/:id/salary", put(set_worker_salary))
        .route("/api/payroll/workers/:id", delete(delete_payroll_employee))
        .route(
            "/api/payroll/withdrawals",
            get(list_salary_withdrawals).post(create_salary_withdrawal),
        )
        .route(
            "/api/payroll/withdrawals/:id",
            patch(update_salary_withdrawal).delete(delete_salary_withdrawal),
        )
        .route(
            "/api/payroll/deductions",
            get(list_salary_deductions).post(create_salary_deduction),
        )
        .route(
            "/api/payroll/deductions/:id",
            patch(update_salary_deduction).delete(delete_salary_deduction),
        )
        .route(
            "/api/showroom-payments",
            get(list_showroom_payments).post(create_showroom_payment),
        )
        .route(
            "/api/showroom-payments/:id",
            patch(update_showroom_payment).delete(delete_showroom_payment),
        )
        .route("/api/expenses", get(list_expenses).post(create_expense))
        .route(
            "/api/expenses/:id",
            get(expense_detail)
                .patch(update_expense)
                .delete(delete_expense),
        )
        .route("/api/finance/overview", get(finance_overview))
        .route("/api/reports/operational", get(operational_report))
        .route("/api/reports/financial", get(financial_report))
        .route("/api/settings", get(get_settings).put(update_settings))
        .route("/api/users", get(list_users).post(create_user))
        .route("/api/users/:id", patch(update_user).delete(delete_user))
        .route(
            "/api/users/:id/profile-picture",
            get(get_profile_picture)
                .put(upload_profile_picture)
                .delete(delete_profile_picture),
        )
        .route("/api/users/:id/permissions", put(update_user_permissions))
        .route("/api/roles", get(list_roles))
        .route("/api/roles/:id/permissions", put(update_role_permissions))
        .route("/api/audit-logs", get(list_audit_logs))
        .route("/api/backups", get(list_backups).post(create_backup))
        .route("/api/backups/:id", delete(delete_backup))
        .route("/api/backups/:id/download", get(download_backup))
        .route("/api/backups/:id/export", put(export_backup))
        .route("/api/backups/restore", post(restore_backup))
        .route("/api/backups/restore-upload", post(restore_backup_upload))
        .layer(cors)
        .layer(DefaultBodyLimit::max(100 * 1024 * 1024))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn health() -> ApiResult {
    Ok(ok(
        json!({"service": "مركز الكحيلي لغسيل السيارات", "status": "ok"}),
    ))
}

fn token_hash(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

fn random_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn normalized_username(value: &str) -> Result<String, ApiError> {
    let username = value.trim().to_lowercase();
    if !(3..=48).contains(&username.chars().count()) {
        return Err(ApiError::bad("يجب أن يتكون اسم المستخدم من 3 إلى 48 حرفًا"));
    }
    if username.chars().any(|character| character.is_control()) {
        return Err(ApiError::bad("اسم المستخدم غير صالح"));
    }
    Ok(username)
}

fn valid_password(value: &str) -> Result<(), ApiError> {
    if value.chars().count() < 10 {
        return Err(ApiError::bad("يجب ألا تقل كلمة المرور عن 10 أحرف"));
    }
    Ok(())
}

fn hash_password(password: &str) -> Result<String, ApiError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|value| value.to_string())
        .map_err(ApiError::internal)
}

fn principal_from_headers(state: &AppState, headers: &HeaderMap) -> Result<Principal, ApiError> {
    let raw = headers
        .get(header::AUTHORIZATION)
        .and_then(|header_value| header_value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or_else(ApiError::unauthorized)?;
    let hash = token_hash(raw);
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let row = db.conn.query_row(
        "SELECT u.id, u.full_name, u.username_norm, u.worker_id, r.code, r.name_ar, COALESCE(p.theme, 'light'), s.expires_at
         FROM sessions s
         JOIN users u ON u.id=s.user_id
         JOIN user_roles ur ON ur.user_id=u.id
         JOIN roles r ON r.id=ur.role_id
         LEFT JOIN user_preferences p ON p.user_id=u.id
         WHERE s.token_hash=?1 AND s.revoked_at IS NULL AND u.is_active=1 AND u.deleted_at IS NULL
         ORDER BY CASE r.code WHEN 'manager' THEN 0 ELSE 1 END LIMIT 1",
        [hash],
        |row| Ok((
            Principal {
                id: row.get(0)?, full_name: row.get(1)?, username: row.get(2)?, worker_id: row.get(3)?, role_code: row.get(4)?, role_name: row.get(5)?, theme: row.get(6)?, permissions: Vec::new(),
            },
            row.get::<_, String>(7)?,
        )),
    ).optional().map_err(ApiError::internal)?;
    let (mut principal, expires_at) = row.ok_or_else(ApiError::unauthorized)?;
    let expires =
        DateTime::parse_from_rfc3339(&expires_at).map_err(|_| ApiError::unauthorized())?;
    if expires.with_timezone(&Utc) <= Utc::now() {
        return Err(ApiError::unauthorized());
    }
    principal.permissions = permission_codes_for_user(&db.conn, &principal.id)?;
    Ok(principal)
}

fn permission_codes_for_user(conn: &Connection, user_id: &str) -> Result<Vec<String>, ApiError> {
    let has_profile: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM user_permission_profiles WHERE user_id=?1)",
            [user_id],
            |row| row.get(0),
        )
        .map_err(ApiError::internal)?;
    let sql = if has_profile {
        "SELECT DISTINCT p.code FROM permissions p
         JOIN user_permissions up ON up.permission_id=p.id
         WHERE up.user_id=?1 ORDER BY p.code"
    } else {
        "SELECT DISTINCT p.code FROM permissions p
         JOIN role_permissions rp ON rp.permission_id=p.id
         JOIN user_roles ur ON ur.role_id=rp.role_id
         WHERE ur.user_id=?1 ORDER BY p.code"
    };
    let mut statement = conn.prepare(sql).map_err(ApiError::internal)?;
    let rows = statement
        .query_map([user_id], |row| row.get::<_, String>(0))
        .map_err(ApiError::internal)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(ApiError::internal)
}

fn authorize(
    state: &AppState,
    headers: &HeaderMap,
    permission: &str,
) -> Result<Principal, ApiError> {
    let principal = principal_from_headers(state, headers)?;
    if !principal.has_permission(permission) {
        return Err(ApiError::forbidden());
    }
    Ok(principal)
}

fn manager(state: &AppState, headers: &HeaderMap) -> Result<Principal, ApiError> {
    let principal = principal_from_headers(state, headers)?;
    if !principal.is_manager() {
        return Err(ApiError::forbidden());
    }
    Ok(principal)
}

fn ensure_worker_access(principal: &Principal, worker_id: &str) -> Result<(), ApiError> {
    if principal.is_manager() || principal.worker_id.as_deref() == Some(worker_id) {
        Ok(())
    } else {
        Err(ApiError::forbidden())
    }
}

fn create_session(conn: &Connection, user_id: &str) -> Result<String, ApiError> {
    let token = random_token();
    conn.execute(
        "INSERT INTO sessions(id, token_hash, user_id, expires_at, created_at) VALUES(?1, ?2, ?3, ?4, ?5)",
        params![new_id(), token_hash(&token), user_id, (Utc::now() + Duration::days(7)).to_rfc3339(), now()],
    ).map_err(ApiError::internal)?;
    Ok(token)
}

fn insert_audit_tx(
    tx: &Transaction<'_>,
    user_id: Option<&str>,
    action: &str,
    entity_type: &str,
    entity_id: Option<&str>,
    description: &str,
    metadata: Option<&Value>,
) -> Result<(), ApiError> {
    tx.execute(
        "INSERT INTO audit_logs(id, user_id, action, entity_type, entity_id, description, metadata_json, created_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![new_id(), user_id, action, entity_type, entity_id, description, metadata.map(Value::to_string), now()],
    ).map_err(ApiError::internal)?;
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InitialManagerInput {
    full_name: String,
    username: String,
    password: String,
}

async fn setup_status(State(state): State<AppState>) -> ApiResult {
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let count: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))
        .map_err(ApiError::internal)?;
    Ok(ok(json!({"needsSetup": count == 0})))
}

async fn initial_manager(
    State(state): State<AppState>,
    Json(input): Json<InitialManagerInput>,
) -> ApiResult {
    let full_name = input.full_name.trim();
    if full_name.chars().count() < 3 {
        return Err(ApiError::bad("أدخل الاسم الكامل للمدير"));
    }
    let username = normalized_username(&input.username)?;
    valid_password(&input.password)?;
    let hash = hash_password(&input.password)?;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let existing: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))
        .map_err(ApiError::internal)?;
    if existing > 0 {
        return Err(ApiError::new(StatusCode::CONFLICT, "تم إعداد النظام مسبقًا"));
    }
    let user_id = new_id();
    let timestamp = now();
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    tx.execute(
        "INSERT INTO users(id, full_name, username_norm, password_hash, is_active, created_at, updated_at) VALUES(?1, ?2, ?3, ?4, 1, ?5, ?5)",
        params![user_id, full_name, username, hash, timestamp],
    ).map_err(|error| ApiError::new(StatusCode::CONFLICT, format!("تعذر إنشاء المدير: {error}")))?;
    tx.execute(
        "INSERT INTO user_roles(user_id, role_id) VALUES(?1, 'role-manager')",
        params![user_id],
    )
    .map_err(ApiError::internal)?;
    tx.execute(
        "INSERT INTO user_preferences(user_id, theme, updated_at) VALUES(?1, 'light', ?2)",
        params![user_id, now()],
    )
    .map_err(ApiError::internal)?;
    insert_audit_tx(
        &tx,
        Some(&user_id),
        "USER_CREATED",
        "user",
        Some(&user_id),
        "تم إنشاء حساب المدير الأول",
        None,
    )?;
    tx.commit().map_err(ApiError::internal)?;
    let token = create_session(&db.conn, &user_id)?;
    Ok(ok(json!({
        "token": token,
        "user": {"id": user_id, "fullName": full_name, "username": username, "roleCode": "manager", "roleName": "مدير", "theme": "light"}
    })))
}

#[derive(Deserialize)]
struct LoginInput {
    username: String,
    password: String,
}

async fn login(State(state): State<AppState>, Json(input): Json<LoginInput>) -> ApiResult {
    let username = normalized_username(&input.username)?;
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let user = db
        .conn
        .query_row(
            "SELECT id, full_name, password_hash, is_active FROM users WHERE username_norm=?1 AND deleted_at IS NULL",
            [username.clone()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()
        .map_err(ApiError::internal)?;
    let (user_id, _full_name, password_hash, active) = user.ok_or_else(ApiError::unauthorized)?;
    if active != 1 {
        return Err(ApiError::new(StatusCode::FORBIDDEN, "هذا الحساب معطّل"));
    }
    let parsed_hash = PasswordHash::new(&password_hash).map_err(ApiError::internal)?;
    if Argon2::default()
        .verify_password(input.password.as_bytes(), &parsed_hash)
        .is_err()
    {
        return Err(ApiError::unauthorized());
    }
    let token = create_session(&db.conn, &user_id)?;
    insert_audit(
        &db.conn,
        Some(&user_id),
        "LOGIN",
        "session",
        None,
        "تم تسجيل الدخول",
        None,
    )
    .map_err(ApiError::internal)?;
    let principal = principal_for_user(&db.conn, &user_id)?;
    Ok(ok(json!({"token": token, "user": principal})))
}

fn principal_for_user(conn: &Connection, user_id: &str) -> Result<Principal, ApiError> {
    let mut principal = conn.query_row(
        "SELECT u.id, u.full_name, u.username_norm, u.worker_id, r.code, r.name_ar, COALESCE(p.theme,'light')
         FROM users u JOIN user_roles ur ON ur.user_id=u.id JOIN roles r ON r.id=ur.role_id
         LEFT JOIN user_preferences p ON p.user_id=u.id WHERE u.id=?1
         ORDER BY CASE r.code WHEN 'manager' THEN 0 ELSE 1 END LIMIT 1",
        [user_id],
        |row| {
            Ok(Principal {
                id: row.get(0)?,
                full_name: row.get(1)?,
                username: row.get(2)?,
                worker_id: row.get(3)?,
                role_code: row.get(4)?,
                role_name: row.get(5)?,
                theme: row.get(6)?,
                permissions: Vec::new(),
            })
        },
    )
    .map_err(ApiError::internal)?;
    principal.permissions = permission_codes_for_user(conn, user_id)?;
    Ok(principal)
}

async fn me(State(state): State<AppState>, headers: HeaderMap) -> ApiResult {
    let principal = principal_from_headers(&state, &headers)?;
    Ok(ok(
        serde_json::to_value(principal).map_err(ApiError::internal)?
    ))
}

fn profile_picture_path(data_dir: &FsPath, user_id: &str) -> PathBuf {
    data_dir
        .join("profile-pictures")
        .join(format!("{user_id}.img"))
}

fn can_manage_profile_picture(
    state: &AppState,
    headers: &HeaderMap,
    target_id: &str,
) -> Result<Principal, ApiError> {
    let principal = principal_from_headers(state, headers)?;
    if principal.id != target_id && !principal.has_permission("users.manage") {
        return Err(ApiError::forbidden());
    }
    Ok(principal)
}

async fn get_profile_picture(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(user_id): Path<String>,
) -> Result<Response, ApiError> {
    let _principal = can_manage_profile_picture(&state, &headers, &user_id)?;
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let content_type: Option<String> = db
        .conn
        .query_row(
            "SELECT content_type FROM user_profile_pictures WHERE user_id=?1",
            [user_id.clone()],
            |row| row.get(0),
        )
        .optional()
        .map_err(ApiError::internal)?;
    let Some(content_type) = content_type else {
        return Err(ApiError::not_found());
    };
    let path = profile_picture_path(&state.data_dir, &user_id);
    let bytes = fs::read(path).map_err(|_| ApiError::not_found())?;
    Ok((
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "no-cache".to_owned()),
        ],
        bytes,
    )
        .into_response())
}

async fn upload_profile_picture(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(user_id): Path<String>,
    mut multipart: Multipart,
) -> ApiResult {
    let principal = can_manage_profile_picture(&state, &headers, &user_id)?;
    let mut content_type = None;
    let mut bytes = None;
    while let Some(field) = multipart.next_field().await.map_err(ApiError::internal)? {
        if field.name() == Some("file") {
            content_type = field.content_type().map(str::to_owned);
            bytes = Some(field.bytes().await.map_err(ApiError::internal)?);
            break;
        }
    }
    let content_type = content_type.ok_or_else(|| ApiError::bad("اختر صورة صالحة"))?;
    let bytes = bytes.ok_or_else(|| ApiError::bad("اختر صورة صالحة"))?;
    if bytes.is_empty() || bytes.len() > 5 * 1024 * 1024 {
        return Err(ApiError::bad("حجم الصورة يجب ألا يتجاوز 5 ميغابايت"));
    }
    let valid = match content_type.as_str() {
        "image/jpeg" => bytes.starts_with(&[0xff, 0xd8, 0xff]),
        "image/png" => bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]),
        "image/webp" => bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
        _ => false,
    };
    if !valid {
        return Err(ApiError::bad("نوع الصورة غير مدعوم أو الملف غير صالح"));
    }
    let picture_dir = state.data_dir.join("profile-pictures");
    fs::create_dir_all(&picture_dir).map_err(ApiError::internal)?;
    let path = profile_picture_path(&state.data_dir, &user_id);
    let temp_path = picture_dir.join(format!(".{user_id}.upload"));
    fs::write(&temp_path, &bytes).map_err(ApiError::internal)?;
    if let Err(error) = fs::rename(&temp_path, &path) {
        let _ = fs::remove_file(&temp_path);
        return Err(ApiError::internal(error));
    }
    let timestamp = now();
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    db.conn.execute(
        "INSERT INTO user_profile_pictures(user_id,file_name,content_type,created_at,updated_at)
         VALUES(?1,?2,?3,?4,?4)
         ON CONFLICT(user_id) DO UPDATE SET file_name=excluded.file_name,content_type=excluded.content_type,updated_at=excluded.updated_at",
        params![user_id, path.file_name().and_then(|value| value.to_str()).unwrap_or("profile.img"), content_type, timestamp],
    ).map_err(ApiError::internal)?;
    insert_audit(
        &db.conn,
        Some(&principal.id),
        "PROFILE_PICTURE_UPDATED",
        "user",
        Some(&user_id),
        "تم تحديث صورة الحساب",
        None,
    )
    .map_err(ApiError::internal)?;
    Ok(ok(json!({"updated":true})))
}

async fn delete_profile_picture(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(user_id): Path<String>,
) -> ApiResult {
    let principal = can_manage_profile_picture(&state, &headers, &user_id)?;
    let path = profile_picture_path(&state.data_dir, &user_id);
    if path.is_file() {
        fs::remove_file(&path).map_err(ApiError::internal)?;
    }
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    db.conn
        .execute(
            "DELETE FROM user_profile_pictures WHERE user_id=?1",
            [user_id.clone()],
        )
        .map_err(ApiError::internal)?;
    insert_audit(
        &db.conn,
        Some(&principal.id),
        "PROFILE_PICTURE_REMOVED",
        "user",
        Some(&user_id),
        "تمت إزالة صورة الحساب",
        None,
    )
    .map_err(ApiError::internal)?;
    Ok(ok(json!({"deleted":true})))
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> ApiResult {
    let raw = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or_else(ApiError::unauthorized)?;
    let principal = principal_from_headers(&state, &headers)?;
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    db.conn
        .execute(
            "UPDATE sessions SET revoked_at=?1 WHERE token_hash=?2",
            params![now(), token_hash(raw)],
        )
        .map_err(ApiError::internal)?;
    insert_audit(
        &db.conn,
        Some(&principal.id),
        "LOGOUT",
        "session",
        None,
        "تم تسجيل الخروج",
        None,
    )
    .map_err(ApiError::internal)?;
    Ok(ok(json!({"loggedOut": true})))
}

#[derive(Deserialize)]
struct ThemeInput {
    theme: String,
}

async fn update_theme(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<ThemeInput>,
) -> ApiResult {
    let principal = principal_from_headers(&state, &headers)?;
    if !matches!(input.theme.as_str(), "light" | "dark" | "system") {
        return Err(ApiError::bad("السمة المختارة غير صالحة"));
    }
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    db.conn
        .execute(
            "INSERT INTO user_preferences(user_id, theme, updated_at) VALUES(?1, ?2, ?3)
         ON CONFLICT(user_id) DO UPDATE SET theme=excluded.theme, updated_at=excluded.updated_at",
            params![principal.id, input.theme, now()],
        )
        .map_err(ApiError::internal)?;
    Ok(ok(json!({"theme": input.theme})))
}

fn parse_milli(value: &str) -> Result<i64, ApiError> {
    let normalized = value.trim().replace(',', ".");
    if normalized.is_empty() || normalized.starts_with('-') {
        return Err(ApiError::bad("أدخل مبلغًا موجبًا صالحًا"));
    }
    let (whole, fraction) = normalized.split_once('.').unwrap_or((&normalized, ""));
    if whole.is_empty()
        || !whole.chars().all(|c| c.is_ascii_digit())
        || !fraction.chars().all(|c| c.is_ascii_digit())
        || fraction.len() > 3
    {
        return Err(ApiError::bad(
            "صيغة المبلغ غير صالحة؛ استخدم حتى 3 منازل عشرية",
        ));
    }
    let integer: i64 = whole
        .parse()
        .map_err(|_| ApiError::bad("المبلغ كبير جدًا"))?;
    let fraction_value = if fraction.is_empty() {
        0
    } else {
        format!("{fraction:0<3}")
            .parse::<i64>()
            .map_err(|_| ApiError::bad("صيغة المبلغ غير صالحة"))?
    };
    integer
        .checked_mul(MONEY_SCALE)
        .and_then(|value| value.checked_add(fraction_value))
        .filter(|value| *value > 0)
        .ok_or_else(|| ApiError::bad("المبلغ غير صالح"))
}

// Libya has used UTC+02:00 year-round since 2013. Working calendar dates are
// interpreted in the business timezone (Africa/Tripoli), independently from
// the operating system timezone that runs the API.
const BUSINESS_UTC_OFFSET_HOURS: i64 = 2;

fn business_today() -> NaiveDate {
    (Utc::now() + Duration::hours(BUSINESS_UTC_OFFSET_HOURS)).date_naive()
}

fn selected_business_date(query: &HashMap<String, String>) -> Result<Option<NaiveDate>, ApiError> {
    let Some(value) = query
        .get("date")
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    let selected = NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|_| ApiError::bad("تاريخ العمل المحدد غير صالح"))?;
    if selected > business_today() {
        return Err(ApiError::bad("لا يمكن عرض تاريخ بعد اليوم"));
    }
    Ok(Some(selected))
}

fn business_day_range(selected: NaiveDate) -> Result<(String, String), ApiError> {
    let start = selected
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| ApiError::bad("تاريخ العمل المحدد غير صالح"))?
        - Duration::hours(BUSINESS_UTC_OFFSET_HOURS);
    let end = start + Duration::days(1) - Duration::milliseconds(1);
    Ok((
        format!("{}Z", start.format("%Y-%m-%dT%H:%M:%S%.3f")),
        format!("{}Z", end.format("%Y-%m-%dT%H:%M:%S%.3f")),
    ))
}

fn date_range(query: &HashMap<String, String>) -> Result<(String, String), ApiError> {
    if let Some(selected) = selected_business_date(query)? {
        return business_day_range(selected);
    }
    if !query.contains_key("from") && !query.contains_key("to") {
        return business_day_range(business_today());
    }
    let from = query
        .get("from")
        .cloned()
        .unwrap_or_else(|| "0000-01-01T00:00:00Z".into());
    let to = query
        .get("to")
        .cloned()
        .unwrap_or_else(|| "9999-12-31T23:59:59Z".into());
    Ok((from, to))
}

fn round_percentage(amount: i64, bps: i64) -> i64 {
    (amount.saturating_mul(bps).saturating_add(5000)) / 10000
}

fn total_for(conn: &Connection, sql: &str, params: impl rusqlite::Params) -> Result<i64, ApiError> {
    conn.query_row(sql, params, |row| row.get::<_, i64>(0))
        .map_err(ApiError::internal)
}

async fn dashboard(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "operational.read")?;
    let can_view_all = principal.is_manager();
    let owner_id = principal.id.clone();
    let selected_date = selected_business_date(&query)?.unwrap_or_else(business_today);
    let selected_date_key = selected_date.format("%Y-%m-%d").to_string();
    let selected_month_key = selected_date.format("%Y-%m").to_string();
    let mut selected_day_query = query.clone();
    selected_day_query.insert("date".to_owned(), selected_date_key.clone());
    let (selected_day_start, selected_day_end) = date_range(&selected_day_query)?;
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let today_washes = total_for(&db.conn, "SELECT COUNT(*) FROM wash_operations WHERE status='posted' AND occurred_at BETWEEN ?1 AND ?2 AND (?3=1 OR created_by=?4)", params![selected_day_start.clone(), selected_day_end.clone(), if can_view_all { 1 } else { 0 }, owner_id.clone()])?;
    let month_washes = total_for(&db.conn, "SELECT COUNT(*) FROM wash_operations WHERE status='posted' AND strftime('%Y-%m', occurred_at,'+2 hours')=?1 AND (?2=1 OR created_by=?3)", params![selected_month_key.clone(), if can_view_all { 1 } else { 0 }, owner_id.clone()])?;
    let mut recent = Vec::new();
    let mut statement = db.conn.prepare(
        "SELECT w.id, w.vehicle_make, w.vehicle_model, w.license_plate, w.price_milli, w.occurred_at, w.payment_type, w.status, worker.full_name, w.commission_milli
         FROM wash_operations w JOIN workers worker ON worker.id=w.worker_id
         WHERE w.status='posted' AND w.is_paid=0 AND w.occurred_at BETWEEN ?1 AND ?2 AND (?3=1 OR w.created_by=?4)
         ORDER BY w.occurred_at DESC LIMIT 8",
    ).map_err(ApiError::internal)?;
    let can_view_financial = principal.has_permission("financial.manage");
    let rows = statement.query_map(params![selected_day_start.clone(), selected_day_end.clone(), if can_view_all { 1 } else { 0 }, owner_id.clone()], move |row| {
        let mut item = json!({
            "id": row.get::<_, String>(0)?, "vehicleMake": row.get::<_, String>(1)?, "vehicleModel": row.get::<_, String>(2)?,
            "licensePlate": row.get::<_, Option<String>>(3)?, "occurredAt": row.get::<_, String>(5)?,
            "paymentType": row.get::<_, String>(6)?, "status": row.get::<_, String>(7)?, "worker": {"fullName": row.get::<_, String>(8)?}
        });
        if can_view_financial { item["priceMilli"] = json!(row.get::<_, i64>(4)?); item["commissionMilli"] = json!(row.get::<_, i64>(9)?); }
        Ok(item)
    }).map_err(ApiError::internal)?;
    for row in rows {
        recent.push(row.map_err(ApiError::internal)?);
    }
    let mut response = json!({
        "role": principal.role_code,
        "todayWashes": today_washes,
        "monthWashes": month_washes,
        "recentWashes": recent,
        "selectedDate": selected_date_key,
        "businessTimeZone": "Africa/Tripoli",
    });
    if principal.has_permission("dashboard.daily_revenue.read") {
        let today_revenue = total_for(&db.conn, "SELECT COALESCE(SUM(price_milli),0) FROM wash_operations WHERE status='posted' AND occurred_at BETWEEN ?1 AND ?2 AND (?3=1 OR created_by=?4)", params![selected_day_start.clone(), selected_day_end.clone(), if can_view_all { 1 } else { 0 }, owner_id.clone()])?;
        response["financial"] = json!({"todayRevenue": today_revenue});
    }
    if principal.has_permission("financial.manage") {
        let month_revenue = total_for(&db.conn, "SELECT COALESCE(SUM(price_milli),0) FROM wash_operations WHERE status='posted' AND strftime('%Y-%m', occurred_at,'+2 hours')=?1 AND (?2=1 OR created_by=?3)", params![selected_month_key.clone(), if can_view_all { 1 } else { 0 }, owner_id.clone()])?;
        let worker_payable = total_for(&db.conn,
            "SELECT MAX(0, COALESCE((SELECT SUM(commission_milli) FROM wash_operations WHERE status='posted' AND strftime('%Y-%m',occurred_at,'+2 hours')=?1 AND (?2=1 OR created_by=?3)),0) -
                     COALESCE((SELECT SUM(ea.amount_milli) FROM expense_allocations ea JOIN expenses e ON e.id=ea.expense_id WHERE strftime('%Y-%m',e.occurred_at,'+2 hours')=?1 AND (?2=1 OR e.created_by=?3)),0))", params![selected_month_key.clone(), if can_view_all { 1 } else { 0 }, owner_id.clone()])?;
        let expenses = total_for(
            &db.conn,
            "SELECT COALESCE(SUM(amount_milli),0) FROM expenses WHERE strftime('%Y-%m', occurred_at,'+2 hours')=?1 AND (?2=1 OR created_by=?3)",
            params![selected_month_key.clone(), if can_view_all { 1 } else { 0 }, owner_id.clone()],
        )?;
        let showroom_outstanding = total_for(&db.conn,
            "SELECT COALESCE((SELECT SUM(price_milli) FROM wash_operations WHERE status='posted' AND payment_type='showroom' AND strftime('%Y-%m',occurred_at,'+2 hours')=?1 AND (?2=1 OR created_by=?3)) -
                     (SELECT COALESCE(SUM(amount_milli),0) FROM showroom_payments WHERE strftime('%Y-%m',paid_at,'+2 hours')=?1 AND (?2=1 OR created_by=?3)),0)", params![selected_month_key.clone(), if can_view_all { 1 } else { 0 }, owner_id.clone()])?;
        let business_share = total_for(&db.conn, "SELECT COALESCE(SUM(business_share_milli),0) FROM wash_operations WHERE status='posted' AND strftime('%Y-%m',occurred_at,'+2 hours')=?1 AND (?2=1 OR created_by=?3)", params![selected_month_key.clone(), if can_view_all { 1 } else { 0 }, owner_id.clone()])?;
        let business_expenses = total_for(
            &db.conn,
            "SELECT COALESCE(SUM(business_amount_milli),0) FROM expenses WHERE strftime('%Y-%m',occurred_at,'+2 hours')=?1 AND (?2=1 OR created_by=?3)",
            params![selected_month_key, if can_view_all { 1 } else { 0 }, owner_id.clone()],
        )?;
        if response.get("financial").is_none() {
            response["financial"] = json!({});
        }
        response["financial"]["monthRevenue"] = json!(month_revenue);
        response["financial"]["workerPayable"] = json!(worker_payable);
        response["financial"]["expenses"] = json!(expenses);
        response["financial"]["showroomOutstanding"] = json!(showroom_outstanding);
        response["financial"]["businessShare"] = json!(business_share);
        response["financial"]["netProfit"] = json!(business_share - business_expenses);
    }
    if principal.is_manager() {
        let today_customer_revenue = total_for(&db.conn, "SELECT COALESCE(SUM(price_milli),0) FROM wash_operations WHERE status='posted' AND payment_type='cash' AND occurred_at BETWEEN ?1 AND ?2", params![selected_day_start.clone(), selected_day_end.clone()])?;
        let today_net_profit = total_for(&db.conn, "SELECT COALESCE(SUM(business_share_milli),0) FROM wash_operations WHERE status='posted' AND payment_type='cash' AND occurred_at BETWEEN ?1 AND ?2", params![selected_day_start.clone(), selected_day_end.clone()])?;
        let today_showroom_revenue = total_for(&db.conn, "SELECT COALESCE(SUM(price_milli),0) FROM wash_operations WHERE status='posted' AND payment_type='showroom' AND occurred_at BETWEEN ?1 AND ?2", params![selected_day_start.clone(), selected_day_end.clone()])?;
        let today_showroom_net_profit = total_for(&db.conn, "SELECT COALESCE(SUM(business_share_milli),0) FROM wash_operations WHERE status='posted' AND payment_type='showroom' AND occurred_at BETWEEN ?1 AND ?2", params![selected_day_start, selected_day_end])?;
        if response.get("financial").is_none() {
            response["financial"] = json!({});
        }
        response["financial"]["todayCustomerRevenue"] = json!(today_customer_revenue);
        response["financial"]["todayNetProfit"] = json!(today_net_profit);
        response["financial"]["todayShowroomRevenue"] = json!(today_showroom_revenue);
        response["financial"]["todayShowroomNetProfit"] = json!(today_showroom_net_profit);
    }
    Ok(ok(response))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WashInput {
    vehicle_make: String,
    vehicle_model: String,
    manufacture_year: Option<i32>,
    license_plate: Option<String>,
    car_color: Option<String>,
    price: String,
    worker_id: String,
    payment_type: String,
    showroom_id: Option<String>,
    showroom_payment_method: Option<String>,
    occurred_at: Option<String>,
    client_request_id: Option<String>,
    mark_as_overnight: Option<bool>,
}

fn trim_required(value: &str, label: &str) -> Result<String, ApiError> {
    let normalized = value.trim();
    if normalized.is_empty() || normalized.chars().count() > 120 {
        return Err(ApiError::bad(format!("أدخل {label} بصورة صحيحة")));
    }
    Ok(normalized.to_owned())
}

async fn list_washes(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "operational.read")?;
    let (from, to) = date_range(&query)?;
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let mut washes = Vec::new();
    if principal.has_permission("financial.manage") {
        let can_manage_overnight = principal.is_manager();
        let can_view_all = principal.is_manager();
        let owner_id = principal.id.clone();
        let mut statement = db.conn.prepare(
            "SELECT w.id,w.vehicle_make,w.vehicle_model,w.manufacture_year,w.license_plate,w.car_color,w.price_milli,w.occurred_at,w.payment_type,w.status,
                    worker.id,worker.full_name,showroom.id,showroom.name,w.commission_bps,w.commission_milli,w.business_share_milli,w.showroom_payment_method,
                    EXISTS(SELECT 1 FROM overnight_cars overnight WHERE overnight.wash_id=w.id),w.is_paid,w.paid_at
             FROM wash_operations w JOIN workers worker ON worker.id=w.worker_id LEFT JOIN showrooms showroom ON showroom.id=w.showroom_id
             WHERE w.status='posted' AND w.is_paid=0 AND w.occurred_at BETWEEN ?1 AND ?2 AND (?3=1 OR w.created_by=?4) ORDER BY w.occurred_at DESC LIMIT 300",
        ).map_err(ApiError::internal)?;
        let rows = statement.query_map(params![from, to, if can_view_all { 1 } else { 0 }, owner_id], move |row| {
            let mut item = json!({
                "id": row.get::<_, String>(0)?, "vehicleMake": row.get::<_, String>(1)?, "vehicleModel": row.get::<_, String>(2)?,
                "manufactureYear": row.get::<_, Option<i32>>(3)?, "licensePlate": row.get::<_, Option<String>>(4)?, "carColor": row.get::<_, Option<String>>(5)?, "priceMilli": row.get::<_, i64>(6)?,
                "occurredAt": row.get::<_, String>(7)?, "paymentType": row.get::<_, String>(8)?, "status": row.get::<_, String>(9)?,
                "worker": {"id": row.get::<_, String>(10)?, "fullName": row.get::<_, String>(11)?},
                "showroom": row.get::<_, Option<String>>(12)?.map(|id| json!({"id": id, "name": row.get::<_, Option<String>>(13).ok().flatten()})),
                "commissionBps": row.get::<_, i64>(14)?, "commissionMilli": row.get::<_, i64>(15)?, "businessShareMilli": row.get::<_, i64>(16)?,
                "showroomPaymentMethod": row.get::<_, Option<String>>(17)?
            });
            if can_manage_overnight {
                item["isOvernight"] = json!(row.get::<_, i64>(18)? == 1);
            }
            item["isPaid"] = json!(row.get::<_, i64>(19)? == 1);
            item["paidAt"] = json!(row.get::<_, Option<String>>(20)?);
            Ok(item)
        }).map_err(ApiError::internal)?;
        for row in rows {
            washes.push(row.map_err(ApiError::internal)?);
        }
    } else {
        let can_view_all = principal.is_manager();
        let owner_id = principal.id.clone();
        let mut statement = db.conn.prepare(
            "SELECT w.id,w.vehicle_make,w.vehicle_model,w.manufacture_year,w.license_plate,w.car_color,w.price_milli,w.occurred_at,w.payment_type,w.status,
                    worker.id,worker.full_name,showroom.id,showroom.name,w.showroom_payment_method,w.is_paid,w.paid_at
             FROM wash_operations w JOIN workers worker ON worker.id=w.worker_id LEFT JOIN showrooms showroom ON showroom.id=w.showroom_id
             WHERE w.status='posted' AND w.is_paid=0 AND w.occurred_at BETWEEN ?1 AND ?2 AND (?3=1 OR w.created_by=?4) ORDER BY w.occurred_at DESC LIMIT 300",
        ).map_err(ApiError::internal)?;
        let rows = statement.query_map(params![from, to, if can_view_all { 1 } else { 0 }, owner_id], |row| Ok(json!({
            "id": row.get::<_, String>(0)?, "vehicleMake": row.get::<_, String>(1)?, "vehicleModel": row.get::<_, String>(2)?,
            "manufactureYear": row.get::<_, Option<i32>>(3)?, "licensePlate": row.get::<_, Option<String>>(4)?, "carColor": row.get::<_, Option<String>>(5)?, "priceMilli": row.get::<_, i64>(6)?,
            "occurredAt": row.get::<_, String>(7)?, "paymentType": row.get::<_, String>(8)?, "status": row.get::<_, String>(9)?,
            "worker": {"id": row.get::<_, String>(10)?, "fullName": row.get::<_, String>(11)?},
            "showroom": row.get::<_, Option<String>>(12)?.map(|id| json!({"id": id, "name": row.get::<_, Option<String>>(13).ok().flatten()})),
            "showroomPaymentMethod": row.get::<_, Option<String>>(14)?,
            "isPaid": row.get::<_, i64>(15)? == 1,
            "paidAt": row.get::<_, Option<String>>(16)?
        }))).map_err(ApiError::internal)?;
        for row in rows {
            washes.push(row.map_err(ApiError::internal)?);
        }
    }
    Ok(ok(json!({"items": washes})))
}

async fn list_paid_cars(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "operational.read")?;
    let (from, to) = date_range(&query)?;
    let can_view_all = principal.is_manager();
    let include_financials = principal.has_permission("financial.manage");
    let owner_id = principal.id.clone();
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let mut statement = db.conn.prepare(
        "SELECT w.id,w.vehicle_make,w.vehicle_model,w.manufacture_year,w.license_plate,w.car_color,w.price_milli,
                w.occurred_at,w.payment_type,w.status,worker.id,worker.full_name,showroom.id,showroom.name,
                w.commission_bps,w.commission_milli,w.business_share_milli,w.showroom_payment_method,w.paid_at,
                creator.id,creator.full_name
         FROM wash_operations w
         JOIN workers worker ON worker.id=w.worker_id
         LEFT JOIN showrooms showroom ON showroom.id=w.showroom_id
         JOIN users creator ON creator.id=w.created_by
         WHERE w.status='posted' AND w.is_paid=1 AND COALESCE(w.paid_at,w.occurred_at) BETWEEN ?1 AND ?2
               AND (?3=1 OR w.created_by=?4)
         ORDER BY COALESCE(w.paid_at,w.occurred_at) DESC,w.occurred_at DESC
         LIMIT 500",
    ).map_err(ApiError::internal)?;
    let rows = statement.query_map(
        params![&from, &to, if can_view_all { 1 } else { 0 }, owner_id.clone()],
        move |row| {
            let mut item = json!({
                "id":row.get::<_,String>(0)?,"vehicleMake":row.get::<_,String>(1)?,"vehicleModel":row.get::<_,String>(2)?,
                "manufactureYear":row.get::<_,Option<i32>>(3)?,"licensePlate":row.get::<_,Option<String>>(4)?,"carColor":row.get::<_,Option<String>>(5)?,
                "priceMilli":row.get::<_,i64>(6)?,"occurredAt":row.get::<_,String>(7)?,"paymentType":row.get::<_,String>(8)?,
                "status":row.get::<_,String>(9)?,"worker":{"id":row.get::<_,String>(10)?,"fullName":row.get::<_,String>(11)?},
                "showroom":row.get::<_,Option<String>>(12)?.map(|id|json!({"id":id,"name":row.get::<_,Option<String>>(13).ok().flatten()})),
                "showroomPaymentMethod":row.get::<_,Option<String>>(17)?,"isPaid":true,"paidAt":row.get::<_,Option<String>>(18)?,
                "createdBy":{"id":row.get::<_,String>(19)?,"fullName":row.get::<_,String>(20)?}
            });
            if include_financials {
                item["commissionBps"] = json!(row.get::<_,i64>(14)?);
                item["commissionMilli"] = json!(row.get::<_,i64>(15)?);
                item["businessShareMilli"] = json!(row.get::<_,i64>(16)?);
            }
            Ok(item)
        },
    ).map_err(ApiError::internal)?;
    let mut items = Vec::new();
    for row in rows {
        items.push(row.map_err(ApiError::internal)?);
    }
    let settlement = total_for(
        &db.conn,
        "SELECT COALESCE(SUM(price_milli),0)
         FROM wash_operations
         WHERE status='posted' AND is_paid=1 AND COALESCE(paid_at,occurred_at) BETWEEN ?1 AND ?2
               AND (?3=1 OR created_by=?4)",
        params![&from, &to, if can_view_all { 1 } else { 0 }, owner_id],
    )?;
    Ok(ok(json!({"items":items,"settlementMilli":settlement})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PaidStatusInput {
    is_paid: bool,
}

async fn set_wash_paid(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    Json(input): Json<PaidStatusInput>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "operational.write")?;
    let (from, to) = date_range(&query)?;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let operation = db
        .conn
        .query_row(
            "SELECT created_by,status,is_paid FROM wash_operations WHERE id=?1",
            [id.clone()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    if !principal.is_manager() && operation.0 != principal.id {
        return Err(ApiError::forbidden());
    }
    if operation.1 != "posted" {
        return Err(ApiError::bad("لا يمكن تغيير حالة السداد لعملية ملغاة"));
    }
    let desired = if input.is_paid { 1 } else { 0 };
    if operation.2 != desired {
        let changed_at = now();
        let paid_by = if input.is_paid {
            Some(principal.id.clone())
        } else {
            None
        };
        let tx = db.conn.transaction().map_err(ApiError::internal)?;
        tx.execute(
            "UPDATE wash_operations
             SET is_paid=?1,paid_at=?2,paid_by=?3,revision=revision+1,updated_at=?4,updated_by=?5
             WHERE id=?6",
            params![
                desired,
                if input.is_paid {
                    Some(changed_at.clone())
                } else {
                    None::<String>
                },
                paid_by,
                changed_at,
                &principal.id,
                &id,
            ],
        )
        .map_err(ApiError::internal)?;
        let (action, description) = if input.is_paid {
            ("WASH_MARKED_PAID", "تم تعليم عملية الغسيل كسيارة خالصة")
        } else {
            ("WASH_MARKED_UNPAID", "تم إرجاع عملية الغسيل إلى غير خالصة")
        };
        insert_audit_tx(
            &tx,
            Some(&principal.id),
            action,
            "wash",
            Some(&id),
            description,
            Some(&json!({"isPaid":input.is_paid})),
        )?;
        tx.commit().map_err(ApiError::internal)?;
    }
    let wash = wash_item_by_id(&db.conn, &id, principal.has_permission("financial.manage"))?;
    let settlement = total_for(
        &db.conn,
        "SELECT COALESCE(SUM(price_milli),0)
         FROM wash_operations
         WHERE status='posted' AND is_paid=1 AND COALESCE(paid_at,occurred_at) BETWEEN ?1 AND ?2
               AND (?3=1 OR created_by=?4)",
        params![
            from,
            to,
            if principal.is_manager() { 1 } else { 0 },
            &principal.id
        ],
    )?;
    Ok(ok(
        json!({"updated":operation.2 != desired,"wash":wash,"settlementMilli":settlement}),
    ))
}

fn wash_item_by_id(
    conn: &Connection,
    id: &str,
    include_financials: bool,
) -> Result<Value, ApiError> {
    conn.query_row(
        "SELECT w.id,w.vehicle_make,w.vehicle_model,w.manufacture_year,w.license_plate,w.car_color,w.price_milli,
                w.occurred_at,w.payment_type,w.status,worker.id,worker.full_name,showroom.id,showroom.name,
                w.commission_bps,w.commission_milli,w.business_share_milli,w.showroom_payment_method,
                EXISTS(SELECT 1 FROM overnight_cars overnight WHERE overnight.wash_id=w.id),w.is_paid,w.paid_at,
                creator.id,creator.full_name
         FROM wash_operations w JOIN workers worker ON worker.id=w.worker_id
         LEFT JOIN showrooms showroom ON showroom.id=w.showroom_id
         JOIN users creator ON creator.id=w.created_by WHERE w.id=?1",
        [id],
        |row| {
            let mut item = json!({
                "id":row.get::<_,String>(0)?,"vehicleMake":row.get::<_,String>(1)?,"vehicleModel":row.get::<_,String>(2)?,
                "manufactureYear":row.get::<_,Option<i32>>(3)?,"licensePlate":row.get::<_,Option<String>>(4)?,"carColor":row.get::<_,Option<String>>(5)?,
                "occurredAt":row.get::<_,String>(7)?,"paymentType":row.get::<_,String>(8)?,"status":row.get::<_,String>(9)?,
                "worker":{"id":row.get::<_,String>(10)?,"fullName":row.get::<_,String>(11)?},
                "showroom":row.get::<_,Option<String>>(12)?.map(|showroom_id|json!({"id":showroom_id,"name":row.get::<_,Option<String>>(13).ok().flatten()})),
                "showroomPaymentMethod":row.get::<_,Option<String>>(17)?,"isOvernight":row.get::<_,i64>(18)?==1,
                "isPaid":row.get::<_,i64>(19)?==1,"paidAt":row.get::<_,Option<String>>(20)?,
                "createdBy":{"id":row.get::<_,String>(21)?,"fullName":row.get::<_,String>(22)?}
            });
            item["priceMilli"] = json!(row.get::<_,i64>(6)?);
            if include_financials {
                item["commissionBps"] = json!(row.get::<_,i64>(14)?);
                item["commissionMilli"] = json!(row.get::<_,i64>(15)?);
                item["businessShareMilli"] = json!(row.get::<_,i64>(16)?);
            }
            Ok(item)
        },
    ).optional().map_err(ApiError::internal)?.ok_or_else(ApiError::not_found)
}

fn add_financial_transaction(
    tx: &Transaction<'_>,
    source_type: &str,
    source_id: &str,
    occurred_at: &str,
    created_by: &str,
    entries: &[(&str, &str, i64, Option<&str>, Option<&str>)],
) -> Result<(), ApiError> {
    let transaction_id = new_id();
    tx.execute(
        "INSERT INTO financial_transactions(id, source_type, source_id, occurred_at, created_by, created_at) VALUES(?1,?2,?3,?4,?5,?6)",
        params![transaction_id, source_type, source_id, occurred_at, created_by, now()],
    ).map_err(ApiError::internal)?;
    for (account, side, amount, worker_id, showroom_id) in entries {
        if *amount == 0 {
            continue;
        }
        tx.execute(
            "INSERT INTO ledger_entries(id, transaction_id, account_code, entry_side, amount_milli, related_worker_id, related_showroom_id, created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![new_id(), transaction_id, account, side, amount, worker_id, showroom_id, now()],
        ).map_err(ApiError::internal)?;
    }
    Ok(())
}

async fn create_wash(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<WashInput>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "operational.write")?;
    let vehicle_make = trim_required(&input.vehicle_make, "اسم الشركة المصنعة")?;
    let vehicle_model = trim_required(&input.vehicle_model, "طراز المركبة")?;
    let car_color = input
        .car_color
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    if car_color
        .as_deref()
        .is_some_and(|value| value.chars().count() > 60)
    {
        return Err(ApiError::bad("لون السيارة طويل جدًا"));
    }
    let price = parse_milli(&input.price)?;
    if let Some(year) = input.manufacture_year {
        if !(1900..=2100).contains(&year) {
            return Err(ApiError::bad("سنة الصنع غير صالحة"));
        }
    }
    if !matches!(input.payment_type.as_str(), "cash" | "showroom") {
        return Err(ApiError::bad("طريقة الدفع غير صالحة"));
    }
    let showroom_id = input
        .showroom_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let showroom_payment_method = input
        .showroom_payment_method
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    if input.payment_type == "showroom" && showroom_id.is_none() {
        return Err(ApiError::bad("اختر معرضًا لحساب المعرض"));
    }
    if input.payment_type == "cash" && showroom_id.is_some() {
        return Err(ApiError::bad("لا يمكن إرفاق معرض بعملية نقدية"));
    }
    if input.payment_type == "showroom"
        && !matches!(
            showroom_payment_method.as_deref(),
            Some("cash") | Some("bank")
        )
    {
        return Err(ApiError::bad("اختر طريقة دفع المعرض: نقدي أو مصرفي"));
    }
    if input.payment_type == "cash" && showroom_payment_method.is_some() {
        return Err(ApiError::bad("لا يمكن حفظ طريقة دفع معرض لزبون عادي"));
    }
    let occurred_at = input
        .occurred_at
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(now);
    DateTime::parse_from_rfc3339(&occurred_at).map_err(|_| ApiError::bad("وقت الغسلة غير صالح"))?;
    let request_id = input.client_request_id.unwrap_or_else(new_id);
    if request_id.len() > 100 {
        return Err(ApiError::bad("معرف الطلب غير صالح"));
    }
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    if let Some(id) = db
        .conn
        .query_row(
            "SELECT id FROM wash_operations WHERE client_request_id=?1",
            [request_id.clone()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(ApiError::internal)?
    {
        let wash = wash_item_by_id(&db.conn, &id, principal.has_permission("financial.manage"))?;
        return Ok(ok(json!({"id": id, "duplicate": true, "wash": wash})));
    }
    let worker: Option<(i64, Option<i64>)> = db
        .conn
        .query_row(
            "SELECT is_active, commission_bps_override FROM workers WHERE id=?1",
            [input.worker_id.clone()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(ApiError::internal)?;
    let (active, override_bps) = worker.ok_or_else(ApiError::not_found)?;
    if active != 1 {
        return Err(ApiError::bad("العامل المحدد غير نشط"));
    }
    if let Some(showroom_id) = &showroom_id {
        let showroom_active: Option<i64> = db
            .conn
            .query_row(
                "SELECT is_active FROM showrooms WHERE id=?1",
                [showroom_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(ApiError::internal)?;
        if showroom_active != Some(1) {
            return Err(ApiError::bad("المعرض المحدد غير متاح"));
        }
    }
    let commission_bps =
        override_bps.unwrap_or(default_commission_bps(&db.conn).map_err(ApiError::internal)?);
    let commission = round_percentage(price, commission_bps);
    let business_share = price - commission;
    let wash_id = new_id();
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    tx.execute(
        "INSERT INTO wash_operations(id,vehicle_make,vehicle_model,manufacture_year,license_plate,car_color,price_milli,worker_id,payment_type,showroom_id,showroom_payment_method,occurred_at,commission_bps,commission_milli,business_share_milli,created_by,client_request_id,created_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)",
        params![wash_id, vehicle_make, vehicle_model, input.manufacture_year, input.license_plate.map(|value| value.trim().to_owned()).filter(|value| !value.is_empty()), car_color, price, input.worker_id, input.payment_type, showroom_id, showroom_payment_method, occurred_at, commission_bps, commission, business_share, principal.id, request_id, now()],
    ).map_err(ApiError::internal)?;
    let debit_account = if input.payment_type == "cash" {
        "CASH"
    } else {
        "SHOWROOM_RECEIVABLE"
    };
    add_financial_transaction(
        &tx,
        "wash",
        &wash_id,
        &occurred_at,
        &principal.id,
        &[
            (debit_account, "debit", price, None, showroom_id.as_deref()),
            (
                "WASH_REVENUE",
                "credit",
                price,
                None,
                showroom_id.as_deref(),
            ),
            (
                "WORKER_COMMISSION_EXPENSE",
                "debit",
                commission,
                Some(&input.worker_id),
                None,
            ),
            (
                "WORKER_PAYABLE",
                "credit",
                commission,
                Some(&input.worker_id),
                None,
            ),
        ],
    )?;
    insert_audit_tx(
        &tx,
        Some(&principal.id),
        "WASH_CREATED",
        "wash",
        Some(&wash_id),
        "تم تسجيل عملية غسيل",
        Some(&json!({"paymentType": input.payment_type, "workerId": input.worker_id})),
    )?;
    tx.commit().map_err(ApiError::internal)?;
    let wash = wash_item_by_id(
        &db.conn,
        &wash_id,
        principal.has_permission("financial.manage"),
    )?;
    Ok(ok(json!({"id": wash_id, "duplicate": false, "wash": wash})))
}

async fn update_wash(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<WashInput>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "operational.write")?;
    let vehicle_make = trim_required(&input.vehicle_make, "اسم الشركة المصنعة")?;
    let vehicle_model = trim_required(&input.vehicle_model, "طراز المركبة")?;
    let car_color = input
        .car_color
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    if car_color
        .as_deref()
        .is_some_and(|value| value.chars().count() > 60)
    {
        return Err(ApiError::bad("لون السيارة طويل جدًا"));
    }
    if input
        .manufacture_year
        .is_some_and(|year| !(1900..=2100).contains(&year))
    {
        return Err(ApiError::bad("سنة الصنع غير صالحة"));
    }
    if !matches!(input.payment_type.as_str(), "cash" | "showroom") {
        return Err(ApiError::bad("طريقة الدفع غير صالحة"));
    }
    let showroom_id = input
        .showroom_id
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned);
    if (input.payment_type == "showroom") != showroom_id.is_some() {
        return Err(ApiError::bad("اختر معرضًا فقط عند استخدام حساب المعرض"));
    }
    let showroom_payment_method = input
        .showroom_payment_method
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    if input.payment_type == "showroom"
        && !matches!(
            showroom_payment_method.as_deref(),
            Some("cash") | Some("bank")
        )
    {
        return Err(ApiError::bad("اختر طريقة دفع المعرض: نقدي أو مصرفي"));
    }
    if input.payment_type == "cash" && showroom_payment_method.is_some() {
        return Err(ApiError::bad("لا يمكن حفظ طريقة دفع معرض لزبون عادي"));
    }
    let occurred_at = input
        .occurred_at
        .as_deref()
        .filter(|v| !v.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(now);
    DateTime::parse_from_rfc3339(&occurred_at).map_err(|_| ApiError::bad("وقت الغسلة غير صالح"))?;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let old = db.conn.query_row(
        "SELECT price_milli,commission_milli,worker_id,payment_type,showroom_id,occurred_at,status,revision FROM wash_operations WHERE id=?1",
        [id.clone()], |row| Ok((row.get::<_,i64>(0)?,row.get::<_,i64>(1)?,row.get::<_,String>(2)?,row.get::<_,String>(3)?,row.get::<_,Option<String>>(4)?,row.get::<_,String>(5)?,row.get::<_,String>(6)?,row.get::<_,i64>(7)?)),
    ).optional().map_err(ApiError::internal)?.ok_or_else(ApiError::not_found)?;
    let price = if input.price.trim().is_empty() {
        old.0
    } else {
        parse_milli(&input.price)?
    };
    let created_by: String = db
        .conn
        .query_row(
            "SELECT created_by FROM wash_operations WHERE id=?1",
            [id.clone()],
            |row| row.get(0),
        )
        .map_err(ApiError::internal)?;
    if !principal.is_manager() && created_by != principal.id {
        return Err(ApiError::forbidden());
    }
    if old.6 != "posted" {
        return Err(ApiError::bad("لا يمكن تعديل عملية ملغاة"));
    }
    let worker = db
        .conn
        .query_row(
            "SELECT is_active,commission_bps_override FROM workers WHERE id=?1",
            [input.worker_id.clone()],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?)),
        )
        .optional()
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    if worker.0 != 1 && input.worker_id != old.2 {
        return Err(ApiError::bad("العامل المحدد غير نشط"));
    }
    if let Some(showroom) = &showroom_id {
        let active = db
            .conn
            .query_row(
                "SELECT is_active FROM showrooms WHERE id=?1",
                [showroom],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(ApiError::internal)?;
        if active != Some(1) {
            return Err(ApiError::bad("المعرض المحدد غير متاح"));
        }
    }
    let commission_bps = worker
        .1
        .unwrap_or(default_commission_bps(&db.conn).map_err(ApiError::internal)?);
    let commission = round_percentage(price, commission_bps);
    let business_share = price - commission;
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    let old_account = if old.3 == "cash" {
        "CASH"
    } else {
        "SHOWROOM_RECEIVABLE"
    };
    add_financial_transaction(
        &tx,
        &format!("wash_edit_reverse_{}", old.7),
        &id,
        &old.5,
        &principal.id,
        &[
            ("WASH_REVENUE", "debit", old.0, None, old.4.as_deref()),
            (old_account, "credit", old.0, None, old.4.as_deref()),
            ("WORKER_PAYABLE", "debit", old.1, Some(&old.2), None),
            (
                "WORKER_COMMISSION_EXPENSE",
                "credit",
                old.1,
                Some(&old.2),
                None,
            ),
        ],
    )?;
    let new_account = if input.payment_type == "cash" {
        "CASH"
    } else {
        "SHOWROOM_RECEIVABLE"
    };
    add_financial_transaction(
        &tx,
        &format!("wash_edit_post_{}", old.7),
        &id,
        &occurred_at,
        &principal.id,
        &[
            (new_account, "debit", price, None, showroom_id.as_deref()),
            (
                "WASH_REVENUE",
                "credit",
                price,
                None,
                showroom_id.as_deref(),
            ),
            (
                "WORKER_COMMISSION_EXPENSE",
                "debit",
                commission,
                Some(&input.worker_id),
                None,
            ),
            (
                "WORKER_PAYABLE",
                "credit",
                commission,
                Some(&input.worker_id),
                None,
            ),
        ],
    )?;
    tx.execute("UPDATE wash_operations SET vehicle_make=?1,vehicle_model=?2,manufacture_year=?3,license_plate=?4,car_color=?5,price_milli=?6,worker_id=?7,payment_type=?8,showroom_id=?9,showroom_payment_method=?10,occurred_at=?11,commission_bps=?12,commission_milli=?13,business_share_milli=?14,revision=revision+1,updated_at=?15,updated_by=?16 WHERE id=?17",
        params![vehicle_make,vehicle_model,input.manufacture_year,input.license_plate.map(|v|v.trim().to_owned()).filter(|v|!v.is_empty()),car_color,price,input.worker_id,input.payment_type,showroom_id,showroom_payment_method,occurred_at,commission_bps,commission,business_share,now(),principal.id,id]).map_err(ApiError::internal)?;
    if input.mark_as_overnight == Some(true) {
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO overnight_cars(id,wash_id,marked_by,marked_at) VALUES(?1,?2,?3,?4)",
            params![new_id(), id, principal.id, now()],
        ).map_err(ApiError::internal)?;
        if inserted == 1 {
            insert_audit_tx(
                &tx,
                Some(&principal.id),
                "OVERNIGHT_CAR_MARKED",
                "overnight_car",
                Some(&id),
                "تم تعليم السيارة كسيارة مبيتة وربطها بعملية الغسيل",
                None,
            )?;
        }
    } else {
        let deleted = tx
            .execute("DELETE FROM overnight_cars WHERE wash_id=?1", [id.clone()])
            .map_err(ApiError::internal)?;
        if deleted == 1 {
            insert_audit_tx(
                &tx,
                Some(&principal.id),
                "OVERNIGHT_CAR_UNMARKED",
                "overnight_car",
                Some(&id),
                "تم إلغاء تعليم السيارة كسيارة مبيتة مع الاحتفاظ بعملية الغسيل",
                None,
            )?;
        }
    }
    insert_audit_tx(
        &tx,
        Some(&principal.id),
        "WASH_UPDATED",
        "wash",
        Some(&id),
        "تم تعديل عملية الغسيل وإعادة احتساب آثارها المالية",
        None,
    )?;
    tx.commit().map_err(ApiError::internal)?;
    let wash = wash_item_by_id(&db.conn, &id, principal.has_permission("financial.manage"))?;
    Ok(ok(json!({"updated":true,"id":id,"wash":wash})))
}

async fn list_overnight_cars(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "operational.read")?;
    let (from, to) = date_range(&query)?;
    let can_view_all = principal.is_manager();
    let owner_id = principal.id.clone();
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let mut statement = db.conn.prepare(
        "SELECT overnight.id,overnight.marked_at,marker.full_name,
                wash.id,wash.vehicle_make,wash.vehicle_model,wash.manufacture_year,wash.license_plate,wash.car_color,wash.price_milli,
                wash.occurred_at,wash.payment_type,wash.status,wash.showroom_payment_method,
                worker.id,worker.full_name,showroom.id,showroom.name,wash.commission_milli
         FROM overnight_cars overnight
         JOIN wash_operations wash ON wash.id=overnight.wash_id
         JOIN workers worker ON worker.id=wash.worker_id
         LEFT JOIN showrooms showroom ON showroom.id=wash.showroom_id
         JOIN users marker ON marker.id=overnight.marked_by
         WHERE wash.status='posted' AND overnight.marked_at BETWEEN ?1 AND ?2
               AND (?3=1 OR wash.created_by=?4)
         ORDER BY overnight.marked_at DESC"
    ).map_err(ApiError::internal)?;
    let rows = statement.query_map(params![from, to, if can_view_all { 1 } else { 0 }, owner_id], move |row| {
        let mut wash = json!({
            "id": row.get::<_,String>(3)?, "vehicleMake": row.get::<_,String>(4)?, "vehicleModel": row.get::<_,String>(5)?,
            "manufactureYear": row.get::<_,Option<i32>>(6)?, "licensePlate": row.get::<_,Option<String>>(7)?, "carColor": row.get::<_,Option<String>>(8)?, "priceMilli": row.get::<_,i64>(9)?,
            "occurredAt": row.get::<_,String>(10)?, "paymentType": row.get::<_,String>(11)?, "status": row.get::<_,String>(12)?,
            "showroomPaymentMethod": row.get::<_,Option<String>>(13)?,
            "worker": {"id": row.get::<_,String>(14)?, "fullName": row.get::<_,String>(15)?},
            "showroom": row.get::<_,Option<String>>(16)?.map(|id| json!({"id":id,"name":row.get::<_,Option<String>>(17).ok().flatten()})),
            "isOvernight": true
        });
        if can_view_all {
            wash["commissionMilli"] = json!(row.get::<_,i64>(18)?);
        }
        Ok(json!({"id":row.get::<_,String>(0)?,"markedAt":row.get::<_,String>(1)?,"markedBy":row.get::<_,String>(2)?,"wash":wash}))
    }).map_err(ApiError::internal)?;
    let mut items = Vec::new();
    for row in rows {
        items.push(row.map_err(ApiError::internal)?);
    }
    Ok(ok(json!({"items":items})))
}

async fn delete_overnight_car(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "operational.write")?;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let (wash_id, created_by): (String, String) = db.conn.query_row(
        "SELECT overnight.wash_id,wash.created_by FROM overnight_cars overnight JOIN wash_operations wash ON wash.id=overnight.wash_id WHERE overnight.id=?1",
        [id.clone()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional().map_err(ApiError::internal)?.ok_or_else(ApiError::not_found)?;
    if !principal.is_manager() && created_by != principal.id {
        return Err(ApiError::forbidden());
    }
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    let deleted = tx
        .execute("DELETE FROM overnight_cars WHERE id=?1", [id.clone()])
        .map_err(ApiError::internal)?;
    if deleted == 0 {
        return Err(ApiError::not_found());
    }
    insert_audit_tx(
        &tx,
        Some(&principal.id),
        "OVERNIGHT_CAR_DELETED",
        "overnight_car",
        Some(&id),
        "تم حذف سجل سيارة المبيت مع الاحتفاظ بعملية الغسيل الأصلية",
        Some(&json!({"washId":wash_id})),
    )?;
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(json!({"deleted":true,"id":id,"washId":wash_id})))
}

#[derive(Deserialize)]
struct VoidInput {
    reason: String,
}

async fn void_wash(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<VoidInput>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "operational.write")?;
    let reason = trim_required(&input.reason, "سبب الإلغاء")?;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let wash = db.conn.query_row(
        "SELECT price_milli,commission_milli,worker_id,payment_type,showroom_id,occurred_at,status FROM wash_operations WHERE id=?1", [id.clone()],
        |row| Ok((row.get::<_,i64>(0)?,row.get::<_,i64>(1)?,row.get::<_,String>(2)?,row.get::<_,String>(3)?,row.get::<_,Option<String>>(4)?,row.get::<_,String>(5)?,row.get::<_,String>(6)?)),
    ).optional().map_err(ApiError::internal)?.ok_or_else(ApiError::not_found)?;
    let created_by: String = db
        .conn
        .query_row(
            "SELECT created_by FROM wash_operations WHERE id=?1",
            [id.clone()],
            |row| row.get(0),
        )
        .map_err(ApiError::internal)?;
    if !principal.is_manager() && created_by != principal.id {
        return Err(ApiError::forbidden());
    }
    if wash.6 != "posted" {
        return Err(ApiError::bad("هذه الغسلة ملغاة بالفعل"));
    }
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    tx.execute(
        "UPDATE wash_operations
         SET status='voided',voided_at=?1,void_reason=?2,is_paid=0,paid_at=NULL,paid_by=NULL
         WHERE id=?3",
        params![now(), reason, id],
    )
    .map_err(ApiError::internal)?;
    tx.execute("DELETE FROM overnight_cars WHERE wash_id=?1", [id.clone()])
        .map_err(ApiError::internal)?;
    let account = if wash.3 == "cash" {
        "CASH"
    } else {
        "SHOWROOM_RECEIVABLE"
    };
    add_financial_transaction(
        &tx,
        "wash_void",
        &id,
        &wash.5,
        &principal.id,
        &[
            ("WASH_REVENUE", "debit", wash.0, None, wash.4.as_deref()),
            (account, "credit", wash.0, None, wash.4.as_deref()),
            ("WORKER_PAYABLE", "debit", wash.1, Some(&wash.2), None),
            (
                "WORKER_COMMISSION_EXPENSE",
                "credit",
                wash.1,
                Some(&wash.2),
                None,
            ),
        ],
    )?;
    insert_audit_tx(
        &tx,
        Some(&principal.id),
        "WASH_VOIDED",
        "wash",
        Some(&id),
        "تم إلغاء عملية غسيل مع قيد عكسي",
        None,
    )?;
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(json!({"voided": true})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkerInput {
    full_name: String,
    phone: Option<String>,
    notes: Option<String>,
    is_active: Option<bool>,
    commission_bps_override: Option<i64>,
}

async fn list_workers(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "operational.read")?;
    let (from, to) = date_range(&query)?;
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let mut items = Vec::new();
    let own_worker_id = principal.worker_id.clone().unwrap_or_default();
    let mut statement = db.conn.prepare(
        "SELECT w.id,w.full_name,w.phone,w.notes,w.is_active,w.commission_bps_override,
                COUNT(CASE WHEN wash.status='posted' AND wash.occurred_at BETWEEN ?1 AND ?2 THEN 1 END),
                COALESCE(SUM(CASE WHEN wash.status='posted' AND wash.occurred_at BETWEEN ?1 AND ?2 THEN wash.commission_milli ELSE 0 END),0),
                COALESCE((SELECT SUM(ea.amount_milli) FROM expense_allocations ea JOIN expenses e ON e.id=ea.expense_id WHERE ea.worker_id=w.id AND e.occurred_at BETWEEN ?1 AND ?2),0)
         FROM workers w LEFT JOIN wash_operations wash ON wash.worker_id=w.id
         WHERE w.is_active=1 AND (?3=1 OR w.id=?4)
         GROUP BY w.id ORDER BY w.full_name",
    ).map_err(ApiError::internal)?;
    let rows = statement.query_map(params![from, to, if principal.is_manager() { 1 } else { 0 }, own_worker_id], |row| {
        let gross: i64 = row.get(7)?; let deductions: i64 = row.get(8)?;
        Ok(json!({
            "id":row.get::<_,String>(0)?,"fullName":row.get::<_,String>(1)?,"phone":row.get::<_,Option<String>>(2)?,"notes":row.get::<_,Option<String>>(3)?,
            "isActive":row.get::<_,i64>(4)? == 1,"commissionBpsOverride":row.get::<_,Option<i64>>(5)?,"washCount":row.get::<_,i64>(6)?,
            "financial":{"grossCommissionMilli":gross,"deductionsMilli":deductions,"paidMilli":0,"remainingMilli":(gross-deductions).max(0)}
        }))
    }).map_err(ApiError::internal)?;
    for row in rows {
        items.push(row.map_err(ApiError::internal)?);
    }
    if query.get("status").is_some_and(|value| value == "active") {
        items.retain(|item| item["isActive"] == true);
    }
    Ok(ok(json!({"items":items})))
}

async fn create_worker(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<WorkerInput>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "operational.write")?;
    let full_name = trim_required(&input.full_name, "اسم العامل")?;
    if let Some(bps) = input.commission_bps_override {
        if !(0..=10000).contains(&bps) {
            return Err(ApiError::bad("نسبة العمولة الخاصة غير صالحة"));
        }
    }
    let id = new_id();
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    db.conn.execute(
        "INSERT INTO workers(id,full_name,phone,notes,is_active,commission_bps_override,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?7)",
        params![id, full_name, input.phone.map(|v|v.trim().to_owned()).filter(|v|!v.is_empty()), input.notes.map(|v|v.trim().to_owned()).filter(|v|!v.is_empty()), if input.is_active.unwrap_or(true){1}else{0}, input.commission_bps_override, now()],
    ).map_err(ApiError::internal)?;
    insert_audit(
        &db.conn,
        Some(&principal.id),
        "WORKER_CREATED",
        "worker",
        Some(&id),
        "تم إنشاء ملف عامل",
        None,
    )
    .map_err(ApiError::internal)?;
    Ok(ok(json!({"id":id})))
}

async fn update_worker(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<WorkerInput>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "operational.write")?;
    let full_name = trim_required(&input.full_name, "اسم العامل")?;
    if let Some(bps) = input.commission_bps_override {
        if !(0..=10000).contains(&bps) {
            return Err(ApiError::bad("نسبة العمولة الخاصة غير صالحة"));
        }
    }
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    ensure_worker_access(&principal, &id)?;
    let previous_active = db
        .conn
        .query_row(
            "SELECT is_active FROM workers WHERE id=?1",
            [id.clone()],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    let next_active = input.is_active.unwrap_or(previous_active == 1);
    let affected = db.conn.execute(
        "UPDATE workers SET full_name=?1,phone=?2,notes=?3,is_active=?4,commission_bps_override=?5,updated_at=?6,deactivated_at=CASE WHEN ?4=0 THEN COALESCE(deactivated_at,?6) ELSE NULL END,deactivated_by=CASE WHEN ?4=0 THEN ?8 ELSE NULL END WHERE id=?7",
        params![full_name, input.phone.map(|v|v.trim().to_owned()).filter(|v|!v.is_empty()), input.notes.map(|v|v.trim().to_owned()).filter(|v|!v.is_empty()), if next_active{1}else{0}, input.commission_bps_override, now(), id, principal.id],
    ).map_err(ApiError::internal)?;
    if affected == 0 {
        return Err(ApiError::not_found());
    }
    insert_audit(
        &db.conn,
        Some(&principal.id),
        if previous_active == 1 && !next_active {
            "WORKER_DEACTIVATED"
        } else {
            "WORKER_UPDATED"
        },
        "worker",
        Some(&id),
        if previous_active == 1 && !next_active {
            "تم تعطيل العامل مع الاحتفاظ بكامل سجله التاريخي"
        } else {
            "تم تعديل ملف عامل"
        },
        None,
    )
    .map_err(ApiError::internal)?;
    Ok(ok(json!({"updated":true})))
}

async fn delete_worker(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult {
    let principal = manager(&state, &headers)?;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let worker_name: String = db
        .conn
        .query_row(
            "SELECT full_name FROM workers WHERE id=?1",
            [id.clone()],
            |row| row.get(0),
        )
        .optional()
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    let timestamp = now();
    let affected = db.conn.execute(
        "UPDATE workers SET is_active=0,deactivated_at=COALESCE(deactivated_at,?1),deactivated_by=?2,updated_at=?1 WHERE id=?3",
        params![timestamp, principal.id, id],
    ).map_err(ApiError::internal)?;
    if affected == 0 {
        return Err(ApiError::not_found());
    }
    insert_audit(
        &db.conn,
        Some(&principal.id),
        "WORKER_DELETED_SAFELY",
        "worker",
        Some(&id),
        "تم حذف الموظف من القائمة النشطة مع الاحتفاظ بكل السجلات التاريخية",
        None,
    )
    .map_err(ApiError::internal)?;
    Ok(ok(
        json!({"deleted":true,"archived":true,"worker":{"id":id,"fullName":worker_name}}),
    ))
}

async fn worker_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "operational.read")?;
    let (from, to) = date_range(&query)?;
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    ensure_worker_access(&principal, &id)?;
    let worker: Option<Value> = db.conn.query_row("SELECT id,full_name,phone,notes,is_active FROM workers WHERE id=?1",[id.clone()],|row|Ok(json!({"id":row.get::<_,String>(0)?,"fullName":row.get::<_,String>(1)?,"phone":row.get::<_,Option<String>>(2)?,"notes":row.get::<_,Option<String>>(3)?,"isActive":row.get::<_,i64>(4)?==1}))).optional().map_err(ApiError::internal)?;
    let worker = worker.ok_or_else(ApiError::not_found)?;
    let value_date = selected_business_date(&query)?
        .unwrap_or_else(business_today)
        .format("%Y-%m-%d")
        .to_string();
    let daily_value = db.conn.query_row(
            "SELECT value_date,amount_milli FROM worker_daily_values WHERE worker_id=?1 AND value_date=?2",
            params![id.clone(), value_date.clone()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        ).optional().map_err(ApiError::internal)?;
    let mut history = Vec::new();
    let mut statement = db.conn.prepare(
        "SELECT wash.id,wash.vehicle_make,wash.vehicle_model,wash.manufacture_year,wash.license_plate,wash.price_milli,wash.occurred_at,wash.payment_type,wash.status,worker.id,worker.full_name,creator.id,creator.full_name
         FROM wash_operations wash
         JOIN workers worker ON worker.id=wash.worker_id
         JOIN users creator ON creator.id=wash.created_by
         WHERE wash.worker_id=?1 AND wash.status='posted' AND wash.occurred_at BETWEEN ?2 AND ?3
         ORDER BY wash.occurred_at DESC",
    ).map_err(ApiError::internal)?;
    let rows = statement.query_map(params![id,from,to], |row| {
        let mut value = json!({"id":row.get::<_,String>(0)?,"vehicleMake":row.get::<_,String>(1)?,"vehicleModel":row.get::<_,String>(2)?,"manufactureYear":row.get::<_,Option<i32>>(3)?,"licensePlate":row.get::<_,Option<String>>(4)?,"occurredAt":row.get::<_,String>(6)?,"paymentType":row.get::<_,String>(7)?,"status":row.get::<_,String>(8)?,"worker":{"id":row.get::<_,String>(9)?,"fullName":row.get::<_,String>(10)?},"createdBy":{"id":row.get::<_,String>(11)?,"fullName":row.get::<_,String>(12)?}});
        value["priceMilli"] = json!(row.get::<_,i64>(5)?);
        Ok(value)
    }).map_err(ApiError::internal)?;
    for row in rows {
        history.push(row.map_err(ApiError::internal)?);
    }
    let mut response = json!({"worker":worker,"history":history});
    response["dailyValue"] = daily_value.map_or_else(
        || json!({"date": value_date, "amountMilli": null}),
        |(date, amount)| json!({"date": date, "amountMilli": amount}),
    );
    Ok(ok(response))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkerDailyValueInput {
    value_date: String,
    amount: String,
}

async fn update_worker_daily_value(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(worker_id): Path<String>,
    Json(input): Json<WorkerDailyValueInput>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "worker.daily_value.manage")?;
    let date = NaiveDate::parse_from_str(input.value_date.trim(), "%Y-%m-%d")
        .map_err(|_| ApiError::bad("تاريخ القيمة اليومية غير صالح"))?;
    let value_date = date.format("%Y-%m-%d").to_string();
    let amount = parse_milli(&input.amount)?;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    ensure_worker_access(&principal, &worker_id)?;
    let exists: Option<String> = db
        .conn
        .query_row(
            "SELECT id FROM workers WHERE id=?1",
            [worker_id.clone()],
            |row| row.get(0),
        )
        .optional()
        .map_err(ApiError::internal)?;
    if exists.is_none() {
        return Err(ApiError::not_found());
    }
    let timestamp = now();
    db.conn.execute(
        "INSERT INTO worker_daily_values(worker_id,value_date,amount_milli,set_by,created_at,updated_at)
         VALUES(?1,?2,?3,?4,?5,?5)
         ON CONFLICT(worker_id,value_date) DO UPDATE SET amount_milli=excluded.amount_milli,set_by=excluded.set_by,updated_at=excluded.updated_at",
        params![worker_id, value_date, amount, principal.id, timestamp],
    ).map_err(ApiError::internal)?;
    Ok(ok(
        json!({"workerId": worker_id, "valueDate": date.format("%Y-%m-%d").to_string(), "amountMilli": amount}),
    ))
}

async fn worker_financial(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "operational.read")?;
    let (from, to) = date_range(&query)?;
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    ensure_worker_access(&principal, &id)?;
    let exists: Option<String> = db
        .conn
        .query_row("SELECT id FROM workers WHERE id=?1", [id.clone()], |row| {
            row.get(0)
        })
        .optional()
        .map_err(ApiError::internal)?;
    if exists.is_none() {
        return Err(ApiError::not_found());
    }
    let gross=total_for(&db.conn,"SELECT COALESCE(SUM(commission_milli),0) FROM wash_operations WHERE status='posted' AND worker_id=?1 AND occurred_at BETWEEN ?2 AND ?3",params![id,from,to])?;
    let deductions=total_for(&db.conn,"SELECT COALESCE(SUM(ea.amount_milli),0) FROM expense_allocations ea JOIN expenses e ON e.id=ea.expense_id WHERE ea.worker_id=?1 AND e.occurred_at BETWEEN ?2 AND ?3",params![id,from,to])?;
    Ok(ok(
        json!({"grossCommissionMilli":gross,"deductionsMilli":deductions,"netEarningsMilli":(gross-deductions).max(0),"paidMilli":0,"remainingMilli":(gross-deductions).max(0),"payments":[]}),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkerWithdrawalReturnInput {
    transaction_type: String,
    amount: String,
    occurred_at: String,
    notes: Option<String>,
}

async fn worker_withdrawal_returns(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    manager(&state, &headers)?;
    let (from, to) = date_range(&query)?;
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let worker_name: String = db
        .conn
        .query_row(
            "SELECT full_name FROM workers WHERE id=?1",
            [id.clone()],
            |row| row.get(0),
        )
        .optional()
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    let total_withdrawals = total_for(&db.conn, "SELECT COALESCE(SUM(amount_milli),0) FROM worker_withdrawal_returns WHERE worker_id=?1 AND transaction_type='withdrawal' AND occurred_at BETWEEN ?2 AND ?3", params![id.clone(),from.clone(),to.clone()])?;
    let total_returns = total_for(&db.conn, "SELECT COALESCE(SUM(amount_milli),0) FROM worker_withdrawal_returns WHERE worker_id=?1 AND transaction_type='return' AND occurred_at BETWEEN ?2 AND ?3", params![id.clone(),from.clone(),to.clone()])?;
    let mut transactions = Vec::new();
    let mut statement = db.conn.prepare(
        "SELECT movement.id,movement.transaction_type,movement.amount_milli,movement.occurred_at,movement.notes,user.full_name
         FROM worker_withdrawal_returns movement JOIN users user ON user.id=movement.created_by
         WHERE movement.worker_id=?1 AND movement.occurred_at BETWEEN ?2 AND ?3
         ORDER BY movement.occurred_at DESC,movement.created_at DESC"
    ).map_err(ApiError::internal)?;
    let rows = statement.query_map(params![id.clone(),from,to], |row| Ok(json!({
        "id":row.get::<_,String>(0)?,"type":row.get::<_,String>(1)?,"amountMilli":row.get::<_,i64>(2)?,
        "occurredAt":row.get::<_,String>(3)?,"notes":row.get::<_,Option<String>>(4)?,"createdByName":row.get::<_,String>(5)?
    }))).map_err(ApiError::internal)?;
    for row in rows {
        transactions.push(row.map_err(ApiError::internal)?);
    }
    Ok(ok(json!({
        "worker":{"id":id,"fullName":worker_name},"totalWithdrawalsMilli":total_withdrawals,
        "totalReturnsMilli":total_returns,"outstandingBalanceMilli":total_withdrawals-total_returns,"transactions":transactions
    })))
}

async fn create_worker_withdrawal_return(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(worker_id): Path<String>,
    Json(input): Json<WorkerWithdrawalReturnInput>,
) -> ApiResult {
    let principal = manager(&state, &headers)?;
    if !matches!(input.transaction_type.as_str(), "withdrawal" | "return") {
        return Err(ApiError::bad("نوع الحركة غير صالح"));
    }
    let amount = parse_milli(&input.amount)?;
    DateTime::parse_from_rfc3339(&input.occurred_at)
        .map_err(|_| ApiError::bad("تاريخ الحركة غير صالح"))?;
    let notes = input
        .notes
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    if notes
        .as_deref()
        .is_some_and(|value| value.chars().count() > 500)
    {
        return Err(ApiError::bad("الملاحظة طويلة جدًا"));
    }
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let worker_name: String = db
        .conn
        .query_row(
            "SELECT full_name FROM workers WHERE id=?1",
            [worker_id.clone()],
            |row| row.get(0),
        )
        .optional()
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    let id = new_id();
    let created_at = now();
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    tx.execute(
        "INSERT INTO worker_withdrawal_returns(id,worker_id,transaction_type,amount_milli,occurred_at,notes,created_by,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![id,worker_id,input.transaction_type,amount,input.occurred_at,notes,principal.id,created_at],
    ).map_err(ApiError::internal)?;
    insert_audit_tx(
        &tx,
        Some(&principal.id),
        "WORKER_WITHDRAWAL_RETURN_CREATED",
        "worker_withdrawal_return",
        Some(&id),
        "تم تسجيل حركة مستقلة في سجل مسحوبات ومرتجعات العامل",
        Some(
            &json!({"workerId":worker_id,"workerName":worker_name,"type":input.transaction_type,"amountMilli":amount}),
        ),
    )?;
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(json!({"id":id,"created":true})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShowroomInput {
    name: String,
    contact_name: Option<String>,
    phone: Option<String>,
    notes: Option<String>,
    is_active: Option<bool>,
}

async fn list_showroom_debts(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    let _principal = authorize(&state, &headers, "financial.manage")?;
    let (_, to) = date_range(&query)?;
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let mut statement = db.conn.prepare(
        "SELECT showroom.id,showroom.name,showroom.contact_name,showroom.phone,showroom.notes,showroom.is_active,
                COUNT(wash.id),
                COALESCE(SUM(wash.price_milli),0) - COALESCE((SELECT SUM(payment.amount_milli)
                    FROM showroom_payments payment WHERE payment.showroom_id=showroom.id AND payment.paid_at<=?1),0),
                MAX(wash.occurred_at)
         FROM showrooms showroom
         JOIN wash_operations wash ON wash.showroom_id=showroom.id
             AND wash.payment_type='showroom' AND wash.status='posted' AND wash.occurred_at<=?1
         GROUP BY showroom.id
         HAVING COALESCE(SUM(wash.price_milli),0) - COALESCE((SELECT SUM(payment.amount_milli)
                    FROM showroom_payments payment WHERE payment.showroom_id=showroom.id AND payment.paid_at<=?1),0) > 0
         ORDER BY MAX(wash.occurred_at) DESC,showroom.name",
    ).map_err(ApiError::internal)?;
    let rows = statement
        .query_map([to], |row| {
            Ok(json!({
                "showroom": {
                    "id": row.get::<_,String>(0)?,
                    "name": row.get::<_,String>(1)?,
                    "contactName": row.get::<_,Option<String>>(2)?,
                    "phone": row.get::<_,Option<String>>(3)?,
                    "notes": row.get::<_,Option<String>>(4)?,
                    "isActive": row.get::<_,i64>(5)?==1
                },
                "outstandingWashCount": row.get::<_,i64>(6)?,
                "totalOutstandingMilli": row.get::<_,i64>(7)?,
                "latestWashAt": row.get::<_,Option<String>>(8)?
            }))
        })
        .map_err(ApiError::internal)?;
    let mut items = Vec::new();
    for row in rows {
        items.push(row.map_err(ApiError::internal)?);
    }
    Ok(ok(json!({"items":items})))
}

async fn showroom_debt_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    let _principal = authorize(&state, &headers, "financial.manage")?;
    let (from, to) = date_range(&query)?;
    if from > to {
        return Err(ApiError::bad("تاريخ البداية يجب أن يسبق تاريخ النهاية"));
    }
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let showroom: Option<Value> = db.conn.query_row(
        "SELECT id,name,contact_name,phone,notes,is_active,created_at FROM showrooms WHERE id=?1",
        [id.clone()],
        |row| Ok(json!({
            "id":row.get::<_,String>(0)?,"name":row.get::<_,String>(1)?,
            "contactName":row.get::<_,Option<String>>(2)?,"phone":row.get::<_,Option<String>>(3)?,
            "notes":row.get::<_,Option<String>>(4)?,"isActive":row.get::<_,i64>(5)?==1,
            "createdAt":row.get::<_,String>(6)?
        })),
    ).optional().map_err(ApiError::internal)?;
    let showroom = showroom.ok_or_else(ApiError::not_found)?;
    let mut statement = db.conn.prepare(
        "SELECT wash.id,wash.vehicle_make,wash.vehicle_model,wash.manufacture_year,wash.license_plate,wash.car_color,
                wash.price_milli,wash.occurred_at,wash.showroom_payment_method,wash.created_at,
                worker.id,worker.full_name,creator.full_name
         FROM wash_operations wash
         JOIN workers worker ON worker.id=wash.worker_id
         JOIN users creator ON creator.id=wash.created_by
         WHERE wash.showroom_id=?1 AND wash.payment_type='showroom' AND wash.status='posted'
             AND wash.occurred_at BETWEEN ?2 AND ?3
         ORDER BY wash.occurred_at DESC,wash.id",
    ).map_err(ApiError::internal)?;
    let rows = statement.query_map(params![id,from,to], |row| Ok(json!({
        "id":row.get::<_,String>(0)?,"vehicleMake":row.get::<_,String>(1)?,"vehicleModel":row.get::<_,String>(2)?,
        "manufactureYear":row.get::<_,Option<i32>>(3)?,"licensePlate":row.get::<_,Option<String>>(4)?,
        "carColor":row.get::<_,Option<String>>(5)?,"priceMilli":row.get::<_,i64>(6)?,
        "occurredAt":row.get::<_,String>(7)?,"paymentType":"showroom","status":"posted",
        "showroomPaymentMethod":row.get::<_,Option<String>>(8)?,"createdAt":row.get::<_,String>(9)?,
        "worker":{"id":row.get::<_,String>(10)?,"fullName":row.get::<_,String>(11)?},
        "recordedBy":row.get::<_,String>(12)?
    }))).map_err(ApiError::internal)?;
    let mut operations = Vec::new();
    for row in rows {
        let operation = row.map_err(ApiError::internal)?;
        operations.push(operation);
    }
    let total_charges = total_for(&db.conn,
        "SELECT COALESCE(SUM(price_milli),0) FROM wash_operations
         WHERE showroom_id=?1 AND payment_type='showroom' AND status='posted' AND occurred_at BETWEEN ?2 AND ?3",
        params![id,from,to])?;
    let total_payments = total_for(
        &db.conn,
        "SELECT COALESCE(SUM(amount_milli),0) FROM showroom_payments
         WHERE showroom_id=?1 AND paid_at BETWEEN ?2 AND ?3",
        params![id, from, to],
    )?;
    let mut payments = Vec::new();
    let mut payment_statement = db
        .conn
        .prepare(
            "SELECT payment.id,payment.amount_milli,payment.paid_at,payment.notes,
                showroom.id,showroom.name,user.full_name
         FROM showroom_payments payment
         JOIN showrooms showroom ON showroom.id=payment.showroom_id
         JOIN users user ON user.id=payment.created_by
         WHERE payment.showroom_id=?1 AND payment.paid_at BETWEEN ?2 AND ?3
         ORDER BY payment.paid_at DESC,payment.id DESC",
        )
        .map_err(ApiError::internal)?;
    let payment_rows = payment_statement
        .query_map(params![id, from, to], |row| {
            Ok(json!({
                "id": row.get::<_,String>(0)?, "amountMilli": row.get::<_,i64>(1)?,
                "paidAt": row.get::<_,String>(2)?, "notes": row.get::<_,Option<String>>(3)?,
                "showroom": {"id": row.get::<_,String>(4)?, "name": row.get::<_,String>(5)?},
                "recordedBy": row.get::<_,String>(6)?
            }))
        })
        .map_err(ApiError::internal)?;
    for row in payment_rows {
        payments.push(row.map_err(ApiError::internal)?);
    }
    Ok(ok(json!({
        "showroom":showroom,"from":from,"to":to,
        "outstandingWashCount":operations.len(),
        "totalChargesMilli":total_charges,
        "totalPaymentsMilli":total_payments,
        "totalOutstandingMilli":(total_charges - total_payments).max(0),
        "operations":operations,"payments":payments
    })))
}

async fn list_showrooms(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "operational.read")?;
    let (from, to) = date_range(&query)?;
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let mut items = Vec::new();
    if principal.has_permission("financial.manage") {
        let mut statement=db.conn.prepare("SELECT s.id,s.name,s.contact_name,s.phone,s.notes,s.is_active,COUNT(CASE WHEN w.status='posted' AND w.payment_type='showroom' AND w.occurred_at BETWEEN ?1 AND ?2 THEN 1 END),COALESCE(SUM(CASE WHEN w.status='posted' AND w.payment_type='showroom' AND w.occurred_at BETWEEN ?1 AND ?2 THEN w.price_milli ELSE 0 END),0),COALESCE((SELECT SUM(amount_milli) FROM showroom_payments sp WHERE sp.showroom_id=s.id AND sp.paid_at BETWEEN ?1 AND ?2),0) FROM showrooms s LEFT JOIN wash_operations w ON w.showroom_id=s.id GROUP BY s.id ORDER BY s.is_active DESC,s.name").map_err(ApiError::internal)?;
        let rows=statement.query_map(params![from,to],|row|{let charges:i64=row.get(7)?;let payments:i64=row.get(8)?;Ok(json!({"id":row.get::<_,String>(0)?,"name":row.get::<_,String>(1)?,"contactName":row.get::<_,Option<String>>(2)?,"phone":row.get::<_,Option<String>>(3)?,"notes":row.get::<_,Option<String>>(4)?,"isActive":row.get::<_,i64>(5)?==1,"washCount":row.get::<_,i64>(6)?,"financial":{"chargesMilli":charges,"paymentsMilli":payments,"outstandingMilli":(charges-payments).max(0)}}))}).map_err(ApiError::internal)?;
        for row in rows {
            items.push(row.map_err(ApiError::internal)?);
        }
    } else {
        let mut statement=db.conn.prepare("SELECT s.id,s.name,s.contact_name,s.phone,s.notes,s.is_active,COUNT(CASE WHEN w.status='posted' AND w.payment_type='showroom' AND w.occurred_at BETWEEN ?1 AND ?2 THEN 1 END) FROM showrooms s LEFT JOIN wash_operations w ON w.showroom_id=s.id GROUP BY s.id ORDER BY s.is_active DESC,s.name").map_err(ApiError::internal)?;
        let rows=statement.query_map(params![from,to],|row|Ok(json!({"id":row.get::<_,String>(0)?,"name":row.get::<_,String>(1)?,"contactName":row.get::<_,Option<String>>(2)?,"phone":row.get::<_,Option<String>>(3)?,"notes":row.get::<_,Option<String>>(4)?,"isActive":row.get::<_,i64>(5)?==1,"washCount":row.get::<_,i64>(6)?}))).map_err(ApiError::internal)?;
        for row in rows {
            items.push(row.map_err(ApiError::internal)?);
        }
    }
    Ok(ok(json!({"items":items})))
}

async fn create_showroom(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<ShowroomInput>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "operational.write")?;
    let name = trim_required(&input.name, "اسم المعرض")?;
    let id = new_id();
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    db.conn.execute("INSERT INTO showrooms(id,name,contact_name,phone,notes,is_active,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?7)",params![id,name,input.contact_name.map(|v|v.trim().to_owned()).filter(|v|!v.is_empty()),input.phone.map(|v|v.trim().to_owned()).filter(|v|!v.is_empty()),input.notes.map(|v|v.trim().to_owned()).filter(|v|!v.is_empty()),if input.is_active.unwrap_or(true){1}else{0},now()]).map_err(|error|ApiError::new(StatusCode::CONFLICT,format!("تعذر إنشاء المعرض: {error}")))?;
    insert_audit(
        &db.conn,
        Some(&principal.id),
        "SHOWROOM_CREATED",
        "showroom",
        Some(&id),
        "تم إنشاء ملف معرض",
        None,
    )
    .map_err(ApiError::internal)?;
    Ok(ok(json!({"id":id})))
}

async fn update_showroom(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<ShowroomInput>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "operational.write")?;
    let name = trim_required(&input.name, "اسم المعرض")?;
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let previous_active: i64 = db
        .conn
        .query_row(
            "SELECT is_active FROM showrooms WHERE id=?1",
            params![id],
            |row| row.get(0),
        )
        .optional()
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    let is_active = input
        .is_active
        .map(|value| if value { 1 } else { 0 })
        .unwrap_or(previous_active);
    let count=db.conn.execute("UPDATE showrooms SET name=?1,contact_name=?2,phone=?3,notes=?4,is_active=?5,updated_at=?6 WHERE id=?7",params![name,input.contact_name.map(|v|v.trim().to_owned()).filter(|v|!v.is_empty()),input.phone.map(|v|v.trim().to_owned()).filter(|v|!v.is_empty()),input.notes.map(|v|v.trim().to_owned()).filter(|v|!v.is_empty()),is_active,now(),id]).map_err(ApiError::internal)?;
    if count == 0 {
        return Err(ApiError::not_found());
    }
    insert_audit(
        &db.conn,
        Some(&principal.id),
        "SHOWROOM_UPDATED",
        "showroom",
        Some(&id),
        "تم تعديل ملف معرض",
        None,
    )
    .map_err(ApiError::internal)?;
    Ok(ok(json!({"updated":true})))
}

async fn delete_showroom(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "operational.write")?;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let exists: bool = db
        .conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM showrooms WHERE id=?1)",
            [id.clone()],
            |row| row.get(0),
        )
        .map_err(ApiError::internal)?;
    if !exists {
        return Err(ApiError::not_found());
    }
    let wash_count: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM wash_operations WHERE showroom_id=?1",
            [id.clone()],
            |row| row.get(0),
        )
        .map_err(ApiError::internal)?;
    let payment_count: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM showroom_payments WHERE showroom_id=?1",
            [id.clone()],
            |row| row.get(0),
        )
        .map_err(ApiError::internal)?;
    if wash_count > 0 || payment_count > 0 {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "لا يمكن حذف معرض مرتبط بعمليات أو دفعات محفوظة",
        ));
    }
    db.conn
        .execute("DELETE FROM showrooms WHERE id=?1", [id.clone()])
        .map_err(ApiError::internal)?;
    insert_audit(
        &db.conn,
        Some(&principal.id),
        "SHOWROOM_DELETED",
        "showroom",
        Some(&id),
        "تم حذف المعرض نهائيًا",
        None,
    )
    .map_err(ApiError::internal)?;
    Ok(ok(json!({"deleted":true,"id":id})))
}

async fn showroom_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "operational.read")?;
    let (from, to) = date_range(&query)?;
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let showroom:Option<Value>=db.conn.query_row("SELECT id,name,contact_name,phone,notes,is_active FROM showrooms WHERE id=?1",[id.clone()],|row|Ok(json!({"id":row.get::<_,String>(0)?,"name":row.get::<_,String>(1)?,"contactName":row.get::<_,Option<String>>(2)?,"phone":row.get::<_,Option<String>>(3)?,"notes":row.get::<_,Option<String>>(4)?,"isActive":row.get::<_,i64>(5)?==1}))).optional().map_err(ApiError::internal)?;
    let showroom = showroom.ok_or_else(ApiError::not_found)?;
    let mut history = Vec::new();
    let can_view_all = principal.is_manager();
    let owner_id = principal.id.clone();
    let mut statement=db.conn.prepare("SELECT w.id,w.vehicle_make,w.vehicle_model,w.manufacture_year,w.license_plate,w.price_milli,w.occurred_at,w.status,worker.full_name FROM wash_operations w JOIN workers worker ON worker.id=w.worker_id WHERE w.showroom_id=?1 AND w.occurred_at BETWEEN ?2 AND ?3 AND (?4=1 OR w.created_by=?5) ORDER BY w.occurred_at DESC").map_err(ApiError::internal)?;
    let rows=statement.query_map(params![id,from,to,if can_view_all { 1 } else { 0 },owner_id],|row|{let mut value=json!({"id":row.get::<_,String>(0)?,"vehicleMake":row.get::<_,String>(1)?,"vehicleModel":row.get::<_,String>(2)?,"manufactureYear":row.get::<_,Option<i32>>(3)?,"licensePlate":row.get::<_,Option<String>>(4)?,"occurredAt":row.get::<_,String>(6)?,"status":row.get::<_,String>(7)?,"workerName":row.get::<_,String>(8)?});if principal.has_permission("financial.manage"){value["priceMilli"]=json!(row.get::<_,i64>(5)?);}Ok(value)}).map_err(ApiError::internal)?;
    for row in rows {
        history.push(row.map_err(ApiError::internal)?);
    }
    Ok(ok(json!({"showroom":showroom,"history":history})))
}

async fn showroom_statistics(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "operational.read")?;
    let (from, to) = date_range(&query)?;
    if from > to {
        return Err(ApiError::bad("تاريخ البداية يجب أن يسبق تاريخ النهاية"));
    }
    let payment = match query
        .get("paymentType")
        .map(String::as_str)
        .unwrap_or("all")
    {
        "all" => None,
        "cash" => Some("cash"),
        "debt" | "showroom" => Some("showroom"),
        _ => return Err(ApiError::bad("نوع الدفع المحدد غير صالح")),
    };
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let exists: bool = db
        .conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM showrooms WHERE id=?1)",
            [id.clone()],
            |row| row.get(0),
        )
        .map_err(ApiError::internal)?;
    if !exists {
        return Err(ApiError::not_found());
    }
    let count = if let Some(payment_type) = payment {
        total_for(&db.conn, "SELECT COUNT(*) FROM wash_operations WHERE showroom_id=?1 AND status='posted' AND occurred_at BETWEEN ?2 AND ?3 AND payment_type=?4 AND (?5=1 OR created_by=?6)", params![id,from,to,payment_type,if principal.is_manager() { 1 } else { 0 },principal.id.clone()])?
    } else {
        total_for(&db.conn, "SELECT COUNT(*) FROM wash_operations WHERE showroom_id=?1 AND status='posted' AND occurred_at BETWEEN ?2 AND ?3 AND (?4=1 OR created_by=?5)", params![id,from,to,if principal.is_manager() { 1 } else { 0 },principal.id.clone()])?
    };
    Ok(ok(
        json!({"carCount":count,"paymentType":query.get("paymentType").map(String::as_str).unwrap_or("all"),"from":from,"to":to}),
    ))
}

async fn showroom_financial(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    let _principal = authorize(&state, &headers, "financial.manage")?;
    let (from, to) = date_range(&query)?;
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let exists: Option<String> = db
        .conn
        .query_row(
            "SELECT id FROM showrooms WHERE id=?1",
            [id.clone()],
            |row| row.get(0),
        )
        .optional()
        .map_err(ApiError::internal)?;
    if exists.is_none() {
        return Err(ApiError::not_found());
    }
    let charges=total_for(&db.conn,"SELECT COALESCE(SUM(price_milli),0) FROM wash_operations WHERE status='posted' AND showroom_id=?1 AND occurred_at BETWEEN ?2 AND ?3",params![id,from,to])?;
    let paid=total_for(&db.conn,"SELECT COALESCE(SUM(amount_milli),0) FROM showroom_payments WHERE showroom_id=?1 AND paid_at BETWEEN ?2 AND ?3",params![id,from,to])?;
    let mut payments = Vec::new();
    let mut statement=db.conn.prepare("SELECT sp.id,sp.amount_milli,sp.paid_at,sp.notes,s.id,s.name,u.full_name FROM showroom_payments sp JOIN showrooms s ON s.id=sp.showroom_id JOIN users u ON u.id=sp.created_by WHERE sp.showroom_id=?1 AND sp.paid_at BETWEEN ?2 AND ?3 ORDER BY sp.paid_at DESC").map_err(ApiError::internal)?;
    let rows=statement.query_map(params![id,from,to],|row|Ok(json!({"id":row.get::<_,String>(0)?,"amountMilli":row.get::<_,i64>(1)?,"paidAt":row.get::<_,String>(2)?,"notes":row.get::<_,Option<String>>(3)?,"showroom":{"id":row.get::<_,String>(4)?,"name":row.get::<_,String>(5)?},"recordedBy":row.get::<_,String>(6)?}))).map_err(ApiError::internal)?;
    for row in rows {
        payments.push(row.map_err(ApiError::internal)?);
    }
    Ok(ok(
        json!({"chargesMilli":charges,"paymentsMilli":paid,"outstandingMilli":(charges-paid).max(0),"payments":payments}),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PaymentInput {
    showroom_id: Option<String>,
    amount: String,
    paid_at: Option<String>,
    notes: Option<String>,
}

fn payment_time(value: &Option<String>) -> Result<String, ApiError> {
    let value = value
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(now);
    DateTime::parse_from_rfc3339(&value).map_err(|_| ApiError::bad("تاريخ الدفع غير صالح"))?;
    Ok(value)
}

fn showroom_payment_item_by_id(conn: &Connection, id: &str) -> Result<Value, ApiError> {
    conn.query_row(
        "SELECT payment.id,payment.amount_milli,payment.paid_at,payment.notes,
                showroom.id,showroom.name,user.full_name
         FROM showroom_payments payment
         JOIN showrooms showroom ON showroom.id=payment.showroom_id
         JOIN users user ON user.id=payment.created_by
         WHERE payment.id=?1",
        [id],
        |row| {
            Ok(json!({
                "id": row.get::<_,String>(0)?, "amountMilli": row.get::<_,i64>(1)?,
                "paidAt": row.get::<_,String>(2)?, "notes": row.get::<_,Option<String>>(3)?,
                "showroom": {"id": row.get::<_,String>(4)?, "name": row.get::<_,String>(5)?},
                "recordedBy": row.get::<_,String>(6)?
            }))
        },
    )
    .optional()
    .map_err(ApiError::internal)?
    .ok_or_else(ApiError::not_found)
}

async fn update_showroom_payment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<PaymentInput>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "financial.manage")?;
    let showroom_id = input
        .showroom_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::bad("اختر المعرض"))?
        .to_owned();
    let amount = parse_milli(&input.amount)?;
    let paid_at = payment_time(&input.paid_at)?;
    let notes = input
        .notes
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let old = db
        .conn
        .query_row(
            "SELECT showroom_id,amount_milli,paid_at FROM showroom_payments WHERE id=?1",
            [id.clone()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    let showroom_exists: bool = db
        .conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM showrooms WHERE id=?1)",
            [showroom_id.clone()],
            |row| row.get(0),
        )
        .map_err(ApiError::internal)?;
    if !showroom_exists {
        return Err(ApiError::not_found());
    }
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    add_financial_transaction(
        &tx,
        &format!("showroom_payment_edit_reverse_{id}"),
        &id,
        &old.2,
        &principal.id,
        &[
            ("SHOWROOM_RECEIVABLE", "debit", old.1, None, Some(&old.0)),
            ("CASH", "credit", old.1, None, None),
        ],
    )?;
    add_financial_transaction(
        &tx,
        &format!("showroom_payment_edit_post_{id}"),
        &id,
        &paid_at,
        &principal.id,
        &[
            ("CASH", "debit", amount, None, None),
            (
                "SHOWROOM_RECEIVABLE",
                "credit",
                amount,
                None,
                Some(&showroom_id),
            ),
        ],
    )?;
    tx.execute(
        "UPDATE showroom_payments SET showroom_id=?1,amount_milli=?2,paid_at=?3,notes=?4 WHERE id=?5",
        params![showroom_id, amount, paid_at, notes, id],
    ).map_err(ApiError::internal)?;
    insert_audit_tx(
        &tx,
        Some(&principal.id),
        "SHOWROOM_PAYMENT_UPDATED",
        "showroom_payment",
        Some(&id),
        "تم تعديل دفعة معرض وإعادة احتساب الرصيد",
        None,
    )?;
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(showroom_payment_item_by_id(&db.conn, &id)?))
}

async fn delete_showroom_payment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "financial.manage")?;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let old = db
        .conn
        .query_row(
            "SELECT showroom_id,amount_milli,paid_at FROM showroom_payments WHERE id=?1",
            [id.clone()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    add_financial_transaction(
        &tx,
        &format!("showroom_payment_delete_reverse_{id}"),
        &id,
        &old.2,
        &principal.id,
        &[
            ("SHOWROOM_RECEIVABLE", "debit", old.1, None, Some(&old.0)),
            ("CASH", "credit", old.1, None, None),
        ],
    )?;
    tx.execute("DELETE FROM showroom_payments WHERE id=?1", [id.clone()])
        .map_err(ApiError::internal)?;
    insert_audit_tx(
        &tx,
        Some(&principal.id),
        "SHOWROOM_PAYMENT_DELETED",
        "showroom_payment",
        Some(&id),
        "تم حذف دفعة معرض وعكس أثرها على الرصيد",
        None,
    )?;
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(json!({"deleted":true,"id":id})))
}

fn parse_payroll_month(value: Option<&String>) -> Result<String, ApiError> {
    let month = value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| Utc::now().format("%Y-%m").to_string());
    NaiveDate::parse_from_str(&format!("{month}-01"), "%Y-%m-%d")
        .map_err(|_| ApiError::bad("الشهر المحدد غير صالح"))?;
    Ok(month)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SalaryInput {
    month: String,
    salary: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PayrollEmployeeInput {
    full_name: String,
    month: String,
    salary: String,
}

async fn payroll_summary(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    let _principal = authorize(&state, &headers, "financial.manage")?;
    let selected_date = selected_business_date(&query)?;
    let month = match selected_date {
        Some(date) => date.format("%Y-%m").to_string(),
        None => parse_payroll_month(query.get("month"))?,
    };
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let mut employees = Vec::new();
    let mut total_salary = 0_i64;
    let mut total_withdrawals = 0_i64;
    let mut total_deductions = 0_i64;
    let mut statement = db
        .conn
        .prepare(
            "SELECT w.id,w.full_name,w.is_active,
                COALESCE((SELECT rate.salary_milli FROM worker_salary_rates rate
                          WHERE rate.worker_id=w.id AND rate.effective_month<=?1
                          ORDER BY rate.effective_month DESC LIMIT 1),0),
                COALESCE((SELECT SUM(sw.amount_milli) FROM salary_withdrawals sw
                          WHERE sw.worker_id=w.id AND substr(sw.withdrawn_at,1,7)=?1),0)
                ,COALESCE((SELECT SUM(sd.amount_milli) FROM salary_deductions sd
                          WHERE sd.worker_id=w.id AND sd.deduction_month=?1),0)
         FROM workers w
         WHERE w.is_active=1
         ORDER BY w.is_active DESC,w.full_name,w.id",
        )
        .map_err(ApiError::internal)?;
    let rows = statement
        .query_map([month.clone()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)? == 1,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })
        .map_err(ApiError::internal)?;
    for row in rows {
        let (id, full_name, is_active, salary, withdrawals, deductions) =
            row.map_err(ApiError::internal)?;
        total_salary += salary;
        total_withdrawals += withdrawals;
        total_deductions += deductions;
        employees.push(json!({
            "worker": {"id": id, "fullName": full_name, "isActive": is_active},
            "salaryMilli": salary,
            "totalWithdrawalsMilli": withdrawals,
            "totalDeductionsMilli": deductions,
            "remainingSalaryMilli": salary - withdrawals - deductions,
            "salaryConfigured": salary > 0
        }));
    }
    Ok(ok(json!({
        "month": month,
        "employees": employees,
        "totalSalaryMilli": total_salary,
        "totalWithdrawalsMilli": total_withdrawals,
        "totalDeductionsMilli": total_deductions,
        "totalRemainingMilli": total_salary - total_withdrawals - total_deductions
    })))
}

async fn create_payroll_employee(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<PayrollEmployeeInput>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "financial.manage")?;
    let full_name = trim_required(&input.full_name, "اسم الموظف")?;
    let month = parse_payroll_month(Some(&input.month))?;
    let salary = parse_milli(&input.salary)?;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let worker_id = new_id();
    let timestamp = now();
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    tx.execute(
        "INSERT INTO workers(id,full_name,is_active,created_at,updated_at) VALUES(?1,?2,1,?3,?3)",
        params![worker_id, full_name, timestamp],
    )
    .map_err(ApiError::internal)?;
    tx.execute(
        "INSERT INTO worker_salary_rates(worker_id,effective_month,salary_milli,set_by,created_at,updated_at)
         VALUES(?1,?2,?3,?4,?5,?5)",
        params![worker_id, month, salary, principal.id, timestamp],
    ).map_err(ApiError::internal)?;
    insert_audit_tx(
        &tx,
        Some(&principal.id),
        "PAYROLL_EMPLOYEE_CREATED",
        "worker",
        Some(&worker_id),
        "تم إنشاء موظف في المرتبات دون إنشاء حساب مستخدم",
        Some(&json!({"workerId":worker_id,"month":month,"salaryMilli":salary})),
    )?;
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(json!({
        "worker": {"id":worker_id,"fullName":full_name,"isActive":true},
        "month":month,
        "salaryMilli":salary
    })))
}

async fn set_worker_salary(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(worker_id): Path<String>,
    Json(input): Json<SalaryInput>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "financial.manage")?;
    let month = parse_payroll_month(Some(&input.month))?;
    let salary = parse_milli(&input.salary)?;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let worker_name: String = db
        .conn
        .query_row(
            "SELECT full_name FROM workers WHERE id=?1",
            [worker_id.clone()],
            |row| row.get(0),
        )
        .optional()
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    let timestamp = now();
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    tx.execute(
        "INSERT INTO worker_salary_rates(worker_id,effective_month,salary_milli,set_by,created_at,updated_at)
         VALUES(?1,?2,?3,?4,?5,?5)
         ON CONFLICT(worker_id,effective_month) DO UPDATE SET salary_milli=excluded.salary_milli,set_by=excluded.set_by,updated_at=excluded.updated_at",
        params![worker_id, month, salary, principal.id, timestamp],
    ).map_err(ApiError::internal)?;
    insert_audit_tx(
        &tx,
        Some(&principal.id),
        "WORKER_SALARY_SET",
        "worker_salary_rate",
        Some(&worker_id),
        "تم تعيين الراتب الشهري للموظف",
        Some(&json!({"workerId":worker_id,"month":month,"salaryMilli":salary})),
    )?;
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(json!({
        "worker": {"id": worker_id, "fullName": worker_name},
        "month": month,
        "salaryMilli": salary
    })))
}

async fn delete_payroll_employee(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(worker_id): Path<String>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "financial.manage")?;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let worker_name: String = db
        .conn
        .query_row(
            "SELECT full_name FROM workers WHERE id=?1",
            [worker_id.clone()],
            |row| row.get(0),
        )
        .optional()
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    let timestamp = now();
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    tx.execute(
        "UPDATE workers SET is_active=0,deactivated_at=COALESCE(deactivated_at,?1),deactivated_by=?2,updated_at=?1 WHERE id=?3",
        params![timestamp, principal.id, worker_id],
    )
    .map_err(ApiError::internal)?;
    insert_audit_tx(
        &tx,
        Some(&principal.id),
        "WORKER_ARCHIVED_FROM_PAYROLL",
        "worker",
        Some(&worker_id),
        "تمت أرشفة الموظف من قسم المرتبات مع الاحتفاظ بكل السجلات التاريخية",
        None,
    )?;
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(
        json!({"archived":true,"worker":{"id":worker_id,"fullName":worker_name}}),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SalaryWithdrawalInput {
    worker_id: String,
    amount: String,
    withdrawn_at: String,
    notes: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SalaryDeductionInput {
    worker_id: String,
    amount: String,
    deducted_at: Option<String>,
    month: Option<String>,
    notes: Option<String>,
}

fn deduction_time(input: &SalaryDeductionInput) -> Result<(String, String), ApiError> {
    if let Some(value) = input.deducted_at.as_deref() {
        let deducted_at = value.trim();
        DateTime::parse_from_rfc3339(deducted_at)
            .map_err(|_| ApiError::bad("تاريخ الخصم غير صالح"))?;
        let deducted_at = deducted_at.to_owned();
        return Ok((deducted_at[..7].to_owned(), deducted_at));
    }
    let month = parse_payroll_month(input.month.as_ref())?;
    Ok((month.clone(), format!("{month}-01T12:00:00Z")))
}

async fn list_salary_deductions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    let _principal = authorize(&state, &headers, "financial.manage")?;
    let selected_date = selected_business_date(&query)?;
    let month = match selected_date {
        Some(date) => date.format("%Y-%m").to_string(),
        None => parse_payroll_month(query.get("month"))?,
    };
    let (from, to) = date_range(&query)?;
    let filter_by_date = if selected_date.is_some() { 1 } else { 0 };
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let mut statement = db.conn.prepare(
        "SELECT sd.id,sd.amount_milli,sd.deducted_at,sd.notes,w.id,w.full_name,creator.full_name,sd.created_at,sd.updated_at
         FROM salary_deductions sd
         JOIN workers w ON w.id=sd.worker_id
         JOIN users creator ON creator.id=sd.created_by
         WHERE (?1=0 AND sd.deduction_month=?2)
            OR (?1=1 AND sd.deducted_at BETWEEN ?3 AND ?4)
         ORDER BY sd.deducted_at DESC,sd.created_at DESC"
    ).map_err(ApiError::internal)?;
    let rows = statement.query_map(params![filter_by_date,month,from,to], |row| Ok(json!({
        "id":row.get::<_,String>(0)?,"amountMilli":row.get::<_,i64>(1)?,"deductedAt":row.get::<_,String>(2)?,
        "notes":row.get::<_,Option<String>>(3)?,"worker":{"id":row.get::<_,String>(4)?,"fullName":row.get::<_,String>(5)?},
        "recordedBy":row.get::<_,String>(6)?,"createdAt":row.get::<_,String>(7)?,"updatedAt":row.get::<_,String>(8)?
    }))).map_err(ApiError::internal)?;
    let mut items = Vec::new();
    for row in rows {
        items.push(row.map_err(ApiError::internal)?);
    }
    Ok(ok(json!({"items":items})))
}

async fn create_salary_deduction(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<SalaryDeductionInput>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "financial.manage")?;
    let worker_id = input.worker_id.trim().to_owned();
    if worker_id.is_empty() {
        return Err(ApiError::bad("اختر الموظف"));
    }
    let amount = parse_milli(&input.amount)?;
    let (month, deducted_at) = deduction_time(&input)?;
    let notes = input
        .notes
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let worker_name: String = db
        .conn
        .query_row(
            "SELECT full_name FROM workers WHERE id=?1",
            [worker_id.clone()],
            |row| row.get(0),
        )
        .optional()
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    let id = new_id();
    let timestamp = now();
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    tx.execute(
        "INSERT INTO salary_deductions(id,worker_id,amount_milli,deduction_month,deducted_at,notes,created_by,created_at,updated_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?8)",
        params![id, worker_id, amount, month, deducted_at, notes, principal.id, timestamp],
    ).map_err(ApiError::internal)?;
    insert_audit_tx(
        &tx,
        Some(&principal.id),
        "SALARY_DEDUCTION_CREATED",
        "salary_deduction",
        Some(&id),
        "تم تسجيل خصم موظف",
        Some(&json!({"workerId":worker_id,"amountMilli":amount,"month":month})),
    )?;
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(
        json!({"id":id,"amountMilli":amount,"month":month,"deductedAt":deducted_at,"notes":notes,"worker":{"id":worker_id,"fullName":worker_name},"recordedBy":principal.full_name,"createdAt":timestamp,"updatedAt":timestamp}),
    ))
}

async fn update_salary_deduction(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<SalaryDeductionInput>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "financial.manage")?;
    let worker_id = input.worker_id.trim().to_owned();
    if worker_id.is_empty() {
        return Err(ApiError::bad("اختر الموظف"));
    }
    let amount = parse_milli(&input.amount)?;
    let (month, deducted_at) = deduction_time(&input)?;
    let notes = input
        .notes
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let worker_name: String = db
        .conn
        .query_row(
            "SELECT full_name FROM workers WHERE id=?1",
            [worker_id.clone()],
            |row| row.get(0),
        )
        .optional()
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    let timestamp = now();
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    let changed = tx.execute(
        "UPDATE salary_deductions SET worker_id=?1,amount_milli=?2,deduction_month=?3,deducted_at=?4,notes=?5,updated_by=?6,updated_at=?7 WHERE id=?8",
        params![worker_id,amount,month,deducted_at,notes,principal.id,timestamp,id],
    ).map_err(ApiError::internal)?;
    if changed == 0 {
        return Err(ApiError::not_found());
    }
    insert_audit_tx(
        &tx,
        Some(&principal.id),
        "SALARY_DEDUCTION_UPDATED",
        "salary_deduction",
        Some(&id),
        "تم تعديل خصم موظف وإعادة احتساب الراتب المتبقي",
        Some(&json!({"workerId":worker_id,"amountMilli":amount,"month":month})),
    )?;
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(
        json!({"id":id,"amountMilli":amount,"month":month,"deductedAt":deducted_at,"notes":notes,"worker":{"id":worker_id,"fullName":worker_name},"recordedBy":principal.full_name,"updatedAt":timestamp}),
    ))
}

async fn delete_salary_deduction(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "financial.manage")?;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let worker_id: String = db
        .conn
        .query_row(
            "SELECT worker_id FROM salary_deductions WHERE id=?1",
            [id.clone()],
            |row| row.get(0),
        )
        .optional()
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    tx.execute("DELETE FROM salary_deductions WHERE id=?1", [id.clone()])
        .map_err(ApiError::internal)?;
    insert_audit_tx(
        &tx,
        Some(&principal.id),
        "SALARY_DEDUCTION_DELETED",
        "salary_deduction",
        Some(&id),
        "تم حذف خصم موظف وإعادة احتساب الراتب المتبقي",
        Some(&json!({"workerId":worker_id})),
    )?;
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(json!({"deleted":true,"id":id})))
}

fn withdrawal_time(value: &str) -> Result<String, ApiError> {
    let value = value.trim();
    DateTime::parse_from_rfc3339(value).map_err(|_| ApiError::bad("تاريخ المسحوب غير صالح"))?;
    Ok(value.to_owned())
}

async fn list_salary_withdrawals(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    let _principal = authorize(&state, &headers, "financial.manage")?;
    let selected_date = selected_business_date(&query)?;
    let month = match selected_date {
        Some(date) => date.format("%Y-%m").to_string(),
        None => parse_payroll_month(query.get("month"))?,
    };
    let (from, to) = date_range(&query)?;
    let filter_by_date = if selected_date.is_some() { 1 } else { 0 };
    let worker_filter = query
        .get("workerId")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty());
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let mut items = Vec::new();
    if let Some(worker_id) = worker_filter {
        let mut statement = db.conn.prepare(
            "SELECT sw.id,sw.amount_milli,sw.withdrawn_at,sw.notes,w.id,w.full_name,creator.full_name,sw.created_at,sw.updated_at
             FROM salary_withdrawals sw
             JOIN workers w ON w.id=sw.worker_id
             JOIN users creator ON creator.id=sw.created_by
             WHERE ((?1=0 AND substr(sw.withdrawn_at,1,7)=?2)
                    OR (?1=1 AND sw.withdrawn_at BETWEEN ?3 AND ?4))
               AND sw.worker_id=?5
             ORDER BY sw.withdrawn_at DESC,sw.created_at DESC",
        ).map_err(ApiError::internal)?;
        let rows = statement.query_map(params![filter_by_date,month,from,to,worker_id], |row| Ok(json!({
            "id":row.get::<_,String>(0)?,"amountMilli":row.get::<_,i64>(1)?,"withdrawnAt":row.get::<_,String>(2)?,
            "notes":row.get::<_,Option<String>>(3)?,"worker":{"id":row.get::<_,String>(4)?,"fullName":row.get::<_,String>(5)?},
            "recordedBy":row.get::<_,String>(6)?,"createdAt":row.get::<_,String>(7)?,"updatedAt":row.get::<_,String>(8)?
        }))).map_err(ApiError::internal)?;
        for row in rows {
            items.push(row.map_err(ApiError::internal)?);
        }
    } else {
        let mut statement = db.conn.prepare(
            "SELECT sw.id,sw.amount_milli,sw.withdrawn_at,sw.notes,w.id,w.full_name,creator.full_name,sw.created_at,sw.updated_at
             FROM salary_withdrawals sw
             JOIN workers w ON w.id=sw.worker_id
             JOIN users creator ON creator.id=sw.created_by
             WHERE (?1=0 AND substr(sw.withdrawn_at,1,7)=?2)
                OR (?1=1 AND sw.withdrawn_at BETWEEN ?3 AND ?4)
             ORDER BY sw.withdrawn_at DESC,sw.created_at DESC",
        ).map_err(ApiError::internal)?;
        let rows = statement.query_map(params![filter_by_date,month,from,to], |row| Ok(json!({
            "id":row.get::<_,String>(0)?,"amountMilli":row.get::<_,i64>(1)?,"withdrawnAt":row.get::<_,String>(2)?,
            "notes":row.get::<_,Option<String>>(3)?,"worker":{"id":row.get::<_,String>(4)?,"fullName":row.get::<_,String>(5)?},
            "recordedBy":row.get::<_,String>(6)?,"createdAt":row.get::<_,String>(7)?,"updatedAt":row.get::<_,String>(8)?
        }))).map_err(ApiError::internal)?;
        for row in rows {
            items.push(row.map_err(ApiError::internal)?);
        }
    }
    Ok(ok(json!({"items":items})))
}

async fn create_salary_withdrawal(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<SalaryWithdrawalInput>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "financial.manage")?;
    let worker_id = input.worker_id.trim().to_owned();
    if worker_id.is_empty() {
        return Err(ApiError::bad("اختر الموظف"));
    }
    let amount = parse_milli(&input.amount)?;
    let withdrawn_at = withdrawal_time(&input.withdrawn_at)?;
    let notes = input
        .notes
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let worker_name: String = db
        .conn
        .query_row(
            "SELECT full_name FROM workers WHERE id=?1",
            [worker_id.clone()],
            |row| row.get(0),
        )
        .optional()
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    let id = new_id();
    let timestamp = now();
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    tx.execute(
        "INSERT INTO salary_withdrawals(id,worker_id,amount_milli,withdrawn_at,notes,created_by,created_at,updated_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?7)",
        params![id,worker_id,amount,withdrawn_at,notes,principal.id,timestamp],
    ).map_err(ApiError::internal)?;
    insert_audit_tx(
        &tx,
        Some(&principal.id),
        "SALARY_WITHDRAWAL_CREATED",
        "salary_withdrawal",
        Some(&id),
        "تم تسجيل مسحوب موظف",
        Some(&json!({"workerId":worker_id,"amountMilli":amount})),
    )?;
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(json!({
        "id":id,"amountMilli":amount,"withdrawnAt":withdrawn_at,"notes":notes,
        "worker":{"id":worker_id,"fullName":worker_name},"recordedBy":principal.full_name,"createdAt":timestamp,"updatedAt":timestamp
    })))
}

async fn update_salary_withdrawal(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<SalaryWithdrawalInput>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "financial.manage")?;
    let worker_id = input.worker_id.trim().to_owned();
    if worker_id.is_empty() {
        return Err(ApiError::bad("اختر الموظف"));
    }
    let amount = parse_milli(&input.amount)?;
    let withdrawn_at = withdrawal_time(&input.withdrawn_at)?;
    let notes = input
        .notes
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let worker_name: String = db
        .conn
        .query_row(
            "SELECT full_name FROM workers WHERE id=?1",
            [worker_id.clone()],
            |row| row.get(0),
        )
        .optional()
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    let timestamp = now();
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    let affected = tx.execute(
        "UPDATE salary_withdrawals SET worker_id=?1,amount_milli=?2,withdrawn_at=?3,notes=?4,updated_by=?5,updated_at=?6 WHERE id=?7",
        params![worker_id,amount,withdrawn_at,notes,principal.id,timestamp,id],
    ).map_err(ApiError::internal)?;
    if affected == 0 {
        return Err(ApiError::not_found());
    }
    insert_audit_tx(
        &tx,
        Some(&principal.id),
        "SALARY_WITHDRAWAL_UPDATED",
        "salary_withdrawal",
        Some(&id),
        "تم تعديل مسحوب موظف وإعادة احتساب الراتب المتبقي",
        Some(&json!({"workerId":worker_id,"amountMilli":amount})),
    )?;
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(json!({
        "id":id,"amountMilli":amount,"withdrawnAt":withdrawn_at,"notes":notes,
        "worker":{"id":worker_id,"fullName":worker_name},"recordedBy":principal.full_name,"updatedAt":timestamp
    })))
}

async fn delete_salary_withdrawal(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "financial.manage")?;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let worker_id: String = db
        .conn
        .query_row(
            "SELECT worker_id FROM salary_withdrawals WHERE id=?1",
            [id.clone()],
            |row| row.get(0),
        )
        .optional()
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    tx.execute("DELETE FROM salary_withdrawals WHERE id=?1", [id.clone()])
        .map_err(ApiError::internal)?;
    insert_audit_tx(
        &tx,
        Some(&principal.id),
        "SALARY_WITHDRAWAL_DELETED",
        "salary_withdrawal",
        Some(&id),
        "تم حذف مسحوب موظف وإعادة احتساب الراتب المتبقي",
        Some(&json!({"workerId":worker_id})),
    )?;
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(json!({"deleted":true,"id":id})))
}

async fn list_showroom_payments(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    let _principal = authorize(&state, &headers, "financial.manage")?;
    let (from, to) = date_range(&query)?;
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let mut items = Vec::new();
    let mut statement=db.conn.prepare("SELECT sp.id,sp.amount_milli,sp.paid_at,sp.notes,s.id,s.name,u.full_name FROM showroom_payments sp JOIN showrooms s ON s.id=sp.showroom_id JOIN users u ON u.id=sp.created_by WHERE sp.paid_at BETWEEN ?1 AND ?2 ORDER BY sp.paid_at DESC").map_err(ApiError::internal)?;
    let rows=statement.query_map(params![from,to],|row|Ok(json!({"id":row.get::<_,String>(0)?,"amountMilli":row.get::<_,i64>(1)?,"paidAt":row.get::<_,String>(2)?,"notes":row.get::<_,Option<String>>(3)?,"showroom":{"id":row.get::<_,String>(4)?,"name":row.get::<_,String>(5)?},"recordedBy":row.get::<_,String>(6)?}))).map_err(ApiError::internal)?;
    for row in rows {
        items.push(row.map_err(ApiError::internal)?);
    }
    Ok(ok(json!({"items":items})))
}

async fn create_showroom_payment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<PaymentInput>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "financial.manage")?;
    let showroom_id = input
        .showroom_id
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| ApiError::bad("اختر المعرض"))?
        .to_owned();
    let amount = parse_milli(&input.amount)?;
    let paid_at = payment_time(&input.paid_at)?;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let exists: Option<String> = db
        .conn
        .query_row(
            "SELECT id FROM showrooms WHERE id=?1",
            [showroom_id.clone()],
            |row| row.get(0),
        )
        .optional()
        .map_err(ApiError::internal)?;
    if exists.is_none() {
        return Err(ApiError::not_found());
    }
    let id = new_id();
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    tx.execute("INSERT INTO showroom_payments(id,showroom_id,amount_milli,paid_at,notes,created_by,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![id,showroom_id,amount,paid_at,input.notes.map(|v|v.trim().to_owned()).filter(|v|!v.is_empty()),principal.id,now()]).map_err(ApiError::internal)?;
    add_financial_transaction(
        &tx,
        "showroom_payment",
        &id,
        &paid_at,
        &principal.id,
        &[
            ("CASH", "debit", amount, None, None),
            (
                "SHOWROOM_RECEIVABLE",
                "credit",
                amount,
                None,
                Some(&showroom_id),
            ),
        ],
    )?;
    insert_audit_tx(
        &tx,
        Some(&principal.id),
        "SHOWROOM_PAYMENT_RECORDED",
        "showroom_payment",
        Some(&id),
        "تم تسجيل دفعة معرض",
        Some(&json!({"showroomId":showroom_id})),
    )?;
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(showroom_payment_item_by_id(&db.conn, &id)?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExpenseInput {
    description: String,
    category: String,
    amount: String,
    occurred_at: Option<String>,
    notes: Option<String>,
    allocation_type: String,
    business_bps: Option<i64>,
}

async fn list_expenses(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    let _principal = authorize(&state, &headers, "financial.manage")?;
    let (from, to) = date_range(&query)?;
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let mut items = Vec::new();
    let mut statement=db.conn.prepare("SELECT e.id,e.description,e.category,e.amount_milli,e.occurred_at,e.notes,e.allocation_type,e.business_bps,e.workers_bps,e.business_amount_milli,e.workers_amount_milli,u.full_name FROM expenses e JOIN users u ON u.id=e.created_by WHERE e.occurred_at BETWEEN ?1 AND ?2 ORDER BY e.occurred_at DESC").map_err(ApiError::internal)?;
    let rows=statement.query_map(params![from,to],|row|Ok(json!({"id":row.get::<_,String>(0)?,"description":row.get::<_,String>(1)?,"category":row.get::<_,String>(2)?,"amountMilli":row.get::<_,i64>(3)?,"occurredAt":row.get::<_,String>(4)?,"notes":row.get::<_,Option<String>>(5)?,"allocationType":row.get::<_,String>(6)?,"businessBps":row.get::<_,i64>(7)?,"workersBps":row.get::<_,i64>(8)?,"businessAmountMilli":row.get::<_,i64>(9)?,"workersAmountMilli":row.get::<_,i64>(10)?,"recordedBy":row.get::<_,String>(11)?}))).map_err(ApiError::internal)?;
    for row in rows {
        items.push(row.map_err(ApiError::internal)?);
    }
    Ok(ok(json!({"items":items})))
}

async fn create_expense(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<ExpenseInput>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "financial.manage")?;
    let description = trim_required(&input.description, "وصف المصروف")?;
    let category = trim_required(&input.category, "فئة المصروف")?;
    let amount = parse_milli(&input.amount)?;
    let occurred_at = payment_time(&input.occurred_at)?;
    let (business_bps, workers_bps) = match input.allocation_type.as_str() {
        "business" => (10000, 0),
        "workers" => (0, 10000),
        "shared" => {
            let bps = input.business_bps.unwrap_or(5000);
            if !(1..=9999).contains(&bps) {
                return Err(ApiError::bad(
                    "نسبة التوزيع المشترك يجب أن تكون بين 1% و99%",
                ));
            }
            (bps, 10000 - bps)
        }
        _ => return Err(ApiError::bad("نوع توزيع المصروف غير صالح")),
    };
    let business_amount = round_percentage(amount, business_bps);
    let workers_amount = amount - business_amount;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let workers: Vec<String> = if workers_amount > 0 {
        let mut statement = db
            .conn
            .prepare("SELECT id FROM workers WHERE is_active=1 ORDER BY full_name,id")
            .map_err(ApiError::internal)?;
        let listed = statement
            .query_map([], |row| row.get(0))
            .map_err(ApiError::internal)?
            .collect::<Result<Vec<String>, _>>()
            .map_err(ApiError::internal)?;
        listed
    } else {
        Vec::new()
    };
    if workers_amount > 0 && workers.is_empty() {
        return Err(ApiError::bad("لا يمكن توزيع حصة العمال دون وجود عامل نشط"));
    }
    let id = new_id();
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    tx.execute("INSERT INTO expenses(id,description,category,amount_milli,occurred_at,notes,allocation_type,business_bps,workers_bps,business_amount_milli,workers_amount_milli,created_by,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",params![id,description,category,amount,occurred_at,input.notes.map(|v|v.trim().to_owned()).filter(|v|!v.is_empty()),input.allocation_type,business_bps,workers_bps,business_amount,workers_amount,principal.id,now()]).map_err(ApiError::internal)?;
    if workers_amount > 0 {
        let each = workers_amount / workers.len() as i64;
        let remainder = workers_amount % workers.len() as i64;
        for (order, worker_id) in workers.iter().enumerate() {
            let share = each + if (order as i64) < remainder { 1 } else { 0 };
            tx.execute("INSERT INTO expense_allocations(id,expense_id,worker_id,amount_milli,allocation_order,created_at) VALUES(?1,?2,?3,?4,?5,?6)",params![new_id(),id,worker_id,share,order as i64,now()]).map_err(ApiError::internal)?;
        }
    }
    add_financial_transaction(
        &tx,
        "expense",
        &id,
        &occurred_at,
        &principal.id,
        &[
            ("BUSINESS_EXPENSE", "debit", business_amount, None, None),
            ("WORKER_PAYABLE", "debit", workers_amount, None, None),
            ("CASH", "credit", amount, None, None),
        ],
    )?;
    insert_audit_tx(
        &tx,
        Some(&principal.id),
        "EXPENSE_CREATED",
        "expense",
        Some(&id),
        "تم تسجيل مصروف وتوزيعه",
        Some(&json!({"allocationType":input.allocation_type,"workerCount":workers.len()})),
    )?;
    if workers_amount > 0 {
        insert_audit_tx(
            &tx,
            Some(&principal.id),
            "WORKER_EXPENSE_DEDUCTIONS_CREATED",
            "expense",
            Some(&id),
            "تم إنشاء استقطاعات العمال المرتبطة بالمصروف",
            Some(&json!({"workerCount":workers.len(),"workersAmountMilli":workers_amount})),
        )?;
    }
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(
        json!({"id":id,"businessAmountMilli":business_amount,"workersAmountMilli":workers_amount,"workerCount":workers.len()}),
    ))
}

async fn expense_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult {
    let _principal = authorize(&state, &headers, "financial.manage")?;
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let expense = db.conn.query_row("SELECT e.id,e.description,e.amount_milli,e.occurred_at,e.notes,e.allocation_type,e.business_amount_milli,e.workers_amount_milli,u.full_name,e.created_at FROM expenses e JOIN users u ON u.id=e.created_by WHERE e.id=?1",[id.clone()],|row|Ok(json!({"id":row.get::<_,String>(0)?,"description":row.get::<_,String>(1)?,"amountMilli":row.get::<_,i64>(2)?,"occurredAt":row.get::<_,String>(3)?,"notes":row.get::<_,Option<String>>(4)?,"allocationType":row.get::<_,String>(5)?,"businessAmountMilli":row.get::<_,i64>(6)?,"workersAmountMilli":row.get::<_,i64>(7)?,"recordedBy":row.get::<_,String>(8)?,"createdAt":row.get::<_,String>(9)?}))).optional().map_err(ApiError::internal)?.ok_or_else(ApiError::not_found)?;
    let mut allocations = Vec::new();
    let mut statement=db.conn.prepare("SELECT ea.worker_id,w.full_name,ea.amount_milli,ea.created_at FROM expense_allocations ea JOIN workers w ON w.id=ea.worker_id WHERE ea.expense_id=?1 ORDER BY ea.allocation_order").map_err(ApiError::internal)?;
    let rows=statement.query_map([id],|row|Ok(json!({"workerId":row.get::<_,String>(0)?,"workerName":row.get::<_,String>(1)?,"amountMilli":row.get::<_,i64>(2)?,"createdAt":row.get::<_,String>(3)?}))).map_err(ApiError::internal)?;
    for row in rows {
        allocations.push(row.map_err(ApiError::internal)?);
    }
    Ok(ok(json!({"expense":expense,"allocations":allocations})))
}

async fn update_expense(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<ExpenseInput>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "financial.manage")?;
    let description = trim_required(&input.description, "وصف المصروف")?;
    let amount = parse_milli(&input.amount)?;
    let occurred_at = payment_time(&input.occurred_at)?;
    let (business_bps, workers_bps) = match input.allocation_type.as_str() {
        "business" => (10000, 0),
        "shared" => (5000, 5000),
        _ => return Err(ApiError::bad("نوع توزيع المصروف غير صالح")),
    };
    let business_amount = round_percentage(amount, business_bps);
    let workers_amount = amount - business_amount;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let old=db.conn.query_row("SELECT amount_milli,business_amount_milli,workers_amount_milli,occurred_at FROM expenses WHERE id=?1",[id.clone()],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,i64>(1)?,row.get::<_,i64>(2)?,row.get::<_,String>(3)?))).optional().map_err(ApiError::internal)?.ok_or_else(ApiError::not_found)?;
    let mut workers: Vec<String> = {
        let mut s=db.conn.prepare("SELECT worker_id FROM expense_allocations WHERE expense_id=?1 ORDER BY allocation_order").map_err(ApiError::internal)?;
        let values = s
            .query_map([id.clone()], |r| r.get(0))
            .map_err(ApiError::internal)?
            .collect::<Result<Vec<String>, _>>()
            .map_err(ApiError::internal)?;
        values
    };
    if workers_amount > 0 && workers.is_empty() {
        let mut s = db
            .conn
            .prepare("SELECT id FROM workers WHERE is_active=1 ORDER BY full_name,id")
            .map_err(ApiError::internal)?;
        workers = s
            .query_map([], |r| r.get(0))
            .map_err(ApiError::internal)?
            .collect::<Result<Vec<String>, _>>()
            .map_err(ApiError::internal)?;
    }
    if workers_amount > 0 && workers.is_empty() {
        return Err(ApiError::bad("لا يمكن توزيع حصة العمال دون وجود عامل نشط"));
    }
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    add_financial_transaction(
        &tx,
        "expense_edit_reverse",
        &new_id(),
        &old.3,
        &principal.id,
        &[
            ("BUSINESS_EXPENSE", "credit", old.1, None, None),
            ("WORKER_PAYABLE", "credit", old.2, None, None),
            ("CASH", "debit", old.0, None, None),
        ],
    )?;
    tx.execute(
        "DELETE FROM expense_allocations WHERE expense_id=?1",
        [id.clone()],
    )
    .map_err(ApiError::internal)?;
    if workers_amount > 0 {
        let each = workers_amount / workers.len() as i64;
        let remainder = workers_amount % workers.len() as i64;
        for (order, worker_id) in workers.iter().enumerate() {
            let share = each + if (order as i64) < remainder { 1 } else { 0 };
            tx.execute("INSERT INTO expense_allocations(id,expense_id,worker_id,amount_milli,allocation_order,created_at) VALUES(?1,?2,?3,?4,?5,?6)",params![new_id(),id,worker_id,share,order as i64,now()]).map_err(ApiError::internal)?;
        }
    }
    tx.execute("UPDATE expenses SET description=?1,amount_milli=?2,occurred_at=?3,notes=?4,allocation_type=?5,business_bps=?6,workers_bps=?7,business_amount_milli=?8,workers_amount_milli=?9 WHERE id=?10",params![description,amount,occurred_at,input.notes.map(|v|v.trim().to_owned()).filter(|v|!v.is_empty()),input.allocation_type,business_bps,workers_bps,business_amount,workers_amount,id]).map_err(ApiError::internal)?;
    add_financial_transaction(
        &tx,
        "expense_edit_post",
        &new_id(),
        &occurred_at,
        &principal.id,
        &[
            ("BUSINESS_EXPENSE", "debit", business_amount, None, None),
            ("WORKER_PAYABLE", "debit", workers_amount, None, None),
            ("CASH", "credit", amount, None, None),
        ],
    )?;
    insert_audit_tx(
        &tx,
        Some(&principal.id),
        "EXPENSE_UPDATED",
        "expense",
        Some(&id),
        "تم تعديل المصروف وإعادة توزيع الاستقطاعات على لقطة العمال الأصلية",
        Some(&json!({"workerCount":workers.len()})),
    )?;
    if old.2 > 0 {
        insert_audit_tx(
            &tx,
            Some(&principal.id),
            "WORKER_EXPENSE_DEDUCTIONS_REVERSED",
            "expense",
            Some(&id),
            "تم عكس استقطاعات العمال السابقة للمصروف",
            Some(&json!({"workersAmountMilli":old.2})),
        )?;
    }
    if workers_amount > 0 {
        insert_audit_tx(
            &tx,
            Some(&principal.id),
            "WORKER_EXPENSE_DEDUCTIONS_CREATED",
            "expense",
            Some(&id),
            "تم إنشاء استقطاعات العمال المحدثة للمصروف",
            Some(&json!({"workerCount":workers.len(),"workersAmountMilli":workers_amount})),
        )?;
    }
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(json!({"updated":true})))
}

async fn delete_expense(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "financial.manage")?;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let old=db.conn.query_row("SELECT amount_milli,business_amount_milli,workers_amount_milli,occurred_at,description FROM expenses WHERE id=?1",[id.clone()],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,i64>(1)?,row.get::<_,i64>(2)?,row.get::<_,String>(3)?,row.get::<_,String>(4)?))).optional().map_err(ApiError::internal)?.ok_or_else(ApiError::not_found)?;
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    add_financial_transaction(
        &tx,
        "expense_delete_reverse",
        &new_id(),
        &old.3,
        &principal.id,
        &[
            ("BUSINESS_EXPENSE", "credit", old.1, None, None),
            ("WORKER_PAYABLE", "credit", old.2, None, None),
            ("CASH", "debit", old.0, None, None),
        ],
    )?;
    tx.execute(
        "DELETE FROM expense_allocations WHERE expense_id=?1",
        [id.clone()],
    )
    .map_err(ApiError::internal)?;
    tx.execute("DELETE FROM expenses WHERE id=?1", [id.clone()])
        .map_err(ApiError::internal)?;
    insert_audit_tx(
        &tx,
        Some(&principal.id),
        "EXPENSE_DELETED",
        "expense",
        Some(&id),
        "تم حذف المصروف وعكس آثاره المرتبطة",
        Some(&json!({"description":old.4,"workersAmountMilli":old.2})),
    )?;
    if old.2 > 0 {
        insert_audit_tx(
            &tx,
            Some(&principal.id),
            "WORKER_EXPENSE_DEDUCTIONS_REVERSED",
            "expense",
            Some(&id),
            "تم عكس استقطاعات العمال بسبب حذف المصروف",
            Some(&json!({"workersAmountMilli":old.2})),
        )?;
    }
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(json!({"deleted":true})))
}

fn financial_summary(
    conn: &Connection,
    from: &str,
    to: &str,
    owner_id: Option<&str>,
) -> Result<Value, ApiError> {
    let revenue=total_for(conn,"SELECT COALESCE(SUM(price_milli),0) FROM wash_operations WHERE status='posted' AND occurred_at BETWEEN ?1 AND ?2 AND (?3 IS NULL OR created_by=?3)",params![from,to,owner_id])?;
    let cash=total_for(conn,"SELECT COALESCE(SUM(price_milli),0) FROM wash_operations WHERE status='posted' AND payment_type='cash' AND occurred_at BETWEEN ?1 AND ?2 AND (?3 IS NULL OR created_by=?3)",params![from,to,owner_id])?;
    let showroom_revenue=total_for(conn,"SELECT COALESCE(SUM(price_milli),0) FROM wash_operations WHERE status='posted' AND payment_type='showroom' AND occurred_at BETWEEN ?1 AND ?2 AND (?3 IS NULL OR created_by=?3)",params![from,to,owner_id])?;
    let commissions=total_for(conn,"SELECT COALESCE(SUM(commission_milli),0) FROM wash_operations WHERE status='posted' AND occurred_at BETWEEN ?1 AND ?2 AND (?3 IS NULL OR created_by=?3)",params![from,to,owner_id])?;
    let business_share=total_for(conn,"SELECT COALESCE(SUM(business_share_milli),0) FROM wash_operations WHERE status='posted' AND occurred_at BETWEEN ?1 AND ?2 AND (?3 IS NULL OR created_by=?3)",params![from,to,owner_id])?;
    let expenses = total_for(conn,"SELECT COALESCE(SUM(amount_milli),0) FROM expenses WHERE occurred_at BETWEEN ?1 AND ?2 AND (?3 IS NULL OR created_by=?3)",params![from,to,owner_id])?;
    let business_expenses=total_for(conn,"SELECT COALESCE(SUM(business_amount_milli),0) FROM expenses WHERE occurred_at BETWEEN ?1 AND ?2 AND (?3 IS NULL OR created_by=?3)",params![from,to,owner_id])?;
    let workers_expenses=total_for(conn,"SELECT COALESCE(SUM(workers_amount_milli),0) FROM expenses WHERE occurred_at BETWEEN ?1 AND ?2 AND (?3 IS NULL OR created_by=?3)",params![from,to,owner_id])?;
    let showroom_payments=total_for(conn,"SELECT COALESCE(SUM(amount_milli),0) FROM showroom_payments WHERE paid_at BETWEEN ?1 AND ?2 AND (?3 IS NULL OR created_by=?3)",params![from,to,owner_id])?;
    let worker_deductions=total_for(conn,"SELECT COALESCE(SUM(ea.amount_milli),0) FROM expense_allocations ea JOIN expenses e ON e.id=ea.expense_id WHERE e.occurred_at BETWEEN ?1 AND ?2 AND (?3 IS NULL OR e.created_by=?3)",params![from,to,owner_id])?;
    let outstanding_worker=total_for(conn,"SELECT MAX(0, COALESCE((SELECT SUM(commission_milli) FROM wash_operations WHERE status='posted' AND occurred_at BETWEEN ?1 AND ?2 AND (?3 IS NULL OR created_by=?3)),0)-COALESCE((SELECT SUM(ea.amount_milli) FROM expense_allocations ea JOIN expenses e ON e.id=ea.expense_id WHERE e.occurred_at BETWEEN ?1 AND ?2 AND (?3 IS NULL OR e.created_by=?3)),0))",params![from,to,owner_id])?;
    let outstanding_showroom=total_for(conn,"SELECT COALESCE((SELECT SUM(price_milli) FROM wash_operations WHERE status='posted' AND payment_type='showroom' AND occurred_at BETWEEN ?1 AND ?2 AND (?3 IS NULL OR created_by=?3))-(SELECT COALESCE(SUM(amount_milli),0) FROM showroom_payments WHERE paid_at BETWEEN ?1 AND ?2 AND (?3 IS NULL OR created_by=?3)),0)",params![from,to,owner_id])?;
    Ok(
        json!({"totalWashRevenueMilli":revenue,"cashRevenueMilli":cash,"showroomRevenueMilli":showroom_revenue,"businessShareMilli":business_share,"workerCommissionsMilli":commissions,"workerDeductionsMilli":worker_deductions,"outstandingWorkerBalancesMilli":outstanding_worker,"expensesMilli":expenses,"businessExpensesMilli":business_expenses,"workerExpensesMilli":workers_expenses,"showroomPaymentsMilli":showroom_payments,"outstandingShowroomDebtMilli":outstanding_showroom,"netBusinessProfitMilli":business_share-business_expenses}),
    )
}

async fn finance_overview(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "financial.manage")?;
    let (from, to) = date_range(&query)?;
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    Ok(ok(financial_summary(
        &db.conn,
        &from,
        &to,
        if principal.is_manager() {
            None
        } else {
            Some(&principal.id)
        },
    )?))
}

async fn operational_report(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "operational.read")?;
    let (from, to) = date_range(&query)?;
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let can_view_all = principal.is_manager();
    let owner_id = principal.id.clone();
    let wash_count=total_for(&db.conn,"SELECT COUNT(*) FROM wash_operations WHERE status='posted' AND occurred_at BETWEEN ?1 AND ?2 AND (?3=1 OR created_by=?4)",params![&from,&to,if can_view_all { 1 } else { 0 },&owner_id])?;
    let mut workers = Vec::new();
    let mut statement=db.conn.prepare("SELECT worker.id,worker.full_name,COUNT(w.id) FROM workers worker LEFT JOIN wash_operations w ON w.worker_id=worker.id AND w.status='posted' AND w.occurred_at BETWEEN ?1 AND ?2 AND (?3=1 OR w.created_by=?4) GROUP BY worker.id ORDER BY COUNT(w.id) DESC,worker.full_name").map_err(ApiError::internal)?;
    let rows=statement.query_map(params![&from,&to,if can_view_all { 1 } else { 0 },&owner_id],|row|Ok(json!({"workerId":row.get::<_,String>(0)?,"workerName":row.get::<_,String>(1)?,"carsWashed":row.get::<_,i64>(2)?}))).map_err(ApiError::internal)?;
    for row in rows {
        workers.push(row.map_err(ApiError::internal)?);
    }
    let mut washes = Vec::new();
    let mut history=db.conn.prepare("SELECT w.id,w.vehicle_make,w.vehicle_model,w.manufacture_year,w.license_plate,w.occurred_at,w.payment_type,w.status,worker.id,worker.full_name,showroom.id,showroom.name FROM wash_operations w JOIN workers worker ON worker.id=w.worker_id LEFT JOIN showrooms showroom ON showroom.id=w.showroom_id WHERE w.status='posted' AND w.occurred_at BETWEEN ?1 AND ?2 AND (?3=1 OR w.created_by=?4) ORDER BY w.occurred_at DESC LIMIT 300").map_err(ApiError::internal)?;
    let rows=history.query_map(params![&from,&to,if can_view_all { 1 } else { 0 },&owner_id],|row|Ok(json!({"id":row.get::<_,String>(0)?,"vehicleMake":row.get::<_,String>(1)?,"vehicleModel":row.get::<_,String>(2)?,"manufactureYear":row.get::<_,Option<i32>>(3)?,"licensePlate":row.get::<_,Option<String>>(4)?,"occurredAt":row.get::<_,String>(5)?,"paymentType":row.get::<_,String>(6)?,"status":row.get::<_,String>(7)?,"worker":{"id":row.get::<_,String>(8)?,"fullName":row.get::<_,String>(9)?},"showroom":row.get::<_,Option<String>>(10)?.map(|id|json!({"id":id,"name":row.get::<_,Option<String>>(11).ok().flatten()}))}))).map_err(ApiError::internal)?;
    for row in rows {
        washes.push(row.map_err(ApiError::internal)?);
    }
    Ok(ok(
        json!({"from":from,"to":to,"carsWashed":wash_count,"workerPerformance":workers,"washes":washes}),
    ))
}

async fn financial_report(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "financial.manage")?;
    let (from, to) = date_range(&query)?;
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let summary = financial_summary(
        &db.conn,
        &from,
        &to,
        if principal.is_manager() {
            None
        } else {
            Some(&principal.id)
        },
    )?;
    let mut workers = Vec::new();
    let can_view_all = principal.is_manager();
    let owner_id = principal.id.clone();
    let mut statement=db.conn.prepare("SELECT worker.id,worker.full_name,COUNT(w.id),COALESCE(SUM(w.price_milli),0),COALESCE(SUM(w.commission_milli),0),COALESCE((SELECT SUM(ea.amount_milli) FROM expense_allocations ea JOIN expenses e ON e.id=ea.expense_id WHERE ea.worker_id=worker.id AND e.occurred_at BETWEEN ?1 AND ?2 AND (?3=1 OR e.created_by=?4)),0) FROM workers worker LEFT JOIN wash_operations w ON w.worker_id=worker.id AND w.status='posted' AND w.occurred_at BETWEEN ?1 AND ?2 AND (?3=1 OR w.created_by=?4) GROUP BY worker.id ORDER BY worker.full_name").map_err(ApiError::internal)?;
    let rows=statement.query_map(params![from,to,if can_view_all { 1 } else { 0 },owner_id],|row|{let commission:i64=row.get(4)?;let deductions:i64=row.get(5)?;Ok(json!({"workerId":row.get::<_,String>(0)?,"workerName":row.get::<_,String>(1)?,"carsWashed":row.get::<_,i64>(2)?,"revenueMilli":row.get::<_,i64>(3)?,"commissionMilli":commission,"deductionsMilli":deductions,"remainingMilli":(commission-deductions).max(0)}))}).map_err(ApiError::internal)?;
    for row in rows {
        workers.push(row.map_err(ApiError::internal)?);
    }
    Ok(ok(
        json!({"from":from,"to":to,"summary":summary,"workerPerformance":workers}),
    ))
}

async fn get_settings(State(state): State<AppState>, headers: HeaderMap) -> ApiResult {
    let _principal = authorize(&state, &headers, "settings.manage")?;
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let mut values = serde_json::Map::new();
    let mut statement = db
        .conn
        .prepare("SELECT key,value_json,updated_at FROM settings ORDER BY key")
        .map_err(ApiError::internal)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(ApiError::internal)?;
    for row in rows {
        let (key, raw, updated_at) = row.map_err(ApiError::internal)?;
        values.insert(key,json!({"value":serde_json::from_str::<Value>(&raw).unwrap_or(Value::String(raw)),"updatedAt":updated_at}));
    }
    Ok(ok(Value::Object(values)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SettingsInput {
    business_name: Option<String>,
    currency: Option<String>,
    default_worker_commission_bps: Option<i64>,
}

async fn update_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<SettingsInput>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "settings.manage")?;
    if let Some(value) = input.default_worker_commission_bps {
        if !(0..=10000).contains(&value) {
            return Err(ApiError::bad("نسبة العمولة الافتراضية غير صالحة"));
        }
    }
    if let Some(ref name) = input.business_name {
        trim_required(name, "اسم النشاط")?;
    }
    if let Some(ref currency) = input.currency {
        if currency.trim().is_empty() || currency.chars().count() > 10 {
            return Err(ApiError::bad("العملة غير صالحة"));
        }
    }
    let mut updates: Vec<(&str, String)> = Vec::new();
    if let Some(value) = input.business_name {
        updates.push((
            "business_name",
            serde_json::to_string(&value.trim()).map_err(ApiError::internal)?,
        ));
    }
    if let Some(value) = input.currency {
        updates.push((
            "currency",
            serde_json::to_string(&value.trim()).map_err(ApiError::internal)?,
        ));
    }
    if let Some(value) = input.default_worker_commission_bps {
        updates.push(("default_worker_commission_bps", value.to_string()));
    }
    if updates.is_empty() {
        return Err(ApiError::bad("لا توجد إعدادات لتحديثها"));
    }
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    let commission_changed = updates
        .iter()
        .any(|(key, _)| *key == "default_worker_commission_bps");
    for (key, value) in &updates {
        tx.execute("INSERT INTO settings(key,value_json,updated_by,updated_at) VALUES(?1,?2,?3,?4) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json,updated_by=excluded.updated_by,updated_at=excluded.updated_at",params![key,value,principal.id,now()]).map_err(ApiError::internal)?;
    }
    let action = if commission_changed {
        "COMMISSION_RATE_CHANGED"
    } else {
        "SETTINGS_UPDATED"
    };
    let description = if commission_changed {
        "تم تغيير نسبة عمولة العمال الافتراضية للغسيلات الجديدة فقط"
    } else {
        "تم تعديل إعدادات النظام"
    };
    insert_audit_tx(
        &tx,
        Some(&principal.id),
        action,
        "settings",
        None,
        description,
        None,
    )?;
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(json!({"updated":true})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserInput {
    full_name: String,
    username: String,
    password: String,
    role_code: String,
    is_active: Option<bool>,
    worker_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserUpdateInput {
    full_name: Option<String>,
    username: Option<String>,
    password: Option<String>,
    role_code: Option<String>,
    is_active: Option<bool>,
    worker_id: Option<String>,
}

fn role_id_for(conn: &Connection, role_code: &str) -> Result<String, ApiError> {
    if !matches!(role_code, "manager" | "employee") {
        return Err(ApiError::bad("الدور المختار غير صالح"));
    }
    conn.query_row("SELECT id FROM roles WHERE code=?1", [role_code], |row| {
        row.get(0)
    })
    .map_err(ApiError::internal)
}

async fn list_users(State(state): State<AppState>, headers: HeaderMap) -> ApiResult {
    let _principal = authorize(&state, &headers, "users.manage")?;
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let mut items = Vec::new();
    let mut statement=db.conn.prepare("SELECT u.id,u.full_name,u.username_norm,u.is_active,u.created_at,r.code,r.name_ar,COALESCE(p.theme,'light'),u.worker_id FROM users u JOIN user_roles ur ON ur.user_id=u.id JOIN roles r ON r.id=ur.role_id LEFT JOIN user_preferences p ON p.user_id=u.id WHERE u.deleted_at IS NULL ORDER BY u.created_at").map_err(ApiError::internal)?;
    let rows=statement.query_map([],|row|Ok((row.get::<_,String>(0)?,json!({"id":row.get::<_,String>(0)?,"fullName":row.get::<_,String>(1)?,"username":row.get::<_,String>(2)?,"isActive":row.get::<_,i64>(3)?==1,"createdAt":row.get::<_,String>(4)?,"roleCode":row.get::<_,String>(5)?,"roleName":row.get::<_,String>(6)?,"theme":row.get::<_,String>(7)?,"workerId":row.get::<_,Option<String>>(8)?})))).map_err(ApiError::internal)?;
    for row in rows {
        let (user_id, mut item) = row.map_err(ApiError::internal)?;
        item["permissions"] = json!(permission_codes_for_user(&db.conn, &user_id)?);
        items.push(item);
    }
    Ok(ok(json!({"items":items})))
}

async fn create_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<UserInput>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "users.manage")?;
    if input.role_code == "manager" && !principal.is_manager() {
        return Err(ApiError::forbidden());
    }
    let full_name = trim_required(&input.full_name, "الاسم الكامل")?;
    let username = normalized_username(&input.username)?;
    valid_password(&input.password)?;
    let hash = hash_password(&input.password)?;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let role_id = role_id_for(&db.conn, &input.role_code)?;
    let worker_id = if input.role_code == "employee" {
        if let Some(worker_id) = input
            .worker_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let exists: bool = db
                .conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM workers WHERE id=?1 AND is_active=1)",
                    [worker_id],
                    |row| row.get(0),
                )
                .map_err(ApiError::internal)?;
            if !exists {
                return Err(ApiError::bad("العامل المرتبط غير صالح"));
            }
            Some(worker_id.to_owned())
        } else {
            None
        }
    } else {
        None
    };
    let id = new_id();
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    tx.execute("INSERT INTO users(id,full_name,username_norm,password_hash,is_active,worker_id,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?7)",params![id,full_name,username,hash,if input.is_active.unwrap_or(true){1}else{0},worker_id,now()]).map_err(|error|ApiError::new(StatusCode::CONFLICT,format!("تعذر إنشاء المستخدم: {error}")))?;
    tx.execute(
        "INSERT INTO user_roles(user_id,role_id) VALUES(?1,?2)",
        params![id, role_id],
    )
    .map_err(ApiError::internal)?;
    tx.execute(
        "INSERT INTO user_preferences(user_id,theme,updated_at) VALUES(?1,'light',?2)",
        params![id, now()],
    )
    .map_err(ApiError::internal)?;
    insert_audit_tx(
        &tx,
        Some(&principal.id),
        "USER_CREATED",
        "user",
        Some(&id),
        "تم إنشاء مستخدم جديد",
        Some(&json!({"roleCode":input.role_code})),
    )?;
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(json!({"id":id})))
}

async fn update_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<UserUpdateInput>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "users.manage")?;
    if id == principal.id && input.is_active == Some(false) {
        return Err(ApiError::bad("لا يمكنك تعطيل حسابك الحالي"));
    }
    if id == principal.id
        && input.role_code.as_deref() != None
        && input.role_code.as_deref() != Some("manager")
    {
        return Err(ApiError::bad("لا يمكنك خفض دور حسابك الحالي"));
    }
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let existing_role: Option<String> = db
        .conn
        .query_row("SELECT r.code FROM users u JOIN user_roles ur ON ur.user_id=u.id JOIN roles r ON r.id=ur.role_id WHERE u.id=?1", [id.clone()], |row| row.get(0))
        .optional()
        .map_err(ApiError::internal)?;
    let existing_role = existing_role.ok_or_else(ApiError::not_found)?;
    let deleted: i64 = db
        .conn
        .query_row(
            "SELECT deleted_at IS NOT NULL FROM users WHERE id=?1",
            [id.clone()],
            |row| row.get(0),
        )
        .map_err(ApiError::internal)?;
    if deleted == 1 {
        return Err(ApiError::not_found());
    }
    if !principal.is_manager()
        && (existing_role == "manager" || input.role_code.as_deref() == Some("manager"))
    {
        return Err(ApiError::forbidden());
    }
    let full_name = match input.full_name {
        Some(value) => Some(trim_required(&value, "الاسم الكامل")?),
        None => None,
    };
    let username = match input.username {
        Some(value) => Some(normalized_username(&value)?),
        None => None,
    };
    let password_hash = match input.password {
        Some(value) => {
            valid_password(&value)?;
            Some(hash_password(&value)?)
        }
        None => None,
    };
    let role_id = match input.role_code.as_deref() {
        Some(code) => Some(role_id_for(&db.conn, code)?),
        None => None,
    };
    let requested_role = input.role_code.as_deref().unwrap_or(&existing_role);
    let worker_id = if requested_role == "manager" {
        None
    } else if let Some(value) = input
        .worker_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let exists: bool = db
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM workers WHERE id=?1 AND is_active=1)",
                [value],
                |row| row.get(0),
            )
            .map_err(ApiError::internal)?;
        if !exists {
            return Err(ApiError::bad("العامل المرتبط غير صالح"));
        }
        Some(value.to_owned())
    } else {
        db.conn
            .query_row(
                "SELECT worker_id FROM users WHERE id=?1",
                [id.clone()],
                |row| row.get::<_, Option<String>>(0),
            )
            .map_err(ApiError::internal)?
    };
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    if let Some(value) = full_name {
        tx.execute(
            "UPDATE users SET full_name=?1,updated_at=?2 WHERE id=?3",
            params![value, now(), id],
        )
        .map_err(ApiError::internal)?;
    }
    if let Some(value) = username {
        tx.execute(
            "UPDATE users SET username_norm=?1,updated_at=?2 WHERE id=?3",
            params![value, now(), id],
        )
        .map_err(|error| {
            ApiError::new(
                StatusCode::CONFLICT,
                format!("تعذر تعديل المستخدم: {error}"),
            )
        })?;
    }
    if let Some(value) = password_hash {
        tx.execute(
            "UPDATE users SET password_hash=?1,updated_at=?2 WHERE id=?3",
            params![value, now(), id],
        )
        .map_err(ApiError::internal)?;
    }
    if let Some(value) = input.is_active {
        tx.execute(
            "UPDATE users SET is_active=?1,updated_at=?2 WHERE id=?3",
            params![if value { 1 } else { 0 }, now(), id],
        )
        .map_err(ApiError::internal)?;
    }
    if let Some(role) = role_id {
        tx.execute("DELETE FROM user_roles WHERE user_id=?1", params![id])
            .map_err(ApiError::internal)?;
        tx.execute(
            "INSERT INTO user_roles(user_id,role_id) VALUES(?1,?2)",
            params![id, role],
        )
        .map_err(ApiError::internal)?;
    }
    tx.execute(
        "UPDATE users SET worker_id=?1,updated_at=?2 WHERE id=?3",
        params![worker_id, now(), id],
    )
    .map_err(ApiError::internal)?;
    let action = if input.is_active == Some(false) {
        "USER_DISABLED"
    } else if input.role_code.is_some() {
        "USER_ROLE_CHANGED"
    } else {
        "USER_UPDATED"
    };
    insert_audit_tx(
        &tx,
        Some(&principal.id),
        action,
        "user",
        Some(&id),
        "تم تحديث بيانات المستخدم",
        None,
    )?;
    tx.commit().map_err(ApiError::internal)?;
    if input.role_code.is_some() || input.is_active == Some(false) {
        db.conn
            .execute(
                "UPDATE sessions SET revoked_at=?1 WHERE user_id=?2",
                params![now(), id],
            )
            .map_err(ApiError::internal)?;
    }
    Ok(ok(json!({"updated":true})))
}

async fn delete_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult {
    let principal = manager(&state, &headers)?;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let (full_name, role_code, deleted_at): (String, String, Option<String>) = db.conn.query_row(
        "SELECT u.full_name,r.code,u.deleted_at FROM users u JOIN user_roles ur ON ur.user_id=u.id JOIN roles r ON r.id=ur.role_id WHERE u.id=?1",
        [id.clone()],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).optional().map_err(ApiError::internal)?.ok_or_else(ApiError::not_found)?;
    if role_code == "manager" {
        return Err(ApiError::bad("لا يمكن حذف حساب المدير الرئيسي"));
    }
    if deleted_at.is_some() {
        return Ok(ok(json!({"deleted":true,"alreadyInactive":true,"id":id})));
    }
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    let deleted_at = now();
    tx.execute(
        "UPDATE users SET is_active=0,deleted_at=?1,deleted_by=?2,updated_at=?1 WHERE id=?3",
        params![deleted_at, principal.id, id.clone()],
    )
    .map_err(ApiError::internal)?;
    tx.execute(
        "UPDATE sessions SET revoked_at=?1 WHERE user_id=?2 AND revoked_at IS NULL",
        params![deleted_at, id.clone()],
    )
    .map_err(ApiError::internal)?;
    insert_audit_tx(
        &tx,
        Some(&principal.id),
        "USER_DELETED_SAFELY",
        "user",
        Some(&id),
        "تم حذف حساب المستخدم مع الاحتفاظ بالسجلات التاريخية",
        Some(&json!({"fullName":full_name})),
    )?;
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(json!({"deleted":true,"id":id})))
}

async fn update_user_permissions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<RolePermissionsInput>,
) -> ApiResult {
    let principal = manager(&state, &headers)?;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let role_code: Option<String> = db.conn.query_row(
        "SELECT r.code FROM users u JOIN user_roles ur ON ur.user_id=u.id JOIN roles r ON r.id=ur.role_id WHERE u.id=?1",
        [id.clone()],
        |row| row.get(0),
    ).optional().map_err(ApiError::internal)?;
    let role_code = role_code.ok_or_else(ApiError::not_found)?;
    if role_code == "manager" {
        return Err(ApiError::bad("صلاحيات المدير كاملة وثابتة"));
    }
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    tx.execute(
        "INSERT INTO user_permission_profiles(user_id,updated_at) VALUES(?1,?2)
         ON CONFLICT(user_id) DO UPDATE SET updated_at=excluded.updated_at",
        params![id, now()],
    )
    .map_err(ApiError::internal)?;
    tx.execute("DELETE FROM user_permissions WHERE user_id=?1", params![id])
        .map_err(ApiError::internal)?;
    for code in input.permission_codes {
        let permission: Option<String> = tx
            .query_row("SELECT id FROM permissions WHERE code=?1", [code], |row| {
                row.get(0)
            })
            .optional()
            .map_err(ApiError::internal)?;
        let permission = permission.ok_or_else(|| ApiError::bad("توجد صلاحية غير معروفة"))?;
        tx.execute(
            "INSERT INTO user_permissions(user_id,permission_id) VALUES(?1,?2)",
            params![id, permission],
        )
        .map_err(ApiError::internal)?;
    }
    insert_audit_tx(
        &tx,
        Some(&principal.id),
        "USER_PERMISSIONS_CHANGED",
        "user",
        Some(&id),
        "تم تعديل صلاحيات الموظف",
        None,
    )?;
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(json!({"updated":true})))
}

async fn list_roles(State(state): State<AppState>, headers: HeaderMap) -> ApiResult {
    let _principal = authorize(&state, &headers, "users.manage")?;
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let mut roles = Vec::new();
    let mut statement=db.conn.prepare("SELECT id,code,name_ar,is_system FROM roles ORDER BY CASE code WHEN 'manager' THEN 0 ELSE 1 END").map_err(ApiError::internal)?;
    let rows=statement.query_map([],|row|{let id:String=row.get(0)?;let mut permissions=Vec::new();let mut ps=db.conn.prepare("SELECT p.code,p.name_ar FROM role_permissions rp JOIN permissions p ON p.id=rp.permission_id WHERE rp.role_id=?1 ORDER BY p.code").map_err(|_|rusqlite::Error::InvalidQuery)?;let prs=ps.query_map([id.clone()],|p|Ok(json!({"code":p.get::<_,String>(0)?,"name":p.get::<_,String>(1)?})))?;for permission in prs{permissions.push(permission?);}Ok(json!({"id":id,"code":row.get::<_,String>(1)?,"name":row.get::<_,String>(2)?,"isSystem":row.get::<_,i64>(3)?==1,"permissions":permissions}))}).map_err(ApiError::internal)?;
    for row in rows {
        roles.push(row.map_err(ApiError::internal)?);
    }
    Ok(ok(json!({"items":roles})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RolePermissionsInput {
    permission_codes: Vec<String>,
}

async fn update_role_permissions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<RolePermissionsInput>,
) -> ApiResult {
    let principal = manager(&state, &headers)?;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let role_code: Option<String> = db
        .conn
        .query_row("SELECT code FROM roles WHERE id=?1", [id.clone()], |row| {
            row.get(0)
        })
        .optional()
        .map_err(ApiError::internal)?;
    let role_code = role_code.ok_or_else(ApiError::not_found)?;
    if role_code == "manager" {
        return Err(ApiError::bad("صلاحيات المدير كاملة وثابتة"));
    }
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    tx.execute("DELETE FROM role_permissions WHERE role_id=?1", params![id])
        .map_err(ApiError::internal)?;
    for code in input.permission_codes {
        let permission: Option<String> = tx
            .query_row("SELECT id FROM permissions WHERE code=?1", [code], |row| {
                row.get(0)
            })
            .optional()
            .map_err(ApiError::internal)?;
        let permission = permission.ok_or_else(|| ApiError::bad("توجد صلاحية غير معروفة"))?;
        tx.execute(
            "INSERT INTO role_permissions(role_id,permission_id) VALUES(?1,?2)",
            params![id, permission],
        )
        .map_err(ApiError::internal)?;
    }
    insert_audit_tx(
        &tx,
        Some(&principal.id),
        "ROLE_PERMISSIONS_CHANGED",
        "role",
        Some(&id),
        "تم تعديل مصفوفة الصلاحيات",
        None,
    )?;
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(json!({"updated":true})))
}

async fn list_audit_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    let _principal = authorize(&state, &headers, "audit.read")?;
    let (from, to) = date_range(&query)?;
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let limit = query
        .get("limit")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(150)
        .clamp(1, 500);
    let mut items = Vec::new();
    let mut statement=db.conn.prepare("SELECT a.id,a.action,a.entity_type,a.entity_id,a.description,a.created_at,u.full_name FROM audit_logs a LEFT JOIN users u ON u.id=a.user_id WHERE a.created_at BETWEEN ?1 AND ?2 ORDER BY a.created_at DESC LIMIT ?3").map_err(ApiError::internal)?;
    let rows=statement.query_map(params![from,to,limit],|row|Ok(json!({"id":row.get::<_,String>(0)?,"action":row.get::<_,String>(1)?,"entityType":row.get::<_,String>(2)?,"entityId":row.get::<_,Option<String>>(3)?,"description":row.get::<_,String>(4)?,"createdAt":row.get::<_,String>(5)?,"userName":row.get::<_,Option<String>>(6)?}))).map_err(ApiError::internal)?;
    for row in rows {
        items.push(row.map_err(ApiError::internal)?);
    }
    Ok(ok(json!({"items":items})))
}

fn safe_backup_path(data_dir: &FsPath, requested: Option<&str>) -> Result<PathBuf, ApiError> {
    let fallback = data_dir.join("backups").join(format!(
        "alkaheli-backup-{}-{}.db",
        Utc::now().format("%Y%m%d-%H%M%S"),
        new_id()
    ));
    let path = requested
        .map(|value| PathBuf::from(value.trim()))
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or(fallback);
    if path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.eq_ignore_ascii_case("db"))
        .unwrap_or(false)
        == false
    {
        return Err(ApiError::bad("يجب أن يكون امتداد ملف النسخة الاحتياطية .db"));
    }
    let parent = path
        .parent()
        .ok_or_else(|| ApiError::bad("مسار النسخة الاحتياطية غير صالح"))?;
    fs::create_dir_all(parent).map_err(ApiError::internal)?;
    Ok(path)
}

fn vacuum_into(conn: &Connection, path: &FsPath) -> Result<(), ApiError> {
    if path.exists() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "ملف النسخة الاحتياطية موجود بالفعل",
        ));
    }
    let escaped = path.to_string_lossy().replace('\'', "''");
    conn.execute_batch(&format!("VACUUM INTO '{escaped}'"))
        .map_err(ApiError::internal)?;
    Database::verify_backup(path).map_err(ApiError::internal)
}

#[derive(Deserialize)]
struct BackupInput {
    path: Option<String>,
}

async fn list_backups(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    let _principal = authorize(&state, &headers, "backup.manage")?;
    let (from, to) = date_range(&query)?;
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let mut items = Vec::new();
    let mut statement=db.conn.prepare("SELECT id,backup_path,created_at FROM backup_history WHERE status='completed' AND created_at BETWEEN ?1 AND ?2 ORDER BY created_at DESC LIMIT 100").map_err(ApiError::internal)?;
    let rows = statement
        .query_map(params![from, to], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(ApiError::internal)?;
    let mut stale = Vec::new();
    for row in rows {
        let (id, path, created_at) = row.map_err(ApiError::internal)?;
        let Some(path) = path.map(PathBuf::from) else {
            stale.push(id);
            continue;
        };
        if !path.is_file() || Database::verify_backup(&path).is_err() {
            stale.push(id);
            continue;
        }
        let size = fs::metadata(&path).map_err(ApiError::internal)?.len();
        items.push(json!({"id":id.clone(),"path":path.to_string_lossy(),"createdAt":created_at,"sizeBytes":size,"downloadUrl":format!("/api/backups/{id}/download")}));
    }
    drop(statement);
    for id in stale {
        db.conn
            .execute("DELETE FROM backup_history WHERE id=?1", [id])
            .map_err(ApiError::internal)?;
    }
    Ok(ok(json!({"items":items})))
}

async fn create_backup(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<BackupInput>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "backup.manage")?;
    let path = safe_backup_path(&state.data_dir, input.path.as_deref())?;
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    vacuum_into(&db.conn, &path)?;
    let id = new_id();
    db.conn.execute("INSERT INTO backup_history(id,backup_path,created_by,created_at,status,notes) VALUES(?1,?2,?3,?4,'completed',NULL)",params![id,path.to_string_lossy().to_string(),principal.id,now()]).map_err(ApiError::internal)?;
    insert_audit(
        &db.conn,
        Some(&principal.id),
        "BACKUP_CREATED",
        "backup",
        Some(&id),
        "تم إنشاء نسخة احتياطية آمنة لقاعدة البيانات",
        None,
    )
    .map_err(ApiError::internal)?;
    let size = fs::metadata(&path).map_err(ApiError::internal)?.len();
    Ok(ok(
        json!({"id":id.clone(),"path":path.to_string_lossy(),"createdAt":now(),"sizeBytes":size,"downloadUrl":format!("/api/backups/{id}/download")}),
    ))
}

fn backup_path_for_id(conn: &Connection, id: &str) -> Result<PathBuf, ApiError> {
    let path: Option<Option<String>> = conn
        .query_row(
            "SELECT backup_path FROM backup_history WHERE id=?1 AND status='completed'",
            [id],
            |row| row.get(0),
        )
        .optional()
        .map_err(ApiError::internal)?;
    path.flatten()
        .map(PathBuf::from)
        .ok_or_else(ApiError::not_found)
}

async fn download_backup(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let _principal = authorize(&state, &headers, "backup.manage")?;
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let path = backup_path_for_id(&db.conn, &id)?;
    if !path.is_file() || Database::verify_backup(&path).is_err() {
        return Err(ApiError::not_found());
    }
    let bytes = fs::read(&path).map_err(ApiError::internal)?;
    let filename = path
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| value.is_ascii())
        .unwrap_or("alkaheli-backup.db");
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            ),
        ],
        bytes,
    )
        .into_response())
}

async fn delete_backup(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "backup.manage")?;
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let path = backup_path_for_id(&db.conn, &id)?;
    if path.is_file() {
        fs::remove_file(&path).map_err(ApiError::internal)?;
    }
    db.conn
        .execute("DELETE FROM backup_history WHERE id=?1", [id.clone()])
        .map_err(ApiError::internal)?;
    insert_audit(
        &db.conn,
        Some(&principal.id),
        "BACKUP_DELETED",
        "backup",
        Some(&id),
        "تم حذف ملف نسخة احتياطية نهائيًا",
        None,
    )
    .map_err(ApiError::internal)?;
    Ok(ok(json!({"deleted":true})))
}

async fn export_backup(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<BackupInput>,
) -> ApiResult {
    let _principal = authorize(&state, &headers, "backup.manage")?;
    let target = safe_backup_path(&state.data_dir, input.path.as_deref())?;
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let source = backup_path_for_id(&db.conn, &id)?;
    if !source.is_file() || Database::verify_backup(&source).is_err() {
        return Err(ApiError::not_found());
    }
    if source.canonicalize().ok() == target.canonicalize().ok() {
        return Err(ApiError::bad("اختر موقعًا مختلفًا لحفظ النسخة"));
    }
    fs::copy(&source, &target).map_err(ApiError::internal)?;
    Database::verify_backup(&target).map_err(|_| ApiError::bad("تعذر التحقق من الملف المنزّل"))?;
    Ok(ok(json!({"exported":true,"path":target.to_string_lossy()})))
}

#[derive(Deserialize)]
struct RestoreInput {
    path: String,
    confirmation: String,
}

async fn restore_backup(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<RestoreInput>,
) -> ApiResult {
    let principal = authorize(&state, &headers, "backup.manage")?;
    if input.confirmation.trim() != "RESTORE" {
        return Err(ApiError::bad(
            "اكتب RESTORE لتأكيد استعادة النسخة الاحتياطية",
        ));
    }
    let source = PathBuf::from(input.path.trim());
    let payload = apply_restore(&state, &principal, &source)?;
    Ok(ok(payload))
}

async fn restore_backup_upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> ApiResult {
    let principal = authorize(&state, &headers, "backup.manage")?;
    let upload_dir = state.data_dir.join("restore-uploads");
    fs::create_dir_all(&upload_dir).map_err(ApiError::internal)?;
    let staged = upload_dir.join(format!("restore-upload-{}.db", new_id()));
    let mut saved = false;
    let mut confirmed = false;
    while let Some(field) = multipart.next_field().await.map_err(ApiError::internal)? {
        match field.name() {
            Some("confirmation") => {
                confirmed = field.text().await.map_err(ApiError::internal)?.trim() == "RESTORE";
            }
            Some("backup") => {
                let bytes = field.bytes().await.map_err(ApiError::internal)?;
                if bytes.len() > 100 * 1024 * 1024 {
                    return Err(ApiError::bad("حجم النسخة الاحتياطية يتجاوز الحد المسموح"));
                }
                fs::write(&staged, &bytes).map_err(ApiError::internal)?;
                saved = true;
            }
            _ => {}
        }
    }
    if !saved {
        return Err(ApiError::bad("اختر ملف نسخة احتياطية صالحًا"));
    }
    if !confirmed {
        let _ = fs::remove_file(&staged);
        return Err(ApiError::bad("لم يتم تأكيد استعادة النسخة الاحتياطية"));
    }
    let result = apply_restore(&state, &principal, &staged);
    let _ = fs::remove_file(&staged);
    Ok(ok(result?))
}

fn apply_restore(
    state: &AppState,
    _principal: &Principal,
    source: &FsPath,
) -> Result<Value, ApiError> {
    if !source.is_file() {
        return Err(ApiError::bad("ملف النسخة الاحتياطية غير موجود"));
    }
    Database::verify_backup(source).map_err(|_| ApiError::bad("ملف النسخة غير صالح أو تالف"))?;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let current_path = db.path.clone();
    if source.canonicalize().ok() == current_path.canonicalize().ok() {
        return Err(ApiError::bad("لا يمكن استعادة قاعدة البيانات نفسها"));
    }
    let emergency = state.data_dir.join("backups").join(format!(
        "pre-restore-{}.db",
        Utc::now().format("%Y%m%d-%H%M%S")
    ));
    fs::create_dir_all(emergency.parent().unwrap()).map_err(ApiError::internal)?;
    vacuum_into(&db.conn, &emergency)?;
    let staged = state
        .data_dir
        .join(format!("restore-staged-{}.db", new_id()));
    fs::copy(&source, &staged).map_err(ApiError::internal)?;
    if let Err(_) = Database::verify_backup(&staged) {
        let _ = fs::remove_file(&staged);
        return Err(ApiError::bad("تعذر التحقق من النسخة قبل الاستعادة"));
    }
    let mut preserved_backups = Vec::new();
    {
        let mut statement = db.conn.prepare(
            "SELECT id,backup_path,created_at,notes FROM backup_history WHERE status='completed'",
        ).map_err(ApiError::internal)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(ApiError::internal)?;
        for row in rows {
            let item = row.map_err(ApiError::internal)?;
            if item
                .1
                .as_ref()
                .is_some_and(|path| FsPath::new(path).is_file())
            {
                preserved_backups.push(item);
            }
        }
    }
    // Replace only after a verified staging copy and a fresh emergency backup exist.
    let old_conn = std::mem::replace(
        &mut db.conn,
        Connection::open_in_memory().map_err(ApiError::internal)?,
    );
    drop(old_conn);
    let _ = fs::remove_file(current_path.with_extension("db-wal"));
    let _ = fs::remove_file(current_path.with_extension("db-shm"));
    fs::copy(&staged, &current_path).map_err(ApiError::internal)?;
    let _ = fs::remove_file(&staged);
    let reopened = Database::open(&state.data_dir).map_err(ApiError::internal)?;
    *db = reopened;
    for (id, path, created_at, notes) in preserved_backups {
        db.conn.execute(
            "INSERT OR IGNORE INTO backup_history(id,backup_path,created_by,created_at,status,notes) VALUES(?1,?2,NULL,?3,'completed',?4)",
            params![id,path,created_at,notes],
        ).map_err(ApiError::internal)?;
    }
    db.conn
        .execute("DELETE FROM sessions", [])
        .map_err(ApiError::internal)?;
    let source_text = source.to_string_lossy().to_string();
    let history_id = new_id();
    insert_audit(
        &db.conn,
        None,
        "BACKUP_RESTORED",
        "backup",
        Some(&history_id),
        "تمت استعادة نسخة احتياطية وإبطال كل الجلسات",
        Some(&source_text),
    )
    .map_err(ApiError::internal)?;
    Ok(
        json!({"restored":true,"emergencyBackupPath":emergency.to_string_lossy(),"reauthenticationRequired":true}),
    )
}

#[cfg(test)]
mod paid_cars_tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::Request,
        Router,
    };
    use tower::ServiceExt;

    async fn request_json(
        app: &Router,
        method: Method,
        uri: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(token) = token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        if body.is_some() {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
        }
        let request = builder
            .body(Body::from(
                body.map(|value| value.to_string()).unwrap_or_default(),
            ))
            .expect("valid test request");
        let response = app.clone().oneshot(request).await.expect("router response");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body");
        let payload = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, payload)
    }

    fn response_data(payload: &Value) -> &Value {
        payload.get("data").expect("response data")
    }

    #[tokio::test]
    async fn paid_cars_are_persistent_scoped_and_counted_once() {
        let data_dir = std::env::temp_dir().join(format!("alkaheli-paid-cars-test-{}", new_id()));
        let state = crate::create_state(data_dir.clone()).expect("test database");
        let app = build_router(state.clone());

        let (status, manager_setup) = request_json(
            &app,
            Method::POST,
            "/api/setup/initial-manager",
            None,
            Some(json!({"fullName":"مدير الاختبار","username":"manager-test","password":"StrongPass123!"})),
        ).await;
        assert_eq!(status, StatusCode::OK);
        let manager_token = response_data(&manager_setup)["token"]
            .as_str()
            .unwrap()
            .to_owned();

        let (status, worker) = request_json(
            &app,
            Method::POST,
            "/api/workers",
            Some(&manager_token),
            Some(json!({"fullName":"عامل الاختبار","commissionBpsOverride":5000})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let worker_id = response_data(&worker)["id"].as_str().unwrap().to_owned();

        for (full_name, username) in [
            ("الموظف الأول", "employee-a"),
            ("الموظف الثاني", "employee-b"),
        ] {
            let (status, _) = request_json(
                &app,
                Method::POST,
                "/api/users",
                Some(&manager_token),
                Some(json!({"fullName":full_name,"username":username,"password":"EmployeePass123!","roleCode":"employee"})),
            ).await;
            assert_eq!(status, StatusCode::OK);
        }

        let mut employee_tokens = Vec::new();
        for username in ["employee-a", "employee-b"] {
            let (status, login) = request_json(
                &app,
                Method::POST,
                "/api/auth/login",
                None,
                Some(json!({"username":username,"password":"EmployeePass123!"})),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            employee_tokens.push(response_data(&login)["token"].as_str().unwrap().to_owned());
        }

        let mut wash_ids = Vec::new();
        for (index, (token, price)) in employee_tokens.iter().zip(["100", "80"]).enumerate() {
            let (status, wash) = request_json(
                &app,
                Method::POST,
                "/api/washes",
                Some(token),
                Some(json!({
                    "vehicleMake":"Toyota","vehicleModel":format!("Test {index}"),"price":price,
                    "workerId":worker_id,"paymentType":"cash","occurredAt":now(),"clientRequestId":new_id()
                })),
            ).await;
            assert_eq!(status, StatusCode::OK);
            wash_ids.push(response_data(&wash)["id"].as_str().unwrap().to_owned());
        }

        for (token, wash_id) in employee_tokens.iter().zip(wash_ids.iter()) {
            let (status, result) = request_json(
                &app,
                Method::PATCH,
                &format!("/api/washes/{wash_id}/paid"),
                Some(token),
                Some(json!({"isPaid":true})),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(response_data(&result)["wash"]["isPaid"], true);
        }

        let (status, forbidden) = request_json(
            &app,
            Method::PATCH,
            &format!("/api/washes/{}/paid", wash_ids[1]),
            Some(&employee_tokens[0]),
            Some(json!({"isPaid":false})),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{forbidden}");

        for (token, expected_settlement) in employee_tokens.iter().zip([100_000, 80_000]) {
            let (status, list) =
                request_json(&app, Method::GET, "/api/paid-cars", Some(token), None).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(response_data(&list)["items"].as_array().unwrap().len(), 1);
            assert_eq!(response_data(&list)["settlementMilli"], expected_settlement);
            let (status, unpaid) =
                request_json(&app, Method::GET, "/api/washes", Some(token), None).await;
            assert_eq!(status, StatusCode::OK);
            assert!(response_data(&unpaid)["items"]
                .as_array()
                .unwrap()
                .is_empty());
            let (_, dashboard) =
                request_json(&app, Method::GET, "/api/dashboard", Some(token), None).await;
            assert!(response_data(&dashboard)["recentWashes"]
                .as_array()
                .unwrap()
                .is_empty());
        }

        let (_, employee_dashboard) = request_json(
            &app,
            Method::GET,
            "/api/dashboard",
            Some(&employee_tokens[0]),
            None,
        )
        .await;
        assert!(response_data(&employee_dashboard)
            .get("settlementMilli")
            .is_none());

        let (_, manager_list) = request_json(
            &app,
            Method::GET,
            "/api/paid-cars",
            Some(&manager_token),
            None,
        )
        .await;
        assert_eq!(
            response_data(&manager_list)["items"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(response_data(&manager_list)["settlementMilli"], 180_000);

        // Settlement follows the full paid wash price immediately, never the commission.
        let (status, _) = request_json(
            &app,
            Method::PATCH,
            &format!("/api/washes/{}", wash_ids[0]),
            Some(&employee_tokens[0]),
            Some(json!({
                "vehicleMake":"Toyota","vehicleModel":"Test 0","price":"120",
                "workerId":worker_id,"paymentType":"cash","occurredAt":now()
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (_, employee_after_price_edit) = request_json(
            &app,
            Method::GET,
            "/api/paid-cars",
            Some(&employee_tokens[0]),
            None,
        )
        .await;
        assert_eq!(
            response_data(&employee_after_price_edit)["settlementMilli"],
            120_000
        );
        let (_, manager_after_price_edit) = request_json(
            &app,
            Method::GET,
            "/api/paid-cars",
            Some(&manager_token),
            None,
        )
        .await;
        assert_eq!(
            response_data(&manager_after_price_edit)["settlementMilli"],
            200_000
        );

        // A manager can revert and restore any paid operation without duplicating it.
        let (status, _) = request_json(
            &app,
            Method::PATCH,
            &format!("/api/washes/{}/paid", wash_ids[0]),
            Some(&manager_token),
            Some(json!({"isPaid":false})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (_, employee_after_manager_revert) = request_json(
            &app,
            Method::GET,
            "/api/paid-cars",
            Some(&employee_tokens[0]),
            None,
        )
        .await;
        assert!(response_data(&employee_after_manager_revert)["items"]
            .as_array()
            .unwrap()
            .is_empty());
        let (status, _) = request_json(
            &app,
            Method::PATCH,
            &format!("/api/washes/{}/paid", wash_ids[0]),
            Some(&manager_token),
            Some(json!({"isPaid":true})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // Repeating the same desired status is idempotent and cannot duplicate the operation.
        let _ = request_json(
            &app,
            Method::PATCH,
            &format!("/api/washes/{}/paid", wash_ids[1]),
            Some(&employee_tokens[1]),
            Some(json!({"isPaid":true})),
        )
        .await;
        let (_, unchanged_list) = request_json(
            &app,
            Method::GET,
            "/api/paid-cars",
            Some(&manager_token),
            None,
        )
        .await;
        assert_eq!(
            response_data(&unchanged_list)["items"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(response_data(&unchanged_list)["settlementMilli"], 200_000);

        let (status, reverted) = request_json(
            &app,
            Method::PATCH,
            &format!("/api/washes/{}/paid", wash_ids[1]),
            Some(&employee_tokens[1]),
            Some(json!({"isPaid":false})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response_data(&reverted)["settlementMilli"], 0);
        let (_, manager_after_revert) = request_json(
            &app,
            Method::GET,
            "/api/paid-cars",
            Some(&manager_token),
            None,
        )
        .await;
        assert_eq!(
            response_data(&manager_after_revert)["items"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            response_data(&manager_after_revert)["settlementMilli"],
            120_000
        );
        let (_, employee_unpaid) = request_json(
            &app,
            Method::GET,
            "/api/washes",
            Some(&employee_tokens[1]),
            None,
        )
        .await;
        assert_eq!(
            response_data(&employee_unpaid)["items"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            response_data(&employee_unpaid)["items"][0]["id"],
            wash_ids[1]
        );

        drop(app);
        drop(state);
        let restarted = crate::create_state(data_dir.clone()).expect("reopened database");
        let restarted_app = build_router(restarted.clone());
        let (_, relogin) = request_json(
            &restarted_app,
            Method::POST,
            "/api/auth/login",
            None,
            Some(json!({"username":"employee-a","password":"EmployeePass123!"})),
        )
        .await;
        let relogin_token = response_data(&relogin)["token"].as_str().unwrap();
        let (_, persisted) = request_json(
            &restarted_app,
            Method::GET,
            "/api/paid-cars",
            Some(relogin_token),
            None,
        )
        .await;
        assert_eq!(
            response_data(&persisted)["items"].as_array().unwrap().len(),
            1
        );
        assert_eq!(response_data(&persisted)["settlementMilli"], 120_000);
        let (_, persisted_unpaid) = request_json(
            &restarted_app,
            Method::GET,
            "/api/washes",
            Some(relogin_token),
            None,
        )
        .await;
        assert!(response_data(&persisted_unpaid)["items"]
            .as_array()
            .unwrap()
            .is_empty());

        drop(restarted_app);
        drop(restarted);
        let _ = std::fs::remove_dir_all(data_dir);
    }
}
