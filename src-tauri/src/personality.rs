use crate::db::Db;
use crate::error::AppResult;
use crate::models::{new_id, PersonalityEvent, Relationship, Settings};

/// 性格軸 (設計5.2 初期案・要件9-4のプロトタイプ検証対象)
pub const TRAIT_KEYS: [&str; 5] = ["sociability", "empathy", "caution", "assertiveness", "cheerfulness"];

pub const TRAIT_LABELS_JA: [(&str, &str); 5] = [
    ("sociability", "社交性"),
    ("empathy", "共感性"),
    ("caution", "慎重さ"),
    ("assertiveness", "自己主張"),
    ("cheerfulness", "明朗さ"),
];

pub fn trait_label_ja(key: &str) -> &str {
    TRAIT_LABELS_JA.iter().find(|(k, _)| *k == key).map(|(_, l)| *l).unwrap_or(key)
}

pub const DEFAULT_TRAIT_VALUE: i64 = 50;
pub const DEFAULT_INTIMACY: i64 = 20;

/// FR-12: 変化量上限はLLMではなくコード側で強制する
pub fn clamp_delta(delta: i64, cap: i64) -> i64 {
    delta.clamp(-cap.abs(), cap.abs())
}

pub fn clamp_value(v: i64) -> i64 {
    v.clamp(0, 100)
}

/// LLM評定の1参加者分 (worker が JSON から構築する)
#[derive(Debug, Clone, Default)]
pub struct PartnerAssessment {
    pub intimacy_delta: i64,
    pub impression: Option<String>,
}

/// 性格軸デルタを上限付きで適用し、変化をイベントとして追記する (FR-12/13)
pub fn apply_trait_deltas(
    db: &Db,
    persona_id: &str,
    session_id: &str,
    deltas: &[(String, i64)],
    settings: &Settings,
    now_ms: i64,
) -> AppResult<Vec<PersonalityEvent>> {
    let current = db.traits_of(persona_id)?;
    let mut events = Vec::new();
    for (key, raw_delta) in deltas {
        if !TRAIT_KEYS.contains(&key.as_str()) {
            continue; // 未知の軸は無視
        }
        let old = current
            .iter()
            .find(|t| &t.key == key)
            .map(|t| t.value)
            .unwrap_or(DEFAULT_TRAIT_VALUE);
        let delta = clamp_delta(*raw_delta, settings.trait_delta_cap);
        let new = clamp_value(old + delta);
        if new == old {
            continue;
        }
        db.set_trait(persona_id, key, new)?;
        let e = PersonalityEvent {
            id: new_id(),
            persona_id: persona_id.to_string(),
            session_id: Some(session_id.to_string()),
            item: format!("trait:{key}"),
            old_value: old.to_string(),
            new_value: new.to_string(),
            created_at: now_ms,
        };
        db.insert_personality_event(&e)?;
        events.push(e);
    }
    Ok(events)
}

/// 相手との関係性 (親密度・印象) を上限付きで更新する (FR-12/15)
#[allow(clippy::too_many_arguments)]
pub fn apply_relationship(
    db: &Db,
    persona_id: &str,
    session_id: &str,
    target_kind: &str,
    target_id: &str,
    target_name: &str,
    assessment: &PartnerAssessment,
    settings: &Settings,
    now_ms: i64,
) -> AppResult<Vec<PersonalityEvent>> {
    let existing = db.get_relationship(persona_id, target_kind, target_id)?;
    let old_intimacy = existing.as_ref().map(|r| r.intimacy).unwrap_or(DEFAULT_INTIMACY);
    let old_impression = existing.as_ref().map(|r| r.impression_text.clone()).unwrap_or_default();

    let delta = clamp_delta(assessment.intimacy_delta, settings.intimacy_delta_cap);
    let new_intimacy = clamp_value(old_intimacy + delta);
    let new_impression = assessment
        .impression
        .as_ref()
        .map(|s| s.chars().take(200).collect::<String>())
        .unwrap_or_else(|| old_impression.clone());

    db.upsert_relationship(&Relationship {
        persona_id: persona_id.to_string(),
        target_kind: target_kind.to_string(),
        target_id: target_id.to_string(),
        target_name: target_name.to_string(),
        intimacy: new_intimacy,
        impression_text: new_impression.clone(),
        updated_at: now_ms,
    })?;

    let mut events = Vec::new();
    if new_intimacy != old_intimacy {
        let e = PersonalityEvent {
            id: new_id(),
            persona_id: persona_id.to_string(),
            session_id: Some(session_id.to_string()),
            item: format!("intimacy:{target_name}"),
            old_value: old_intimacy.to_string(),
            new_value: new_intimacy.to_string(),
            created_at: now_ms,
        };
        db.insert_personality_event(&e)?;
        events.push(e);
    }
    if new_impression != old_impression {
        let e = PersonalityEvent {
            id: new_id(),
            persona_id: persona_id.to_string(),
            session_id: Some(session_id.to_string()),
            item: format!("impression:{target_name}"),
            old_value: old_impression,
            new_value: new_impression,
            created_at: now_ms,
        };
        db.insert_personality_event(&e)?;
        events.push(e);
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{new_id, now_ms, Persona, TraitValue};

    fn test_env() -> (tempfile::TempDir, Db, Persona) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("p.db")).unwrap();
        let p = Persona {
            id: new_id(),
            name: "アリス".into(),
            description: String::new(),
            speech_style: String::new(),
            values_text: String::new(),
            self_intro: String::new(),
            created_at: now_ms(),
            last_talked_at: None,
        };
        db.create_persona(&p, &[TraitValue { key: "sociability".into(), value: 50 }]).unwrap();
        (dir, db, p)
    }

    #[test]
    fn clamp_delta_boundaries() {
        // FR-12 受け入れ基準: 1セッションの変化量が上限を超えない
        assert_eq!(clamp_delta(10, 2), 2);
        assert_eq!(clamp_delta(-10, 2), -2);
        assert_eq!(clamp_delta(1, 2), 1);
        assert_eq!(clamp_delta(0, 2), 0);
        assert_eq!(clamp_delta(-3, 5), -3);
    }

    #[test]
    fn trait_delta_clamped_and_logged() {
        let (_d, db, p) = test_env();
        let s = Settings::default(); // trait_delta_cap = 2
        let events = apply_trait_deltas(
            &db,
            &p.id,
            "sess1",
            &[("sociability".into(), 5), ("unknown_axis".into(), 5)],
            &s,
            now_ms(),
        )
        .unwrap();
        // 未知の軸は無視、+5 は +2 にクランプ
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].old_value, "50");
        assert_eq!(events[0].new_value, "52");
        assert_eq!(db.traits_of(&p.id).unwrap()[0].value, 52);
        // イベントが履歴に残る (FR-13)
        assert_eq!(db.personality_events_of(&p.id).unwrap().len(), 1);
    }

    #[test]
    fn trait_value_stays_in_range() {
        let (_d, db, p) = test_env();
        let mut s = Settings::default();
        s.trait_delta_cap = 100;
        db.set_trait(&p.id, "sociability", 99).unwrap();
        apply_trait_deltas(&db, &p.id, "s", &[("sociability".into(), 50)], &s, now_ms()).unwrap();
        assert_eq!(db.traits_of(&p.id).unwrap()[0].value, 100); // 0-100 に収まる
    }

    #[test]
    fn relationship_created_and_clamped() {
        let (_d, db, p) = test_env();
        let s = Settings::default(); // intimacy_delta_cap = 5
        let a = PartnerAssessment { intimacy_delta: 20, impression: Some("話しやすい".into()) };
        let events =
            apply_relationship(&db, &p.id, "sess1", "user", "user", "ユーザー", &a, &s, now_ms()).unwrap();
        let rel = db.get_relationship(&p.id, "user", "user").unwrap().unwrap();
        assert_eq!(rel.intimacy, DEFAULT_INTIMACY + 5); // +20 → +5 にクランプ
        assert_eq!(rel.impression_text, "話しやすい");
        assert_eq!(events.len(), 2); // intimacy + impression
    }

    #[test]
    fn repeated_positive_sessions_raise_intimacy() {
        // FR-12 受け入れ基準: 好意的セッション10回で親密度が上昇
        let (_d, db, p) = test_env();
        let s = Settings::default();
        for i in 0..10 {
            let a = PartnerAssessment { intimacy_delta: 3, impression: None };
            apply_relationship(&db, &p.id, &format!("s{i}"), "user", "user", "ユーザー", &a, &s, now_ms())
                .unwrap();
        }
        let rel = db.get_relationship(&p.id, "user", "user").unwrap().unwrap();
        assert_eq!(rel.intimacy, DEFAULT_INTIMACY + 30);
        assert!(rel.intimacy > DEFAULT_INTIMACY);
    }
}
