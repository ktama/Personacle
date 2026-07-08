pub mod commands;
pub mod context;
pub mod conversation;
pub mod db;
pub mod error;
pub mod export;
pub mod inference;
pub mod memory;
pub mod models;
pub mod personality;
pub mod prompt;
pub mod worker;

#[cfg(test)]
pub mod test_util;

#[cfg(test)]
mod integration_test;

#[cfg(test)]
mod poc_test;

use std::sync::Arc;

use tauri::{Emitter, Manager};

use crate::commands::AppState;
use crate::context::{AppCtx, EventSink};
use crate::conversation::ConversationManager;
use crate::db::Db;
use crate::inference::HttpInference;

/// Tauri イベントとしてフロントへ送出する (設計6.2)
struct TauriSink(tauri::AppHandle);

impl EventSink for TauriSink {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        if let Err(e) = self.0.emit(event, payload) {
            tracing::warn!("イベント送出に失敗 ({event}): {e}");
        }
    }
}

/// 14日より古いログファイルを削除する (NFR-06)
fn cleanup_old_logs(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(14 * 86400);
    for entry in entries.flatten() {
        if let Ok(meta) = entry.metadata() {
            if let Ok(modified) = meta.modified() {
                if modified < cutoff {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;

            // ログ: 日次ローテーション・14日保持・会話本文は記録しない (NFR-06)
            let log_dir = data_dir.join("logs");
            std::fs::create_dir_all(&log_dir)?;
            cleanup_old_logs(&log_dir);
            let file_appender = tracing_appender::rolling::daily(&log_dir, "personacle.log");
            let (writer, guard) = tracing_appender::non_blocking(file_appender);
            // guard はプロセス終了まで保持する必要がある
            Box::leak(Box::new(guard));
            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| "info".into()),
                )
                .with_writer(writer)
                .with_ansi(false)
                .init();
            tracing::info!("Personacle 起動");

            let db = Arc::new(Db::open(&data_dir.join("personacle.db")).map_err(|e| e.to_string())?);
            let settings = db.load_settings().map_err(|e| e.to_string())?;
            let http = Arc::new(HttpInference::new(settings.endpoint.clone()));
            let (worker_tx, worker_rx) = tokio::sync::mpsc::unbounded_channel();

            let ctx = AppCtx {
                db,
                inference: http.clone(),
                sink: Arc::new(TauriSink(app.handle().clone())),
                conv: Arc::new(ConversationManager::default()),
                worker_tx,
            };

            // バックグラウンドワーカーと起動時リカバリ (ADR-06, EC-03)
            tauri::async_runtime::spawn(worker::run_worker(ctx.clone(), worker_rx));
            if let Err(e) = worker::startup_recovery(&ctx) {
                tracing::error!("起動時リカバリに失敗: {e}");
            }

            app.manage(AppState { ctx, http });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_personas,
            commands::get_persona,
            commands::create_persona,
            commands::update_persona,
            commands::delete_persona,
            commands::suggest_traits,
            commands::start_session,
            commands::send_message,
            commands::cancel_generation,
            commands::end_session,
            commands::start_autonomous_turns,
            commands::stop_session,
            commands::list_sessions,
            commands::get_session_utterances,
            commands::list_memories,
            commands::update_memory,
            commands::delete_memory,
            commands::get_personality_history,
            commands::export_persona,
            commands::import_persona,
            commands::get_settings,
            commands::update_settings,
            commands::test_connection,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
