use chrono::{SecondsFormat, Utc};
use rusqlite::{Connection, OpenFlags, Result};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub const MONEY_SCALE: i64 = 1000;
pub const DEFAULT_COMMISSION_BPS: i64 = 5000;

pub fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

pub fn new_id() -> String {
    Uuid::new_v4().to_string()
}

pub fn default_data_dir() -> PathBuf {
    if let Ok(value) = std::env::var("ALKAHILI_DATA_DIR") {
        return PathBuf::from(value);
    }
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("data"))
        .join("AlkaheliCarWashERP")
}

pub struct Database {
    pub conn: Connection,
    pub path: PathBuf,
}

impl Database {
    pub fn open(data_dir: &Path) -> Result<Self> {
        fs::create_dir_all(data_dir)
            .map_err(|_| rusqlite::Error::InvalidPath(data_dir.to_path_buf()))?;
        let path = data_dir.join("carwash.db");
        let conn = Connection::open(&path)?;
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA busy_timeout = 5000;",
        )?;
        let mut database = Self { conn, path };
        database.migrate()?;
        Ok(database)
    }

    pub fn verify_backup(path: &Path) -> Result<()> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let integrity: String = conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        if integrity != "ok" {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let has_users: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='users'",
            [],
            |row| row.get(0),
        )?;
        if has_users != 1 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        Ok(())
    }

    fn migrate(&mut self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);

             CREATE TABLE IF NOT EXISTS roles (
                id TEXT PRIMARY KEY,
                code TEXT NOT NULL UNIQUE,
                name_ar TEXT NOT NULL,
                is_system INTEGER NOT NULL DEFAULT 1
             );
             CREATE TABLE IF NOT EXISTS permissions (
                id TEXT PRIMARY KEY,
                code TEXT NOT NULL UNIQUE,
                group_code TEXT NOT NULL,
                name_ar TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS role_permissions (
                role_id TEXT NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
                permission_id TEXT NOT NULL REFERENCES permissions(id) ON DELETE CASCADE,
                PRIMARY KEY(role_id, permission_id)
             );
             CREATE TABLE IF NOT EXISTS user_permission_profiles (
                user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
                updated_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS user_permissions (
                user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                permission_id TEXT NOT NULL REFERENCES permissions(id) ON DELETE CASCADE,
                PRIMARY KEY(user_id, permission_id)
             );
             CREATE TABLE IF NOT EXISTS users (
                id TEXT PRIMARY KEY,
                full_name TEXT NOT NULL,
                username_norm TEXT NOT NULL UNIQUE,
                password_hash TEXT NOT NULL,
                is_active INTEGER NOT NULL DEFAULT 1,
                deleted_at TEXT,
                deleted_by TEXT REFERENCES users(id) ON DELETE SET NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS user_profile_pictures (
                user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
                file_name TEXT NOT NULL,
                content_type TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS user_roles (
                user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                role_id TEXT NOT NULL REFERENCES roles(id) ON DELETE RESTRICT,
                PRIMARY KEY(user_id, role_id)
             );
             CREATE TABLE IF NOT EXISTS user_preferences (
                user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
                theme TEXT NOT NULL DEFAULT 'light' CHECK(theme IN ('light','dark','system')),
                updated_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                token_hash TEXT NOT NULL UNIQUE,
                user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                expires_at TEXT NOT NULL,
                revoked_at TEXT,
                created_at TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_sessions_token ON sessions(token_hash);
             CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value_json TEXT NOT NULL,
                updated_by TEXT REFERENCES users(id) ON DELETE SET NULL,
                updated_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS workers (
                id TEXT PRIMARY KEY,
                full_name TEXT NOT NULL,
                phone TEXT,
                notes TEXT,
                is_active INTEGER NOT NULL DEFAULT 1,
                commission_bps_override INTEGER CHECK(commission_bps_override BETWEEN 0 AND 10000),
                deactivated_at TEXT,
                deactivated_by TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS showrooms (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                contact_name TEXT,
                phone TEXT,
                notes TEXT,
                is_active INTEGER NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS wash_operations (
                id TEXT PRIMARY KEY,
                vehicle_make TEXT NOT NULL,
                vehicle_model TEXT NOT NULL,
                manufacture_year INTEGER,
                license_plate TEXT,
                car_color TEXT,
                price_milli INTEGER NOT NULL CHECK(price_milli > 0),
                worker_id TEXT NOT NULL REFERENCES workers(id) ON DELETE RESTRICT,
                payment_type TEXT NOT NULL CHECK(payment_type IN ('cash','showroom')),
                showroom_id TEXT REFERENCES showrooms(id) ON DELETE RESTRICT,
                showroom_payment_method TEXT,
                occurred_at TEXT NOT NULL,
                commission_bps INTEGER NOT NULL CHECK(commission_bps BETWEEN 0 AND 10000),
                commission_milli INTEGER NOT NULL CHECK(commission_milli >= 0),
                business_share_milli INTEGER NOT NULL CHECK(business_share_milli >= 0),
                created_by TEXT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
                client_request_id TEXT NOT NULL UNIQUE,
                status TEXT NOT NULL DEFAULT 'posted' CHECK(status IN ('posted','voided')),
                is_paid INTEGER NOT NULL DEFAULT 0 CHECK(is_paid IN (0,1)),
                paid_at TEXT,
                paid_by TEXT REFERENCES users(id) ON DELETE SET NULL,
                voided_at TEXT,
                void_reason TEXT,
                created_at TEXT NOT NULL,
                revision INTEGER NOT NULL DEFAULT 1,
                updated_at TEXT,
                updated_by TEXT,
                CHECK((payment_type='cash' AND showroom_id IS NULL) OR (payment_type='showroom' AND showroom_id IS NOT NULL))
             );
             CREATE INDEX IF NOT EXISTS idx_washes_occured ON wash_operations(occurred_at, status);
             CREATE INDEX IF NOT EXISTS idx_washes_worker ON wash_operations(worker_id, occurred_at);
             CREATE INDEX IF NOT EXISTS idx_washes_showroom ON wash_operations(showroom_id, occurred_at);
             CREATE TABLE IF NOT EXISTS worker_payments (
                id TEXT PRIMARY KEY,
                worker_id TEXT NOT NULL REFERENCES workers(id) ON DELETE RESTRICT,
                amount_milli INTEGER NOT NULL CHECK(amount_milli > 0),
                paid_at TEXT NOT NULL,
                notes TEXT,
                created_by TEXT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
                created_at TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_worker_payments_worker ON worker_payments(worker_id, paid_at);
             CREATE TABLE IF NOT EXISTS worker_salary_rates (
                worker_id TEXT NOT NULL REFERENCES workers(id) ON DELETE RESTRICT,
                effective_month TEXT NOT NULL CHECK(length(effective_month) = 7),
                salary_milli INTEGER NOT NULL CHECK(salary_milli > 0),
                set_by TEXT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY(worker_id, effective_month)
             );
             CREATE INDEX IF NOT EXISTS idx_worker_salary_rates_month ON worker_salary_rates(effective_month, worker_id);
             CREATE TABLE IF NOT EXISTS salary_withdrawals (
                id TEXT PRIMARY KEY,
                worker_id TEXT NOT NULL REFERENCES workers(id) ON DELETE RESTRICT,
                amount_milli INTEGER NOT NULL CHECK(amount_milli > 0),
                withdrawn_at TEXT NOT NULL,
                notes TEXT,
                created_by TEXT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
                created_at TEXT NOT NULL,
                updated_by TEXT REFERENCES users(id) ON DELETE RESTRICT,
                updated_at TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_salary_withdrawals_worker_month ON salary_withdrawals(worker_id, withdrawn_at);
             CREATE TABLE IF NOT EXISTS showroom_payments (
                id TEXT PRIMARY KEY,
                showroom_id TEXT NOT NULL REFERENCES showrooms(id) ON DELETE RESTRICT,
                amount_milli INTEGER NOT NULL CHECK(amount_milli > 0),
                paid_at TEXT NOT NULL,
                notes TEXT,
                created_by TEXT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
                created_at TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_showroom_payments_showroom ON showroom_payments(showroom_id, paid_at);
             CREATE TABLE IF NOT EXISTS expenses (
                id TEXT PRIMARY KEY,
                description TEXT NOT NULL,
                category TEXT NOT NULL,
                amount_milli INTEGER NOT NULL CHECK(amount_milli > 0),
                occurred_at TEXT NOT NULL,
                notes TEXT,
                allocation_type TEXT NOT NULL CHECK(allocation_type IN ('business','workers','shared')),
                business_bps INTEGER NOT NULL CHECK(business_bps BETWEEN 0 AND 10000),
                workers_bps INTEGER NOT NULL CHECK(workers_bps BETWEEN 0 AND 10000),
                business_amount_milli INTEGER NOT NULL CHECK(business_amount_milli >= 0),
                workers_amount_milli INTEGER NOT NULL CHECK(workers_amount_milli >= 0),
                created_by TEXT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
                created_at TEXT NOT NULL,
                CHECK(business_bps + workers_bps = 10000),
                CHECK(business_amount_milli + workers_amount_milli = amount_milli)
             );
             CREATE INDEX IF NOT EXISTS idx_expenses_date ON expenses(occurred_at);
             CREATE TABLE IF NOT EXISTS expense_allocations (
                id TEXT PRIMARY KEY,
                expense_id TEXT NOT NULL REFERENCES expenses(id) ON DELETE RESTRICT,
                worker_id TEXT NOT NULL REFERENCES workers(id) ON DELETE RESTRICT,
                amount_milli INTEGER NOT NULL CHECK(amount_milli >= 0),
                allocation_order INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                UNIQUE(expense_id, worker_id)
             );
             CREATE INDEX IF NOT EXISTS idx_expense_allocations_worker ON expense_allocations(worker_id);
             CREATE TABLE IF NOT EXISTS financial_transactions (
                id TEXT PRIMARY KEY,
                source_type TEXT NOT NULL,
                source_id TEXT NOT NULL,
                occurred_at TEXT NOT NULL,
                created_by TEXT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
                created_at TEXT NOT NULL,
                UNIQUE(source_type, source_id)
             );
             CREATE TABLE IF NOT EXISTS ledger_entries (
                id TEXT PRIMARY KEY,
                transaction_id TEXT NOT NULL REFERENCES financial_transactions(id) ON DELETE CASCADE,
                account_code TEXT NOT NULL,
                entry_side TEXT NOT NULL CHECK(entry_side IN ('debit','credit')),
                amount_milli INTEGER NOT NULL CHECK(amount_milli > 0),
                related_worker_id TEXT REFERENCES workers(id) ON DELETE RESTRICT,
                related_showroom_id TEXT REFERENCES showrooms(id) ON DELETE RESTRICT,
                created_at TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_ledger_transaction ON ledger_entries(transaction_id);
             CREATE TABLE IF NOT EXISTS audit_logs (
                id TEXT PRIMARY KEY,
                user_id TEXT REFERENCES users(id) ON DELETE SET NULL,
                action TEXT NOT NULL,
                entity_type TEXT NOT NULL,
                entity_id TEXT,
                description TEXT NOT NULL,
                metadata_json TEXT,
                created_at TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_audit_time ON audit_logs(created_at DESC);
             CREATE TABLE IF NOT EXISTS backup_history (
                id TEXT PRIMARY KEY,
                backup_path TEXT,
                created_by TEXT REFERENCES users(id) ON DELETE SET NULL,
                created_at TEXT NOT NULL,
                status TEXT NOT NULL,
                notes TEXT
             );",
        )?;
        self.ensure_column("workers", "deactivated_at", "deactivated_at TEXT")?;
        self.ensure_column("workers", "deactivated_by", "deactivated_by TEXT")?;
        self.ensure_column(
            "users",
            "worker_id",
            "worker_id TEXT REFERENCES workers(id) ON DELETE SET NULL",
        )?;
        // Older installations did not explicitly associate an employee login with a
        // worker. Preserve existing data while making the safest deterministic links:
        // first by an exact account/worker name, then by a single worker previously
        // used by that account's recorded wash operations.
        self.conn.execute(
            "UPDATE users
             SET worker_id=(
                 SELECT w.id FROM workers w
                 WHERE lower(trim(w.full_name)) IN (lower(trim(users.full_name)), lower(trim(users.username_norm)))
                 ORDER BY w.id LIMIT 1
             )
             WHERE worker_id IS NULL
               AND EXISTS(SELECT 1 FROM user_roles ur JOIN roles r ON r.id=ur.role_id WHERE ur.user_id=users.id AND r.code='employee')",
            [],
        )?;
        self.conn.execute(
            "UPDATE users
             SET worker_id=(
                 SELECT w.id FROM workers w
                 WHERE w.is_active=1
                   AND (lower(trim(users.full_name)) LIKE lower(trim(w.full_name)) || '%'
                        OR lower(trim(users.username_norm)) LIKE lower(trim(w.full_name)) || '%')
                   AND (abs(length(trim(users.full_name))-length(trim(w.full_name))) <= 1
                        OR abs(length(trim(users.username_norm))-length(trim(w.full_name))) <= 1)
                 ORDER BY w.id LIMIT 1
             )
             WHERE worker_id IS NULL
               AND (SELECT COUNT(*) FROM workers w
                    WHERE w.is_active=1
                      AND (lower(trim(users.full_name)) LIKE lower(trim(w.full_name)) || '%'
                           OR lower(trim(users.username_norm)) LIKE lower(trim(w.full_name)) || '%')
                      AND (abs(length(trim(users.full_name))-length(trim(w.full_name))) <= 1
                           OR abs(length(trim(users.username_norm))-length(trim(w.full_name))) <= 1))=1
               AND EXISTS(SELECT 1 FROM user_roles ur JOIN roles r ON r.id=ur.role_id WHERE ur.user_id=users.id AND r.code='employee')",
            [],
        )?;
        self.conn.execute(
            "UPDATE users
             SET worker_id=(SELECT MIN(wash.worker_id) FROM wash_operations wash WHERE wash.created_by=users.id)
             WHERE worker_id IS NULL
               AND (SELECT COUNT(DISTINCT wash.worker_id) FROM wash_operations wash WHERE wash.created_by=users.id)=1
               AND EXISTS(SELECT 1 FROM user_roles ur JOIN roles r ON r.id=ur.role_id WHERE ur.user_id=users.id AND r.code='employee')",
            [],
        )?;
        self.conn
            .execute_batch("CREATE INDEX IF NOT EXISTS idx_users_worker ON users(worker_id);")?;
        self.ensure_column(
            "wash_operations",
            "revision",
            "revision INTEGER NOT NULL DEFAULT 1",
        )?;
        self.ensure_column("wash_operations", "updated_at", "updated_at TEXT")?;
        self.ensure_column("wash_operations", "updated_by", "updated_by TEXT")?;
        self.ensure_column(
            "wash_operations",
            "showroom_payment_method",
            "showroom_payment_method TEXT",
        )?;
        self.ensure_column("wash_operations", "car_color", "car_color TEXT")?;
        self.ensure_column(
            "wash_operations",
            "is_paid",
            "is_paid INTEGER NOT NULL DEFAULT 0 CHECK(is_paid IN (0,1))",
        )?;
        self.ensure_column("wash_operations", "paid_at", "paid_at TEXT")?;
        self.ensure_column(
            "wash_operations",
            "paid_by",
            "paid_by TEXT REFERENCES users(id) ON DELETE SET NULL",
        )?;
        self.ensure_column("users", "deleted_at", "deleted_at TEXT")?;
        self.ensure_column(
            "users",
            "deleted_by",
            "deleted_by TEXT REFERENCES users(id) ON DELETE SET NULL",
        )?;
        self.conn.execute(
            "UPDATE users
             SET deleted_at=COALESCE(deleted_at,(
                    SELECT MIN(a.created_at) FROM audit_logs a
                    WHERE a.entity_type='user' AND a.entity_id=users.id AND a.action='USER_DELETED_SAFELY'
                 )),
                 deleted_by=COALESCE(deleted_by,(
                    SELECT a.user_id FROM audit_logs a
                    WHERE a.entity_type='user' AND a.entity_id=users.id AND a.action='USER_DELETED_SAFELY'
                    ORDER BY a.created_at DESC LIMIT 1
                 ))
             WHERE is_active=0 AND EXISTS(
                    SELECT 1 FROM audit_logs a
                    WHERE a.entity_type='user' AND a.entity_id=users.id AND a.action='USER_DELETED_SAFELY'
                 )",
            [],
        )?;
        self.conn.execute(
            "UPDATE wash_operations SET showroom_payment_method='cash' WHERE payment_type='showroom' AND showroom_payment_method IS NULL",
            [],
        )?;
        self.conn.execute(
            "INSERT OR IGNORE INTO schema_migrations(version, applied_at) VALUES(2, ?1)",
            [now()],
        )?;
        self.seed()?;
        self.conn.execute(
            "INSERT OR IGNORE INTO role_permissions(role_id,permission_id)
             SELECT 'role-manager', id FROM permissions WHERE code='worker.daily_value.manage'",
            [],
        )?;
        let permissions_seeded: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version=3",
            [],
            |row| row.get(0),
        )?;
        if permissions_seeded == 0 {
            for permission_id in ["perm-operational-read", "perm-operational-write"] {
                self.conn.execute(
                    "INSERT OR IGNORE INTO role_permissions(role_id, permission_id) VALUES('role-employee', ?1)",
                    [permission_id],
                )?;
            }
            for permission_id in [
                "perm-operational-read",
                "perm-operational-write",
                "perm-financial",
                "perm-settings",
                "perm-users",
                "perm-audit",
                "perm-backup",
            ] {
                self.conn.execute(
                    "INSERT OR IGNORE INTO role_permissions(role_id, permission_id) VALUES('role-manager', ?1)",
                    [permission_id],
                )?;
            }
            self.conn.execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES(3, ?1)",
                [now()],
            )?;
        }
        self.conn.execute(
            "INSERT OR IGNORE INTO schema_migrations(version, applied_at) VALUES(4, ?1)",
            [now()],
        )?;
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS salary_deductions (
                id TEXT PRIMARY KEY,
                worker_id TEXT NOT NULL REFERENCES workers(id) ON DELETE RESTRICT,
                amount_milli INTEGER NOT NULL CHECK(amount_milli > 0),
                deduction_month TEXT NOT NULL CHECK(length(deduction_month) = 7),
                notes TEXT,
                created_by TEXT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
                created_at TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_salary_deductions_worker_month ON salary_deductions(worker_id, deduction_month);"
        )?;
        self.ensure_column("salary_deductions", "deducted_at", "deducted_at TEXT")?;
        self.ensure_column(
            "salary_deductions",
            "updated_by",
            "updated_by TEXT REFERENCES users(id) ON DELETE RESTRICT",
        )?;
        self.ensure_column("salary_deductions", "updated_at", "updated_at TEXT")?;
        self.conn.execute(
            "UPDATE salary_deductions
             SET deducted_at=COALESCE(deducted_at,deduction_month || '-01T12:00:00Z'),
                 updated_at=COALESCE(updated_at,created_at)",
            [],
        )?;
        self.conn.execute(
            "INSERT OR IGNORE INTO schema_migrations(version, applied_at) VALUES(5, ?1)",
            [now()],
        )?;
        self.conn.execute(
            "INSERT OR IGNORE INTO schema_migrations(version, applied_at) VALUES(6, ?1)",
            [now()],
        )?;
        self.conn.execute(
            "INSERT OR IGNORE INTO role_permissions(role_id,permission_id)
             VALUES('role-manager','perm-dashboard-daily-revenue')",
            [],
        )?;
        self.conn.execute(
            "INSERT OR IGNORE INTO schema_migrations(version, applied_at) VALUES(7, ?1)",
            [now()],
        )?;
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS overnight_cars (
                id TEXT PRIMARY KEY,
                wash_id TEXT NOT NULL UNIQUE REFERENCES wash_operations(id) ON DELETE RESTRICT,
                marked_by TEXT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
                marked_at TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_overnight_cars_marked_at ON overnight_cars(marked_at);",
        )?;
        self.conn.execute(
            "INSERT OR IGNORE INTO schema_migrations(version, applied_at) VALUES(8, ?1)",
            [now()],
        )?;
        self.conn.execute(
            "INSERT OR IGNORE INTO schema_migrations(version, applied_at) VALUES(9, ?1)",
            [now()],
        )?;
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS worker_withdrawal_returns (
                id TEXT PRIMARY KEY,
                worker_id TEXT NOT NULL REFERENCES workers(id) ON DELETE RESTRICT,
                transaction_type TEXT NOT NULL CHECK(transaction_type IN ('withdrawal','return')),
                amount_milli INTEGER NOT NULL CHECK(amount_milli > 0),
                occurred_at TEXT NOT NULL,
                notes TEXT,
                created_by TEXT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
                created_at TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_worker_withdrawal_returns_worker ON worker_withdrawal_returns(worker_id, occurred_at DESC);"
        )?;
        self.conn.execute(
            "INSERT OR IGNORE INTO schema_migrations(version, applied_at) VALUES(10, ?1)",
            [now()],
        )?;
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS worker_daily_values (
                worker_id TEXT NOT NULL REFERENCES workers(id) ON DELETE RESTRICT,
                value_date TEXT NOT NULL CHECK(length(value_date) = 10),
                amount_milli INTEGER NOT NULL CHECK(amount_milli > 0),
                set_by TEXT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY(worker_id, value_date)
             );
             CREATE INDEX IF NOT EXISTS idx_worker_daily_values_date ON worker_daily_values(value_date, worker_id);"
        )?;
        self.conn.execute(
            "INSERT OR IGNORE INTO schema_migrations(version, applied_at) VALUES(11, ?1)",
            [now()],
        )?;
        let worker_payments_removed: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version=12",
            [],
            |row| row.get(0),
        )?;
        if worker_payments_removed == 0 {
            let tx = self.conn.transaction()?;
            tx.execute(
                "DELETE FROM ledger_entries
                 WHERE transaction_id IN (
                    SELECT id FROM financial_transactions WHERE source_type='worker_payment'
                 )",
                [],
            )?;
            tx.execute(
                "DELETE FROM financial_transactions WHERE source_type='worker_payment'",
                [],
            )?;
            tx.execute(
                "DELETE FROM audit_logs
                 WHERE entity_type='worker_payment' OR action='WORKER_PAYMENT_RECORDED'",
                [],
            )?;
            tx.execute("DELETE FROM worker_payments", [])?;
            tx.execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES(12, ?1)",
                [now()],
            )?;
            tx.commit()?;
        }
        self.conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_washes_paid_owner
             ON wash_operations(is_paid, status, created_by, occurred_at DESC);",
        )?;
        self.conn.execute(
            "INSERT OR IGNORE INTO schema_migrations(version, applied_at) VALUES(13, ?1)",
            [now()],
        )?;
        Ok(())
    }

    fn ensure_column(
        &mut self,
        table_name: &str,
        column_name: &str,
        column_definition: &str,
    ) -> Result<()> {
        let mut statement = self
            .conn
            .prepare(&format!("PRAGMA table_info({table_name})"))?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let existing_name: String = row.get(1)?;
            if existing_name == column_name {
                return Ok(());
            }
        }
        drop(rows);
        drop(statement);
        self.conn.execute_batch(&format!(
            "ALTER TABLE {table_name} ADD COLUMN {column_definition};"
        ))?;
        Ok(())
    }

    fn seed(&mut self) -> Result<()> {
        let timestamp = now();
        let manager_role = "role-manager";
        let employee_role = "role-employee";
        self.conn.execute(
            "INSERT OR IGNORE INTO roles(id, code, name_ar, is_system) VALUES(?1, 'manager', 'مدير', 1)",
            [manager_role],
        )?;
        self.conn.execute(
            "INSERT OR IGNORE INTO roles(id, code, name_ar, is_system) VALUES(?1, 'employee', 'موظف', 1)",
            [employee_role],
        )?;
        for (id, code, group, name) in [
            (
                "perm-operational-read",
                "operational.read",
                "operational",
                "عرض البيانات التشغيلية",
            ),
            (
                "perm-operational-write",
                "operational.write",
                "operational",
                "تسجيل عمليات الغسيل",
            ),
            (
                "perm-financial",
                "financial.manage",
                "financial",
                "إدارة البيانات المالية",
            ),
            (
                "perm-dashboard-daily-revenue",
                "dashboard.daily_revenue.read",
                "financial",
                "عرض إيراد اليوم",
            ),
            (
                "perm-worker-daily-value",
                "worker.daily_value.manage",
                "financial",
                "إدارة القيمة اليومية للعامل",
            ),
            (
                "perm-settings",
                "settings.manage",
                "administration",
                "إدارة الإعدادات",
            ),
            (
                "perm-users",
                "users.manage",
                "administration",
                "إدارة المستخدمين",
            ),
            (
                "perm-audit",
                "audit.read",
                "administration",
                "عرض سجل التدقيق",
            ),
            (
                "perm-backup",
                "backup.manage",
                "administration",
                "النسخ الاحتياطي والاستعادة",
            ),
        ] {
            self.conn.execute(
                "INSERT OR IGNORE INTO permissions(id, code, group_code, name_ar) VALUES(?1, ?2, ?3, ?4)",
                (id, code, group, name),
            )?;
        }
        for (key, value) in [
            ("business_name", "\"مركز الكحيلي لغسيل السيارات\""),
            ("currency", "\"د.ل\""),
            ("default_worker_commission_bps", "5000"),
        ] {
            self.conn.execute(
                "INSERT OR IGNORE INTO settings(key, value_json, updated_by, updated_at) VALUES(?1, ?2, NULL, ?3)",
                (key, value, &timestamp),
            )?;
        }
        Ok(())
    }
}

pub fn insert_audit(
    conn: &Connection,
    user_id: Option<&str>,
    action: &str,
    entity_type: &str,
    entity_id: Option<&str>,
    description: &str,
    metadata_json: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO audit_logs(id, user_id, action, entity_type, entity_id, description, metadata_json, created_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        (
            new_id(), user_id, action, entity_type, entity_id, description, metadata_json, now(),
        ),
    )?;
    Ok(())
}

pub fn default_commission_bps(conn: &Connection) -> Result<i64> {
    let raw: String = conn.query_row(
        "SELECT value_json FROM settings WHERE key='default_worker_commission_bps'",
        [],
        |row| row.get(0),
    )?;
    Ok(serde_json::from_str(&raw).unwrap_or(DEFAULT_COMMISSION_BPS))
}
