//! テスト専用のモック実装 (#[cfg(test)] で lib.rs から取り込む)

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::context::{AppCtx, EventSink, Job};
use crate::conversation::ConversationManager;
use crate::db::Db;
use crate::inference::{ChatRequest, InferenceApi, InferError, StreamOutcome};
use crate::models::{new_id, now_ms, Persona, TraitValue};

/// 応答をキューから返すモック推論エンジン
pub struct MockInference {
    pub replies: Mutex<VecDeque<String>>,
    /// chat_stream のチャンク間ディレイ (キャンセルのテスト用)
    pub chunk_delay_ms: u64,
    pub embed_ok: bool,
    pub embed_value: Vec<f32>,
}

impl Default for MockInference {
    fn default() -> Self {
        MockInference {
            replies: Mutex::new(VecDeque::new()),
            chunk_delay_ms: 0,
            embed_ok: true,
            embed_value: vec![1.0, 0.0],
        }
    }
}

impl MockInference {
    pub fn with_replies(replies: &[&str]) -> Self {
        MockInference {
            replies: Mutex::new(replies.iter().map(|s| s.to_string()).collect()),
            ..Default::default()
        }
    }

    fn pop(&self) -> Result<String, InferError> {
        self.replies
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| InferError::Generation("モックの応答が尽きました".into()))
    }
}

#[async_trait]
impl InferenceApi for MockInference {
    async fn chat_stream(
        &self,
        _req: ChatRequest,
        cancel: CancellationToken,
        on_delta: &mut (dyn FnMut(String) + Send),
    ) -> Result<StreamOutcome, InferError> {
        let reply = self.pop()?;
        let mut text = String::new();
        // 3文字ずつのチャンクでストリーミングを模す
        let chars: Vec<char> = reply.chars().collect();
        for chunk in chars.chunks(3) {
            if self.chunk_delay_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(self.chunk_delay_ms)).await;
            }
            if cancel.is_cancelled() {
                return Ok(StreamOutcome { text, canceled: true });
            }
            let piece: String = chunk.iter().collect();
            text.push_str(&piece);
            on_delta(piece);
        }
        Ok(StreamOutcome { text, canceled: false })
    }

    async fn chat_once(&self, _req: ChatRequest) -> Result<String, InferError> {
        self.pop()
    }

    async fn embed(&self, _model: &str, texts: &[String]) -> Result<Vec<Vec<f32>>, InferError> {
        if !self.embed_ok {
            return Err(InferError::Connection("embed失敗(モック)".into()));
        }
        Ok(texts.iter().map(|_| self.embed_value.clone()).collect())
    }

    async fn list_models(&self) -> Result<Vec<String>, InferError> {
        Ok(vec!["mock-model".into()])
    }
}

/// 発行イベントを収集するシンク
#[derive(Default)]
pub struct CollectSink {
    pub events: Mutex<Vec<(String, serde_json::Value)>>,
}

impl EventSink for CollectSink {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        self.events.lock().unwrap().push((event.to_string(), payload));
    }
}

impl CollectSink {
    pub fn names(&self) -> Vec<String> {
        self.events.lock().unwrap().iter().map(|(n, _)| n.clone()).collect()
    }
}

pub struct TestEnv {
    pub _dir: tempfile::TempDir,
    pub ctx: AppCtx,
    pub sink: Arc<CollectSink>,
    pub mock: Arc<MockInference>,
    pub job_rx: tokio::sync::mpsc::UnboundedReceiver<Job>,
}

pub fn test_ctx(mock: MockInference) -> TestEnv {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Db::open(&dir.path().join("t.db")).unwrap());
    let sink = Arc::new(CollectSink::default());
    let mock = Arc::new(mock);
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let ctx = AppCtx {
        db,
        inference: mock.clone(),
        sink: sink.clone(),
        conv: Arc::new(ConversationManager::default()),
        worker_tx: tx,
    };
    TestEnv { _dir: dir, ctx, sink, mock, job_rx: rx }
}

pub fn add_persona(ctx: &AppCtx, name: &str) -> Persona {
    let p = Persona {
        id: new_id(),
        name: name.into(),
        description: "テスト用".into(),
        speech_style: "普通".into(),
        values_text: String::new(),
        self_intro: String::new(),
        created_at: now_ms(),
        last_talked_at: None,
    };
    ctx.db
        .create_persona(&p, &[TraitValue { key: "sociability".into(), value: 50 }])
        .unwrap();
    p
}
