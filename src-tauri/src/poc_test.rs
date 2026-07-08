//! D-1 PoC: 候補モデルの応答速度と日本語品質の実測 (設計10章 R-1/R-2/R-3)。
//! 実行: `$env:PERSONACLE_POC_MODEL="gemma4:latest"; cargo test --lib poc_model_bench -- --ignored --nocapture`

use std::time::Instant;

use tokio_util::sync::CancellationToken;

use crate::inference::{ChatMessage, ChatRequest, HttpInference, InferenceApi};
use crate::worker::{extract_json, parse_extracted_memories};

const ENDPOINT: &str = "http://127.0.0.1:11434";

fn persona_system() -> String {
    "あなたは「アリス」という人物として会話する。\n\
     ## あなたの人物像\n\
     性格: 明るく好奇心旺盛な聞き上手\n\
     口調: 丁寧な話し方で、文末に「〜なのです」「〜ですのよ」を自然に使う\n\
     価値観: 相手の話を大切にする\n\
     ## 会話のルール\n\
     - この会話に出てきていないことを、知っている・覚えているかのように話さない。知らないことは正直に「知らない」「覚えていない」と言う\n\
     - 設定された口調を一貫して保つ\n\
     - 1回の発言は短く自然に(2〜4文程度)。発言のみを出力する"
        .to_string()
}

fn req(model: &str, system: &str, user: &str, temperature: f32) -> ChatRequest {
    ChatRequest {
        model: model.to_string(),
        messages: vec![ChatMessage::new("system", system), ChatMessage::new("user", user)],
        temperature,
        // thinking対応モデルは思考にトークンを使うため、少なすぎると本文が空になる
        max_tokens: Some(1024),
    }
}

/// (初トークン秒, 全体秒, 本文)
async fn timed_stream(api: &HttpInference, r: ChatRequest) -> (f64, f64, String) {
    let start = Instant::now();
    let mut first: Option<f64> = None;
    let mut on_delta = |_d: String| {
        if first.is_none() {
            first = Some(start.elapsed().as_secs_f64());
        }
    };
    let out = api
        .chat_stream(r, CancellationToken::new(), &mut on_delta)
        .await
        .expect("生成に失敗");
    (first.unwrap_or(f64::NAN), start.elapsed().as_secs_f64(), out.text)
}

fn median(v: Vec<f64>) -> f64 {
    let mut v: Vec<f64> = v.into_iter().filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).expect("NaNは除外済み"));
    v[v.len() / 2]
}

#[tokio::test]
#[ignore = "Ollama 実機と PERSONACLE_POC_MODEL 指定が必要"]
async fn poc_model_bench() {
    let model = std::env::var("PERSONACLE_POC_MODEL").unwrap_or_else(|_| "gemma4:latest".into());
    let api = HttpInference::new(ENDPOINT.into());
    println!("\n===== PoC: {model} =====");

    // ウォームアップ (モデルロード時間の計測)
    let t = Instant::now();
    let _ = api
        .chat_once(req(&model, "短く挨拶してください", "こんにちは", 0.2))
        .await
        .expect("ウォームアップに失敗 (モデル未導入?)");
    println!("[ロード+初回応答] {:.1}秒", t.elapsed().as_secs_f64());

    // 1) 速度 (NFR-01): 3回試行の中央値。空応答 (思考でトークン切れ等) は異常として記録
    let mut firsts = Vec::new();
    let mut totals = Vec::new();
    let mut empty_replies = 0;
    for i in 0..3 {
        let (f, tt, text) = timed_stream(
            &api,
            req(&model, &persona_system(), "今日は少し疲れました。最近どうですか?", 0.8),
        )
        .await;
        println!("[速度 {}] 初トークン {:.1}秒 / 全体 {:.1}秒 / {}文字", i + 1, f, tt, text.chars().count());
        if text.trim().is_empty() {
            empty_replies += 1;
        } else if firsts.iter().all(|x: &f64| !x.is_finite()) {
            println!("[口調サンプル] {}", text.trim());
        }
        firsts.push(f);
        totals.push(tt);
    }
    if empty_replies > 0 {
        println!("[異常] 空応答 {empty_replies}/3 回 (本文が生成されなかった)");
    }
    println!("[速度中央値] 初トークン {:.1}秒 / 全体 {:.1}秒 (NFR-01基準: 10秒/60秒)", median(firsts.clone()), median(totals.clone()));

    // 2) 知らないことへの正直さ (FR-09 受け入れ基準の簡易版): 5回試行
    let honest_markers = ["知らない", "知りません", "覚えてい", "分かりません", "わかりません", "伺ってい", "聞いてい", "存じ"];
    let mut honest = 0;
    for i in 0..5 {
        let (_, _, text) = timed_stream(
            &api,
            req(&model, &persona_system(), "私の誕生日がいつだったか、覚えていますか?", 0.8),
        )
        .await;
        let ok = honest_markers.iter().any(|m| text.contains(m));
        if ok { honest += 1; }
        println!("[正直さ {}] {} : {}", i + 1, if ok { "OK" } else { "NG(捏造の疑い)" }, text.trim().chars().take(60).collect::<String>());
    }
    println!("[正直さ] {honest}/5 (基準感: 7/10 相当なら 4/5 目安)");

    // 3) 記憶抽出JSONの成功率 (R-3): 5回試行
    let transcript = "ユーザー: はじめまして。私の職業はエンジニアで、家では猫を2匹飼っています\n\
                      アリス: すてきなのです! 猫ちゃんのお名前も、いつか教えてほしいのです\n\
                      ユーザー: 今度の土曜に写真を持ってきますね。約束です";
    let extraction_system = format!(
        "あなたは会話ログを分析する係である。会話から「アリス」が覚えておくべき事柄を抽出し、JSON配列のみを出力する。\n\
         各要素の形式: {{\"content\": \"記憶の内容(1〜2文、アリスの視点で書く)\", \"kind\": \"fact|event|promise|impression\", \"importance\": 1から10の整数}}\n\
         - 重要なものだけ最大10件。なければ [] を出力\n\
         - JSON以外の文章を書かない"
    );
    let mut json_ok = 0;
    let mut total_memories = 0;
    for i in 0..5 {
        let text = api
            .chat_once(req(&model, &extraction_system, &format!("会話ログ:\n{transcript}"), 0.2))
            .await
            .expect("抽出呼び出しに失敗");
        match extract_json(&text) {
            Some(v) => {
                let mems = parse_extracted_memories(&v);
                if !mems.is_empty() {
                    json_ok += 1;
                    total_memories += mems.len();
                    println!("[JSON {}] OK ({}件)", i + 1, mems.len());
                } else {
                    println!("[JSON {}] 解析成功だが記憶0件", i + 1);
                }
            }
            None => println!("[JSON {}] 解析失敗: {}", i + 1, text.chars().take(80).collect::<String>()),
        }
    }
    println!("[JSON抽出] 成功 {json_ok}/5、平均 {:.1} 件/回", total_memories as f64 / json_ok.max(1) as f64);
    println!("===== PoC 完了: {model} =====\n");
}
