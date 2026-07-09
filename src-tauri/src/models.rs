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

/// ムード変化の履歴 (v0.2, FR-25, ADR-13)。追記のみ。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoodEvent {
    pub id: String,
    pub persona_id: String,
    pub session_id: Option<String>,
    pub old_value: i64,
    pub new_value: i64,
    pub label: String,
    pub created_at: i64,
}

/// 減衰計算済みの現在ムード (v0.2, FR-25)。get_mood の戻り値。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoodState {
    pub value: i64,
    pub label: String,
    pub rated_at: Option<i64>,
    pub recent_event: Option<MoodEvent>,
}

/// 日記 (v0.2, FR-26/27)。(persona_id, date) で一意。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diary {
    pub id: String,
    pub persona_id: String,
    pub date: String, // "YYYY-MM-DD" セッション開始日(ローカル, EC-17)
    pub content: String,
    pub updated_at: i64,
}

/// 成長ダッシュボードの時系列の1点 (v0.2, FR-29)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeriesPoint {
    pub t: i64,
    pub value: i64,
}

/// 名前付き時系列 (性格軸ごと等, v0.2, FR-29)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Series {
    pub key: String,
    pub points: Vec<SeriesPoint>,
}

/// ペルソナ関係図のノード (v0.2, FR-30)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphNode {
    pub id: String,
    pub name: String,
    pub kind: String, // "user" | "persona"
}

/// 関係図の辺 (from が to への親密度を持つ, v0.2, FR-30)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
    pub intimacy: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelationshipGraph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
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
    // --- v0.2 (要件9-8〜9-12 の提案初期値。設計 app_setting) ---
    pub greeting_enabled: bool,       // 話しかけ有効 (FR-21)
    pub greeting_interval_min: i64,   // 話しかけ再生成間隔(分, EC-14)
    pub elapsed_short_hours: i64,     // 経過時間ラベル閾値(短, ADR-11)
    pub elapsed_mid_hours: i64,       // 同(中)
    pub elapsed_long_days: i64,       // 同(長)
    pub consolidate_sim: f64,         // 統合の類似度閾値 (ADR-12)
    pub consolidate_min_cluster: i64, // 統合の最小クラスタ件数
    pub mood_halflife_hours: i64,     // ムード半減期(時, ADR-13)
    pub mood_delta_cap: i64,          // 1セッションのムード変化量上限
    pub chain_limit: i64,             // グループ連鎖発話上限 (FR-33)
    pub group_max: i64,               // グループ参加上限 (ADR-15)
    pub stagnation_sim: f64,          // 停滞判定の類似度閾値 (ADR-16)
    pub stagnation_streak: i64,       // 停滞判定の連続ターン数
    pub topic_shift_limit: i64,       // 1セッションの話題転換上限
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
            greeting_enabled: true,
            greeting_interval_min: 60,
            elapsed_short_hours: 6,
            elapsed_mid_hours: 48,
            elapsed_long_days: 14,
            consolidate_sim: 0.80,
            consolidate_min_cluster: 5,
            mood_halflife_hours: 24,
            mood_delta_cap: 50,
            chain_limit: 2,
            group_max: 6,
            stagnation_sim: 0.85,
            stagnation_streak: 2,
            topic_shift_limit: 2,
        }
    }
}

/// 自律会話のターン数上限(設定値の上限。設計 FR-14)
pub const AUTO_TURN_HARD_MAX: i64 = 50;

/// 自律会話の参加ペルソナ数の上限 (FR-19, ADR-08)
pub const MAX_AUTONOMOUS_PARTICIPANTS: usize = 6;

/// グループチャットの参加ペルソナ数の上限 (FR-31, ADR-15)。連鎖テンポの実測(R-6)まで自律会話と同値。
pub const MAX_GROUP_PARTICIPANTS: usize = 6;

/// ムードの値域と平常判定バンド (ADR-13)。|value| < BAND なら「平常」ラベル。
pub const MOOD_MIN: i64 = -100;
pub const MOOD_MAX: i64 = 100;
pub const MOOD_NEUTRAL_BAND: i64 = 10;

/// ローカル日付 "YYYY-MM-DD" を壁時計から得る (日記の帰属日, EC-17)
pub fn local_date_of(ts_ms: i64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_millis_opt(ts_ms).single() {
        Some(dt) => dt.format("%Y-%m-%d").to_string(),
        None => String::new(),
    }
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
