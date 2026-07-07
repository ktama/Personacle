use crate::inference::ChatMessage;
use crate::models::{Memory, Persona, Relationship, TraitValue, Utterance};
use crate::personality::trait_label_ja;

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

/// system プロンプトを組み立てる (設計6.4 の構成順)
pub fn build_system(
    persona: &Persona,
    traits: &[TraitValue],
    relationship: Option<&Relationship>,
    partner_name: &str,
    memories: &[Memory],
    theme: Option<&str>,
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
    // 3. 相手との関係性
    s.push_str(&format!("\n## 会話相手: {partner_name}\n"));
    if let Some(rel) = relationship {
        s.push_str(&format!("- 距離感: {}\n", intimacy_phrase(rel.intimacy)));
        if !rel.impression_text.is_empty() {
            s.push_str(&format!("- あなたが抱いている印象: {}\n", rel.impression_text));
        }
    } else {
        s.push_str("- 初対面の相手である\n");
    }
    // テーマ (自律会話 FR-14)
    if let Some(t) = theme {
        if !t.is_empty() {
            s.push_str(&format!("\n## 会話のテーマ\n{t}\n"));
        }
    }
    // 4. 想起された記憶
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
        let s = build_system(&p, &traits, Some(&rel), "ユーザー", &[mem], None);
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
        let s = build_system(&p, &[], None, "ボブ", &[], Some("休日の過ごし方"));
        assert!(s.contains("休日の過ごし方"));
        assert!(s.contains("初対面"));
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
