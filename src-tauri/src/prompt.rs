use crate::inference::ChatMessage;
use crate::models::{Memory, Persona, Relationship, Settings, TraitValue, Utterance, MOOD_NEUTRAL_BAND};
use crate::personality::trait_label_ja;

/// 前回対話からの経過時間ラベル (ADR-11, FR-20)。
/// コアが区分ラベル化して注入し、SLM に日時計算をさせない。
/// 経過が閾値未満、または負(時計巻き戻し, EC-18)なら None。
pub fn elapsed_label(last_talked_at: Option<i64>, now_ms: i64, s: &Settings) -> Option<String> {
    let last = last_talked_at?;
    let elapsed_ms = now_ms - last;
    if elapsed_ms < 0 {
        return None; // EC-18: 時計巻き戻しでは言及しない
    }
    let hours = elapsed_ms / 3_600_000;
    let days = hours / 24;
    if hours < s.elapsed_short_hours {
        None
    } else if hours < s.elapsed_mid_hours {
        Some("前回の会話から少し間が空いている".to_string())
    } else if days < s.elapsed_long_days {
        Some(format!("前回の会話から約{days}日ぶりである"))
    } else {
        Some("前回会話してから長い間が空いている".to_string())
    }
}

/// 現在ムードの言語化 (ADR-13, FR-24)。平常(バンド内)は None を返し注入しない。
pub fn mood_phrase(value: i64, label: &str) -> Option<String> {
    if value.abs() < MOOD_NEUTRAL_BAND {
        return None;
    }
    let degree = match value.abs() {
        0..=39 => "少し",
        40..=69 => "",
        _ => "とても",
    };
    let dir = if value > 0 { "前向きな気分" } else { "沈んだ気分" };
    let label = if label.is_empty() { dir } else { label };
    Some(format!("今は{degree}{label}({dir})"))
}

/// 数値の性格軸を程度表現に言語化する (設計6.4)
pub fn trait_phrase(key: &str, value: i64) -> String {
    let label = trait_label_ja(key);
    let degree = match value {
        0..=19 => "とても低い",
        20..=39 => "低め",
        40..=59 => "ふつう",
        60..=79 => "高め",
        _ => "とても高い",
    };
    format!("{label}: {degree}({value}/100)")
}

fn intimacy_phrase(intimacy: i64) -> &'static str {
    match intimacy {
        0..=19 => "初対面に近い距離感。丁寧に、少し遠慮がちに接する",
        20..=39 => "知り合い程度。礼儀正しく接する",
        40..=59 => "打ち解けてきた相手。肩の力を抜いて話す",
        60..=79 => "親しい相手。気軽に、率直に話す",
        _ => "とても親しい相手。心を開いて話す",
    }
}

fn format_date(ms: i64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|t| t.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

/// 会話相手1人分の情報 (自律会話では自分以外の全参加者分を渡す: FR-19)
pub struct PartnerInfo<'a> {
    pub name: &'a str,
    pub relationship: Option<&'a Relationship>,
}

/// system プロンプトを組み立てる (設計6.4 の構成順)
pub fn build_system(
    persona: &Persona,
    traits: &[TraitValue],
    partners: &[PartnerInfo],
    memories: &[Memory],
    theme: Option<&str>,
    mood: Option<&str>,
    elapsed: Option<&str>,
) -> String {
    let mut s = String::new();
    // 1. 初期設定
    s.push_str(&format!(
        "あなたは「{}」という人物として会話する。\n## あなたの人物像\n性格: {}\n口調: {}\n価値観: {}\n自己紹介: {}\n",
        persona.name, persona.description, persona.speech_style, persona.values_text, persona.self_intro
    ));
    // 2. 現在の性格傾向
    if !traits.is_empty() {
        s.push_str("\n## 現在の性格傾向\n");
        for t in traits {
            s.push_str(&format!("- {}\n", trait_phrase(&t.key, t.value)));
        }
    }
    // 3. 現在のムード (v0.2, ADR-13。平常時は None で注入されない)
    if let Some(m) = mood {
        s.push_str(&format!("\n## 今の気分\n{m}\n"));
    }
    // 3b. 相手との関係性 (複数相手なら全員分: FR-19)
    if partners.len() == 1 {
        s.push_str(&format!("\n## 会話相手: {}\n", partners[0].name));
    } else {
        let names: Vec<&str> = partners.iter().map(|p| p.name).collect();
        s.push_str(&format!("\n## 会話相手 (複数): {}\n", names.join("、")));
    }
    for p in partners {
        if partners.len() > 1 {
            s.push_str(&format!("### {}\n", p.name));
        }
        if let Some(rel) = p.relationship {
            s.push_str(&format!("- 距離感: {}\n", intimacy_phrase(rel.intimacy)));
            if !rel.impression_text.is_empty() {
                s.push_str(&format!("- あなたが抱いている印象: {}\n", rel.impression_text));
            }
        } else {
            s.push_str("- 初対面の相手である\n");
        }
    }
    // テーマ (自律会話 FR-14)
    if let Some(t) = theme {
        if !t.is_empty() {
            s.push_str(&format!("\n## 会話のテーマ\n{t}\n"));
        }
    }
    // 5. 経過時間ラベル (v0.2, ADR-11。閾値未満・時計巻き戻し時は None で注入されない)
    if let Some(e) = elapsed {
        s.push_str(&format!("\n## 前回からの経過\n{e}。自然な範囲でこのことに触れてよい\n"));
    }
    // 6. 想起された記憶
    if !memories.is_empty() {
        s.push_str("\n## あなたの記憶 (関連する過去の出来事)\n");
        for m in memories {
            s.push_str(&format!("- [{}] {}\n", format_date(m.created_at), m.content));
        }
    }
    // 5. 行動指示 (FR-09: 知らないことは知らないと言う)
    s.push_str(
        "\n## 会話のルール\n\
         - 上記の記憶とこの会話に出てきていないことを、知っている・覚えているかのように話さない。知らないことは正直に「知らない」「覚えていない」と言う\n\
         - 設定された口調を一貫して保つ\n\
         - 1回の発言は短く自然に(2〜4文程度)。地の文や説明は書かず、発言のみを出力する\n",
    );
    s
}

/// 停滞時の話題転換プロンプト (ADR-16, FR-35)。これまでのテーマから派生する新しい話題を1文で生成させる。
pub fn build_topic_shift(theme: &str, history: &[Utterance]) -> String {
    let mut s = String::from("会話が同じ話題で停滞しています。会話を続けるために、これまでの流れから自然に派生する新しい話題を1つ、短い一文で提案してください。\n");
    if !theme.is_empty() {
        s.push_str(&format!("元のテーマ: {theme}\n"));
    }
    s.push_str("## 直近の会話\n");
    for u in history.iter().rev().take(4).rev() {
        s.push_str(&format!("{}: {}\n", u.speaker_name, u.content));
    }
    s.push_str("\n新しい話題を促す一文のみを出力してください(例:「ところで、〜についてはどう思う?」)。");
    s
}

/// グループチャットの話者選択プロンプト (ADR-15)。
/// 直近履歴と参加者一覧(未応答者を優先明示)を渡し、次に応答する人物名のみを出力させる。
/// allow_none=true のとき「発話なし」も選択肢に含める(連鎖判定)。
pub fn build_speaker_selection(
    history: &[Utterance],
    participants: &[(String, String)],
    responded: &std::collections::HashSet<&str>,
    allow_none: bool,
) -> String {
    let mut s = String::from("あなたは会話の司会です。次の会話で、次に発言するのが自然な人物を1人だけ選んでください。\n\n## 参加者\n");
    for (id, name) in participants {
        let mark = if responded.contains(id.as_str()) { "" } else { " (まだ発言していません。優先的に検討)" };
        s.push_str(&format!("- {name}{mark}\n"));
    }
    s.push_str("\n## 直近の会話\n");
    // 直近8発話程度
    let recent: Vec<&Utterance> = history.iter().rev().take(8).collect();
    for u in recent.iter().rev() {
        s.push_str(&format!("{}: {}\n", u.speaker_name, u.content));
    }
    s.push_str("\n## 指示\n");
    if allow_none {
        s.push_str("直前の発言を受けて、誰かがさらに続けて話すのが自然なら、その人物の名前だけを出力してください。会話が一区切りついていて誰も続けないのが自然なら「発話なし」と出力してください。\n");
    } else {
        s.push_str("最も自然に応答できる人物の名前だけを出力してください。\n");
    }
    s.push_str("名前のみを出力し、説明や記号は付けないこと。");
    s
}

/// 会話履歴を文字数予算内で ChatMessage 列に変換する (設計6.4)。
/// 自分の発話は assistant、他者の発話は user (名前プレフィックス付き) にする。
pub fn assemble_messages(
    system: String,
    history: &[Utterance],
    self_persona_id: &str,
    budget_chars: usize,
) -> Vec<ChatMessage> {
    // 新しい発話から遡って予算内に収める
    let mut picked: Vec<&Utterance> = Vec::new();
    let mut used = 0usize;
    for u in history.iter().rev() {
        let cost = u.content.chars().count() + u.speaker_name.chars().count() + 4;
        if used + cost > budget_chars && !picked.is_empty() {
            break;
        }
        used += cost;
        picked.push(u);
        if used > budget_chars {
            break;
        }
    }
    picked.reverse();

    let mut messages = vec![ChatMessage::new("system", system)];
    for u in picked {
        if u.speaker_kind == "persona" && u.speaker_id == self_persona_id {
            messages.push(ChatMessage::new("assistant", u.content.clone()));
        } else {
            messages.push(ChatMessage::new("user", format!("{}: {}", u.speaker_name, u.content)));
        }
    }
    messages
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{new_id, now_ms};

    fn mk_persona() -> Persona {
        Persona {
            id: "p1".into(),
            name: "アリス".into(),
            description: "明るく好奇心旺盛".into(),
            speech_style: "です・ます調".into(),
            values_text: "正直さ".into(),
            self_intro: "アリスです".into(),
            created_at: now_ms(),
            last_talked_at: None,
        }
    }

    fn mk_utt(speaker_id: &str, kind: &str, name: &str, content: &str) -> Utterance {
        Utterance {
            id: new_id(),
            session_id: "s1".into(),
            speaker_kind: kind.into(),
            speaker_id: speaker_id.into(),
            speaker_name: name.into(),
            content: content.into(),
            state: "complete".into(),
            created_at: now_ms(),
        }
    }

    #[test]
    fn system_contains_all_sections() {
        let p = mk_persona();
        let traits = vec![TraitValue { key: "sociability".into(), value: 72 }];
        let rel = Relationship {
            persona_id: "p1".into(),
            target_kind: "user".into(),
            target_id: "user".into(),
            target_name: "ユーザー".into(),
            intimacy: 65,
            impression_text: "優しい人".into(),
            updated_at: now_ms(),
        };
        let mem = Memory {
            id: new_id(),
            persona_id: "p1".into(),
            content: "ユーザーの好物はカレー".into(),
            kind: "fact".into(),
            importance: 6,
            has_embedding: true,
            source_session_id: None,
            created_at: now_ms(),
            archived: false,
            user_edited: false,
        };
        let partners = [PartnerInfo { name: "ユーザー", relationship: Some(&rel) }];
        let s = build_system(&p, &traits, &partners, &[mem], None, None, None);
        assert!(s.contains("アリス"));
        assert!(s.contains("社交性: 高め(72/100)"));
        assert!(s.contains("親しい相手"));
        assert!(s.contains("優しい人"));
        assert!(s.contains("カレー"));
        assert!(s.contains("知らない"));
    }

    #[test]
    fn theme_included_for_autonomous() {
        let p = mk_persona();
        let partners = [PartnerInfo { name: "ボブ", relationship: None }];
        let s = build_system(&p, &[], &partners, &[], Some("休日の過ごし方"), None, None);
        assert!(s.contains("休日の過ごし方"));
        assert!(s.contains("初対面"));
    }

    #[test]
    fn multiple_partners_listed_fr19() {
        let p = mk_persona();
        let rel = Relationship {
            persona_id: "p1".into(),
            target_kind: "persona".into(),
            target_id: "p2".into(),
            target_name: "ボブ".into(),
            intimacy: 70,
            impression_text: "頼れる人".into(),
            updated_at: now_ms(),
        };
        let partners = [
            PartnerInfo { name: "ボブ", relationship: Some(&rel) },
            PartnerInfo { name: "キャロル", relationship: None },
        ];
        let s = build_system(&p, &[], &partners, &[], Some("旅行の計画"), None, None);
        // 全相手が列挙され、それぞれの関係性が出る
        assert!(s.contains("ボブ、キャロル"));
        assert!(s.contains("### ボブ"));
        assert!(s.contains("頼れる人"));
        assert!(s.contains("### キャロル"));
        assert!(s.contains("初対面"));
    }

    #[test]
    fn elapsed_label_thresholds() {
        // FR-20/ADR-11: 閾値ごとの区分ラベル。EC-18: 巻き戻しは None
        let s = Settings::default(); // short 6h, mid 48h, long 14日
        let now = 1_000_000_000_000i64;
        let h = 3_600_000i64;
        assert_eq!(elapsed_label(None, now, &s), None); // 初対話
        assert_eq!(elapsed_label(Some(now - 3 * h), now, &s), None); // 6h未満
        assert!(elapsed_label(Some(now - 24 * h), now, &s).unwrap().contains("少し間が空い"));
        assert!(elapsed_label(Some(now - 5 * 24 * h), now, &s).unwrap().contains("約5日ぶり"));
        assert!(elapsed_label(Some(now - 30 * 24 * h), now, &s).unwrap().contains("長い間"));
        assert_eq!(elapsed_label(Some(now + 10 * h), now, &s), None); // EC-18 巻き戻し
    }

    #[test]
    fn mood_phrase_neutral_is_none() {
        // ADR-13: 平常(バンド内)は注入しない
        assert_eq!(mood_phrase(5, "上機嫌"), None);
        assert!(mood_phrase(60, "上機嫌").unwrap().contains("上機嫌"));
        assert!(mood_phrase(-60, "落ち込み").unwrap().contains("落ち込み"));
    }

    #[test]
    fn system_includes_mood_and_elapsed() {
        let p = mk_persona();
        let partners = [PartnerInfo { name: "ユーザー", relationship: None }];
        let s = build_system(&p, &[], &partners, &[], None, Some("今はとても上機嫌(前向きな気分)"), Some("前回の会話から約3日ぶりである"));
        assert!(s.contains("今の気分"));
        assert!(s.contains("上機嫌"));
        assert!(s.contains("前回からの経過"));
        assert!(s.contains("約3日ぶり"));
    }

    #[test]
    fn assemble_roles_and_prefix() {
        let history = vec![
            mk_utt("user", "user", "ユーザー", "こんにちは"),
            mk_utt("p1", "persona", "アリス", "こんにちは!"),
            mk_utt("p2", "persona", "ボブ", "やあ"),
        ];
        let msgs = assemble_messages("SYS".into(), &history, "p1", 10_000);
        assert_eq!(msgs.len(), 4);
        assert_eq!(msgs[0].role, "system");
        assert_eq!(msgs[1].role, "user");
        assert_eq!(msgs[1].content, "ユーザー: こんにちは");
        assert_eq!(msgs[2].role, "assistant"); // 自分の発話
        assert_eq!(msgs[2].content, "こんにちは!");
        assert_eq!(msgs[3].role, "user");
        assert_eq!(msgs[3].content, "ボブ: やあ"); // 他ペルソナは名前付き
    }

    #[test]
    fn assemble_respects_budget_keeps_recent() {
        let mut history = Vec::new();
        for i in 0..100 {
            history.push(mk_utt("user", "user", "ユーザー", &format!("メッセージ{i:03} {}", "あ".repeat(50))));
        }
        let msgs = assemble_messages("SYS".into(), &history, "p1", 500);
        // 予算内に収まり、最新の発話が必ず含まれる
        assert!(msgs.len() < 100);
        assert!(msgs.last().unwrap().content.contains("メッセージ099"));
    }
}
