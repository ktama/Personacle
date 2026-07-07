use std::sync::RwLock;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

/// 推論エンジンとの通信エラー分類 (設計8章)
#[derive(Debug, thiserror::Error)]
pub enum InferError {
    #[error("推論エンジンに接続できません: {0}")]
    Connection(String),
    #[error("応答の生成に失敗しました: {0}")]
    Generation(String),
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatMessage {
    pub role: String, // "system" | "user" | "assistant"
    pub content: String,
}

impl ChatMessage {
    pub fn new(role: &str, content: impl Into<String>) -> Self {
        ChatMessage { role: role.into(), content: content.into() }
    }
}

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub temperature: f32,
    pub max_tokens: Option<u32>,
}

pub struct StreamOutcome {
    pub text: String,
    pub canceled: bool,
}

/// OpenAI 互換 API (ADR-02)。テスト用にモック実装と差し替えられる。
#[async_trait]
pub trait InferenceApi: Send + Sync {
    /// ストリーミング生成。delta ごとに on_delta を呼ぶ。キャンセル時は途中までの本文を返す (FR-07)
    async fn chat_stream(
        &self,
        req: ChatRequest,
        cancel: CancellationToken,
        on_delta: &mut (dyn FnMut(String) + Send),
    ) -> Result<StreamOutcome, InferError>;

    /// 非ストリーミング生成 (記憶抽出・人格評定などの JSON 出力用)
    async fn chat_once(&self, req: ChatRequest) -> Result<String, InferError>;

    async fn embed(&self, model: &str, texts: &[String]) -> Result<Vec<Vec<f32>>, InferError>;

    async fn list_models(&self) -> Result<Vec<String>, InferError>;
}

// ---------- SSE 解析 (純粋関数・単体テスト対象) ----------

#[derive(Debug, PartialEq)]
pub enum SseEvent {
    Delta(String),
    Done,
    Ignore,
}

pub fn parse_sse_line(line: &str) -> SseEvent {
    let l = line.trim();
    let Some(payload) = l.strip_prefix("data:") else { return SseEvent::Ignore };
    let payload = payload.trim();
    if payload == "[DONE]" {
        return SseEvent::Done;
    }
    match serde_json::from_str::<Value>(payload) {
        Ok(v) => {
            if let Some(s) = v["choices"][0]["delta"]["content"].as_str() {
                if !s.is_empty() {
                    return SseEvent::Delta(s.to_string());
                }
            }
            SseEvent::Ignore
        }
        Err(_) => SseEvent::Ignore,
    }
}

// ---------- HTTP 実装 ----------

pub struct HttpInference {
    client: reqwest::Client,
    endpoint: RwLock<String>,
}

impl HttpInference {
    pub fn new(endpoint: String) -> Self {
        // localhost 通信がシステムプロキシに吸われないよう no_proxy を明示
        let client = reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(5))
            .build()
            .expect("reqwest client");
        HttpInference { client, endpoint: RwLock::new(endpoint) }
    }

    pub fn set_endpoint(&self, endpoint: String) {
        *self.endpoint.write().expect("endpoint lock") = endpoint;
    }

    fn base(&self) -> String {
        self.endpoint.read().expect("endpoint lock").trim_end_matches('/').to_string()
    }

    fn map_send_err(e: reqwest::Error) -> InferError {
        if e.is_connect() || e.is_timeout() {
            InferError::Connection(e.to_string())
        } else {
            InferError::Generation(e.to_string())
        }
    }

    async fn check_status(resp: reqwest::Response) -> Result<reqwest::Response, InferError> {
        if resp.status().is_success() {
            return Ok(resp);
        }
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        let snippet: String = body.chars().take(300).collect();
        Err(InferError::Generation(format!("HTTP {status}: {snippet}")))
    }
}

#[async_trait]
impl InferenceApi for HttpInference {
    async fn chat_stream(
        &self,
        req: ChatRequest,
        cancel: CancellationToken,
        on_delta: &mut (dyn FnMut(String) + Send),
    ) -> Result<StreamOutcome, InferError> {
        let url = format!("{}/v1/chat/completions", self.base());
        let body = json!({
            "model": req.model,
            "messages": req.messages,
            "temperature": req.temperature,
            "max_tokens": req.max_tokens,
            "stream": true,
        });
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(Self::map_send_err)?;
        let resp = Self::check_status(resp).await?;

        let mut stream = resp.bytes_stream();
        let mut text = String::new();
        let mut buf = String::new();
        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    return Ok(StreamOutcome { text, canceled: true });
                }
                chunk = stream.next() => {
                    match chunk {
                        None => break,
                        Some(Err(e)) => {
                            // 途中までの本文があっても、通信断は生成失敗として扱う
                            return Err(InferError::Generation(format!("ストリーム中断: {e}")));
                        }
                        Some(Ok(bytes)) => {
                            buf.push_str(&String::from_utf8_lossy(&bytes));
                            while let Some(pos) = buf.find('\n') {
                                let line: String = buf.drain(..=pos).collect();
                                match parse_sse_line(&line) {
                                    SseEvent::Delta(d) => {
                                        text.push_str(&d);
                                        on_delta(d);
                                    }
                                    SseEvent::Done => return Ok(StreamOutcome { text, canceled: false }),
                                    SseEvent::Ignore => {}
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(StreamOutcome { text, canceled: false })
    }

    async fn chat_once(&self, req: ChatRequest) -> Result<String, InferError> {
        let url = format!("{}/v1/chat/completions", self.base());
        let body = json!({
            "model": req.model,
            "messages": req.messages,
            "temperature": req.temperature,
            "max_tokens": req.max_tokens,
            "stream": false,
        });
        let resp = self
            .client
            .post(&url)
            .timeout(Duration::from_secs(300))
            .json(&body)
            .send()
            .await
            .map_err(Self::map_send_err)?;
        let resp = Self::check_status(resp).await?;
        let v: Value = resp
            .json()
            .await
            .map_err(|e| InferError::Generation(format!("応答の解析に失敗: {e}")))?;
        v["choices"][0]["message"]["content"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| InferError::Generation("応答に content がありません".into()))
    }

    async fn embed(&self, model: &str, texts: &[String]) -> Result<Vec<Vec<f32>>, InferError> {
        let url = format!("{}/v1/embeddings", self.base());
        let resp = self
            .client
            .post(&url)
            .timeout(Duration::from_secs(120))
            .json(&json!({ "model": model, "input": texts }))
            .send()
            .await
            .map_err(Self::map_send_err)?;
        let resp = Self::check_status(resp).await?;
        let v: Value = resp
            .json()
            .await
            .map_err(|e| InferError::Generation(format!("埋め込み応答の解析に失敗: {e}")))?;
        let data = v["data"]
            .as_array()
            .ok_or_else(|| InferError::Generation("埋め込み応答に data がありません".into()))?;
        let mut out = Vec::with_capacity(data.len());
        for item in data {
            let emb = item["embedding"]
                .as_array()
                .ok_or_else(|| InferError::Generation("embedding がありません".into()))?
                .iter()
                .map(|x| x.as_f64().unwrap_or(0.0) as f32)
                .collect();
            out.push(emb);
        }
        Ok(out)
    }

    async fn list_models(&self) -> Result<Vec<String>, InferError> {
        let url = format!("{}/v1/models", self.base());
        let resp = self
            .client
            .get(&url)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(Self::map_send_err)?;
        let resp = Self::check_status(resp).await?;
        let v: Value = resp
            .json()
            .await
            .map_err(|e| InferError::Generation(format!("モデル一覧の解析に失敗: {e}")))?;
        Ok(v["data"]
            .as_array()
            .map(|arr| arr.iter().filter_map(|m| m["id"].as_str().map(String::from)).collect())
            .unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_delta_line() {
        let line = r#"data: {"choices":[{"delta":{"content":"こん"}}]}"#;
        assert_eq!(parse_sse_line(line), SseEvent::Delta("こん".into()));
    }

    #[test]
    fn sse_done_line() {
        assert_eq!(parse_sse_line("data: [DONE]"), SseEvent::Done);
    }

    #[test]
    fn sse_ignores_role_only_and_empty() {
        // 先頭チャンクは role のみで content がない
        let line = r#"data: {"choices":[{"delta":{"role":"assistant"}}]}"#;
        assert_eq!(parse_sse_line(line), SseEvent::Ignore);
        assert_eq!(parse_sse_line(""), SseEvent::Ignore);
        assert_eq!(parse_sse_line(": keep-alive"), SseEvent::Ignore);
        // 空文字 content も無視
        let line = r#"data: {"choices":[{"delta":{"content":""}}]}"#;
        assert_eq!(parse_sse_line(line), SseEvent::Ignore);
    }

    #[test]
    fn sse_ignores_broken_json() {
        assert_eq!(parse_sse_line("data: {broken"), SseEvent::Ignore);
    }
}
