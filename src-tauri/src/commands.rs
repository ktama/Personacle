use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::context::{AppCtx, Job};
use crate::conversation;
use crate::error::{AppError, AppResult};
use crate::inference::{ChatMessage, ChatRequest, HttpInference};
use crate::models::*;
use crate::personality::{DEFAULT_TRAIT_VALUE, TRAIT_KEYS};
use crate::worker::extract_json;

pub struct AppState {
    pub ctx: AppCtx,
    pub http: Arc<HttpInference>,
}

// ---------- 入力検証 (Command Facade, EC-05/09/10) ----------

/// 制御文字を除去する (改行・タブは保持)。EC-10
pub fn sanitize(input: &str) -> String {
    input
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect()
}

/// メッセージ検証: 空 (EC-09)・上限超過 (EC-05)
pub fn validate_message(text: &str, max_chars: i64) -> AppResult<String> {
    let cleaned = sanitize(text);
    if cleaned.trim().is_empty() {
        return Err(AppError::Validation("メッセージが空です".into()));
    }
    let count = cleaned.chars().count() as i64;
    if count > max_chars {
        return Err(AppError::Validation(format!(
            "メッセージが長すぎます ({count}文字)。上限は{max_chars}文字です"
        )));
    }
    Ok(cleaned)
}

pub fn validate_name(name: &str) -> AppResult<String> {
    let cleaned = sanitize(name).trim().to_string();
    if cleaned.is_empty() {
        return Err(AppError::Validation("名前は必須です".into()));
    }
    if cleaned.chars().count() > 50 {
        return Err(AppError::Validation("名前は50文字以内にしてください".into()));
    }
    Ok(cleaned)
}

fn clamp_field(s: &str, max: usize) -> String {
    sanitize(s).chars().take(max).collect()
}

// ---------- ペルソナ管理 (FR-01〜04) ----------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonaInput {
    pub name: String,
    pub description: String,
    pub speech_style: String,
    pub values_text: String,
    pub self_intro: String,
    #[serde(default)]
    pub traits: Vec<TraitValue>,
    #[serde(default)]
    pub force: bool,
}

pub fn do_create_persona(ctx: &AppCtx, input: PersonaInput) -> AppResult<Persona> {
    let name = validate_name(&input.name)?;
    // EC-04: 同名は警告し、force で作成を許可する
    if !input.force && ctx.db.persona_name_exists(&name, None)? {
        return Err(AppError::DuplicateName(format!("「{name}」という名前のペルソナは既に存在します")));
    }
    let p = Persona {
        id: new_id(),
        name,
        description: clamp_field(&input.description, 2000),
        speech_style: clamp_field(&input.speech_style, 1000),
        values_text: clamp_field(&input.values_text, 1000),
        self_intro: clamp_field(&input.self_intro, 1000),
        created_at: now_ms(),
        last_talked_at: None,
    };
    let traits: Vec<TraitValue> = TRAIT_KEYS
        .iter()
        .map(|key| {
            let v = input
                .traits
                .iter()
                .find(|t| t.key == *key)
                .map(|t| t.value.clamp(0, 100))
                .unwrap_or(DEFAULT_TRAIT_VALUE);
            TraitValue { key: key.to_string(), value: v }
        })
        .collect();
    ctx.db.create_persona(&p, &traits)?;
    Ok(p)
}

pub fn do_update_persona(ctx: &AppCtx, id: &str, input: PersonaInput) -> AppResult<()> {
    let existing = ctx
        .db
        .get_persona(id)?
        .ok_or_else(|| AppError::NotFound("ペルソナが見つかりません".into()))?;
    let name = validate_name(&input.name)?;
    if !input.force && name != existing.name && ctx.db.persona_name_exists(&name, Some(id))? {
        return Err(AppError::DuplicateName(format!("「{name}」という名前のペルソナは既に存在します")));
    }
    // FR-03: 初期設定のみ更新。記憶・成長分は別テーブルのため保持される
    ctx.db.update_persona(&Persona {
        id: id.to_string(),
        name,
        description: clamp_field(&input.description, 2000),
        speech_style: clamp_field(&input.speech_style, 1000),
        values_text: clamp_field(&input.values_text, 1000),
        self_intro: clamp_field(&input.self_intro, 1000),
        created_at: existing.created_at,
        last_talked_at: existing.last_talked_at,
    })
}

#[tauri::command]
pub fn list_personas(state: State<AppState>) -> AppResult<Vec<Persona>> {
    state.ctx.db.list_personas()
}

#[tauri::command]
pub fn get_persona(state: State<AppState>, id: String) -> AppResult<PersonaDetail> {
    let persona = state
        .ctx
        .db
        .get_persona(&id)?
        .ok_or_else(|| AppError::NotFound("ペルソナが見つかりません".into()))?;
    Ok(PersonaDetail {
        traits: state.ctx.db.traits_of(&id)?,
        relationships: state.ctx.db.relationships_of(&id)?,
        persona,
    })
}

#[tauri::command]
pub fn create_persona(state: State<AppState>, input: PersonaInput) -> AppResult<Persona> {
    do_create_persona(&state.ctx, input)
}

#[tauri::command]
pub fn update_persona(state: State<AppState>, id: String, input: PersonaInput) -> AppResult<()> {
    do_update_persona(&state.ctx, &id, input)
}

#[tauri::command]
pub fn delete_persona(state: State<AppState>, id: String) -> AppResult<()> {
    // フロント側で確認ダイアログ済み (FR-04)。参加中セッションがあれば拒否
    let sessions = state.ctx.db.list_sessions_for_persona(&id)?;
    if sessions.iter().any(|s| s.status == "active") {
        return Err(AppError::Busy("会話に参加中のペルソナは削除できません。先に会話を終了してください".into()));
    }
    state.ctx.db.delete_persona(&id)
}

/// 初期設定文から性格軸の初期値を LLM に提案させる (設計5.2)
#[tauri::command]
pub async fn suggest_traits(state: State<'_, AppState>, description: String) -> AppResult<Vec<TraitValue>> {
    let settings = state.ctx.db.load_settings()?;
    if settings.chat_model.is_empty() {
        return Err(AppError::Validation("チャットモデルが未設定です".into()));
    }
    let system = format!(
        "次の人物設定を読み、性格軸を0〜100で評定してJSONのみを出力する。\n\
         形式: {{\"sociability\": 整数, \"empathy\": 整数, \"caution\": 整数, \"assertiveness\": 整数, \"cheerfulness\": 整数}}"
    );
    let req = ChatRequest {
        model: settings.chat_model,
        messages: vec![
            ChatMessage::new("system", system),
            ChatMessage::new("user", clamp_field(&description, 2000)),
        ],
        temperature: 0.2,
        // thinking対応モデルの思考分の余裕を持たせる
        max_tokens: Some(1024),
    };
    let text = state.ctx.inference.chat_once(req).await.map_err(|e| AppError::Generation(e.to_string()))?;
    let v = extract_json(&text)
        .ok_or_else(|| AppError::Generation("性格評定の解析に失敗しました".into()))?;
    Ok(TRAIT_KEYS
        .iter()
        .map(|key| TraitValue {
            key: key.to_string(),
            value: v[key].as_i64().unwrap_or(DEFAULT_TRAIT_VALUE).clamp(0, 100),
        })
        .collect())
}

// ---------- セッション・対話 (FR-05〜07, FR-14) ----------

#[tauri::command]
pub fn start_session(
    state: State<AppState>,
    kind: String,
    persona_ids: Vec<String>,
    theme: Option<String>,
) -> AppResult<Session> {
    let theme = clamp_field(&theme.unwrap_or_default(), 500);
    conversation::start_session(&state.ctx, &kind, &persona_ids, &theme)
}

#[tauri::command]
pub fn send_message(state: State<AppState>, session_id: String, text: String) -> AppResult<Utterance> {
    let settings = state.ctx.db.load_settings()?;
    let cleaned = validate_message(&text, settings.input_max_chars)?;
    // 応答生成は非同期に行い、結果はイベントで届く (設計6.1)
    let ctx = state.ctx.clone();
    let sid = session_id.clone();
    let preview = Utterance {
        id: new_id(),
        session_id: session_id.clone(),
        speaker_kind: "user".into(),
        speaker_id: "user".into(),
        speaker_name: "ユーザー".into(),
        content: cleaned.clone(),
        state: "complete".into(),
        created_at: now_ms(),
    };
    tauri::async_runtime::spawn(async move {
        if let Err(e) = conversation::send_user_message(&ctx, &sid, &cleaned).await {
            // 推論エラーは send_user_message 内で generation_failed を発行済み。
            // それ以外 (検証・データ) もここで通知する
            if !matches!(e, AppError::Connection(_) | AppError::Generation(_)) {
                ctx.sink.emit(
                    "generation_failed",
                    serde_json::json!({ "sessionId": sid, "kind": e.kind(), "message": e.to_string() }),
                );
            }
        }
    });
    Ok(preview)
}

#[tauri::command]
pub fn cancel_generation(state: State<AppState>, session_id: String) -> AppResult<()> {
    state.ctx.conv.cancel_generation(&session_id);
    Ok(())
}

#[tauri::command]
pub fn end_session(state: State<AppState>, session_id: String) -> AppResult<()> {
    conversation::end_session(&state.ctx, &session_id)
}

#[tauri::command]
pub fn start_autonomous_turns(state: State<AppState>, session_id: String) -> AppResult<()> {
    let ctx = state.ctx.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = conversation::run_autonomous(&ctx, &session_id).await {
            tracing::error!("自律会話の実行に失敗: {e}");
        }
    });
    Ok(())
}

#[tauri::command]
pub fn stop_session(state: State<AppState>, session_id: String) -> AppResult<()> {
    // FR-14: 次の発話生成前に停止。進行中の発話もキャンセルする
    state.ctx.conv.set_stop(&session_id);
    Ok(())
}

#[tauri::command]
pub fn list_sessions(state: State<AppState>, persona_id: String) -> AppResult<Vec<Session>> {
    state.ctx.db.list_sessions_for_persona(&persona_id)
}

#[tauri::command]
pub fn get_session_utterances(state: State<AppState>, session_id: String) -> AppResult<Vec<Utterance>> {
    state.ctx.db.utterances_of(&session_id)
}

// ---------- 記憶 (FR-10/11) ----------

#[tauri::command]
pub fn list_memories(
    state: State<AppState>,
    persona_id: String,
    include_archived: bool,
) -> AppResult<Vec<Memory>> {
    state.ctx.db.memories_of(&persona_id, include_archived)
}

#[tauri::command]
pub fn update_memory(state: State<AppState>, id: String, content: String) -> AppResult<()> {
    let cleaned = validate_message(&content, 300)?;
    state.ctx.db.update_memory_content(&id, &cleaned)?;
    // 編集後の埋め込み再計算をキュー投入 (FR-11)
    let _ = state.ctx.worker_tx.send(Job::Reembed);
    Ok(())
}

#[tauri::command]
pub fn delete_memory(state: State<AppState>, id: String) -> AppResult<()> {
    state.ctx.db.delete_memory(&id)
}

// ---------- 人格 (FR-13) ----------

#[tauri::command]
pub fn get_personality_history(
    state: State<AppState>,
    persona_id: String,
) -> AppResult<Vec<PersonalityEvent>> {
    state.ctx.db.personality_events_of(&persona_id)
}

// ---------- エクスポート/インポート (FR-18) ----------

#[tauri::command]
pub fn export_persona(
    state: State<AppState>,
    persona_id: String,
    include_history: bool,
    path: String,
) -> AppResult<crate::export::ExportSummary> {
    crate::export::export_to_file(&state.ctx, &persona_id, include_history, &path)
}

#[tauri::command]
pub fn import_persona(state: State<AppState>, path: String, force: bool) -> AppResult<Persona> {
    crate::export::import_from_file(&state.ctx, &path, force)
}

// ---------- 設定・接続 (FR-16) ----------

#[tauri::command]
pub fn get_settings(state: State<AppState>) -> AppResult<Settings> {
    state.ctx.db.load_settings()
}

#[tauri::command]
pub fn update_settings(state: State<AppState>, settings: Settings) -> AppResult<()> {
    state.ctx.db.save_settings(&settings)?;
    state.http.set_endpoint(settings.endpoint.clone());
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionTestResult {
    pub connected: bool,
    pub models: Vec<String>,
    pub chat_model_found: bool,
    pub embed_ok: bool,
    pub message: String,
}

#[tauri::command]
pub async fn test_connection(state: State<'_, AppState>) -> AppResult<ConnectionTestResult> {
    let settings = state.ctx.db.load_settings()?;
    match state.ctx.inference.list_models().await {
        Ok(models) => {
            let chat_model_found =
                !settings.chat_model.is_empty() && models.contains(&settings.chat_model);
            // embeddings は個別に疎通確認する (設計6.1)
            let embed_ok = if settings.embed_model.is_empty() {
                false
            } else {
                state
                    .ctx
                    .inference
                    .embed(&settings.embed_model, &["接続確認".to_string()])
                    .await
                    .is_ok()
            };
            Ok(ConnectionTestResult {
                connected: true,
                models,
                chat_model_found,
                embed_ok,
                message: "接続に成功しました".into(),
            })
        }
        Err(e) => Ok(ConnectionTestResult {
            connected: false,
            models: vec![],
            chat_model_found: false,
            embed_ok: false,
            message: format!("接続できません: {e}。推論エンジンの起動と接続先設定を確認してください"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::*;

    #[test]
    fn sanitize_removes_control_chars_ec10() {
        assert_eq!(sanitize("abc\u{0007}def"), "abcdef");
        assert_eq!(sanitize("改行\nタブ\tは残る"), "改行\nタブ\tは残る");
        assert_eq!(sanitize("絵文字🎉サロゲート𠮷"), "絵文字🎉サロゲート𠮷");
    }

    #[test]
    fn validate_message_rejects_empty_and_long() {
        // EC-09: 空・空白のみは拒否
        assert!(validate_message("", 100).is_err());
        assert!(validate_message("   \n ", 100).is_err());
        // EC-05: 上限超過は拒否、上限ちょうどは通る
        assert!(validate_message(&"あ".repeat(101), 100).is_err());
        assert!(validate_message(&"あ".repeat(100), 100).is_ok());
    }

    #[test]
    fn validate_name_rules() {
        assert!(validate_name("").is_err()); // FR-01: 名前必須
        assert!(validate_name("  ").is_err());
        assert!(validate_name(&"あ".repeat(51)).is_err());
        assert_eq!(validate_name(" アリス ").unwrap(), "アリス");
    }

    fn input(name: &str) -> PersonaInput {
        PersonaInput {
            name: name.into(),
            description: "明るい".into(),
            speech_style: String::new(),
            values_text: String::new(),
            self_intro: String::new(),
            traits: vec![],
            force: false,
        }
    }

    #[test]
    fn duplicate_name_warns_then_force_creates_ec04() {
        let env = test_ctx(MockInference::default());
        do_create_persona(&env.ctx, input("アリス")).unwrap();
        // 同名は duplicate_name エラー
        let err = do_create_persona(&env.ctx, input("アリス")).unwrap_err();
        assert_eq!(err.kind(), "duplicate_name");
        // force で作成できる (別個体)
        let mut forced = input("アリス");
        forced.force = true;
        do_create_persona(&env.ctx, forced).unwrap();
        assert_eq!(env.ctx.db.list_personas().unwrap().len(), 2);
    }

    #[test]
    fn create_persona_initializes_all_traits() {
        let env = test_ctx(MockInference::default());
        let p = do_create_persona(
            &env.ctx,
            PersonaInput {
                traits: vec![TraitValue { key: "sociability".into(), value: 150 }],
                ..input("アリス")
            },
        )
        .unwrap();
        let traits = env.ctx.db.traits_of(&p.id).unwrap();
        assert_eq!(traits.len(), TRAIT_KEYS.len()); // 全軸が初期化される
        // 範囲外は 0-100 にクランプ
        assert_eq!(traits.iter().find(|t| t.key == "sociability").unwrap().value, 100);
        assert_eq!(traits.iter().find(|t| t.key == "empathy").unwrap().value, DEFAULT_TRAIT_VALUE);
    }

    #[test]
    fn update_persona_keeps_growth_fr03() {
        let env = test_ctx(MockInference::default());
        let p = do_create_persona(&env.ctx, input("アリス")).unwrap();
        env.ctx.db.set_trait(&p.id, "sociability", 77).unwrap(); // 成長分
        do_update_persona(&env.ctx, &p.id, input("アリス改")).unwrap();
        // 名前は変わり、成長分の trait は保持される
        assert_eq!(env.ctx.db.get_persona(&p.id).unwrap().unwrap().name, "アリス改");
        assert_eq!(
            env.ctx.db.traits_of(&p.id).unwrap().iter().find(|t| t.key == "sociability").unwrap().value,
            77
        );
    }
}
