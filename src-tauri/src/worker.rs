use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::context::{AppCtx, Job};
use crate::error::AppResult;
use crate::inference::{ChatMessage, ChatRequest};
use crate::memory::{self, embedding_to_blob};
use crate:: models::*;
use crate::personality::{self, PartnerAssessment, TRAIT_KEYS};

const MAX_MEMORIES_PER_SESSION: usize = 10;
const MAX_MEMORY_CONTENT_CHARS: usize = 300;
const TRANSCRIPT_BUDGET_CHARS: usize = 6000;

// ---------- 頑健な JSON 抽出 (設計10章 R-3 対応・単体テスト対象) ----------

/// LLM出力からJSON値を取り出す。コードフェンスや前後の文章に埋まっていても拾う。
pub fn extract_json(text: &str) -> Option<Value> {
    let trimmed = text.trim();
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        return Some(v);
    }
    // 最初の '{' または '[' から括弧の対応を数えて切り出す (文字列リテラル内は無視)
    let start = trimmed.find(|c| c == '{' || c == '[')?;
    let chars: Vec<char> = trimmed.chars().collect();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    let mut char_start = None;
    let mut byte_start = 0usize;
    // char index → byte index を辿りながら走査
    let mut byte_pos = 0usize;
    for (i, &c) in chars.iter().enumerate() {
        if byte_pos == start && char_start.is_none() {
            char_start = Some(i);
            byte_start = byte_pos;
        }
        if char_start.is_some() {
            if in_string {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    in_string = false;
                }
            } else {
                match c {
                    '"' => in_string = true,
                    '{' | '[' => depth += 1,
                    '}' | ']' => {
                        depth -= 1;
                        if depth == 0 {
                            let end = byte_pos + c.len_utf8();
                            let candidate = &trimmed[byte_start..end];
                            return serde_json::from_str::<Value>(candidate).ok();
                        }
                    }
                    _ => {}
                }
            }
        }
        byte_pos += c.len_utf8();
    }
    None
}

// ---------- 記憶抽出 (FR-08) ----------

#[derive(Debug, Clone)]
pub struct ExtractedMemory {
    pub content: String,
    pub kind: String,
    pub importance: i64,
}

/// 抽出結果JSONを検証付きで ExtractedMemory 列に変換する
pub fn parse_extracted_memories(v: &Value) -> Vec<ExtractedMemory> {
    let Some(arr) = v.as_array() else { return vec![] };
    let mut out = Vec::new();
    for item in arr.iter().take(MAX_MEMORIES_PER_SESSION) {
        let Some(content) = item["content"].as_str() else { continue };
        let content: String = content.trim().chars().take(MAX_MEMORY_CONTENT_CHARS).collect();
        if content.is_empty() {
            continue;
        }
        let kind = match item["kind"].as_str() {
            Some(k @ ("fact" | "event" | "promise" | "impression")) => k.to_string(),
            _ => "fact".to_string(),
        };
        let importance = item["importance"].as_i64().unwrap_or(5).clamp(1, 10);
        out.push(ExtractedMemory { content, kind, importance });
    }
    out
}

fn extraction_prompt(persona_name: &str, transcript: &str) -> Vec<ChatMessage> {
    let system = format!(
        "あなたは会話ログを分析する係である。会話から「{persona_name}」が覚えておくべき事柄を抽出し、JSON配列のみを出力する。\n\
         各要素の形式: {{\"content\": \"記憶の内容(1〜2文、{persona_name}の視点で書く)\", \"kind\": \"fact|event|promise|impression\", \"importance\": 1から10の整数}}\n\
         - fact=相手について知った事実 / event=出来事 / promise=約束 / impression=抱いた感想\n\
         - 重要なものだけ最大{MAX_MEMORIES_PER_SESSION}件。なければ [] を出力\n\
         - JSON以外の文章を書かない"
    );
    vec![
        ChatMessage::new("system", system),
        ChatMessage::new("user", format!("会話ログ:\n{transcript}")),
    ]
}

// ---------- 人格評定 (FR-12) ----------

/// 評定JSONを PartnerAssessment + 性格デルタに変換する
pub fn parse_assessment(v: &Value) -> (Vec<(String, i64)>, PartnerAssessment) {
    let mut trait_deltas = Vec::new();
    if let Some(traits) = v["traits"].as_object() {
        for key in TRAIT_KEYS {
            if let Some(d) = traits.get(key).and_then(|x| x.as_i64()) {
                trait_deltas.push((key.to_string(), d));
            }
        }
    }
    let pa = PartnerAssessment {
        intimacy_delta: v["intimacyDelta"].as_i64().or(v["intimacy_delta"].as_i64()).unwrap_or(0),
        impression: v["impression"].as_str().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
    };
    (trait_deltas, pa)
}

fn assessment_prompt(
    persona: &Persona,
    traits: &[TraitValue],
    partner_name: &str,
    current_intimacy: i64,
    transcript: &str,
) -> Vec<ChatMessage> {
    let trait_lines: String = traits
        .iter()
        .map(|t| format!("{}: {}", t.key, t.value))
        .collect::<Vec<_>>()
        .join(", ");
    let system = format!(
        "あなたは会話ログを分析し、「{}」の心境の変化を評定する係である。JSONのみを出力する。\n\
         形式: {{\"traits\": {{\"sociability\": 整数, \"empathy\": 整数, \"caution\": 整数, \"assertiveness\": 整数, \"cheerfulness\": 整数}}, \"intimacy_delta\": 整数, \"impression\": \"相手({})への現在の印象を50字以内\"}}\n\
         - traits は各性格軸の変化量。-2〜2 の範囲。変化なしは 0\n\
         - intimacy_delta は相手への親密度の変化量。-5〜5 の範囲\n\
         - 現在の性格 (0-100): {}\n\
         - 相手への現在の親密度 (0-100): {}",
        persona.name, partner_name, trait_lines, current_intimacy
    );
    vec![
        ChatMessage::new("system", system),
        ChatMessage::new("user", format!("会話ログ:\n{transcript}")),
    ]
}

// ---------- セッション後処理 (設計7章フロー3) ----------

fn build_transcript(utterances: &[Utterance]) -> String {
    let mut lines: Vec<String> = utterances
        .iter()
        .filter(|u| !u.content.is_empty())
        .map(|u| format!("{}: {}", u.speaker_name, u.content))
        .collect();
    // 長すぎる場合は末尾(直近)を優先して収める
    let mut total = 0usize;
    let mut keep_from = lines.len();
    for (i, l) in lines.iter().enumerate().rev() {
        total += l.chars().count();
        if total > TRANSCRIPT_BUDGET_CHARS {
            break;
        }
        keep_from = i;
    }
    lines.drain(..keep_from);
    lines.join("\n")
}

async fn chat_json_with_retry(ctx: &AppCtx, model: &str, messages: Vec<ChatMessage>) -> Option<Value> {
    for attempt in 0..2 {
        let req = ChatRequest {
            model: model.to_string(),
            messages: messages.clone(),
            temperature: 0.2,
            max_tokens: Some(1024),
        };
        match ctx.inference.chat_once(req).await {
            Ok(text) => {
                if let Some(v) = extract_json(&text) {
                    return Some(v);
                }
                tracing::warn!("JSON解析に失敗 (試行{}回目)", attempt + 1);
            }
            Err(e) => {
                tracing::warn!("後処理の推論呼び出しに失敗 (試行{}回目): {e}", attempt + 1);
            }
        }
    }
    None
}

pub async fn postprocess_session(ctx: &AppCtx, session_id: &str) -> AppResult<()> {
    let Some(session) = ctx.db.get_session(session_id)? else {
        return Ok(());
    };
    if session.status == "processed" || session.status == "active" {
        return Ok(());
    }
    let settings = ctx.db.load_settings()?;
    if settings.chat_model.is_empty() {
        tracing::warn!("チャットモデル未設定のため後処理を保留: session={session_id}");
        return Ok(());
    }

    let utterances = ctx.db.utterances_of(session_id)?;
    let transcript = build_transcript(&utterances);
    let participants = ctx.db.participants_of(session_id)?;

    let mut total_memories = 0usize;
    let mut total_events = 0usize;
    let mut all_done = true;

    for (pid, pname, processed) in &participants {
        if *processed {
            continue;
        }
        let Some(persona) = ctx.db.get_persona(pid)? else {
            // 削除済みペルソナの後処理は不要
            ctx.db.mark_participant_processed(session_id, pid)?;
            continue;
        };
        // 発話のないセッションは記憶化せず処理済みにする
        if transcript.is_empty() {
            ctx.db.mark_participant_processed(session_id, pid)?;
            continue;
        }

        // 1. 記憶抽出 (失敗時は未処理のまま残し、次回起動時に再試行する)
        let Some(json_v) =
            chat_json_with_retry(ctx, &settings.chat_model, extraction_prompt(&persona.name, &transcript)).await
        else {
            tracing::error!("記憶抽出に失敗 (extract_failed): session={session_id} persona={pname}");
            all_done = false;
            continue;
        };
        let extracted = parse_extracted_memories(&json_v);

        // 2. 埋め込み計算 (失敗しても NULL のまま保存し、後で再計算 EC-02縮退)
        let embeddings: Vec<Option<Vec<u8>>> = if !settings.embed_model.is_empty() && !extracted.is_empty() {
            let texts: Vec<String> = extracted.iter().map(|m| m.content.clone()).collect();
            match ctx.inference.embed(&settings.embed_model, &texts).await {
                Ok(vecs) => vecs.into_iter().map(|v| Some(embedding_to_blob(&v))).collect(),
                Err(e) => {
                    tracing::warn!("記憶の埋め込みに失敗 (後で再計算): {e}");
                    extracted.iter().map(|_| None).collect()
                }
            }
        } else {
            extracted.iter().map(|_| None).collect()
        };

        for (m, emb) in extracted.iter().zip(embeddings.iter()) {
            ctx.db.insert_memory(
                &Memory {
                    id: new_id(),
                    persona_id: pid.clone(),
                    content: m.content.clone(),
                    kind: m.kind.clone(),
                    importance: m.importance,
                    has_embedding: emb.is_some(),
                    source_session_id: Some(session_id.to_string()),
                    created_at: now_ms(),
                    archived: false,
                    user_edited: false,
                },
                emb.as_deref(),
            )?;
            total_memories += 1;
        }

        // 3. 人格評定 → 4. クランプ適用 (FR-12: 上限はコード側で強制)
        // 相手一覧: 1対1=ユーザー、自律会話=自分以外の全参加者 (FR-19。名前スナップショットを使う EC-07)
        let partners: Vec<(String, String, String)> = if session.kind == "user_dialogue" {
            vec![("user".to_string(), "user".to_string(), "ユーザー".to_string())]
        } else {
            participants
                .iter()
                .filter(|(other_id, _, _)| other_id != pid)
                .map(|(oid, oname, _)| ("persona".to_string(), oid.clone(), oname.clone()))
                .collect()
        };
        let traits = ctx.db.traits_of(pid)?;
        // ADR-08: 性格軸デルタは相手ごとに重ねず、セッションあたり1回だけ適用する
        // (相手数分適用すると FR-12 の1セッション変化量上限を実質超えるため)
        let mut traits_applied = false;
        for (partner_kind, partner_id, partner_name) in &partners {
            let current_intimacy = ctx
                .db
                .get_relationship(pid, partner_kind, partner_id)?
                .map(|r| r.intimacy)
                .unwrap_or(personality::DEFAULT_INTIMACY);

            if let Some(assess_v) = chat_json_with_retry(
                ctx,
                &settings.chat_model,
                assessment_prompt(&persona, &traits, partner_name, current_intimacy, &transcript),
            )
            .await
            {
                let (trait_deltas, pa) = parse_assessment(&assess_v);
                if !traits_applied {
                    let ev1 = personality::apply_trait_deltas(
                        &ctx.db, pid, session_id, &trait_deltas, &settings, now_ms(),
                    )?;
                    total_events += ev1.len();
                    traits_applied = true;
                }
                let ev2 = personality::apply_relationship(
                    &ctx.db, pid, session_id, partner_kind, partner_id, partner_name, &pa, &settings, now_ms(),
                )?;
                total_events += ev2.len();
            } else {
                tracing::warn!("人格評定に失敗 (この相手の評定をスキップ): persona={pname} partner={partner_name}");
            }
        }

        ctx.db.mark_participant_processed(session_id, pid)?;

        // 記憶上限チェック (EC-06)
        let archived = memory::archive_overflow(&ctx.db, pid, &settings, now_ms())?;
        if archived > 0 {
            tracing::info!("記憶上限により {archived} 件をアーカイブ: persona={pname}");
        }
    }

    // 全参加者が処理済みなら確定 (設計5.2 status 遷移)
    let done = all_done && ctx.db.participants_of(session_id)?.iter().all(|(_, _, p)| *p);
    if done {
        ctx.db.set_session_status(session_id, "processed", None)?;
        ctx.sink.emit(
            "postprocess_completed",
            json!({
                "sessionId": session_id,
                "memoryCount": total_memories,
                "eventCount": total_events,
            }),
        );
        ctx.sink.emit(
            "session_status_changed",
            json!({ "sessionId": session_id, "status": "processed" }),
        );
    }
    Ok(())
}

/// 埋め込み未計算の記憶を再計算する (起動時リカバリ・FR-11 の編集後再計算)
pub async fn reembed_missing(ctx: &AppCtx) -> AppResult<()> {
    let settings = ctx.db.load_settings()?;
    if settings.embed_model.is_empty() {
        return Ok(());
    }
    let missing = ctx.db.memories_missing_embedding()?;
    for chunk in missing.chunks(16) {
        let texts: Vec<String> = chunk.iter().map(|(_, c)| c.clone()).collect();
        match ctx.inference.embed(&settings.embed_model, &texts).await {
            Ok(vecs) => {
                for ((id, _), v) in chunk.iter().zip(vecs.iter()) {
                    ctx.db.set_memory_embedding(id, &embedding_to_blob(v))?;
                }
            }
            Err(e) => {
                tracing::warn!("埋め込み再計算に失敗 (次回起動時に再試行): {e}");
                break;
            }
        }
    }
    Ok(())
}

/// 起動時リカバリ (設計7章フロー4, EC-03)
pub fn startup_recovery(ctx: &AppCtx) -> AppResult<()> {
    // 強制終了の痕跡 (active のまま残ったセッション) を ended に倒す
    for sid in ctx.db.sessions_by_status("active")? {
        tracing::info!("未終了セッションを回収: {sid}");
        ctx.db.set_session_status(&sid, "ended", Some(now_ms()))?;
    }
    // 後処理未完了のセッションをキューへ
    for sid in ctx.db.sessions_by_status("ended")? {
        let _ = ctx.worker_tx.send(Job::Postprocess(sid));
    }
    let _ = ctx.worker_tx.send(Job::Reembed);
    Ok(())
}

/// ジョブを直列に処理するワーカーループ (ADR-06)
pub async fn run_worker(ctx: AppCtx, mut rx: mpsc::UnboundedReceiver<Job>) {
    while let Some(job) = rx.recv().await {
        let result = match &job {
            Job::Postprocess(sid) => postprocess_session(&ctx, sid).await,
            Job::Reembed => reembed_missing(&ctx).await,
        };
        if let Err(e) = result {
            tracing::error!("バックグラウンド処理に失敗 ({job:?}): {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::{end_session, send_user_message, start_session};
    use crate::test_util::*;

    #[test]
    fn extract_json_direct_fenced_and_wrapped() {
        assert!(extract_json(r#"[{"a":1}]"#).is_some());
        assert!(extract_json("```json\n[{\"a\":1}]\n```").is_some());
        assert!(extract_json("結果は次のとおりです: {\"a\": {\"b\": [1,2]}} 以上").is_some());
        // 文字列内の括弧に惑わされない
        let v = extract_json(r#"前置き {"s": "閉じ括弧}を含む"} 後書き"#).unwrap();
        assert_eq!(v["s"], "閉じ括弧}を含む");
        assert!(extract_json("JSONはありません").is_none());
        assert!(extract_json("{壊れたJSON").is_none());
    }

    #[test]
    fn parse_memories_validates() {
        let v: Value = serde_json::from_str(
            r#"[
                {"content":"ユーザーの職業はエンジニア","kind":"fact","importance":7},
                {"content":"","kind":"fact","importance":5},
                {"content":"変な種別","kind":"unknown","importance":99},
                {"note":"contentなし"}
            ]"#,
        )
        .unwrap();
        let mems = parse_extracted_memories(&v);
        assert_eq!(mems.len(), 2); // 空contentとcontentなしは除外
        assert_eq!(mems[0].importance, 7);
        assert_eq!(mems[1].kind, "fact"); // 未知の kind は fact に落とす
        assert_eq!(mems[1].importance, 10); // 99 → 10 にクランプ
    }

    #[test]
    fn parse_assessment_reads_traits_and_partner() {
        let v: Value = serde_json::from_str(
            r#"{"traits":{"sociability":2,"empathy":-1,"unknown":5},"intimacy_delta":3,"impression":"楽しい人"}"#,
        )
        .unwrap();
        let (deltas, pa) = parse_assessment(&v);
        assert_eq!(deltas.len(), 2); // 未知の軸は無視
        assert_eq!(pa.intimacy_delta, 3);
        assert_eq!(pa.impression.as_deref(), Some("楽しい人"));
    }

    /// FR-08/12/15: 対話→終了→後処理で記憶・関係性・イベントが生まれる
    #[tokio::test]
    async fn postprocess_creates_memories_and_personality() {
        let env = test_ctx(MockInference::with_replies(&[
            "ペルソナの応答です",                                                       // 対話の応答
            r#"[{"content":"ユーザーの好物はカレーだ","kind":"fact","importance":7}]"#, // 記憶抽出
            r#"{"traits":{"sociability":2},"intimacy_delta":10,"impression":"優しい人"}"#, // 評定(+10は+5へクランプ)
        ]));
        let mut settings = env.ctx.db.load_settings().unwrap();
        settings.chat_model = "mock".into();
        settings.embed_model = "mock-embed".into();
        env.ctx.db.save_settings(&settings).unwrap();

        let a = add_persona(&env.ctx, "アリス");
        let s = start_session(&env.ctx, "user_dialogue", &[a.id.clone()], "").unwrap();
        send_user_message(&env.ctx, &s.id, "好物はカレーなんだ").await.unwrap();
        end_session(&env.ctx, &s.id).unwrap();

        postprocess_session(&env.ctx, &s.id).await.unwrap();

        // 記憶 (FR-08): 内容・出所・埋め込み
        let mems = env.ctx.db.memories_of(&a.id, false).unwrap();
        assert_eq!(mems.len(), 1);
        assert!(mems[0].content.contains("カレー"));
        assert_eq!(mems[0].source_session_id.as_deref(), Some(s.id.as_str()));
        assert!(mems[0].has_embedding);

        // 関係性 (FR-12): +10 は上限 +5 にクランプ
        let rel = env.ctx.db.get_relationship(&a.id, "user", "user").unwrap().unwrap();
        assert_eq!(rel.intimacy, personality::DEFAULT_INTIMACY + 5);
        assert_eq!(rel.impression_text, "優しい人");

        // 性格軸 +2 とイベント記録 (FR-13)
        let traits = env.ctx.db.traits_of(&a.id).unwrap();
        assert_eq!(traits.iter().find(|t| t.key == "sociability").unwrap().value, 52);
        assert!(!env.ctx.db.personality_events_of(&a.id).unwrap().is_empty());

        // セッション確定と通知
        assert_eq!(env.ctx.db.get_session(&s.id).unwrap().unwrap().status, "processed");
        assert!(env.sink.names().contains(&"postprocess_completed".to_string()));
    }

    /// FR-15: 自律会話は参加者それぞれが記憶・関係性を得る
    #[tokio::test]
    async fn postprocess_autonomous_both_participants() {
        let env = test_ctx(MockInference::with_replies(&[
            "アリスの発話",
            "ボブの発話",
            // アリスの後処理
            r#"[{"content":"ボブは釣りが趣味だ","kind":"fact","importance":6}]"#,
            r#"{"traits":{},"intimacy_delta":2,"impression":"穏やかな人"}"#,
            // ボブの後処理
            r#"[{"content":"アリスと趣味の話をした","kind":"event","importance":4}]"#,
            r#"{"traits":{},"intimacy_delta":3,"impression":"明るい人"}"#,
        ]));
        let mut settings = env.ctx.db.load_settings().unwrap();
        settings.chat_model = "mock".into();
        settings.auto_turn_limit = 2;
        env.ctx.db.save_settings(&settings).unwrap();

        let a = add_persona(&env.ctx, "アリス");
        let b = add_persona(&env.ctx, "ボブ");
        let s = start_session(&env.ctx, "autonomous", &[a.id.clone(), b.id.clone()], "趣味").unwrap();
        crate::conversation::run_autonomous(&env.ctx, &s.id).await.unwrap();
        postprocess_session(&env.ctx, &s.id).await.unwrap();

        assert_eq!(env.ctx.db.memories_of(&a.id, false).unwrap().len(), 1);
        assert_eq!(env.ctx.db.memories_of(&b.id, false).unwrap().len(), 1);
        // 相互の関係性が名前付きで記録される
        let rel_ab = env.ctx.db.get_relationship(&a.id, "persona", &b.id).unwrap().unwrap();
        assert_eq!(rel_ab.target_name, "ボブ");
        let rel_ba = env.ctx.db.get_relationship(&b.id, "persona", &a.id).unwrap().unwrap();
        assert_eq!(rel_ba.intimacy, personality::DEFAULT_INTIMACY + 3);
        assert_eq!(env.ctx.db.get_session(&s.id).unwrap().unwrap().status, "processed");
    }

    /// FR-19: 3体の自律会話は各参加者が「他の2体それぞれ」との関係性を得る。
    /// 性格軸はセッションあたり1回のみ適用され FR-12 の上限内に収まる。
    #[tokio::test]
    async fn postprocess_three_participants() {
        // 各ペルソナ: 抽出1回 + 評定2回 (相手2体分) = 3体で9応答
        let mut replies: Vec<String> = Vec::new();
        for _ in 0..3 {
            replies.push(r#"[{"content":"三人で旅行の計画を立てた","kind":"event","importance":5}]"#.into());
            // 両方の評定が sociability +2 を返す → 2回適用なら +4 になってしまうケース
            replies.push(r#"{"traits":{"sociability":2},"intimacy_delta":3,"impression":"良い人"}"#.into());
            replies.push(r#"{"traits":{"sociability":2},"intimacy_delta":4,"impression":"面白い人"}"#.into());
        }
        let reply_refs: Vec<&str> = replies.iter().map(|s| s.as_str()).collect();
        let env = test_ctx(MockInference::with_replies(&reply_refs));
        let mut settings = env.ctx.db.load_settings().unwrap();
        settings.chat_model = "mock".into();
        env.ctx.db.save_settings(&settings).unwrap();

        let a = add_persona(&env.ctx, "アリス");
        let b = add_persona(&env.ctx, "ボブ");
        let c = add_persona(&env.ctx, "キャロル");
        let s = start_session(
            &env.ctx,
            "autonomous",
            &[a.id.clone(), b.id.clone(), c.id.clone()],
            "旅行",
        )
        .unwrap();
        env.ctx.db.insert_utterance(&Utterance {
            id: new_id(), session_id: s.id.clone(), speaker_kind: "persona".into(),
            speaker_id: a.id.clone(), speaker_name: "アリス".into(),
            content: "旅行に行きましょう".into(), state: "complete".into(), created_at: now_ms(),
        }).unwrap();
        end_session(&env.ctx, &s.id).unwrap();

        postprocess_session(&env.ctx, &s.id).await.unwrap();

        // 各参加者が他の2体との関係性を持つ
        for (me, others) in [(&a, [&b, &c]), (&b, [&a, &c]), (&c, [&a, &b])] {
            for other in others {
                let rel = env.ctx.db.get_relationship(&me.id, "persona", &other.id).unwrap();
                assert!(rel.is_some(), "{}→{} の関係性がない", me.name, other.name);
            }
            // FR-12: 性格軸は +2(上限) まで。2相手分の +2 が重なって +4 にならない
            let soc = env.ctx.db.traits_of(&me.id).unwrap()
                .iter().find(|t| t.key == "sociability").unwrap().value;
            assert_eq!(soc, 52, "{} の性格変化がセッション上限を超えている", me.name);
        }
        assert_eq!(env.ctx.db.get_session(&s.id).unwrap().unwrap().status, "processed");
    }

    /// 抽出JSONが1回壊れても再試行で成功する (設計フロー3)
    #[tokio::test]
    async fn postprocess_retries_broken_json() {
        let env = test_ctx(MockInference::with_replies(&[
            "壊れた出力です。JSONはありません",
            r#"[{"content":"再試行で取れた記憶","kind":"fact","importance":5}]"#,
            r#"{"traits":{},"intimacy_delta":0}"#,
        ]));
        let mut settings = env.ctx.db.load_settings().unwrap();
        settings.chat_model = "mock".into();
        env.ctx.db.save_settings(&settings).unwrap();

        let a = add_persona(&env.ctx, "アリス");
        let s = start_session(&env.ctx, "user_dialogue", &[a.id.clone()], "").unwrap();
        env.ctx.db.insert_utterance(&Utterance {
            id: new_id(), session_id: s.id.clone(), speaker_kind: "user".into(),
            speaker_id: "user".into(), speaker_name: "ユーザー".into(),
            content: "こんにちは".into(), state: "complete".into(), created_at: now_ms(),
        }).unwrap();
        end_session(&env.ctx, &s.id).unwrap();

        postprocess_session(&env.ctx, &s.id).await.unwrap();
        let mems = env.ctx.db.memories_of(&a.id, false).unwrap();
        assert_eq!(mems.len(), 1);
        assert_eq!(mems[0].content, "再試行で取れた記憶");
    }

    /// EC-03: active のまま残ったセッションが起動時に回収される
    #[tokio::test]
    async fn recovery_collects_stale_sessions() {
        let mut env = test_ctx(MockInference::default());
        let a = add_persona(&env.ctx, "アリス");
        let s = start_session(&env.ctx, "user_dialogue", &[a.id.clone()], "").unwrap();
        env.ctx.db.insert_utterance(&Utterance {
            id: new_id(), session_id: s.id.clone(), speaker_kind: "user".into(),
            speaker_id: "user".into(), speaker_name: "ユーザー".into(),
            content: "強制終了前の発話".into(), state: "complete".into(), created_at: now_ms(),
        }).unwrap();
        // end_session を呼ばない = 強制終了を模す

        startup_recovery(&env.ctx).unwrap();

        assert_eq!(env.ctx.db.get_session(&s.id).unwrap().unwrap().status, "ended");
        assert_eq!(env.job_rx.try_recv().unwrap(), Job::Postprocess(s.id.clone()));
        assert_eq!(env.job_rx.try_recv().unwrap(), Job::Reembed);
    }

    /// モデル未設定なら後処理は保留され、クラッシュしない
    #[tokio::test]
    async fn postprocess_deferred_without_model() {
        let env = test_ctx(MockInference::default());
        let a = add_persona(&env.ctx, "アリス");
        let s = start_session(&env.ctx, "user_dialogue", &[a.id.clone()], "").unwrap();
        end_session(&env.ctx, &s.id).unwrap();
        postprocess_session(&env.ctx, &s.id).await.unwrap();
        assert_eq!(env.ctx.db.get_session(&s.id).unwrap().unwrap().status, "ended"); // 保留
    }
}
