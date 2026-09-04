pub mod api;
pub mod db;

use api::{build_router, AppState};
use db::{now, Database};
use std::sync::{Arc, Mutex};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};
use tauri::Manager;

const LOCAL_API_ADDRESS: &str = "127.0.0.1:8787";

fn log_app_event(data_dir: &Path, level: &str, message: &str) {
    let log_dir = data_dir.join("logs");
    if fs::create_dir_all(&log_dir).is_err() {
        return;
    }
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join("app.log"))
    {
        let _ = writeln!(file, "{} [{}] {message}", now(), level);
    }
}

pub fn create_state(data_dir: PathBuf) -> Result<AppState, String> {
    let database =
        Database::open(&data_dir).map_err(|error| format!("تعذر فتح قاعدة البيانات: {error}"))?;
    Ok(AppState {
        db: Arc::new(Mutex::new(database)),
        data_dir,
    })
}

pub async fn bind_local_api() -> Result<tokio::net::TcpListener, String> {
    tokio::net::TcpListener::bind(LOCAL_API_ADDRESS)
        .await
        .map_err(|error| format!("تعذر تشغيل الخدمة المحلية على {LOCAL_API_ADDRESS}: {error}"))
}

pub async fn serve_on_listener(
    state: AppState,
    listener: tokio::net::TcpListener,
) -> Result<(), String> {
    axum::serve(listener, build_router(state))
        .await
        .map_err(|error| format!("توقفت الخدمة المحلية: {error}"))
}

pub async fn serve(state: AppState) -> Result<(), String> {
    let listener = bind_local_api().await?;
    serve_on_listener(state, listener).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app
                .path()
                .app_local_data_dir()
                .map_err(|error| format!("تعذر تحديد مجلد بيانات التطبيق: {error}"))?
                .join("AlkaheliCarWashERP");
            let state = create_state(data_dir)?;
            let log_data_dir = state.data_dir.clone();
            log_app_event(
                &log_data_dir,
                "INFO",
                "بدء تشغيل خدمة التطبيق المحلية المدمجة",
            );
            let handle = tauri::async_runtime::handle();
            match tauri::async_runtime::block_on(bind_local_api()) {
                Ok(listener) => {
                    // The port is reserved before setup returns, so the packaged UI
                    // cannot race the embedded API during application startup.
                    handle.spawn(async move {
                        if let Err(error) = serve_on_listener(state, listener).await {
                            log_app_event(&log_data_dir, "ERROR", &error);
                            eprintln!("{error}");
                        }
                    });
                }
                Err(error) if cfg!(debug_assertions) => {
                    // `tauri dev` starts the shared development API in beforeDevCommand.
                    log_app_event(&log_data_dir, "INFO", &error);
                    eprintln!("{error}; سيتم استخدام خدمة التطوير الحالية");
                }
                Err(error) => {
                    log_app_event(&log_data_dir, "ERROR", &error);
                    return Err(error.into());
                }
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("تعذر تشغيل مركز الكحيلي لغسيل السيارات");
}
