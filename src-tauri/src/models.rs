use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Persona {
    pub id: String,
    pub name: String,
    pub description: String,
    pub speech_style: String,
    pub values_text: String,
    pub self_intro: String,
    pub created_at: i64,
    pub last_talked_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraitValue {
    pub key: String,
    pub value: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Relationship {
    pub persona_id: String,
    pub target_kind: String, // "user" | "persona"
    pub target_id: String,
    pub target_name: String,
    pub intimacy: i64,
    pub impression_text: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonaDetail {
    pub persona: Persona,
    pub traits: Vec<TraitValue>,
    pub relationships: Vec<Relationship>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub kind: String, // "user_dialogue" | "autonomous"
    pub theme: String,
    pub status: String, // "active" | "ended" | "processed"
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub participant_ids: Vec<String>,
    pub participant_names: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Utterance {
    pub id: String,
    pub session_id: String,
    pub speaker_kind: String, // "user" | "persona"
    pub speaker_id: String,
    pub speaker_name: String,
    pub content: String,
    pub state: String, // "complete" | "canceled"
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Memory {
    pub id: String,
    pub persona_id: String,
    pub content: String,
    pub kind: String, // fact | event | promise | impression
    pub importance: i64,
    pub has_embedding: bool,
    pub source_session_id: Option<String>,
    pub created_at: i64,
    pub archived: bool,
    pub user_edited: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonalityEvent {
    pub id: String,
    pub persona_id: String,
    pub session_id: Option<String>,
    pub item: String,
    pub old_value: String,
    pub new_value: String,
    pub created_at: i64,
}

/// アプリ設定 (設計5.2 app_setting)。key-value を型付きで扱う。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub endpoint: String,
    pub chat_model: String,
    pub embed_model: String,
    pub auto_turn_limit: i64,
    pub input_max_chars: i64,
    pub recall_k: i64,
    pub w_sim: f64,
    pub w_rec: f64,
    pub w_imp: f64,
    pub trait_delta_cap: i64,
    pub intimacy_delta_cap: i64,
    pub memory_cap: i64,
    pub context_chars: i64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            endpoint: "http://127.0.0.1:11434".into(),
            chat_model: String::new(),
            embed_model: String::new(),
            auto_turn_limit: 12,
            input_max_chars: 4000,
            recall_k: 8,
            w_sim: 1.0,
            w_rec: 0.5,
            w_imp: 0.5,
            trait_delta_cap: 2,
            intimacy_delta_cap: 5,
            memory_cap: 10000,
            context_chars: 8000,
        }
    }
}

/// 自律会話のターン数上限(設定値の上限。設計 FR-14)
pub const AUTO_TURN_HARD_MAX: i64 = 50;

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
