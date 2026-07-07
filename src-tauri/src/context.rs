use std::sync::Arc;

use tokio::sync::mpsc;

use crate::db::Db;
use crate::inference::InferenceApi;

/// フロントへのイベント送出の抽象 (テストでは収集用実装に差し替える)
pub trait EventSink: Send + Sync {
    fn emit(&self, event: &str, payload: serde_json::Value);
}

pub struct NullSink;
impl EventSink for NullSink {
    fn emit(&self, _event: &str, _payload: serde_json::Value) {}
}

/// バックグラウンド処理のジョブ (ADR-06)
#[derive(Debug, Clone, PartialEq)]
pub enum Job {
    /// セッション確定後の後処理 (記憶抽出→埋め込み→人格評定)
    Postprocess(String),
    /// 埋め込み未計算の記憶の再計算 (起動時リカバリ)
    Reembed,
}

/// アプリ全体の共有コンテキスト
#[derive(Clone)]
pub struct AppCtx {
    pub db: Arc<Db>,
    pub inference: Arc<dyn InferenceApi>,
    pub sink: Arc<dyn EventSink>,
    pub conv: Arc<crate::conversation::ConversationManager>,
    pub worker_tx: mpsc::UnboundedSender<Job>,
}
