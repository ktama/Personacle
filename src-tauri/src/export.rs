//! FR-18: ペルソナのエクスポート/インポート。
//! JSON 1ファイルに persona/trait/relationship/memory/personality_event を書き出す。
//! 会話履歴 (sessions) は選択制。埋め込みベクトルは環境依存のため含めず、取込後に再計算する。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::commands::{sanitize, validate_name};
use crate::context::{AppCtx, Job};
use crate::error::{AppError, AppResult};
use crate::models::*;
use crate::personality::{DEFAULT_TRAIT_VALUE, TRAIT_KEYS};

pub const EXPORT_FORMAT: &str = "personacle-persona";
pub const EXPORT_FORMAT_VERSION: i64 = 1;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportFile {
    pub format: String,
    pub format_version: i64,
    /// 書き出し元アプリの DB schema_version (設計5.3)
    pub app_schema_version: i64,
    pub exported_at: i64,
    pub persona: ExportPersona,
    pub traits: Vec<TraitValue>,
    pub relationships: Vec<ExportRelationship>,
    pub memories: Vec<ExportMemory>,
    pub personality_events: Vec<ExportEvent>,
    #[serde(default)]
    pub sessions: Vec<ExportSession>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportPersona {
    /// 書き出し元でのID。取込時の参照の張り替えにのみ使う
    pub id: String,
    pub name: String,
    pub description: String,
    pub speech_style: String,
    pub values_text: String,
    pub self_intro: String,
    pub created_at: i64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportRelationship {
    pub target_kind: String,
    pub target_id: String,
    pub target_name: String,
    pub intimacy: i64,
    pub impression_text: String,
    pub updated_at: i64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportMemory {
    pub content: String,
    pub kind: String,
    pub importance: i64,
    pub created_at: i64,
    pub archived: bool,
    pub user_edited: bool,
    pub source_session_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportEvent {
    pub session_id: Option<String>,
    pub item: String,
    pub old_value: String,
    pub new_value: String,
    pub created_at: i64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportSession {
    pub id: String,
    pub kind: String,
    pub theme: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub participants: Vec<ExportParticipant>,
    pub utterances: Vec<ExportUtterance>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportParticipant {
    pub persona_id: String,
    pub persona_name: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportUtterance {
    pub speaker_kind: String,
    pub speaker_id: String,
    pub speaker_name: String,
    pub content: String,
    pub state: String,
    pub created_at: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportSummary {
    pub memory_count: usize,
    pub session_count: usize,
}

// ---------- エクスポート ----------

pub fn build_export(ctx: &AppCtx, persona_id: &str, include_history: bool) -> AppResult<ExportFile> {
    let persona = ctx
        .db
        .get_persona(persona_id)?
        .ok_or_else(|| AppError::NotFound("ペルソナが見つかりません".into()))?;

    let sessions = if include_history {
        let mut out = Vec::new();
        for s in ctx.db.list_sessions_for_persona(persona_id)? {
            let participants = ctx
                .db
                .participants_of(&s.id)?
                .into_iter()
                .map(|(pid, pname, _)| ExportParticipant { persona_id: pid, persona_name: pname })
                .collect();
            let utterances = ctx
                .db
                .utterances_of(&s.id)?
                .into_iter()
                .map(|u| ExportUtterance {
                    speaker_kind: u.speaker_kind,
                    speaker_id: u.speaker_id,
                    speaker_name: u.speaker_name,
                    content: u.content,
                    state: u.state,
                    created_at: u.created_at,
                })
                .collect();
            out.push(ExportSession {
                id: s.id,
                kind: s.kind,
                theme: s.theme,
                started_at: s.started_at,
                ended_at: s.ended_at,
                participants,
                utterances,
            });
        }
        out
    } else {
        vec![]
    };

    Ok(ExportFile {
        format: EXPORT_FORMAT.into(),
        format_version: EXPORT_FORMAT_VERSION,
        app_schema_version: crate::db::SCHEMA_VERSION,
        exported_at: now_ms(),
        traits: ctx.db.traits_of(persona_id)?,
        relationships: ctx
            .db
            .relationships_of(persona_id)?
            .into_iter()
            .map(|r| ExportRelationship {
                target_kind: r.target_kind,
                target_id: r.target_id,
                target_name: r.target_name,
                intimacy: r.intimacy,
                impression_text: r.impression_text,
                updated_at: r.updated_at,
            })
            .collect(),
        memories: ctx
            .db
            .memories_of(persona_id, true)?
            .into_iter()
            .map(|m| ExportMemory {
                content: m.content,
                kind: m.kind,
                importance: m.importance,
                created_at: m.created_at,
                archived: m.archived,
                user_edited: m.user_edited,
                source_session_id: m.source_session_id,
            })
            .collect(),
        personality_events: ctx
            .db
            .personality_events_of(persona_id)?
            .into_iter()
            .map(|e| ExportEvent {
                session_id: e.session_id,
                item: e.item,
                old_value: e.old_value,
                new_value: e.new_value,
                created_at: e.created_at,
            })
            .collect(),
        persona: ExportPersona {
            id: persona.id,
            name: persona.name,
            description: persona.description,
            speech_style: persona.speech_style,
            values_text: persona.values_text,
            self_intro: persona.self_intro,
            created_at: persona.created_at,
        },
        sessions,
    })
}

pub fn export_to_file(
    ctx: &AppCtx,
    persona_id: &str,
    include_history: bool,
    path: &str,
) -> AppResult<ExportSummary> {
    let file = build_export(ctx, persona_id, include_history)?;
    let json = serde_json::to_string_pretty(&file)
        .map_err(|e| AppError::Data(format!("エクスポートの直列化に失敗: {e}")))?;
    std::fs::write(path, json)
        .map_err(|e| AppError::Data(format!("ファイルの書き込みに失敗 ({path}): {e}")))?;
    Ok(ExportSummary { memory_count: file.memories.len(), session_count: file.sessions.len() })
}

// ---------- インポート ----------

fn clamp_chars(s: &str, max: usize) -> String {
    sanitize(s).chars().take(max).collect()
}

/// 取込前の形式・バージョン検査。構造体変換より先に行い、分かるエラーを返す
pub fn validate_export_value(v: &Value) -> AppResult<()> {
    if v["format"].as_str() != Some(EXPORT_FORMAT) {
        return Err(AppError::Validation("Personacle のペルソナファイルではありません".into()));
    }
    let version = v["formatVersion"].as_i64().unwrap_or(0);
    if version < 1 || version > EXPORT_FORMAT_VERSION {
        return Err(AppError::Validation(format!(
            "このファイルの形式バージョン ({version}) には対応していません。アプリを更新してください"
        )));
    }
    Ok(())
}

pub fn import_value(ctx: &AppCtx, v: &Value, force: bool) -> AppResult<Persona> {
    validate_export_value(v)?;
    let file: ExportFile = serde_json::from_value(v.clone())
        .map_err(|e| AppError::Validation(format!("インポートファイルの内容が不正です: {e}")))?;

    let name = validate_name(&file.persona.name)?;
    // EC-04 と同様: 同名は警告し force で取込
    if !force && ctx.db.persona_name_exists(&name, None)? {
        return Err(AppError::DuplicateName(format!(
            "「{name}」という名前のペルソナは既に存在します"
        )));
    }

    let old_persona_id = file.persona.id.clone();
    let new_persona = Persona {
        id: new_id(),
        name,
        description: clamp_chars(&file.persona.description, 2000),
        speech_style: clamp_chars(&file.persona.speech_style, 1000),
        values_text: clamp_chars(&file.persona.values_text, 1000),
        self_intro: clamp_chars(&file.persona.self_intro, 1000),
        created_at: file.persona.created_at,
        last_talked_at: None,
    };
    let traits: Vec<TraitValue> = TRAIT_KEYS
        .iter()
        .map(|key| TraitValue {
            key: key.to_string(),
            value: file
                .traits
                .iter()
                .find(|t| t.key == *key)
                .map(|t| t.value.clamp(0, 100))
                .unwrap_or(DEFAULT_TRAIT_VALUE),
        })
        .collect();
    ctx.db.create_persona(&new_persona, &traits)?;

    // 会話履歴 (選択制)。取込後の後処理再実行を防ぐため processed として登録する
    let mut session_map: HashMap<String, String> = HashMap::new();
    for s in &file.sessions {
        let new_sid = new_id();
        session_map.insert(s.id.clone(), new_sid.clone());
        let map_pid = |pid: &str| -> String {
            if pid == old_persona_id { new_persona.id.clone() } else { pid.to_string() }
        };
        ctx.db.create_session(&Session {
            id: new_sid.clone(),
            kind: s.kind.clone(),
            theme: clamp_chars(&s.theme, 500),
            status: "processed".into(),
            started_at: s.started_at,
            ended_at: s.ended_at,
            participant_ids: s.participants.iter().map(|p| map_pid(&p.persona_id)).collect(),
            participant_names: s.participants.iter().map(|p| clamp_chars(&p.persona_name, 50)).collect(),
        })?;
        for p in &s.participants {
            ctx.db.mark_participant_processed(&new_sid, &map_pid(&p.persona_id))?;
        }
        for u in &s.utterances {
            ctx.db.insert_utterance(&Utterance {
                id: new_id(),
                session_id: new_sid.clone(),
                speaker_kind: u.speaker_kind.clone(),
                speaker_id: map_pid(&u.speaker_id),
                speaker_name: clamp_chars(&u.speaker_name, 50),
                content: sanitize(&u.content),
                state: u.state.clone(),
                created_at: u.created_at,
            })?;
        }
    }

    // 記憶: 埋め込みは環境依存のため含まれない。NULL で取込み、後で再計算する
    for m in &file.memories {
        let content = clamp_chars(&m.content, 300);
        if content.trim().is_empty() {
            continue;
        }
        ctx.db.insert_memory(
            &Memory {
                id: new_id(),
                persona_id: new_persona.id.clone(),
                content,
                kind: match m.kind.as_str() {
                    k @ ("fact" | "event" | "promise" | "impression") => k.to_string(),
                    _ => "fact".to_string(),
                },
                importance: m.importance.clamp(1, 10),
                has_embedding: false,
                source_session_id: m
                    .source_session_id
                    .as_ref()
                    .and_then(|old| session_map.get(old).cloned()),
                created_at: m.created_at,
                archived: m.archived,
                user_edited: m.user_edited,
            },
            None,
        )?;
    }

    // 関係性: 相手ペルソナはこの環境に存在しない可能性があるが、
    // 名前スナップショットにより閲覧できる (EC-07 の削除済み表示と同じ扱い)
    for r in &file.relationships {
        ctx.db.upsert_relationship(&Relationship {
            persona_id: new_persona.id.clone(),
            target_kind: r.target_kind.clone(),
            target_id: r.target_id.clone(),
            target_name: clamp_chars(&r.target_name, 50),
            intimacy: r.intimacy.clamp(0, 100),
            impression_text: clamp_chars(&r.impression_text, 200),
            updated_at: r.updated_at,
        })?;
    }

    for e in &file.personality_events {
        ctx.db.insert_personality_event(&PersonalityEvent {
            id: new_id(),
            persona_id: new_persona.id.clone(),
            session_id: e.session_id.as_ref().and_then(|old| session_map.get(old).cloned()),
            item: clamp_chars(&e.item, 100),
            old_value: clamp_chars(&e.old_value, 300),
            new_value: clamp_chars(&e.new_value, 300),
            created_at: e.created_at,
        })?;
    }

    // 埋め込みの再計算をキュー投入
    let _ = ctx.worker_tx.send(Job::Reembed);
    Ok(new_persona)
}

pub fn import_from_file(ctx: &AppCtx, path: &str, force: bool) -> AppResult<Persona> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| AppError::Data(format!("ファイルを読み込めません ({path}): {e}")))?;
    let v: Value = serde_json::from_str(&text)
        .map_err(|_| AppError::Validation("JSONファイルとして読み込めません".into()))?;
    import_value(ctx, &v, force)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::*;

    /// 記憶・関係・履歴・変化イベントを持つペルソナを作る
    fn seed(env: &TestEnv) -> (Persona, String) {
        let p = add_persona(&env.ctx, "アリス");
        env.ctx.db.set_trait(&p.id, "sociability", 72).unwrap();

        let s = Session {
            id: new_id(),
            kind: "user_dialogue".into(),
            theme: String::new(),
            status: "processed".into(),
            started_at: now_ms(),
            ended_at: Some(now_ms()),
            participant_ids: vec![p.id.clone()],
            participant_names: vec![p.name.clone()],
        };
        env.ctx.db.create_session(&s).unwrap();
        env.ctx.db.insert_utterance(&Utterance {
            id: new_id(), session_id: s.id.clone(), speaker_kind: "user".into(),
            speaker_id: "user".into(), speaker_name: "ユーザー".into(),
            content: "好物はカレーです".into(), state: "complete".into(), created_at: now_ms(),
        }).unwrap();

        env.ctx.db.insert_memory(
            &Memory {
                id: new_id(), persona_id: p.id.clone(),
                content: "ユーザーの好物はカレー".into(), kind: "fact".into(), importance: 7,
                has_embedding: true, source_session_id: Some(s.id.clone()),
                created_at: 1_700_000_000_000, archived: false, user_edited: false,
            },
            Some(&[1u8; 8]),
        ).unwrap();
        env.ctx.db.insert_memory(
            &Memory {
                id: new_id(), persona_id: p.id.clone(),
                content: "古い雑談".into(), kind: "event".into(), importance: 2,
                has_embedding: false, source_session_id: None,
                created_at: 1_600_000_000_000, archived: true, user_edited: false,
            },
            None,
        ).unwrap();

        env.ctx.db.upsert_relationship(&Relationship {
            persona_id: p.id.clone(), target_kind: "user".into(), target_id: "user".into(),
            target_name: "ユーザー".into(), intimacy: 55,
            impression_text: "優しい人".into(), updated_at: now_ms(),
        }).unwrap();

        env.ctx.db.insert_personality_event(&PersonalityEvent {
            id: new_id(), persona_id: p.id.clone(), session_id: Some(s.id.clone()),
            item: "intimacy:ユーザー".into(), old_value: "50".into(), new_value: "55".into(),
            created_at: now_ms(),
        }).unwrap();

        (p, s.id)
    }

    /// FR-18 受け入れ基準: 別環境へのインポートで初期設定・人格・記憶が再現される (履歴あり)
    #[test]
    fn roundtrip_with_history() {
        let env1 = test_ctx(MockInference::default());
        let (p, _sid) = seed(&env1);
        let file = build_export(&env1.ctx, &p.id, true).unwrap();
        let v = serde_json::to_value(&file).unwrap();

        // 「別のPC」= 独立した空のDB
        let mut env2 = test_ctx(MockInference::default());
        let imported = import_value(&env2.ctx, &v, false).unwrap();

        // 初期設定
        let got = env2.ctx.db.get_persona(&imported.id).unwrap().unwrap();
        assert_eq!(got.name, "アリス");
        assert_eq!(got.description, p.description);
        assert_ne!(got.id, p.id); // IDは新規発行

        // 人格プロファイル
        let traits = env2.ctx.db.traits_of(&imported.id).unwrap();
        assert_eq!(traits.iter().find(|t| t.key == "sociability").unwrap().value, 72);
        let rel = env2.ctx.db.get_relationship(&imported.id, "user", "user").unwrap().unwrap();
        assert_eq!(rel.intimacy, 55);
        assert_eq!(rel.impression_text, "優しい人");
        assert_eq!(env2.ctx.db.personality_events_of(&imported.id).unwrap().len(), 1);

        // 記憶 (アーカイブ含め全件。埋め込みは再計算待ちで NULL)
        let mems = env2.ctx.db.memories_of(&imported.id, true).unwrap();
        assert_eq!(mems.len(), 2);
        let curry = mems.iter().find(|m| m.content.contains("カレー")).unwrap();
        assert_eq!(curry.importance, 7);
        assert_eq!(curry.created_at, 1_700_000_000_000); // 発生日時を保持
        assert!(!curry.has_embedding);
        assert!(curry.source_session_id.is_some()); // 出所が新セッションIDに張り替わる

        // 会話履歴: processed で取り込まれ、後処理は再実行されない
        let sessions = env2.ctx.db.list_sessions_for_persona(&imported.id).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].status, "processed");
        assert_eq!(curry.source_session_id.as_deref(), Some(sessions[0].id.as_str()));
        assert_eq!(env2.ctx.db.utterances_of(&sessions[0].id).unwrap().len(), 1);
        assert!(env2.ctx.db.participants_of(&sessions[0].id).unwrap().iter().all(|(_, _, done)| *done));

        // 埋め込み再計算がキュー投入される
        assert_eq!(env2.job_rx.try_recv().unwrap(), Job::Reembed);
    }

    /// 会話履歴を含めない選択 (FR-18)
    #[test]
    fn roundtrip_without_history() {
        let env1 = test_ctx(MockInference::default());
        let (p, _sid) = seed(&env1);
        let file = build_export(&env1.ctx, &p.id, false).unwrap();
        assert!(file.sessions.is_empty());
        let v = serde_json::to_value(&file).unwrap();

        let env2 = test_ctx(MockInference::default());
        let imported = import_value(&env2.ctx, &v, false).unwrap();

        assert!(env2.ctx.db.list_sessions_for_persona(&imported.id).unwrap().is_empty());
        let mems = env2.ctx.db.memories_of(&imported.id, true).unwrap();
        assert_eq!(mems.len(), 2);
        // 出所セッションが存在しないため参照は外れるが記憶は残る
        assert!(mems.iter().all(|m| m.source_session_id.is_none()));
        // 変化イベントのセッション参照も外れる
        assert!(env2.ctx.db.personality_events_of(&imported.id).unwrap()[0].session_id.is_none());
    }

    /// 同名ペルソナが既存なら警告し、force で別個体として取込 (EC-04 相当)
    #[test]
    fn duplicate_name_requires_force() {
        let env1 = test_ctx(MockInference::default());
        let (p, _) = seed(&env1);
        let v = serde_json::to_value(build_export(&env1.ctx, &p.id, false).unwrap()).unwrap();

        // 同じ環境に取り込む = 同名が存在する
        let err = import_value(&env1.ctx, &v, false).unwrap_err();
        assert_eq!(err.kind(), "duplicate_name");
        import_value(&env1.ctx, &v, true).unwrap();
        assert_eq!(env1.ctx.db.list_personas().unwrap().len(), 2);
    }

    #[test]
    fn rejects_invalid_files() {
        let env = test_ctx(MockInference::default());
        // 別形式
        let v: Value = serde_json::json!({"format": "other-app", "formatVersion": 1});
        assert_eq!(import_value(&env.ctx, &v, false).unwrap_err().kind(), "validation");
        // 未来のバージョン
        let v: Value = serde_json::json!({"format": EXPORT_FORMAT, "formatVersion": 99});
        assert_eq!(import_value(&env.ctx, &v, false).unwrap_err().kind(), "validation");
        // 必須フィールド欠落
        let v: Value = serde_json::json!({"format": EXPORT_FORMAT, "formatVersion": 1});
        assert_eq!(import_value(&env.ctx, &v, false).unwrap_err().kind(), "validation");
    }

    /// FR-18 受け入れ基準 (ファイル経由): エクスポート→ファイル→別環境でインポート
    #[test]
    fn roundtrip_via_file() {
        let env1 = test_ctx(MockInference::default());
        let (p, _) = seed(&env1);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("アリス.personacle.json");
        let path_str = path.to_str().unwrap();

        let summary = export_to_file(&env1.ctx, &p.id, true, path_str).unwrap();
        assert_eq!(summary.memory_count, 2);
        assert_eq!(summary.session_count, 1);

        let env2 = test_ctx(MockInference::default());
        let imported = import_from_file(&env2.ctx, path_str, false).unwrap();
        assert_eq!(imported.name, "アリス");
        assert_eq!(env2.ctx.db.memories_of(&imported.id, true).unwrap().len(), 2);

        // 壊れたファイル
        std::fs::write(&path, "{broken").unwrap();
        assert_eq!(import_from_file(&env2.ctx, path_str, false).unwrap_err().kind(), "validation");
    }
}
