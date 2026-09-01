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
                wash_type TEXT,
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
                payment_method TEXT NOT NULL DEFAULT 'cash' CHECK(payment_method IN ('cash','bank')),
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
             );"
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
                transaction_type TEXT NOT NULL CHECK(transaction_type IN ('withdrawal','return','deduction_payment','settlement')),
                amount_milli INTEGER NOT NULL CHECK(amount_milli > 0),
                occurred_at TEXT NOT NULL,
                notes TEXT,
                created_by TEXT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
                created_at TEXT NOT NULL,
                deleted_at TEXT,
                deleted_by TEXT REFERENCES users(id) ON DELETE SET NULL
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
        let user_worker_link_removed: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version=14",
            [],
            |row| row.get(0),
        )?;
        if user_worker_link_removed == 0 {
            let has_worker_id = {
                let mut statement = self.conn.prepare("PRAGMA table_info(users)")?;
                let columns = statement.query_map([], |row| row.get::<_, String>(1))?;
                columns
                    .collect::<Result<Vec<_>>>()?
                    .iter()
                    .any(|column| column == "worker_id")
            };
            let tx = self.conn.transaction()?;
            tx.execute_batch("DROP INDEX IF EXISTS idx_users_worker;")?;
            if has_worker_id {
                tx.execute_batch("ALTER TABLE users DROP COLUMN worker_id;")?;
            }
            tx.execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES(14, ?1)",
                [now()],
            )?;
            tx.commit()?;
        }
        let worker_movement_settlements_added: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version=15",
            [],
            |row| row.get(0),
        )?;
        if worker_movement_settlements_added == 0 {
            let movement_schema: String = self.conn.query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='worker_withdrawal_returns'",
                [],
                |row| row.get(0),
            )?;
            let tx = self.conn.transaction()?;
            if !movement_schema.contains("'settlement'") || !movement_schema.contains("deleted_at") {
                tx.execute_batch(
                    "DROP INDEX IF EXISTS idx_worker_withdrawal_returns_worker;
                     ALTER TABLE worker_withdrawal_returns RENAME TO worker_withdrawal_returns_legacy;
                     CREATE TABLE worker_withdrawal_returns (
                        id TEXT PRIMARY KEY,
                        worker_id TEXT NOT NULL REFERENCES workers(id) ON DELETE RESTRICT,
                        transaction_type TEXT NOT NULL CHECK(transaction_type IN ('withdrawal','return','settlement')),
                        amount_milli INTEGER NOT NULL CHECK(amount_milli > 0),
                        occurred_at TEXT NOT NULL,
                        notes TEXT,
                        created_by TEXT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
                        created_at TEXT NOT NULL,
                        deleted_at TEXT,
                        deleted_by TEXT REFERENCES users(id) ON DELETE SET NULL
                     );
                     INSERT INTO worker_withdrawal_returns(
                        id,worker_id,transaction_type,amount_milli,occurred_at,notes,created_by,created_at,deleted_at,deleted_by
                     )
                     SELECT id,worker_id,transaction_type,amount_milli,occurred_at,notes,created_by,created_at,NULL,NULL
                     FROM worker_withdrawal_returns_legacy;
                     DROP TABLE worker_withdrawal_returns_legacy;",
                )?;
            }
            tx.execute_batch(
                "CREATE INDEX IF NOT EXISTS idx_worker_withdrawal_returns_worker
                 ON worker_withdrawal_returns(worker_id, occurred_at DESC);",
            )?;
            tx.execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES(15, ?1)",
                [now()],
            )?;
            tx.commit()?;
        }
        let worker_deduction_payments_added: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version=16",
            [],
            |row| row.get(0),
        )?;
        if worker_deduction_payments_added == 0 {
            let movement_schema: String = self.conn.query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='worker_withdrawal_returns'",
                [],
                |row| row.get(0),
            )?;
            let tx = self.conn.transaction()?;
            if !movement_schema.contains("'deduction_payment'") {
                tx.execute_batch(
                    "DROP INDEX IF EXISTS idx_worker_withdrawal_returns_worker;
                     ALTER TABLE worker_withdrawal_returns RENAME TO worker_withdrawal_returns_legacy;
                     CREATE TABLE worker_withdrawal_returns (
                        id TEXT PRIMARY KEY,
                        worker_id TEXT NOT NULL REFERENCES workers(id) ON DELETE RESTRICT,
                        transaction_type TEXT NOT NULL CHECK(transaction_type IN ('withdrawal','return','deduction_payment','settlement')),
                        amount_milli INTEGER NOT NULL CHECK(amount_milli > 0),
                        occurred_at TEXT NOT NULL,
                        notes TEXT,
                        created_by TEXT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
                        created_at TEXT NOT NULL,
                        deleted_at TEXT,
                        deleted_by TEXT REFERENCES users(id) ON DELETE SET NULL
                     );
                     INSERT INTO worker_withdrawal_returns(
                        id,worker_id,transaction_type,amount_milli,occurred_at,notes,created_by,created_at,deleted_at,deleted_by
                     )
                     SELECT id,worker_id,transaction_type,amount_milli,occurred_at,notes,created_by,created_at,deleted_at,deleted_by
                     FROM worker_withdrawal_returns_legacy;
                     DROP TABLE worker_withdrawal_returns_legacy;",
                )?;
            }
            tx.execute_batch(
                "CREATE INDEX IF NOT EXISTS idx_worker_withdrawal_returns_worker
                 ON worker_withdrawal_returns(worker_id, occurred_at DESC);",
            )?;
            tx.execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES(16, ?1)",
                [now()],
            )?;
            tx.commit()?;
        }
        let payroll_employees_separated: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version=17",
            [],
            |row| row.get(0),
        )?;
        if payroll_employees_separated == 0 {
            let tx = self.conn.transaction()?;
            tx.execute_batch(
                "CREATE TABLE payroll_employees (
                    id TEXT PRIMARY KEY,
                    full_name TEXT NOT NULL,
                    is_active INTEGER NOT NULL DEFAULT 1,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    archived_at TEXT,
                    archived_by TEXT REFERENCES users(id) ON DELETE SET NULL
                 );
                 INSERT INTO payroll_employees(id,full_name,is_active,created_at,updated_at,archived_at,archived_by)
                 SELECT 'payroll-' || worker.id,worker.full_name,worker.is_active,worker.created_at,worker.updated_at,
                        CASE WHEN worker.is_active=0 THEN worker.deactivated_at ELSE NULL END,
                        CASE WHEN worker.is_active=0 THEN worker.deactivated_by ELSE NULL END
                 FROM workers worker
                 WHERE EXISTS(SELECT 1 FROM worker_salary_rates rate WHERE rate.worker_id=worker.id)
                    OR EXISTS(SELECT 1 FROM salary_withdrawals withdrawal WHERE withdrawal.worker_id=worker.id)
                    OR EXISTS(SELECT 1 FROM salary_deductions deduction WHERE deduction.worker_id=worker.id);

                 DROP INDEX IF EXISTS idx_worker_salary_rates_month;
                 ALTER TABLE worker_salary_rates RENAME TO worker_salary_rates_legacy;
                 CREATE TABLE payroll_salary_rates (
                    employee_id TEXT NOT NULL REFERENCES payroll_employees(id) ON DELETE RESTRICT,
                    effective_month TEXT NOT NULL CHECK(length(effective_month) = 7),
                    salary_milli INTEGER NOT NULL CHECK(salary_milli > 0),
                    set_by TEXT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    PRIMARY KEY(employee_id, effective_month)
                 );
                 INSERT INTO payroll_salary_rates(employee_id,effective_month,salary_milli,set_by,created_at,updated_at)
                 SELECT 'payroll-' || worker_id,effective_month,salary_milli,set_by,created_at,updated_at
                 FROM worker_salary_rates_legacy;
                 DROP TABLE worker_salary_rates_legacy;
                 CREATE INDEX idx_payroll_salary_rates_month
                 ON payroll_salary_rates(effective_month, employee_id);

                 DROP INDEX IF EXISTS idx_salary_withdrawals_worker_month;
                 ALTER TABLE salary_withdrawals RENAME TO salary_withdrawals_legacy;
                 CREATE TABLE salary_withdrawals (
                    id TEXT PRIMARY KEY,
                    employee_id TEXT NOT NULL REFERENCES payroll_employees(id) ON DELETE RESTRICT,
                    amount_milli INTEGER NOT NULL CHECK(amount_milli > 0),
                    withdrawn_at TEXT NOT NULL,
                    notes TEXT,
                    created_by TEXT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
                    created_at TEXT NOT NULL,
                    updated_by TEXT REFERENCES users(id) ON DELETE RESTRICT,
                    updated_at TEXT NOT NULL
                 );
                 INSERT INTO salary_withdrawals(id,employee_id,amount_milli,withdrawn_at,notes,created_by,created_at,updated_by,updated_at)
                 SELECT id,'payroll-' || worker_id,amount_milli,withdrawn_at,notes,created_by,created_at,updated_by,updated_at
                 FROM salary_withdrawals_legacy;
                 DROP TABLE salary_withdrawals_legacy;
                 CREATE INDEX idx_salary_withdrawals_employee_month
                 ON salary_withdrawals(employee_id, withdrawn_at);

                 DROP INDEX IF EXISTS idx_salary_deductions_worker_month;
                 ALTER TABLE salary_deductions RENAME TO salary_deductions_legacy;
                 CREATE TABLE salary_deductions (
                    id TEXT PRIMARY KEY,
                    employee_id TEXT NOT NULL REFERENCES payroll_employees(id) ON DELETE RESTRICT,
                    amount_milli INTEGER NOT NULL CHECK(amount_milli > 0),
                    deduction_month TEXT NOT NULL CHECK(length(deduction_month) = 7),
                    notes TEXT,
                    created_by TEXT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
                    created_at TEXT NOT NULL,
                    deducted_at TEXT NOT NULL,
                    updated_by TEXT REFERENCES users(id) ON DELETE RESTRICT,
                    updated_at TEXT NOT NULL
                 );
                 INSERT INTO salary_deductions(id,employee_id,amount_milli,deduction_month,notes,created_by,created_at,deducted_at,updated_by,updated_at)
                 SELECT id,'payroll-' || worker_id,amount_milli,deduction_month,notes,created_by,created_at,deducted_at,updated_by,updated_at
                 FROM salary_deductions_legacy;
                 DROP TABLE salary_deductions_legacy;
                 CREATE INDEX idx_salary_deductions_employee_month
                 ON salary_deductions(employee_id, deduction_month);",
            )?;
            tx.execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES(17, ?1)",
                [now()],
            )?;
            tx.commit()?;
        }
        self.conn
            .execute_batch("DROP TABLE IF EXISTS worker_salary_rates;")?;
        let payroll_employee_ids_separated: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version=18",
            [],
            |row| row.get(0),
        )?;
        if payroll_employee_ids_separated == 0 {
            let tx = self.conn.transaction()?;
            tx.execute_batch(
                "INSERT INTO payroll_employees(id,full_name,is_active,created_at,updated_at,archived_at,archived_by)
                 SELECT 'payroll-' || employee.id,employee.full_name,employee.is_active,employee.created_at,employee.updated_at,employee.archived_at,employee.archived_by
                 FROM payroll_employees employee
                 WHERE EXISTS(SELECT 1 FROM workers worker WHERE worker.id=employee.id);
                 UPDATE payroll_salary_rates
                 SET employee_id='payroll-' || employee_id
                 WHERE EXISTS(SELECT 1 FROM workers worker WHERE worker.id=payroll_salary_rates.employee_id);
                 UPDATE salary_withdrawals
                 SET employee_id='payroll-' || employee_id
                 WHERE EXISTS(SELECT 1 FROM workers worker WHERE worker.id=salary_withdrawals.employee_id);
                 UPDATE salary_deductions
                 SET employee_id='payroll-' || employee_id
                 WHERE EXISTS(SELECT 1 FROM workers worker WHERE worker.id=salary_deductions.employee_id);
                 DELETE FROM payroll_employees
                 WHERE EXISTS(SELECT 1 FROM workers worker WHERE worker.id=payroll_employees.id);",
            )?;
            tx.execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES(18, ?1)",
                [now()],
            )?;
            tx.commit()?;
        }
        let wash_type_added: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version=19",
            [],
            |row| row.get(0),
        )?;
        if wash_type_added == 0 {
            self.ensure_column("wash_operations", "wash_type", "wash_type TEXT")?;
            self.conn.execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES(19, ?1)",
                [now()],
            )?;
        }
        let expense_payment_method_added: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version=20",
            [],
            |row| row.get(0),
        )?;
        if expense_payment_method_added == 0 {
            self.ensure_column(
                "expenses",
                "payment_method",
                "payment_method TEXT NOT NULL DEFAULT 'cash' CHECK(payment_method IN ('cash','bank'))",
            )?;
            self.conn.execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES(20, ?1)",
                [now()],
            )?;
        }
        let section_permissions_added: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version=21",
            [],
            |row| row.get(0),
        )?;
        if section_permissions_added == 0 {
            let operational_sections = [
                "section.dashboard.access",
                "section.washes.access",
                "section.paid_cars.access",
                "section.overnight.access",
                "section.workers.access",
                "section.showrooms.access",
                "section.reports.access",
            ];
            let financial_sections = [
                "section.finance.access",
                "section.showroom_debts.access",
                "section.salaries.access",
            ];
            for code in operational_sections {
                self.conn.execute(
                    "INSERT OR IGNORE INTO role_permissions(role_id,permission_id)
                     SELECT rp.role_id,p.id FROM role_permissions rp
                     JOIN permissions legacy ON legacy.id=rp.permission_id
                     JOIN permissions p ON p.code=?1
                     WHERE legacy.code='operational.read'",
                    [code],
                )?;
                self.conn.execute(
                    "INSERT OR IGNORE INTO user_permissions(user_id,permission_id)
                     SELECT up.user_id,p.id FROM user_permissions up
                     JOIN permissions legacy ON legacy.id=up.permission_id
                     JOIN permissions p ON p.code=?1
                     JOIN user_permission_profiles profile ON profile.user_id=up.user_id
                     WHERE legacy.code='operational.read'",
                    [code],
                )?;
            }
            for code in financial_sections {
                self.conn.execute(
                    "INSERT OR IGNORE INTO role_permissions(role_id,permission_id)
                     SELECT rp.role_id,p.id FROM role_permissions rp
                     JOIN permissions legacy ON legacy.id=rp.permission_id
                     JOIN permissions p ON p.code=?1
                     WHERE legacy.code='financial.manage'",
                    [code],
                )?;
                self.conn.execute(
                    "INSERT OR IGNORE INTO user_permissions(user_id,permission_id)
                     SELECT up.user_id,p.id FROM user_permissions up
                     JOIN permissions legacy ON legacy.id=up.permission_id
                     JOIN permissions p ON p.code=?1
                     JOIN user_permission_profiles profile ON profile.user_id=up.user_id
                     WHERE legacy.code='financial.manage'",
                    [code],
                )?;
            }
            for (section_code, legacy_codes) in [
                ("section.settings.access", ["settings.manage", "users.manage"]),
                ("section.audit.access", ["audit.read", "audit.read"]),
                ("section.backup.access", ["backup.manage", "backup.manage"]),
            ] {
                self.conn.execute(
                    "INSERT OR IGNORE INTO role_permissions(role_id,permission_id)
                     SELECT rp.role_id,p.id FROM role_permissions rp
                     JOIN permissions legacy ON legacy.id=rp.permission_id
                     JOIN permissions p ON p.code=?1
                     WHERE legacy.code IN (?2,?3)",
                    (section_code, legacy_codes[0], legacy_codes[1]),
                )?;
                self.conn.execute(
                    "INSERT OR IGNORE INTO user_permissions(user_id,permission_id)
                     SELECT up.user_id,p.id FROM user_permissions up
                     JOIN permissions legacy ON legacy.id=up.permission_id
                     JOIN permissions p ON p.code=?1
                     JOIN user_permission_profiles profile ON profile.user_id=up.user_id
                     WHERE legacy.code IN (?2,?3)",
                    (section_code, legacy_codes[0], legacy_codes[1]),
                )?;
            }
            for code in [
                "section.dashboard.access",
                "section.washes.access",
                "section.paid_cars.access",
                "section.overnight.access",
                "section.workers.access",
                "section.showrooms.access",
                "section.reports.access",
                "section.finance.access",
                "section.showroom_debts.access",
                "section.salaries.access",
                "section.settings.access",
                "section.audit.access",
                "section.backup.access",
            ] {
                self.conn.execute(
                    "INSERT OR IGNORE INTO role_permissions(role_id,permission_id)
                     SELECT 'role-manager',id FROM permissions WHERE code=?1",
                    [code],
                )?;
            }
            self.conn.execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES(21, ?1)",
                [now()],
            )?;
        }
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
            ("perm-section-dashboard", "section.dashboard.access", "sections", "لوحة المتابعة"),
            ("perm-section-washes", "section.washes.access", "sections", "عمليات الغسيل"),
            ("perm-section-paid-cars", "section.paid_cars.access", "sections", "السيارات الخالصة"),
            ("perm-section-overnight", "section.overnight.access", "sections", "سيارات المبيت"),
            ("perm-section-workers", "section.workers.access", "sections", "العمال"),
            ("perm-section-showrooms", "section.showrooms.access", "sections", "المعارض"),
            ("perm-section-reports", "section.reports.access", "sections", "التقارير المالية"),
            ("perm-section-finance", "section.finance.access", "sections", "التنفيذ المالي"),
            ("perm-section-showroom-debts", "section.showroom_debts.access", "sections", "ديون المعارض"),
            ("perm-section-salaries", "section.salaries.access", "sections", "المرتبات"),
            ("perm-section-settings", "section.settings.access", "sections", "الإعدادات"),
            ("perm-section-audit", "section.audit.access", "sections", "سجل التدقيق"),
            ("perm-section-backup", "section.backup.access", "sections", "النسخ الاحتياطي"),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expense_payment_method_migration_preserves_existing_expenses() {
        let data_dir = std::env::temp_dir()
            .join("alkaheli-expense-payment-method-migration-tests")
            .join(Uuid::new_v4().to_string());
        fs::create_dir_all(&data_dir).unwrap();
        let database = Database::open(&data_dir).unwrap();
        database.conn.execute(
            "INSERT INTO users(id,full_name,username_norm,password_hash,is_active,created_at,updated_at) VALUES('legacy-user','Legacy Manager','legacy.manager','unused',1,'2026-08-01T00:00:00Z','2026-08-01T00:00:00Z')",
            [],
        ).unwrap();
        database.conn.execute_batch(
            "DELETE FROM schema_migrations WHERE version=20;
             ALTER TABLE expenses DROP COLUMN payment_method;
             INSERT INTO expenses(id,description,category,amount_milli,occurred_at,notes,allocation_type,business_bps,workers_bps,business_amount_milli,workers_amount_milli,created_by,created_at)
             VALUES('legacy-expense','Legacy expense','Legacy category',42000,'2026-08-01T12:00:00Z','preserve me','business',10000,0,42000,0,'legacy-user','2026-08-01T12:00:00Z');",
        ).unwrap();
        drop(database);

        let reopened = Database::open(&data_dir).unwrap();
        let stored: (String, String, i64, String, String) = reopened.conn.query_row(
            "SELECT description,category,amount_milli,notes,payment_method FROM expenses WHERE id='legacy-expense'",
            [],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)),
        ).unwrap();
        assert_eq!(stored, ("Legacy expense".to_owned(), "Legacy category".to_owned(), 42_000, "preserve me".to_owned(), "cash".to_owned()));
        drop(reopened);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn section_permission_migration_preserves_explicit_employee_permissions() {
        let data_dir = std::env::temp_dir()
            .join("alkaheli-section-permission-migration-tests")
            .join(Uuid::new_v4().to_string());
        fs::create_dir_all(&data_dir).unwrap();
        let database = Database::open(&data_dir).unwrap();
        database.conn.execute(
            "INSERT INTO users(id,full_name,username_norm,password_hash,is_active,created_at,updated_at) VALUES('section-user','Section Employee','section.employee','unused',1,'2026-09-01T00:00:00Z','2026-09-01T00:00:00Z')",
            [],
        ).unwrap();
        database.conn.execute(
            "INSERT INTO user_roles(user_id,role_id) VALUES('section-user','role-employee')",
            [],
        ).unwrap();
        database.conn.execute(
            "INSERT INTO user_permission_profiles(user_id,updated_at) VALUES('section-user','2026-09-01T00:00:00Z')",
            [],
        ).unwrap();
        for code in [
            "operational.read",
            "operational.write",
            "dashboard.daily_revenue.read",
            "worker.daily_value.manage",
        ] {
            database.conn.execute(
                "INSERT INTO user_permissions(user_id,permission_id) SELECT 'section-user',id FROM permissions WHERE code=?1",
                [code],
            ).unwrap();
        }
        database.conn.execute_batch(
            "DELETE FROM schema_migrations WHERE version=21;
             DELETE FROM permissions WHERE group_code='sections';",
        ).unwrap();
        drop(database);

        let reopened = Database::open(&data_dir).unwrap();
        let mut statement = reopened.conn.prepare(
            "SELECT p.code FROM user_permissions up JOIN permissions p ON p.id=up.permission_id WHERE up.user_id='section-user' ORDER BY p.code",
        ).unwrap();
        let permissions = statement.query_map([], |row| row.get::<_, String>(0)).unwrap()
            .collect::<Result<Vec<_>, _>>().unwrap();
        for preserved in [
            "operational.read",
            "operational.write",
            "dashboard.daily_revenue.read",
            "worker.daily_value.manage",
        ] {
            assert!(permissions.iter().any(|code| code == preserved));
        }
        for migrated in [
            "section.dashboard.access",
            "section.washes.access",
            "section.paid_cars.access",
            "section.overnight.access",
            "section.workers.access",
            "section.showrooms.access",
            "section.reports.access",
        ] {
            assert!(permissions.iter().any(|code| code == migrated));
        }
        assert!(!permissions.iter().any(|code| code == "section.finance.access"));
        drop(statement);
        drop(reopened);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn migrations_preserve_workers_and_separate_legacy_payroll_records() {
        let data_dir = std::env::temp_dir()
            .join("alkaheli-user-worker-migration-tests")
            .join(Uuid::new_v4().to_string());
        fs::create_dir_all(&data_dir).unwrap();
        let database_path = data_dir.join("carwash.db");
        let connection = Connection::open(&database_path).unwrap();
        connection.execute_batch(
            "PRAGMA foreign_keys=ON;
             CREATE TABLE workers (
                id TEXT PRIMARY KEY,
                full_name TEXT NOT NULL,
                phone TEXT,
                notes TEXT,
                is_active INTEGER NOT NULL DEFAULT 1,
                commission_bps_override INTEGER,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
             );
             CREATE TABLE users (
                id TEXT PRIMARY KEY,
                full_name TEXT NOT NULL,
                username_norm TEXT NOT NULL UNIQUE,
                password_hash TEXT NOT NULL,
                is_active INTEGER NOT NULL DEFAULT 1,
                worker_id TEXT REFERENCES workers(id) ON DELETE SET NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
             );
             CREATE INDEX idx_users_worker ON users(worker_id);
             CREATE TABLE worker_withdrawal_returns (
                id TEXT PRIMARY KEY,
                worker_id TEXT NOT NULL REFERENCES workers(id) ON DELETE RESTRICT,
                transaction_type TEXT NOT NULL CHECK(transaction_type IN ('withdrawal','return')),
                amount_milli INTEGER NOT NULL CHECK(amount_milli > 0),
                occurred_at TEXT NOT NULL,
                notes TEXT,
                created_by TEXT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
                created_at TEXT NOT NULL
             );
             CREATE INDEX idx_worker_withdrawal_returns_worker
             ON worker_withdrawal_returns(worker_id, occurred_at DESC);
             CREATE TABLE worker_salary_rates (
                worker_id TEXT NOT NULL REFERENCES workers(id) ON DELETE RESTRICT,
                effective_month TEXT NOT NULL,
                salary_milli INTEGER NOT NULL,
                set_by TEXT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY(worker_id,effective_month)
             );
             CREATE TABLE salary_withdrawals (
                id TEXT PRIMARY KEY,
                worker_id TEXT NOT NULL REFERENCES workers(id) ON DELETE RESTRICT,
                amount_milli INTEGER NOT NULL,
                withdrawn_at TEXT NOT NULL,
                notes TEXT,
                created_by TEXT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
                created_at TEXT NOT NULL,
                updated_by TEXT REFERENCES users(id) ON DELETE RESTRICT,
                updated_at TEXT NOT NULL
             );
             CREATE TABLE salary_deductions (
                id TEXT PRIMARY KEY,
                worker_id TEXT NOT NULL REFERENCES workers(id) ON DELETE RESTRICT,
                amount_milli INTEGER NOT NULL,
                deduction_month TEXT NOT NULL,
                notes TEXT,
                created_by TEXT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
                created_at TEXT NOT NULL,
                deducted_at TEXT NOT NULL,
                updated_by TEXT REFERENCES users(id) ON DELETE RESTRICT,
                updated_at TEXT NOT NULL
             );
             INSERT INTO workers(id,full_name,is_active,created_at,updated_at)
             VALUES('worker-existing','عامل محفوظ',1,'2026-08-01','2026-08-01');
             INSERT INTO users(id,full_name,username_norm,password_hash,is_active,worker_id,created_at,updated_at)
             VALUES('user-existing','مستخدم محفوظ','existing.user','hash',1,'worker-existing','2026-08-01','2026-08-01');
             INSERT INTO worker_withdrawal_returns(id,worker_id,transaction_type,amount_milli,occurred_at,notes,created_by,created_at)
             VALUES('movement-existing','worker-existing','withdrawal',500000,'2026-08-10T12:00:00Z','سجل محفوظ','user-existing','2026-08-10T12:00:00Z');
             INSERT INTO worker_salary_rates(worker_id,effective_month,salary_milli,set_by,created_at,updated_at)
             VALUES('worker-existing','2026-08',1000000,'user-existing','2026-08-01','2026-08-01');
             INSERT INTO salary_withdrawals(id,worker_id,amount_milli,withdrawn_at,notes,created_by,created_at,updated_at)
             VALUES('salary-withdrawal-existing','worker-existing',100000,'2026-08-15T12:00:00Z','مسحوب محفوظ','user-existing','2026-08-15','2026-08-15');
             INSERT INTO salary_deductions(id,worker_id,amount_milli,deduction_month,notes,created_by,created_at,deducted_at,updated_at)
             VALUES('salary-deduction-existing','worker-existing',50000,'2026-08','خصم محفوظ','user-existing','2026-08-16','2026-08-16T12:00:00Z','2026-08-16');",
        ).unwrap();
        drop(connection);

        let database = Database::open(&data_dir).unwrap();
        let worker: (String, String) = database
            .conn
            .query_row(
                "SELECT id,full_name FROM workers WHERE id='worker-existing'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(worker, ("worker-existing".to_owned(), "عامل محفوظ".to_owned()));
        let has_worker_id: bool = {
            let mut statement = database.conn.prepare("PRAGMA table_info(users)").unwrap();
            let columns = statement
                .query_map([], |row| row.get::<_, String>(1))
                .unwrap();
            columns
                .collect::<Result<Vec<_>>>()
                .unwrap()
                .iter()
                .any(|column| column == "worker_id")
        };
        assert!(!has_worker_id);
        let preserved_movement: (String, i64, Option<String>) = database
            .conn
            .query_row(
                "SELECT transaction_type,amount_milli,deleted_at FROM worker_withdrawal_returns WHERE id='movement-existing'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(preserved_movement, ("withdrawal".to_owned(), 500_000, None));
        database.conn.execute(
            "INSERT INTO worker_withdrawal_returns(id,worker_id,transaction_type,amount_milli,occurred_at,notes,created_by,created_at)
             VALUES('movement-settlement','worker-existing','settlement',500000,'2026-08-11T12:00:00Z','تصفية','user-existing','2026-08-11T12:00:00Z')",
            [],
        ).unwrap();
        database.conn.execute(
            "INSERT INTO worker_withdrawal_returns(id,worker_id,transaction_type,amount_milli,occurred_at,notes,created_by,created_at)
             VALUES('movement-deduction-payment','worker-existing','deduction_payment',100000,'2026-08-12T12:00:00Z','تسديد','user-existing','2026-08-12T12:00:00Z')",
            [],
        ).unwrap();
        let migration_16: i64 = database.conn.query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version=16",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(migration_16, 1);
        let migrated_employee: (String, String, i64) = database.conn.query_row(
            "SELECT id,full_name,is_active FROM payroll_employees WHERE id='payroll-worker-existing'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).unwrap();
        assert_eq!(migrated_employee, ("payroll-worker-existing".to_owned(), "عامل محفوظ".to_owned(), 1));
        let migrated_salary: i64 = database.conn.query_row(
            "SELECT salary_milli FROM payroll_salary_rates WHERE employee_id='payroll-worker-existing' AND effective_month='2026-08'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(migrated_salary, 1_000_000);
        let migrated_withdrawal: i64 = database.conn.query_row(
            "SELECT amount_milli FROM salary_withdrawals WHERE employee_id='payroll-worker-existing'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(migrated_withdrawal, 100_000);
        let migrated_deduction: i64 = database.conn.query_row(
            "SELECT amount_milli FROM salary_deductions WHERE employee_id='payroll-worker-existing'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(migrated_deduction, 50_000);
        let legacy_salary_table: i64 = database.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='worker_salary_rates'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(legacy_salary_table, 0);
        let migration_17: i64 = database.conn.query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version=17",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(migration_17, 1);
        let migration_18: i64 = database.conn.query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version=18",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(migration_18, 1);
        let migration_19: i64 = database.conn.query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version=19",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(migration_19, 1);
        let has_wash_type: bool = {
            let mut statement = database.conn.prepare("PRAGMA table_info(wash_operations)").unwrap();
            statement.query_map([], |row| row.get::<_, String>(1)).unwrap()
                .collect::<Result<Vec<_>, _>>().unwrap()
                .iter().any(|column| column == "wash_type")
        };
        assert!(has_wash_type);
        drop(database);
        fs::remove_dir_all(data_dir).unwrap();
    }
}
