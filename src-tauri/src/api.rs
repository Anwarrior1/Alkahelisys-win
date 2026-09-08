use crate::db::{default_commission_bps, insert_audit, new_id, now, Database, MONEY_SCALE};
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post, put},
    Json, Router,
};
use chrono::{DateTime, Datelike, Duration, NaiveDate, SecondsFormat, Utc};
use rand::RngCore;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    io::{BufReader, BufWriter, Read, Write},
    ops::Deref,
    path::{Path as FsPath, PathBuf},
    sync::{Arc, Mutex, RwLock, RwLockReadGuard},
};
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;
use tower_http::{
    cors::{AllowOrigin, CorsLayer},
    trace::TraceLayer,
};

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Mutex<Database>>,
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    pub read_gate: Arc<RwLock<()>>,
}

struct ReadDatabase<'a> {
    database: Database,
    _restore_guard: RwLockReadGuard<'a, ()>,
}

impl Deref for ReadDatabase<'_> {
    type Target = Database;

    fn deref(&self) -> &Self::Target {
        &self.database
    }
}

impl AppState {
    fn read_db(&self) -> Result<ReadDatabase<'_>, ApiError> {
        let restore_guard = self
            .read_gate
            .read()
            .map_err(|_| ApiError::internal("قفل قراءة قاعدة البيانات"))?;
        let database = Database::open_read_only(&self.db_path).map_err(ApiError::internal)?;
        Ok(ReadDatabase {
            database,
            _restore_guard: restore_guard,
        })
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Principal {
    id: String,
    full_name: String,
    username: String,
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
type LedgerEntry<'a> = (&'a str, &'a str, i64, Option<&'a str>, Option<&'a str>);

fn ok(data: Value) -> Json<Value> {
    Json(json!({ "data": data }))
}

fn operation_request_id(value: Option<String>) -> Result<Option<String>, ApiError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let normalized = value.trim();
    if normalized.is_empty()
        || normalized.chars().count() > 100
        || normalized.chars().any(char::is_control)
    {
        return Err(ApiError::bad("معرف الطلب غير صالح"));
    }
    Ok(Some(normalized.to_owned()))
}

fn replay_operation(
    conn: &Connection,
    actor_id: &str,
    operation_type: &str,
    request_id: Option<&str>,
) -> Result<Option<Value>, ApiError> {
    let Some(request_id) = request_id else {
        return Ok(None);
    };
    let stored: Option<String> = conn
        .query_row(
            "SELECT response_json FROM operation_requests
             WHERE actor_id=?1 AND operation_type=?2 AND request_id=?3",
            params![actor_id, operation_type, request_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(ApiError::internal)?;
    stored
        .map(|value| serde_json::from_str(&value).map_err(ApiError::internal))
        .transpose()
}

fn record_operation(
    tx: &Transaction<'_>,
    actor_id: &str,
    operation_type: &str,
    request_id: Option<&str>,
    response: &Value,
) -> Result<(), ApiError> {
    let Some(request_id) = request_id else {
        return Ok(());
    };
    let response_json = serde_json::to_string(response).map_err(ApiError::internal)?;
    tx.execute(
        "INSERT INTO operation_requests(actor_id,operation_type,request_id,response_json,created_at)
         VALUES(?1,?2,?3,?4,?5)",
        params![actor_id, operation_type, request_id, response_json, now()],
    )
    .map_err(ApiError::internal)?;
    Ok(())
}

/// Runs blocking filesystem or backup-file work on Tokio's blocking pool so it neither occupies an
/// async worker thread nor forces the caller to hold the live database mutex while it executes.
///
/// The global `Arc<Mutex<Database>>` guard is deliberately not `Send`, so any handler that awaits
/// this helper is required by the compiler to have released the lock first. That property is what
/// keeps long filesystem operations out of the database critical section.
async fn blocking<F, T>(task: F) -> Result<T, ApiError>
where
    F: FnOnce() -> Result<T, ApiError> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(task)
        .await
        .map_err(ApiError::internal)?
}

pub fn build_router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list([
            HeaderValue::from_static("http://tauri.localhost"),
            HeaderValue::from_static("https://tauri.localhost"),
            HeaderValue::from_static("tauri://localhost"),
            HeaderValue::from_static("http://localhost:1420"),
            HeaderValue::from_static("http://127.0.0.1:1420"),
        ]))
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
        .route(
            "/api/preferences/financial-report-card-order",
            get(financial_report_card_order).put(update_financial_report_card_order),
        )
        .route(
            "/api/preferences/dashboard-card-order",
            get(dashboard_card_order).put(update_dashboard_card_order),
        )
        .route("/api/dashboard", get(dashboard))
        .route("/api/washes", get(list_washes).post(create_wash))
        .route("/api/washes/:id", patch(update_wash))
        .route("/api/washes/:id/overnight", patch(set_wash_overnight))
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
        .route(
            "/api/workers/:id/withdrawals-returns/settle",
            post(settle_worker_withdrawal_returns),
        )
        .route(
            "/api/workers/:id/withdrawals-returns/reset",
            post(reset_worker_financial_records),
        )
        .route(
            "/api/workers/:worker_id/withdrawals-returns/:movement_id",
            patch(update_worker_deduction_payment).delete(delete_worker_withdrawal_return),
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
        .route(
            "/api/payroll/employees/:id/salary",
            put(set_employee_salary),
        )
        .route(
            "/api/payroll/employees/:id",
            delete(delete_payroll_employee),
        )
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
        .route(
            "/api/backups/restore-upload",
            post(restore_backup_upload).layer(DefaultBodyLimit::disable()),
        )
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
    let length = value.chars().count();
    if !(10..=128).contains(&length) {
        return Err(ApiError::bad("يجب أن تتكون كلمة المرور من 10 إلى 128 حرفًا"));
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

async fn hash_password_blocking(password: String) -> Result<String, ApiError> {
    blocking(move || hash_password(&password)).await
}

async fn verify_password_blocking(
    password: String,
    encoded_hash: String,
) -> Result<bool, ApiError> {
    blocking(move || {
        let parsed_hash = PasswordHash::new(&encoded_hash).map_err(ApiError::internal)?;
        Ok(Argon2::default()
            .verify_password(password.as_bytes(), &parsed_hash)
            .is_ok())
    })
    .await
}

fn principal_from_headers(state: &AppState, headers: &HeaderMap) -> Result<Principal, ApiError> {
    let raw = headers
        .get(header::AUTHORIZATION)
        .and_then(|header_value| header_value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or_else(ApiError::unauthorized)?;
    let hash = token_hash(raw);
    let db = state.read_db()?;
    let row = db.conn.query_row(
        "SELECT u.id, u.full_name, u.username_norm, r.code, r.name_ar, COALESCE(p.theme, 'light'), s.expires_at
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
                id: row.get(0)?, full_name: row.get(1)?, username: row.get(2)?, role_code: row.get(3)?, role_name: row.get(4)?, theme: row.get(5)?, permissions: Vec::new(),
            },
            row.get::<_, String>(6)?,
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

fn authorize_section(
    state: &AppState,
    headers: &HeaderMap,
    section: &str,
    permission: &str,
) -> Result<Principal, ApiError> {
    let principal = authorize(state, headers, permission)?;
    if !principal.has_permission(section) {
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
    let db = state.read_db()?;
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
    let hash = hash_password_blocking(input.password.clone()).await?;
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
    if input.password.chars().count() > 128 {
        return Err(ApiError::unauthorized());
    }
    let user = {
        let db = state.read_db()?;
        db.conn.query_row(
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
        ).optional().map_err(ApiError::internal)?
    };
    let (user_id, _full_name, password_hash, active) = user.ok_or_else(ApiError::unauthorized)?;
    if active != 1 {
        return Err(ApiError::new(StatusCode::FORBIDDEN, "هذا الحساب معطّل"));
    }
    if !verify_password_blocking(input.password, password_hash.clone()).await? {
        return Err(ApiError::unauthorized());
    }
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    // Password verification is deliberately outside the writer lock. Rechecking the credential
    // state before creating the session preserves the behavior if an administrator changed or
    // disabled the account while Argon2 was running.
    let current: Option<(String, i64)> = db
        .conn
        .query_row(
            "SELECT password_hash,is_active FROM users WHERE id=?1 AND deleted_at IS NULL",
            [&user_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(ApiError::internal)?;
    if current.as_ref() != Some(&(password_hash, 1)) {
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
        "SELECT u.id, u.full_name, u.username_norm, r.code, r.name_ar, COALESCE(p.theme,'light')
         FROM users u JOIN user_roles ur ON ur.user_id=u.id JOIN roles r ON r.id=ur.role_id
         LEFT JOIN user_preferences p ON p.user_id=u.id WHERE u.id=?1
         ORDER BY CASE r.code WHEN 'manager' THEN 0 ELSE 1 END LIMIT 1",
        [user_id],
        |row| {
            Ok(Principal {
                id: row.get(0)?,
                full_name: row.get(1)?,
                username: row.get(2)?,
                role_code: row.get(3)?,
                role_name: row.get(4)?,
                theme: row.get(5)?,
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
    let content_type: Option<String> = {
        let db = state.read_db()?;
        db.conn
            .query_row(
                "SELECT content_type FROM user_profile_pictures WHERE user_id=?1",
                [user_id.clone()],
                |row| row.get(0),
            )
            .optional()
            .map_err(ApiError::internal)?
    };
    let Some(content_type) = content_type else {
        return Err(ApiError::not_found());
    };
    let path = profile_picture_path(&state.data_dir, &user_id);
    let bytes = blocking(move || fs::read(path).map_err(|_| ApiError::not_found())).await?;
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
    let path = profile_picture_path(&state.data_dir, &user_id);
    let temp_path = picture_dir.join(format!(".{user_id}.upload"));
    let write_path = path.clone();
    blocking(move || {
        fs::create_dir_all(&picture_dir).map_err(ApiError::internal)?;
        fs::write(&temp_path, &bytes).map_err(ApiError::internal)?;
        if let Err(error) = fs::rename(&temp_path, &write_path) {
            let _ = fs::remove_file(&temp_path);
            return Err(ApiError::internal(error));
        }
        Ok(())
    })
    .await?;
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
    blocking(move || {
        if path.is_file() {
            fs::remove_file(&path).map_err(ApiError::internal)?;
        }
        Ok(())
    })
    .await?;
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

async fn financial_report_card_order(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult {
    let principal = principal_from_headers(&state, &headers)?;
    let db = state.read_db()?;
    let raw = db
        .conn
        .query_row(
            "SELECT financial_report_card_order_json FROM user_preferences WHERE user_id=?1",
            [&principal.id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(ApiError::internal)?
        .flatten();
    let card_order = raw
        .and_then(|value| serde_json::from_str::<Vec<String>>(&value).ok())
        .unwrap_or_default();
    Ok(ok(json!({"cardOrder": card_order})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CardOrderInput {
    card_order: Vec<String>,
}

async fn update_financial_report_card_order(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<CardOrderInput>,
) -> ApiResult {
    let principal = principal_from_headers(&state, &headers)?;
    if input.card_order.len() > 64
        || input.card_order.iter().any(|id| {
            id.is_empty()
                || id.len() > 64
                || !id
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '_')
        })
    {
        return Err(ApiError::bad("ترتيب بطاقات التقرير غير صالح"));
    }
    let mut unique = std::collections::HashSet::new();
    if input.card_order.iter().any(|id| !unique.insert(id)) {
        return Err(ApiError::bad("ترتيب بطاقات التقرير يحتوي على تكرار"));
    }
    let serialized = serde_json::to_string(&input.card_order).map_err(ApiError::internal)?;
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    db.conn
        .execute(
            "INSERT INTO user_preferences(user_id, theme, financial_report_card_order_json, updated_at)
             VALUES(?1, 'light', ?2, ?3)
             ON CONFLICT(user_id) DO UPDATE SET
                financial_report_card_order_json=excluded.financial_report_card_order_json,
                updated_at=excluded.updated_at",
            params![principal.id, serialized, now()],
        )
        .map_err(ApiError::internal)?;
    Ok(ok(json!({"cardOrder": input.card_order})))
}

async fn dashboard_card_order(State(state): State<AppState>, headers: HeaderMap) -> ApiResult {
    let principal = principal_from_headers(&state, &headers)?;
    let db = state.read_db()?;
    let raw = db
        .conn
        .query_row(
            "SELECT dashboard_card_order_json FROM user_preferences WHERE user_id=?1",
            [&principal.id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(ApiError::internal)?
        .flatten();
    let card_order = raw
        .and_then(|value| serde_json::from_str::<Vec<String>>(&value).ok())
        .unwrap_or_default();
    Ok(ok(json!({"cardOrder": card_order})))
}

async fn update_dashboard_card_order(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<CardOrderInput>,
) -> ApiResult {
    let principal = principal_from_headers(&state, &headers)?;
    if input.card_order.len() > 64
        || input.card_order.iter().any(|id| {
            id.is_empty()
                || id.len() > 64
                || !id
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '_')
        })
    {
        return Err(ApiError::bad("ترتيب بطاقات لوحة المتابعة غير صالح"));
    }
    let mut unique = std::collections::HashSet::new();
    if input.card_order.iter().any(|id| !unique.insert(id)) {
        return Err(ApiError::bad("ترتيب بطاقات لوحة المتابعة يحتوي على تكرار"));
    }
    let serialized = serde_json::to_string(&input.card_order).map_err(ApiError::internal)?;
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    db.conn
        .execute(
            "INSERT INTO user_preferences(user_id, theme, dashboard_card_order_json, updated_at)
             VALUES(?1, 'light', ?2, ?3)
             ON CONFLICT(user_id) DO UPDATE SET
                dashboard_card_order_json=excluded.dashboard_card_order_json,
                updated_at=excluded.updated_at",
            params![principal.id, serialized, now()],
        )
        .map_err(ApiError::internal)?;
    Ok(ok(json!({"cardOrder": input.card_order})))
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

fn canonical_timestamp(value: &str, error_message: &str) -> Result<String, ApiError> {
    DateTime::parse_from_rfc3339(value.trim())
        .map(|timestamp| {
            timestamp
                .with_timezone(&Utc)
                .to_rfc3339_opts(SecondsFormat::Millis, true)
        })
        .map_err(|_| ApiError::bad(error_message))
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

fn business_month_range(selected: NaiveDate) -> Result<(String, String), ApiError> {
    let start_date = NaiveDate::from_ymd_opt(selected.year(), selected.month(), 1)
        .ok_or_else(|| ApiError::bad("الشهر المحدد غير صالح"))?;
    let next_month = if selected.month() == 12 {
        NaiveDate::from_ymd_opt(selected.year() + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(selected.year(), selected.month() + 1, 1)
    }
    .ok_or_else(|| ApiError::bad("الشهر المحدد غير صالح"))?;
    let start = start_date
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| ApiError::bad("الشهر المحدد غير صالح"))?
        - Duration::hours(BUSINESS_UTC_OFFSET_HOURS);
    let end = next_month
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| ApiError::bad("الشهر المحدد غير صالح"))?
        - Duration::hours(BUSINESS_UTC_OFFSET_HOURS)
        - Duration::milliseconds(1);
    Ok((
        format!("{}Z", start.format("%Y-%m-%dT%H:%M:%S%.3f")),
        format!("{}Z", end.format("%Y-%m-%dT%H:%M:%S%.3f")),
    ))
}

fn business_month_range_from_key(month: &str) -> Result<(String, String), ApiError> {
    let selected = NaiveDate::parse_from_str(&format!("{month}-01"), "%Y-%m-%d")
        .map_err(|_| ApiError::bad("الشهر المحدد غير صالح"))?;
    business_month_range(selected)
}

fn date_range(query: &HashMap<String, String>) -> Result<(String, String), ApiError> {
    if let Some(selected) = selected_business_date(query)? {
        return business_day_range(selected);
    }
    if !query.contains_key("from") && !query.contains_key("to") {
        return business_day_range(business_today());
    }
    let from = match query.get("from") {
        Some(value) => canonical_timestamp(value, "بداية الفترة غير صالحة")?,
        None => "0000-01-01T00:00:00Z".into(),
    };
    let to = match query.get("to") {
        Some(value) => canonical_timestamp(value, "نهاية الفترة غير صالحة")?,
        None => "9999-12-31T23:59:59Z".into(),
    };
    if from > to {
        return Err(ApiError::bad("تاريخ البداية يجب أن يسبق تاريخ النهاية"));
    }
    Ok((from, to))
}

const HISTORY_PAGE_SIZE: i64 = 100;
const HISTORY_MAX_PAGE_SIZE: i64 = 300;

fn history_limit(query: &HashMap<String, String>) -> i64 {
    history_limit_named(query, "limit")
}

fn history_limit_named(query: &HashMap<String, String>, limit_key: &str) -> i64 {
    query
        .get(limit_key)
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(HISTORY_PAGE_SIZE)
        .clamp(1, HISTORY_MAX_PAGE_SIZE)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PageCursor {
    timestamp: String,
    id: String,
    scope: String,
}

fn cursor_scope(parts: &[&str]) -> String {
    parts.join("\u{1f}")
}

fn history_cursor(
    query: &HashMap<String, String>,
    key: &str,
    scope: &str,
) -> Result<Option<PageCursor>, ApiError> {
    let Some(encoded) = query.get(key).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let cursor: PageCursor =
        serde_json::from_str(encoded).map_err(|_| ApiError::bad("مؤشر الصفحة غير صالح"))?;
    if cursor.scope != scope
        || cursor.id.is_empty()
        || cursor.id.chars().count() > 120
        || cursor.id.chars().any(char::is_control)
    {
        return Err(ApiError::bad("مؤشر الصفحة لا يطابق نطاق البحث الحالي"));
    }
    let canonical = canonical_timestamp(&cursor.timestamp, "مؤشر الصفحة غير صالح")?;
    if canonical != cursor.timestamp {
        return Err(ApiError::bad("مؤشر الصفحة غير صالح"));
    }
    Ok(Some(cursor))
}

fn cursor_boundary(cursor: Option<&PageCursor>) -> (String, String) {
    cursor
        .map(|value| (value.timestamp.clone(), value.id.clone()))
        .unwrap_or_else(|| ("~".to_owned(), "~".to_owned()))
}

fn value_at_path<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    path.iter()
        .try_fold(value, |current, key| current.get(*key))?
        .as_str()
}

fn encode_page_cursor(timestamp: &str, id: &str, scope: &str) -> Result<String, ApiError> {
    serde_json::to_string(&PageCursor {
        timestamp: timestamp.to_owned(),
        id: id.to_owned(),
        scope: scope.to_owned(),
    })
    .map_err(ApiError::internal)
}

fn finish_cursor_page(
    items: &mut Vec<Value>,
    limit: i64,
    scope: &str,
    timestamp_path: &[&str],
) -> Result<(bool, Option<String>), ApiError> {
    finish_cursor_page_with_id(items, limit, scope, timestamp_path, &["id"])
}

fn finish_cursor_page_with_id(
    items: &mut Vec<Value>,
    limit: i64,
    scope: &str,
    timestamp_path: &[&str],
    id_path: &[&str],
) -> Result<(bool, Option<String>), ApiError> {
    let has_more = items.len() > limit as usize;
    if has_more {
        items.pop();
    }
    let next_cursor = if has_more {
        let last = items
            .last()
            .ok_or_else(|| ApiError::internal("تعذر إنشاء مؤشر الصفحة"))?;
        let timestamp = value_at_path(last, timestamp_path)
            .ok_or_else(|| ApiError::internal("تعذر قراءة تاريخ مؤشر الصفحة"))?;
        let id = value_at_path(last, id_path)
            .ok_or_else(|| ApiError::internal("تعذر قراءة سجل مؤشر الصفحة"))?;
        Some(encode_page_cursor(timestamp, id, scope)?)
    } else {
        None
    };
    Ok((has_more, next_cursor))
}

fn round_percentage(amount: i64, bps: i64) -> i64 {
    // Both inputs are validated as non-negative and bps is capped at 10,000. The mathematical
    // result therefore always fits in i64 when the amount does, but the intermediate product may
    // not. Use i128 so accepted large integer-money values are rounded exactly instead of being
    // silently undercounted by saturating multiplication.
    (((amount as i128) * (bps as i128) + 5000) / 10000) as i64
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
    let principal = authorize_section(
        &state,
        &headers,
        "section.dashboard.access",
        "operational.read",
    )?;
    let can_view_all = principal.is_manager();
    let owner_id = principal.id.clone();
    let selected_date = selected_business_date(&query)?.unwrap_or_else(business_today);
    let selected_date_key = selected_date.format("%Y-%m-%d").to_string();
    let mut selected_day_query = query.clone();
    selected_day_query.insert("date".to_owned(), selected_date_key.clone());
    let (selected_day_start, selected_day_end) = date_range(&selected_day_query)?;
    let (selected_month_start, selected_month_end) = business_month_range(selected_date)?;
    let response = blocking(move || {
    let db = state.read_db()?;
    struct DashboardMetrics {
        today_salary_withdrawals: i64,
        today_washes: i64,
        month_washes: i64,
        today_revenue_before_withdrawals: i64,
        month_revenue: i64,
        month_commissions: i64,
        month_expenses: i64,
        month_worker_deductions: i64,
        month_showroom_revenue: i64,
        month_showroom_payments: i64,
        month_business_share: i64,
        month_business_expenses: i64,
        today_paid_customer_revenue: i64,
        today_paid_customer_profit: i64,
        today_showroom_revenue: i64,
        today_showroom_profit: i64,
    }
    let metrics = db.conn.query_row(
        "WITH wash AS (
             SELECT
                COALESCE(SUM(CASE WHEN occurred_at BETWEEN ?3 AND ?4 THEN 1 ELSE 0 END),0),
                COUNT(*),
                COALESCE(SUM(CASE WHEN occurred_at BETWEEN ?3 AND ?4 AND ((payment_type='cash' AND is_paid=1) OR payment_type='showroom') THEN price_milli ELSE 0 END),0),
                COALESCE(SUM(price_milli),0),
                COALESCE(SUM(commission_milli),0),
                COALESCE(SUM(CASE WHEN payment_type='showroom' THEN price_milli ELSE 0 END),0),
                COALESCE(SUM(business_share_milli),0),
                COALESCE(SUM(CASE WHEN occurred_at BETWEEN ?3 AND ?4 AND payment_type='cash' AND is_paid=1 THEN price_milli ELSE 0 END),0),
                COALESCE(SUM(CASE WHEN occurred_at BETWEEN ?3 AND ?4 AND payment_type='cash' AND is_paid=1 THEN business_share_milli ELSE 0 END),0),
                COALESCE(SUM(CASE WHEN occurred_at BETWEEN ?3 AND ?4 AND payment_type='showroom' THEN price_milli ELSE 0 END),0),
                COALESCE(SUM(CASE WHEN occurred_at BETWEEN ?3 AND ?4 AND payment_type='showroom' THEN business_share_milli ELSE 0 END),0)
             FROM wash_operations
             WHERE status='posted' AND occurred_at BETWEEN ?1 AND ?2
               AND (?5=1 OR created_by=?6)
         ), expense AS (
             SELECT COALESCE(SUM(amount_milli),0),COALESCE(SUM(business_amount_milli),0)
             FROM expenses WHERE occurred_at BETWEEN ?1 AND ?2 AND (?5=1 OR created_by=?6)
         ), deduction AS (
             SELECT COALESCE(SUM(ea.amount_milli),0)
             FROM expense_allocations ea JOIN expenses e ON e.id=ea.expense_id
             WHERE e.occurred_at BETWEEN ?1 AND ?2 AND (?5=1 OR e.created_by=?6)
         ), payment AS (
             SELECT COALESCE(SUM(amount_milli),0)
             FROM showroom_payments WHERE paid_at BETWEEN ?1 AND ?2 AND (?5=1 OR created_by=?6)
         ), withdrawal AS (
             SELECT CASE WHEN ?5=1 THEN COALESCE(SUM(amount_milli),0) ELSE 0 END
             FROM salary_withdrawals WHERE withdrawn_at BETWEEN ?3 AND ?4
         )
         SELECT withdrawal.*,wash.*,expense.*,deduction.*,payment.*
         FROM withdrawal,wash,expense,deduction,payment",
        params![selected_month_start, selected_month_end, selected_day_start, selected_day_end, if can_view_all { 1 } else { 0 }, owner_id],
        |row| Ok(DashboardMetrics {
            today_salary_withdrawals: row.get(0)?, today_washes: row.get(1)?, month_washes: row.get(2)?,
            today_revenue_before_withdrawals: row.get(3)?, month_revenue: row.get(4)?, month_commissions: row.get(5)?,
            month_showroom_revenue: row.get(6)?, month_business_share: row.get(7)?,
            today_paid_customer_revenue: row.get(8)?, today_paid_customer_profit: row.get(9)?,
            today_showroom_revenue: row.get(10)?, today_showroom_profit: row.get(11)?,
            month_expenses: row.get(12)?, month_business_expenses: row.get(13)?,
            month_worker_deductions: row.get(14)?, month_showroom_payments: row.get(15)?,
        }),
    ).map_err(ApiError::internal)?;
    let mut recent = Vec::new();
    let mut statement = db.conn.prepare(
        "SELECT w.id, w.vehicle_make, w.vehicle_model, w.license_plate, w.price_milli, w.occurred_at, w.payment_type, w.status, worker.full_name, w.commission_milli
         FROM wash_operations w JOIN workers worker ON worker.id=w.worker_id
         WHERE w.status='posted' AND w.is_paid=0
               AND NOT EXISTS(SELECT 1 FROM overnight_cars overnight WHERE overnight.wash_id=w.id)
               AND w.occurred_at BETWEEN ?1 AND ?2 AND (?3=1 OR w.created_by=?4)
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
        "todayWashes": metrics.today_washes,
        "monthWashes": metrics.month_washes,
        "recentWashes": recent,
        "selectedDate": selected_date_key,
        "businessTimeZone": "Africa/Tripoli",
    });
    if principal.has_permission("dashboard.daily_revenue.read") {
        let today_revenue =
            metrics.today_revenue_before_withdrawals - metrics.today_salary_withdrawals;
        response["financial"] = json!({"todayRevenue": today_revenue});
    }
    if principal.has_permission("financial.manage") {
        if response.get("financial").is_none() {
            response["financial"] = json!({});
        }
        response["financial"]["monthRevenue"] = json!(metrics.month_revenue);
        response["financial"]["workerPayable"] =
            json!((metrics.month_commissions - metrics.month_worker_deductions).max(0));
        response["financial"]["expenses"] = json!(metrics.month_expenses);
        response["financial"]["showroomOutstanding"] =
            json!(metrics.month_showroom_revenue - metrics.month_showroom_payments);
        response["financial"]["businessShare"] = json!(metrics.month_business_share);
        response["financial"]["netProfit"] =
            json!(metrics.month_business_share - metrics.month_business_expenses);
    }
    if principal.is_manager() {
        let revenue_before_withdrawals =
            metrics.today_paid_customer_revenue + metrics.today_showroom_revenue;
        let today_net_profit =
            metrics.today_paid_customer_profit - metrics.today_salary_withdrawals;
        if response.get("financial").is_none() {
            response["financial"] = json!({});
        }
        response["financial"]["todayRevenue"] =
            json!(revenue_before_withdrawals - metrics.today_salary_withdrawals);
        response["financial"]["todayRevenueBeforeWithdrawals"] = json!(revenue_before_withdrawals);
        response["financial"]["todayCustomerRevenue"] = json!(metrics.today_paid_customer_revenue);
        response["financial"]["todayNetProfit"] = json!(today_net_profit);
        response["financial"]["todayShowroomRevenue"] = json!(metrics.today_showroom_revenue);
        response["financial"]["todayShowroomNetProfit"] = json!(metrics.today_showroom_profit);
    }
    Ok(response)
    }).await?;
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
    wash_type: Option<String>,
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

fn trim_optional(
    value: Option<String>,
    max_chars: usize,
    error_message: &str,
) -> Result<Option<String>, ApiError> {
    let normalized = value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    if normalized
        .as_deref()
        .is_some_and(|value| value.chars().count() > max_chars)
    {
        return Err(ApiError::bad(error_message));
    }
    Ok(normalized)
}

async fn list_washes(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    let principal = authorize_section(
        &state,
        &headers,
        "section.washes.access",
        "operational.read",
    )?;
    let (from, to) = date_range(&query)?;
    let limit = query
        .get("limit")
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(150)
        .clamp(1, 300);
    let can_view_all = principal.is_manager();
    let owner_id = principal.id.clone();
    let scope = cursor_scope(&[
        "washes",
        &from,
        &to,
        if can_view_all { "all" } else { "owner" },
        &owner_id,
        if principal.has_permission("financial.manage") {
            "financial"
        } else {
            "operational"
        },
    ]);
    let cursor = history_cursor(&query, "cursor", &scope)?;
    let (cursor_timestamp, cursor_id) = cursor_boundary(cursor.as_ref());
    let db = state.read_db()?;
    let mut washes = Vec::new();
    if principal.has_permission("financial.manage") {
        let can_manage_overnight = principal.is_manager();
        let mut statement = db.conn.prepare(
            "SELECT w.id,w.vehicle_make,w.vehicle_model,w.manufacture_year,w.license_plate,w.car_color,w.price_milli,w.occurred_at,w.payment_type,w.status,
                    worker.id,worker.full_name,showroom.id,showroom.name,w.commission_bps,w.commission_milli,w.business_share_milli,w.showroom_payment_method,
                    EXISTS(SELECT 1 FROM overnight_cars overnight WHERE overnight.wash_id=w.id),w.is_paid,w.paid_at,w.wash_type
             FROM wash_operations w JOIN workers worker ON worker.id=w.worker_id LEFT JOIN showrooms showroom ON showroom.id=w.showroom_id
             WHERE w.status='posted' AND w.is_paid=0
                   AND NOT EXISTS(SELECT 1 FROM overnight_cars overnight WHERE overnight.wash_id=w.id)
                   AND w.occurred_at BETWEEN ?1 AND ?2 AND (?3=1 OR w.created_by=?4)
                   AND (w.occurred_at<?5 OR (w.occurred_at=?5 AND w.id<?6))
             ORDER BY w.occurred_at DESC,w.id DESC LIMIT ?7",
        ).map_err(ApiError::internal)?;
        let rows = statement.query_map(params![from, to, if can_view_all { 1 } else { 0 }, owner_id.clone(), cursor_timestamp, cursor_id, limit+1], move |row| {
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
            item["washType"] = json!(row.get::<_, Option<String>>(21)?);
            Ok(item)
        }).map_err(ApiError::internal)?;
        for row in rows {
            washes.push(row.map_err(ApiError::internal)?);
        }
    } else {
        let mut statement = db.conn.prepare(
            "SELECT w.id,w.vehicle_make,w.vehicle_model,w.manufacture_year,w.license_plate,w.car_color,w.price_milli,w.occurred_at,w.payment_type,w.status,
                    worker.id,worker.full_name,showroom.id,showroom.name,w.showroom_payment_method,w.is_paid,w.paid_at,w.wash_type
             FROM wash_operations w JOIN workers worker ON worker.id=w.worker_id LEFT JOIN showrooms showroom ON showroom.id=w.showroom_id
             WHERE w.status='posted' AND w.is_paid=0
                   AND NOT EXISTS(SELECT 1 FROM overnight_cars overnight WHERE overnight.wash_id=w.id)
                   AND w.occurred_at BETWEEN ?1 AND ?2 AND (?3=1 OR w.created_by=?4)
                   AND (w.occurred_at<?5 OR (w.occurred_at=?5 AND w.id<?6))
             ORDER BY w.occurred_at DESC,w.id DESC LIMIT ?7",
        ).map_err(ApiError::internal)?;
        let rows = statement.query_map(params![from, to, if can_view_all { 1 } else { 0 }, owner_id, cursor_timestamp, cursor_id, limit+1], |row| Ok(json!({
            "id": row.get::<_, String>(0)?, "vehicleMake": row.get::<_, String>(1)?, "vehicleModel": row.get::<_, String>(2)?,
            "manufactureYear": row.get::<_, Option<i32>>(3)?, "licensePlate": row.get::<_, Option<String>>(4)?, "carColor": row.get::<_, Option<String>>(5)?, "priceMilli": row.get::<_, i64>(6)?,
            "occurredAt": row.get::<_, String>(7)?, "paymentType": row.get::<_, String>(8)?, "status": row.get::<_, String>(9)?,
            "worker": {"id": row.get::<_, String>(10)?, "fullName": row.get::<_, String>(11)?},
            "showroom": row.get::<_, Option<String>>(12)?.map(|id| json!({"id": id, "name": row.get::<_, Option<String>>(13).ok().flatten()})),
            "showroomPaymentMethod": row.get::<_, Option<String>>(14)?,
            "isPaid": row.get::<_, i64>(15)? == 1,
            "paidAt": row.get::<_, Option<String>>(16)?,
            "washType": row.get::<_, Option<String>>(17)?
        }))).map_err(ApiError::internal)?;
        for row in rows {
            washes.push(row.map_err(ApiError::internal)?);
        }
    }
    let (has_more, next_cursor) = finish_cursor_page(&mut washes, limit, &scope, &["occurredAt"])?;
    Ok(ok(
        json!({"items": washes,"hasMore":has_more,"nextCursor":next_cursor}),
    ))
}

async fn list_paid_cars(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    let principal = authorize_section(
        &state,
        &headers,
        "section.paid_cars.access",
        "operational.read",
    )?;
    let (from, to) = date_range(&query)?;
    let can_view_all = principal.is_manager();
    let include_financials = principal.has_permission("financial.manage");
    let owner_id = principal.id.clone();
    let limit = query
        .get("limit")
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(150)
        .clamp(1, 300);
    let scope = cursor_scope(&[
        "paid-cars",
        &from,
        &to,
        if can_view_all { "all" } else { "owner" },
        &owner_id,
        if include_financials {
            "financial"
        } else {
            "operational"
        },
    ]);
    let cursor = history_cursor(&query, "cursor", &scope)?;
    let (cursor_timestamp, cursor_id) = cursor_boundary(cursor.as_ref());
    let db = state.read_db()?;
    let mut statement = db.conn.prepare(
        "SELECT w.id,w.vehicle_make,w.vehicle_model,w.manufacture_year,w.license_plate,w.car_color,w.price_milli,
                w.occurred_at,w.payment_type,w.status,worker.id,worker.full_name,showroom.id,showroom.name,
                w.commission_bps,w.commission_milli,w.business_share_milli,w.showroom_payment_method,w.paid_at,
                creator.id,creator.full_name,w.wash_type
         FROM wash_operations w
         JOIN workers worker ON worker.id=w.worker_id
         LEFT JOIN showrooms showroom ON showroom.id=w.showroom_id
         JOIN users creator ON creator.id=w.created_by
         WHERE w.status='posted' AND w.is_paid=1 AND w.occurred_at BETWEEN ?1 AND ?2
               AND (?3=1 OR w.created_by=?4)
               AND (w.occurred_at<?5 OR (w.occurred_at=?5 AND w.id<?6))
         ORDER BY w.occurred_at DESC,w.id DESC
         LIMIT ?7",
    ).map_err(ApiError::internal)?;
    let rows = statement.query_map(
        params![&from, &to, if can_view_all { 1 } else { 0 }, owner_id.clone(), cursor_timestamp, cursor_id, limit+1],
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
            item["washType"] = json!(row.get::<_,Option<String>>(21)?);
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
    let (has_more, next_cursor) = finish_cursor_page(&mut items, limit, &scope, &["occurredAt"])?;
    let (total_count, settlement) = db
        .conn
        .query_row(
            "SELECT COUNT(*),COALESCE(SUM(price_milli),0)
         FROM wash_operations
         WHERE status='posted' AND is_paid=1 AND occurred_at BETWEEN ?1 AND ?2
               AND (?3=1 OR created_by=?4)",
            params![&from, &to, if can_view_all { 1 } else { 0 }, owner_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .map_err(ApiError::internal)?;
    Ok(ok(
        json!({"items":items,"totalCount":total_count,"settlementMilli":settlement,"hasMore":has_more,"nextCursor":next_cursor}),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OvernightStatusInput {
    is_overnight: bool,
}

async fn set_wash_overnight(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<OvernightStatusInput>,
) -> ApiResult {
    let principal = authorize_section(
        &state,
        &headers,
        "section.overnight.access",
        "operational.write",
    )?;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let (created_by, status, is_paid): (String, String, i64) = db
        .conn
        .query_row(
            "SELECT created_by,status,is_paid FROM wash_operations WHERE id=?1",
            [id.clone()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    if !principal.is_manager() && created_by != principal.id {
        return Err(ApiError::forbidden());
    }
    if status != "posted" {
        return Err(ApiError::bad("لا يمكن تغيير حالة المبيت لعملية ملغاة"));
    }
    if input.is_overnight && is_paid == 1 {
        return Err(ApiError::bad(
            "أعد السيارة إلى غير خالصة قبل تعليمها كسيارة مبيتة",
        ));
    }
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    let changed = if input.is_overnight {
        tx.execute(
            "INSERT OR IGNORE INTO overnight_cars(id,wash_id,marked_by,marked_at) VALUES(?1,?2,?3,?4)",
            params![new_id(), &id, &principal.id, now()],
        ).map_err(ApiError::internal)? == 1
    } else {
        tx.execute("DELETE FROM overnight_cars WHERE wash_id=?1", [&id])
            .map_err(ApiError::internal)?
            == 1
    };
    if changed {
        let (action, description) = if input.is_overnight {
            (
                "OVERNIGHT_CAR_MARKED",
                "تم تعليم السيارة كسيارة مبيتة وربطها بعملية الغسيل الأصلية",
            )
        } else {
            (
                "OVERNIGHT_CAR_UNMARKED",
                "تم إلغاء تعليم السيارة كسيارة مبيتة مع الاحتفاظ بعملية الغسيل الأصلية",
            )
        };
        insert_audit_tx(
            &tx,
            Some(&principal.id),
            action,
            "overnight_car",
            Some(&id),
            description,
            Some(&json!({"washId":&id,"isOvernight":input.is_overnight})),
        )?;
    }
    tx.commit().map_err(ApiError::internal)?;
    let wash = wash_item_by_id(&db.conn, &id, principal.has_permission("financial.manage"))?;
    Ok(ok(json!({"updated":changed,"wash":wash})))
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
    let principal = authorize_section(
        &state,
        &headers,
        "section.paid_cars.access",
        "operational.write",
    )?;
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
                None::<String>,
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
         WHERE status='posted' AND is_paid=1 AND occurred_at BETWEEN ?1 AND ?2
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
                creator.id,creator.full_name,w.wash_type
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
            item["washType"] = json!(row.get::<_,Option<String>>(23)?);
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
    entries: &[LedgerEntry<'_>],
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
    let principal = authorize_section(
        &state,
        &headers,
        "section.washes.access",
        "operational.write",
    )?;
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
    let wash_type = input
        .wash_type
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    if wash_type
        .as_deref()
        .is_some_and(|value| value.chars().count() > 120)
    {
        return Err(ApiError::bad("نوع الغسيل طويل جدًا"));
    }
    let license_plate = trim_optional(input.license_plate, 60, "رقم لوحة السيارة طويل جدًا")?;
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
    let raw_occurred_at = input
        .occurred_at
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(now);
    let occurred_at = canonical_timestamp(&raw_occurred_at, "وقت الغسلة غير صالح")?;
    let request_id = operation_request_id(input.client_request_id)?.unwrap_or_else(new_id);
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
        "INSERT INTO wash_operations(id,vehicle_make,vehicle_model,manufacture_year,license_plate,car_color,wash_type,price_milli,worker_id,payment_type,showroom_id,showroom_payment_method,occurred_at,commission_bps,commission_milli,business_share_milli,created_by,client_request_id,created_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)",
        params![wash_id, vehicle_make, vehicle_model, input.manufacture_year, license_plate, car_color, wash_type, price, input.worker_id, input.payment_type, showroom_id, showroom_payment_method, occurred_at, commission_bps, commission, business_share, principal.id, request_id, now()],
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
    let principal = authorize_section(
        &state,
        &headers,
        "section.washes.access",
        "operational.write",
    )?;
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
    let wash_type = input
        .wash_type
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    if wash_type
        .as_deref()
        .is_some_and(|value| value.chars().count() > 120)
    {
        return Err(ApiError::bad("نوع الغسيل طويل جدًا"));
    }
    let license_plate = trim_optional(input.license_plate, 60, "رقم لوحة السيارة طويل جدًا")?;
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
    let raw_occurred_at = input
        .occurred_at
        .as_deref()
        .filter(|v| !v.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(now);
    let occurred_at = canonical_timestamp(&raw_occurred_at, "وقت الغسلة غير صالح")?;
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
    tx.execute("UPDATE wash_operations SET vehicle_make=?1,vehicle_model=?2,manufacture_year=?3,license_plate=?4,car_color=?5,wash_type=?6,price_milli=?7,worker_id=?8,payment_type=?9,showroom_id=?10,showroom_payment_method=?11,occurred_at=?12,commission_bps=?13,commission_milli=?14,business_share_milli=?15,revision=revision+1,updated_at=?16,updated_by=?17 WHERE id=?18",
        params![vehicle_make,vehicle_model,input.manufacture_year,license_plate,car_color,wash_type,price,input.worker_id,input.payment_type,showroom_id,showroom_payment_method,occurred_at,commission_bps,commission,business_share,now(),principal.id,id]).map_err(ApiError::internal)?;
    if let Some(mark_as_overnight) = input.mark_as_overnight {
        if mark_as_overnight {
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
    let principal = authorize_section(
        &state,
        &headers,
        "section.overnight.access",
        "operational.read",
    )?;
    let (from, to) = date_range(&query)?;
    let limit = history_limit(&query);
    let can_view_all = principal.is_manager();
    let owner_id = principal.id.clone();
    let scope = cursor_scope(&[
        "overnight-cars",
        &from,
        &to,
        if can_view_all { "all" } else { "owner" },
        &owner_id,
        if principal.has_permission("financial.manage") {
            "financial"
        } else {
            "operational"
        },
    ]);
    let cursor = history_cursor(&query, "cursor", &scope)?;
    let (cursor_timestamp, cursor_id) = cursor_boundary(cursor.as_ref());
    let db = state.read_db()?;
    let mut statement = db.conn.prepare(
        "SELECT overnight.id,overnight.marked_at,marker.full_name,
                wash.id,wash.vehicle_make,wash.vehicle_model,wash.manufacture_year,wash.license_plate,wash.car_color,wash.price_milli,
                wash.occurred_at,wash.payment_type,wash.status,wash.showroom_payment_method,
                worker.id,worker.full_name,showroom.id,showroom.name,wash.commission_milli,wash.wash_type
         FROM overnight_cars overnight
         JOIN wash_operations wash ON wash.id=overnight.wash_id
         JOIN workers worker ON worker.id=wash.worker_id
         LEFT JOIN showrooms showroom ON showroom.id=wash.showroom_id
         JOIN users marker ON marker.id=overnight.marked_by
         WHERE wash.status='posted' AND wash.is_paid=0 AND wash.occurred_at BETWEEN ?1 AND ?2
               AND (?3=1 OR wash.created_by=?4)
               AND (wash.occurred_at<?5 OR (wash.occurred_at=?5 AND wash.id<?6))
         ORDER BY wash.occurred_at DESC,wash.id DESC
         LIMIT ?7"
    ).map_err(ApiError::internal)?;
    let rows = statement.query_map(params![from, to, if can_view_all { 1 } else { 0 }, owner_id, cursor_timestamp, cursor_id, limit+1], move |row| {
        let mut wash = json!({
            "id": row.get::<_,String>(3)?, "vehicleMake": row.get::<_,String>(4)?, "vehicleModel": row.get::<_,String>(5)?,
            "manufactureYear": row.get::<_,Option<i32>>(6)?, "licensePlate": row.get::<_,Option<String>>(7)?, "carColor": row.get::<_,Option<String>>(8)?, "priceMilli": row.get::<_,i64>(9)?,
            "occurredAt": row.get::<_,String>(10)?, "paymentType": row.get::<_,String>(11)?, "status": row.get::<_,String>(12)?,
            "showroomPaymentMethod": row.get::<_,Option<String>>(13)?,
            "worker": {"id": row.get::<_,String>(14)?, "fullName": row.get::<_,String>(15)?},
            "showroom": row.get::<_,Option<String>>(16)?.map(|id| json!({"id":id,"name":row.get::<_,Option<String>>(17).ok().flatten()})),
            "washType": row.get::<_,Option<String>>(19)?,
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
    let (has_more, next_cursor) = finish_cursor_page_with_id(
        &mut items,
        limit,
        &scope,
        &["wash", "occurredAt"],
        &["wash", "id"],
    )?;
    Ok(ok(
        json!({"items":items,"hasMore":has_more,"nextCursor":next_cursor}),
    ))
}

async fn delete_overnight_car(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult {
    let principal = authorize_section(
        &state,
        &headers,
        "section.overnight.access",
        "operational.write",
    )?;
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
    let principal = authorize_section(
        &state,
        &headers,
        "section.washes.access",
        "operational.write",
    )?;
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
    let principal = authorize_section(
        &state,
        &headers,
        "section.workers.access",
        "operational.read",
    )?;
    let (from, to) = date_range(&query)?;
    let result = blocking(move || {
    let db = state.read_db()?;
    let mut items = Vec::new();
    let can_view_financial = principal.has_permission("financial.manage");
    let mut statement = db.conn.prepare(
        "SELECT w.id,w.full_name,w.phone,w.notes,w.is_active,w.commission_bps_override,
                COUNT(wash.worker_id),COALESCE(SUM(wash.commission_milli),0),
                COALESCE((SELECT SUM(ea.amount_milli) FROM expense_allocations ea JOIN expenses e ON e.id=ea.expense_id WHERE ea.worker_id=w.id AND e.occurred_at BETWEEN ?1 AND ?2),0)
         FROM workers w LEFT JOIN wash_operations wash ON wash.worker_id=w.id
              AND wash.status='posted' AND wash.occurred_at BETWEEN ?1 AND ?2
         WHERE w.is_active=1
         GROUP BY w.id ORDER BY w.full_name",
    ).map_err(ApiError::internal)?;
    let rows = statement.query_map(params![from, to], |row| {
        let commission_bps_override: Option<i64> = row.get(5)?;
        let gross: i64 = row.get(7)?; let deductions: i64 = row.get(8)?;
        let mut item = json!({
            "id":row.get::<_,String>(0)?,"fullName":row.get::<_,String>(1)?,"phone":row.get::<_,Option<String>>(2)?,"notes":row.get::<_,Option<String>>(3)?,
            "isActive":row.get::<_,i64>(4)? == 1,"washCount":row.get::<_,i64>(6)?
        });
        if can_view_financial {
            item["commissionBpsOverride"] = json!(commission_bps_override);
            item["financial"] = json!({"grossCommissionMilli":gross,"deductionsMilli":deductions,"paidMilli":0,"remainingMilli":(gross-deductions).max(0)});
        }
        Ok(item)
    }).map_err(ApiError::internal)?;
    for row in rows {
        items.push(row.map_err(ApiError::internal)?);
    }
    if query.get("status").is_some_and(|value| value == "active") {
        items.retain(|item| item["isActive"] == true);
    }
    Ok(json!({"items":items}))
    }).await?;
    Ok(ok(result))
}

async fn create_worker(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<WorkerInput>,
) -> ApiResult {
    let principal = authorize_section(
        &state,
        &headers,
        "section.workers.access",
        "operational.write",
    )?;
    let full_name = trim_required(&input.full_name, "اسم العامل")?;
    if input.commission_bps_override.is_some() && !principal.has_permission("financial.manage") {
        return Err(ApiError::forbidden());
    }
    if let Some(bps) = input.commission_bps_override {
        if !(0..=10000).contains(&bps) {
            return Err(ApiError::bad("نسبة العمولة الخاصة غير صالحة"));
        }
    }
    let phone = trim_optional(input.phone, 60, "رقم الهاتف طويل جدًا")?;
    let notes = trim_optional(input.notes, 500, "الملاحظة طويلة جدًا")?;
    let id = new_id();
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    db.conn.execute(
        "INSERT INTO workers(id,full_name,phone,notes,is_active,commission_bps_override,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?7)",
        params![id, full_name, phone, notes, if input.is_active.unwrap_or(true){1}else{0}, input.commission_bps_override, now()],
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
    let principal = authorize_section(
        &state,
        &headers,
        "section.workers.access",
        "operational.write",
    )?;
    let full_name = trim_required(&input.full_name, "اسم العامل")?;
    if input.commission_bps_override.is_some() && !principal.has_permission("financial.manage") {
        return Err(ApiError::forbidden());
    }
    if let Some(bps) = input.commission_bps_override {
        if !(0..=10000).contains(&bps) {
            return Err(ApiError::bad("نسبة العمولة الخاصة غير صالحة"));
        }
    }
    let phone = trim_optional(input.phone, 60, "رقم الهاتف طويل جدًا")?;
    let notes = trim_optional(input.notes, 500, "الملاحظة طويلة جدًا")?;
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let (previous_active, previous_commission_bps_override) = db
        .conn
        .query_row(
            "SELECT is_active,commission_bps_override FROM workers WHERE id=?1",
            [id.clone()],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?)),
        )
        .optional()
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    let next_active = input.is_active.unwrap_or(previous_active == 1);
    let next_commission_bps_override = if principal.has_permission("financial.manage") {
        input.commission_bps_override
    } else {
        previous_commission_bps_override
    };
    let affected = db.conn.execute(
        "UPDATE workers SET full_name=?1,phone=?2,notes=?3,is_active=?4,commission_bps_override=?5,updated_at=?6,deactivated_at=CASE WHEN ?4=0 THEN COALESCE(deactivated_at,?6) ELSE NULL END,deactivated_by=CASE WHEN ?4=0 THEN ?8 ELSE NULL END WHERE id=?7",
        params![full_name, phone, notes, if next_active{1}else{0}, next_commission_bps_override, now(), id, principal.id],
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
        "تم حذف العامل من قائمة العمال النشطة مع الاحتفاظ بكل السجلات التاريخية",
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
    let _principal = authorize_section(
        &state,
        &headers,
        "section.workers.access",
        "operational.read",
    )?;
    let (from, to) = date_range(&query)?;
    let limit = history_limit(&query);
    let scope = cursor_scope(&["worker-history", &id, &from, &to]);
    let cursor = history_cursor(&query, "cursor", &scope)?;
    let (cursor_timestamp, cursor_id) = cursor_boundary(cursor.as_ref());
    let db = state.read_db()?;
    let worker: Option<Value> = db.conn.query_row("SELECT id,full_name,phone,notes,is_active FROM workers WHERE id=?1",[id.clone()],|row|Ok(json!({"id":row.get::<_,String>(0)?,"fullName":row.get::<_,String>(1)?,"phone":row.get::<_,Option<String>>(2)?,"notes":row.get::<_,Option<String>>(3)?,"isActive":row.get::<_,i64>(4)?==1}))).optional().map_err(ApiError::internal)?;
    let mut worker = worker.ok_or_else(ApiError::not_found)?;
    let (wash_count, total_wash_value): (i64, i64) = db
        .conn
        .query_row(
            "SELECT COUNT(*),COALESCE(SUM(price_milli),0)
         FROM wash_operations
         WHERE worker_id=?1 AND status='posted' AND occurred_at BETWEEN ?2 AND ?3",
            params![id.clone(), from.clone(), to.clone()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(ApiError::internal)?;
    worker["washCount"] = json!(wash_count);
    worker["totalWashValueMilli"] = json!(total_wash_value);
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
               AND (wash.occurred_at<?4 OR (wash.occurred_at=?4 AND wash.id<?5))
         ORDER BY wash.occurred_at DESC,wash.id DESC
         LIMIT ?6",
    ).map_err(ApiError::internal)?;
    let rows = statement.query_map(params![id,from,to,cursor_timestamp,cursor_id,limit+1], |row| {
        let mut value = json!({"id":row.get::<_,String>(0)?,"vehicleMake":row.get::<_,String>(1)?,"vehicleModel":row.get::<_,String>(2)?,"manufactureYear":row.get::<_,Option<i32>>(3)?,"licensePlate":row.get::<_,Option<String>>(4)?,"occurredAt":row.get::<_,String>(6)?,"paymentType":row.get::<_,String>(7)?,"status":row.get::<_,String>(8)?,"worker":{"id":row.get::<_,String>(9)?,"fullName":row.get::<_,String>(10)?},"createdBy":{"id":row.get::<_,String>(11)?,"fullName":row.get::<_,String>(12)?}});
        value["priceMilli"] = json!(row.get::<_,i64>(5)?);
        Ok(value)
    }).map_err(ApiError::internal)?;
    for row in rows {
        history.push(row.map_err(ApiError::internal)?);
    }
    let (history_has_more, history_next_cursor) =
        finish_cursor_page(&mut history, limit, &scope, &["occurredAt"])?;
    let mut response = json!({"worker":worker,"history":history,"historyHasMore":history_has_more,"historyNextCursor":history_next_cursor});
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
    let principal = authorize_section(
        &state,
        &headers,
        "section.workers.access",
        "worker.daily_value.manage",
    )?;
    let date = NaiveDate::parse_from_str(input.value_date.trim(), "%Y-%m-%d")
        .map_err(|_| ApiError::bad("تاريخ القيمة اليومية غير صالح"))?;
    let value_date = date.format("%Y-%m-%d").to_string();
    let amount = parse_milli(&input.amount)?;
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
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
    let _principal = authorize_section(
        &state,
        &headers,
        "section.workers.access",
        "financial.manage",
    )?;
    let (from, to) = date_range(&query)?;
    let db = state.read_db()?;
    let commission_bps_override: Option<i64> = db
        .conn
        .query_row(
            "SELECT commission_bps_override FROM workers WHERE id=?1",
            [id.clone()],
            |row| row.get(0),
        )
        .optional()
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    let gross=total_for(&db.conn,"SELECT COALESCE(SUM(commission_milli),0) FROM wash_operations WHERE status='posted' AND worker_id=?1 AND occurred_at BETWEEN ?2 AND ?3",params![id,from,to])?;
    let deductions=total_for(&db.conn,"SELECT COALESCE(SUM(ea.amount_milli),0) FROM expense_allocations ea JOIN expenses e ON e.id=ea.expense_id WHERE ea.worker_id=?1 AND e.occurred_at BETWEEN ?2 AND ?3",params![id,from,to])?;
    Ok(ok(
        json!({"commissionBpsOverride":commission_bps_override,"grossCommissionMilli":gross,"deductionsMilli":deductions,"netEarningsMilli":(gross-deductions).max(0),"paidMilli":0,"remainingMilli":(gross-deductions).max(0),"payments":[]}),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkerWithdrawalReturnInput {
    transaction_type: String,
    amount: String,
    occurred_at: String,
    notes: Option<String>,
    client_request_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkerSettlementInput {
    occurred_at: String,
}

struct WorkerMovementTotals {
    withdrawals: i64,
    deductions: i64,
    returns: i64,
    deduction_payments: i64,
    settlements: i64,
    remaining_returns: i64,
    withdrawal_debt: i64,
    deduction_outstanding: i64,
}

fn worker_financial_reset_at(conn: &Connection, worker_id: &str) -> Result<String, ApiError> {
    conn.query_row(
        "SELECT COALESCE(MAX(reset_at),'') FROM worker_financial_resets WHERE worker_id=?1",
        [worker_id],
        |row| row.get(0),
    )
    .map_err(ApiError::internal)
}

fn worker_movement_totals_excluding(
    conn: &Connection,
    worker_id: &str,
    from: &str,
    to: &str,
    excluded_movement_id: Option<&str>,
) -> Result<WorkerMovementTotals, ApiError> {
    let reset_at = worker_financial_reset_at(conn, worker_id)?;
    let (withdrawals, returns, deduction_payments, settlements): (i64, i64, i64, i64) = conn
        .query_row(
            "SELECT
                COALESCE(SUM(CASE WHEN transaction_type='withdrawal' THEN amount_milli ELSE 0 END),0),
                COALESCE(SUM(CASE WHEN transaction_type='return' THEN amount_milli ELSE 0 END),0),
                COALESCE(SUM(CASE WHEN transaction_type='deduction_payment' THEN amount_milli ELSE 0 END),0),
                COALESCE(SUM(CASE WHEN transaction_type='settlement' THEN amount_milli ELSE 0 END),0)
             FROM worker_withdrawal_returns
             WHERE worker_id=?1 AND deleted_at IS NULL AND occurred_at BETWEEN ?2 AND ?3
                   AND created_at>?4 AND (?5 IS NULL OR id<>?5)",
            params![worker_id, from, to, reset_at, excluded_movement_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(ApiError::internal)?;
    let deductions = total_for(
        conn,
        "SELECT COALESCE(SUM(ea.amount_milli),0)
         FROM expense_allocations ea JOIN expenses e ON e.id=ea.expense_id
         WHERE ea.worker_id=?1 AND e.occurred_at BETWEEN ?2 AND ?3 AND ea.created_at>?4",
        params![worker_id, from, to, reset_at],
    )?;
    let remaining_returns = withdrawals.saturating_sub(returns).max(0);
    let withdrawal_debt = remaining_returns.saturating_sub(settlements).max(0);
    let deduction_outstanding = deductions.saturating_sub(deduction_payments).max(0);
    Ok(WorkerMovementTotals {
        withdrawals,
        deductions,
        returns,
        deduction_payments,
        settlements,
        remaining_returns,
        withdrawal_debt,
        deduction_outstanding,
    })
}

fn worker_movement_totals(
    conn: &Connection,
    worker_id: &str,
    from: &str,
    to: &str,
) -> Result<WorkerMovementTotals, ApiError> {
    worker_movement_totals_excluding(conn, worker_id, from, to, None)
}

async fn worker_withdrawal_returns(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    manager(&state, &headers)?;
    let (from, to) = date_range(&query)?;
    let limit = history_limit(&query);
    let scope = cursor_scope(&["worker-movements", &id, &from, &to]);
    let cursor = history_cursor(&query, "cursor", &scope)?;
    let (cursor_timestamp, cursor_id) = cursor_boundary(cursor.as_ref());
    let db = state.read_db()?;
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
    let totals = worker_movement_totals(&db.conn, &id, &from, &to)?;
    let reset_at = worker_financial_reset_at(&db.conn, &id)?;
    let mut transactions = Vec::new();
    let mut statement = db.conn.prepare(
        "SELECT id,transaction_type,amount_milli,occurred_at,notes,created_by_name,editable,deletable
         FROM (
             SELECT movement.id id,movement.transaction_type transaction_type,movement.amount_milli amount_milli,
                    movement.occurred_at occurred_at,movement.notes notes,user.full_name created_by_name,
                    movement.transaction_type='deduction_payment' editable,1 deletable,movement.created_at sort_created_at
             FROM worker_withdrawal_returns movement JOIN users user ON user.id=movement.created_by
             WHERE movement.worker_id=?1 AND movement.deleted_at IS NULL AND movement.occurred_at BETWEEN ?2 AND ?3
                   AND movement.created_at>?4
             UNION ALL
             SELECT 'deduction:'||allocation.id,'deduction',allocation.amount_milli,expense.occurred_at,
                    expense.description,user.full_name,0,0,allocation.created_at
             FROM expense_allocations allocation
             JOIN expenses expense ON expense.id=allocation.expense_id
             JOIN users user ON user.id=expense.created_by
             WHERE allocation.worker_id=?1 AND allocation.amount_milli>0 AND expense.occurred_at BETWEEN ?2 AND ?3
                   AND allocation.created_at>?4
         )
         WHERE occurred_at<?5 OR (occurred_at=?5 AND id<?6)
         ORDER BY occurred_at DESC,id DESC
         LIMIT ?7"
    ).map_err(ApiError::internal)?;
    let rows = statement.query_map(params![id.clone(),from,to,reset_at,cursor_timestamp,cursor_id,limit+1], |row| Ok(json!({
        "id":row.get::<_,String>(0)?,"type":row.get::<_,String>(1)?,"amountMilli":row.get::<_,i64>(2)?,
        "occurredAt":row.get::<_,String>(3)?,"notes":row.get::<_,Option<String>>(4)?,"createdByName":row.get::<_,String>(5)?,
        "editable":row.get::<_,i64>(6)? == 1,"deletable":row.get::<_,i64>(7)? == 1
    }))).map_err(ApiError::internal)?;
    for row in rows {
        transactions.push(row.map_err(ApiError::internal)?);
    }
    let (has_more, next_cursor) =
        finish_cursor_page(&mut transactions, limit, &scope, &["occurredAt"])?;
    Ok(ok(json!({
        "worker":{"id":id,"fullName":worker_name},"totalWithdrawalsMilli":totals.withdrawals,
        "totalDeductionsMilli":totals.deductions,"totalReturnsMilli":totals.returns,
        "totalDeductionPaymentsMilli":totals.deduction_payments,"totalSettlementsMilli":totals.settlements,
        "remainingReturnsMilli":totals.remaining_returns,
        "remainingWithdrawalDebtMilli":totals.withdrawal_debt,
        "outstandingDeductionBalanceMilli":totals.deduction_outstanding,
        "transactions":transactions,"hasMore":has_more,"nextCursor":next_cursor
    })))
}

async fn create_worker_withdrawal_return(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(worker_id): Path<String>,
    Json(input): Json<WorkerWithdrawalReturnInput>,
) -> ApiResult {
    let principal = manager(&state, &headers)?;
    if !matches!(
        input.transaction_type.as_str(),
        "withdrawal" | "return" | "deduction_payment"
    ) {
        return Err(ApiError::bad("نوع الحركة غير صالح"));
    }
    let amount = parse_milli(&input.amount)?;
    let occurred_at = canonical_timestamp(&input.occurred_at, "تاريخ الحركة غير صالح")?;
    let notes = trim_optional(input.notes, 500, "الملاحظة طويلة جدًا")?;
    let request_id = operation_request_id(input.client_request_id)?;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    if let Some(response) = replay_operation(
        &db.conn,
        &principal.id,
        "worker_withdrawal_return.create",
        request_id.as_deref(),
    )? {
        return Ok(ok(response));
    }
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
    if matches!(
        input.transaction_type.as_str(),
        "return" | "deduction_payment"
    ) {
        let totals = worker_movement_totals(
            &db.conn,
            &worker_id,
            "0000-01-01T00:00:00Z",
            "9999-12-31T23:59:59Z",
        )?;
        let available = if input.transaction_type == "return" {
            totals.withdrawal_debt
        } else {
            totals.deduction_outstanding
        };
        if amount > available {
            return Err(ApiError::bad(if input.transaction_type == "return" {
                "لا يمكن أن يتجاوز المرتجع الرصيد القائم للعامل"
            } else {
                "لا يمكن أن يتجاوز تسديد الاستقطاع الرصيد القائم للعامل"
            }));
        }
    }
    let id = new_id();
    let created_at = now();
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    tx.execute(
        "INSERT INTO worker_withdrawal_returns(id,worker_id,transaction_type,amount_milli,occurred_at,notes,created_by,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![id,worker_id,input.transaction_type,amount,occurred_at,notes,principal.id,created_at],
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
    let response = json!({"id":id,"created":true});
    record_operation(
        &tx,
        &principal.id,
        "worker_withdrawal_return.create",
        request_id.as_deref(),
        &response,
    )?;
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(response))
}

async fn update_worker_deduction_payment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((worker_id, movement_id)): Path<(String, String)>,
    Json(input): Json<WorkerWithdrawalReturnInput>,
) -> ApiResult {
    let principal = manager(&state, &headers)?;
    if input.transaction_type != "deduction_payment" {
        return Err(ApiError::bad("يمكن تعديل حركات تسديد الاستقطاع فقط"));
    }
    let amount = parse_milli(&input.amount)?;
    let occurred_at = canonical_timestamp(&input.occurred_at, "تاريخ الحركة غير صالح")?;
    let notes = trim_optional(input.notes, 500, "الملاحظة طويلة جدًا")?;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let previous_amount: i64 = db
        .conn
        .query_row(
            "SELECT amount_milli FROM worker_withdrawal_returns
             WHERE id=?1 AND worker_id=?2 AND transaction_type='deduction_payment' AND deleted_at IS NULL",
            params![movement_id.clone(),worker_id.clone()],
            |row| row.get(0),
        )
        .optional()
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    let available = worker_movement_totals_excluding(
        &db.conn,
        &worker_id,
        "0000-01-01T00:00:00Z",
        "9999-12-31T23:59:59Z",
        Some(&movement_id),
    )?
    .deduction_outstanding;
    if amount > available {
        return Err(ApiError::bad(
            "لا يمكن أن يتجاوز تسديد الاستقطاع الرصيد القائم بعد إعادة الاحتساب",
        ));
    }
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    tx.execute(
        "UPDATE worker_withdrawal_returns
         SET amount_milli=?1,occurred_at=?2,notes=?3
         WHERE id=?4 AND worker_id=?5 AND transaction_type='deduction_payment' AND deleted_at IS NULL",
        params![amount,occurred_at,notes,movement_id,worker_id],
    ).map_err(ApiError::internal)?;
    insert_audit_tx(
        &tx,
        Some(&principal.id),
        "WORKER_DEDUCTION_PAYMENT_UPDATED",
        "worker_withdrawal_return",
        Some(&movement_id),
        "تم تعديل حركة تسديد استقطاع وإعادة احتساب الرصيد من السجلات المحفوظة",
        Some(
            &json!({"workerId":worker_id,"previousAmountMilli":previous_amount,"amountMilli":amount}),
        ),
    )?;
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(json!({"id":movement_id,"updated":true})))
}

async fn settle_worker_withdrawal_returns(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(worker_id): Path<String>,
    Json(input): Json<WorkerSettlementInput>,
) -> ApiResult {
    let principal = manager(&state, &headers)?;
    let occurred_at = canonical_timestamp(&input.occurred_at, "تاريخ التصفية غير صالح")?;
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
    let outstanding = worker_movement_totals(
        &db.conn,
        &worker_id,
        "0000-01-01T00:00:00Z",
        "9999-12-31T23:59:59Z",
    )?
    .withdrawal_debt;
    if outstanding == 0 {
        return Err(ApiError::bad("لا يوجد دين مسحوبات قائم يحتاج إلى تسوية"));
    }
    let id = new_id();
    let created_at = now();
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    tx.execute(
        "INSERT INTO worker_withdrawal_returns(id,worker_id,transaction_type,amount_milli,occurred_at,notes,created_by,created_at)
         VALUES(?1,?2,'settlement',?3,?4,?5,?6,?7)",
        params![id,worker_id,outstanding,occurred_at,"تسوية دين المسحوبات",principal.id,created_at],
    ).map_err(ApiError::internal)?;
    insert_audit_tx(
        &tx,
        Some(&principal.id),
        "WORKER_WITHDRAWAL_DEBT_SETTLED",
        "worker_withdrawal_return",
        Some(&id),
        "تمت تسوية دين مسحوبات العامل مع الاحتفاظ بالسجل السابق",
        Some(&json!({"workerId":worker_id,"workerName":worker_name,"amountMilli":outstanding})),
    )?;
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(
        json!({"id":id,"created":true,"amountMilli":outstanding,"remainingWithdrawalDebtMilli":0}),
    ))
}

async fn reset_worker_financial_records(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(worker_id): Path<String>,
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
            [worker_id.clone()],
            |row| row.get(0),
        )
        .optional()
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    let reset_id = new_id();
    let reset_at = now();
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    tx.execute(
        "INSERT INTO worker_financial_resets(id,worker_id,reset_at,reset_by) VALUES(?1,?2,?3,?4)",
        params![reset_id, worker_id, reset_at, principal.id],
    )
    .map_err(ApiError::internal)?;
    insert_audit_tx(
        &tx,
        Some(&principal.id),
        "WORKER_FINANCIAL_RECORDS_RESET",
        "worker_financial_reset",
        Some(&reset_id),
        "تم تصفير سجل المسحوبات والمرتجعات والاستقطاعات للعامل المحدد",
        Some(&json!({"workerId":worker_id,"workerName":worker_name})),
    )?;
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(json!({"id":reset_id,"reset":true})))
}

async fn delete_worker_withdrawal_return(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((worker_id, movement_id)): Path<(String, String)>,
) -> ApiResult {
    let principal = manager(&state, &headers)?;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let movement: (String, i64) = db
        .conn
        .query_row(
            "SELECT transaction_type,amount_milli
             FROM worker_withdrawal_returns
             WHERE id=?1 AND worker_id=?2 AND deleted_at IS NULL",
            params![movement_id.clone(), worker_id.clone()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    let deleted_at = now();
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    tx.execute(
        "UPDATE worker_withdrawal_returns SET deleted_at=?1,deleted_by=?2
         WHERE id=?3 AND worker_id=?4 AND deleted_at IS NULL",
        params![deleted_at, principal.id, movement_id, worker_id],
    )
    .map_err(ApiError::internal)?;
    insert_audit_tx(
        &tx,
        Some(&principal.id),
        "WORKER_WITHDRAWAL_RETURN_DELETED",
        "worker_withdrawal_return",
        Some(&movement_id),
        "تم حذف حركة من سجل مسحوبات ومرتجعات العامل وإعادة احتساب الرصيد من السجلات المتبقية",
        Some(&json!({"workerId":worker_id,"type":movement.0,"amountMilli":movement.1})),
    )?;
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(json!({"id":movement_id,"deleted":true})))
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
    let _principal = authorize_section(
        &state,
        &headers,
        "section.showroom_debts.access",
        "financial.manage",
    )?;
    let (_, to) = date_range(&query)?;
    let db = state.read_db()?;
    let mut statement = db.conn.prepare(
        "WITH wash_totals AS (
             SELECT showroom_id,COUNT(*) wash_count,COALESCE(SUM(price_milli),0) charges,
                    MAX(occurred_at) latest_wash_at
             FROM wash_operations
             WHERE payment_type='showroom' AND status='posted' AND occurred_at<=?1
             GROUP BY showroom_id
         ), payment_totals AS (
             SELECT showroom_id,COALESCE(SUM(amount_milli),0) payments
             FROM showroom_payments WHERE paid_at<=?1 GROUP BY showroom_id
         )
         SELECT showroom.id,showroom.name,showroom.contact_name,showroom.phone,showroom.notes,showroom.is_active,
                wash_totals.wash_count,
                wash_totals.charges-COALESCE(payment_totals.payments,0),
                wash_totals.latest_wash_at
         FROM wash_totals
         JOIN showrooms showroom ON showroom.id=wash_totals.showroom_id
         LEFT JOIN payment_totals ON payment_totals.showroom_id=showroom.id
         WHERE wash_totals.charges-COALESCE(payment_totals.payments,0)>0
         ORDER BY wash_totals.latest_wash_at DESC,showroom.name",
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
    let _principal = authorize_section(
        &state,
        &headers,
        "section.showroom_debts.access",
        "financial.manage",
    )?;
    let (from, to) = date_range(&query)?;
    if from > to {
        return Err(ApiError::bad("تاريخ البداية يجب أن يسبق تاريخ النهاية"));
    }
    let operations_limit = history_limit_named(&query, "operationsLimit");
    let payments_limit = history_limit_named(&query, "paymentsLimit");
    let operations_scope = cursor_scope(&["showroom-debt-operations", &id, &from, &to]);
    let payments_scope = cursor_scope(&["showroom-debt-payments", &id, &from, &to]);
    let operations_cursor = history_cursor(&query, "operationsCursor", &operations_scope)?;
    let payments_cursor = history_cursor(&query, "paymentsCursor", &payments_scope)?;
    let (operations_cursor_timestamp, operations_cursor_id) =
        cursor_boundary(operations_cursor.as_ref());
    let (payments_cursor_timestamp, payments_cursor_id) = cursor_boundary(payments_cursor.as_ref());
    let db = state.read_db()?;
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
             AND (wash.occurred_at<?4 OR (wash.occurred_at=?4 AND wash.id<?5))
         ORDER BY wash.occurred_at DESC,wash.id DESC
         LIMIT ?6",
    ).map_err(ApiError::internal)?;
    let rows = statement.query_map(params![id,from,to,operations_cursor_timestamp,operations_cursor_id,operations_limit+1], |row| Ok(json!({
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
    let (operations_has_more, operations_next_cursor) = finish_cursor_page(
        &mut operations,
        operations_limit,
        &operations_scope,
        &["occurredAt"],
    )?;
    let (outstanding_wash_count,total_charges):(i64,i64)=db.conn.query_row(
        "SELECT COUNT(*),COALESCE(SUM(price_milli),0) FROM wash_operations
         WHERE showroom_id=?1 AND payment_type='showroom' AND status='posted' AND occurred_at BETWEEN ?2 AND ?3",
        params![id,from,to],|row|Ok((row.get(0)?,row.get(1)?))).map_err(ApiError::internal)?;
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
               AND (payment.paid_at<?4 OR (payment.paid_at=?4 AND payment.id<?5))
         ORDER BY payment.paid_at DESC,payment.id DESC
         LIMIT ?6",
        )
        .map_err(ApiError::internal)?;
    let payment_rows = payment_statement
        .query_map(
            params![
                id,
                from,
                to,
                payments_cursor_timestamp,
                payments_cursor_id,
                payments_limit + 1
            ],
            |row| {
                Ok(json!({
                    "id": row.get::<_,String>(0)?, "amountMilli": row.get::<_,i64>(1)?,
                    "paidAt": row.get::<_,String>(2)?, "notes": row.get::<_,Option<String>>(3)?,
                    "showroom": {"id": row.get::<_,String>(4)?, "name": row.get::<_,String>(5)?},
                    "recordedBy": row.get::<_,String>(6)?
                }))
            },
        )
        .map_err(ApiError::internal)?;
    for row in payment_rows {
        payments.push(row.map_err(ApiError::internal)?);
    }
    let (payments_has_more, payments_next_cursor) =
        finish_cursor_page(&mut payments, payments_limit, &payments_scope, &["paidAt"])?;
    Ok(ok(json!({
        "showroom":showroom,"from":from,"to":to,
        "outstandingWashCount":outstanding_wash_count,
        "totalChargesMilli":total_charges,
        "totalPaymentsMilli":total_payments,
        "totalOutstandingMilli":(total_charges - total_payments).max(0),
        "operations":operations,"operationsHasMore":operations_has_more,"operationsNextCursor":operations_next_cursor,
        "payments":payments,"paymentsHasMore":payments_has_more,"paymentsNextCursor":payments_next_cursor
    })))
}

async fn list_showrooms(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    let principal = authorize_section(
        &state,
        &headers,
        "section.showrooms.access",
        "operational.read",
    )?;
    let (from, to) = date_range(&query)?;
    let db = state.read_db()?;
    let mut items = Vec::new();
    if principal.has_permission("financial.manage") {
        let mut statement=db.conn.prepare("SELECT s.id,s.name,s.contact_name,s.phone,s.notes,s.is_active,COUNT(w.showroom_id),COALESCE(SUM(w.price_milli),0),COALESCE((SELECT SUM(amount_milli) FROM showroom_payments sp WHERE sp.showroom_id=s.id AND sp.paid_at BETWEEN ?1 AND ?2),0) FROM showrooms s LEFT JOIN wash_operations w ON w.showroom_id=s.id AND w.status='posted' AND w.payment_type='showroom' AND w.occurred_at BETWEEN ?1 AND ?2 GROUP BY s.id ORDER BY s.is_active DESC,s.name").map_err(ApiError::internal)?;
        let rows=statement.query_map(params![from,to],|row|{let charges:i64=row.get(7)?;let payments:i64=row.get(8)?;Ok(json!({"id":row.get::<_,String>(0)?,"name":row.get::<_,String>(1)?,"contactName":row.get::<_,Option<String>>(2)?,"phone":row.get::<_,Option<String>>(3)?,"notes":row.get::<_,Option<String>>(4)?,"isActive":row.get::<_,i64>(5)?==1,"washCount":row.get::<_,i64>(6)?,"financial":{"chargesMilli":charges,"paymentsMilli":payments,"outstandingMilli":(charges-payments).max(0)}}))}).map_err(ApiError::internal)?;
        for row in rows {
            items.push(row.map_err(ApiError::internal)?);
        }
    } else {
        let mut statement=db.conn.prepare("SELECT s.id,s.name,s.contact_name,s.phone,s.notes,s.is_active,COUNT(w.showroom_id) FROM showrooms s LEFT JOIN wash_operations w ON w.showroom_id=s.id AND w.status='posted' AND w.payment_type='showroom' AND w.occurred_at BETWEEN ?1 AND ?2 GROUP BY s.id ORDER BY s.is_active DESC,s.name").map_err(ApiError::internal)?;
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
    let principal = authorize_section(
        &state,
        &headers,
        "section.showrooms.access",
        "operational.write",
    )?;
    let name = trim_required(&input.name, "اسم المعرض")?;
    let contact_name = trim_optional(input.contact_name, 120, "اسم جهة الاتصال طويل جدًا")?;
    let phone = trim_optional(input.phone, 60, "رقم الهاتف طويل جدًا")?;
    let notes = trim_optional(input.notes, 500, "الملاحظة طويلة جدًا")?;
    let id = new_id();
    let db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    db.conn.execute("INSERT INTO showrooms(id,name,contact_name,phone,notes,is_active,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?7)",params![id,name,contact_name,phone,notes,if input.is_active.unwrap_or(true){1}else{0},now()]).map_err(|error|ApiError::new(StatusCode::CONFLICT,format!("تعذر إنشاء المعرض: {error}")))?;
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
    let principal = authorize_section(
        &state,
        &headers,
        "section.showrooms.access",
        "operational.write",
    )?;
    let name = trim_required(&input.name, "اسم المعرض")?;
    let contact_name = trim_optional(input.contact_name, 120, "اسم جهة الاتصال طويل جدًا")?;
    let phone = trim_optional(input.phone, 60, "رقم الهاتف طويل جدًا")?;
    let notes = trim_optional(input.notes, 500, "الملاحظة طويلة جدًا")?;
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
    let count=db.conn.execute("UPDATE showrooms SET name=?1,contact_name=?2,phone=?3,notes=?4,is_active=?5,updated_at=?6 WHERE id=?7",params![name,contact_name,phone,notes,is_active,now(),id]).map_err(ApiError::internal)?;
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
    let principal = authorize_section(
        &state,
        &headers,
        "section.showrooms.access",
        "operational.write",
    )?;
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
    let principal = authorize_section(
        &state,
        &headers,
        "section.showrooms.access",
        "operational.read",
    )?;
    let (from, to) = date_range(&query)?;
    let limit = history_limit(&query);
    let can_view_all = principal.is_manager();
    let can_view_financial = principal.has_permission("financial.manage");
    let owner_id = principal.id.clone();
    let scope = cursor_scope(&[
        "showroom-history",
        &id,
        &from,
        &to,
        if can_view_all { "all" } else { "owner" },
        &owner_id,
        if can_view_financial {
            "financial"
        } else {
            "operational"
        },
    ]);
    let cursor = history_cursor(&query, "cursor", &scope)?;
    let (cursor_timestamp, cursor_id) = cursor_boundary(cursor.as_ref());
    let db = state.read_db()?;
    let showroom:Option<Value>=db.conn.query_row("SELECT id,name,contact_name,phone,notes,is_active FROM showrooms WHERE id=?1",[id.clone()],|row|Ok(json!({"id":row.get::<_,String>(0)?,"name":row.get::<_,String>(1)?,"contactName":row.get::<_,Option<String>>(2)?,"phone":row.get::<_,Option<String>>(3)?,"notes":row.get::<_,Option<String>>(4)?,"isActive":row.get::<_,i64>(5)?==1}))).optional().map_err(ApiError::internal)?;
    let showroom = showroom.ok_or_else(ApiError::not_found)?;
    let mut history = Vec::new();
    let mut statement=db.conn.prepare("SELECT w.id,w.vehicle_make,w.vehicle_model,w.manufacture_year,w.license_plate,w.price_milli,w.occurred_at,w.status,worker.full_name FROM wash_operations w JOIN workers worker ON worker.id=w.worker_id WHERE w.showroom_id=?1 AND w.occurred_at BETWEEN ?2 AND ?3 AND (?4=1 OR w.created_by=?5) AND (w.occurred_at<?6 OR (w.occurred_at=?6 AND w.id<?7)) ORDER BY w.occurred_at DESC,w.id DESC LIMIT ?8").map_err(ApiError::internal)?;
    let rows=statement.query_map(params![id,from,to,if can_view_all { 1 } else { 0 },owner_id,cursor_timestamp,cursor_id,limit+1],|row|{let mut value=json!({"id":row.get::<_,String>(0)?,"vehicleMake":row.get::<_,String>(1)?,"vehicleModel":row.get::<_,String>(2)?,"manufactureYear":row.get::<_,Option<i32>>(3)?,"licensePlate":row.get::<_,Option<String>>(4)?,"occurredAt":row.get::<_,String>(6)?,"status":row.get::<_,String>(7)?,"workerName":row.get::<_,String>(8)?});if can_view_financial{value["priceMilli"]=json!(row.get::<_,i64>(5)?);}Ok(value)}).map_err(ApiError::internal)?;
    for row in rows {
        history.push(row.map_err(ApiError::internal)?);
    }
    let (history_has_more, history_next_cursor) =
        finish_cursor_page(&mut history, limit, &scope, &["occurredAt"])?;
    Ok(ok(
        json!({"showroom":showroom,"history":history,"historyHasMore":history_has_more,"historyNextCursor":history_next_cursor}),
    ))
}

async fn showroom_statistics(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    let principal = authorize_section(
        &state,
        &headers,
        "section.showrooms.access",
        "operational.read",
    )?;
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
    let db = state.read_db()?;
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
    let _principal = authorize_section(
        &state,
        &headers,
        "section.showrooms.access",
        "financial.manage",
    )?;
    let (from, to) = date_range(&query)?;
    let limit = history_limit(&query);
    let scope = cursor_scope(&["showroom-financial-payments", &id, &from, &to]);
    let cursor = history_cursor(&query, "cursor", &scope)?;
    let (cursor_timestamp, cursor_id) = cursor_boundary(cursor.as_ref());
    let db = state.read_db()?;
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
    let mut statement=db.conn.prepare("SELECT sp.id,sp.amount_milli,sp.paid_at,sp.notes,s.id,s.name,u.full_name FROM showroom_payments sp JOIN showrooms s ON s.id=sp.showroom_id JOIN users u ON u.id=sp.created_by WHERE sp.showroom_id=?1 AND sp.paid_at BETWEEN ?2 AND ?3 AND (sp.paid_at<?4 OR (sp.paid_at=?4 AND sp.id<?5)) ORDER BY sp.paid_at DESC,sp.id DESC LIMIT ?6").map_err(ApiError::internal)?;
    let rows=statement.query_map(params![id,from,to,cursor_timestamp,cursor_id,limit+1],|row|Ok(json!({"id":row.get::<_,String>(0)?,"amountMilli":row.get::<_,i64>(1)?,"paidAt":row.get::<_,String>(2)?,"notes":row.get::<_,Option<String>>(3)?,"showroom":{"id":row.get::<_,String>(4)?,"name":row.get::<_,String>(5)?},"recordedBy":row.get::<_,String>(6)?}))).map_err(ApiError::internal)?;
    for row in rows {
        payments.push(row.map_err(ApiError::internal)?);
    }
    let (payments_has_more, payments_next_cursor) =
        finish_cursor_page(&mut payments, limit, &scope, &["paidAt"])?;
    Ok(ok(
        json!({"chargesMilli":charges,"paymentsMilli":paid,"outstandingMilli":(charges-paid).max(0),"payments":payments,"paymentsHasMore":payments_has_more,"paymentsNextCursor":payments_next_cursor}),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PaymentInput {
    showroom_id: Option<String>,
    amount: String,
    paid_at: Option<String>,
    notes: Option<String>,
    client_request_id: Option<String>,
}

fn payment_time(value: &Option<String>) -> Result<String, ApiError> {
    let value = value
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(now);
    canonical_timestamp(&value, "تاريخ الدفع غير صالح")
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
    let principal = authorize_section(
        &state,
        &headers,
        "section.finance.access",
        "financial.manage",
    )?;
    let showroom_id = input
        .showroom_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::bad("اختر المعرض"))?
        .to_owned();
    let amount = parse_milli(&input.amount)?;
    let paid_at = payment_time(&input.paid_at)?;
    let notes = trim_optional(input.notes, 500, "الملاحظة طويلة جدًا")?;
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
    let principal = authorize_section(
        &state,
        &headers,
        "section.finance.access",
        "financial.manage",
    )?;
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
        .unwrap_or_else(|| business_today().format("%Y-%m").to_string());
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
    let _principal = authorize_section(
        &state,
        &headers,
        "section.salaries.access",
        "financial.manage",
    )?;
    let selected_date = selected_business_date(&query)?;
    let month = match selected_date {
        Some(date) => date.format("%Y-%m").to_string(),
        None => parse_payroll_month(query.get("month"))?,
    };
    let (month_start, month_end) = business_month_range_from_key(&month)?;
    let db = state.read_db()?;
    let mut employees = Vec::new();
    let mut total_salary = 0_i64;
    let mut total_withdrawals = 0_i64;
    let mut total_deductions = 0_i64;
    let mut statement = db
        .conn
        .prepare(
            "WITH withdrawal_totals AS (
                SELECT employee_id,COALESCE(SUM(amount_milli),0) total
                FROM salary_withdrawals
                WHERE withdrawn_at BETWEEN ?2 AND ?3 GROUP BY employee_id
             ), deduction_totals AS (
                SELECT employee_id,COALESCE(SUM(amount_milli),0) total
                FROM salary_deductions
                WHERE deducted_at BETWEEN ?2 AND ?3 GROUP BY employee_id
             )
             SELECT employee.id,employee.full_name,employee.is_active,
                COALESCE((SELECT rate.salary_milli FROM payroll_salary_rates rate
                          WHERE rate.employee_id=employee.id AND rate.effective_month<=?1
                          ORDER BY rate.effective_month DESC LIMIT 1),0),
                COALESCE(withdrawal_totals.total,0),COALESCE(deduction_totals.total,0)
         FROM payroll_employees employee
         LEFT JOIN withdrawal_totals ON withdrawal_totals.employee_id=employee.id
         LEFT JOIN deduction_totals ON deduction_totals.employee_id=employee.id
         WHERE employee.is_active=1
         ORDER BY employee.full_name,employee.id",
        )
        .map_err(ApiError::internal)?;
    let rows = statement
        .query_map(params![month.clone(), month_start, month_end], |row| {
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
            "employee": {"id": id, "fullName": full_name, "isActive": is_active},
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
    let principal = authorize_section(
        &state,
        &headers,
        "section.salaries.access",
        "financial.manage",
    )?;
    let full_name = trim_required(&input.full_name, "اسم الموظف")?;
    let month = parse_payroll_month(Some(&input.month))?;
    let salary = parse_milli(&input.salary)?;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let employee_id = new_id();
    let timestamp = now();
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    tx.execute(
        "INSERT INTO payroll_employees(id,full_name,is_active,created_at,updated_at) VALUES(?1,?2,1,?3,?3)",
        params![employee_id, full_name, timestamp],
    )
    .map_err(ApiError::internal)?;
    tx.execute(
        "INSERT INTO payroll_salary_rates(employee_id,effective_month,salary_milli,set_by,created_at,updated_at)
         VALUES(?1,?2,?3,?4,?5,?5)",
        params![employee_id, month, salary, principal.id, timestamp],
    ).map_err(ApiError::internal)?;
    insert_audit_tx(
        &tx,
        Some(&principal.id),
        "PAYROLL_EMPLOYEE_CREATED",
        "payroll_employee",
        Some(&employee_id),
        "تم إنشاء موظف مستقل في قسم المرتبات",
        Some(&json!({"employeeId":employee_id,"month":month,"salaryMilli":salary})),
    )?;
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(json!({
        "employee": {"id":employee_id,"fullName":full_name,"isActive":true},
        "month":month,
        "salaryMilli":salary
    })))
}

async fn set_employee_salary(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(employee_id): Path<String>,
    Json(input): Json<SalaryInput>,
) -> ApiResult {
    let principal = authorize_section(
        &state,
        &headers,
        "section.salaries.access",
        "financial.manage",
    )?;
    let month = parse_payroll_month(Some(&input.month))?;
    let salary = parse_milli(&input.salary)?;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let employee_name: String = db
        .conn
        .query_row(
            "SELECT full_name FROM payroll_employees WHERE id=?1 AND is_active=1",
            [employee_id.clone()],
            |row| row.get(0),
        )
        .optional()
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    let timestamp = now();
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    tx.execute(
        "INSERT INTO payroll_salary_rates(employee_id,effective_month,salary_milli,set_by,created_at,updated_at)
         VALUES(?1,?2,?3,?4,?5,?5)
         ON CONFLICT(employee_id,effective_month) DO UPDATE SET salary_milli=excluded.salary_milli,set_by=excluded.set_by,updated_at=excluded.updated_at",
        params![employee_id, month, salary, principal.id, timestamp],
    ).map_err(ApiError::internal)?;
    insert_audit_tx(
        &tx,
        Some(&principal.id),
        "PAYROLL_EMPLOYEE_SALARY_SET",
        "payroll_salary_rate",
        Some(&employee_id),
        "تم تعيين الراتب الشهري للموظف",
        Some(&json!({"employeeId":employee_id,"month":month,"salaryMilli":salary})),
    )?;
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(json!({
        "employee": {"id": employee_id, "fullName": employee_name},
        "month": month,
        "salaryMilli": salary
    })))
}

async fn delete_payroll_employee(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(employee_id): Path<String>,
) -> ApiResult {
    let principal = authorize_section(
        &state,
        &headers,
        "section.salaries.access",
        "financial.manage",
    )?;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let employee_name: String = db
        .conn
        .query_row(
            "SELECT full_name FROM payroll_employees WHERE id=?1 AND is_active=1",
            [employee_id.clone()],
            |row| row.get(0),
        )
        .optional()
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    let timestamp = now();
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    tx.execute(
        "UPDATE payroll_employees SET is_active=0,archived_at=COALESCE(archived_at,?1),archived_by=?2,updated_at=?1 WHERE id=?3",
        params![timestamp, principal.id, employee_id],
    )
    .map_err(ApiError::internal)?;
    insert_audit_tx(
        &tx,
        Some(&principal.id),
        "PAYROLL_EMPLOYEE_ARCHIVED",
        "payroll_employee",
        Some(&employee_id),
        "تمت أرشفة الموظف من قسم المرتبات مع الاحتفاظ بكل السجلات التاريخية",
        None,
    )?;
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(
        json!({"archived":true,"employee":{"id":employee_id,"fullName":employee_name}}),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SalaryWithdrawalInput {
    employee_id: String,
    amount: String,
    withdrawn_at: String,
    notes: Option<String>,
    client_request_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SalaryDeductionInput {
    employee_id: String,
    amount: String,
    deducted_at: Option<String>,
    month: Option<String>,
    notes: Option<String>,
    client_request_id: Option<String>,
}

fn deduction_time(input: &SalaryDeductionInput) -> Result<(String, String), ApiError> {
    if let Some(value) = input.deducted_at.as_deref() {
        let parsed = DateTime::parse_from_rfc3339(value.trim())
            .map_err(|_| ApiError::bad("تاريخ الخصم غير صالح"))?;
        let utc = parsed.with_timezone(&Utc);
        let month = (utc + Duration::hours(BUSINESS_UTC_OFFSET_HOURS))
            .format("%Y-%m")
            .to_string();
        let deducted_at = utc.to_rfc3339_opts(SecondsFormat::Millis, true);
        return Ok((month, deducted_at));
    }
    let month = parse_payroll_month(input.month.as_ref())?;
    let deducted_at =
        canonical_timestamp(&format!("{month}-01T12:00:00Z"), "تاريخ الخصم غير صالح")?;
    Ok((month, deducted_at))
}

async fn list_salary_deductions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    let _principal = authorize_section(
        &state,
        &headers,
        "section.salaries.access",
        "financial.manage",
    )?;
    let selected_date = selected_business_date(&query)?;
    let month = match selected_date {
        Some(date) => date.format("%Y-%m").to_string(),
        None => parse_payroll_month(query.get("month"))?,
    };
    let (from, to) = match selected_date {
        Some(_) => date_range(&query)?,
        None => business_month_range_from_key(&month)?,
    };
    let limit = history_limit(&query);
    let scope = cursor_scope(&["salary-deductions", &month, &from, &to]);
    let cursor = history_cursor(&query, "cursor", &scope)?;
    let (cursor_timestamp, cursor_id) = cursor_boundary(cursor.as_ref());
    let db = state.read_db()?;
    let mut statement = db.conn.prepare(
        "SELECT sd.id,sd.amount_milli,sd.deducted_at,sd.notes,employee.id,employee.full_name,creator.full_name,sd.created_at,sd.updated_at
         FROM salary_deductions sd
         JOIN payroll_employees employee ON employee.id=sd.employee_id
         JOIN users creator ON creator.id=sd.created_by
         WHERE sd.deducted_at BETWEEN ?1 AND ?2
               AND (sd.deducted_at<?3 OR (sd.deducted_at=?3 AND sd.id<?4))
         ORDER BY sd.deducted_at DESC,sd.id DESC
         LIMIT ?5"
    ).map_err(ApiError::internal)?;
    let rows = statement.query_map(params![from,to,cursor_timestamp,cursor_id,limit+1], |row| Ok(json!({
        "id":row.get::<_,String>(0)?,"amountMilli":row.get::<_,i64>(1)?,"deductedAt":row.get::<_,String>(2)?,
        "notes":row.get::<_,Option<String>>(3)?,"employee":{"id":row.get::<_,String>(4)?,"fullName":row.get::<_,String>(5)?},
        "recordedBy":row.get::<_,String>(6)?,"createdAt":row.get::<_,String>(7)?,"updatedAt":row.get::<_,String>(8)?
    }))).map_err(ApiError::internal)?;
    let mut items = Vec::new();
    for row in rows {
        items.push(row.map_err(ApiError::internal)?);
    }
    let (has_more, next_cursor) = finish_cursor_page(&mut items, limit, &scope, &["deductedAt"])?;
    Ok(ok(
        json!({"items":items,"hasMore":has_more,"nextCursor":next_cursor}),
    ))
}

async fn create_salary_deduction(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<SalaryDeductionInput>,
) -> ApiResult {
    let principal = authorize_section(
        &state,
        &headers,
        "section.salaries.access",
        "financial.manage",
    )?;
    let employee_id = input.employee_id.trim().to_owned();
    if employee_id.is_empty() {
        return Err(ApiError::bad("اختر الموظف"));
    }
    let amount = parse_milli(&input.amount)?;
    let (month, deducted_at) = deduction_time(&input)?;
    let notes = trim_optional(input.notes, 500, "الملاحظة طويلة جدًا")?;
    let request_id = operation_request_id(input.client_request_id)?;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    if let Some(response) = replay_operation(
        &db.conn,
        &principal.id,
        "salary_deduction.create",
        request_id.as_deref(),
    )? {
        return Ok(ok(response));
    }
    let employee_name: String = db
        .conn
        .query_row(
            "SELECT full_name FROM payroll_employees WHERE id=?1 AND is_active=1",
            [employee_id.clone()],
            |row| row.get(0),
        )
        .optional()
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    let id = new_id();
    let timestamp = now();
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    tx.execute(
        "INSERT INTO salary_deductions(id,employee_id,amount_milli,deduction_month,deducted_at,notes,created_by,created_at,updated_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?8)",
        params![id, employee_id, amount, month, deducted_at, notes, principal.id, timestamp],
    ).map_err(ApiError::internal)?;
    insert_audit_tx(
        &tx,
        Some(&principal.id),
        "SALARY_DEDUCTION_CREATED",
        "salary_deduction",
        Some(&id),
        "تم تسجيل خصم موظف",
        Some(&json!({"employeeId":employee_id,"amountMilli":amount,"month":month})),
    )?;
    let response = json!({"id":id,"amountMilli":amount,"month":month,"deductedAt":deducted_at,"notes":notes,"employee":{"id":employee_id,"fullName":employee_name},"recordedBy":principal.full_name,"createdAt":timestamp,"updatedAt":timestamp});
    record_operation(
        &tx,
        &principal.id,
        "salary_deduction.create",
        request_id.as_deref(),
        &response,
    )?;
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(response))
}

async fn update_salary_deduction(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<SalaryDeductionInput>,
) -> ApiResult {
    let principal = authorize_section(
        &state,
        &headers,
        "section.salaries.access",
        "financial.manage",
    )?;
    let employee_id = input.employee_id.trim().to_owned();
    if employee_id.is_empty() {
        return Err(ApiError::bad("اختر الموظف"));
    }
    let amount = parse_milli(&input.amount)?;
    let (month, deducted_at) = deduction_time(&input)?;
    let notes = trim_optional(input.notes, 500, "الملاحظة طويلة جدًا")?;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let employee_name: String = db
        .conn
        .query_row(
            "SELECT full_name FROM payroll_employees WHERE id=?1 AND is_active=1",
            [employee_id.clone()],
            |row| row.get(0),
        )
        .optional()
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    let timestamp = now();
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    let changed = tx.execute(
        "UPDATE salary_deductions SET employee_id=?1,amount_milli=?2,deduction_month=?3,deducted_at=?4,notes=?5,updated_by=?6,updated_at=?7 WHERE id=?8",
        params![employee_id,amount,month,deducted_at,notes,principal.id,timestamp,id],
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
        Some(&json!({"employeeId":employee_id,"amountMilli":amount,"month":month})),
    )?;
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(
        json!({"id":id,"amountMilli":amount,"month":month,"deductedAt":deducted_at,"notes":notes,"employee":{"id":employee_id,"fullName":employee_name},"recordedBy":principal.full_name,"updatedAt":timestamp}),
    ))
}

async fn delete_salary_deduction(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult {
    let principal = authorize_section(
        &state,
        &headers,
        "section.salaries.access",
        "financial.manage",
    )?;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let employee_id: String = db
        .conn
        .query_row(
            "SELECT employee_id FROM salary_deductions WHERE id=?1",
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
        Some(&json!({"employeeId":employee_id})),
    )?;
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(json!({"deleted":true,"id":id})))
}

fn withdrawal_time(value: &str) -> Result<String, ApiError> {
    canonical_timestamp(value, "تاريخ المسحوب غير صالح")
}

async fn list_salary_withdrawals(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    let _principal = authorize_section(
        &state,
        &headers,
        "section.salaries.access",
        "financial.manage",
    )?;
    let selected_date = selected_business_date(&query)?;
    let month = match selected_date {
        Some(date) => date.format("%Y-%m").to_string(),
        None => parse_payroll_month(query.get("month"))?,
    };
    let (from, to) = match selected_date {
        Some(_) => date_range(&query)?,
        None => business_month_range_from_key(&month)?,
    };
    let employee_filter = query
        .get("employeeId")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty());
    let limit = history_limit(&query);
    let employee_scope = employee_filter.unwrap_or("");
    let scope = cursor_scope(&["salary-withdrawals", &month, &from, &to, employee_scope]);
    let cursor = history_cursor(&query, "cursor", &scope)?;
    let (cursor_timestamp, cursor_id) = cursor_boundary(cursor.as_ref());
    let db = state.read_db()?;
    let mut items = Vec::new();
    if let Some(employee_id) = employee_filter {
        let mut statement = db.conn.prepare(
            "SELECT sw.id,sw.amount_milli,sw.withdrawn_at,sw.notes,employee.id,employee.full_name,creator.full_name,sw.created_at,sw.updated_at
             FROM salary_withdrawals sw
             JOIN payroll_employees employee ON employee.id=sw.employee_id
             JOIN users creator ON creator.id=sw.created_by
             WHERE sw.withdrawn_at BETWEEN ?1 AND ?2 AND sw.employee_id=?3
                   AND (sw.withdrawn_at<?4 OR (sw.withdrawn_at=?4 AND sw.id<?5))
             ORDER BY sw.withdrawn_at DESC,sw.id DESC
             LIMIT ?6",
        ).map_err(ApiError::internal)?;
        let rows = statement.query_map(params![from,to,employee_id,cursor_timestamp,cursor_id,limit+1], |row| Ok(json!({
            "id":row.get::<_,String>(0)?,"amountMilli":row.get::<_,i64>(1)?,"withdrawnAt":row.get::<_,String>(2)?,
            "notes":row.get::<_,Option<String>>(3)?,"employee":{"id":row.get::<_,String>(4)?,"fullName":row.get::<_,String>(5)?},
            "recordedBy":row.get::<_,String>(6)?,"createdAt":row.get::<_,String>(7)?,"updatedAt":row.get::<_,String>(8)?
        }))).map_err(ApiError::internal)?;
        for row in rows {
            items.push(row.map_err(ApiError::internal)?);
        }
    } else {
        let mut statement = db.conn.prepare(
            "SELECT sw.id,sw.amount_milli,sw.withdrawn_at,sw.notes,employee.id,employee.full_name,creator.full_name,sw.created_at,sw.updated_at
             FROM salary_withdrawals sw
             JOIN payroll_employees employee ON employee.id=sw.employee_id
             JOIN users creator ON creator.id=sw.created_by
             WHERE sw.withdrawn_at BETWEEN ?1 AND ?2
                   AND (sw.withdrawn_at<?3 OR (sw.withdrawn_at=?3 AND sw.id<?4))
             ORDER BY sw.withdrawn_at DESC,sw.id DESC
             LIMIT ?5",
        ).map_err(ApiError::internal)?;
        let rows = statement.query_map(params![from,to,cursor_timestamp,cursor_id,limit+1], |row| Ok(json!({
            "id":row.get::<_,String>(0)?,"amountMilli":row.get::<_,i64>(1)?,"withdrawnAt":row.get::<_,String>(2)?,
            "notes":row.get::<_,Option<String>>(3)?,"employee":{"id":row.get::<_,String>(4)?,"fullName":row.get::<_,String>(5)?},
            "recordedBy":row.get::<_,String>(6)?,"createdAt":row.get::<_,String>(7)?,"updatedAt":row.get::<_,String>(8)?
        }))).map_err(ApiError::internal)?;
        for row in rows {
            items.push(row.map_err(ApiError::internal)?);
        }
    }
    let (has_more, next_cursor) = finish_cursor_page(&mut items, limit, &scope, &["withdrawnAt"])?;
    Ok(ok(
        json!({"items":items,"hasMore":has_more,"nextCursor":next_cursor}),
    ))
}

async fn create_salary_withdrawal(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<SalaryWithdrawalInput>,
) -> ApiResult {
    let principal = authorize_section(
        &state,
        &headers,
        "section.salaries.access",
        "financial.manage",
    )?;
    let employee_id = input.employee_id.trim().to_owned();
    if employee_id.is_empty() {
        return Err(ApiError::bad("اختر الموظف"));
    }
    let amount = parse_milli(&input.amount)?;
    let withdrawn_at = withdrawal_time(&input.withdrawn_at)?;
    let notes = trim_optional(input.notes, 500, "الملاحظة طويلة جدًا")?;
    let request_id = operation_request_id(input.client_request_id)?;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    if let Some(response) = replay_operation(
        &db.conn,
        &principal.id,
        "salary_withdrawal.create",
        request_id.as_deref(),
    )? {
        return Ok(ok(response));
    }
    let employee_name: String = db
        .conn
        .query_row(
            "SELECT full_name FROM payroll_employees WHERE id=?1 AND is_active=1",
            [employee_id.clone()],
            |row| row.get(0),
        )
        .optional()
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    let id = new_id();
    let timestamp = now();
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    tx.execute(
        "INSERT INTO salary_withdrawals(id,employee_id,amount_milli,withdrawn_at,notes,created_by,created_at,updated_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?7)",
        params![id,employee_id,amount,withdrawn_at,notes,principal.id,timestamp],
    ).map_err(ApiError::internal)?;
    insert_audit_tx(
        &tx,
        Some(&principal.id),
        "SALARY_WITHDRAWAL_CREATED",
        "salary_withdrawal",
        Some(&id),
        "تم تسجيل مسحوب موظف",
        Some(&json!({"employeeId":employee_id,"amountMilli":amount})),
    )?;
    let response = json!({
        "id":id,"amountMilli":amount,"withdrawnAt":withdrawn_at,"notes":notes,
        "employee":{"id":employee_id,"fullName":employee_name},"recordedBy":principal.full_name,"createdAt":timestamp,"updatedAt":timestamp
    });
    record_operation(
        &tx,
        &principal.id,
        "salary_withdrawal.create",
        request_id.as_deref(),
        &response,
    )?;
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(response))
}

async fn update_salary_withdrawal(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<SalaryWithdrawalInput>,
) -> ApiResult {
    let principal = authorize_section(
        &state,
        &headers,
        "section.salaries.access",
        "financial.manage",
    )?;
    let employee_id = input.employee_id.trim().to_owned();
    if employee_id.is_empty() {
        return Err(ApiError::bad("اختر الموظف"));
    }
    let amount = parse_milli(&input.amount)?;
    let withdrawn_at = withdrawal_time(&input.withdrawn_at)?;
    let notes = trim_optional(input.notes, 500, "الملاحظة طويلة جدًا")?;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let employee_name: String = db
        .conn
        .query_row(
            "SELECT full_name FROM payroll_employees WHERE id=?1 AND is_active=1",
            [employee_id.clone()],
            |row| row.get(0),
        )
        .optional()
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    let timestamp = now();
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    let affected = tx.execute(
        "UPDATE salary_withdrawals SET employee_id=?1,amount_milli=?2,withdrawn_at=?3,notes=?4,updated_by=?5,updated_at=?6 WHERE id=?7",
        params![employee_id,amount,withdrawn_at,notes,principal.id,timestamp,id],
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
        Some(&json!({"employeeId":employee_id,"amountMilli":amount})),
    )?;
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(json!({
        "id":id,"amountMilli":amount,"withdrawnAt":withdrawn_at,"notes":notes,
        "employee":{"id":employee_id,"fullName":employee_name},"recordedBy":principal.full_name,"updatedAt":timestamp
    })))
}

async fn delete_salary_withdrawal(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult {
    let principal = authorize_section(
        &state,
        &headers,
        "section.salaries.access",
        "financial.manage",
    )?;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let employee_id: String = db
        .conn
        .query_row(
            "SELECT employee_id FROM salary_withdrawals WHERE id=?1",
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
        Some(&json!({"employeeId":employee_id})),
    )?;
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(json!({"deleted":true,"id":id})))
}

async fn list_showroom_payments(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    let _principal = authorize_section(
        &state,
        &headers,
        "section.finance.access",
        "financial.manage",
    )?;
    let (from, to) = date_range(&query)?;
    let limit = history_limit(&query);
    let scope = cursor_scope(&["showroom-payments", &from, &to]);
    let cursor = history_cursor(&query, "cursor", &scope)?;
    let (cursor_timestamp, cursor_id) = cursor_boundary(cursor.as_ref());
    let db = state.read_db()?;
    let mut items = Vec::new();
    let mut statement=db.conn.prepare("SELECT sp.id,sp.amount_milli,sp.paid_at,sp.notes,s.id,s.name,u.full_name FROM showroom_payments sp JOIN showrooms s ON s.id=sp.showroom_id JOIN users u ON u.id=sp.created_by WHERE sp.paid_at BETWEEN ?1 AND ?2 AND (sp.paid_at<?3 OR (sp.paid_at=?3 AND sp.id<?4)) ORDER BY sp.paid_at DESC,sp.id DESC LIMIT ?5").map_err(ApiError::internal)?;
    let rows=statement.query_map(params![from,to,cursor_timestamp,cursor_id,limit+1],|row|Ok(json!({"id":row.get::<_,String>(0)?,"amountMilli":row.get::<_,i64>(1)?,"paidAt":row.get::<_,String>(2)?,"notes":row.get::<_,Option<String>>(3)?,"showroom":{"id":row.get::<_,String>(4)?,"name":row.get::<_,String>(5)?},"recordedBy":row.get::<_,String>(6)?}))).map_err(ApiError::internal)?;
    for row in rows {
        items.push(row.map_err(ApiError::internal)?);
    }
    let (has_more, next_cursor) = finish_cursor_page(&mut items, limit, &scope, &["paidAt"])?;
    Ok(ok(
        json!({"items":items,"hasMore":has_more,"nextCursor":next_cursor}),
    ))
}

async fn create_showroom_payment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<PaymentInput>,
) -> ApiResult {
    let principal = authorize_section(
        &state,
        &headers,
        "section.finance.access",
        "financial.manage",
    )?;
    let showroom_id = input
        .showroom_id
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| ApiError::bad("اختر المعرض"))?
        .to_owned();
    let amount = parse_milli(&input.amount)?;
    let paid_at = payment_time(&input.paid_at)?;
    let notes = trim_optional(input.notes, 500, "الملاحظة طويلة جدًا")?;
    let request_id = operation_request_id(input.client_request_id)?;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    if let Some(response) = replay_operation(
        &db.conn,
        &principal.id,
        "showroom_payment.create",
        request_id.as_deref(),
    )? {
        return Ok(ok(response));
    }
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
    tx.execute("INSERT INTO showroom_payments(id,showroom_id,amount_milli,paid_at,notes,created_by,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![id,showroom_id,amount,paid_at,notes,principal.id,now()]).map_err(ApiError::internal)?;
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
    let response = showroom_payment_item_by_id(&tx, &id)?;
    record_operation(
        &tx,
        &principal.id,
        "showroom_payment.create",
        request_id.as_deref(),
        &response,
    )?;
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(response))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExpenseInput {
    description: String,
    category: String,
    payment_method: Option<String>,
    amount: String,
    occurred_at: Option<String>,
    notes: Option<String>,
    allocation_type: String,
    business_bps: Option<i64>,
    client_request_id: Option<String>,
}

async fn list_expenses(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    let _principal = authorize_section(
        &state,
        &headers,
        "section.finance.access",
        "financial.manage",
    )?;
    let (from, to) = date_range(&query)?;
    let limit = history_limit(&query);
    let scope = cursor_scope(&["expenses", &from, &to]);
    let cursor = history_cursor(&query, "cursor", &scope)?;
    let (cursor_timestamp, cursor_id) = cursor_boundary(cursor.as_ref());
    let db = state.read_db()?;
    let mut items = Vec::new();
    let mut statement=db.conn.prepare("SELECT e.id,e.description,e.category,e.payment_method,e.amount_milli,e.occurred_at,e.notes,e.allocation_type,e.business_bps,e.workers_bps,e.business_amount_milli,e.workers_amount_milli,u.full_name FROM expenses e JOIN users u ON u.id=e.created_by WHERE e.occurred_at BETWEEN ?1 AND ?2 AND (e.occurred_at<?3 OR (e.occurred_at=?3 AND e.id<?4)) ORDER BY e.occurred_at DESC,e.id DESC LIMIT ?5").map_err(ApiError::internal)?;
    let rows=statement.query_map(params![from,to,cursor_timestamp,cursor_id,limit+1],|row|Ok(json!({"id":row.get::<_,String>(0)?,"description":row.get::<_,String>(1)?,"category":row.get::<_,String>(2)?,"paymentMethod":row.get::<_,String>(3)?,"amountMilli":row.get::<_,i64>(4)?,"occurredAt":row.get::<_,String>(5)?,"notes":row.get::<_,Option<String>>(6)?,"allocationType":row.get::<_,String>(7)?,"businessBps":row.get::<_,i64>(8)?,"workersBps":row.get::<_,i64>(9)?,"businessAmountMilli":row.get::<_,i64>(10)?,"workersAmountMilli":row.get::<_,i64>(11)?,"recordedBy":row.get::<_,String>(12)?}))).map_err(ApiError::internal)?;
    for row in rows {
        items.push(row.map_err(ApiError::internal)?);
    }
    let (has_more, next_cursor) = finish_cursor_page(&mut items, limit, &scope, &["occurredAt"])?;
    Ok(ok(
        json!({"items":items,"hasMore":has_more,"nextCursor":next_cursor}),
    ))
}

async fn create_expense(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<ExpenseInput>,
) -> ApiResult {
    let principal = authorize_section(
        &state,
        &headers,
        "section.finance.access",
        "financial.manage",
    )?;
    let description = trim_required(&input.description, "وصف المصروف")?;
    let category = trim_required(&input.category, "فئة المصروف")?;
    let payment_method = input.payment_method.as_deref().unwrap_or("cash");
    if !matches!(payment_method, "cash" | "bank") {
        return Err(ApiError::bad("طريقة دفع المصروف غير صالحة"));
    }
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
    let notes = trim_optional(input.notes, 500, "الملاحظة طويلة جدًا")?;
    let request_id = operation_request_id(input.client_request_id)?;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    if let Some(response) = replay_operation(
        &db.conn,
        &principal.id,
        "expense.create",
        request_id.as_deref(),
    )? {
        return Ok(ok(response));
    }
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
    tx.execute("INSERT INTO expenses(id,description,category,payment_method,amount_milli,occurred_at,notes,allocation_type,business_bps,workers_bps,business_amount_milli,workers_amount_milli,created_by,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",params![id,description,category,payment_method,amount,occurred_at,notes,input.allocation_type,business_bps,workers_bps,business_amount,workers_amount,principal.id,now()]).map_err(ApiError::internal)?;
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
        Some(
            &json!({"allocationType":input.allocation_type,"workerCount":workers.len(),"category":category,"paymentMethod":payment_method}),
        ),
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
    let response = json!({"id":id,"businessAmountMilli":business_amount,"workersAmountMilli":workers_amount,"workerCount":workers.len()});
    record_operation(
        &tx,
        &principal.id,
        "expense.create",
        request_id.as_deref(),
        &response,
    )?;
    tx.commit().map_err(ApiError::internal)?;
    Ok(ok(response))
}

async fn expense_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult {
    let _principal = authorize_section(
        &state,
        &headers,
        "section.finance.access",
        "financial.manage",
    )?;
    let db = state.read_db()?;
    let expense = db.conn.query_row("SELECT e.id,e.description,e.category,e.payment_method,e.amount_milli,e.occurred_at,e.notes,e.allocation_type,e.business_amount_milli,e.workers_amount_milli,u.full_name,e.created_at FROM expenses e JOIN users u ON u.id=e.created_by WHERE e.id=?1",[id.clone()],|row|Ok(json!({"id":row.get::<_,String>(0)?,"description":row.get::<_,String>(1)?,"category":row.get::<_,String>(2)?,"paymentMethod":row.get::<_,String>(3)?,"amountMilli":row.get::<_,i64>(4)?,"occurredAt":row.get::<_,String>(5)?,"notes":row.get::<_,Option<String>>(6)?,"allocationType":row.get::<_,String>(7)?,"businessAmountMilli":row.get::<_,i64>(8)?,"workersAmountMilli":row.get::<_,i64>(9)?,"recordedBy":row.get::<_,String>(10)?,"createdAt":row.get::<_,String>(11)?}))).optional().map_err(ApiError::internal)?.ok_or_else(ApiError::not_found)?;
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
    let principal = authorize_section(
        &state,
        &headers,
        "section.finance.access",
        "financial.manage",
    )?;
    let description = trim_required(&input.description, "وصف المصروف")?;
    let category = trim_required(&input.category, "فئة المصروف")?;
    let payment_method = input.payment_method.as_deref().unwrap_or("cash");
    if !matches!(payment_method, "cash" | "bank") {
        return Err(ApiError::bad("طريقة دفع المصروف غير صالحة"));
    }
    let amount = parse_milli(&input.amount)?;
    let occurred_at = payment_time(&input.occurred_at)?;
    let (business_bps, workers_bps) = match input.allocation_type.as_str() {
        "business" => (10000, 0),
        "shared" => (5000, 5000),
        _ => return Err(ApiError::bad("نوع توزيع المصروف غير صالح")),
    };
    let business_amount = round_percentage(amount, business_bps);
    let workers_amount = amount - business_amount;
    let notes = trim_optional(input.notes, 500, "الملاحظة طويلة جدًا")?;
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
    tx.execute("UPDATE expenses SET description=?1,category=?2,payment_method=?3,amount_milli=?4,occurred_at=?5,notes=?6,allocation_type=?7,business_bps=?8,workers_bps=?9,business_amount_milli=?10,workers_amount_milli=?11 WHERE id=?12",params![description,category,payment_method,amount,occurred_at,notes,input.allocation_type,business_bps,workers_bps,business_amount,workers_amount,id]).map_err(ApiError::internal)?;
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
        Some(
            &json!({"workerCount":workers.len(),"category":category,"paymentMethod":payment_method}),
        ),
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
    let principal = authorize_section(
        &state,
        &headers,
        "section.finance.access",
        "financial.manage",
    )?;
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

#[derive(Default)]
struct FinancialTotals {
    revenue: i64,
    cash: i64,
    paid_customer_revenue: i64,
    showroom_revenue: i64,
    showroom_commissions: i64,
    commissions: i64,
    business_share: i64,
    paid_cars_profit: i64,
    expenses: i64,
    business_expenses: i64,
    workers_expenses: i64,
    worker_withdrawals: i64,
    showroom_payments: i64,
    worker_deductions: i64,
}

fn financial_summary_value(totals: &FinancialTotals) -> Value {
    let outstanding_worker = (totals.commissions - totals.worker_deductions).max(0);
    let outstanding_showroom = totals.showroom_revenue - totals.showroom_payments;
    json!({
        "totalWashRevenueMilli": totals.revenue,
        "cashRevenueMilli": totals.cash,
        "paidCustomerRevenueMilli": totals.paid_customer_revenue,
        "paidCustomerRevenueAfterDeductionsMilli": totals.paid_customer_revenue - totals.expenses - totals.worker_withdrawals,
        "showroomRevenueMilli": totals.showroom_revenue,
        "showroomNetProfitMilli": totals.showroom_revenue - totals.showroom_commissions,
        "businessShareMilli": totals.business_share,
        "paidCarsProfitMilli": totals.paid_cars_profit,
        "workerCommissionsMilli": totals.commissions,
        "workerDeductionsMilli": totals.worker_deductions,
        "workerWithdrawalsMilli": totals.worker_withdrawals,
        "outstandingWorkerBalancesMilli": outstanding_worker,
        "expensesMilli": totals.expenses,
        "businessExpensesMilli": totals.business_expenses,
        "workerExpensesMilli": totals.workers_expenses,
        "showroomPaymentsMilli": totals.showroom_payments,
        "outstandingShowroomDebtMilli": outstanding_showroom,
        "netProfitBeforeExpensesMilli": totals.paid_cars_profit,
        "netProfitAfterExpensesMilli": totals.paid_cars_profit - totals.business_expenses - totals.worker_withdrawals,
        "netBusinessProfitMilli": totals.business_share - totals.business_expenses,
    })
}

fn financial_summary(
    conn: &Connection,
    from: &str,
    to: &str,
    owner_id: Option<&str>,
) -> Result<Value, ApiError> {
    let totals = conn
        .query_row(
            "WITH
             wash AS (
                SELECT COALESCE(SUM(price_milli),0) revenue,
                       COALESCE(SUM(CASE WHEN payment_type='cash' THEN price_milli ELSE 0 END),0) cash,
                       COALESCE(SUM(CASE WHEN payment_type='cash' AND is_paid=1 THEN price_milli ELSE 0 END),0) paid_customer_revenue,
                       COALESCE(SUM(CASE WHEN payment_type='showroom' THEN price_milli ELSE 0 END),0) showroom_revenue,
                       COALESCE(SUM(CASE WHEN payment_type='showroom' THEN commission_milli ELSE 0 END),0) showroom_commissions,
                       COALESCE(SUM(commission_milli),0) commissions,
                       COALESCE(SUM(business_share_milli),0) business_share,
                       COALESCE(SUM(CASE WHEN payment_type='cash' AND is_paid=1 THEN business_share_milli ELSE 0 END),0) paid_cars_profit
                FROM wash_operations
                WHERE status='posted' AND occurred_at BETWEEN ?1 AND ?2
                  AND (?3 IS NULL OR created_by=?3)
             ),
             expense AS (
                SELECT COALESCE(SUM(amount_milli),0) expenses,
                       COALESCE(SUM(business_amount_milli),0) business_expenses,
                       COALESCE(SUM(workers_amount_milli),0) workers_expenses
                FROM expenses
                WHERE occurred_at BETWEEN ?1 AND ?2 AND (?3 IS NULL OR created_by=?3)
             ),
             withdrawal AS (
                SELECT COALESCE(SUM(amount_milli),0) worker_withdrawals
                FROM salary_withdrawals
                WHERE withdrawn_at BETWEEN ?1 AND ?2 AND (?3 IS NULL OR created_by=?3)
             ),
             payment AS (
                SELECT COALESCE(SUM(amount_milli),0) showroom_payments
                FROM showroom_payments
                WHERE paid_at BETWEEN ?1 AND ?2 AND (?3 IS NULL OR created_by=?3)
             ),
             deduction AS (
                SELECT COALESCE(SUM(ea.amount_milli),0) worker_deductions
                FROM expense_allocations ea JOIN expenses e ON e.id=ea.expense_id
                WHERE e.occurred_at BETWEEN ?1 AND ?2 AND (?3 IS NULL OR e.created_by=?3)
             )
             SELECT wash.revenue,wash.cash,wash.paid_customer_revenue,wash.showroom_revenue,
                    wash.showroom_commissions,wash.commissions,wash.business_share,wash.paid_cars_profit,
                    expense.expenses,expense.business_expenses,expense.workers_expenses,
                    withdrawal.worker_withdrawals,payment.showroom_payments,deduction.worker_deductions
             FROM wash,expense,withdrawal,payment,deduction",
            params![from, to, owner_id],
            |row| Ok(FinancialTotals {
                revenue: row.get(0)?,
                cash: row.get(1)?,
                paid_customer_revenue: row.get(2)?,
                showroom_revenue: row.get(3)?,
                showroom_commissions: row.get(4)?,
                commissions: row.get(5)?,
                business_share: row.get(6)?,
                paid_cars_profit: row.get(7)?,
                expenses: row.get(8)?,
                business_expenses: row.get(9)?,
                workers_expenses: row.get(10)?,
                worker_withdrawals: row.get(11)?,
                showroom_payments: row.get(12)?,
                worker_deductions: row.get(13)?,
            }),
        )
        .map_err(ApiError::internal)?;
    Ok(financial_summary_value(&totals))
}

async fn finance_overview(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    let principal = authorize_section(
        &state,
        &headers,
        "section.finance.access",
        "financial.manage",
    )?;
    let (from, to) = date_range(&query)?;
    let result = blocking(move || {
        let db = state.read_db()?;
        financial_summary(
            &db.conn,
            &from,
            &to,
            if principal.is_manager() {
                None
            } else {
                Some(&principal.id)
            },
        )
    })
    .await?;
    Ok(ok(result))
}

async fn operational_report(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    let principal = authorize_section(
        &state,
        &headers,
        "section.reports.access",
        "operational.read",
    )?;
    let (from, to) = date_range(&query)?;
    let limit = query
        .get("limit")
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(50)
        .clamp(1, 200);
    let can_view_all = principal.is_manager();
    let owner_id = principal.id.clone();
    let scope = cursor_scope(&[
        "operational-report-washes",
        &from,
        &to,
        if can_view_all { "all" } else { "owner" },
        &owner_id,
    ]);
    let cursor = history_cursor(&query, "cursor", &scope)?;
    let (cursor_timestamp, cursor_id) = cursor_boundary(cursor.as_ref());
    let result = blocking(move || {
    let db = state.read_db()?;
    let mut workers = Vec::new();
    let mut wash_count = 0_i64;
    if can_view_all {
        let mut statement = db.conn.prepare(
            "SELECT worker.id,worker.full_name,COUNT(w.worker_id)
             FROM workers worker
             LEFT JOIN wash_operations w ON w.worker_id=worker.id AND w.status='posted'
                  AND w.occurred_at BETWEEN ?1 AND ?2
             GROUP BY worker.id
             ORDER BY COUNT(w.worker_id) DESC,worker.full_name",
        ).map_err(ApiError::internal)?;
        let rows = statement.query_map(params![&from, &to], |row| Ok(json!({
            "workerId":row.get::<_,String>(0)?,"workerName":row.get::<_,String>(1)?,"carsWashed":row.get::<_,i64>(2)?
        }))).map_err(ApiError::internal)?;
        for row in rows {
            let worker = row.map_err(ApiError::internal)?;
            wash_count += worker["carsWashed"].as_i64().unwrap_or(0);
            workers.push(worker);
        }
    } else {
        let mut statement = db.conn.prepare(
            "SELECT worker.id,worker.full_name,COUNT(w.worker_id)
             FROM workers worker
             LEFT JOIN wash_operations w ON w.worker_id=worker.id AND w.status='posted'
                  AND w.occurred_at BETWEEN ?1 AND ?2 AND w.created_by=?3
             GROUP BY worker.id
             ORDER BY COUNT(w.worker_id) DESC,worker.full_name",
        ).map_err(ApiError::internal)?;
        let rows = statement.query_map(params![&from, &to, &owner_id], |row| Ok(json!({
            "workerId":row.get::<_,String>(0)?,"workerName":row.get::<_,String>(1)?,"carsWashed":row.get::<_,i64>(2)?
        }))).map_err(ApiError::internal)?;
        for row in rows {
            let worker = row.map_err(ApiError::internal)?;
            wash_count += worker["carsWashed"].as_i64().unwrap_or(0);
            workers.push(worker);
        }
    }
    let mut washes = Vec::new();
    let mut history=db.conn.prepare("SELECT w.id,w.vehicle_make,w.vehicle_model,w.manufacture_year,w.license_plate,w.occurred_at,w.payment_type,w.status,worker.id,worker.full_name,showroom.id,showroom.name FROM wash_operations w JOIN workers worker ON worker.id=w.worker_id LEFT JOIN showrooms showroom ON showroom.id=w.showroom_id WHERE w.status='posted' AND w.occurred_at BETWEEN ?1 AND ?2 AND (?3=1 OR w.created_by=?4) AND (w.occurred_at<?5 OR (w.occurred_at=?5 AND w.id<?6)) ORDER BY w.occurred_at DESC,w.id DESC LIMIT ?7").map_err(ApiError::internal)?;
    let rows=history.query_map(params![&from,&to,if can_view_all { 1 } else { 0 },&owner_id,cursor_timestamp,cursor_id,limit+1],|row|Ok(json!({"id":row.get::<_,String>(0)?,"vehicleMake":row.get::<_,String>(1)?,"vehicleModel":row.get::<_,String>(2)?,"manufactureYear":row.get::<_,Option<i32>>(3)?,"licensePlate":row.get::<_,Option<String>>(4)?,"occurredAt":row.get::<_,String>(5)?,"paymentType":row.get::<_,String>(6)?,"status":row.get::<_,String>(7)?,"worker":{"id":row.get::<_,String>(8)?,"fullName":row.get::<_,String>(9)?},"showroom":row.get::<_,Option<String>>(10)?.map(|id|json!({"id":id,"name":row.get::<_,Option<String>>(11).ok().flatten()}))}))).map_err(ApiError::internal)?;
    for row in rows {
        washes.push(row.map_err(ApiError::internal)?);
    }
    let (has_more,next_cursor)=finish_cursor_page(&mut washes,limit,&scope,&["occurredAt"])?;
    Ok(json!({"from":from,"to":to,"carsWashed":wash_count,"workerPerformance":workers,"washes":washes,"washesHasMore":has_more,"washesNextCursor":next_cursor}))
    }).await?;
    Ok(ok(result))
}

async fn financial_report(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    let principal = authorize_section(
        &state,
        &headers,
        "section.reports.access",
        "financial.manage",
    )?;
    let (from, to) = date_range(&query)?;
    let result = blocking(move || {
    let db = state.read_db()?;
    let mut workers = Vec::new();
    let owner_id = if principal.is_manager() { None } else { Some(principal.id.as_str()) };
    let mut statement = db.conn.prepare(
        "WITH
         deduction_by_worker AS (
            SELECT ea.worker_id,COALESCE(SUM(ea.amount_milli),0) deductions_milli
            FROM expense_allocations ea JOIN expenses e ON e.id=ea.expense_id
            WHERE e.occurred_at BETWEEN ?1 AND ?2 AND (?3 IS NULL OR e.created_by=?3)
            GROUP BY ea.worker_id
         ),
         wash_by_worker AS (
            SELECT worker_id,COUNT(*) cars_washed,
                   COALESCE(SUM(price_milli),0) revenue_milli,
                   COALESCE(SUM(commission_milli),0) commission_milli,
                   COALESCE(SUM(CASE WHEN payment_type='cash' THEN price_milli ELSE 0 END),0) cash_milli,
                   COALESCE(SUM(CASE WHEN payment_type='cash' AND is_paid=1 THEN price_milli ELSE 0 END),0) paid_customer_revenue_milli,
                   COALESCE(SUM(CASE WHEN payment_type='showroom' THEN price_milli ELSE 0 END),0) showroom_revenue_milli,
                   COALESCE(SUM(CASE WHEN payment_type='showroom' THEN commission_milli ELSE 0 END),0) showroom_commissions_milli,
                   COALESCE(SUM(business_share_milli),0) business_share_milli,
                   COALESCE(SUM(CASE WHEN payment_type='cash' AND is_paid=1 THEN business_share_milli ELSE 0 END),0) paid_cars_profit_milli
            FROM wash_operations
            WHERE status='posted' AND occurred_at BETWEEN ?1 AND ?2
                  AND (?3 IS NULL OR created_by=?3)
            GROUP BY worker_id
         ),
         worker_rows AS (
            SELECT worker.id worker_id,worker.full_name worker_name,
                   COALESCE(wash_by_worker.cars_washed,0) cars_washed,
                   COALESCE(wash_by_worker.revenue_milli,0) revenue_milli,
                   COALESCE(wash_by_worker.commission_milli,0) commission_milli,
                   COALESCE(deduction_by_worker.deductions_milli,0) deductions_milli,
                   COALESCE(wash_by_worker.cash_milli,0) cash_milli,
                   COALESCE(wash_by_worker.paid_customer_revenue_milli,0) paid_customer_revenue_milli,
                   COALESCE(wash_by_worker.showroom_revenue_milli,0) showroom_revenue_milli,
                   COALESCE(wash_by_worker.showroom_commissions_milli,0) showroom_commissions_milli,
                   COALESCE(wash_by_worker.business_share_milli,0) business_share_milli,
                   COALESCE(wash_by_worker.paid_cars_profit_milli,0) paid_cars_profit_milli
            FROM workers worker
            LEFT JOIN wash_by_worker ON wash_by_worker.worker_id=worker.id
            LEFT JOIN deduction_by_worker ON deduction_by_worker.worker_id=worker.id
         ),
         report_totals AS (
            SELECT COALESCE(SUM(revenue_milli),0) revenue,
                   COALESCE(SUM(cash_milli),0) cash,
                   COALESCE(SUM(paid_customer_revenue_milli),0) paid_customer_revenue,
                   COALESCE(SUM(showroom_revenue_milli),0) showroom_revenue,
                   COALESCE(SUM(showroom_commissions_milli),0) showroom_commissions,
                   COALESCE(SUM(commission_milli),0) commissions,
                   COALESCE(SUM(business_share_milli),0) business_share,
                   COALESCE(SUM(paid_cars_profit_milli),0) paid_cars_profit,
                   COALESCE(SUM(deductions_milli),0) worker_deductions
            FROM worker_rows
         ),
         expense AS (
            SELECT COALESCE(SUM(amount_milli),0) expenses,
                   COALESCE(SUM(business_amount_milli),0) business_expenses,
                   COALESCE(SUM(workers_amount_milli),0) workers_expenses
            FROM expenses
            WHERE occurred_at BETWEEN ?1 AND ?2 AND (?3 IS NULL OR created_by=?3)
         ),
         withdrawal AS (
            SELECT COALESCE(SUM(amount_milli),0) worker_withdrawals
            FROM salary_withdrawals
            WHERE withdrawn_at BETWEEN ?1 AND ?2 AND (?3 IS NULL OR created_by=?3)
         ),
         payment AS (
            SELECT COALESCE(SUM(amount_milli),0) showroom_payments
            FROM showroom_payments
            WHERE paid_at BETWEEN ?1 AND ?2 AND (?3 IS NULL OR created_by=?3)
         )
         SELECT worker_rows.worker_id,worker_rows.worker_name,worker_rows.cars_washed,
                worker_rows.revenue_milli,worker_rows.commission_milli,worker_rows.deductions_milli,
                report_totals.revenue,report_totals.cash,report_totals.paid_customer_revenue,
                report_totals.showroom_revenue,report_totals.showroom_commissions,
                report_totals.commissions,report_totals.business_share,report_totals.paid_cars_profit,
                expense.expenses,expense.business_expenses,expense.workers_expenses,
                withdrawal.worker_withdrawals,payment.showroom_payments,report_totals.worker_deductions
         FROM (SELECT 1) anchor
         LEFT JOIN worker_rows ON 1=1
         CROSS JOIN report_totals CROSS JOIN expense CROSS JOIN withdrawal CROSS JOIN payment
         ORDER BY worker_rows.worker_name",
    ).map_err(ApiError::internal)?;
    let rows = statement.query_map(params![&from, &to, owner_id], |row| {
        let worker_id = row.get::<_, Option<String>>(0)?;
        let worker_name = row.get::<_, Option<String>>(1)?.unwrap_or_default();
        let cars_washed = row.get::<_, Option<i64>>(2)?.unwrap_or(0);
        let revenue_milli = row.get::<_, Option<i64>>(3)?.unwrap_or(0);
        let commission = row.get::<_, Option<i64>>(4)?.unwrap_or(0);
        let deductions = row.get::<_, Option<i64>>(5)?.unwrap_or(0);
        let worker = worker_id.map(|id| json!({
            "workerId": id,
            "workerName": worker_name,
            "carsWashed": cars_washed,
            "revenueMilli": revenue_milli,
            "commissionMilli": commission,
            "deductionsMilli": deductions,
            "remainingMilli": (commission-deductions).max(0)
        }));
        let totals = FinancialTotals {
            revenue: row.get(6)?, cash: row.get(7)?, paid_customer_revenue: row.get(8)?,
            showroom_revenue: row.get(9)?, showroom_commissions: row.get(10)?,
            commissions: row.get(11)?, business_share: row.get(12)?, paid_cars_profit: row.get(13)?,
            expenses: row.get(14)?, business_expenses: row.get(15)?, workers_expenses: row.get(16)?,
            worker_withdrawals: row.get(17)?, showroom_payments: row.get(18)?, worker_deductions: row.get(19)?,
        };
        Ok((worker, totals))
    }).map_err(ApiError::internal)?;
    let mut totals = None;
    for row in rows {
        let (worker, row_totals) = row.map_err(ApiError::internal)?;
        totals.get_or_insert(row_totals);
        if let Some(worker) = worker {
            workers.push(worker);
        }
    }
    let summary = financial_summary_value(&totals.unwrap_or_default());
    Ok(json!({"from":from,"to":to,"summary":summary,"workerPerformance":workers}))
    }).await?;
    Ok(ok(result))
}

async fn get_settings(State(state): State<AppState>, headers: HeaderMap) -> ApiResult {
    let _principal = authorize_section(
        &state,
        &headers,
        "section.settings.access",
        "settings.manage",
    )?;
    let db = state.read_db()?;
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
    let principal = authorize_section(
        &state,
        &headers,
        "section.settings.access",
        "settings.manage",
    )?;
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
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserUpdateInput {
    full_name: Option<String>,
    username: Option<String>,
    password: Option<String>,
    role_code: Option<String>,
    is_active: Option<bool>,
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
    let _principal =
        authorize_section(&state, &headers, "section.settings.access", "users.manage")?;
    let db = state.read_db()?;
    let mut items = Vec::new();
    let mut statement=db.conn.prepare("SELECT u.id,u.full_name,u.username_norm,u.is_active,u.created_at,r.code,r.name_ar,COALESCE(p.theme,'light') FROM users u JOIN user_roles ur ON ur.user_id=u.id JOIN roles r ON r.id=ur.role_id LEFT JOIN user_preferences p ON p.user_id=u.id WHERE u.deleted_at IS NULL ORDER BY u.created_at").map_err(ApiError::internal)?;
    let rows=statement.query_map([],|row|Ok((row.get::<_,String>(0)?,json!({"id":row.get::<_,String>(0)?,"fullName":row.get::<_,String>(1)?,"username":row.get::<_,String>(2)?,"isActive":row.get::<_,i64>(3)?==1,"createdAt":row.get::<_,String>(4)?,"roleCode":row.get::<_,String>(5)?,"roleName":row.get::<_,String>(6)?,"theme":row.get::<_,String>(7)?})))).map_err(ApiError::internal)?;
    for row in rows {
        items.push(row.map_err(ApiError::internal)?);
    }
    drop(statement);
    let mut permissions_by_user: HashMap<String, Vec<String>> = HashMap::new();
    let mut permission_statement = db
        .conn
        .prepare(
            "SELECT DISTINCT user_id,code FROM (
             SELECT role.user_id user_id,permission.code code
             FROM user_roles role
             JOIN users user ON user.id=role.user_id AND user.deleted_at IS NULL
             LEFT JOIN user_permission_profiles profile ON profile.user_id=role.user_id
             JOIN role_permissions role_permission ON role_permission.role_id=role.role_id
             JOIN permissions permission ON permission.id=role_permission.permission_id
             WHERE profile.user_id IS NULL
             UNION ALL
             SELECT user_permission.user_id,permission.code
             FROM user_permissions user_permission
             JOIN users user ON user.id=user_permission.user_id AND user.deleted_at IS NULL
             JOIN permissions permission ON permission.id=user_permission.permission_id
         )
         ORDER BY user_id,code",
        )
        .map_err(ApiError::internal)?;
    let permission_rows = permission_statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(ApiError::internal)?;
    for row in permission_rows {
        let (user_id, permission) = row.map_err(ApiError::internal)?;
        permissions_by_user
            .entry(user_id)
            .or_default()
            .push(permission);
    }
    let items = items
        .into_iter()
        .map(|(user_id, mut item)| {
            item["permissions"] = json!(permissions_by_user.remove(&user_id).unwrap_or_default());
            item
        })
        .collect::<Vec<_>>();
    Ok(ok(json!({"items":items})))
}

async fn create_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<UserInput>,
) -> ApiResult {
    let principal = authorize_section(&state, &headers, "section.settings.access", "users.manage")?;
    if input.role_code == "manager" && !principal.is_manager() {
        return Err(ApiError::forbidden());
    }
    let full_name = trim_required(&input.full_name, "الاسم الكامل")?;
    let username = normalized_username(&input.username)?;
    valid_password(&input.password)?;
    let hash = hash_password_blocking(input.password.clone()).await?;
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let role_id = role_id_for(&db.conn, &input.role_code)?;
    let id = new_id();
    let tx = db.conn.transaction().map_err(ApiError::internal)?;
    tx.execute("INSERT INTO users(id,full_name,username_norm,password_hash,is_active,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?6)",params![id,full_name,username,hash,if input.is_active.unwrap_or(true){1}else{0},now()]).map_err(|error|ApiError::new(StatusCode::CONFLICT,format!("تعذر إنشاء المستخدم: {error}")))?;
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
    let principal = authorize_section(&state, &headers, "section.settings.access", "users.manage")?;
    if id == principal.id && input.is_active == Some(false) {
        return Err(ApiError::bad("لا يمكنك تعطيل حسابك الحالي"));
    }
    if id == principal.id
        && input.role_code.as_deref().is_some()
        && input.role_code.as_deref() != Some("manager")
    {
        return Err(ApiError::bad("لا يمكنك خفض دور حسابك الحالي"));
    }
    // Normalize and hash request data before entering the writer critical section. In particular,
    // Argon2 is intentionally expensive and must not serialize unrelated financial writes.
    let full_name = match input.full_name.as_deref() {
        Some(value) => Some(trim_required(value, "الاسم الكامل")?),
        None => None,
    };
    let username = match input.username.as_deref() {
        Some(value) => Some(normalized_username(value)?),
        None => None,
    };
    let password_hash = match input.password.as_deref() {
        Some(value) => {
            valid_password(value)?;
            Some(hash_password_blocking(value.to_owned()).await?)
        }
        None => None,
    };
    let password_changed = password_hash.is_some();
    let revoke_sessions =
        password_changed || input.role_code.is_some() || input.is_active == Some(false);
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
    let role_id = match input.role_code.as_deref() {
        Some(code) => Some(role_id_for(&db.conn, code)?),
        None => None,
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
    if revoke_sessions {
        tx.execute(
            "UPDATE sessions SET revoked_at=?1 WHERE user_id=?2 AND revoked_at IS NULL",
            params![now(), id],
        )
        .map_err(ApiError::internal)?;
    }
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
    Ok(ok(
        json!({"updated":true,"reauthenticationRequired":password_changed}),
    ))
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
    for code in normalized_permission_codes(input.permission_codes) {
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
    let _principal =
        authorize_section(&state, &headers, "section.settings.access", "users.manage")?;
    let db = state.read_db()?;
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

fn normalized_permission_codes(mut codes: Vec<String>) -> Vec<String> {
    codes.sort();
    codes.dedup();
    let has_any = |candidates: &[&str]| {
        candidates
            .iter()
            .any(|candidate| codes.iter().any(|code| code == candidate))
    };
    let mut required = Vec::new();
    if has_any(&[
        "section.dashboard.access",
        "section.washes.access",
        "section.paid_cars.access",
        "section.overnight.access",
        "section.workers.access",
        "section.showrooms.access",
        "section.reports.access",
    ]) {
        required.push("operational.read");
    }
    if has_any(&[
        "section.reports.access",
        "section.finance.access",
        "section.showroom_debts.access",
        "section.salaries.access",
    ]) {
        required.push("financial.manage");
    }
    if has_any(&["section.settings.access"]) && !has_any(&["settings.manage", "users.manage"]) {
        required.push("settings.manage");
    }
    if has_any(&["section.audit.access"]) {
        required.push("audit.read");
    }
    if has_any(&["section.backup.access"]) {
        required.push("backup.manage");
    }
    for code in required {
        if !codes.iter().any(|existing| existing == code) {
            codes.push(code.to_owned());
        }
    }
    codes
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
    for code in normalized_permission_codes(input.permission_codes) {
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
    let _principal = authorize_section(&state, &headers, "section.audit.access", "audit.read")?;
    let (from, to) = date_range(&query)?;
    let db = state.read_db()?;
    let limit = query
        .get("limit")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(150)
        .clamp(1, 500);
    let scope = cursor_scope(&["audit-logs", &from, &to]);
    let cursor = history_cursor(&query, "cursor", &scope)?;
    let (cursor_timestamp, cursor_id) = cursor_boundary(cursor.as_ref());
    let mut items = Vec::new();
    let mut statement=db.conn.prepare("SELECT a.id,a.action,a.entity_type,a.entity_id,a.description,a.created_at,u.full_name FROM audit_logs a LEFT JOIN users u ON u.id=a.user_id WHERE a.created_at BETWEEN ?1 AND ?2 AND (a.created_at<?3 OR (a.created_at=?3 AND a.id<?4)) ORDER BY a.created_at DESC,a.id DESC LIMIT ?5").map_err(ApiError::internal)?;
    let rows=statement.query_map(params![from,to,cursor_timestamp,cursor_id,limit+1],|row|Ok(json!({"id":row.get::<_,String>(0)?,"action":row.get::<_,String>(1)?,"entityType":row.get::<_,String>(2)?,"entityId":row.get::<_,Option<String>>(3)?,"description":row.get::<_,String>(4)?,"createdAt":row.get::<_,String>(5)?,"userName":row.get::<_,Option<String>>(6)?}))).map_err(ApiError::internal)?;
    for row in rows {
        items.push(row.map_err(ApiError::internal)?);
    }
    let (has_more, next_cursor) = finish_cursor_page(&mut items, limit, &scope, &["createdAt"])?;
    Ok(ok(
        json!({"items":items,"hasMore":has_more,"nextCursor":next_cursor}),
    ))
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
    if !path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.eq_ignore_ascii_case("db"))
        .unwrap_or(false)
    {
        return Err(ApiError::bad("يجب أن يكون امتداد ملف النسخة الاحتياطية .db"));
    }
    path.parent()
        .ok_or_else(|| ApiError::bad("مسار النسخة الاحتياطية غير صالح"))?;
    Ok(path)
}

const FILE_IO_BUFFER_BYTES: usize = 256 * 1024;

fn temporary_sibling(path: &FsPath, purpose: &str) -> Result<PathBuf, ApiError> {
    let parent = path
        .parent()
        .ok_or_else(|| ApiError::internal("مسار الملف المؤقت غير صالح"))?;
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("backup.db");
    Ok(parent.join(format!(".{name}.{purpose}-{}.db", new_id())))
}

fn digest_hex(digest: impl AsRef<[u8]>) -> String {
    digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn sha256_file(path: &FsPath) -> Result<String, ApiError> {
    let file = fs::File::open(path).map_err(ApiError::internal)?;
    let mut reader = BufReader::with_capacity(FILE_IO_BUFFER_BYTES, file);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; FILE_IO_BUFFER_BYTES];
    loop {
        let read = reader.read(&mut buffer).map_err(ApiError::internal)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(digest_hex(hasher.finalize()))
}

fn copy_file_hashed(source: &FsPath, destination: &FsPath) -> Result<(u64, String), ApiError> {
    let source_file = fs::File::open(source).map_err(ApiError::internal)?;
    let destination_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(ApiError::internal)?;
    let mut reader = BufReader::with_capacity(FILE_IO_BUFFER_BYTES, source_file);
    let mut writer = BufWriter::with_capacity(FILE_IO_BUFFER_BYTES, destination_file);
    let mut hasher = Sha256::new();
    let mut copied = 0_u64;
    let mut buffer = vec![0_u8; FILE_IO_BUFFER_BYTES];
    loop {
        let read = reader.read(&mut buffer).map_err(ApiError::internal)?;
        if read == 0 {
            break;
        }
        writer
            .write_all(&buffer[..read])
            .map_err(ApiError::internal)?;
        hasher.update(&buffer[..read]);
        copied += read as u64;
    }
    writer.flush().map_err(ApiError::internal)?;
    writer.get_ref().sync_all().map_err(ApiError::internal)?;
    Ok((copied, digest_hex(hasher.finalize())))
}

fn finalize_new_file(temporary: &FsPath, target: &FsPath) -> Result<(), ApiError> {
    if target.exists() {
        let _ = fs::remove_file(temporary);
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "ملف النسخة الاحتياطية موجود بالفعل",
        ));
    }
    fs::rename(temporary, target).map_err(ApiError::internal)
}

fn replace_file_atomically(temporary: &FsPath, target: &FsPath) -> Result<(), ApiError> {
    if !target.exists() {
        return fs::rename(temporary, target).map_err(ApiError::internal);
    }
    let displaced = temporary_sibling(target, "previous")?;
    fs::rename(target, &displaced).map_err(ApiError::internal)?;
    match fs::rename(temporary, target) {
        Ok(()) => {
            let _ = fs::remove_file(displaced);
            Ok(())
        }
        Err(error) => {
            let _ = fs::rename(&displaced, target);
            Err(ApiError::internal(error))
        }
    }
}

fn expected_backup_hash(notes: Option<&str>) -> Option<&str> {
    notes?.strip_prefix("sha256:")
}

/// Writes the snapshot only. `VACUUM INTO` has to run on the live connection, so this is the one
/// backup step that legitimately holds the database lock; callers verify the produced file
/// afterwards, off the lock, via [`blocking`].
fn vacuum_into(conn: &Connection, path: &FsPath) -> Result<(), ApiError> {
    if path.exists() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "ملف النسخة الاحتياطية موجود بالفعل",
        ));
    }
    let escaped = path.to_string_lossy().replace('\'', "''");
    conn.execute_batch(&format!("VACUUM INTO '{escaped}'"))
        .map_err(ApiError::internal)
}

/// Full snapshot verification for the restore path, where the live connection is already held and
/// correctness outranks latency. Kept so emergency-snapshot creation kept its previous guarantees.
fn vacuum_into_verified(conn: &Connection, path: &FsPath) -> Result<(), ApiError> {
    vacuum_into(conn, path)?;
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
    let _principal = authorize_section(&state, &headers, "section.backup.access", "backup.manage")?;
    let (from, to) = date_range(&query)?;
    let limit = query
        .get("limit")
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(100)
        .clamp(1, 100);
    let scope = cursor_scope(&["backup-history", &from, &to]);
    let cursor = history_cursor(&query, "cursor", &scope)?;
    let (cursor_timestamp, cursor_id) = cursor_boundary(cursor.as_ref());

    // Phase 1: read the index rows under a short lock on an indexed, bounded query.
    let mut rows: Vec<(String, Option<String>, String)> = {
        let db = state.read_db()?;
        let mut statement=db.conn.prepare("SELECT id,backup_path,created_at FROM backup_history WHERE status='completed' AND created_at BETWEEN ?1 AND ?2 AND (created_at<?3 OR (created_at=?3 AND id<?4)) ORDER BY created_at DESC,id DESC LIMIT ?5").map_err(ApiError::internal)?;
        let mapped = statement
            .query_map(
                params![from, to, cursor_timestamp, cursor_id, limit + 1],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .map_err(ApiError::internal)?;
        let mut collected = Vec::new();
        for row in mapped {
            collected.push(row.map_err(ApiError::internal)?);
        }
        collected
    }; // the database guard is released here, before any filesystem access
    let has_more = rows.len() > limit as usize;
    if has_more {
        rows.pop();
    }
    let next_cursor = if has_more {
        let last = rows
            .last()
            .ok_or_else(|| ApiError::internal("تعذر إنشاء مؤشر سجل النسخ"))?;
        Some(encode_page_cursor(&last.2, &last.0, &scope)?)
    } else {
        None
    };

    // Phase 2: off the lock, one `stat` per row. Listing deliberately does NOT open backup files as
    // SQLite databases: integrity verification is reserved for download, export and restore, where
    // the file is actually consumed. Missing files are omitted without mutating history from GET;
    // explicit deletion remains the only destructive list operation.
    let items = blocking(move || {
        let mut items = Vec::new();
        for (id, path, created_at) in rows {
            let Some(path) = path.map(PathBuf::from) else {
                continue;
            };
            match fs::metadata(&path) {
                Ok(metadata) if metadata.is_file() => {
                    items.push(json!({"id":id.clone(),"path":path.to_string_lossy(),"createdAt":created_at,"sizeBytes":metadata.len(),"downloadUrl":format!("/api/backups/{id}/download")}));
                }
                _ => {}
            }
        }
        Ok(items)
    })
    .await?;
    Ok(ok(
        json!({"items":items,"hasMore":has_more,"nextCursor":next_cursor}),
    ))
}

async fn create_backup(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<BackupInput>,
) -> ApiResult {
    let principal = authorize_section(&state, &headers, "section.backup.access", "backup.manage")?;
    let path = safe_backup_path(&state.data_dir, input.path.as_deref())?;
    let temporary_path = temporary_sibling(&path, "creating")?;

    // Phase 1: the snapshot itself. `VACUUM INTO` must read the live connection, so the lock is
    // held for exactly this step and nothing else. It runs on the blocking pool because a large
    // database copy is synchronous even though independent WAL readers can continue.
    let snapshot_state = state.clone();
    let snapshot_path = temporary_path.clone();
    let snapshot_result = blocking(move || {
        let parent = snapshot_path
            .parent()
            .ok_or_else(|| ApiError::bad("مسار النسخة الاحتياطية غير صالح"))?;
        fs::create_dir_all(parent).map_err(ApiError::internal)?;
        let db = snapshot_state
            .db
            .lock()
            .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
        vacuum_into(&db.conn, &snapshot_path)
    })
    .await;
    if let Err(error) = snapshot_result {
        let _ = fs::remove_file(&temporary_path);
        return Err(error);
    }

    // Phase 2: verify the produced file off the lock. A backup is still never recorded as
    // 'completed' unless it passes verification; a failed snapshot is removed rather than kept.
    let verify_path = temporary_path.clone();
    let final_path = path.clone();
    let verification = blocking(move || {
        let result = (|| {
            Database::verify_backup(&verify_path).map_err(|_| {
                ApiError::internal("تعذر التحقق من سلامة النسخة الاحتياطية بعد إنشائها")
            })?;
            let size = fs::metadata(&verify_path)
                .map_err(ApiError::internal)?
                .len();
            let hash = sha256_file(&verify_path)?;
            fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&verify_path)
                .and_then(|file| file.sync_all())
                .map_err(ApiError::internal)?;
            finalize_new_file(&verify_path, &final_path)?;
            Ok((size, hash))
        })();
        if result.is_err() {
            let _ = fs::remove_file(&verify_path);
        }
        result
    })
    .await;
    let (size, hash) = verification?;

    // Phase 3: record the verified backup, under a short lock.
    let id = new_id();
    {
        let db = state
            .db
            .lock()
            .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
        db.conn.execute("INSERT INTO backup_history(id,backup_path,created_by,created_at,status,notes) VALUES(?1,?2,?3,?4,'completed',?5)",params![id,path.to_string_lossy().to_string(),principal.id,now(),format!("sha256:{hash}")]).map_err(ApiError::internal)?;
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
    }
    Ok(ok(
        json!({"id":id.clone(),"path":path.to_string_lossy(),"createdAt":now(),"sizeBytes":size,"sha256":hash,"downloadUrl":format!("/api/backups/{id}/download")}),
    ))
}

fn backup_path_for_id(conn: &Connection, id: &str) -> Result<(PathBuf, Option<String>), ApiError> {
    let record: Option<(Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT backup_path,notes FROM backup_history WHERE id=?1 AND status='completed'",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(ApiError::internal)?;
    record
        .and_then(|(path, notes)| path.map(|path| (PathBuf::from(path), notes)))
        .ok_or_else(ApiError::not_found)
}

async fn download_backup(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let _principal = authorize_section(&state, &headers, "section.backup.access", "backup.manage")?;
    let (path, notes) = {
        let db = state.read_db()?;
        backup_path_for_id(&db.conn, &id)?
    };
    // Verification is retained here because the file is about to leave the application, but it now
    // runs off the database lock together with the full-file read.
    let (path, filename, size, hash) = blocking(move || {
        if !path.is_file() || Database::verify_backup(&path).is_err() {
            return Err(ApiError::not_found());
        }
        let hash = sha256_file(&path)?;
        if expected_backup_hash(notes.as_deref()).is_some_and(|expected| expected != hash.as_str())
        {
            return Err(ApiError::not_found());
        }
        let size = fs::metadata(&path).map_err(ApiError::internal)?.len();
        let filename = path
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| value.is_ascii())
            .unwrap_or("alkaheli-backup.db")
            .to_owned();
        Ok((path, filename, size, hash))
    })
    .await?;
    let file = tokio::fs::File::open(path)
        .await
        .map_err(ApiError::internal)?;
    Response::builder()
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{filename}\""),
        )
        .header(header::CONTENT_LENGTH, size)
        .header(header::ETAG, format!("\"{hash}\""))
        .body(Body::from_stream(ReaderStream::new(file)))
        .map_err(ApiError::internal)
}

async fn delete_backup(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult {
    let principal = authorize_section(&state, &headers, "section.backup.access", "backup.manage")?;
    let (path, _) = {
        let db = state.read_db()?;
        backup_path_for_id(&db.conn, &id)?
    };
    blocking(move || {
        if path.is_file() {
            fs::remove_file(&path).map_err(ApiError::internal)?;
        }
        Ok(())
    })
    .await?;
    {
        let db = state
            .db
            .lock()
            .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
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
    }
    Ok(ok(json!({"deleted":true})))
}

async fn export_backup(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<BackupInput>,
) -> ApiResult {
    let _principal = authorize_section(&state, &headers, "section.backup.access", "backup.manage")?;
    let target = safe_backup_path(&state.data_dir, input.path.as_deref())?;
    let (source, notes) = {
        let db = state.read_db()?;
        backup_path_for_id(&db.conn, &id)?
    };
    // Source and target verification are both retained: the exported copy leaves the application.
    // Only the lock is given up, not any validation.
    let exported = blocking(move || {
        let parent = target
            .parent()
            .ok_or_else(|| ApiError::bad("مسار النسخة الاحتياطية غير صالح"))?;
        fs::create_dir_all(parent).map_err(ApiError::internal)?;
        let temporary = temporary_sibling(&target, "exporting")?;
        let result = (|| {
            if !source.is_file() || Database::verify_backup(&source).is_err() {
                return Err(ApiError::not_found());
            }
            if source.canonicalize().ok() == target.canonicalize().ok() {
                return Err(ApiError::bad("اختر موقعًا مختلفًا لحفظ النسخة"));
            }
            let (_, copied_hash) = copy_file_hashed(&source, &temporary)?;
            if expected_backup_hash(notes.as_deref())
                .is_some_and(|expected| expected != copied_hash.as_str())
            {
                return Err(ApiError::bad("فشل التحقق من بصمة النسخة الاحتياطية"));
            }
            Database::verify_backup(&temporary)
                .map_err(|_| ApiError::bad("تعذر التحقق من الملف المنزّل"))?;
            replace_file_atomically(&temporary, &target)?;
            Ok(target.to_string_lossy().to_string())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    })
    .await?;
    Ok(ok(json!({"exported":true,"path":exported})))
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
    let principal = authorize_section(&state, &headers, "section.backup.access", "backup.manage")?;
    if input.confirmation.trim() != "RESTORE" {
        return Err(ApiError::bad(
            "اكتب RESTORE لتأكيد استعادة النسخة الاحتياطية",
        ));
    }
    let source = PathBuf::from(input.path.trim());
    let payload = blocking(move || apply_restore(&state, &principal, &source)).await?;
    Ok(ok(payload))
}

async fn restore_backup_upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> ApiResult {
    let _principal = authorize_section(&state, &headers, "section.backup.access", "backup.manage")?;
    let staging_dir = state.data_dir.join(format!("restore-staged-{}", new_id()));
    tokio::fs::create_dir_all(&staging_dir)
        .await
        .map_err(ApiError::internal)?;
    let staged = staging_dir.join("carwash.db");
    let mut saved = false;
    let mut confirmed = false;
    let mut uploaded_hash = None;
    let intake: Result<(), ApiError> = async {
        while let Some(mut field) = multipart.next_field().await.map_err(ApiError::internal)? {
            let field_name = field.name().map(str::to_owned);
            match field_name.as_deref() {
                Some("confirmation") => {
                    let mut value = Vec::new();
                    while let Some(chunk) = field.chunk().await.map_err(ApiError::internal)? {
                        if value.len().saturating_add(chunk.len()) > 64 {
                            return Err(ApiError::bad("قيمة تأكيد الاستعادة غير صالحة"));
                        }
                        value.extend_from_slice(&chunk);
                    }
                    confirmed = std::str::from_utf8(&value)
                        .map_err(|_| ApiError::bad("قيمة تأكيد الاستعادة غير صالحة"))?
                        .trim()
                        == "RESTORE";
                }
                Some("backup") => {
                    if saved {
                        return Err(ApiError::bad("أرسل ملف نسخة احتياطية واحدًا فقط"));
                    }
                    let mut file = tokio::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&staged)
                        .await
                        .map_err(ApiError::internal)?;
                    let mut hasher = Sha256::new();
                    while let Some(chunk) = field.chunk().await.map_err(ApiError::internal)? {
                        file.write_all(&chunk).await.map_err(ApiError::internal)?;
                        hasher.update(&chunk);
                    }
                    file.flush().await.map_err(ApiError::internal)?;
                    file.sync_all().await.map_err(ApiError::internal)?;
                    uploaded_hash = Some(digest_hex(hasher.finalize()));
                    saved = true;
                }
                _ => {}
            }
        }
        Ok(())
    }
    .await;
    if let Err(error) = intake {
        let _ = tokio::fs::remove_dir_all(&staging_dir).await;
        return Err(error);
    }
    if !saved {
        let _ = tokio::fs::remove_dir_all(&staging_dir).await;
        return Err(ApiError::bad("اختر ملف نسخة احتياطية صالحًا"));
    }
    if !confirmed {
        let _ = tokio::fs::remove_dir_all(&staging_dir).await;
        return Err(ApiError::bad("لم يتم تأكيد استعادة النسخة الاحتياطية"));
    }
    let expected_hash =
        uploaded_hash.ok_or_else(|| ApiError::bad("تعذر حساب بصمة ملف النسخة الاحتياطية"))?;
    let result = blocking(move || {
        let result = apply_staged_restore(
            &state,
            &staging_dir,
            "ملف نسخة احتياطية مرفوع",
            Some(&expected_hash),
            || Ok(()),
        );
        let _ = fs::remove_dir_all(&staging_dir);
        result
    })
    .await;
    Ok(ok(result?))
}

fn apply_restore(
    state: &AppState,
    principal: &Principal,
    source: &FsPath,
) -> Result<Value, ApiError> {
    apply_restore_with_post_replace(state, principal, source, || Ok(()))
}

fn apply_restore_with_post_replace<F>(
    state: &AppState,
    _principal: &Principal,
    source: &FsPath,
    post_replace: F,
) -> Result<Value, ApiError>
where
    F: FnOnce() -> Result<(), ApiError>,
{
    if !source.is_file() {
        return Err(ApiError::bad("ملف النسخة الاحتياطية غير موجود"));
    }
    let current_path = state.data_dir.join("carwash.db");
    if source.canonicalize().ok() == current_path.canonicalize().ok() {
        return Err(ApiError::bad("لا يمكن استعادة قاعدة البيانات نفسها"));
    }
    let staging_dir = state.data_dir.join(format!("restore-staged-{}", new_id()));
    fs::create_dir_all(&staging_dir).map_err(ApiError::internal)?;
    let staged = staging_dir.join("carwash.db");
    let result = (|| {
        let (_, copied_hash) = copy_file_hashed(source, &staged)
            .map_err(|_| ApiError::bad("تعذر قراءة ملف النسخة الاحتياطية"))?;
        apply_staged_restore(
            state,
            &staging_dir,
            source.to_string_lossy().as_ref(),
            Some(&copied_hash),
            post_replace,
        )
    })();
    let _ = fs::remove_dir_all(&staging_dir);
    result
}

fn apply_staged_restore<F>(
    state: &AppState,
    staging_dir: &FsPath,
    source_description: &str,
    expected_hash: Option<&str>,
    post_replace: F,
) -> Result<Value, ApiError>
where
    F: FnOnce() -> Result<(), ApiError>,
{
    let staged = staging_dir.join("carwash.db");
    if !staged.is_file() {
        return Err(ApiError::bad("ملف النسخة الاحتياطية غير موجود"));
    }
    let staged_hash = sha256_file(&staged)?;
    if expected_hash.is_some_and(|expected| expected != staged_hash.as_str()) {
        return Err(ApiError::bad("فشل التحقق من بصمة ملف النسخة الاحتياطية"));
    }
    Database::prepare_restore_candidate(staging_dir)
        .map_err(|_| ApiError::bad("ملف النسخة الاحتياطية غير مكتمل أو غير متوافق مع هذا الإصدار"))?;
    apply_verified_restore(state, &staged, source_description, post_replace)
}

fn remove_database_sidecars(path: &FsPath) {
    let _ = fs::remove_file(path.with_extension("db-wal"));
    let _ = fs::remove_file(path.with_extension("db-shm"));
}

fn reopen_from_snapshot(
    data_dir: &FsPath,
    current_path: &FsPath,
    snapshot: &FsPath,
) -> Result<Database, String> {
    remove_database_sidecars(current_path);
    fs::copy(snapshot, current_path)
        .map_err(|error| format!("failed to restore emergency database: {error}"))?;
    let database = Database::open(data_dir)
        .map_err(|error| format!("failed to reopen emergency database: {error}"))?;
    database
        .verify_runtime_database()
        .map_err(|error| format!("emergency database verification failed: {error}"))?;
    Ok(database)
}

fn reopen_from_displaced(
    data_dir: &FsPath,
    current_path: &FsPath,
    displaced: &FsPath,
) -> Result<Database, String> {
    remove_database_sidecars(current_path);
    let _ = fs::remove_file(current_path);
    fs::rename(displaced, current_path)
        .map_err(|error| format!("failed to restore displaced database: {error}"))?;
    let database = Database::open(data_dir)
        .map_err(|error| format!("failed to reopen displaced database: {error}"))?;
    database
        .verify_runtime_database()
        .map_err(|error| format!("displaced database verification failed: {error}"))?;
    Ok(database)
}

fn apply_verified_restore<F>(
    state: &AppState,
    staged: &FsPath,
    source_description: &str,
    post_replace: F,
) -> Result<Value, ApiError>
where
    F: FnOnce() -> Result<(), ApiError>,
{
    // The writer lock protects the consistent emergency snapshot and prevents mutations while
    // restore metadata is collected. Independent WAL readers can continue during this long copy.
    let mut db = state
        .db
        .lock()
        .map_err(|_| ApiError::internal("قفل قاعدة البيانات"))?;
    let current_path = db.path.clone();
    let emergency = state.data_dir.join("backups").join(format!(
        "pre-restore-{}-{}.db",
        Utc::now().format("%Y%m%d-%H%M%S"),
        new_id()
    ));
    let emergency_parent = emergency
        .parent()
        .ok_or_else(|| ApiError::internal("مسار نسخة الطوارئ غير صالح"))?;
    fs::create_dir_all(emergency_parent).map_err(ApiError::internal)?;
    if let Err(error) = vacuum_into_verified(&db.conn, &emergency) {
        let _ = fs::remove_file(&emergency);
        return Err(error);
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

    // Only file replacement and reopen require every independent read connection to be closed.
    // Candidate hashing/migration/validation and emergency snapshot creation all happen first.
    let _exclusive_restore = state
        .read_gate
        .write()
        .map_err(|_| ApiError::internal("قفل استعادة قاعدة البيانات"))?;

    // Close the live connection only after both the restore candidate and emergency snapshot
    // have passed full integrity checks.
    let old_conn = std::mem::replace(
        &mut db.conn,
        Connection::open_in_memory().map_err(ApiError::internal)?,
    );
    drop(old_conn);
    remove_database_sidecars(&current_path);
    let displaced = state
        .data_dir
        .join(format!("pre-restore-live-{}.db", new_id()));

    let restore_attempt: Result<Database, ApiError> = (|| {
        fs::rename(&current_path, &displaced).map_err(ApiError::internal)?;
        fs::rename(staged, &current_path).map_err(ApiError::internal)?;
        post_replace()?;
        let mut reopened = Database::open(&state.data_dir).map_err(ApiError::internal)?;
        reopened
            .verify_runtime_database()
            .map_err(ApiError::internal)?;

        let tx = reopened.conn.transaction().map_err(ApiError::internal)?;
        for (id, path, created_at, notes) in preserved_backups {
            tx.execute(
                "INSERT OR IGNORE INTO backup_history(id,backup_path,created_by,created_at,status,notes) VALUES(?1,?2,NULL,?3,'completed',?4)",
                params![id,path,created_at,notes],
            )
            .map_err(ApiError::internal)?;
        }
        tx.execute("DELETE FROM sessions", [])
            .map_err(ApiError::internal)?;
        tx.execute(
            "INSERT INTO audit_logs(id,user_id,action,entity_type,entity_id,description,metadata_json,created_at)
             VALUES(?1,NULL,'BACKUP_RESTORED','backup',?2,?3,?4,?5)",
            params![
                new_id(),
                new_id(),
                "تمت استعادة نسخة احتياطية وإبطال كل الجلسات",
                source_description,
                now()
            ],
        )
        .map_err(ApiError::internal)?;
        tx.commit().map_err(ApiError::internal)?;
        reopened
            .verify_runtime_database()
            .map_err(ApiError::internal)?;
        Ok(reopened)
    })();

    match restore_attempt {
        Ok(reopened) => {
            *db = reopened;
            let _ = fs::remove_file(&displaced);
            Ok(
                json!({"restored":true,"emergencyBackupPath":emergency.to_string_lossy(),"reauthenticationRequired":true}),
            )
        }
        Err(_restore_error) => {
            eprintln!(
                "Restore failed after live replacement; rolling back the displaced live database"
            );
            match reopen_from_displaced(&state.data_dir, &current_path, &displaced) {
                Ok(original) => {
                    *db = original;
                    Err(ApiError::new(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "فشلت الاستعادة وتمت إعادة قاعدة البيانات الأصلية بأمان",
                    ))
                }
                Err(rollback_error) => {
                    eprintln!(
                        "Displaced database rollback failed ({rollback_error}); using {}",
                        emergency.display()
                    );
                    match reopen_from_snapshot(&state.data_dir, &current_path, &emergency) {
                        Ok(original) => {
                            *db = original;
                            let _ = fs::remove_file(&displaced);
                            Err(ApiError::new(
                                StatusCode::INTERNAL_SERVER_ERROR,
                                "فشلت الاستعادة وتمت إعادة قاعدة البيانات الأصلية بأمان",
                            ))
                        }
                        Err(emergency_error) => {
                            eprintln!(
                                "Critical restore rollback failure after both recovery attempts"
                            );
                            Err(ApiError::internal(emergency_error))
                        }
                    }
                }
            }
        }
    }
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

    #[test]
    fn percentage_rounding_remains_exact_for_large_integer_money_values() {
        let amount = i64::MAX - 7;
        assert_eq!(round_percentage(amount, 10_000), amount);
        assert_eq!(
            round_percentage(amount, 5_000),
            (((amount as i128) * 5_000 + 5_000) / 10_000) as i64
        );
    }

    #[test]
    fn incremental_sha256_and_streamed_copy_match_reference_digest() {
        let directory = std::env::temp_dir()
            .join("alkaheli-streamed-file-tests")
            .join(new_id());
        fs::create_dir_all(&directory).expect("test directory");
        let source = directory.join("source.db");
        let target = directory.join("target.db");
        let bytes: Vec<u8> = (0..(FILE_IO_BUFFER_BYTES * 2 + 31))
            .map(|index| ((index * 31 + 17) % 251) as u8)
            .collect();
        fs::write(&source, &bytes).expect("source file");

        let reference = digest_hex(Sha256::digest(&bytes));
        assert_eq!(sha256_file(&source).expect("streamed hash"), reference);
        let (copied, copied_hash) =
            copy_file_hashed(&source, &target).expect("streamed copy and hash");
        assert_eq!(copied, bytes.len() as u64);
        assert_eq!(copied_hash, reference);
        assert_eq!(sha256_file(&target).expect("copied file hash"), reference);
        assert_eq!(fs::read(&target).expect("copied bytes"), bytes);

        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn restore_hash_mismatch_is_rejected_before_live_replacement() {
        let data_dir = std::env::temp_dir()
            .join("alkaheli-restore-hash-tests")
            .join(new_id());
        let staging_dir = data_dir.join(format!("restore-staged-{}", new_id()));
        let live_database = Database::open(&data_dir).expect("live test database");
        live_database
            .conn
            .execute(
                "INSERT INTO settings(key,value_json,updated_by,updated_at) VALUES('restore-hash-marker','\"original\"',NULL,?1)",
                [now()],
            )
            .expect("original marker");
        let state = AppState {
            db: Arc::new(Mutex::new(live_database)),
            data_dir: data_dir.clone(),
            db_path: data_dir.join("carwash.db"),
            read_gate: Arc::new(RwLock::new(())),
        };
        let candidate = Database::open(&staging_dir).expect("restore candidate");
        candidate
            .verify_runtime_database()
            .expect("valid restore candidate");
        drop(candidate);

        let result = apply_staged_restore(
            &state,
            &staging_dir,
            "hash-mismatch-test",
            Some("0000000000000000000000000000000000000000000000000000000000000000"),
            || Ok(()),
        );
        assert!(
            result.is_err(),
            "mismatched candidate hash must be rejected"
        );
        let database = state.db.lock().expect("live database lock");
        let marker: String = database
            .conn
            .query_row(
                "SELECT value_json FROM settings WHERE key='restore-hash-marker'",
                [],
                |row| row.get(0),
            )
            .expect("live marker remains");
        assert_eq!(marker, "\"original\"");
        drop(database);
        drop(state);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn restore_failure_after_replacement_rolls_back_and_reconnects_original_database() {
        let data_dir = std::env::temp_dir()
            .join("alkaheli-restore-rollback-tests")
            .join(new_id());
        let source_dir = std::env::temp_dir()
            .join("alkaheli-restore-rollback-source-tests")
            .join(new_id());
        let live_database = Database::open(&data_dir).expect("live test database");
        live_database
            .conn
            .execute(
                "INSERT INTO settings(key,value_json,updated_by,updated_at) VALUES('restore-marker','\"original\"',NULL,?1)",
                [now()],
            )
            .expect("original marker");
        let state = AppState {
            db: Arc::new(Mutex::new(live_database)),
            data_dir: data_dir.clone(),
            db_path: data_dir.join("carwash.db"),
            read_gate: Arc::new(RwLock::new(())),
        };

        let source_database = Database::open(&source_dir).expect("source test database");
        source_database
            .conn
            .execute(
                "INSERT INTO settings(key,value_json,updated_by,updated_at) VALUES('restore-marker','\"replacement\"',NULL,?1)",
                [now()],
            )
            .expect("replacement marker");
        let source = source_dir.join("complete-backup.db");
        vacuum_into(&source_database.conn, &source).expect("complete source backup");
        drop(source_database);

        let principal = Principal {
            id: "restore-test-manager".to_owned(),
            full_name: "Restore Test Manager".to_owned(),
            username: "restore.test".to_owned(),
            role_code: "manager".to_owned(),
            role_name: "Manager".to_owned(),
            theme: "light".to_owned(),
            permissions: Vec::new(),
        };
        let result = apply_restore_with_post_replace(&state, &principal, &source, || {
            Err(ApiError::internal("forced post-replacement failure"))
        });
        assert!(
            result.is_err(),
            "the injected restore failure must be returned"
        );

        {
            let database = state.db.lock().expect("restored database lock");
            assert_eq!(database.path, data_dir.join("carwash.db"));
            database
                .verify_runtime_database()
                .expect("rolled-back live connection must be valid");
            let marker: String = database
                .conn
                .query_row(
                    "SELECT value_json FROM settings WHERE key='restore-marker'",
                    [],
                    |row| row.get(0),
                )
                .expect("original marker after rollback");
            assert_eq!(marker, "\"original\"");
        }

        drop(state);
        let reopened = Database::open(&data_dir).expect("rolled-back database must reopen");
        reopened
            .verify_runtime_database()
            .expect("reopened rolled-back database must be valid");
        let marker: String = reopened
            .conn
            .query_row(
                "SELECT value_json FROM settings WHERE key='restore-marker'",
                [],
                |row| row.get(0),
            )
            .expect("persisted original marker");
        assert_eq!(marker, "\"original\"");
        drop(reopened);
        let _ = fs::remove_dir_all(data_dir);
        let _ = fs::remove_dir_all(source_dir);
    }

    async fn financial_summary_for_uri(app: &Router, token: &str, uri: &str) -> Value {
        let (status, report) = request_json(app, Method::GET, uri, Some(token), None).await;
        assert_eq!(status, StatusCode::OK, "{report}");
        response_data(&report)["summary"].clone()
    }

    async fn create_test_wash(
        app: &Router,
        token: &str,
        worker_id: &str,
        showroom_id: Option<&str>,
        price: &str,
        occurred_at: &str,
        mark_paid: bool,
    ) -> String {
        let payment_type = if showroom_id.is_some() {
            "showroom"
        } else {
            "cash"
        };
        let (status, wash) = request_json(
            app,
            Method::POST,
            "/api/washes",
            Some(token),
            Some(json!({
                "vehicleMake":"Test","vehicleModel":"Finance","price":price,
                "workerId":worker_id,"paymentType":payment_type,"showroomId":showroom_id,
                "showroomPaymentMethod":showroom_id.map(|_| "cash"),
                "occurredAt":occurred_at,"clientRequestId":new_id()
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{wash}");
        let wash_id = response_data(&wash)["id"].as_str().unwrap().to_owned();
        if mark_paid {
            let (status, paid) = request_json(
                app,
                Method::PATCH,
                &format!("/api/washes/{wash_id}/paid"),
                Some(token),
                Some(json!({"isPaid":true})),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{paid}");
        }
        wash_id
    }

    #[tokio::test]
    async fn paid_customer_revenue_after_deductions_excludes_showrooms_and_filters_dates() {
        let data_dir =
            std::env::temp_dir().join(format!("alkaheli-revenue-card-test-{}", new_id()));
        let state = crate::create_state(data_dir.clone()).expect("test database");
        let app = build_router(state.clone());

        let (status, manager_setup) = request_json(
            &app,
            Method::POST,
            "/api/setup/initial-manager",
            None,
            Some(json!({"fullName":"مدير التقارير","username":"finance-manager","password":"StrongPass123!"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let token = response_data(&manager_setup)["token"]
            .as_str()
            .unwrap()
            .to_owned();

        let (status, worker) = request_json(
            &app,
            Method::POST,
            "/api/workers",
            Some(&token),
            Some(json!({"fullName":"عامل التقارير","commissionBpsOverride":5000})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let worker_id = response_data(&worker)["id"].as_str().unwrap().to_owned();

        let (status, showroom) = request_json(
            &app,
            Method::POST,
            "/api/showrooms",
            Some(&token),
            Some(json!({"name":"معرض اختبار التقارير"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let showroom_id = response_data(&showroom)["id"].as_str().unwrap().to_owned();

        let (status, employee) = request_json(
            &app,
            Method::POST,
            "/api/payroll/employees",
            Some(&token),
            Some(json!({"fullName":"موظف التقارير","month":"2025-01","salary":"1000"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{employee}");
        let employee_id = response_data(&employee)["employee"]["id"]
            .as_str()
            .unwrap()
            .to_owned();

        let day_ten = "/api/reports/financial?date=2025-01-10";
        let zero = financial_summary_for_uri(&app, &token, day_ten).await;
        assert_eq!(zero["paidCustomerRevenueMilli"], 0);
        assert_eq!(zero["paidCustomerRevenueAfterDeductionsMilli"], 0);

        create_test_wash(
            &app,
            &token,
            &worker_id,
            None,
            "1000",
            "2025-01-10T10:00:00Z",
            true,
        )
        .await;
        let paid_only = financial_summary_for_uri(&app, &token, day_ten).await;
        assert_eq!(paid_only["paidCustomerRevenueMilli"], 1_000_000);
        assert_eq!(
            paid_only["paidCustomerRevenueAfterDeductionsMilli"],
            1_000_000
        );

        create_test_wash(
            &app,
            &token,
            &worker_id,
            Some(&showroom_id),
            "500",
            "2025-01-10T11:00:00Z",
            false,
        )
        .await;
        let with_showroom = financial_summary_for_uri(&app, &token, day_ten).await;
        assert_eq!(with_showroom["paidCustomerRevenueMilli"], 1_000_000);
        assert_eq!(
            with_showroom["paidCustomerRevenueAfterDeductionsMilli"],
            1_000_000
        );

        let (status, withdrawal) = request_json(
            &app,
            Method::POST,
            "/api/payroll/withdrawals",
            Some(&token),
            Some(json!({"employeeId":employee_id,"amount":"200","withdrawnAt":"2025-01-10T12:00:00Z"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{withdrawal}");
        let after_withdrawal = financial_summary_for_uri(&app, &token, day_ten).await;
        assert_eq!(after_withdrawal["paidCustomerRevenueMilli"], 1_000_000);
        assert_eq!(
            after_withdrawal["paidCustomerRevenueAfterDeductionsMilli"],
            800_000
        );

        let (status, expense) = request_json(
            &app,
            Method::POST,
            "/api/expenses",
            Some(&token),
            Some(json!({"description":"مصروف الفترة","category":"أخرى","amount":"150","occurredAt":"2025-01-10T13:00:00Z","allocationType":"business"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{expense}");
        let after_both = financial_summary_for_uri(&app, &token, day_ten).await;
        assert_eq!(after_both["paidCustomerRevenueMilli"], 1_000_000);
        assert_eq!(
            after_both["paidCustomerRevenueAfterDeductionsMilli"],
            650_000
        );

        create_test_wash(
            &app,
            &token,
            &worker_id,
            Some(&showroom_id),
            "1000",
            "2025-01-11T10:00:00Z",
            false,
        )
        .await;
        let showroom_only =
            financial_summary_for_uri(&app, &token, "/api/reports/financial?date=2025-01-11").await;
        assert_eq!(showroom_only["paidCustomerRevenueMilli"], 0);
        assert_eq!(showroom_only["paidCustomerRevenueAfterDeductionsMilli"], 0);

        let (status, _) = request_json(
            &app,
            Method::POST,
            "/api/payroll/withdrawals",
            Some(&token),
            Some(json!({"employeeId":employee_id,"amount":"70","withdrawnAt":"2025-01-12T10:00:00Z"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = request_json(
            &app,
            Method::POST,
            "/api/expenses",
            Some(&token),
            Some(json!({"description":"مصروف تاريخ آخر","category":"أخرى","amount":"30","occurredAt":"2025-01-12T11:00:00Z","allocationType":"business"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let day_ten_again = financial_summary_for_uri(&app, &token, day_ten).await;
        assert_eq!(day_ten_again["paidCustomerRevenueMilli"], 1_000_000);
        assert_eq!(
            day_ten_again["paidCustomerRevenueAfterDeductionsMilli"],
            650_000
        );

        let range = financial_summary_for_uri(
            &app,
            &token,
            "/api/reports/financial?from=2025-01-10T00%3A00%3A00Z&to=2025-01-12T23%3A59%3A59Z",
        )
        .await;
        assert_eq!(range["paidCustomerRevenueMilli"], 1_000_000);
        assert_eq!(range["paidCustomerRevenueAfterDeductionsMilli"], 550_000);

        let empty_day =
            financial_summary_for_uri(&app, &token, "/api/reports/financial?date=2025-01-13").await;
        assert_eq!(empty_day["paidCustomerRevenueMilli"], 0);
        assert_eq!(empty_day["paidCustomerRevenueAfterDeductionsMilli"], 0);

        drop(app);
        drop(state);
        let _ = std::fs::remove_dir_all(data_dir);
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
