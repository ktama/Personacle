use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::{AppError, AppResult};
use crate::models::*;

/// SQLite 単一コネクション (ADR-07)。シングルユーザーのため Mutex 直列化で足りる。
pub struct Db {
    conn: Mutex<Connection>,
}

const SCHEMA_V1: &str = r#"
CREATE TABLE persona (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  speech_style TEXT NOT NULL DEFAULT '',
  values_text TEXT NOT NULL DEFAULT '',
  self_intro TEXT NOT NULL DEFAULT '',
  created_at INTEGER NOT NULL,
  last_talked_at INTEGER
);
CREATE TABLE trait (
  persona_id TEXT NOT NULL REFERENCES persona(id) ON DELETE CASCADE,
  trait_key TEXT NOT NULL,
  value INTEGER NOT NULL,
  PRIMARY KEY (persona_id, trait_key)
);
CREATE TABLE relationship (
  persona_id TEXT NOT NULL REFERENCES persona(id) ON DELETE CASCADE,
  target_kind TEXT NOT NULL,
  target_id TEXT NOT NULL,
  target_name TEXT NOT NULL,
  intimacy INTEGER NOT NULL DEFAULT 20,
  impression_text TEXT NOT NULL DEFAULT '',
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (persona_id, target_kind, target_id)
);
CREATE TABLE session (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  theme TEXT NOT NULL DEFAULT '',
  status TEXT NOT NULL,
  started_at INTEGER NOT NULL,
  ended_at INTEGER
);
-- persona_id に外部キーを張らない: ペルソナ削除後もセッション履歴を残す (EC-07)
CREATE TABLE session_participant (
  session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
  persona_id TEXT NOT NULL,
  persona_name TEXT NOT NULL,
  processed INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (session_id, persona_id)
);
CREATE TABLE utterance (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
  speaker_kind TEXT NOT NULL,
  speaker_id TEXT NOT NULL,
  speaker_name TEXT NOT NULL,
  content TEXT NOT NULL,
  state TEXT NOT NULL DEFAULT 'complete',
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_utterance_session ON utterance(session_id, created_at);
CREATE TABLE memory (
  id TEXT PRIMARY KEY,
  persona_id TEXT NOT NULL REFERENCES persona(id) ON DELETE CASCADE,
  content TEXT NOT NULL,
  kind TEXT NOT NULL,
  importance INTEGER NOT NULL,
  embedding BLOB,
  source_session_id TEXT,
  created_at INTEGER NOT NULL,
  archived INTEGER NOT NULL DEFAULT 0,
  user_edited INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_memory_persona ON memory(persona_id, archived, created_at);
CREATE TABLE personality_event (
  id TEXT PRIMARY KEY,
  persona_id TEXT NOT NULL REFERENCES persona(id) ON DELETE CASCADE,
  session_id TEXT,
  item TEXT NOT NULL,
  old_value TEXT NOT NULL,
  new_value TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_pevent_persona ON personality_event(persona_id, created_at);
CREATE TABLE app_setting (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
"#;

impl Db {
    pub fn open(path: &Path) -> AppResult<Self> {
        let conn = Connection::open(path)
            .map_err(|e| AppError::Data(format!("DBを開けません: {e}")))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::init(conn)
    }

    /// テスト用: 一時ファイルDB (WALは通常ファイルでのみ有効)
    pub fn open_at(path: &Path) -> AppResult<Self> {
        Self::open(path)
    }

    fn init(conn: Connection) -> AppResult<Self> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        // schema_version は PRAGMA user_version で管理 (設計5.3)
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < 1 {
            conn.execute_batch(SCHEMA_V1)?;
            conn.pragma_update(None, "user_version", 1)?;
        }
        Ok(Db { conn: Mutex::new(conn) })
    }

    fn with<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> AppResult<T> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        f(&conn).map_err(AppError::from)
    }

    // ---------- persona ----------

    pub fn create_persona(&self, p: &Persona, traits: &[TraitValue]) -> AppResult<()> {
        let mut guard = self.conn.lock().expect("db mutex poisoned");
        let tx = guard.transaction().map_err(AppError::from)?;
        tx.execute(
            "INSERT INTO persona (id, name, description, speech_style, values_text, self_intro, created_at, last_talked_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![p.id, p.name, p.description, p.speech_style, p.values_text, p.self_intro, p.created_at, p.last_talked_at],
        ).map_err(AppError::from)?;
        for t in traits {
            tx.execute(
                "INSERT INTO trait (persona_id, trait_key, value) VALUES (?1, ?2, ?3)",
                params![p.id, t.key, t.value],
            ).map_err(AppError::from)?;
        }
        tx.commit().map_err(AppError::from)
    }

    pub fn list_personas(&self) -> AppResult<Vec<Persona>> {
        self.with(|c| {
            let mut stmt = c.prepare(
                "SELECT id, name, description, speech_style, values_text, self_intro, created_at, last_talked_at
                 FROM persona ORDER BY created_at",
            )?;
            let rows = stmt.query_map([], row_to_persona)?;
            rows.collect()
        })
    }

    pub fn get_persona(&self, id: &str) -> AppResult<Option<Persona>> {
        self.with(|c| {
            c.query_row(
                "SELECT id, name, description, speech_style, values_text, self_intro, created_at, last_talked_at
                 FROM persona WHERE id = ?1",
                params![id],
                row_to_persona,
            )
            .optional()
        })
    }

    pub fn persona_name_exists(&self, name: &str, exclude_id: Option<&str>) -> AppResult<bool> {
        self.with(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM persona WHERE name = ?1 AND id != COALESCE(?2, '')",
                params![name, exclude_id],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n > 0)
        })
    }

    pub fn update_persona(&self, p: &Persona) -> AppResult<()> {
        self.with(|c| {
            c.execute(
                "UPDATE persona SET name=?2, description=?3, speech_style=?4, values_text=?5, self_intro=?6 WHERE id=?1",
                params![p.id, p.name, p.description, p.speech_style, p.values_text, p.self_intro],
            )
            .map(|_| ())
        })
    }

    pub fn delete_persona(&self, id: &str) -> AppResult<()> {
        // 本人所有のデータは CASCADE で物理削除。他ペルソナ側の記憶・関係は残る (EC-07)
        self.with(|c| c.execute("DELETE FROM persona WHERE id = ?1", params![id]).map(|_| ()))
    }

    pub fn touch_last_talked(&self, id: &str, ts: i64) -> AppResult<()> {
        self.with(|c| {
            c.execute("UPDATE persona SET last_talked_at=?2 WHERE id=?1", params![id, ts]).map(|_| ())
        })
    }

    // ---------- trait ----------

    pub fn traits_of(&self, persona_id: &str) -> AppResult<Vec<TraitValue>> {
        self.with(|c| {
            let mut stmt =
                c.prepare("SELECT trait_key, value FROM trait WHERE persona_id=?1 ORDER BY trait_key")?;
            let rows = stmt.query_map(params![persona_id], |r| {
                Ok(TraitValue { key: r.get(0)?, value: r.get(1)? })
            })?;
            rows.collect()
        })
    }

    pub fn set_trait(&self, persona_id: &str, key: &str, value: i64) -> AppResult<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO trait (persona_id, trait_key, value) VALUES (?1, ?2, ?3)
                 ON CONFLICT(persona_id, trait_key) DO UPDATE SET value=excluded.value",
                params![persona_id, key, value],
            )
            .map(|_| ())
        })
    }

    // ---------- relationship ----------

    pub fn get_relationship(
        &self,
        persona_id: &str,
        target_kind: &str,
        target_id: &str,
    ) -> AppResult<Option<Relationship>> {
        self.with(|c| {
            c.query_row(
                "SELECT persona_id, target_kind, target_id, target_name, intimacy, impression_text, updated_at
                 FROM relationship WHERE persona_id=?1 AND target_kind=?2 AND target_id=?3",
                params![persona_id, target_kind, target_id],
                row_to_relationship,
            )
            .optional()
        })
    }

    pub fn upsert_relationship(&self, r: &Relationship) -> AppResult<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO relationship (persona_id, target_kind, target_id, target_name, intimacy, impression_text, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(persona_id, target_kind, target_id) DO UPDATE SET
                   target_name=excluded.target_name, intimacy=excluded.intimacy,
                   impression_text=excluded.impression_text, updated_at=excluded.updated_at",
                params![r.persona_id, r.target_kind, r.target_id, r.target_name, r.intimacy, r.impression_text, r.updated_at],
            )
            .map(|_| ())
        })
    }

    pub fn relationships_of(&self, persona_id: &str) -> AppResult<Vec<Relationship>> {
        self.with(|c| {
            let mut stmt = c.prepare(
                "SELECT persona_id, target_kind, target_id, target_name, intimacy, impression_text, updated_at
                 FROM relationship WHERE persona_id=?1 ORDER BY updated_at DESC",
            )?;
            let rows = stmt.query_map(params![persona_id], row_to_relationship)?;
            rows.collect()
        })
    }

    // ---------- session ----------

    pub fn create_session(&self, s: &Session) -> AppResult<()> {
        let mut guard = self.conn.lock().expect("db mutex poisoned");
        let tx = guard.transaction().map_err(AppError::from)?;
        tx.execute(
            "INSERT INTO session (id, kind, theme, status, started_at, ended_at) VALUES (?1,?2,?3,?4,?5,?6)",
            params![s.id, s.kind, s.theme, s.status, s.started_at, s.ended_at],
        ).map_err(AppError::from)?;
        for (pid, pname) in s.participant_ids.iter().zip(s.participant_names.iter()) {
            tx.execute(
                "INSERT INTO session_participant (session_id, persona_id, persona_name) VALUES (?1,?2,?3)",
                params![s.id, pid, pname],
            ).map_err(AppError::from)?;
        }
        tx.commit().map_err(AppError::from)
    }

    pub fn get_session(&self, id: &str) -> AppResult<Option<Session>> {
        let base = self.with(|c| {
            c.query_row(
                "SELECT id, kind, theme, status, started_at, ended_at FROM session WHERE id=?1",
                params![id],
                |r| {
                    Ok(Session {
                        id: r.get(0)?,
                        kind: r.get(1)?,
                        theme: r.get(2)?,
                        status: r.get(3)?,
                        started_at: r.get(4)?,
                        ended_at: r.get(5)?,
                        participant_ids: vec![],
                        participant_names: vec![],
                    })
                },
            )
            .optional()
        })?;
        let Some(mut s) = base else { return Ok(None) };
        let parts = self.participants_of(id)?;
        s.participant_ids = parts.iter().map(|(id, _, _)| id.clone()).collect();
        s.participant_names = parts.iter().map(|(_, name, _)| name.clone()).collect();
        Ok(Some(s))
    }

    /// (persona_id, persona_name, processed)
    pub fn participants_of(&self, session_id: &str) -> AppResult<Vec<(String, String, bool)>> {
        self.with(|c| {
            let mut stmt = c.prepare(
                "SELECT persona_id, persona_name, processed FROM session_participant WHERE session_id=?1 ORDER BY rowid",
            )?;
            let rows = stmt.query_map(params![session_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)? != 0))
            })?;
            rows.collect()
        })
    }

    pub fn set_session_status(&self, id: &str, status: &str, ended_at: Option<i64>) -> AppResult<()> {
        self.with(|c| {
            c.execute(
                "UPDATE session SET status=?2, ended_at=COALESCE(?3, ended_at) WHERE id=?1",
                params![id, status, ended_at],
            )
            .map(|_| ())
        })
    }

    pub fn mark_participant_processed(&self, session_id: &str, persona_id: &str) -> AppResult<()> {
        self.with(|c| {
            c.execute(
                "UPDATE session_participant SET processed=1 WHERE session_id=?1 AND persona_id=?2",
                params![session_id, persona_id],
            )
            .map(|_| ())
        })
    }

    pub fn sessions_by_status(&self, status: &str) -> AppResult<Vec<String>> {
        self.with(|c| {
            let mut stmt = c.prepare("SELECT id FROM session WHERE status=?1 ORDER BY started_at")?;
            let rows = stmt.query_map(params![status], |r| r.get::<_, String>(0))?;
            rows.collect()
        })
    }

    pub fn list_sessions_for_persona(&self, persona_id: &str) -> AppResult<Vec<Session>> {
        let ids: Vec<String> = self.with(|c| {
            let mut stmt = c.prepare(
                "SELECT s.id FROM session s JOIN session_participant sp ON sp.session_id = s.id
                 WHERE sp.persona_id=?1 ORDER BY s.started_at DESC",
            )?;
            let rows = stmt.query_map(params![persona_id], |r| r.get::<_, String>(0))?;
            rows.collect()
        })?;
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(s) = self.get_session(&id)? {
                out.push(s);
            }
        }
        Ok(out)
    }

    // ---------- utterance ----------

    pub fn insert_utterance(&self, u: &Utterance) -> AppResult<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO utterance (id, session_id, speaker_kind, speaker_id, speaker_name, content, state, created_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                params![u.id, u.session_id, u.speaker_kind, u.speaker_id, u.speaker_name, u.content, u.state, u.created_at],
            )
            .map(|_| ())
        })
    }

    pub fn utterances_of(&self, session_id: &str) -> AppResult<Vec<Utterance>> {
        self.with(|c| {
            let mut stmt = c.prepare(
                "SELECT id, session_id, speaker_kind, speaker_id, speaker_name, content, state, created_at
                 FROM utterance WHERE session_id=?1 ORDER BY created_at, rowid",
            )?;
            let rows = stmt.query_map(params![session_id], row_to_utterance)?;
            rows.collect()
        })
    }

    // ---------- memory ----------

    pub fn insert_memory(&self, m: &Memory, embedding: Option<&[u8]>) -> AppResult<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO memory (id, persona_id, content, kind, importance, embedding, source_session_id, created_at, archived, user_edited)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                params![m.id, m.persona_id, m.content, m.kind, m.importance, embedding, m.source_session_id, m.created_at, m.archived as i64, m.user_edited as i64],
            )
            .map(|_| ())
        })
    }

    pub fn memories_of(&self, persona_id: &str, include_archived: bool) -> AppResult<Vec<Memory>> {
        self.with(|c| {
            let sql = format!(
                "SELECT id, persona_id, content, kind, importance, embedding IS NOT NULL, source_session_id, created_at, archived, user_edited
                 FROM memory WHERE persona_id=?1 {} ORDER BY created_at DESC",
                if include_archived { "" } else { "AND archived=0" }
            );
            let mut stmt = c.prepare(&sql)?;
            let rows = stmt.query_map(params![persona_id], row_to_memory)?;
            rows.collect()
        })
    }

    /// 想起用: 未アーカイブ記憶を (Memory, embedding) で返す
    pub fn memories_for_recall(&self, persona_id: &str) -> AppResult<Vec<(Memory, Option<Vec<u8>>)>> {
        self.with(|c| {
            let mut stmt = c.prepare(
                "SELECT id, persona_id, content, kind, importance, embedding IS NOT NULL, source_session_id, created_at, archived, user_edited, embedding
                 FROM memory WHERE persona_id=?1 AND archived=0",
            )?;
            let rows = stmt.query_map(params![persona_id], |r| {
                let m = row_to_memory(r)?;
                let blob: Option<Vec<u8>> = r.get(10)?;
                Ok((m, blob))
            })?;
            rows.collect()
        })
    }

    pub fn update_memory_content(&self, id: &str, content: &str) -> AppResult<()> {
        // 編集時は埋め込みを無効化し、再計算対象にする (FR-11)
        self.with(|c| {
            c.execute(
                "UPDATE memory SET content=?2, user_edited=1, embedding=NULL WHERE id=?1",
                params![id, content],
            )
            .map(|_| ())
        })
    }

    pub fn delete_memory(&self, id: &str) -> AppResult<()> {
        self.with(|c| c.execute("DELETE FROM memory WHERE id=?1", params![id]).map(|_| ()))
    }

    pub fn set_memory_embedding(&self, id: &str, embedding: &[u8]) -> AppResult<()> {
        self.with(|c| {
            c.execute("UPDATE memory SET embedding=?2 WHERE id=?1", params![id, embedding]).map(|_| ())
        })
    }

    /// (memory_id, content) 埋め込み未計算の記憶
    pub fn memories_missing_embedding(&self) -> AppResult<Vec<(String, String)>> {
        self.with(|c| {
            let mut stmt =
                c.prepare("SELECT id, content FROM memory WHERE embedding IS NULL AND archived=0")?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            rows.collect()
        })
    }

    pub fn count_active_memories(&self, persona_id: &str) -> AppResult<i64> {
        self.with(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM memory WHERE persona_id=?1 AND archived=0",
                params![persona_id],
                |r| r.get(0),
            )
        })
    }

    pub fn archive_memories(&self, ids: &[String]) -> AppResult<()> {
        let mut guard = self.conn.lock().expect("db mutex poisoned");
        let tx = guard.transaction().map_err(AppError::from)?;
        for id in ids {
            tx.execute("UPDATE memory SET archived=1 WHERE id=?1", params![id])
                .map_err(AppError::from)?;
        }
        tx.commit().map_err(AppError::from)
    }

    // ---------- personality_event ----------

    pub fn insert_personality_event(&self, e: &PersonalityEvent) -> AppResult<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO personality_event (id, persona_id, session_id, item, old_value, new_value, created_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![e.id, e.persona_id, e.session_id, e.item, e.old_value, e.new_value, e.created_at],
            )
            .map(|_| ())
        })
    }

    pub fn personality_events_of(&self, persona_id: &str) -> AppResult<Vec<PersonalityEvent>> {
        self.with(|c| {
            let mut stmt = c.prepare(
                "SELECT id, persona_id, session_id, item, old_value, new_value, created_at
                 FROM personality_event WHERE persona_id=?1 ORDER BY created_at DESC, rowid DESC",
            )?;
            let rows = stmt.query_map(params![persona_id], |r| {
                Ok(PersonalityEvent {
                    id: r.get(0)?,
                    persona_id: r.get(1)?,
                    session_id: r.get(2)?,
                    item: r.get(3)?,
                    old_value: r.get(4)?,
                    new_value: r.get(5)?,
                    created_at: r.get(6)?,
                })
            })?;
            rows.collect()
        })
    }

    // ---------- settings ----------

    pub fn get_setting(&self, key: &str) -> AppResult<Option<String>> {
        self.with(|c| {
            c.query_row("SELECT value FROM app_setting WHERE key=?1", params![key], |r| r.get(0))
                .optional()
        })
    }

    pub fn set_setting(&self, key: &str, value: &str) -> AppResult<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO app_setting (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![key, value],
            )
            .map(|_| ())
        })
    }

    pub fn load_settings(&self) -> AppResult<Settings> {
        let mut s = Settings::default();
        let get = |k: &str| self.get_setting(k);
        if let Some(v) = get("endpoint")? { s.endpoint = v; }
        if let Some(v) = get("chat_model")? { s.chat_model = v; }
        if let Some(v) = get("embed_model")? { s.embed_model = v; }
        if let Some(v) = get("auto_turn_limit")? { s.auto_turn_limit = v.parse().unwrap_or(s.auto_turn_limit); }
        if let Some(v) = get("input_max_chars")? { s.input_max_chars = v.parse().unwrap_or(s.input_max_chars); }
        if let Some(v) = get("recall_k")? { s.recall_k = v.parse().unwrap_or(s.recall_k); }
        if let Some(v) = get("w_sim")? { s.w_sim = v.parse().unwrap_or(s.w_sim); }
        if let Some(v) = get("w_rec")? { s.w_rec = v.parse().unwrap_or(s.w_rec); }
        if let Some(v) = get("w_imp")? { s.w_imp = v.parse().unwrap_or(s.w_imp); }
        if let Some(v) = get("trait_delta_cap")? { s.trait_delta_cap = v.parse().unwrap_or(s.trait_delta_cap); }
        if let Some(v) = get("intimacy_delta_cap")? { s.intimacy_delta_cap = v.parse().unwrap_or(s.intimacy_delta_cap); }
        if let Some(v) = get("memory_cap")? { s.memory_cap = v.parse().unwrap_or(s.memory_cap); }
        if let Some(v) = get("context_chars")? { s.context_chars = v.parse().unwrap_or(s.context_chars); }
        s.auto_turn_limit = s.auto_turn_limit.clamp(2, AUTO_TURN_HARD_MAX);
        Ok(s)
    }

    pub fn save_settings(&self, s: &Settings) -> AppResult<()> {
        self.set_setting("endpoint", &s.endpoint)?;
        self.set_setting("chat_model", &s.chat_model)?;
        self.set_setting("embed_model", &s.embed_model)?;
        self.set_setting("auto_turn_limit", &s.auto_turn_limit.to_string())?;
        self.set_setting("input_max_chars", &s.input_max_chars.to_string())?;
        self.set_setting("recall_k", &s.recall_k.to_string())?;
        self.set_setting("w_sim", &s.w_sim.to_string())?;
        self.set_setting("w_rec", &s.w_rec.to_string())?;
        self.set_setting("w_imp", &s.w_imp.to_string())?;
        self.set_setting("trait_delta_cap", &s.trait_delta_cap.to_string())?;
        self.set_setting("intimacy_delta_cap", &s.intimacy_delta_cap.to_string())?;
        self.set_setting("memory_cap", &s.memory_cap.to_string())?;
        self.set_setting("context_chars", &s.context_chars.to_string())?;
        Ok(())
    }
}

fn row_to_persona(r: &rusqlite::Row) -> rusqlite::Result<Persona> {
    Ok(Persona {
        id: r.get(0)?,
        name: r.get(1)?,
        description: r.get(2)?,
        speech_style: r.get(3)?,
        values_text: r.get(4)?,
        self_intro: r.get(5)?,
        created_at: r.get(6)?,
        last_talked_at: r.get(7)?,
    })
}

fn row_to_relationship(r: &rusqlite::Row) -> rusqlite::Result<Relationship> {
    Ok(Relationship {
        persona_id: r.get(0)?,
        target_kind: r.get(1)?,
        target_id: r.get(2)?,
        target_name: r.get(3)?,
        intimacy: r.get(4)?,
        impression_text: r.get(5)?,
        updated_at: r.get(6)?,
    })
}

fn row_to_utterance(r: &rusqlite::Row) -> rusqlite::Result<Utterance> {
    Ok(Utterance {
        id: r.get(0)?,
        session_id: r.get(1)?,
        speaker_kind: r.get(2)?,
        speaker_id: r.get(3)?,
        speaker_name: r.get(4)?,
        content: r.get(5)?,
        state: r.get(6)?,
        created_at: r.get(7)?,
    })
}

fn row_to_memory(r: &rusqlite::Row) -> rusqlite::Result<Memory> {
    Ok(Memory {
        id: r.get(0)?,
        persona_id: r.get(1)?,
        content: r.get(2)?,
        kind: r.get(3)?,
        importance: r.get(4)?,
        has_embedding: r.get::<_, i64>(5)? != 0,
        source_session_id: r.get(6)?,
        created_at: r.get(7)?,
        archived: r.get::<_, i64>(8)? != 0,
        user_edited: r.get::<_, i64>(9)? != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{new_id, now_ms};

    fn test_db() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("test.db")).unwrap();
        (dir, db)
    }

    fn mk_persona(name: &str) -> Persona {
        Persona {
            id: new_id(),
            name: name.into(),
            description: "明るい".into(),
            speech_style: "丁寧語".into(),
            values_text: "誠実".into(),
            self_intro: "こんにちは".into(),
            created_at: now_ms(),
            last_talked_at: None,
        }
    }

    #[test]
    fn persona_crud_roundtrip() {
        let (_d, db) = test_db();
        let p = mk_persona("アリス");
        db.create_persona(&p, &[TraitValue { key: "sociability".into(), value: 60 }]).unwrap();
        assert_eq!(db.list_personas().unwrap().len(), 1);
        assert!(db.persona_name_exists("アリス", None).unwrap());
        assert!(!db.persona_name_exists("アリス", Some(&p.id)).unwrap());
        let got = db.get_persona(&p.id).unwrap().unwrap();
        assert_eq!(got.name, "アリス");
        assert_eq!(db.traits_of(&p.id).unwrap()[0].value, 60);

        let mut edited = got.clone();
        edited.description = "冷静".into();
        db.update_persona(&edited).unwrap();
        assert_eq!(db.get_persona(&p.id).unwrap().unwrap().description, "冷静");

        db.delete_persona(&p.id).unwrap();
        assert!(db.get_persona(&p.id).unwrap().is_none());
        // CASCADE で trait も消える
        assert!(db.traits_of(&p.id).unwrap().is_empty());
    }

    #[test]
    fn persistence_across_connections() {
        // FR-17: 別コネクションで再読込しても保持される
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.db");
        let p = mk_persona("ボブ");
        {
            let db = Db::open(&path).unwrap();
            db.create_persona(&p, &[]).unwrap();
            db.set_setting("endpoint", "http://example.local").unwrap();
        }
        let db2 = Db::open(&path).unwrap();
        assert_eq!(db2.get_persona(&p.id).unwrap().unwrap().name, "ボブ");
        assert_eq!(db2.get_setting("endpoint").unwrap().unwrap(), "http://example.local");
    }

    #[test]
    fn deleted_persona_keeps_others_data() {
        // EC-07: A を削除しても B の A に関する記憶・関係は残る
        let (_d, db) = test_db();
        let a = mk_persona("アリス");
        let b = mk_persona("ボブ");
        db.create_persona(&a, &[]).unwrap();
        db.create_persona(&b, &[]).unwrap();

        db.upsert_relationship(&Relationship {
            persona_id: b.id.clone(),
            target_kind: "persona".into(),
            target_id: a.id.clone(),
            target_name: "アリス".into(),
            intimacy: 30,
            impression_text: "面白い人".into(),
            updated_at: now_ms(),
        }).unwrap();
        let m = Memory {
            id: new_id(),
            persona_id: b.id.clone(),
            content: "アリスは猫が好きだ".into(),
            kind: "fact".into(),
            importance: 5,
            has_embedding: false,
            source_session_id: None,
            created_at: now_ms(),
            archived: false,
            user_edited: false,
        };
        db.insert_memory(&m, None).unwrap();

        db.delete_persona(&a.id).unwrap();
        // B の記憶と関係は保持される
        assert_eq!(db.memories_of(&b.id, true).unwrap().len(), 1);
        let rels = db.relationships_of(&b.id).unwrap();
        assert_eq!(rels.len(), 1);
        assert_eq!(rels[0].target_name, "アリス"); // 名前スナップショットで表示可能
    }

    #[test]
    fn session_and_utterances() {
        let (_d, db) = test_db();
        let p = mk_persona("アリス");
        db.create_persona(&p, &[]).unwrap();
        let s = Session {
            id: new_id(),
            kind: "user_dialogue".into(),
            theme: String::new(),
            status: "active".into(),
            started_at: now_ms(),
            ended_at: None,
            participant_ids: vec![p.id.clone()],
            participant_names: vec![p.name.clone()],
        };
        db.create_session(&s).unwrap();
        db.insert_utterance(&Utterance {
            id: new_id(),
            session_id: s.id.clone(),
            speaker_kind: "user".into(),
            speaker_id: "user".into(),
            speaker_name: "ユーザー".into(),
            content: "こんにちは".into(),
            state: "complete".into(),
            created_at: now_ms(),
        }).unwrap();

        let got = db.get_session(&s.id).unwrap().unwrap();
        assert_eq!(got.participant_ids, vec![p.id.clone()]);
        assert_eq!(db.utterances_of(&s.id).unwrap().len(), 1);
        assert_eq!(db.list_sessions_for_persona(&p.id).unwrap().len(), 1);

        db.set_session_status(&s.id, "ended", Some(now_ms())).unwrap();
        assert_eq!(db.sessions_by_status("ended").unwrap(), vec![s.id.clone()]);
        db.mark_participant_processed(&s.id, &p.id).unwrap();
        assert!(db.participants_of(&s.id).unwrap()[0].2);
    }

    #[test]
    fn memory_edit_clears_embedding() {
        // FR-11: 編集で埋め込みが無効化され再計算対象になる
        let (_d, db) = test_db();
        let p = mk_persona("アリス");
        db.create_persona(&p, &[]).unwrap();
        let m = Memory {
            id: new_id(),
            persona_id: p.id.clone(),
            content: "元の記憶".into(),
            kind: "fact".into(),
            importance: 5,
            has_embedding: true,
            source_session_id: None,
            created_at: now_ms(),
            archived: false,
            user_edited: false,
        };
        db.insert_memory(&m, Some(&[0u8; 8])).unwrap();
        assert!(db.memories_missing_embedding().unwrap().is_empty());

        db.update_memory_content(&m.id, "修正した記憶").unwrap();
        let missing = db.memories_missing_embedding().unwrap();
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].1, "修正した記憶");
        let got = &db.memories_of(&p.id, true).unwrap()[0];
        assert!(got.user_edited);
    }

    #[test]
    fn settings_roundtrip_and_clamp() {
        let (_d, db) = test_db();
        let mut s = db.load_settings().unwrap();
        assert_eq!(s.endpoint, "http://127.0.0.1:11434");
        s.chat_model = "gpt-oss:20b".into();
        s.auto_turn_limit = 999; // 上限50に丸められる
        db.save_settings(&s).unwrap();
        let loaded = db.load_settings().unwrap();
        assert_eq!(loaded.chat_model, "gpt-oss:20b");
        assert_eq!(loaded.auto_turn_limit, AUTO_TURN_HARD_MAX);
    }
}
