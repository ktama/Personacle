use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::context::{AppCtx, Job};
use crate::error::{AppError, AppResult};
use crate::inference::{ChatRequest, InferError};
use crate::memory;
use crate::models::*;
use crate::prompt;

/// セッションの排他・キャンセル・停止フラグの管理 (EC-08, FR-07, FR-14)
#[derive(Default)]
pub struct ConversationManager {
    /// persona_id -> 参加中の session_id
    busy: Mutex<HashMap<String, String>>,
    /// session_id -> 現在の生成のキャンセルトークン
    cancels: Mutex<HashMap<String, CancellationToken>>,
    /// 自律会話の停止フラグ
    stops: Mutex<HashSet<String>>,
}

impl ConversationManager {
    /// EC-08: 1ペルソナは同時に1セッションのみ
    pub fn try_reserve(&self, persona_ids: &[(String, String)], session_id: &str) -> AppResult<()> {
        let mut busy = self.busy.lock().expect("busy lock");
        for (pid, pname) in persona_ids {
            if busy.contains_key(pid) {
                return Err(AppError::Busy(format!("「{pname}」は別の会話に参加中です")));
            }
        }
        for (pid, _) in persona_ids {
            busy.insert(pid.clone(), session_id.to_string());
        }
        Ok(())
    }

    pub fn release_session(&self, session_id: &str) {
        self.busy.lock().expect("busy lock").retain(|_, sid| sid != session_id);
        self.cancels.lock().expect("cancel lock").remove(session_id);
        self.stops.lock().expect("stop lock").remove(session_id);
    }

    fn new_token(&self, session_id: &str) -> CancellationToken {
        let token = CancellationToken::new();
        self.cancels
            .lock()
            .expect("cancel lock")
            .insert(session_id.to_string(), token.clone());
        token
    }

    pub fn cancel_generation(&self, session_id: &str) {
        if let Some(t) = self.cancels.lock().expect("cancel lock").get(session_id) {
            t.cancel();
        }
    }

    pub fn set_stop(&self, session_id: &str) {
        self.stops.lock().expect("stop lock").insert(session_id.to_string());
    }

    pub fn is_stopped(&self, session_id: &str) -> bool {
        self.stops.lock().expect("stop lock").contains(session_id)
    }
}

/// セッション開始 (FR-05 / FR-14)。参加ペルソナの排他を確保して active セッションを作る。
pub fn start_session(
    ctx: &AppCtx,
    kind: &str,
    persona_ids: &[String],
    theme: &str,
) -> AppResult<Session> {
    match kind {
        "user_dialogue" if persona_ids.len() == 1 => {}
        "autonomous" if persona_ids.len() == 2 => {}
        "user_dialogue" | "autonomous" => {
            return Err(AppError::Validation(
                "参加ペルソナ数が不正です (1対1対話は1体、自律会話は2体)".into(),
            ))
        }
        _ => return Err(AppError::Validation(format!("不明なセッション種別: {kind}"))),
    }

    let mut named = Vec::new();
    for pid in persona_ids {
        let p = ctx
            .db
            .get_persona(pid)?
            .ok_or_else(|| AppError::NotFound("ペルソナが見つかりません".into()))?;
        named.push((p.id, p.name));
    }

    let session = Session {
        id: new_id(),
        kind: kind.to_string(),
        theme: theme.to_string(),
        status: "active".into(),
        started_at: now_ms(),
        ended_at: None,
        participant_ids: named.iter().map(|(i, _)| i.clone()).collect(),
        participant_names: named.iter().map(|(_, n)| n.clone()).collect(),
    };
    ctx.conv.try_reserve(&named, &session.id)?;
    if let Err(e) = ctx.db.create_session(&session) {
        ctx.conv.release_session(&session.id);
        return Err(e);
    }
    Ok(session)
}

/// セッション終了 (設計7章)。排他を解放し後処理をキュー投入する (ADR-06)。
pub fn end_session(ctx: &AppCtx, session_id: &str) -> AppResult<()> {
    let Some(session) = ctx.db.get_session(session_id)? else {
        return Err(AppError::NotFound("セッションが見つかりません".into()));
    };
    if session.status == "active" {
        ctx.db.set_session_status(session_id, "ended", Some(now_ms()))?;
        ctx.sink.emit(
            "session_status_changed",
            json!({ "sessionId": session_id, "status": "ended" }),
        );
        let _ = ctx.worker_tx.send(Job::Postprocess(session_id.to_string()));
    }
    ctx.conv.release_session(session_id);
    Ok(())
}

/// ユーザー発話の受理と応答生成 (設計7章フロー1)
pub async fn send_user_message(ctx: &AppCtx, session_id: &str, text: &str) -> AppResult<()> {
    let session = ctx
        .db
        .get_session(session_id)?
        .filter(|s| s.status == "active" && s.kind == "user_dialogue")
        .ok_or_else(|| AppError::Validation("進行中の1対1セッションではありません".into()))?;

    // ユーザー発話を即時保存 (NFR-05: 応答生成前に確定)
    ctx.db.insert_utterance(&Utterance {
        id: new_id(),
        session_id: session_id.to_string(),
        speaker_kind: "user".into(),
        speaker_id: "user".into(),
        speaker_name: "ユーザー".into(),
        content: text.to_string(),
        state: "complete".into(),
        created_at: now_ms(),
    })?;

    let persona = ctx
        .db
        .get_persona(&session.participant_ids[0])?
        .ok_or_else(|| AppError::NotFound("ペルソナが見つかりません".into()))?;

    generate_reply(ctx, &session, &persona, "user", "user", "ユーザー").await?;
    Ok(())
}

/// 1発話の生成 (想起→プロンプト→ストリーミング→保存)。1対1と自律会話で共用。
async fn generate_reply(
    ctx: &AppCtx,
    session: &Session,
    speaker: &Persona,
    partner_kind: &str,
    partner_id: &str,
    partner_name: &str,
) -> AppResult<Utterance> {
    let settings = ctx.db.load_settings()?;
    let history = ctx.db.utterances_of(&session.id)?;

    // 想起クエリ: 直近3発話。埋め込み失敗時は縮退 (設計7章フロー1)
    let query_text: String = history
        .iter()
        .rev()
        .take(3)
        .map(|u| u.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let query_emb = if !settings.embed_model.is_empty() && !query_text.is_empty() {
        match ctx.inference.embed(&settings.embed_model, &[query_text]).await {
            Ok(mut v) if !v.is_empty() => Some(v.remove(0)),
            Ok(_) => None,
            Err(e) => {
                tracing::warn!("想起用の埋め込みに失敗(縮退動作): {e}");
                None
            }
        }
    } else {
        None
    };
    let memories =
        memory::retrieve(&ctx.db, &speaker.id, query_emb.as_deref(), &settings, now_ms())?;

    let traits = ctx.db.traits_of(&speaker.id)?;
    let relationship = ctx.db.get_relationship(&speaker.id, partner_kind, partner_id)?;
    let theme = if session.kind == "autonomous" { Some(session.theme.as_str()) } else { None };
    let system =
        prompt::build_system(speaker, &traits, relationship.as_ref(), partner_name, &memories, theme);
    let messages =
        prompt::assemble_messages(system, &history, &speaker.id, settings.context_chars.max(1000) as usize);

    let utterance_id = new_id();
    ctx.sink.emit(
        "utterance_started",
        json!({
            "sessionId": session.id,
            "utteranceId": utterance_id,
            "speakerId": speaker.id,
            "speakerName": speaker.name,
        }),
    );

    let token = ctx.conv.new_token(&session.id);
    let req = ChatRequest {
        model: settings.chat_model.clone(),
        messages,
        temperature: 0.8,
        max_tokens: Some(512),
    };
    let sink = ctx.sink.clone();
    let uid = utterance_id.clone();
    let sid = session.id.clone();
    let mut on_delta = move |d: String| {
        sink.emit(
            "utterance_delta",
            json!({ "sessionId": sid, "utteranceId": uid, "delta": d }),
        );
    };

    let outcome = match ctx.inference.chat_stream(req, token, &mut on_delta).await {
        Ok(o) => o,
        Err(e) => {
            let kind = match &e {
                InferError::Connection(_) => "connection",
                InferError::Generation(_) => "generation",
            };
            ctx.sink.emit(
                "generation_failed",
                json!({ "sessionId": session.id, "kind": kind, "message": e.to_string() }),
            );
            return Err(match e {
                InferError::Connection(m) => AppError::Connection(m),
                InferError::Generation(m) => AppError::Generation(m),
            });
        }
    };

    // FR-07: キャンセル時も途中までの本文で保存する
    let state = if outcome.canceled { "canceled" } else { "complete" };
    let utterance = Utterance {
        id: utterance_id.clone(),
        session_id: session.id.clone(),
        speaker_kind: "persona".into(),
        speaker_id: speaker.id.clone(),
        speaker_name: speaker.name.clone(),
        content: outcome.text,
        state: state.into(),
        created_at: now_ms(),
    };
    ctx.db.insert_utterance(&utterance)?;
    ctx.db.touch_last_talked(&speaker.id, now_ms())?;
    ctx.sink.emit(
        "utterance_completed",
        json!({ "sessionId": session.id, "utteranceId": utterance_id, "state": state }),
    );
    Ok(utterance)
}

/// 自律会話のターンループ (設計7章フロー2)
pub async fn run_autonomous(ctx: &AppCtx, session_id: &str) -> AppResult<()> {
    let session = ctx
        .db
        .get_session(session_id)?
        .filter(|s| s.status == "active" && s.kind == "autonomous")
        .ok_or_else(|| AppError::Validation("進行中の自律会話セッションではありません".into()))?;

    let mut personas = Vec::new();
    for pid in &session.participant_ids {
        personas.push(
            ctx.db
                .get_persona(pid)?
                .ok_or_else(|| AppError::NotFound("参加ペルソナが見つかりません".into()))?,
        );
    }

    let settings = ctx.db.load_settings()?;
    let limit = settings.auto_turn_limit.clamp(2, AUTO_TURN_HARD_MAX);
    let mut consecutive_failures = 0;

    for turn in 0..limit {
        // FR-14: 停止フラグは次の発話生成前に検査する
        if ctx.conv.is_stopped(session_id) {
            break;
        }
        let speaker = &personas[(turn % 2) as usize];
        let partner = &personas[((turn + 1) % 2) as usize];
        match generate_reply(ctx, &session, speaker, "persona", &partner.id, &partner.name).await {
            Ok(_) => consecutive_failures = 0,
            Err(e) => {
                consecutive_failures += 1;
                tracing::warn!("自律会話の発話生成に失敗 ({consecutive_failures}回目): {e}");
                if consecutive_failures >= 2 {
                    break; // EC-12: 連続失敗で打ち切り
                }
            }
        }
    }
    end_session(ctx, session_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::*;

    #[tokio::test]
    async fn busy_exclusion_ec08() {
        let env = test_ctx(MockInference::default());
        let a = add_persona(&env.ctx, "アリス");
        let b = add_persona(&env.ctx, "ボブ");

        let s1 = start_session(&env.ctx, "user_dialogue", &[a.id.clone()], "").unwrap();
        // アリスは参加中 → 自律会話は Busy
        let err = start_session(&env.ctx, "autonomous", &[a.id.clone(), b.id.clone()], "").unwrap_err();
        assert_eq!(err.kind(), "busy");
        // 終了後は開始できる
        end_session(&env.ctx, &s1.id).unwrap();
        start_session(&env.ctx, "autonomous", &[a.id.clone(), b.id.clone()], "テーマ").unwrap();
    }

    #[tokio::test]
    async fn user_message_saves_and_streams() {
        let env = test_ctx(MockInference::with_replies(&["こんにちは、元気です"]));
        let a = add_persona(&env.ctx, "アリス");
        let s = start_session(&env.ctx, "user_dialogue", &[a.id.clone()], "").unwrap();

        send_user_message(&env.ctx, &s.id, "やあ、元気?").await.unwrap();

        let utts = env.ctx.db.utterances_of(&s.id).unwrap();
        assert_eq!(utts.len(), 2);
        assert_eq!(utts[0].speaker_kind, "user");
        assert_eq!(utts[1].content, "こんにちは、元気です");
        assert_eq!(utts[1].state, "complete");

        let names = env.sink.names();
        assert!(names.contains(&"utterance_started".to_string()));
        assert!(names.contains(&"utterance_delta".to_string())); // FR-05 逐次表示
        assert!(names.contains(&"utterance_completed".to_string()));
        // last_talked_at が更新される (FR-02)
        assert!(env.ctx.db.get_persona(&a.id).unwrap().unwrap().last_talked_at.is_some());
    }

    #[tokio::test]
    async fn cancel_saves_partial_fr07() {
        let mut mock = MockInference::with_replies(&["これはとても長い応答でキャンセルされる予定のもの"]);
        mock.chunk_delay_ms = 30;
        let env = test_ctx(mock);
        let a = add_persona(&env.ctx, "アリス");
        let s = start_session(&env.ctx, "user_dialogue", &[a.id.clone()], "").unwrap();

        let ctx2 = env.ctx.clone();
        let sid = s.id.clone();
        let task = tokio::spawn(async move { send_user_message(&ctx2, &sid, "話して").await });
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        env.ctx.conv.cancel_generation(&s.id);
        task.await.unwrap().unwrap();

        let utts = env.ctx.db.utterances_of(&s.id).unwrap();
        assert_eq!(utts.len(), 2);
        let reply = &utts[1];
        assert_eq!(reply.state, "canceled");
        // 途中までの本文が残る
        assert!(!reply.content.is_empty());
        assert!(reply.content.chars().count() < "これはとても長い応答でキャンセルされる予定のもの".chars().count());
    }

    #[tokio::test]
    async fn generation_failure_emits_event_ec02() {
        let env = test_ctx(MockInference::default()); // 応答キューが空 → 生成失敗
        let a = add_persona(&env.ctx, "アリス");
        let s = start_session(&env.ctx, "user_dialogue", &[a.id.clone()], "").unwrap();

        let err = send_user_message(&env.ctx, &s.id, "やあ").await.unwrap_err();
        assert_eq!(err.kind(), "generation");
        assert!(env.sink.names().contains(&"generation_failed".to_string()));
        // ユーザー発話自体は保存済み (入力は失われない)
        assert_eq!(env.ctx.db.utterances_of(&s.id).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn autonomous_alternates_and_ends_fr14() {
        let env = test_ctx(MockInference::with_replies(&["発話1", "発話2", "発話3", "発話4"]));
        let mut settings = env.ctx.db.load_settings().unwrap();
        settings.auto_turn_limit = 4;
        env.ctx.db.save_settings(&settings).unwrap();

        let a = add_persona(&env.ctx, "アリス");
        let b = add_persona(&env.ctx, "ボブ");
        let s = start_session(&env.ctx, "autonomous", &[a.id.clone(), b.id.clone()], "趣味の話").unwrap();
        run_autonomous(&env.ctx, &s.id).await.unwrap();

        let utts = env.ctx.db.utterances_of(&s.id).unwrap();
        assert_eq!(utts.len(), 4); // ターン上限で自動終了
        // 交互に発話 (FR-14)
        assert_eq!(utts[0].speaker_id, a.id);
        assert_eq!(utts[1].speaker_id, b.id);
        assert_eq!(utts[2].speaker_id, a.id);
        assert_eq!(utts[3].speaker_id, b.id);

        let session = env.ctx.db.get_session(&s.id).unwrap().unwrap();
        assert_eq!(session.status, "ended");
    }

    #[tokio::test]
    async fn autonomous_postprocess_job_enqueued() {
        let mut env = test_ctx(MockInference::with_replies(&["発話1", "発話2"]));
        let mut settings = env.ctx.db.load_settings().unwrap();
        settings.auto_turn_limit = 2;
        env.ctx.db.save_settings(&settings).unwrap();

        let a = add_persona(&env.ctx, "アリス");
        let b = add_persona(&env.ctx, "ボブ");
        let s = start_session(&env.ctx, "autonomous", &[a.id.clone(), b.id.clone()], "").unwrap();
        run_autonomous(&env.ctx, &s.id).await.unwrap();

        // ADR-06: 終了時に後処理ジョブが積まれる
        assert_eq!(env.job_rx.try_recv().unwrap(), Job::Postprocess(s.id.clone()));
    }

    #[tokio::test]
    async fn autonomous_stops_on_flag_fr14() {
        let mut mock = MockInference::with_replies(&["発話1", "発話2", "発話3", "発話4"]);
        mock.chunk_delay_ms = 30;
        let env = test_ctx(mock);
        let mut settings = env.ctx.db.load_settings().unwrap();
        settings.auto_turn_limit = 4;
        env.ctx.db.save_settings(&settings).unwrap();

        let a = add_persona(&env.ctx, "アリス");
        let b = add_persona(&env.ctx, "ボブ");
        let s = start_session(&env.ctx, "autonomous", &[a.id.clone(), b.id.clone()], "").unwrap();

        let ctx2 = env.ctx.clone();
        let sid = s.id.clone();
        let task = tokio::spawn(async move { run_autonomous(&ctx2, &sid).await });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        env.ctx.conv.set_stop(&s.id); // 手動停止
        task.await.unwrap().unwrap();

        let utts = env.ctx.db.utterances_of(&s.id).unwrap();
        assert!(utts.len() < 4, "停止フラグで上限前に止まる (実際: {}件)", utts.len());
        assert_eq!(env.ctx.db.get_session(&s.id).unwrap().unwrap().status, "ended");
    }

    #[tokio::test]
    async fn autonomous_aborts_after_consecutive_failures_ec12() {
        // 応答キュー空 → 全ターン失敗 → 2連続失敗で打ち切り、セッションは終了する
        let env = test_ctx(MockInference::default());
        let a = add_persona(&env.ctx, "アリス");
        let b = add_persona(&env.ctx, "ボブ");
        let s = start_session(&env.ctx, "autonomous", &[a.id.clone(), b.id.clone()], "").unwrap();
        run_autonomous(&env.ctx, &s.id).await.unwrap();
        assert_eq!(env.ctx.db.utterances_of(&s.id).unwrap().len(), 0);
        assert_eq!(env.ctx.db.get_session(&s.id).unwrap().unwrap().status, "ended");
    }
}
