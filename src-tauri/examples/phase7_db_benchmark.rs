use alkaheli_car_wash_erp_lib::db::{new_id, Database};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{
    env, fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

fn percentile(samples: &[Duration], percentile: f64) -> f64 {
    let mut values = samples.to_vec();
    values.sort_unstable();
    let index = ((values.len() as f64 * percentile).ceil() as usize)
        .saturating_sub(1)
        .min(values.len().saturating_sub(1));
    values[index].as_secs_f64() * 1000.0
}

fn summary(samples: &[Duration]) -> Value {
    json!({
        "count": samples.len(),
        "median_ms": percentile(samples, 0.5),
        "p95_ms": percentile(samples, 0.95),
        "worst_ms": samples.iter().max().map_or(0.0, |value| value.as_secs_f64() * 1000.0),
    })
}

fn query_probe(database: &Database) -> rusqlite::Result<i64> {
    database.conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM wash_operations WHERE id='wash-000000001')",
        [],
        |row| row.get(0),
    )
}

fn copy_database(source: &Path, destination_dir: &Path) -> std::io::Result<PathBuf> {
    fs::create_dir_all(destination_dir)?;
    let destination = destination_dir.join("carwash.db");
    fs::copy(source, &destination)?;
    Ok(destination)
}

fn explain(conn: &Connection, sql: &str) -> rusqlite::Result<Vec<String>> {
    let mut statement = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?;
    let result = statement
        .query_map([], |row| row.get::<_, String>(3))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(result)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let source = env::args()
        .nth(1)
        .ok_or("pass a template carwash.db path")?;
    let output = env::args().nth(2);
    let work_dir = env::temp_dir().join(format!("alkaheli-phase7-db-benchmark-{}", new_id()));
    let db_path = copy_database(Path::new(&source), &work_dir)?;

    let startup_started = Instant::now();
    let mut writer = Database::open(&work_dir)?;
    let current_startup = startup_started.elapsed();
    let row_count: i64 =
        writer
            .conn
            .query_row("SELECT COUNT(*) FROM wash_operations", [], |row| row.get(0))?;
    let connection_pragmas = json!({
        "journal_mode": writer.conn.query_row("PRAGMA journal_mode", [], |row| row.get::<_,String>(0))?,
        "synchronous": writer.conn.query_row("PRAGMA synchronous", [], |row| row.get::<_,i64>(0))?,
        "foreign_keys": writer.conn.query_row("PRAGMA foreign_keys", [], |row| row.get::<_,i64>(0))?,
        "busy_timeout_ms": writer.conn.query_row("PRAGMA busy_timeout", [], |row| row.get::<_,i64>(0))?,
        "wal_autocheckpoint_pages": writer.conn.query_row("PRAGMA wal_autocheckpoint", [], |row| row.get::<_,i64>(0))?,
        "auto_vacuum": writer.conn.query_row("PRAGMA auto_vacuum", [], |row| row.get::<_,i64>(0))?,
    });

    let iterations = 500;
    let mut opened = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let started = Instant::now();
        let database = Database::open_read_only(&db_path)?;
        assert_eq!(query_probe(&database)?, 1);
        opened.push(started.elapsed());
    }
    let reused = Database::open_read_only(&db_path)?;
    let mut reused_samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let started = Instant::now();
        assert_eq!(query_probe(&reused)?, 1);
        reused_samples.push(started.elapsed());
    }
    drop(reused);

    let mut double_open = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let started = Instant::now();
        assert_eq!(query_probe(&Database::open_read_only(&db_path)?)?, 1);
        assert_eq!(query_probe(&Database::open_read_only(&db_path)?)?, 1);
        double_open.push(started.elapsed());
    }

    let index_sizes = {
        let mut statement = writer.conn.prepare(
            "SELECT name,SUM(pgsize) FROM dbstat
             WHERE name IN (SELECT name FROM sqlite_master WHERE type='index')
             GROUP BY name ORDER BY SUM(pgsize) DESC",
        )?;
        let result = statement
            .query_map([], |row| {
                Ok(json!({"name":row.get::<_,String>(0)?,"bytes":row.get::<_,i64>(1)?}))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        result
    };

    let query_plans = json!({
        "history": explain(&writer.conn, "SELECT id,occurred_at,status FROM wash_operations WHERE occurred_at BETWEEN '2025-01-01' AND '2027-01-01' ORDER BY occurred_at DESC,id DESC LIMIT 151")?,
        "worker_history": explain(&writer.conn, "SELECT id,occurred_at FROM wash_operations WHERE worker_id='worker-000001' AND status='posted' AND occurred_at BETWEEN '2025-01-01' AND '2027-01-01' ORDER BY occurred_at DESC,id DESC LIMIT 151")?,
        "showroom_summary": explain(&writer.conn, "SELECT SUM(price_milli) FROM wash_operations WHERE showroom_id='showroom-000001' AND status='posted' AND payment_type='showroom' AND occurred_at BETWEEN '2025-01-01' AND '2027-01-01'")?,
        "auth": explain(&writer.conn, "SELECT user_id FROM sessions WHERE token_hash='missing' AND revoked_at IS NULL")?,
    });

    writer.conn.execute_batch(
        "CREATE TABLE phase7_wal_probe(id INTEGER PRIMARY KEY,payload TEXT NOT NULL);",
    )?;
    let long_reader = Database::open_read_only(&db_path)?;
    let _: i64 =
        long_reader
            .conn
            .query_row("SELECT COUNT(*) FROM phase7_wal_probe", [], |row| {
                row.get(0)
            })?;
    let write_started = Instant::now();
    {
        let tx = writer.conn.transaction()?;
        for id in 0..2_000_i64 {
            tx.execute(
                "INSERT INTO phase7_wal_probe(id,payload) VALUES(?1,?2)",
                params![id, "phase7 checkpoint payload"],
            )?;
        }
        tx.commit()?;
    }
    let write_duration = write_started.elapsed();
    let wal_path = db_path.with_extension("db-wal");
    let wal_bytes_with_reader = fs::metadata(&wal_path).map_or(0, |metadata| metadata.len());
    let checkpoint_started = Instant::now();
    let passive_with_reader: (i64, i64, i64) =
        writer
            .conn
            .query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?;
    let checkpoint_with_reader = checkpoint_started.elapsed();
    drop(long_reader);
    let truncate_started = Instant::now();
    let truncate_after_reader: (i64, i64, i64) =
        writer
            .conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?;
    let truncate_duration = truncate_started.elapsed();
    let wal_bytes_after = fs::metadata(&wal_path).map_or(0, |metadata| metadata.len());

    let integrity: String = writer
        .conn
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    let foreign_key_issue: Option<String> = writer
        .conn
        .query_row(
            "SELECT printf('%s:%s',\"table\",rowid) FROM pragma_foreign_key_check LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let database_bytes = fs::metadata(&db_path)?.len();
    drop(writer);

    let result = json!({
        "row_count": row_count,
        "database_bytes": database_bytes,
        "current_database_open_ms": current_startup.as_secs_f64() * 1000.0,
        "connection_pragmas": connection_pragmas,
        "connection_strategy": {
            "open_query_close": summary(&opened),
            "reused_connection_query": summary(&reused_samples),
            "two_open_query_close_cycles": summary(&double_open),
        },
        "wal": {
            "write_2000_rows_ms": write_duration.as_secs_f64() * 1000.0,
            "bytes_with_reader": wal_bytes_with_reader,
            "passive_checkpoint_with_reader": passive_with_reader,
            "passive_checkpoint_ms": checkpoint_with_reader.as_secs_f64() * 1000.0,
            "truncate_after_reader": truncate_after_reader,
            "truncate_ms": truncate_duration.as_secs_f64() * 1000.0,
            "bytes_after_truncate": wal_bytes_after,
        },
        "index_sizes": index_sizes,
        "query_plans": query_plans,
        "integrity_check": integrity,
        "foreign_key_issue": foreign_key_issue,
    });
    let rendered = serde_json::to_string_pretty(&result)?;
    println!("{rendered}");
    if let Some(output) = output {
        fs::write(output, &rendered)?;
    }
    fs::remove_dir_all(work_dir)?;
    Ok(())
}
