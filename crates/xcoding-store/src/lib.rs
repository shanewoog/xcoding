//! SQLite persistence for XCoding sessions and messages.

use std::path::Path;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use thiserror::Error;
use uuid::Uuid;
use xcoding_protocol::{
    ContextCompaction, CreateSessionParams, LocalMemory, Message, MessageRole, PendingAction,
    PendingActionStatus, PersistedSessionEvent, RestorePoint, Session, SessionEvent, SessionStatus,
    ToolCall, WorkspaceConfig,
};

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("invalid stored data: {0}")]
    InvalidData(#[from] serde_json::Error),
    #[error("invalid stored timestamp: {0}")]
    Timestamp(#[from] chrono::ParseError),
    #[error("invalid stored identifier: {0}")]
    Identifier(#[from] uuid::Error),
    #[error("invalid input: {0}")]
    InvalidInput(String),
}

pub struct SessionStore {
    connection: Connection,
}

fn normalize_workspace_root(value: &str) -> String {
    value
        .trim()
        .replace('\\', "/")
        .trim_start_matches("//?/")
        .trim_start_matches("//./")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

fn title_from_message(message: &str) -> String {
    let line = message
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
    const MAX_CHARS: usize = 48;
    if collapsed.chars().count() <= MAX_CHARS {
        return collapsed;
    }
    let mut out: String = collapsed
        .chars()
        .take(MAX_CHARS.saturating_sub(1))
        .collect();
    out.push('…');
    out
}

impl SessionStore {
    fn configure_connection(connection: &Connection) -> Result<(), StoreError> {
        // Multi-session desktop runs open one CoreService/connection per agent worker.
        // WAL + busy_timeout lets concurrent readers/writers cooperate instead of failing.
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "busy_timeout", 5_000i64)?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        Ok(())
    }

    pub fn in_memory() -> Result<Self, StoreError> {
        let connection = Connection::open_in_memory()?;
        Self::configure_connection(&connection)?;
        let store = Self { connection };
        store.migrate()?;
        Ok(store)
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let connection = Connection::open(path)?;
        Self::configure_connection(&connection)?;
        let store = Self { connection };
        store.migrate()?;
        Ok(store)
    }

    pub fn create_session(&self, params: CreateSessionParams) -> Result<Session, StoreError> {
        let now = Utc::now();
        let session = Session {
            id: Uuid::new_v4(),
            workspace_root: params.workspace_root,
            mode: params.mode,
            provider: params.provider,
            model: params.model,
            status: SessionStatus::Created,
            created_at: now,
            updated_at: now,
            title: params.title,
        };

        self.connection.execute(
            "INSERT INTO sessions (
                id, workspace_root, mode, provider, model, status, created_at, updated_at, title
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                session.id.to_string(),
                session.workspace_root,
                serde_json::to_string(&session.mode)?,
                session.provider,
                session.model,
                serde_json::to_string(&session.status)?,
                session.created_at.to_rfc3339(),
                session.updated_at.to_rfc3339(),
                session.title,
            ],
        )?;

        Ok(session)
    }

    pub fn list_sessions(&self, workspace_root: Option<&str>) -> Result<Vec<Session>, StoreError> {
        let mut sessions = Vec::new();

        if let Some(workspace_root) = workspace_root {
            let mut statement = self.connection.prepare(
                "SELECT id, workspace_root, mode, provider, model, status, created_at, updated_at, title
                 FROM sessions WHERE workspace_root = ?1 ORDER BY created_at DESC",
            )?;
            let rows = statement.query_map([workspace_root], Self::row_to_session)?;
            for row in rows {
                sessions.push(self.fill_session_title(row?)?);
            }
        } else {
            let mut statement = self.connection.prepare(
                "SELECT id, workspace_root, mode, provider, model, status, created_at, updated_at, title
                 FROM sessions ORDER BY created_at DESC",
            )?;
            let rows = statement.query_map([], Self::row_to_session)?;
            for row in rows {
                sessions.push(self.fill_session_title(row?)?);
            }
        }

        Ok(sessions)
    }

    pub fn get_workspace_config(
        &self,
        workspace_root: &str,
    ) -> Result<Option<WorkspaceConfig>, StoreError> {
        self.connection
            .query_row(
                "SELECT workspace_root, mode, provider, model, updated_at\n                 FROM workspace_configs WHERE workspace_root = ?1",
                [workspace_root],
                Self::row_to_workspace_config,
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn set_workspace_config(
        &self,
        config: WorkspaceConfig,
    ) -> Result<WorkspaceConfig, StoreError> {
        self.connection.execute(
            "INSERT INTO workspace_configs (workspace_root, mode, provider, model, updated_at)\n             VALUES (?1, ?2, ?3, ?4, ?5)\n             ON CONFLICT(workspace_root) DO UPDATE SET\n                mode = excluded.mode,\n                provider = excluded.provider,\n                model = excluded.model,\n                updated_at = excluded.updated_at",
            params![
                config.workspace_root,
                serde_json::to_string(&config.mode)?,
                config.provider,
                config.model,
                config.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(config)
    }

    pub fn get_session(&self, id: Uuid) -> Result<Option<Session>, StoreError> {
        let session = self
            .connection
            .query_row(
                "SELECT id, workspace_root, mode, provider, model, status, created_at, updated_at, title
                 FROM sessions WHERE id = ?1",
                [id.to_string()],
                Self::row_to_session,
            )
            .optional()
            .map_err(StoreError::from)?;
        match session {
            Some(session) => Ok(Some(self.fill_session_title(session)?)),
            None => Ok(None),
        }
    }

    pub fn delete_session(&self, id: Uuid) -> Result<bool, StoreError> {
        let id = id.to_string();
        self.connection
            .execute("DELETE FROM messages WHERE session_id = ?1", params![id])?;
        self.connection.execute(
            "DELETE FROM pending_actions WHERE session_id = ?1",
            params![id],
        )?;
        self.connection.execute(
            "DELETE FROM restore_points WHERE session_id = ?1",
            params![id],
        )?;
        self.connection.execute(
            "DELETE FROM session_events WHERE session_id = ?1",
            params![id],
        )?;
        self.connection.execute(
            "DELETE FROM context_compactions WHERE session_id = ?1",
            params![id],
        )?;
        let deleted = self
            .connection
            .execute("DELETE FROM sessions WHERE id = ?1", params![id])?;
        Ok(deleted > 0)
    }

    pub fn delete_workspace_sessions(&self, workspace_root: &str) -> Result<usize, StoreError> {
        let workspace_key = normalize_workspace_root(workspace_root);
        if workspace_key.is_empty() {
            return Err(StoreError::InvalidInput(
                "workspace_root must not be empty".to_owned(),
            ));
        }

        let transaction = self.connection.unchecked_transaction()?;
        let session_ids = {
            let mut statement = transaction.prepare("SELECT id, workspace_root FROM sessions")?;
            let rows = statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .filter_map(|(id, stored_root)| {
                    (normalize_workspace_root(&stored_root) == workspace_key).then_some(id)
                })
                .collect::<Vec<_>>()
        };

        for id in &session_ids {
            transaction.execute("DELETE FROM messages WHERE session_id = ?1", params![id])?;
            transaction.execute(
                "DELETE FROM pending_actions WHERE session_id = ?1",
                params![id],
            )?;
            transaction.execute(
                "DELETE FROM restore_points WHERE session_id = ?1",
                params![id],
            )?;
            transaction.execute(
                "DELETE FROM session_events WHERE session_id = ?1",
                params![id],
            )?;
            transaction.execute(
                "DELETE FROM context_compactions WHERE session_id = ?1",
                params![id],
            )?;
            transaction.execute("DELETE FROM sessions WHERE id = ?1", params![id])?;
        }

        let workspace_config_roots = {
            let mut statement =
                transaction.prepare("SELECT workspace_root FROM workspace_configs")?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .filter(|stored_root| normalize_workspace_root(stored_root) == workspace_key)
                .collect::<Vec<_>>()
        };
        for stored_root in workspace_config_roots {
            transaction.execute(
                "DELETE FROM workspace_configs WHERE workspace_root = ?1",
                params![stored_root],
            )?;
        }

        transaction.commit()?;
        Ok(session_ids.len())
    }

    pub fn append_message(
        &self,
        session_id: Uuid,
        role: MessageRole,
        content: impl Into<String>,
    ) -> Result<Message, StoreError> {
        let message = Message {
            id: Uuid::new_v4(),
            session_id,
            role,
            content: content.into(),
            created_at: Utc::now(),
        };

        self.connection.execute(
            "INSERT INTO messages (id, session_id, role, content, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                message.id.to_string(),
                message.session_id.to_string(),
                serde_json::to_string(&message.role)?,
                message.content,
                message.created_at.to_rfc3339(),
            ],
        )?;

        Ok(message)
    }

    pub fn list_messages(&self, session_id: Uuid) -> Result<Vec<Message>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, session_id, role, content, created_at
             FROM messages WHERE session_id = ?1 ORDER BY created_at ASC, rowid ASC",
        )?;
        let rows = statement.query_map([session_id.to_string()], Self::row_to_message)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }
    pub fn get_context_compaction(
        &self,
        session_id: Uuid,
    ) -> Result<Option<ContextCompaction>, StoreError> {
        self.connection
            .query_row(
                "SELECT session_id, summary, compacted_message_count, updated_at
                 FROM context_compactions WHERE session_id = ?1",
                [session_id.to_string()],
                Self::row_to_context_compaction,
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn save_context_compaction(
        &self,
        compaction: ContextCompaction,
    ) -> Result<ContextCompaction, StoreError> {
        if compaction.summary.trim().is_empty() {
            return Err(StoreError::InvalidInput(
                "context compaction summary must not be empty".to_owned(),
            ));
        }
        self.connection.execute(
            "INSERT INTO context_compactions (session_id, summary, compacted_message_count, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(session_id) DO UPDATE SET
                summary = excluded.summary,
                compacted_message_count = excluded.compacted_message_count,
                updated_at = excluded.updated_at",
            params![
                compaction.session_id.to_string(),
                compaction.summary,
                compaction.compacted_message_count as i64,
                compaction.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(compaction)
    }

    /// Store one durable fact for a workspace. Duplicate content is ignored, not duplicated.
    pub fn save_local_memory(
        &self,
        workspace_root: &str,
        content: &str,
    ) -> Result<Option<LocalMemory>, StoreError> {
        let trimmed = content.trim();
        if trimmed.is_empty() {
            return Err(StoreError::InvalidInput(
                "local memory content must not be empty".to_owned(),
            ));
        }
        let memory = LocalMemory {
            id: Uuid::new_v4(),
            workspace_root: normalize_workspace_root(workspace_root),
            content: trimmed.to_owned(),
            created_at: Utc::now(),
        };
        let inserted = self.connection.execute(
            "INSERT INTO local_memories (id, workspace_root, content, created_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(workspace_root, content) DO NOTHING",
            params![
                memory.id.to_string(),
                memory.workspace_root,
                memory.content,
                memory.created_at.to_rfc3339(),
            ],
        )?;
        if inserted == 0 {
            return Ok(None);
        }
        Ok(Some(memory))
    }

    /// Oldest-first memories for a workspace, capped at `limit` most recent entries.
    pub fn list_local_memories(
        &self,
        workspace_root: &str,
        limit: usize,
    ) -> Result<Vec<LocalMemory>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, workspace_root, content, created_at
             FROM (
                SELECT id, workspace_root, content, created_at
                FROM local_memories
                WHERE workspace_root = ?1
                ORDER BY created_at DESC, id DESC
                LIMIT ?2
             )
             ORDER BY created_at ASC, id ASC",
        )?;
        let rows = statement.query_map(
            params![normalize_workspace_root(workspace_root), limit as i64],
            Self::row_to_local_memory,
        )?;
        rows.collect::<Result<Vec<_>, _>>().map_err(StoreError::from)
    }

    pub fn count_local_memories(&self, workspace_root: &str) -> Result<usize, StoreError> {
        let count: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM local_memories WHERE workspace_root = ?1",
            params![normalize_workspace_root(workspace_root)],
            |row| row.get(0),
        )?;
        Ok(count.max(0) as usize)
    }

    /// Delete every memory for one workspace and report how many rows were removed.
    pub fn clear_local_memories(&self, workspace_root: &str) -> Result<usize, StoreError> {
        let removed = self.connection.execute(
            "DELETE FROM local_memories WHERE workspace_root = ?1",
            params![normalize_workspace_root(workspace_root)],
        )?;
        Ok(removed)
    }

    /// Description produced earlier for the same delegate model and image
    /// payload, if any. Survives restarts so a stored screenshot is described
    /// once instead of once per process.
    pub fn get_vision_description(&self, cache_key: &str) -> Result<Option<String>, StoreError> {
        self.connection
            .query_row(
                "SELECT description FROM vision_descriptions WHERE cache_key = ?1",
                params![cache_key],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn save_vision_description(
        &self,
        cache_key: &str,
        description: &str,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO vision_descriptions (cache_key, description, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(cache_key) DO UPDATE SET
                description = excluded.description,
                updated_at = excluded.updated_at",
            params![cache_key, description, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn create_pending_action(
        &self,
        session_id: Uuid,
        tool_call: ToolCall,
    ) -> Result<PendingAction, StoreError> {
        let action = PendingAction {
            id: Uuid::new_v4(),
            session_id,
            tool_call,
            status: PendingActionStatus::Pending,
            created_at: Utc::now(),
            resolved_at: None,
        };
        self.connection.execute(
            "INSERT INTO pending_actions (id, session_id, tool_call, status, created_at, resolved_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                action.id.to_string(),
                action.session_id.to_string(),
                serde_json::to_string(&action.tool_call)?,
                serde_json::to_string(&action.status)?,
                action.created_at.to_rfc3339(),
                Option::<String>::None,
            ],
        )?;
        Ok(action)
    }

    pub fn get_pending_action(&self, id: Uuid) -> Result<Option<PendingAction>, StoreError> {
        self.connection.query_row(
            "SELECT id, session_id, tool_call, status, created_at, resolved_at FROM pending_actions WHERE id = ?1",
            [id.to_string()],
            Self::row_to_pending_action,
        ).optional().map_err(StoreError::from)
    }

    pub fn resolve_pending_action(
        &self,
        id: Uuid,
        status: PendingActionStatus,
    ) -> Result<Option<PendingAction>, StoreError> {
        let changed = self.connection.execute(
            "UPDATE pending_actions SET status = ?1, resolved_at = ?2 WHERE id = ?3 AND status = ?4",
            params![
                serde_json::to_string(&status)?,
                Utc::now().to_rfc3339(),
                id.to_string(),
                serde_json::to_string(&PendingActionStatus::Pending)?,
            ],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        self.get_pending_action(id)
    }

    pub fn reject_pending_actions(&self, session_id: Uuid) -> Result<(), StoreError> {
        let status = serde_json::to_string(&PendingActionStatus::Rejected)?;
        self.connection.execute(
            "UPDATE pending_actions
             SET status = ?1, resolved_at = ?2
             WHERE session_id = ?3 AND status = ?4",
            params![
                status,
                Utc::now().to_rfc3339(),
                session_id.to_string(),
                serde_json::to_string(&PendingActionStatus::Pending)?,
            ],
        )?;
        Ok(())
    }
    pub fn create_restore_point(
        &self,
        session_id: Uuid,
        path: &str,
        original_text: Option<&str>,
        applied_text: &str,
    ) -> Result<RestorePoint, StoreError> {
        let restore_point = RestorePoint {
            id: Uuid::new_v4(),
            session_id,
            path: path.to_owned(),
            original_text: original_text.map(str::to_owned),
            applied_text: Some(applied_text.to_owned()),
            created_at: Utc::now(),
        };
        self.connection.execute(
            "INSERT INTO restore_points (id, session_id, path, original_text, applied_text, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                restore_point.id.to_string(),
                restore_point.session_id.to_string(),
                restore_point.path,
                restore_point.original_text,
                restore_point.applied_text,
                restore_point.created_at.to_rfc3339(),
            ],
        )?;
        Ok(restore_point)
    }

    pub fn list_pending_actions(&self, session_id: Uuid) -> Result<Vec<PendingAction>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, session_id, tool_call, status, created_at, resolved_at
             FROM pending_actions WHERE session_id = ?1 ORDER BY created_at ASC, rowid ASC",
        )?;
        let rows = statement.query_map([session_id.to_string()], Self::row_to_pending_action)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn list_restore_points(&self, session_id: Uuid) -> Result<Vec<RestorePoint>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, session_id, path, original_text, applied_text, created_at
             FROM restore_points WHERE session_id = ?1 ORDER BY created_at DESC, rowid DESC",
        )?;
        let rows = statement.query_map([session_id.to_string()], Self::row_to_restore_point)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn get_restore_point(&self, id: Uuid) -> Result<Option<RestorePoint>, StoreError> {
        self.connection
            .query_row(
                "SELECT id, session_id, path, original_text, applied_text, created_at
             FROM restore_points WHERE id = ?1",
                [id.to_string()],
                Self::row_to_restore_point,
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn record_event(&self, event: &SessionEvent) -> Result<PersistedSessionEvent, StoreError> {
        let persisted = PersistedSessionEvent {
            id: Uuid::new_v4(),
            session_id: session_id_for_event(event),
            event: event.clone(),
            created_at: Utc::now(),
        };
        self.connection.execute(
            "INSERT INTO session_events (id, session_id, event, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![
                persisted.id.to_string(),
                persisted.session_id.to_string(),
                serde_json::to_string(&persisted.event)?,
                persisted.created_at.to_rfc3339(),
            ],
        )?;
        Ok(persisted)
    }

    pub fn list_events(&self, session_id: Uuid) -> Result<Vec<PersistedSessionEvent>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, session_id, event, created_at FROM session_events
             WHERE session_id = ?1 ORDER BY created_at ASC, rowid ASC",
        )?;
        let rows = statement.query_map([session_id.to_string()], Self::row_to_event)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn set_session_status(
        &self,
        id: Uuid,
        status: SessionStatus,
    ) -> Result<Option<Session>, StoreError> {
        let changed = self.connection.execute(
            "UPDATE sessions SET status = ?1, updated_at = ?2 WHERE id = ?3",
            params![
                serde_json::to_string(&status)?,
                Utc::now().to_rfc3339(),
                id.to_string(),
            ],
        )?;

        if changed == 0 {
            return Ok(None);
        }

        self.get_session(id)
    }

    /// Startup reconciliation for zombie rows. The chat turn runs inside a blocking invoke, so a
    /// `Running` row can never outlive the process that owned it. Anything still marked running
    /// when we open the database belongs to a previous process and will never emit another event,
    /// so flip it to `Cancelled` — a status the UI treats as continuable.
    pub fn reconcile_interrupted_sessions(&self) -> Result<usize, StoreError> {
        let changed = self.connection.execute(
            "UPDATE sessions SET status = ?1, updated_at = ?2 WHERE status = ?3",
            params![
                serde_json::to_string(&SessionStatus::Cancelled)?,
                Utc::now().to_rfc3339(),
                serde_json::to_string(&SessionStatus::Running)?,
            ],
        )?;

        Ok(changed)
    }

    pub fn set_session_title(
        &self,
        id: Uuid,
        title: impl Into<String>,
    ) -> Result<Option<Session>, StoreError> {
        let title = title.into();
        let changed = self.connection.execute(
            "UPDATE sessions SET title = ?1, updated_at = ?2 WHERE id = ?3",
            params![title, Utc::now().to_rfc3339(), id.to_string()],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        self.get_session(id)
    }

    /// Update the model used for subsequent turns of an existing session.
    pub fn set_session_model(
        &self,
        id: Uuid,
        model: impl Into<String>,
    ) -> Result<Option<Session>, StoreError> {
        let model = model.into().trim().to_owned();
        if model.is_empty() {
            return Err(StoreError::InvalidInput(
                "model must not be empty".to_owned(),
            ));
        }
        let changed = self.connection.execute(
            "UPDATE sessions SET model = ?1, updated_at = ?2 WHERE id = ?3",
            params![model, Utc::now().to_rfc3339(), id.to_string()],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        self.get_session(id)
    }

    /// Update the mode used for subsequent turns of an existing session.
    pub fn set_session_mode(
        &self,
        id: Uuid,
        mode: xcoding_protocol::Mode,
    ) -> Result<Option<Session>, StoreError> {
        let changed = self.connection.execute(
            "UPDATE sessions SET mode = ?1, updated_at = ?2 WHERE id = ?3",
            params![
                serde_json::to_string(&mode)?,
                Utc::now().to_rfc3339(),
                id.to_string()
            ],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        self.get_session(id)
    }

    pub fn first_user_message_content(
        &self,
        session_id: Uuid,
    ) -> Result<Option<String>, StoreError> {
        self.connection
            .query_row(
                "SELECT content FROM messages
                 WHERE session_id = ?1 AND role = ?2
                 ORDER BY created_at ASC, rowid ASC
                 LIMIT 1",
                params![
                    session_id.to_string(),
                    serde_json::to_string(&MessageRole::User)?
                ],
                |row| row.get(0),
            )
            .optional()
            .map_err(StoreError::from)
    }

    fn fill_session_title(&self, mut session: Session) -> Result<Session, StoreError> {
        if session
            .title
            .as_ref()
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
        {
            return Ok(session);
        }
        if let Some(content) = self.first_user_message_content(session.id)? {
            let derived = title_from_message(&content);
            if !derived.is_empty() {
                let _ = self.connection.execute(
                    "UPDATE sessions SET title = ?1 WHERE id = ?2 AND (title IS NULL OR trim(title) = '')",
                    params![derived.clone(), session.id.to_string()],
                )?;
                session.title = Some(derived);
            }
        }
        Ok(session)
    }

    fn migrate(&self) -> Result<(), StoreError> {
        self.connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY NOT NULL,
                workspace_root TEXT NOT NULL,
                mode TEXT NOT NULL,
                provider TEXT NOT NULL,
                model TEXT NOT NULL,
                status TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                title TEXT
            );

            CREATE TABLE IF NOT EXISTS messages (
                id TEXT PRIMARY KEY NOT NULL,
                session_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at TEXT NOT NULL,
                FOREIGN KEY (session_id) REFERENCES sessions(id)
            );

            CREATE TABLE IF NOT EXISTS pending_actions (
                id TEXT PRIMARY KEY NOT NULL,
                session_id TEXT NOT NULL,
                tool_call TEXT NOT NULL,
                status TEXT NOT NULL,
                created_at TEXT NOT NULL,
                resolved_at TEXT,
                FOREIGN KEY (session_id) REFERENCES sessions(id)
            );

            CREATE TABLE IF NOT EXISTS restore_points (
                id TEXT PRIMARY KEY NOT NULL,
                session_id TEXT NOT NULL,
                path TEXT NOT NULL,
                original_text TEXT,
                applied_text TEXT,
                created_at TEXT NOT NULL,
                FOREIGN KEY (session_id) REFERENCES sessions(id)
            );

            CREATE TABLE IF NOT EXISTS session_events (
                id TEXT PRIMARY KEY NOT NULL,
                session_id TEXT NOT NULL,
                event TEXT NOT NULL,
                created_at TEXT NOT NULL,
                FOREIGN KEY (session_id) REFERENCES sessions(id)
            );

            CREATE TABLE IF NOT EXISTS workspace_configs (
                workspace_root TEXT PRIMARY KEY NOT NULL,
                mode TEXT NOT NULL,
                provider TEXT NOT NULL,
                model TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS context_compactions (
                session_id TEXT PRIMARY KEY NOT NULL,
                summary TEXT NOT NULL,
                compacted_message_count INTEGER NOT NULL,
                updated_at TEXT NOT NULL,
                FOREIGN KEY (session_id) REFERENCES sessions(id)
            );

            CREATE TABLE IF NOT EXISTS local_memories (
                id TEXT PRIMARY KEY NOT NULL,
                workspace_root TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at TEXT NOT NULL,
                UNIQUE (workspace_root, content)
            );

            CREATE TABLE IF NOT EXISTS vision_descriptions (
                cache_key TEXT PRIMARY KEY NOT NULL,
                description TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );",
        )?;
        self.ensure_column("restore_points", "applied_text", "TEXT")?;
        Ok(())
    }

    fn ensure_column(&self, table: &str, column: &str, definition: &str) -> Result<(), StoreError> {
        let columns = {
            let mut statement = self
                .connection
                .prepare(&format!("PRAGMA table_info({table})"))?;
            let rows = statement.query_map([], |row| row.get::<_, String>(1))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        if !columns.iter().any(|existing| existing == column) {
            self.connection.execute_batch(&format!(
                "ALTER TABLE {table} ADD COLUMN {column} {definition}"
            ))?;
        }
        Ok(())
    }

    fn row_to_context_compaction(row: &rusqlite::Row<'_>) -> rusqlite::Result<ContextCompaction> {
        let session_id: String = row.get(0)?;
        let compacted_message_count: i64 = row.get(2)?;
        let updated_at: String = row.get(3)?;
        let parse = |error: StoreError| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        };
        Ok(ContextCompaction {
            session_id: Uuid::parse_str(&session_id)
                .map_err(|error| parse(StoreError::Identifier(error)))?,
            summary: row.get(1)?,
            compacted_message_count: usize::try_from(compacted_message_count).map_err(|_| {
                parse(StoreError::InvalidInput(
                    "stored compacted message count is negative or too large".to_owned(),
                ))
            })?,
            updated_at: DateTime::parse_from_rfc3339(&updated_at)
                .map_err(|error| parse(StoreError::Timestamp(error)))?
                .with_timezone(&Utc),
        })
    }

    fn row_to_local_memory(row: &rusqlite::Row<'_>) -> rusqlite::Result<LocalMemory> {
        let id: String = row.get(0)?;
        let created_at: String = row.get(3)?;
        let parse = |error: StoreError| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        };
        Ok(LocalMemory {
            id: Uuid::parse_str(&id).map_err(|error| parse(StoreError::Identifier(error)))?,
            workspace_root: row.get(1)?,
            content: row.get(2)?,
            created_at: DateTime::parse_from_rfc3339(&created_at)
                .map_err(|error| parse(StoreError::Timestamp(error)))?
                .with_timezone(&Utc),
        })
    }

    fn row_to_workspace_config(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkspaceConfig> {
        let mode: String = row.get(1)?;
        let updated_at: String = row.get(4)?;
        let parse = |error: StoreError| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        };
        Ok(WorkspaceConfig {
            workspace_root: row.get(0)?,
            mode: serde_json::from_str(&mode)
                .map_err(|error| parse(StoreError::InvalidData(error)))?,
            provider: row.get(2)?,
            model: row.get(3)?,
            command_allowlist: Vec::new(),
            command_denylist: Vec::new(),
            updated_at: DateTime::parse_from_rfc3339(&updated_at)
                .map_err(|error| parse(StoreError::Timestamp(error)))?
                .with_timezone(&Utc),
        })
    }

    fn row_to_pending_action(row: &rusqlite::Row<'_>) -> rusqlite::Result<PendingAction> {
        let id: String = row.get(0)?;
        let session_id: String = row.get(1)?;
        let tool_call: String = row.get(2)?;
        let status: String = row.get(3)?;
        let created_at: String = row.get(4)?;
        let resolved_at: Option<String> = row.get(5)?;
        let parse = |error: StoreError| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        };
        Ok(PendingAction {
            id: Uuid::parse_str(&id).map_err(|error| parse(StoreError::Identifier(error)))?,
            session_id: Uuid::parse_str(&session_id)
                .map_err(|error| parse(StoreError::Identifier(error)))?,
            tool_call: serde_json::from_str(&tool_call)
                .map_err(|error| parse(StoreError::InvalidData(error)))?,
            status: serde_json::from_str(&status)
                .map_err(|error| parse(StoreError::InvalidData(error)))?,
            created_at: DateTime::parse_from_rfc3339(&created_at)
                .map_err(|error| parse(StoreError::Timestamp(error)))?
                .with_timezone(&Utc),
            resolved_at: resolved_at
                .map(|value| {
                    DateTime::parse_from_rfc3339(&value)
                        .map(|timestamp| timestamp.with_timezone(&Utc))
                })
                .transpose()
                .map_err(|error| parse(StoreError::Timestamp(error)))?,
        })
    }

    fn row_to_restore_point(row: &rusqlite::Row<'_>) -> rusqlite::Result<RestorePoint> {
        let id: String = row.get(0)?;
        let session_id: String = row.get(1)?;
        let created_at: String = row.get(5)?;
        let parse = |error: StoreError| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        };
        Ok(RestorePoint {
            id: Uuid::parse_str(&id).map_err(|error| parse(StoreError::Identifier(error)))?,
            session_id: Uuid::parse_str(&session_id)
                .map_err(|error| parse(StoreError::Identifier(error)))?,
            path: row.get(2)?,
            original_text: row.get(3)?,
            applied_text: row.get(4)?,
            created_at: DateTime::parse_from_rfc3339(&created_at)
                .map_err(|error| parse(StoreError::Timestamp(error)))?
                .with_timezone(&Utc),
        })
    }

    fn row_to_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<PersistedSessionEvent> {
        let id: String = row.get(0)?;
        let session_id: String = row.get(1)?;
        let event: String = row.get(2)?;
        let created_at: String = row.get(3)?;
        let parse = |error: StoreError| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        };
        Ok(PersistedSessionEvent {
            id: Uuid::parse_str(&id).map_err(|error| parse(StoreError::Identifier(error)))?,
            session_id: Uuid::parse_str(&session_id)
                .map_err(|error| parse(StoreError::Identifier(error)))?,
            event: serde_json::from_str(&event)
                .map_err(|error| parse(StoreError::InvalidData(error)))?,
            created_at: DateTime::parse_from_rfc3339(&created_at)
                .map_err(|error| parse(StoreError::Timestamp(error)))?
                .with_timezone(&Utc),
        })
    }

    fn row_to_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<Session> {
        let id: String = row.get(0)?;
        let mode: String = row.get(2)?;
        let status: String = row.get(5)?;
        let created_at: String = row.get(6)?;
        let updated_at: String = row.get(7)?;

        let parse = |error: StoreError| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        };

        Ok(Session {
            id: Uuid::parse_str(&id).map_err(|error| parse(StoreError::Identifier(error)))?,
            workspace_root: row.get(1)?,
            mode: serde_json::from_str(&mode)
                .map_err(|error| parse(StoreError::InvalidData(error)))?,
            provider: row.get(3)?,
            model: row.get(4)?,
            status: serde_json::from_str(&status)
                .map_err(|error| parse(StoreError::InvalidData(error)))?,
            created_at: DateTime::parse_from_rfc3339(&created_at)
                .map_err(|error| parse(StoreError::Timestamp(error)))?
                .with_timezone(&Utc),
            updated_at: DateTime::parse_from_rfc3339(&updated_at)
                .map_err(|error| parse(StoreError::Timestamp(error)))?
                .with_timezone(&Utc),
            title: row.get(8)?,
        })
    }

    fn row_to_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<Message> {
        let id: String = row.get(0)?;
        let session_id: String = row.get(1)?;
        let role: String = row.get(2)?;
        let created_at: String = row.get(4)?;

        let parse = |error: StoreError| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        };

        Ok(Message {
            id: Uuid::parse_str(&id).map_err(|error| parse(StoreError::Identifier(error)))?,
            session_id: Uuid::parse_str(&session_id)
                .map_err(|error| parse(StoreError::Identifier(error)))?,
            role: serde_json::from_str(&role)
                .map_err(|error| parse(StoreError::InvalidData(error)))?,
            content: row.get(3)?,
            created_at: DateTime::parse_from_rfc3339(&created_at)
                .map_err(|error| parse(StoreError::Timestamp(error)))?
                .with_timezone(&Utc),
        })
    }
}

fn session_id_for_event(event: &SessionEvent) -> Uuid {
    match event {
        SessionEvent::TextDelta { session_id, .. }
        | SessionEvent::MessageCompleted { session_id, .. }
        | SessionEvent::Plan { session_id, .. }
        | SessionEvent::ToolStart { session_id, .. }
        | SessionEvent::ToolEnd { session_id, .. }
        | SessionEvent::PatchPreview { session_id, .. }
        | SessionEvent::ApprovalRequested { session_id, .. }
        | SessionEvent::RestorePointRolledBack { session_id, .. }
        | SessionEvent::SessionCancelled { session_id, .. }
        | SessionEvent::TaskCompleted { session_id, .. }
        | SessionEvent::Retrying { session_id, .. }
        | SessionEvent::ModelCall { session_id, .. }
        | SessionEvent::Error { session_id, .. }
        | SessionEvent::VisionDelegateStart { session_id, .. }
        | SessionEvent::VisionDelegateSuccess { session_id, .. }
        | SessionEvent::VisionDelegateFailed { session_id, .. } => *session_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xcoding_protocol::{Mode, ToolName};

    #[test]
    fn persists_workspace_configurations() {
        let store = SessionStore::in_memory().expect("in-memory database starts");
        let config = WorkspaceConfig {
            workspace_root: "D:/work/configured".to_owned(),
            mode: Mode::AutoEdit,
            provider: "openai".to_owned(),
            model: "configured-model".to_owned(),
            command_allowlist: Vec::new(),
            command_denylist: Vec::new(),
            updated_at: Utc::now(),
        };

        let saved = store
            .set_workspace_config(config.clone())
            .expect("config saves");
        let loaded = store
            .get_workspace_config("D:/work/configured")
            .expect("config loads")
            .expect("config exists");

        assert_eq!(saved, config);
        assert_eq!(loaded, config);
        assert!(
            store
                .get_workspace_config("D:/work/missing")
                .expect("missing config loads")
                .is_none()
        );
    }
    #[test]
    fn persists_sessions_and_messages() {
        let store = SessionStore::in_memory().expect("in-memory database starts");
        let session = store
            .create_session(CreateSessionParams {
                workspace_root: "D:/work/demo".to_owned(),
                mode: Mode::Ask,
                provider: "openai".to_owned(),
                model: "gpt-5.5".to_owned(),
                title: Some("First session".to_owned()),
            })
            .expect("session saves");

        let sessions = store
            .list_sessions(Some("D:/work/demo"))
            .expect("sessions load");
        assert_eq!(sessions, vec![session.clone()]);

        let message = store
            .append_message(session.id, MessageRole::User, "Ship it")
            .expect("message saves");
        let messages = store.list_messages(session.id).expect("messages load");
        let running = store
            .set_session_status(session.id, SessionStatus::Running)
            .expect("status updates")
            .expect("session exists");

        assert_eq!(messages, vec![message]);
        assert_eq!(running.status, SessionStatus::Running);
    }

    #[test]
    fn reconcile_interrupted_sessions_flips_running_to_cancelled() {
        let store = SessionStore::in_memory().expect("in-memory database starts");

        // One session ends up Running (zombie from a previous process).
        let zombie = store
            .create_session(CreateSessionParams {
                workspace_root: "D:/work/demo".to_owned(),
                mode: Mode::Ask,
                provider: "openai".to_owned(),
                model: "gpt-5.5".to_owned(),
                title: None,
            })
            .expect("session saves");
        store
            .set_session_status(zombie.id, SessionStatus::Running)
            .expect("status updates");

        // One session is legitimately Done — must not be touched.
        let done = store
            .create_session(CreateSessionParams {
                workspace_root: "D:/work/demo".to_owned(),
                mode: Mode::Ask,
                provider: "openai".to_owned(),
                model: "gpt-5.5".to_owned(),
                title: None,
            })
            .expect("session saves");
        store
            .set_session_status(done.id, SessionStatus::Done)
            .expect("status updates");

        let changed = store
            .reconcile_interrupted_sessions()
            .expect("reconcile succeeds");
        assert_eq!(changed, 1, "only the Running session should be flipped");

        let sessions = store
            .list_sessions(Some("D:/work/demo"))
            .expect("sessions load");
        let after_zombie = sessions.iter().find(|s| s.id == zombie.id).unwrap();
        let after_done = sessions.iter().find(|s| s.id == done.id).unwrap();
        assert_eq!(after_zombie.status, SessionStatus::Cancelled);
        assert_eq!(after_done.status, SessionStatus::Done);
    }

    #[test]
    fn persists_and_updates_context_compaction_without_replacing_messages() {
        let store = SessionStore::in_memory().expect("in-memory database starts");
        let session = store
            .create_session(CreateSessionParams {
                workspace_root: "D:/work/demo".to_owned(),
                mode: Mode::Ask,
                provider: "openai".to_owned(),
                model: "gpt-5.5".to_owned(),
                title: None,
            })
            .expect("session saves");
        let original = store
            .append_message(session.id, MessageRole::User, "keep original history")
            .expect("message saves");
        let first = ContextCompaction {
            session_id: session.id,
            summary: "# Goal\nInitial handoff".to_owned(),
            compacted_message_count: 1,
            updated_at: Utc::now(),
        };
        store
            .save_context_compaction(first)
            .expect("first compaction saves");
        let updated = ContextCompaction {
            session_id: session.id,
            summary: "# Goal\nUpdated handoff".to_owned(),
            compacted_message_count: 3,
            updated_at: Utc::now(),
        };
        store
            .save_context_compaction(updated.clone())
            .expect("compaction updates");

        assert_eq!(
            store
                .get_context_compaction(session.id)
                .expect("compaction loads"),
            Some(updated)
        );
        assert_eq!(
            store.list_messages(session.id).expect("messages load"),
            vec![original]
        );
    }

    #[test]
    fn scopes_local_memories_per_workspace_and_dedupes_content() {
        let store = SessionStore::in_memory().expect("in-memory database starts");

        let saved = store
            .save_local_memory("D:\\work\\demo", "  Uses pnpm workspaces.  ")
            .expect("memory saves");
        assert_eq!(
            saved.map(|memory| memory.content),
            Some("Uses pnpm workspaces.".to_owned())
        );
        // Same content under a differently spelled but equivalent root must not duplicate.
        assert_eq!(
            store
                .save_local_memory("D:/WORK/demo/", "Uses pnpm workspaces.")
                .expect("duplicate memory is ignored"),
            None
        );
        store
            .save_local_memory("D:\\work\\demo", "Tests run with cargo test.")
            .expect("second memory saves");
        store
            .save_local_memory("D:\\work\\other", "Unrelated project fact.")
            .expect("other workspace memory saves");

        let demo = store
            .list_local_memories("D:\\work\\demo", 10)
            .expect("memories load");
        assert_eq!(
            demo.iter().map(|m| m.content.as_str()).collect::<Vec<_>>(),
            vec!["Uses pnpm workspaces.", "Tests run with cargo test."]
        );
        assert_eq!(
            store
                .count_local_memories("D:\\work\\other")
                .expect("count loads"),
            1
        );

        assert_eq!(
            store
                .clear_local_memories("D:\\work\\demo")
                .expect("memories clear"),
            2
        );
        assert!(
            store
                .list_local_memories("D:\\work\\demo", 10)
                .expect("memories reload")
                .is_empty()
        );
        // Clearing one workspace must leave other workspaces intact.
        assert_eq!(
            store
                .count_local_memories("D:\\work\\other")
                .expect("other count loads"),
            1
        );
        assert!(store.save_local_memory("D:\\work\\demo", "   ").is_err());
    }

    #[test]
    fn updates_session_model_for_later_turns() {
        let store = SessionStore::in_memory().expect("store starts");
        let session = store
            .create_session(CreateSessionParams {
                workspace_root: "D:/work/demo".to_owned(),
                mode: xcoding_protocol::Mode::Ask,
                provider: "openai".to_owned(),
                model: "gpt-5.5".to_owned(),
                title: Some("demo".to_owned()),
            })
            .expect("session created");
        let updated = store
            .set_session_model(session.id, "grok-4.5")
            .expect("model update")
            .expect("session exists");
        assert_eq!(updated.model, "grok-4.5");
        let reloaded = store
            .get_session(session.id)
            .expect("load")
            .expect("exists");
        assert_eq!(reloaded.model, "grok-4.5");
        let empty = store.set_session_model(session.id, "   ");
        assert!(empty.is_err(), "empty model rejected");
    }

    #[test]
    fn deletes_all_workspace_sessions_and_preserves_other_projects() {
        let store = SessionStore::in_memory().expect("in-memory database starts");
        let target_a = store
            .create_session(CreateSessionParams {
                workspace_root: "D:/Work/Demo".to_owned(),
                mode: Mode::Ask,
                provider: "openai".to_owned(),
                model: "gpt-5.5".to_owned(),
                title: Some("Target A".to_owned()),
            })
            .expect("first target session saves");
        let target_b = store
            .create_session(CreateSessionParams {
                workspace_root: "d:\\work\\demo\\".to_owned(),
                mode: Mode::Ask,
                provider: "openai".to_owned(),
                model: "gpt-5.5".to_owned(),
                title: Some("Target B".to_owned()),
            })
            .expect("second target session saves");
        let other = store
            .create_session(CreateSessionParams {
                workspace_root: "D:/Work/Other".to_owned(),
                mode: Mode::Ask,
                provider: "openai".to_owned(),
                model: "gpt-5.5".to_owned(),
                title: Some("Keep me".to_owned()),
            })
            .expect("other session saves");

        for session in [&target_a, &target_b, &other] {
            store
                .append_message(session.id, MessageRole::User, "hello")
                .expect("message saves");
        }
        store
            .create_pending_action(
                target_a.id,
                ToolCall {
                    id: "workspace-call".to_owned(),
                    name: ToolName::ListDir,
                    arguments: serde_json::json!({"path": "."}),
                },
            )
            .expect("pending action");
        store
            .create_restore_point(target_a.id, "README.md", Some("old"), "new")
            .expect("restore point");
        store
            .save_context_compaction(ContextCompaction {
                session_id: target_a.id,
                summary: "remove workspace context".to_owned(),
                compacted_message_count: 1,
                updated_at: Utc::now(),
            })
            .expect("context compaction");
        store
            .record_event(&SessionEvent::MessageCompleted {
                session_id: target_a.id,
                message: Message {
                    id: Uuid::new_v4(),
                    session_id: target_a.id,
                    role: MessageRole::Assistant,
                    content: "done".to_owned(),
                    created_at: Utc::now(),
                },
            })
            .expect("event");
        for root in ["D:/Work/Demo", "D:/Work/Other"] {
            store
                .set_workspace_config(WorkspaceConfig {
                    workspace_root: root.to_owned(),
                    mode: Mode::Ask,
                    provider: "openai".to_owned(),
                    model: "gpt-5.5".to_owned(),
                    command_allowlist: Vec::new(),
                    command_denylist: Vec::new(),
                    updated_at: Utc::now(),
                })
                .expect("workspace config saves");
        }

        assert_eq!(
            store
                .delete_workspace_sessions("\\\\?\\d:\\WORK\\demo\\")
                .expect("workspace delete"),
            2
        );
        let remaining = store.list_sessions(None).expect("remaining sessions");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, other.id);
        for session in [&target_a, &target_b] {
            assert!(store.get_session(session.id).expect("lookup").is_none());
            assert!(
                store
                    .list_messages(session.id)
                    .expect("messages")
                    .is_empty()
            );
            assert!(
                store
                    .list_pending_actions(session.id)
                    .expect("actions")
                    .is_empty()
            );
            assert!(
                store
                    .list_restore_points(session.id)
                    .expect("restore points")
                    .is_empty()
            );
            assert!(store.list_events(session.id).expect("events").is_empty());
            assert!(
                store
                    .get_context_compaction(session.id)
                    .expect("context compaction")
                    .is_none()
            );
        }
        assert!(
            store
                .get_workspace_config("D:/Work/Demo")
                .expect("target config lookup")
                .is_none()
        );
        assert!(
            store
                .get_workspace_config("D:/Work/Other")
                .expect("other config lookup")
                .is_some()
        );
        assert_eq!(
            store
                .delete_workspace_sessions("D:/Work/Demo")
                .expect("repeated delete"),
            0
        );
    }

    #[test]
    fn deletes_session_and_related_rows() {
        let store = SessionStore::in_memory().expect("in-memory database starts");
        let session = store
            .create_session(CreateSessionParams {
                workspace_root: "D:/work/demo".to_owned(),
                mode: Mode::Ask,
                provider: "openai".to_owned(),
                model: "gpt-5.5".to_owned(),
                title: Some("Delete me".to_owned()),
            })
            .expect("session saves");
        store
            .append_message(session.id, MessageRole::User, "hello")
            .expect("message saves");
        store
            .create_pending_action(
                session.id,
                ToolCall {
                    id: "call-1".to_owned(),
                    name: ToolName::ListDir,
                    arguments: serde_json::json!({"path": "."}),
                },
            )
            .expect("pending action");
        store
            .create_restore_point(session.id, "README.md", Some("old"), "new")
            .expect("restore point");
        store
            .save_context_compaction(ContextCompaction {
                session_id: session.id,
                summary: "# Goal\nDelete me too".to_owned(),
                compacted_message_count: 1,
                updated_at: Utc::now(),
            })
            .expect("context compaction");
        let assistant = Message {
            id: Uuid::new_v4(),
            session_id: session.id,
            role: MessageRole::Assistant,
            content: "done".to_owned(),
            created_at: Utc::now(),
        };
        store
            .record_event(&SessionEvent::MessageCompleted {
                session_id: session.id,
                message: assistant,
            })
            .expect("event");

        assert!(store.delete_session(session.id).expect("delete"));
        assert!(store.get_session(session.id).expect("lookup").is_none());
        assert!(
            store
                .list_messages(session.id)
                .expect("messages")
                .is_empty()
        );
        assert!(
            store
                .list_pending_actions(session.id)
                .expect("actions")
                .is_empty()
        );
        assert!(
            store
                .list_restore_points(session.id)
                .expect("restore")
                .is_empty()
        );
        assert!(store.list_events(session.id).expect("events").is_empty());
        assert!(
            store
                .get_context_compaction(session.id)
                .expect("compaction")
                .is_none()
        );
        assert!(!store.delete_session(session.id).expect("second delete"));
    }
}
