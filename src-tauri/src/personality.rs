use crate::db::Db;
use crate::error::AppResult;
use crate::models::{
    new_id, MoodEvent, MoodState, PersonalityEvent, Relationship, Settings, MOOD_MAX, MOOD_MIN,
    MOOD_NEUTRAL_BAND,
};

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

// ---------- ムード (v0.2, ADR-13) ----------

/// 保存値から現在ムードを半減期で減衰導出する (ADR-13)。
/// 経過が負(時計巻き戻し)なら経過0扱いで値を変えない。
pub fn derive_mood(value: i64, rated_at: Option<i64>, now_ms: i64, halflife_hours: i64) -> i64 {
    let Some(rated) = rated_at else { return 0 };
    if value == 0 || halflife_hours <= 0 {
        return value;
    }
    let elapsed_h = ((now_ms - rated).max(0) as f64) / 3_600_000.0;
    let factor = 0.5f64.powf(elapsed_h / halflife_hours as f64);
    (value as f64 * factor).round() as i64
}

/// |value| が平常バンド未満なら「平常」、それ以外は保存ラベルを返す (ADR-13)
pub fn mood_label_for(value: i64, stored_label: &str) -> String {
    if value.abs() < MOOD_NEUTRAL_BAND {
        "平常".to_string()
    } else {
        stored_label.to_string()
    }
}

/// 減衰計算済みの現在ムードと直近の変動要因を返す (FR-25)
pub fn current_mood(db: &Db, persona_id: &str, settings: &Settings, now_ms: i64) -> AppResult<MoodState> {
    let (value, label, rated_at) = db.get_mood_raw(persona_id)?;
    let derived = derive_mood(value, rated_at, now_ms, settings.mood_halflife_hours);
    let eff_label = mood_label_for(derived, &label);
    let recent = db.mood_events_of(persona_id)?.into_iter().next();
    Ok(MoodState { value: derived, label: eff_label, rated_at, recent_event: recent })
}

/// ムードのデルタを上限付きで適用する (FR-24, ADR-13)。
/// 現在の減衰後の値に対してデルタを加え、[-100,100] にクランプして保存する。
/// 変化があれば mood_event を追記して返す。人格(trait/relationship)には一切触れない。
pub fn apply_mood(
    db: &Db,
    persona_id: &str,
    session_id: &str,
    raw_delta: i64,
    label: &str,
    settings: &Settings,
    now_ms: i64,
) -> AppResult<Option<MoodEvent>> {
    let (stored, _stored_label, rated_at) = db.get_mood_raw(persona_id)?;
    let old = derive_mood(stored, rated_at, now_ms, settings.mood_halflife_hours);
    let delta = clamp_delta(raw_delta, settings.mood_delta_cap);
    let new = (old + delta).clamp(MOOD_MIN, MOOD_MAX);
    let eff_label: String = label.trim().chars().take(20).collect();
    // rated_at を now に更新し、以後の減衰の基準にする(平常ラベルは読み出し時に導出)
    db.set_mood(persona_id, new, &eff_label, now_ms)?;
    if new == old {
        return Ok(None);
    }
    let e = MoodEvent {
        id: new_id(),
        persona_id: persona_id.to_string(),
        session_id: Some(session_id.to_string()),
        old_value: old,
        new_value: new,
        label: eff_label,
        created_at: now_ms,
    };
    db.insert_mood_event(&e)?;
    Ok(Some(e))
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

    #[test]
    fn mood_decays_over_time() {
        // FR-24/ADR-13: 半減期で平常へ回帰する
        let now = now_ms();
        let hl = 24;
        assert_eq!(derive_mood(80, Some(now), now, hl), 80); // 直後は変化なし
        assert_eq!(derive_mood(80, Some(now - 24 * 3_600_000), now, hl), 40); // 24h で半減
        assert_eq!(derive_mood(80, Some(now - 48 * 3_600_000), now, hl), 20); // 48h で 1/4
        // 時計巻き戻し(now < rated)でも値は変わらない
        assert_eq!(derive_mood(80, Some(now + 3_600_000), now, hl), 80);
        // 評定なしは平常(0)
        assert_eq!(derive_mood(80, None, now, hl), 0);
    }

    #[test]
    fn mood_neutral_band_label() {
        assert_eq!(mood_label_for(5, "上機嫌"), "平常");
        assert_eq!(mood_label_for(-5, "不機嫌"), "平常");
        assert_eq!(mood_label_for(40, "上機嫌"), "上機嫌");
    }

    #[test]
    fn mood_apply_clamps_and_records() {
        // FR-24: 1セッションの変化量上限、値域クランプ、変動要因の記録
        let (_d, db, p) = test_env();
        let s = Settings::default(); // mood_delta_cap = 50, halflife 24
        let now = now_ms();
        // +100 要求 → +50 にクランプ (0 → 50)
        let e = apply_mood(&db, &p.id, "s1", 100, "上機嫌", &s, now).unwrap().unwrap();
        assert_eq!(e.old_value, 0);
        assert_eq!(e.new_value, 50);
        let m = current_mood(&db, &p.id, &s, now).unwrap();
        assert_eq!(m.value, 50);
        assert_eq!(m.label, "上機嫌");
        // さらに +50 → 100 上限でクランプ (50 → 100)
        let e2 = apply_mood(&db, &p.id, "s2", 50, "大喜び", &s, now).unwrap().unwrap();
        assert_eq!(e2.new_value, 100);
        // ムードは性格・親密度を変えない (FR-24 構造保証の確認)
        assert_eq!(db.traits_of(&p.id).unwrap()[0].value, 50);
        assert!(db.get_relationship(&p.id, "user", "user").unwrap().is_none());
        // 変動要因が2件記録される
        assert_eq!(db.mood_events_of(&p.id).unwrap().len(), 2);
    }
}
