//! Ollama 実機統合テスト (S11)。推論エンジン起動時のみ実行する:
//! `cargo test --lib real_ollama -- --ignored --nocapture`

use std::sync::Arc;
use std::time::Instant;

use crate::context::AppCtx;
use crate::conversation::{
    end_session, request_greeting, send_group_message, send_user_message, start_session,
    ConversationManager,
};
use crate::db::Db;
use crate::inference::HttpInference;
use crate::models::{new_id, now_ms, Persona, TraitValue};
use crate::test_util::CollectSink;
use crate::worker::postprocess_session;

const ENDPOINT: &str = "http://127.0.0.1:11434";
const CHAT_MODEL: &str = "gpt-oss:20b";
const EMBED_MODEL: &str = "nomic-embed-text";

fn real_ctx() -> (tempfile::TempDir, AppCtx, Arc<CollectSink>) {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Db::open(&dir.path().join("integration.db")).unwrap());
    let mut settings = db.load_settings().unwrap();
    settings.chat_model = CHAT_MODEL.into();
    settings.embed_model = EMBED_MODEL.into();
    db.save_settings(&settings).unwrap();
    let sink = Arc::new(CollectSink::default());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let ctx = AppCtx {
        db,
        inference: Arc::new(HttpInference::new(ENDPOINT.into())),
        sink: sink.clone(),
        conv: Arc::new(ConversationManager::default()),
        worker_tx: tx,
    };
    (dir, ctx, sink)
}

/// FR-05/08/09/17 の実機確認: 対話→記憶生成→新セッションで想起
#[tokio::test]
#[ignore = "Ollama 実機が必要"]
async fn real_ollama_end_to_end_memory_recall() {
    let (_dir, ctx, sink) = real_ctx();

    // 疎通確認 (FR-16)
    let models = ctx.inference.list_models().await.expect("Ollamaに接続できません");
    assert!(models.iter().any(|m| m == CHAT_MODEL), "チャットモデル {CHAT_MODEL} が未導入: {models:?}");
    assert!(models.iter().any(|m| m.starts_with("nomic-embed-text")), "埋め込みモデル未導入: {models:?}");

    // ペルソナ作成
    let p = Persona {
        id: new_id(),
        name: "アリス".into(),
        description: "明るく好奇心旺盛な聞き上手".into(),
        speech_style: "です・ます調の丁寧な話し方".into(),
        values_text: "相手の話を大切にする".into(),
        self_intro: "アリスです。よろしくお願いします".into(),
        created_at: now_ms(),
        last_talked_at: None,
    };
    ctx.db
        .create_persona(&p, &[TraitValue { key: "sociability".into(), value: 70 }])
        .unwrap();

    // セッション1: 好物を伝える (FR-05)
    let s1 = start_session(&ctx, "user_dialogue", &[p.id.clone()], "").unwrap();
    let t0 = Instant::now();
    send_user_message(&ctx, &s1.id, "はじめまして。私の好物はカレーライスです。覚えておいてくださいね")
        .await
        .expect("応答生成に失敗");
    let elapsed = t0.elapsed();

    let utts = ctx.db.utterances_of(&s1.id).unwrap();
    assert_eq!(utts.len(), 2);
    assert!(!utts[1].content.is_empty(), "応答が空");
    println!("[NFR-01計測] 応答生成の全体時間: {:.1}秒", elapsed.as_secs_f32());
    println!("[応答1] {}", utts[1].content);

    // ストリーミングイベントが出ている (FR-05 逐次表示)
    let names = sink.names();
    assert!(names.iter().filter(|n| *n == "utterance_delta").count() >= 2, "deltaイベントが少なすぎる");

    // セッション終了 → 後処理 (FR-08: 記憶生成)
    end_session(&ctx, &s1.id).unwrap();
    postprocess_session(&ctx, &s1.id).await.expect("後処理に失敗");

    let memories = ctx.db.memories_of(&p.id, false).unwrap();
    assert!(!memories.is_empty(), "記憶が生成されていない");
    for m in &memories {
        println!("[記憶] ({}/{}) {}", m.kind, m.importance, m.content);
    }
    assert!(
        memories.iter().any(|m| m.content.contains("カレー")),
        "好物の記憶が抽出されていない: {memories:?}"
    );
    assert!(memories.iter().all(|m| m.has_embedding), "埋め込みが計算されていない");
    assert_eq!(ctx.db.get_session(&s1.id).unwrap().unwrap().status, "processed");

    // 関係性が更新されている (FR-12)
    let rel = ctx.db.get_relationship(&p.id, "user", "user").unwrap();
    println!("[関係性] {rel:?}");

    // セッション2 (再起動相当): 記憶からの想起 (FR-09)
    let s2 = start_session(&ctx, "user_dialogue", &[p.id.clone()], "").unwrap();
    send_user_message(&ctx, &s2.id, "私の好物が何だったか、覚えていますか?")
        .await
        .expect("応答生成に失敗");
    let utts2 = ctx.db.utterances_of(&s2.id).unwrap();
    let reply = &utts2[1].content;
    println!("[応答2] {reply}");
    assert!(reply.contains("カレー"), "記憶が想起されていない: {reply}");
}

/// FR-14/15 の実機確認: 自律会話と両者の記憶形成
#[tokio::test]
#[ignore = "Ollama 実機が必要"]
async fn real_ollama_autonomous_conversation() {
    let (_dir, ctx, _sink) = real_ctx();
    let mut settings = ctx.db.load_settings().unwrap();
    settings.auto_turn_limit = 4;
    ctx.db.save_settings(&settings).unwrap();

    let mk = |name: &str, desc: &str, style: &str| Persona {
        id: new_id(),
        name: name.into(),
        description: desc.into(),
        speech_style: style.into(),
        values_text: String::new(),
        self_intro: String::new(),
        created_at: now_ms(),
        last_talked_at: None,
    };
    let a = mk("アリス", "明るく社交的。趣味は料理", "です・ます調");
    let b = mk("ボブ", "物静かで理屈っぽい。趣味は天体観測", "ぶっきらぼうな短い話し方");
    ctx.db.create_persona(&a, &[]).unwrap();
    ctx.db.create_persona(&b, &[]).unwrap();

    let s = start_session(&ctx, "autonomous", &[a.id.clone(), b.id.clone()], "お互いの趣味について").unwrap();
    crate::conversation::run_autonomous(&ctx, &s.id).await.expect("自律会話に失敗");

    let utts = ctx.db.utterances_of(&s.id).unwrap();
    println!("[自律会話 {}ターン]", utts.len());
    for u in &utts {
        println!("  {}: {}", u.speaker_name, u.content.chars().take(60).collect::<String>());
    }
    assert!(utts.len() >= 2, "自律会話が進んでいない");
    // 交互発話 (FR-14)
    assert_eq!(utts[0].speaker_id, a.id);
    assert_eq!(utts[1].speaker_id, b.id);

    postprocess_session(&ctx, &s.id).await.expect("後処理に失敗");
    // FR-15: 両参加者に記憶が生まれる
    let mem_a = ctx.db.memories_of(&a.id, false).unwrap();
    let mem_b = ctx.db.memories_of(&b.id, false).unwrap();
    println!("[記憶] アリス{}件 / ボブ{}件", mem_a.len(), mem_b.len());
    assert!(!mem_a.is_empty() && !mem_b.is_empty(), "両参加者の記憶が生成されていない");
}

/// v0.2 の実機確認: 話しかけ(FR-21) → ムード・日記(FR-24/26)
#[tokio::test]
#[ignore = "Ollama 実機が必要"]
async fn real_ollama_greeting_mood_diary_v02() {
    let (_dir, ctx, sink) = real_ctx();
    let p = Persona {
        id: new_id(),
        name: "アリス".into(),
        description: "明るく好奇心旺盛".into(),
        speech_style: "です・ます調".into(),
        values_text: "誠実さ".into(),
        self_intro: "アリスです".into(),
        created_at: now_ms(),
        last_talked_at: None,
    };
    ctx.db.create_persona(&p, &[TraitValue { key: "sociability".into(), value: 70 }]).unwrap();

    // FR-21: 画面を開いた想定でセッション開始 → 話しかけ
    let s1 = start_session(&ctx, "user_dialogue", &[p.id.clone()], "").unwrap();
    let greeted = request_greeting(&ctx, &s1.id).await.expect("話しかけ生成に失敗");
    assert!(greeted, "話しかけが生成されなかった");
    let utts = ctx.db.utterances_of(&s1.id).unwrap();
    assert_eq!(utts.len(), 1);
    assert_eq!(utts[0].speaker_kind, "persona");
    println!("[話しかけ] {}", utts[0].content);
    assert!(sink.names().iter().any(|n| n == "utterance_delta"));

    // ユーザーが強く褒める → ムードが上がる想定
    send_user_message(&ctx, &s1.id, "アリスさんの明るさに毎日救われています。本当にありがとう!")
        .await
        .expect("応答生成に失敗");
    end_session(&ctx, &s1.id).unwrap();
    postprocess_session(&ctx, &s1.id).await.expect("後処理に失敗");

    // FR-24: ムードが評定され記録される
    let mood = ctx.db.get_mood_raw(&p.id).unwrap();
    println!("[ムード] value={} label={}", mood.0, mood.1);
    let mood_events = ctx.db.mood_events_of(&p.id).unwrap();
    println!("[ムード変化] {}件", mood_events.len());

    // FR-26: 当日分の日記が生成される
    let diaries = ctx.db.list_diaries(&p.id).unwrap();
    assert!(!diaries.is_empty(), "日記が生成されていない");
    println!("[日記 {}] {}", diaries[0].date, diaries[0].content);
}

/// v0.2 の実機確認: グループチャットで文脈に応じた1体が応答する (FR-31)
#[tokio::test]
#[ignore = "Ollama 実機が必要"]
async fn real_ollama_group_chat_v02() {
    let (_dir, ctx, _sink) = real_ctx();
    let mk = |name: &str, desc: &str| Persona {
        id: new_id(), name: name.into(), description: desc.into(),
        speech_style: "です・ます調".into(), values_text: String::new(),
        self_intro: String::new(), created_at: now_ms(), last_talked_at: None,
    };
    let a = mk("アリス", "料理が得意な明るい人");
    let b = mk("ボブ", "天体観測が趣味の物静かな人");
    ctx.db.create_persona(&a, &[]).unwrap();
    ctx.db.create_persona(&b, &[]).unwrap();

    let s = start_session(&ctx, "group", &[a.id.clone(), b.id.clone()], "").unwrap();
    send_group_message(&ctx, &s.id, "こんにちは、二人とも!今日は星がきれいですね", None)
        .await
        .expect("グループ応答に失敗");

    let utts = ctx.db.utterances_of(&s.id).unwrap();
    for u in &utts {
        println!("  {}: {}", u.speaker_name, u.content.chars().take(50).collect::<String>());
    }
    // ユーザー発話 + 少なくとも1体の応答
    assert!(utts.iter().any(|u| u.speaker_kind == "persona"), "誰も応答していない");
}
