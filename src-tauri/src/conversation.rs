use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::context::{AppCtx, Job};
use crate::error::{AppError, AppResult};
use crate::inference::{ChatRequest, InferError};
use crate::memory;
use crate::models::*;
use crate::personality;
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

    pub fn clear_stop(&self, session_id: &str) {
        self.stops.lock().expect("stop lock").remove(session_id);
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
        // FR-19: 自律会話は2〜MAX体
        "autonomous" if (2..=MAX_AUTONOMOUS_PARTICIPANTS).contains(&persona_ids.len()) => {}
        // FR-31: グループチャットは2〜MAX体 (ユーザーは常に参加)
        "group" if (2..=MAX_GROUP_PARTICIPANTS).contains(&persona_ids.len()) => {}
        "user_dialogue" | "autonomous" | "group" => {
            return Err(AppError::Validation(format!(
                "参加ペルソナ数が不正です (1対1対話は1体、自律会話・グループは2〜{MAX_GROUP_PARTICIPANTS}体)"
            )))
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

    generate_reply(ctx, &session, &persona, None).await?;
    Ok(())
}

/// 話しかけ (フロー6, FR-21, ADR-10)。ペルソナから会話を切り出す。
/// 生成したら true、しなかった(無効/間隔内/接続失敗など)なら false を返す。
pub async fn request_greeting(ctx: &AppCtx, session_id: &str) -> AppResult<bool> {
    let session = ctx
        .db
        .get_session(session_id)?
        .filter(|s| s.status == "active" && s.kind == "user_dialogue")
        .ok_or_else(|| AppError::Validation("進行中の1対1セッションではありません".into()))?;

    let settings = ctx.db.load_settings()?;
    if !settings.greeting_enabled {
        return Ok(false); // 設定で無効 (FR-21)
    }
    // 既にこのセッションで発話がある(話しかけ済み/ユーザーが話した)なら生成しない (EC-14)
    if !ctx.db.utterances_of(session_id)?.is_empty() {
        return Ok(false);
    }
    let persona = ctx
        .db
        .get_persona(&session.participant_ids[0])?
        .ok_or_else(|| AppError::NotFound("ペルソナが見つかりません".into()))?;

    // EC-14: 前回の話しかけから再生成間隔(既定60分)未満なら生成しない
    if let Some(last) = ctx.db.get_last_greeting_at(&persona.id)? {
        let elapsed_min = (now_ms() - last).max(0) / 60_000;
        if elapsed_min < settings.greeting_interval_min {
            return Ok(false);
        }
    }

    let hint = "(あなたの方から、この相手に自然に話しかけて会話を始めてください。前回からの間隔や覚えていることがあれば触れてかまいません)";
    // EC-13: 接続・生成失敗は無通知の縮退。エラーを伝播せず false を返す。
    match generate_reply(ctx, &session, &persona, Some(hint)).await {
        Ok(_) => {
            ctx.db.set_last_greeting_at(&persona.id, now_ms())?;
            Ok(true)
        }
        Err(_) => Ok(false),
    }
}

/// 1発話の生成 (想起→プロンプト→ストリーミング→保存)。1対1・自律会話・グループで共用。
/// 会話相手はセッションから導出する (1対1=ユーザー、自律会話=自分以外の全参加者: FR-19、
/// グループ=自分以外の全参加者+ユーザー: FR-31)。
/// opening_hint を渡すと、その指示を最後の user メッセージとして与え、ペルソナから会話を切り出させる (話しかけ FR-21)。
async fn generate_reply(
    ctx: &AppCtx,
    session: &Session,
    speaker: &Persona,
    opening_hint: Option<&str>,
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
    // 会話相手と関係性の一覧を組み立てる
    let mut partners: Vec<(String, Option<crate::models::Relationship>)> = Vec::new();
    if session.kind == "user_dialogue" {
        partners.push(("ユーザー".to_string(), ctx.db.get_relationship(&speaker.id, "user", "user")?));
    } else {
        // 自律会話・グループ: 自分以外の全参加ペルソナ
        for (pid, pname) in session.participant_ids.iter().zip(session.participant_names.iter()) {
            if pid != &speaker.id {
                partners.push((pname.clone(), ctx.db.get_relationship(&speaker.id, "persona", pid)?));
            }
        }
        // グループはユーザーも会話相手 (FR-31)
        if session.kind == "group" {
            partners.push(("ユーザー".to_string(), ctx.db.get_relationship(&speaker.id, "user", "user")?));
        }
    }
    let partner_infos: Vec<prompt::PartnerInfo> = partners
        .iter()
        .map(|(name, rel)| prompt::PartnerInfo { name, relationship: rel.as_ref() })
        .collect();
    let theme = if session.kind == "autonomous" { Some(session.theme.as_str()) } else { None };

    // v0.2: 現在ムードの言語化 (ADR-13, FR-24)。平常なら None。
    let mood_state = personality::current_mood(&ctx.db, &speaker.id, &settings, now_ms())?;
    let mood = prompt::mood_phrase(mood_state.value, &mood_state.label);
    // v0.2: 前回対話からの経過時間ラベル (ADR-11, FR-20)。自律会話では注入しない(相手はユーザーでないため)。
    let elapsed = if session.kind == "autonomous" {
        None
    } else {
        prompt::elapsed_label(speaker.last_talked_at, now_ms(), &settings)
    };

    let system = prompt::build_system(
        speaker,
        &traits,
        &partner_infos,
        &memories,
        theme,
        mood.as_deref(),
        elapsed.as_deref(),
    );
    let mut messages =
        prompt::assemble_messages(system, &history, &speaker.id, settings.context_chars.max(1000) as usize);
    // 話しかけ (FR-21): ペルソナから会話を切り出させる誘導。履歴が空でも発話できるようにする。
    if let Some(hint) = opening_hint {
        messages.push(crate::inference::ChatMessage::new("user", hint.to_string()));
    }

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
        // thinking対応モデルは思考にもトークンを使うため、少なすぎると本文が空になる
        max_tokens: Some(2048),
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

    // 完了したのに本文が空 (thinking がトークンを使い切った等) は生成失敗として扱う
    if !outcome.canceled && outcome.text.trim().is_empty() {
        ctx.sink.emit(
            "generation_failed",
            json!({
                "sessionId": session.id,
                "kind": "generation",
                "message": "応答が空でした。モデルの思考がトークン上限を使い切った可能性があります。もう一度送信してください",
            }),
        );
        return Err(AppError::Generation("応答が空でした".into()));
    }

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

/// 話者選択の結果 (ADR-15)
#[derive(Debug, PartialEq)]
pub enum SpeakerDecision {
    Speak(String), // persona_id
    None,          // 連鎖判定で「発話なし」
}

/// LLM の話者選択出力にコア補正を適用して次話者を決める (ADR-15)。
/// 補正: (2) 同一ペルソナの3連続禁止 (3) 未応答者優先 (4) 不一致はラウンドロビンにフォールバック。
/// allow_none=true(連鎖判定)のとき「発話なし」を返しうる。
pub fn decide_group_speaker(
    llm_name: &str,
    participants: &[(String, String)], // (id, name) 登録順
    history: &[Utterance],
    allow_none: bool,
) -> SpeakerDecision {
    let name = llm_name.trim();
    let none_markers = ["発話なし", "なし", "none", "誰も", "沈黙", "スキップ"];
    if allow_none && (name.is_empty() || none_markers.iter().any(|m| name.contains(m))) {
        return SpeakerDecision::None;
    }

    // 直近のペルソナ発話者列
    let persona_seq: Vec<&str> = history
        .iter()
        .filter(|u| u.speaker_kind == "persona")
        .map(|u| u.speaker_id.as_str())
        .collect();
    let last = persona_seq.last().copied();
    let mut consec = 0;
    for id in persona_seq.iter().rev() {
        if Some(*id) == last {
            consec += 1;
        } else {
            break;
        }
    }
    let responded: HashSet<&str> = persona_seq.iter().copied().collect();
    // 3連続目を禁止 (直前と同一で既に2連続なら不可)
    let is_bad = |id: &str| last == Some(id) && consec >= 2;

    // LLM 名をIDに解決 (完全一致→部分一致)
    let matched = participants
        .iter()
        .find(|(_, n)| n == name)
        .or_else(|| {
            if name.is_empty() {
                Option::None
            } else {
                participants.iter().find(|(_, n)| name.contains(n.as_str()) || n.contains(name))
            }
        })
        .map(|(id, _)| id.clone());

    // 採否: 有効な選択でなければフォールバック(未応答者優先→ラウンドロビン)
    if let Some(c) = &matched {
        if !is_bad(c) {
            return SpeakerDecision::Speak(c.clone());
        }
    }
    // 未応答者を登録順で優先 (3連続に該当しない者)
    if let Some((id, _)) = participants.iter().find(|(id, _)| !responded.contains(id.as_str()) && !is_bad(id)) {
        return SpeakerDecision::Speak(id.clone());
    }
    // 全員応答済み: 直前話者の次(ラウンドロビン)
    if let Some(last_id) = last {
        if let Some(pos) = participants.iter().position(|(id, _)| id == last_id) {
            return SpeakerDecision::Speak(participants[(pos + 1) % participants.len()].0.clone());
        }
    }
    SpeakerDecision::Speak(participants[0].0.clone())
}

/// グループチャットのユーザー発話受理と応答生成 (フロー7, FR-31〜34)
pub async fn send_group_message(
    ctx: &AppCtx,
    session_id: &str,
    text: &str,
    target_persona_id: Option<&str>,
) -> AppResult<()> {
    let session = ctx
        .db
        .get_session(session_id)?
        .filter(|s| s.status == "active" && s.kind == "group")
        .ok_or_else(|| AppError::Validation("進行中のグループセッションではありません".into()))?;

    // ユーザー発話を即時保存
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
    // 新しいユーザー発話が来たので連鎖の中断フラグを解除
    ctx.conv.clear_stop(session_id);

    let participants: Vec<(String, String)> = session
        .participant_ids
        .iter()
        .cloned()
        .zip(session.participant_names.iter().cloned())
        .collect();

    // 初回応答: 指名があればその人、なければ選択推論(発話なしは許さない)
    let first = if let Some(tid) = target_persona_id.filter(|t| participants.iter().any(|(id, _)| id == t)) {
        SpeakerDecision::Speak(tid.to_string())
    } else {
        select_group_speaker(ctx, &session, &participants, false).await
    };
    if let SpeakerDecision::Speak(pid) = first {
        respond_as(ctx, &session, &pid).await?;
    }

    // 連鎖発話 (FR-33): 上限まで「発話なし」込みの選択で続ける。中断フラグ(ユーザー割り込み)で停止。
    let settings = ctx.db.load_settings()?;
    let mut chained = 0;
    while chained < settings.chain_limit {
        if ctx.conv.is_stopped(session_id) {
            break; // ユーザー発話の割り込みを優先 (FR-33)
        }
        match select_group_speaker(ctx, &session, &participants, true).await {
            SpeakerDecision::Speak(pid) => {
                respond_as(ctx, &session, &pid).await?;
                chained += 1;
            }
            SpeakerDecision::None => break,
        }
    }
    Ok(())
}

/// 選択推論を実行してコア補正を適用する (ADR-15)
async fn select_group_speaker(
    ctx: &AppCtx,
    session: &Session,
    participants: &[(String, String)],
    allow_none: bool,
) -> SpeakerDecision {
    ctx.sink.emit("speaker_selecting", json!({ "sessionId": session.id }));
    let history = ctx.db.utterances_of(&session.id).unwrap_or_default();
    let settings = ctx.db.load_settings().unwrap_or_default();
    let responded: HashSet<&str> = history
        .iter()
        .filter(|u| u.speaker_kind == "persona")
        .map(|u| u.speaker_id.as_str())
        .collect();
    let prompt = prompt::build_speaker_selection(&history, participants, &responded, allow_none);
    let req = ChatRequest {
        model: settings.chat_model.clone(),
        messages: vec![crate::inference::ChatMessage::new("user", prompt)],
        temperature: 0.2,
        max_tokens: Some(32),
    };
    // 失敗時は空文字を渡してコア補正のフォールバックに委ねる
    let llm_name = ctx.inference.chat_once(req).await.unwrap_or_default();
    decide_group_speaker(&llm_name, participants, &history, allow_none)
}

/// 指定ペルソナとして1発話生成する (グループ)
async fn respond_as(ctx: &AppCtx, session: &Session, persona_id: &str) -> AppResult<()> {
    let persona = ctx
        .db
        .get_persona(persona_id)?
        .ok_or_else(|| AppError::NotFound("参加ペルソナが見つかりません".into()))?;
    generate_reply(ctx, session, &persona, None).await?;
    Ok(())
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

    // v0.2 停滞検出 (ADR-16, FR-35): 同一話者の直近発話との自己類似度を追う
    let mut last_emb: HashMap<String, Vec<f32>> = HashMap::new();
    let mut self_sims: Vec<f32> = Vec::new();
    let mut topic_shifts = 0i64;
    let stag_th = settings.stagnation_sim as f32;

    for turn in 0..limit {
        // FR-14: 停止フラグは次の発話生成前に検査する
        if ctx.conv.is_stopped(session_id) {
            break;
        }
        // ADR-08: 発話順は参加者の登録順で巡回する (ラウンドロビン)。FR-19
        let speaker = &personas[(turn as usize) % personas.len()];
        match generate_reply(ctx, &session, speaker, None).await {
            Ok(u) => {
                consecutive_failures = 0;
                // FR-35: 発話の埋め込みを計算し、同一話者の直近発話との類似度で停滞を判定
                if !settings.embed_model.is_empty() {
                    if let Ok(mut embs) = ctx.inference.embed(&settings.embed_model, &[u.content.clone()]).await {
                        if let Some(emb) = embs.pop() {
                            if let Some(prev) = last_emb.get(&u.speaker_id) {
                                self_sims.push(memory::cosine(&emb, prev));
                            }
                            last_emb.insert(u.speaker_id.clone(), emb);
                        }
                    }
                    if topic_shifts < settings.topic_shift_limit
                        && stagnation_reached(&self_sims, stag_th, settings.stagnation_streak)
                    {
                        insert_topic_shift(ctx, &session).await;
                        topic_shifts += 1;
                        self_sims.clear(); // 転換直後は再判定をリセット
                    }
                }
            }
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

/// 直近ターンの自己類似度列から停滞を判定する (ADR-16, FR-35)。
/// 末尾 streak 個がすべて閾値以上なら停滞。
pub fn stagnation_reached(self_sims: &[f32], threshold: f32, streak: i64) -> bool {
    let n = streak.max(1) as usize;
    self_sims.len() >= n && self_sims[self_sims.len() - n..].iter().all(|&s| s >= threshold)
}

/// 話題転換の司会システム発話を挿入する (FR-35)。失敗時は何もしない。
async fn insert_topic_shift(ctx: &AppCtx, session: &Session) {
    let settings = ctx.db.load_settings().unwrap_or_default();
    let history = ctx.db.utterances_of(&session.id).unwrap_or_default();
    let req = ChatRequest {
        model: settings.chat_model.clone(),
        messages: vec![crate::inference::ChatMessage::new(
            "user",
            prompt::build_topic_shift(&session.theme, &history),
        )],
        temperature: 0.9,
        max_tokens: Some(128),
    };
    let text = match ctx.inference.chat_once(req).await {
        Ok(t) if !t.trim().is_empty() => t,
        _ => return,
    };
    let u = Utterance {
        id: new_id(),
        session_id: session.id.clone(),
        speaker_kind: "system".into(),
        speaker_id: "system".into(),
        speaker_name: "司会".into(),
        content: text,
        state: "complete".into(),
        created_at: now_ms(),
    };
    // 通常の発話イベントとして流し、フロントに司会発話として表示させる
    ctx.sink.emit(
        "utterance_started",
        json!({ "sessionId": session.id, "utteranceId": u.id, "speakerId": "system", "speakerName": "司会" }),
    );
    ctx.sink.emit(
        "utterance_delta",
        json!({ "sessionId": session.id, "utteranceId": u.id, "delta": u.content }),
    );
    if ctx.db.insert_utterance(&u).is_ok() {
        ctx.sink.emit(
            "utterance_completed",
            json!({ "sessionId": session.id, "utteranceId": u.id, "state": "complete" }),
        );
    }
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

    /// thinking対応モデル対策: 完了したのに本文が空の応答は失敗として扱い、空発話を残さない
    #[tokio::test]
    async fn empty_completion_treated_as_failure() {
        let env = test_ctx(MockInference::with_replies(&[""]));
        let a = add_persona(&env.ctx, "アリス");
        let s = start_session(&env.ctx, "user_dialogue", &[a.id.clone()], "").unwrap();

        let err = send_user_message(&env.ctx, &s.id, "やあ").await.unwrap_err();
        assert_eq!(err.kind(), "generation");
        assert!(env.sink.names().contains(&"generation_failed".to_string()));
        // ユーザー発話のみ保存され、空のペルソナ発話は残らない
        let utts = env.ctx.db.utterances_of(&s.id).unwrap();
        assert_eq!(utts.len(), 1);
        assert_eq!(utts[0].speaker_kind, "user");
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

    /// FR-19 受け入れ基準: 3体を指定して開始すると全員の発話が1回以上現れる
    #[tokio::test]
    async fn three_personas_round_robin_fr19() {
        let env = test_ctx(MockInference::with_replies(&[
            "発話1", "発話2", "発話3", "発話4", "発話5", "発話6",
        ]));
        let mut settings = env.ctx.db.load_settings().unwrap();
        settings.auto_turn_limit = 6;
        env.ctx.db.save_settings(&settings).unwrap();

        let a = add_persona(&env.ctx, "アリス");
        let b = add_persona(&env.ctx, "ボブ");
        let c = add_persona(&env.ctx, "キャロル");
        let s = start_session(
            &env.ctx,
            "autonomous",
            &[a.id.clone(), b.id.clone(), c.id.clone()],
            "三人の話題",
        )
        .unwrap();
        run_autonomous(&env.ctx, &s.id).await.unwrap();

        let utts = env.ctx.db.utterances_of(&s.id).unwrap();
        assert_eq!(utts.len(), 6);
        // ラウンドロビン (ADR-08): 登録順で巡回
        let order: Vec<&str> = utts.iter().map(|u| u.speaker_id.as_str()).collect();
        assert_eq!(order, vec![&a.id, &b.id, &c.id, &a.id, &b.id, &c.id]
            .iter().map(|s| s.as_str()).collect::<Vec<_>>());
        // 全員が1回以上発話 (FR-19 受け入れ基準)
        for pid in [&a.id, &b.id, &c.id] {
            assert!(utts.iter().any(|u| &u.speaker_id == pid));
        }
        assert_eq!(env.ctx.db.get_session(&s.id).unwrap().unwrap().status, "ended");
    }

    #[tokio::test]
    async fn autonomous_participant_count_validated_fr19() {
        let env = test_ctx(MockInference::default());
        let personas: Vec<String> =
            (0..7).map(|i| add_persona(&env.ctx, &format!("P{i}")).id).collect();
        // 1体は不可
        assert_eq!(
            start_session(&env.ctx, "autonomous", &personas[..1], "").unwrap_err().kind(),
            "validation"
        );
        // 上限(6体)超過は不可
        assert_eq!(
            start_session(&env.ctx, "autonomous", &personas[..7], "").unwrap_err().kind(),
            "validation"
        );
        // 6体は開始できる
        start_session(&env.ctx, "autonomous", &personas[..6], "").unwrap();
    }

    #[tokio::test]
    async fn greeting_generated_and_recorded_fr21() {
        // FR-21: 話しかけが有効ならユーザー入力なしにペルソナが発話し、last_greeting_at が記録される
        let env = test_ctx(MockInference::with_replies(&["やあ、久しぶり!元気だった?"]));
        let a = add_persona(&env.ctx, "アリス");
        let s = start_session(&env.ctx, "user_dialogue", &[a.id.clone()], "").unwrap();

        let did = request_greeting(&env.ctx, &s.id).await.unwrap();
        assert!(did);
        let utts = env.ctx.db.utterances_of(&s.id).unwrap();
        assert_eq!(utts.len(), 1);
        assert_eq!(utts[0].speaker_kind, "persona");
        assert_eq!(utts[0].content, "やあ、久しぶり!元気だった?");
        assert!(env.ctx.db.get_last_greeting_at(&a.id).unwrap().is_some());
    }

    #[tokio::test]
    async fn greeting_disabled_returns_false_fr21() {
        // FR-21: 設定で無効なら発話しない
        let env = test_ctx(MockInference::with_replies(&["話しかけ"]));
        let mut s = env.ctx.db.load_settings().unwrap();
        s.greeting_enabled = false;
        env.ctx.db.save_settings(&s).unwrap();
        let a = add_persona(&env.ctx, "アリス");
        let sess = start_session(&env.ctx, "user_dialogue", &[a.id.clone()], "").unwrap();

        assert!(!request_greeting(&env.ctx, &sess.id).await.unwrap());
        assert!(env.ctx.db.utterances_of(&sess.id).unwrap().is_empty());
    }

    #[tokio::test]
    async fn greeting_too_soon_skipped_ec14() {
        // EC-14: 前回の話しかけから間隔内なら再生成しない
        let env = test_ctx(MockInference::with_replies(&["話しかけ1", "話しかけ2"]));
        let a = add_persona(&env.ctx, "アリス");
        // 直前に話しかけたことにする (間隔既定60分内)
        env.ctx.db.set_last_greeting_at(&a.id, now_ms()).unwrap();
        let sess = start_session(&env.ctx, "user_dialogue", &[a.id.clone()], "").unwrap();

        assert!(!request_greeting(&env.ctx, &sess.id).await.unwrap());
        assert!(env.ctx.db.utterances_of(&sess.id).unwrap().is_empty());
    }

    #[tokio::test]
    async fn greeting_connection_failure_is_silent_ec13() {
        // EC-13: 接続失敗時はエラーを出さず false (無通知の縮退)
        let env = test_ctx(MockInference::default()); // 応答キュー空 → 生成失敗
        let a = add_persona(&env.ctx, "アリス");
        let sess = start_session(&env.ctx, "user_dialogue", &[a.id.clone()], "").unwrap();

        // Err にならず false
        assert!(!request_greeting(&env.ctx, &sess.id).await.unwrap());
        // 発話は保存されない
        assert!(env.ctx.db.utterances_of(&sess.id).unwrap().is_empty());
    }

    fn utt(kind: &str, id: &str, name: &str) -> Utterance {
        Utterance {
            id: new_id(), session_id: "s".into(),
            speaker_kind: kind.into(), speaker_id: id.into(), speaker_name: name.into(),
            content: "…".into(), state: "complete".into(), created_at: now_ms(),
        }
    }

    #[test]
    fn decide_speaker_corrections() {
        let parts = vec![("a".to_string(), "アリス".to_string()), ("b".to_string(), "ボブ".to_string())];
        // 完全一致
        assert_eq!(
            decide_group_speaker("アリス", &parts, &[], false),
            SpeakerDecision::Speak("a".into())
        );
        // ゴミ出力 + アリス応答済み → 未応答のボブへフォールバック
        let hist = vec![utt("persona", "a", "アリス")];
        assert_eq!(
            decide_group_speaker("???", &parts, &hist, false),
            SpeakerDecision::Speak("b".into())
        );
        // 3連続禁止: アリスが2連続 → アリス指定でも次(ボブ)へ
        let hist2 = vec![utt("persona", "a", "アリス"), utt("persona", "a", "アリス")];
        assert_eq!(
            decide_group_speaker("アリス", &parts, &hist2, false),
            SpeakerDecision::Speak("b".into())
        );
        // 連鎖判定で「発話なし」
        assert_eq!(decide_group_speaker("発話なし", &parts, &hist, true), SpeakerDecision::None);
        // allow_none=false では「発話なし」は無効入力扱いでフォールバック(None にしない)
        assert!(matches!(
            decide_group_speaker("発話なし", &parts, &[], false),
            SpeakerDecision::Speak(_)
        ));
    }

    #[tokio::test]
    async fn group_selects_one_speaker_fr31() {
        // FR-31: ユーザー発話後、文脈に基づき選ばれた1体が応答する
        let env = test_ctx(MockInference::with_replies(&["アリス", "はい、なんでしょう?"]));
        let a = add_persona(&env.ctx, "アリス");
        let b = add_persona(&env.ctx, "ボブ");
        let s = start_session(&env.ctx, "group", &[a.id.clone(), b.id.clone()], "").unwrap();

        send_group_message(&env.ctx, &s.id, "みんな元気?", None).await.unwrap();

        let utts = env.ctx.db.utterances_of(&s.id).unwrap();
        // user + 選ばれた1体 (連鎖は応答キュー枯渇で終了)
        assert_eq!(utts.len(), 2);
        assert_eq!(utts[0].speaker_kind, "user");
        assert_eq!(utts[1].speaker_id, a.id); // 選択された「アリス」
        assert!(env.sink.names().contains(&"speaker_selecting".to_string()));
    }

    #[tokio::test]
    async fn group_nomination_fr32() {
        // FR-32: 指名したペルソナが応答する(選択推論は走らない)
        let env = test_ctx(MockInference::with_replies(&["ボブが答えます"]));
        let a = add_persona(&env.ctx, "アリス");
        let b = add_persona(&env.ctx, "ボブ");
        let s = start_session(&env.ctx, "group", &[a.id.clone(), b.id.clone()], "").unwrap();

        send_group_message(&env.ctx, &s.id, "ボブはどう思う?", Some(&b.id)).await.unwrap();

        let utts = env.ctx.db.utterances_of(&s.id).unwrap();
        assert_eq!(utts.len(), 2);
        assert_eq!(utts[1].speaker_id, b.id); // 指名どおりボブ
        assert_eq!(utts[1].content, "ボブが答えます");
    }

    #[tokio::test]
    async fn group_chain_capped_fr33() {
        // FR-33: 連鎖発話は上限(既定2)を超えない
        let env = test_ctx(MockInference::with_replies(&[
            "アリス", "初回応答", // 初回
            "ボブ", "連鎖1", // 連鎖1
            "アリス", "連鎖2", // 連鎖2
            "ボブ", "連鎖3", // これは上限で消費されないはず
        ]));
        let a = add_persona(&env.ctx, "アリス");
        let b = add_persona(&env.ctx, "ボブ");
        let s = start_session(&env.ctx, "group", &[a.id.clone(), b.id.clone()], "").unwrap();

        send_group_message(&env.ctx, &s.id, "話して", None).await.unwrap();

        let all = env.ctx.db.utterances_of(&s.id).unwrap();
        let n_persona = all.iter().filter(|u| u.speaker_kind == "persona").count();
        // 初回1 + 連鎖上限2 = 3体分。連鎖3は生成されない
        assert_eq!(n_persona, 3);
    }

    #[test]
    fn stagnation_reached_logic() {
        // FR-35: 末尾 streak 個が閾値以上なら停滞。誤検出しないこと。
        assert!(stagnation_reached(&[0.9, 0.86], 0.85, 2));
        assert!(!stagnation_reached(&[0.9], 0.85, 2)); // データ不足
        assert!(!stagnation_reached(&[0.9, 0.5], 0.85, 2)); // 直近が閾値未満 → 停滞でない
        assert!(!stagnation_reached(&[0.5, 0.6, 0.7], 0.85, 2)); // 全て閾値未満
    }

    #[tokio::test]
    async fn stagnation_inserts_topic_shift_fr35() {
        // FR-35: 停滞を検出したら司会システム発話で話題転換し、セッションは継続する
        // mock embed は全発話に同一ベクトルを返す → 自己類似度=1.0 で停滞が起きる
        let env = test_ctx(MockInference::with_replies(&[
            "発話1", "発話2", "発話3", "発話4",
            "ところで、旅行の話はどう?", // 話題転換1 (chat_once)
            "発話5", "発話6",
            "ところで、音楽はどう?", // 話題転換2
        ]));
        let mut settings = env.ctx.db.load_settings().unwrap();
        settings.auto_turn_limit = 6;
        settings.embed_model = "mock-embed".into(); // 停滞検出を有効化
        env.ctx.db.save_settings(&settings).unwrap();

        let a = add_persona(&env.ctx, "アリス");
        let b = add_persona(&env.ctx, "ボブ");
        let s = start_session(&env.ctx, "autonomous", &[a.id.clone(), b.id.clone()], "趣味").unwrap();
        run_autonomous(&env.ctx, &s.id).await.unwrap();

        let utts = env.ctx.db.utterances_of(&s.id).unwrap();
        let systems: Vec<&Utterance> = utts.iter().filter(|u| u.speaker_kind == "system").collect();
        assert_eq!(systems.len(), 2, "話題転換が2回挿入される");
        assert!(systems[0].content.contains("旅行"));
        assert_eq!(systems[0].speaker_name, "司会");
        // 通常の発話も6件生成され、セッションは終了する
        assert_eq!(utts.iter().filter(|u| u.speaker_kind == "persona").count(), 6);
        assert_eq!(env.ctx.db.get_session(&s.id).unwrap().unwrap().status, "ended");
    }

    #[tokio::test]
    async fn group_exclusion_ec19() {
        // EC-19: グループ参加中のペルソナは1対1を開始できない
        let env = test_ctx(MockInference::default());
        let a = add_persona(&env.ctx, "アリス");
        let b = add_persona(&env.ctx, "ボブ");
        let _g = start_session(&env.ctx, "group", &[a.id.clone(), b.id.clone()], "").unwrap();
        let err = start_session(&env.ctx, "user_dialogue", &[a.id.clone()], "").unwrap_err();
        assert_eq!(err.kind(), "busy");
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
