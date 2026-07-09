use crate::db::Db;
use crate::error::AppResult;
use crate::models::{Memory, Settings};

/// 30日で約 1/e に減衰する新しさスコア (ADR-04)
const RECENCY_TAU_DAYS: f64 = 30.0;
/// 直近この日数の記憶はアーカイブ対象外 (EC-06)
const ARCHIVE_PROTECT_DAYS: i64 = 30;

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let (mut dot, mut na, mut nb) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

pub fn recency_score(created_at_ms: i64, now_ms: i64) -> f64 {
    let age_days = ((now_ms - created_at_ms).max(0) as f64) / 86_400_000.0;
    (-age_days / RECENCY_TAU_DAYS).exp()
}

/// ハイブリッド想起スコア (ADR-04): 類似度 + 新しさ + 重要度
pub fn memory_score(sim: f64, recency: f64, importance: i64, s: &Settings) -> f64 {
    s.w_sim * sim + s.w_rec * recency + s.w_imp * (importance.clamp(1, 10) as f64 / 10.0)
}

pub fn embedding_to_blob(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

pub fn blob_to_embedding(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// 進行中の話題に関連する記憶を上位 K 件返す (FR-09)。
/// query_emb が None (埋め込み未設定・失敗) の場合は新しさ+重要度のみで縮退動作する。
pub fn retrieve(
    db: &Db,
    persona_id: &str,
    query_emb: Option<&[f32]>,
    settings: &Settings,
    now_ms: i64,
) -> AppResult<Vec<Memory>> {
    let rows = db.memories_for_recall(persona_id)?;
    let mut scored: Vec<(f64, Memory)> = rows
        .into_iter()
        .map(|(m, blob)| {
            let sim = match (query_emb, &blob) {
                (Some(q), Some(b)) => cosine(q, &blob_to_embedding(b)) as f64,
                _ => 0.0,
            };
            let score = memory_score(sim, recency_score(m.created_at, now_ms), m.importance, settings);
            (score, m)
        })
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    Ok(scored.into_iter().take(settings.recall_k.max(0) as usize).map(|(_, m)| m).collect())
}

/// 記憶件数が上限を超えたら、保護期間外の記憶から複合スコア下位をアーカイブする (EC-06)
pub fn archive_overflow(db: &Db, persona_id: &str, settings: &Settings, now_ms: i64) -> AppResult<usize> {
    let count = db.count_active_memories(persona_id)?;
    let overflow = count - settings.memory_cap;
    if overflow <= 0 {
        return Ok(0);
    }
    let rows = db.memories_for_recall(persona_id)?;
    let protect_before = now_ms - ARCHIVE_PROTECT_DAYS * 86_400_000;
    let mut candidates: Vec<(f64, String)> = rows
        .into_iter()
        .filter(|(m, _)| m.created_at < protect_before)
        .map(|(m, _)| {
            let keep_score = memory_score(0.0, recency_score(m.created_at, now_ms), m.importance, settings);
            (keep_score, m.id)
        })
        .collect();
    candidates.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let ids: Vec<String> = candidates.into_iter().take(overflow as usize).map(|(_, id)| id).collect();
    let n = ids.len();
    db.archive_memories(&ids)?;
    Ok(n)
}

/// 記憶統合のクラスタ検出 (ADR-12, FR-22)。
/// 新規記憶 new_emb に対しコサイン類似度が閾値以上の非アーカイブ既存記憶を集め、
/// 新規記憶を含めた件数が最小クラスタ件数以上ならメンバーID群(新規を先頭に含む)を返す。
/// candidates は memories_for_recall の戻り(非アーカイブ+埋め込み)。EC-20: 満たなければ None。
pub fn find_consolidation_cluster(
    new_id: &str,
    new_emb: &[f32],
    candidates: &[(Memory, Option<Vec<u8>>)],
    settings: &Settings,
) -> Option<Vec<String>> {
    let sim_th = settings.consolidate_sim as f32;
    let mut ids: Vec<String> = vec![new_id.to_string()];
    for (m, blob) in candidates {
        if m.id == new_id || m.archived {
            continue;
        }
        if let Some(b) = blob {
            if cosine(new_emb, &blob_to_embedding(b)) >= sim_th {
                ids.push(m.id.clone());
            }
        }
    }
    if (ids.len() as i64) >= settings.consolidate_min_cluster.max(2) {
        Some(ids)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{new_id, now_ms, Persona, TraitValue};

    fn test_env() -> (tempfile::TempDir, Db, Persona) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("m.db")).unwrap();
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
        db.create_persona(&p, &[] as &[TraitValue]).unwrap();
        (dir, db, p)
    }

    fn mk_memory(pid: &str, content: &str, importance: i64, created_at: i64) -> Memory {
        Memory {
            id: new_id(),
            persona_id: pid.into(),
            content: content.into(),
            kind: "fact".into(),
            importance,
            has_embedding: false,
            source_session_id: None,
            created_at,
            archived: false,
            user_edited: false,
        }
    }

    #[test]
    fn cosine_basics() {
        assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        assert_eq!(cosine(&[1.0], &[1.0, 2.0]), 0.0); // 次元不一致は 0
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 1.0]), 0.0); // ゼロベクトルは 0
    }

    #[test]
    fn recency_decays() {
        let now = now_ms();
        assert!(recency_score(now, now) > 0.99);
        let d30 = recency_score(now - 30 * 86_400_000, now);
        assert!(d30 > 0.3 && d30 < 0.4); // 30日で約1/e
        assert!(recency_score(now - 365 * 86_400_000, now) < 0.01);
    }

    #[test]
    fn embedding_blob_roundtrip() {
        let v = vec![0.5f32, -1.25, 3.0];
        assert_eq!(blob_to_embedding(&embedding_to_blob(&v)), v);
    }

    #[test]
    fn retrieve_prefers_similar_recent_important() {
        let (_d, db, p) = test_env();
        let now = now_ms();
        let s = Settings { recall_k: 2, ..Settings::default() };

        // 類似度の高い記憶
        let m_sim = mk_memory(&p.id, "猫の話", 5, now - 10 * 86_400_000);
        db.insert_memory(&m_sim, Some(&embedding_to_blob(&[1.0, 0.0]))).unwrap();
        // 直交する古い記憶
        let m_old = mk_memory(&p.id, "天気の話", 5, now - 300 * 86_400_000);
        db.insert_memory(&m_old, Some(&embedding_to_blob(&[0.0, 1.0]))).unwrap();
        // 直交だが直近で重要な記憶
        let m_imp = mk_memory(&p.id, "大事な約束", 10, now);
        db.insert_memory(&m_imp, Some(&embedding_to_blob(&[0.0, 1.0]))).unwrap();

        let got = retrieve(&db, &p.id, Some(&[1.0, 0.0]), &s, now).unwrap();
        assert_eq!(got.len(), 2);
        let contents: Vec<&str> = got.iter().map(|m| m.content.as_str()).collect();
        assert!(contents.contains(&"猫の話")); // 類似
        assert!(contents.contains(&"大事な約束")); // 新しさ+重要度
        assert!(!contents.contains(&"天気の話")); // 古く無関係な記憶は落ちる
    }

    #[test]
    fn retrieve_degrades_without_query_embedding() {
        // 埋め込みなしでも新しさ+重要度で動く (EC-02 縮退)
        let (_d, db, p) = test_env();
        let now = now_ms();
        let s = Settings { recall_k: 1, ..Settings::default() };
        db.insert_memory(&mk_memory(&p.id, "古い", 5, now - 100 * 86_400_000), None).unwrap();
        db.insert_memory(&mk_memory(&p.id, "新しい", 5, now), None).unwrap();
        let got = retrieve(&db, &p.id, None, &s, now).unwrap();
        assert_eq!(got[0].content, "新しい");
    }

    fn mk_scored(pid: &str, content: &str, emb: &[f32]) -> (Memory, Option<Vec<u8>>) {
        (mk_memory(pid, content, 5, now_ms()), Some(embedding_to_blob(emb)))
    }

    #[test]
    fn consolidation_cluster_forms_at_threshold() {
        // FR-22/ADR-12: 類似 (>=0.80) が新規含め5件以上でクラスタ成立
        let s = Settings { consolidate_sim: 0.80, consolidate_min_cluster: 5, ..Settings::default() };
        let pid = "p1";
        let new_id_s = new_id();
        let new_emb = [1.0f32, 0.0];
        // 類似4件 (ほぼ同方向) + 直交1件
        let cands = vec![
            (Memory { id: new_id_s.clone(), ..mk_memory(pid, "新規", 5, now_ms()) }, Some(embedding_to_blob(&new_emb))),
            mk_scored(pid, "類似1", &[0.99, 0.14]),
            mk_scored(pid, "類似2", &[0.98, 0.20]),
            mk_scored(pid, "類似3", &[1.0, 0.05]),
            mk_scored(pid, "類似4", &[0.97, 0.24]),
            mk_scored(pid, "無関係", &[0.0, 1.0]),
        ];
        let cluster = find_consolidation_cluster(&new_id_s, &new_emb, &cands, &s).unwrap();
        // 新規 + 類似4 = 5件。無関係は入らない
        assert_eq!(cluster.len(), 5);
        assert!(cluster.contains(&new_id_s));
    }

    #[test]
    fn consolidation_skips_small_cluster() {
        // EC-20: 類似が足りなければ統合しない
        let s = Settings { consolidate_sim: 0.80, consolidate_min_cluster: 5, ..Settings::default() };
        let pid = "p1";
        let new_id_s = new_id();
        let new_emb = [1.0f32, 0.0];
        let cands = vec![
            mk_scored(pid, "類似1", &[0.99, 0.14]),
            mk_scored(pid, "類似2", &[0.98, 0.20]),
        ];
        assert!(find_consolidation_cluster(&new_id_s, &new_emb, &cands, &s).is_none());
    }

    #[test]
    fn consolidation_excludes_archived_and_unembedded() {
        let s = Settings { consolidate_sim: 0.80, consolidate_min_cluster: 3, ..Settings::default() };
        let pid = "p1";
        let new_id_s = new_id();
        let new_emb = [1.0f32, 0.0];
        let mut archived = mk_scored(pid, "アーカイブ済み", &[1.0, 0.0]);
        archived.0.archived = true;
        let no_emb = (mk_memory(pid, "埋め込みなし", 5, now_ms()), None);
        let cands = vec![
            mk_scored(pid, "類似1", &[0.99, 0.10]),
            archived,
            no_emb,
        ];
        // 新規 + 類似1 = 2件 < 3 → None (アーカイブ済み・埋め込みなしは数えない)
        assert!(find_consolidation_cluster(&new_id_s, &new_emb, &cands, &s).is_none());
    }

    #[test]
    fn archive_overflow_protects_recent() {
        let (_d, db, p) = test_env();
        let now = now_ms();
        let s = Settings { memory_cap: 3, ..Settings::default() };
        // 古い低重要度 x2、古い高重要度 x1、直近 x2 → cap 3 で 2 件超過
        db.insert_memory(&mk_memory(&p.id, "古い雑談1", 1, now - 100 * 86_400_000), None).unwrap();
        db.insert_memory(&mk_memory(&p.id, "古い雑談2", 1, now - 90 * 86_400_000), None).unwrap();
        db.insert_memory(&mk_memory(&p.id, "古い重要", 10, now - 80 * 86_400_000), None).unwrap();
        db.insert_memory(&mk_memory(&p.id, "直近1", 1, now), None).unwrap();
        db.insert_memory(&mk_memory(&p.id, "直近2", 1, now - 86_400_000), None).unwrap();

        let archived = archive_overflow(&db, &p.id, &s, now).unwrap();
        assert_eq!(archived, 2);
        let active: Vec<String> = db
            .memories_of(&p.id, false)
            .unwrap()
            .into_iter()
            .map(|m| m.content)
            .collect();
        // 直近は保護され、古い低重要度から消える
        assert!(active.contains(&"直近1".to_string()));
        assert!(active.contains(&"直近2".to_string()));
        assert!(active.contains(&"古い重要".to_string()));
        // アーカイブ済みも閲覧は可能 (EC-06)
        assert_eq!(db.memories_of(&p.id, true).unwrap().len(), 5);
    }
}
