//! SQLite persistence for XCoding sessions and messages.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, FixedOffset, Utc};
use rusqlite::{Connection, MAIN_DB, OptionalExtension, params};
use thiserror::Error;
use uuid::Uuid;
use xcoding_protocol::{
    ContextCompaction, ContextWindow, CreateSessionParams, KeyCallReport, LocalMemory,
    MAX_CONTEXT_TOOL_LIMIT, MAX_LOCAL_MEMORY_CHARS, MIN_CONTEXT_TOOL_LIMIT, Message, MessageRole,
    ModelCallReport, ModelCallReportEntry, PendingAction, PendingActionStatus,
    PersistedSessionEvent, ProviderCallReport, RestorePoint, Session, SessionEvent,
    SessionMessagePage, SessionMessagePreview, SessionStatus, ToolCall, WorkspaceConfig,
};

const TOOL_OUTPUT_ATTACHMENT_THRESHOLD: usize = 256 * 1024;
const TOOL_OUTPUT_PREVIEW_BYTES: usize = 16 * 1024;
const SESSION_MESSAGE_PAGE_SIZE: usize = 50;

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
    #[error("file operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid input: {0}")]
    InvalidInput(String),
}

pub struct SessionStore {
    connection: Connection,
    attachment_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct RedactionReport {
    pub messages_scanned: usize,
    pub messages_redacted: usize,
    pub events_redacted: usize,
    pub compactions_redacted: usize,
    pub memories_redacted: usize,
}

/// A cached delegate description plus the delegate model that produced it.
#[derive(Debug, Clone)]
pub struct StoredVisionDescription {
    pub description: String,
    /// Empty for rows written before the model became a column.
    pub delegate_model: String,
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

fn attachment_dir_for_database(path: &Path) -> PathBuf {
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("xcoding");
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    parent.join(format!("{stem}.tool-outputs"))
}

fn copy_dir_all(source: &Path, destination: &Path) -> Result<(), StoreError> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&source_path, &destination_path)?;
        } else {
            fs::copy(source_path, destination_path)?;
        }
    }
    Ok(())
}

fn tool_output_preview(bytes: &[u8]) -> String {
    let end = bytes.len().min(TOOL_OUTPUT_PREVIEW_BYTES);
    let prefix = String::from_utf8_lossy(&bytes[..end]);
    format!("{prefix}\n\n[XCoding saved the full tool output as a local attachment.]")
}

fn write_attachment_atomically(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    let temporary = path.with_extension(format!("redact-{}", Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(path)?;
    }
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(StoreError::Io(error));
    }
    Ok(())
}

fn preview_to_message(preview: SessionMessagePreview) -> Message {
    Message {
        id: preview.id,
        session_id: preview.session_id,
        role: preview.role,
        content: preview.content,
        created_at: preview.created_at,
    }
}

fn remove_files(paths: Vec<PathBuf>) {
    for path in paths {
        let _ = fs::remove_file(path);
    }
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
        let store = Self {
            connection,
            attachment_dir: None,
        };
        store.migrate()?;
        Ok(store)
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref();
        let connection = Connection::open(path)?;
        Self::configure_connection(&connection)?;
        let store = Self {
            connection,
            attachment_dir: Some(attachment_dir_for_database(path)),
        };
        store.migrate()?;
        Ok(store)
    }

    pub fn backup_to(&self, path: impl AsRef<Path>) -> Result<(), StoreError> {
        let path = path.as_ref();
        if path.exists() {
            return Err(StoreError::InvalidInput(format!(
                "backup path already exists: {}",
                path.display()
            )));
        }
        let attachment_destination = attachment_dir_for_database(path);
        if self.attachment_dir.as_ref().is_some_and(|dir| dir.exists())
            && attachment_destination.exists()
        {
            return Err(StoreError::InvalidInput(format!(
                "backup attachment directory already exists: {}",
                attachment_destination.display()
            )));
        }
        let backup_path = path.to_string_lossy().replace('\'', "''");
        self.connection
            .execute_batch(&format!("VACUUM INTO '{}'", backup_path))?;
        if let Some(source) = self.attachment_dir.as_ref().filter(|dir| dir.exists()) {
            copy_dir_all(source, &attachment_destination)?;
        }
        Ok(())
    }

    pub fn redact_historical_secrets(
        &self,
        redact: &dyn Fn(&str) -> String,
    ) -> Result<RedactionReport, StoreError> {
        let transaction = self.connection.unchecked_transaction()?;
        let mut report = RedactionReport::default();
        let mut redacted_message_ids = BTreeSet::new();

        {
            let mut statement = transaction.prepare("SELECT id, content FROM messages")?;
            let rows = statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (id, content) = row?;
                report.messages_scanned += 1;
                let redacted = redact(&content);
                if redacted != content {
                    transaction.execute(
                        "UPDATE messages SET content = ?1 WHERE id = ?2",
                        params![redacted, id],
                    )?;
                    report.messages_redacted += 1;
                    redacted_message_ids.insert(id);
                }
            }
        }

        if let Some(root) = self.attachment_dir.as_ref() {
            let attachments = {
                let mut statement = transaction.prepare(
                    "SELECT message_id, relative_path, byte_length FROM tool_output_attachments",
                )?;
                let rows = statement.query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                })?;
                rows.collect::<Result<Vec<_>, _>>()?
            };
            for (message_id, relative_path, _) in attachments {
                let path = root.join(relative_path);
                let content = fs::read_to_string(&path)?;
                let redacted = redact(&content);
                if redacted == content {
                    continue;
                }
                write_attachment_atomically(&path, redacted.as_bytes())?;
                transaction.execute(
                    "UPDATE tool_output_attachments SET byte_length = ?1 WHERE message_id = ?2",
                    params![redacted.len() as i64, message_id],
                )?;
                if redacted_message_ids.insert(message_id) {
                    report.messages_redacted += 1;
                }
            }
        }

        {
            let mut statement = transaction.prepare("SELECT id, event FROM session_events")?;
            let rows = statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (id, content) = row?;
                let redacted = redact(&content);
                if redacted != content {
                    transaction.execute(
                        "UPDATE session_events SET event = ?1 WHERE id = ?2",
                        params![redacted, id],
                    )?;
                    report.events_redacted += 1;
                }
            }
        }

        {
            let mut statement =
                transaction.prepare("SELECT session_id, summary FROM context_compactions")?;
            let rows = statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (session_id, content) = row?;
                let redacted = redact(&content);
                if redacted != content {
                    transaction.execute(
                        "UPDATE context_compactions SET summary = ?1 WHERE session_id = ?2",
                        params![redacted, session_id],
                    )?;
                    report.compactions_redacted += 1;
                }
            }
        }

        {
            let mut statement = transaction.prepare("SELECT id, content FROM local_memories")?;
            let rows = statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (id, content) = row?;
                let redacted = redact(&content);
                if redacted != content {
                    transaction.execute(
                        "UPDATE local_memories SET content = ?1 WHERE id = ?2",
                        params![redacted, id],
                    )?;
                    report.memories_redacted += 1;
                }
            }
        }

        transaction.commit()?;
        Ok(report)
    }

    pub fn create_session(&self, params: CreateSessionParams) -> Result<Session, StoreError> {
        let model = params.model.trim().to_owned();
        if model.is_empty() {
            return Err(StoreError::InvalidInput(
                "model must not be empty".to_owned(),
            ));
        }
        let now = Utc::now();
        let session = Session {
            id: Uuid::new_v4(),
            workspace_root: params.workspace_root,
            mode: params.mode,
            provider: params.provider,
            model,
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
        let attachment_paths = self.attachment_paths_for_session_text(&id)?;
        self.connection.execute(
            "DELETE FROM context_windows WHERE session_id = ?1",
            params![id],
        )?;
        self.connection.execute(
            "DELETE FROM tool_output_attachments WHERE session_id = ?1",
            params![id],
        )?;
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
        // NOTE: vision_descriptions are NOT deleted with the session because
        // they are keyed by image content hash and reusable across sessions.
        let deleted = self
            .connection
            .execute("DELETE FROM sessions WHERE id = ?1", params![id])?;
        remove_files(attachment_paths);
        if deleted > 0 {
            // VACUUM rewrites the SQLite file so deleting a large session also reclaims disk space.
            self.connection.execute_batch("VACUUM")?;
        }
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
        let attachment_paths = session_ids
            .iter()
            .map(|id| self.attachment_paths_for_session_text(id))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();

        for id in &session_ids {
            transaction.execute(
                "DELETE FROM context_windows WHERE session_id = ?1",
                params![id],
            )?;
            transaction.execute("DELETE FROM messages WHERE session_id = ?1", params![id])?;
            transaction.execute(
                "DELETE FROM tool_output_attachments WHERE session_id = ?1",
                params![id],
            )?;
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
            // NOTE: vision_descriptions are preserved across sessions.
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
        remove_files(attachment_paths);
        Ok(session_ids.len())
    }

    fn attachment_paths_for_session_text(
        &self,
        session_id: &str,
    ) -> Result<Vec<PathBuf>, StoreError> {
        let Some(root) = self.attachment_dir.as_ref() else {
            return Ok(Vec::new());
        };
        let mut statement = self
            .connection
            .prepare("SELECT relative_path FROM tool_output_attachments WHERE session_id = ?1")?;
        let rows = statement.query_map([session_id], |row| row.get::<_, String>(0))?;
        Ok(rows
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|relative| root.join(relative))
            .collect())
    }

    pub fn append_message(
        &self,
        session_id: Uuid,
        role: MessageRole,
        content: impl Into<String>,
    ) -> Result<Message, StoreError> {
        let content = content.into();
        let mut message = Message {
            id: Uuid::new_v4(),
            session_id,
            role,
            content: content.clone(),
            created_at: Utc::now(),
        };

        let attachment_path = if message.role == MessageRole::Tool
            && content.len() > TOOL_OUTPUT_ATTACHMENT_THRESHOLD
            && self.attachment_dir.is_some()
        {
            let path =
                self.write_tool_output_attachment(session_id, message.id, content.as_bytes())?;
            message.content = tool_output_preview(content.as_bytes());
            Some(path)
        } else {
            None
        };

        if let Err(error) = self.connection.execute(
            "INSERT INTO messages (id, session_id, role, content, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                message.id.to_string(),
                message.session_id.to_string(),
                serde_json::to_string(&message.role)?,
                message.content,
                message.created_at.to_rfc3339(),
            ],
        ) {
            if let Some(path) = attachment_path.as_ref() {
                let _ = fs::remove_file(path);
            }
            return Err(StoreError::Database(error));
        }
        if let Some(path) = attachment_path {
            let relative_path = self.relative_attachment_path(session_id, message.id)?;
            if let Err(error) = self.connection.execute(
                "INSERT INTO tool_output_attachments (message_id, session_id, relative_path, byte_length)
                 VALUES (?1, ?2, ?3, ?4)",
                params![message.id.to_string(), session_id.to_string(), relative_path, content.len() as i64],
            ) {
                let _ = fs::remove_file(path);
                let _ = self.connection.execute("DELETE FROM messages WHERE id = ?1", [message.id.to_string()]);
                return Err(StoreError::Database(error));
            }
        }

        Ok(message)
    }

    pub fn list_messages(&self, session_id: Uuid) -> Result<Vec<Message>, StoreError> {
        let total = self.count_messages(session_id)?;
        let mut messages = Vec::with_capacity(total);
        for offset in (0..total).step_by(SESSION_MESSAGE_PAGE_SIZE) {
            let page =
                self.list_message_previews_page(session_id, offset, SESSION_MESSAGE_PAGE_SIZE)?;
            messages.extend(page.messages.into_iter().map(preview_to_message));
        }
        Ok(messages)
    }

    pub fn list_message_previews_page(
        &self,
        session_id: Uuid,
        offset: usize,
        limit: usize,
    ) -> Result<SessionMessagePage, StoreError> {
        validate_context_tool_page(offset, limit)?;
        let total_messages = self.count_messages(session_id)?;
        let mut statement = self.connection.prepare(
            "SELECT rowid, id, session_id, role, created_at
             FROM messages WHERE session_id = ?1
             ORDER BY created_at ASC, rowid ASC LIMIT ?2 OFFSET ?3",
        )?;
        let rows = statement.query_map(
            params![session_id.to_string(), limit as i64, offset as i64],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )?;
        let rows = rows.collect::<Result<Vec<_>, _>>()?;
        let mut messages = Vec::with_capacity(rows.len());
        for (rowid, id, session_id, role, created_at) in rows {
            let id = Uuid::parse_str(&id)?;
            let session_id = Uuid::parse_str(&session_id)?;
            let role: MessageRole = serde_json::from_str(&role)?;
            let created_at = DateTime::parse_from_rfc3339(&created_at)?.with_timezone(&Utc);
            messages.push(self.message_preview(rowid, id, session_id, role, created_at)?);
        }
        Ok(SessionMessagePage {
            messages,
            offset,
            total_messages,
            has_older_messages: offset > 0,
        })
    }

    pub fn open_tool_output_attachment(
        &self,
        session_id: Uuid,
        message_id: Uuid,
    ) -> Result<PathBuf, StoreError> {
        if let Some(path) = self.find_tool_output_attachment(session_id, message_id)? {
            return Ok(path);
        }
        let rowid: Option<i64> = self
            .connection
            .query_row(
                "SELECT rowid FROM messages WHERE session_id = ?1 AND id = ?2 AND role = ?3",
                params![
                    session_id.to_string(),
                    message_id.to_string(),
                    serde_json::to_string(&MessageRole::Tool)?
                ],
                |row| row.get(0),
            )
            .optional()?;
        let rowid = rowid.ok_or_else(|| {
            StoreError::InvalidInput("tool output attachment not found".to_owned())
        })?;
        self.migrate_legacy_tool_output(rowid, session_id, message_id)?;
        self.find_tool_output_attachment(session_id, message_id)?
            .ok_or_else(|| {
                StoreError::InvalidInput("tool output is too small to export".to_owned())
            })
    }

    fn message_preview(
        &self,
        rowid: i64,
        id: Uuid,
        session_id: Uuid,
        role: MessageRole,
        created_at: DateTime<Utc>,
    ) -> Result<SessionMessagePreview, StoreError> {
        if let Some((path, byte_length)) =
            self.find_tool_output_attachment_with_size(session_id, id)?
        {
            let content: String = self.connection.query_row(
                "SELECT content FROM messages WHERE id = ?1",
                [id.to_string()],
                |row| row.get(0),
            )?;
            let _ = path;
            return Ok(SessionMessagePreview {
                id,
                session_id,
                role,
                content,
                created_at,
                content_bytes: byte_length,
                full_output_available: true,
            });
        }

        let mut blob = self
            .connection
            .blob_open(MAIN_DB, "messages", "content", rowid, true)?;
        let byte_length = blob.len();
        if role == MessageRole::Tool && byte_length > TOOL_OUTPUT_ATTACHMENT_THRESHOLD {
            drop(blob);
            self.migrate_legacy_tool_output(rowid, session_id, id)?;
            let content: String = self.connection.query_row(
                "SELECT content FROM messages WHERE id = ?1",
                [id.to_string()],
                |row| row.get(0),
            )?;
            return Ok(SessionMessagePreview {
                id,
                session_id,
                role,
                content,
                created_at,
                content_bytes: byte_length as u64,
                full_output_available: true,
            });
        }

        let preview_size = byte_length.min(TOOL_OUTPUT_PREVIEW_BYTES);
        let mut bytes = vec![0; preview_size];
        blob.read_exact(&mut bytes)?;
        let content = if byte_length > TOOL_OUTPUT_PREVIEW_BYTES {
            format!(
                "{}\n\n[Message preview truncated.]",
                String::from_utf8_lossy(&bytes)
            )
        } else {
            String::from_utf8(bytes).map_err(|error| StoreError::InvalidInput(error.to_string()))?
        };
        Ok(SessionMessagePreview {
            id,
            session_id,
            role,
            content,
            created_at,
            content_bytes: byte_length as u64,
            full_output_available: false,
        })
    }

    fn migrate_legacy_tool_output(
        &self,
        rowid: i64,
        session_id: Uuid,
        message_id: Uuid,
    ) -> Result<(), StoreError> {
        if self
            .find_tool_output_attachment(session_id, message_id)?
            .is_some()
        {
            return Ok(());
        }
        let Some(_) = self.attachment_dir else {
            return Ok(());
        };
        let (path, byte_length, prefix) =
            self.write_tool_output_from_blob(rowid, session_id, message_id)?;
        if byte_length <= TOOL_OUTPUT_ATTACHMENT_THRESHOLD as u64 {
            let _ = fs::remove_file(path);
            return Ok(());
        }
        let content = tool_output_preview(&prefix);
        let relative_path = self.relative_attachment_path(session_id, message_id)?;
        let persisted = (|| -> Result<(), StoreError> {
            let transaction = self.connection.unchecked_transaction()?;
            transaction.execute(
                "INSERT INTO tool_output_attachments (message_id, session_id, relative_path, byte_length)
                 VALUES (?1, ?2, ?3, ?4)",
                params![message_id.to_string(), session_id.to_string(), relative_path, byte_length as i64],
            )?;
            transaction.execute(
                "UPDATE messages SET content = ?1 WHERE id = ?2",
                params![content, message_id.to_string()],
            )?;
            transaction.commit()?;
            Ok(())
        })();
        if let Err(error) = persisted {
            let _ = fs::remove_file(path);
            return Err(error);
        }
        Ok(())
    }

    fn write_tool_output_attachment(
        &self,
        session_id: Uuid,
        message_id: Uuid,
        bytes: &[u8],
    ) -> Result<PathBuf, StoreError> {
        let final_path = self.attachment_path(session_id, message_id)?;
        let parent = final_path
            .parent()
            .ok_or_else(|| StoreError::InvalidInput("attachment path has no parent".to_owned()))?;
        fs::create_dir_all(parent)?;
        let temp_path = final_path.with_extension(format!("txt.tmp-{}", Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temp_path, &final_path)?;
        Ok(final_path)
    }

    fn write_tool_output_from_blob(
        &self,
        rowid: i64,
        session_id: Uuid,
        message_id: Uuid,
    ) -> Result<(PathBuf, u64, Vec<u8>), StoreError> {
        let final_path = self.attachment_path(session_id, message_id)?;
        let parent = final_path
            .parent()
            .ok_or_else(|| StoreError::InvalidInput("attachment path has no parent".to_owned()))?;
        fs::create_dir_all(parent)?;
        let temp_path = final_path.with_extension(format!("txt.tmp-{}", Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;
        let mut blob = self
            .connection
            .blob_open(MAIN_DB, "messages", "content", rowid, true)?;
        let byte_length = blob.len() as u64;
        let mut prefix = Vec::with_capacity(TOOL_OUTPUT_PREVIEW_BYTES);
        let mut buffer = vec![0; 1024 * 1024];
        loop {
            let read = blob.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            file.write_all(&buffer[..read])?;
            let needed = TOOL_OUTPUT_PREVIEW_BYTES
                .saturating_sub(prefix.len())
                .min(read);
            prefix.extend_from_slice(&buffer[..needed]);
        }
        drop(blob);
        file.sync_all()?;
        fs::rename(&temp_path, &final_path)?;
        Ok((final_path, byte_length, prefix))
    }

    fn attachment_path(&self, session_id: Uuid, message_id: Uuid) -> Result<PathBuf, StoreError> {
        let root = self.attachment_dir.as_ref().ok_or_else(|| {
            StoreError::InvalidInput(
                "attachments are unavailable for an in-memory store".to_owned(),
            )
        })?;
        Ok(root
            .join(session_id.to_string())
            .join(format!("{message_id}.txt")))
    }

    fn relative_attachment_path(
        &self,
        session_id: Uuid,
        message_id: Uuid,
    ) -> Result<String, StoreError> {
        Ok(format!("{session_id}/{message_id}.txt"))
    }

    fn find_tool_output_attachment(
        &self,
        session_id: Uuid,
        message_id: Uuid,
    ) -> Result<Option<PathBuf>, StoreError> {
        Ok(self
            .find_tool_output_attachment_with_size(session_id, message_id)?
            .map(|(path, _)| path))
    }

    fn find_tool_output_attachment_with_size(
        &self,
        session_id: Uuid,
        message_id: Uuid,
    ) -> Result<Option<(PathBuf, u64)>, StoreError> {
        let entry: Option<(String, i64)> = self.connection.query_row(
            "SELECT relative_path, byte_length FROM tool_output_attachments WHERE session_id = ?1 AND message_id = ?2",
            params![session_id.to_string(), message_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()?;
        entry
            .map(|(relative, length)| {
                let root = self.attachment_dir.as_ref().ok_or_else(|| {
                    StoreError::InvalidInput(
                        "attachments are unavailable for an in-memory store".to_owned(),
                    )
                })?;
                Ok((root.join(relative), u64::try_from(length).unwrap_or(0)))
            })
            .transpose()
    }

    pub fn count_messages(&self, session_id: Uuid) -> Result<usize, StoreError> {
        let count: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM messages WHERE session_id = ?1",
            params![session_id.to_string()],
            |row| row.get(0),
        )?;
        usize::try_from(count).map_err(|_| {
            StoreError::InvalidInput("stored message count is negative or too large".to_owned())
        })
    }

    pub fn latest_context_window_start(&self, session_id: Uuid) -> Result<usize, StoreError> {
        let start: Option<i64> = self.connection.query_row(
            "SELECT MAX(start_message_count) FROM context_windows WHERE session_id = ?1",
            params![session_id.to_string()],
            |row| row.get(0),
        )?;
        usize::try_from(start.unwrap_or(0)).map_err(|_| {
            StoreError::InvalidInput("stored context window boundary is invalid".to_owned())
        })
    }

    pub fn create_context_window(
        &self,
        session_id: Uuid,
        start_message_count: usize,
    ) -> Result<usize, StoreError> {
        if start_message_count == 0 {
            return Err(StoreError::InvalidInput(
                "the initial context window is implicit and cannot be persisted".to_owned(),
            ));
        }
        let session_id = session_id.to_string();
        let boundary = i64::try_from(start_message_count)
            .map_err(|_| StoreError::InvalidInput("message count is too large".to_owned()))?;
        if let Some(window_index) = self
            .connection
            .query_row(
                "SELECT window_index FROM context_windows
                 WHERE session_id = ?1 AND start_message_count = ?2",
                params![session_id, boundary],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
        {
            return usize::try_from(window_index).map_err(|_| {
                StoreError::InvalidInput("stored context window index is invalid".to_owned())
            });
        }

        let latest_start: Option<i64> = self.connection.query_row(
            "SELECT MAX(start_message_count) FROM context_windows WHERE session_id = ?1",
            params![session_id],
            |row| row.get(0),
        )?;
        if latest_start.is_some_and(|latest| boundary < latest) {
            return Err(StoreError::InvalidInput(
                "context window boundary cannot move backward".to_owned(),
            ));
        }

        let role: Option<String> = self
            .connection
            .query_row(
                "SELECT role FROM messages WHERE session_id = ?1
                 ORDER BY created_at ASC, rowid ASC LIMIT 1 OFFSET ?2",
                params![session_id, boundary],
                |row| row.get(0),
            )
            .optional()?;
        let Some(role) = role else {
            return Err(StoreError::InvalidInput(
                "context window boundary must reference an existing message".to_owned(),
            ));
        };
        if serde_json::from_str::<MessageRole>(&role)? != MessageRole::User {
            return Err(StoreError::InvalidInput(
                "context window boundary must reference a user message".to_owned(),
            ));
        }

        let next_index: i64 = self.connection.query_row(
            "SELECT COALESCE(MAX(window_index), 0) + 1 FROM context_windows WHERE session_id = ?1",
            params![session_id],
            |row| row.get(0),
        )?;
        self.connection.execute(
            "INSERT INTO context_windows (session_id, window_index, start_message_count, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                session_id,
                next_index,
                boundary,
                Utc::now().to_rfc3339(),
            ],
        )?;
        usize::try_from(next_index).map_err(|_| {
            StoreError::InvalidInput("stored context window index is invalid".to_owned())
        })
    }

    pub fn list_messages_page(
        &self,
        session_id: Uuid,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Message>, StoreError> {
        Ok(self
            .list_message_previews_page(session_id, offset, limit)?
            .messages
            .into_iter()
            .map(preview_to_message)
            .collect())
    }

    pub fn get_session_message(
        &self,
        session_id: Uuid,
        message_id: Uuid,
    ) -> Result<Option<Message>, StoreError> {
        let row: Option<(i64, String, String, String, String)> = self
            .connection
            .query_row(
                "SELECT rowid, id, session_id, role, created_at
             FROM messages WHERE session_id = ?1 AND id = ?2",
                params![session_id.to_string(), message_id.to_string()],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()?;
        let Some((rowid, id, session_id_text, role_text, created_at_text)) = row else {
            return Ok(None);
        };
        let id = Uuid::parse_str(&id)?;
        let session_id = Uuid::parse_str(&session_id_text)?;
        let role: MessageRole = serde_json::from_str(&role_text)?;
        let created_at = DateTime::parse_from_rfc3339(&created_at_text)?.with_timezone(&Utc);
        let content = if let Some(path) = self.find_tool_output_attachment(session_id, id)? {
            fs::read_to_string(path)?
        } else {
            let blob = self
                .connection
                .blob_open(MAIN_DB, "messages", "content", rowid, true)?;
            if role == MessageRole::Tool
                && blob.len() > TOOL_OUTPUT_ATTACHMENT_THRESHOLD
                && self.attachment_dir.is_some()
            {
                drop(blob);
                self.migrate_legacy_tool_output(rowid, session_id, id)?;
                if let Some(path) = self.find_tool_output_attachment(session_id, id)? {
                    fs::read_to_string(path)?
                } else {
                    return Err(StoreError::InvalidInput(
                        "tool output attachment could not be created".to_owned(),
                    ));
                }
            } else {
                drop(blob);
                self.connection.query_row(
                    "SELECT content FROM messages WHERE id = ?1",
                    [id.to_string()],
                    |row| row.get(0),
                )?
            }
        };
        Ok(Some(Message {
            id,
            session_id,
            role,
            content,
            created_at,
        }))
    }

    pub fn search_session_messages(
        &self,
        session_id: Uuid,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Message>, StoreError> {
        let query = query.trim();
        if query.is_empty() {
            return Err(StoreError::InvalidInput(
                "history search query must not be empty".to_owned(),
            ));
        }
        validate_context_tool_page(offset, limit)?;
        let pattern = format!("%{}%", escape_like_pattern(query));
        let mut statement = self.connection.prepare(
            "SELECT rowid, id, session_id, role, created_at
             FROM messages
             WHERE session_id = ?1 AND content LIKE ?2 ESCAPE '\\' COLLATE NOCASE
             ORDER BY created_at ASC, rowid ASC LIMIT ?3 OFFSET ?4",
        )?;
        let rows = statement.query_map(
            params![session_id.to_string(), pattern, limit as i64, offset as i64],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )?;
        let rows = rows.collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(rowid, id, session_id, role, created_at)| {
                let id = Uuid::parse_str(&id)?;
                let session_id = Uuid::parse_str(&session_id)?;
                let role: MessageRole = serde_json::from_str(&role)?;
                let created_at = DateTime::parse_from_rfc3339(&created_at)?.with_timezone(&Utc);
                self.message_preview(rowid, id, session_id, role, created_at)
                    .map(preview_to_message)
            })
            .collect()
    }

    pub fn list_context_windows(&self, session_id: Uuid) -> Result<Vec<ContextWindow>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT session_id, window_index, start_message_count, created_at
             FROM context_windows WHERE session_id = ?1
             ORDER BY window_index ASC",
        )?;
        let rows = statement.query_map([session_id.to_string()], Self::row_to_context_window)?;
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
        let trimmed = validate_local_memory_content(content)?;
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
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn list_local_memories_page(
        &self,
        workspace_root: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<LocalMemory>, StoreError> {
        validate_context_tool_page(offset, limit)?;
        let mut statement = self.connection.prepare(
            "SELECT id, workspace_root, content, created_at
             FROM local_memories WHERE workspace_root = ?1
             ORDER BY created_at ASC, id ASC LIMIT ?2 OFFSET ?3",
        )?;
        let rows = statement.query_map(
            params![
                normalize_workspace_root(workspace_root),
                limit as i64,
                offset as i64
            ],
            Self::row_to_local_memory,
        )?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn get_local_memory(
        &self,
        workspace_root: &str,
        memory_id: Uuid,
    ) -> Result<Option<LocalMemory>, StoreError> {
        self.connection
            .query_row(
                "SELECT id, workspace_root, content, created_at
                 FROM local_memories WHERE workspace_root = ?1 AND id = ?2",
                params![
                    normalize_workspace_root(workspace_root),
                    memory_id.to_string()
                ],
                Self::row_to_local_memory,
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn search_local_memories(
        &self,
        workspace_root: &str,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<LocalMemory>, StoreError> {
        let query = query.trim();
        if query.is_empty() {
            return Err(StoreError::InvalidInput(
                "notes search query must not be empty".to_owned(),
            ));
        }
        validate_context_tool_page(offset, limit)?;
        let pattern = format!("%{}%", escape_like_pattern(query));
        let mut statement = self.connection.prepare(
            "SELECT id, workspace_root, content, created_at
             FROM local_memories
             WHERE workspace_root = ?1 AND content LIKE ?2 ESCAPE '\\' COLLATE NOCASE
             ORDER BY created_at ASC, id ASC LIMIT ?3 OFFSET ?4",
        )?;
        let rows = statement.query_map(
            params![
                normalize_workspace_root(workspace_root),
                pattern,
                limit as i64,
                offset as i64
            ],
            Self::row_to_local_memory,
        )?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn replace_local_memory(
        &self,
        workspace_root: &str,
        memory_id: Uuid,
        content: &str,
    ) -> Result<Option<LocalMemory>, StoreError> {
        let trimmed = validate_local_memory_content(content)?;
        let workspace_root = normalize_workspace_root(workspace_root);
        let updated = self.connection.execute(
            "UPDATE local_memories SET content = ?1 WHERE workspace_root = ?2 AND id = ?3",
            params![trimmed, workspace_root, memory_id.to_string()],
        )?;
        if updated == 0 {
            return Ok(None);
        }
        self.get_local_memory(&workspace_root, memory_id)
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

    /// Description produced earlier for the same image payload, if any.
    /// Survives restarts so a stored screenshot is described once instead of
    /// once per process. The delegate model that produced it is returned as
    /// well, because a later run may use a different delegate and callers have
    /// to attribute the text honestly.
    pub fn get_vision_description(
        &self,
        cache_key: &str,
    ) -> Result<Option<StoredVisionDescription>, StoreError> {
        self.connection
            .query_row(
                "SELECT description, delegate_model FROM vision_descriptions WHERE cache_key = ?1",
                params![cache_key],
                |row| {
                    Ok(StoredVisionDescription {
                        description: row.get::<_, String>(0)?,
                        delegate_model: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }

    /// `session_id` records where the description was first needed so deleting
    /// that session can clean it up. Lookups stay session-independent because
    /// the cache key is the image payload itself.
    pub fn save_vision_description(
        &self,
        cache_key: &str,
        delegate_model: &str,
        session_id: Option<Uuid>,
        description: &str,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO vision_descriptions (cache_key, delegate_model, session_id, description, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(cache_key) DO UPDATE SET
                delegate_model = excluded.delegate_model,
                session_id = excluded.session_id,
                description = excluded.description,
                updated_at = excluded.updated_at",
            params![
                cache_key,
                delegate_model,
                session_id.map(|id| id.to_string()),
                description,
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    /// Returns the persisted vision-route status for a provider route key,
    /// or `None` when the provider has not been probed yet.
    pub fn get_vision_route_status(&self, route_key: &str) -> Result<Option<String>, StoreError> {
        self.connection
            .query_row(
                "SELECT status FROM vision_routes WHERE route_key = ?1",
                params![route_key],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(StoreError::from)
    }

    /// Persists the vision-route status ("NativeSupported" or
    /// "NativeUnsupported") so future sessions skip the probing request.
    pub fn save_vision_route_status(
        &self,
        route_key: &str,
        status: &str,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO vision_routes (route_key, status, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(route_key) DO UPDATE SET
                status = excluded.status,
                updated_at = excluded.updated_at",
            params![route_key, status, Utc::now().to_rfc3339()],
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

    pub fn model_call_report(
        &self,
        timezone_offset_minutes: i32,
    ) -> Result<ModelCallReport, StoreError> {
        let offset_seconds = timezone_offset_minutes.checked_mul(60).ok_or_else(|| {
            StoreError::InvalidInput("timezone offset is out of range".to_owned())
        })?;
        let timezone = FixedOffset::east_opt(offset_seconds).ok_or_else(|| {
            StoreError::InvalidInput("timezone offset is out of range".to_owned())
        })?;

        // Each persisted ModelCall represents one actual provider HTTP request, so
        // retries naturally remain separate report entries.
        let mut providers: BTreeMap<(String, String), (String, u64, u64)> = BTreeMap::new();
        let mut keys: BTreeMap<(String, String, String), (String, Option<String>, u64, u64)> =
            BTreeMap::new();
        let mut models: BTreeMap<(String, String), (u64, u64)> = BTreeMap::new();

        let mut statement = self.connection.prepare(
            "SELECT event, created_at FROM session_events
             ORDER BY created_at ASC, rowid ASC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;

        for row in rows {
            let (event_json, created_at) = row?;
            let event: SessionEvent = serde_json::from_str(&event_json)?;
            let SessionEvent::ModelCall {
                provider,
                provider_id,
                provider_name,
                key_id,
                key_hint,
                model,
                success,
                ..
            } = event
            else {
                continue;
            };

            let created_at = DateTime::parse_from_rfc3339(&created_at)?.with_timezone(&Utc);
            let date = created_at.with_timezone(&timezone).date_naive().to_string();
            let provider = provider.trim().to_owned();
            let provider_id = if provider_id.trim().is_empty() {
                provider.clone()
            } else {
                provider_id.trim().to_owned()
            };
            let provider_name = if provider_name.trim().is_empty() {
                provider.clone()
            } else {
                provider_name.trim().to_owned()
            };

            let provider_counts = providers
                .entry((date.clone(), provider_id.clone()))
                .or_insert_with(|| (provider_name.clone(), 0, 0));
            if success {
                provider_counts.1 = provider_counts.1.saturating_add(1);
            } else {
                provider_counts.2 = provider_counts.2.saturating_add(1);
            }

            let model_counts = models
                .entry((date.clone(), model.clone()))
                .or_insert((0, 0));
            if success {
                model_counts.0 = model_counts.0.saturating_add(1);
            } else {
                model_counts.1 = model_counts.1.saturating_add(1);
            }

            let key_id = key_id.trim();
            if !key_id.is_empty() {
                let key_hint = key_hint.and_then(|hint| {
                    if hint.trim().is_empty() {
                        None
                    } else {
                        Some(hint)
                    }
                });
                let key_counts = keys
                    .entry((date, provider_id, key_id.to_owned()))
                    .or_insert_with(|| (provider_name, key_hint.clone(), 0, 0));
                if success {
                    key_counts.2 = key_counts.2.saturating_add(1);
                } else {
                    key_counts.3 = key_counts.3.saturating_add(1);
                }
            }
        }

        Ok(ModelCallReport {
            providers: providers
                .into_iter()
                .rev()
                .map(
                    |((date, provider_id), (provider_name, success_count, failure_count))| {
                        ProviderCallReport {
                            date,
                            provider_id,
                            provider_name,
                            success_count,
                            failure_count,
                        }
                    },
                )
                .collect(),
            keys: keys
                .into_iter()
                .rev()
                .map(
                    |(
                        (date, provider_id, key_id),
                        (provider_name, key_hint, success_count, failure_count),
                    )| {
                        KeyCallReport {
                            date,
                            provider_id,
                            provider_name,
                            key_id,
                            key_hint,
                            success_count,
                            failure_count,
                        }
                    },
                )
                .collect(),
            models: models
                .into_iter()
                .rev()
                .map(
                    |((date, model), (success_count, failure_count))| ModelCallReportEntry {
                        date,
                        model,
                        success_count,
                        failure_count,
                    },
                )
                .collect(),
        })
    }

    pub fn purge_model_call_logs(&self, retention_days: u32) -> Result<usize, StoreError> {
        let retention_days = retention_days.max(1);
        let cutoff = Utc::now() - Duration::days(i64::from(retention_days));
        let transaction = self.connection.unchecked_transaction()?;
        let expired_ids = {
            let mut statement = transaction.prepare(
                "SELECT id, event, created_at FROM session_events ORDER BY created_at ASC, rowid ASC",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            let mut expired_ids = Vec::new();
            for row in rows {
                let (id, event_json, created_at) = row?;
                let event: SessionEvent = serde_json::from_str(&event_json)?;
                if !matches!(event, SessionEvent::ModelCall { .. }) {
                    continue;
                }
                let created_at = DateTime::parse_from_rfc3339(&created_at)?.with_timezone(&Utc);
                if created_at < cutoff {
                    expired_ids.push(id);
                }
            }
            expired_ids
        };

        for id in &expired_ids {
            transaction.execute("DELETE FROM session_events WHERE id = ?1", params![id])?;
        }
        transaction.commit()?;
        Ok(expired_ids.len())
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

            CREATE TABLE IF NOT EXISTS context_windows (
                session_id TEXT NOT NULL,
                window_index INTEGER NOT NULL,
                start_message_count INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY (session_id, window_index),
                UNIQUE (session_id, start_message_count),
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
        self.connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS tool_output_attachments (
                message_id TEXT PRIMARY KEY NOT NULL,
                session_id TEXT NOT NULL,
                relative_path TEXT NOT NULL,
                byte_length INTEGER NOT NULL,
                FOREIGN KEY (message_id) REFERENCES messages(id),
                FOREIGN KEY (session_id) REFERENCES sessions(id)
            );
            CREATE INDEX IF NOT EXISTS idx_messages_session_created
                ON messages(session_id, created_at);
            CREATE INDEX IF NOT EXISTS idx_tool_output_attachments_session
                ON tool_output_attachments(session_id);",
        )?;
        self.ensure_column("restore_points", "applied_text", "TEXT")?;
        // Added after the first release of the description cache: the delegate
        // model moved out of the cache key so text edits stop invalidating it,
        // and the session id lets `delete_session` clean up after itself.
        self.ensure_column("vision_descriptions", "delegate_model", "TEXT")?;
        self.ensure_column("vision_descriptions", "session_id", "TEXT")?;

        // Persisted vision-route status: remembers which providers do not
        // support image input so the agent can skip the probing attempt.
        self.connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS vision_routes (
                route_key TEXT PRIMARY KEY NOT NULL,
                status TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );",
        )?;
        self.connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS tool_output_attachments (
                message_id TEXT PRIMARY KEY NOT NULL,
                session_id TEXT NOT NULL,
                relative_path TEXT NOT NULL,
                byte_length INTEGER NOT NULL,
                FOREIGN KEY (message_id) REFERENCES messages(id),
                FOREIGN KEY (session_id) REFERENCES sessions(id)
            );
            CREATE INDEX IF NOT EXISTS idx_messages_session_created
                ON messages(session_id, created_at);
            CREATE INDEX IF NOT EXISTS idx_tool_output_attachments_session
                ON tool_output_attachments(session_id);",
        )?;

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

    fn row_to_context_window(row: &rusqlite::Row<'_>) -> rusqlite::Result<ContextWindow> {
        let session_id: String = row.get(0)?;
        let window_index: i64 = row.get(1)?;
        let start_message_count: i64 = row.get(2)?;
        let created_at: String = row.get(3)?;
        let parse = |error: StoreError| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        };
        Ok(ContextWindow {
            session_id: Uuid::parse_str(&session_id)
                .map_err(|error| parse(StoreError::Identifier(error)))?,
            window_index: usize::try_from(window_index).map_err(|_| {
                parse(StoreError::InvalidInput(
                    "stored context window index is negative or too large".to_owned(),
                ))
            })?,
            start_message_count: usize::try_from(start_message_count).map_err(|_| {
                parse(StoreError::InvalidInput(
                    "stored context window boundary is negative or too large".to_owned(),
                ))
            })?,
            created_at: DateTime::parse_from_rfc3339(&created_at)
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

fn escape_like_pattern(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn validate_context_tool_page(_offset: usize, limit: usize) -> Result<(), StoreError> {
    if !(MIN_CONTEXT_TOOL_LIMIT..=MAX_CONTEXT_TOOL_LIMIT).contains(&limit) {
        return Err(StoreError::InvalidInput(format!(
            "limit must be an integer from {MIN_CONTEXT_TOOL_LIMIT} to {MAX_CONTEXT_TOOL_LIMIT}"
        )));
    }
    Ok(())
}

fn validate_local_memory_content(content: &str) -> Result<&str, StoreError> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Err(StoreError::InvalidInput(
            "local memory content must not be empty".to_owned(),
        ));
    }
    if trimmed.chars().count() > MAX_LOCAL_MEMORY_CHARS {
        return Err(StoreError::InvalidInput(format!(
            "local memory content must be at most {MAX_LOCAL_MEMORY_CHARS} characters"
        )));
    }
    Ok(trimmed)
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
        | SessionEvent::StreamReset { session_id, .. }
        | SessionEvent::ModelCall { session_id, .. }
        | SessionEvent::Error { session_id, .. }
        | SessionEvent::VisionDelegateStart { session_id, .. }
        | SessionEvent::VisionDelegateSuccess { session_id, .. }
        | SessionEvent::VisionDelegateFailed { session_id, .. }
        | SessionEvent::VisionDescriptionsApplied { session_id, .. }
        | SessionEvent::ContextCompacted { session_id, .. } => *session_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xcoding_protocol::{Mode, ToolName};

    fn test_session(store: &SessionStore) -> Session {
        store
            .create_session(CreateSessionParams {
                workspace_root: "D:/work/attachments".to_owned(),
                mode: Mode::Ask,
                provider: "openai".to_owned(),
                model: "test-model".to_owned(),
                title: Some("attachment test".to_owned()),
            })
            .expect("session creates")
    }

    fn temporary_store_path() -> (PathBuf, PathBuf) {
        let directory = std::env::temp_dir().join(format!("xcoding-store-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("temporary directory creates");
        let database = directory.join("xcoding.db");
        (directory, database)
    }

    #[test]
    fn oversized_tool_outputs_use_local_attachments_and_keep_explicit_full_read() {
        let (directory, database) = temporary_store_path();
        let store = SessionStore::open(&database).expect("persistent store opens");
        let session = test_session(&store);
        let content = "tool-output-".repeat(30_000);

        let message = store
            .append_message(session.id, MessageRole::Tool, content.clone())
            .expect("large tool output saves");
        assert!(message.content.len() < TOOL_OUTPUT_ATTACHMENT_THRESHOLD);
        let page = store
            .list_message_previews_page(session.id, 0, 50)
            .expect("preview page loads");
        let preview = &page.messages[0];
        assert!(preview.full_output_available);
        assert_eq!(preview.content_bytes, content.len() as u64);
        assert!(preview.content.len() < TOOL_OUTPUT_ATTACHMENT_THRESHOLD);
        assert!(
            store
                .list_messages(session.id)
                .expect("context history stays compact")[0]
                .content
                .len()
                < TOOL_OUTPUT_ATTACHMENT_THRESHOLD
        );

        let attachment = store
            .open_tool_output_attachment(session.id, message.id)
            .expect("attachment path resolves");
        assert_eq!(
            fs::read_to_string(&attachment).expect("attachment reads"),
            content
        );
        assert_eq!(
            store
                .get_session_message(session.id, message.id)
                .expect("history read succeeds")
                .unwrap()
                .content,
            content
        );

        assert!(store.delete_session(session.id).expect("session deletes"));
        assert!(
            !attachment.exists(),
            "session deletion removes its attachment"
        );
        drop(store);
        fs::remove_dir_all(directory).expect("temporary files clean up");
    }

    #[test]
    fn legacy_oversized_tool_output_is_streamed_to_an_attachment_on_page_load() {
        let (directory, database) = temporary_store_path();
        let store = SessionStore::open(&database).expect("persistent store opens");
        let session = test_session(&store);
        let message_id = Uuid::new_v4();
        let content = "legacy-tool-output-".repeat(24_000);
        store.connection.execute(
            "INSERT INTO messages (id, session_id, role, content, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![message_id.to_string(), session.id.to_string(), serde_json::to_string(&MessageRole::Tool).unwrap(), content, Utc::now().to_rfc3339()],
        ).expect("legacy message inserts");

        let page = store
            .list_message_previews_page(session.id, 0, 50)
            .expect("legacy preview page loads");
        assert!(page.messages[0].full_output_available);
        assert!(page.messages[0].content.len() < TOOL_OUTPUT_ATTACHMENT_THRESHOLD);
        let attachment = store
            .open_tool_output_attachment(session.id, message_id)
            .expect("legacy output migrates incrementally");
        assert_eq!(
            fs::read_to_string(&attachment).expect("legacy file reads"),
            content
        );
        drop(store);
        fs::remove_dir_all(directory).expect("temporary files clean up");
    }

    #[test]
    fn attachments_are_backed_up_and_included_in_historical_secret_redaction() {
        let (directory, database) = temporary_store_path();
        let store = SessionStore::open(&database).expect("persistent store opens");
        let session = test_session(&store);
        let content = format!("api_key=secret-value {}", "safe-output-".repeat(30_000));
        let message = store
            .append_message(session.id, MessageRole::Tool, content)
            .expect("redacted-path tool output saves");
        let attachment = store
            .open_tool_output_attachment(session.id, message.id)
            .expect("attachment resolves");
        let backup = directory.join("backup.db");
        store
            .backup_to(&backup)
            .expect("database and attachments back up");
        let backup_attachment = attachment_dir_for_database(&backup)
            .join(session.id.to_string())
            .join(format!("{}.txt", message.id));
        assert!(backup_attachment.is_file());

        let report = store
            .redact_historical_secrets(&|value| value.replace("secret-value", "[REDACTED]"))
            .expect("attachment redaction succeeds");
        assert_eq!(report.messages_redacted, 1);
        assert!(
            !fs::read_to_string(attachment)
                .expect("redacted attachment reads")
                .contains("secret-value")
        );
        assert!(
            fs::read_to_string(backup_attachment)
                .expect("backup attachment reads")
                .contains("secret-value")
        );

        drop(store);
        fs::remove_dir_all(directory).expect("temporary files clean up");
    }

    fn insert_model_call_event(
        store: &SessionStore,
        session_id: Uuid,
        provider: &str,
        provider_id: &str,
        provider_name: &str,
        key_id: &str,
        key_hint: Option<&str>,
        model: &str,
        attempt: u32,
        success: bool,
        created_at: &str,
    ) {
        let event = SessionEvent::ModelCall {
            session_id,
            provider: provider.to_owned(),
            provider_id: provider_id.to_owned(),
            provider_name: provider_name.to_owned(),
            key_id: key_id.to_owned(),
            key_hint: key_hint.map(str::to_owned),
            model: model.to_owned(),
            effective_model: model.to_owned(),
            endpoint: "https://example.test/v1/chat/completions".to_owned(),
            purpose: "chat".to_owned(),
            round: 1,
            attempt,
            max_attempts: 3,
            success,
            output_chars: 0,
            tool_calls: 0,
            error: (!success).then(|| "request failed".to_owned()),
            model_reported: None,
            ttft_ms: None,
            total_ms: None,
        };
        store
            .connection
            .execute(
                "INSERT INTO session_events (id, session_id, event, created_at) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    Uuid::new_v4().to_string(),
                    session_id.to_string(),
                    serde_json::to_string(&event).expect("model call serializes"),
                    created_at,
                ],
            )
            .expect("model call event saves");
    }

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
    fn context_windows_preserve_history_and_are_removed_with_the_session() {
        let store = SessionStore::in_memory().expect("store starts");
        let session = store
            .create_session(CreateSessionParams {
                workspace_root: "D:/work/demo".to_owned(),
                mode: Mode::Ask,
                provider: "openai".to_owned(),
                model: "gpt-5.5".to_owned(),
                title: None,
            })
            .expect("session created");
        store
            .append_message(session.id, MessageRole::User, "first window")
            .expect("first message saved");
        store
            .append_message(session.id, MessageRole::User, "second window")
            .expect("second message saved");
        let original_history = store.list_messages(session.id).unwrap();
        assert_eq!(store.create_context_window(session.id, 1).unwrap(), 1);
        assert_eq!(store.create_context_window(session.id, 1).unwrap(), 1);

        assert_eq!(store.latest_context_window_start(session.id).unwrap(), 1);
        assert_eq!(store.list_messages(session.id).unwrap(), original_history);
        assert_eq!(
            store.list_messages_page(session.id, 1, 10).unwrap().len(),
            1
        );
        let windows = store.list_context_windows(session.id).unwrap();
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].window_index, 1);
        assert_eq!(windows[0].start_message_count, 1);

        assert!(store.delete_session(session.id).unwrap());
        assert_eq!(store.latest_context_window_start(session.id).unwrap(), 0);
    }

    #[test]
    fn context_window_boundaries_are_user_messages_and_only_advance() {
        let store = SessionStore::in_memory().expect("store starts");
        let session = store
            .create_session(CreateSessionParams {
                workspace_root: "D:/work/demo".to_owned(),
                mode: Mode::Ask,
                provider: "openai".to_owned(),
                model: "gpt-5.5".to_owned(),
                title: None,
            })
            .expect("session created");
        store
            .append_message(session.id, MessageRole::User, "first user")
            .unwrap();
        store
            .append_message(session.id, MessageRole::Assistant, "answer")
            .unwrap();
        store
            .append_message(session.id, MessageRole::User, "second user")
            .unwrap();
        assert_eq!(store.create_context_window(session.id, 2).unwrap(), 1);

        assert!(store.create_context_window(session.id, 1).is_err());
        assert!(store.create_context_window(session.id, 0).is_err());
        assert!(store.create_context_window(session.id, 3).is_err());
        assert_eq!(store.list_context_windows(session.id).unwrap().len(), 1);
    }

    #[test]
    fn first_context_window_boundary_cannot_be_zero() {
        let store = SessionStore::in_memory().expect("store starts");
        let session = store
            .create_session(CreateSessionParams {
                workspace_root: "D:/work/demo".to_owned(),
                mode: Mode::Ask,
                provider: "openai".to_owned(),
                model: "gpt-5.5".to_owned(),
                title: None,
            })
            .expect("session created");
        store
            .append_message(session.id, MessageRole::User, "first user")
            .unwrap();

        assert!(store.create_context_window(session.id, 0).is_err());
        assert!(store.list_context_windows(session.id).unwrap().is_empty());
    }

    #[test]
    fn searches_history_and_manages_workspace_scoped_notes() {
        let store = SessionStore::in_memory().expect("store starts");
        let session = store
            .create_session(CreateSessionParams {
                workspace_root: "D:/work/demo".to_owned(),
                mode: Mode::Ask,
                provider: "openai".to_owned(),
                model: "gpt-5.5".to_owned(),
                title: None,
            })
            .expect("session created");
        let message = store
            .append_message(session.id, MessageRole::User, "literal 100%_match")
            .expect("message saved");
        assert_eq!(
            store
                .search_session_messages(session.id, "100%_match", 0, 10)
                .unwrap(),
            vec![message.clone()]
        );
        assert_eq!(
            store.get_session_message(session.id, message.id).unwrap(),
            Some(message)
        );

        let note = store
            .save_local_memory("D:/work/demo", "remember 100%_match")
            .unwrap()
            .expect("note created");
        assert_eq!(
            store
                .search_local_memories("D:/WORK/demo/", "100%_match", 0, 10)
                .unwrap(),
            vec![note.clone()]
        );
        assert!(
            store
                .get_local_memory("D:/work/other", note.id)
                .unwrap()
                .is_none()
        );
        let updated = store
            .replace_local_memory("D:/work/demo", note.id, "replacement")
            .unwrap()
            .expect("note updated");
        assert_eq!(updated.id, note.id);
        assert_eq!(updated.content, "replacement");
    }

    #[test]
    fn rejects_out_of_range_history_and_notes_page_limits() {
        let store = SessionStore::in_memory().expect("store starts");
        let session = store
            .create_session(CreateSessionParams {
                workspace_root: "D:/work/demo".to_owned(),
                mode: Mode::Ask,
                provider: "openai".to_owned(),
                model: "gpt-5.5".to_owned(),
                title: None,
            })
            .expect("session created");
        store
            .append_message(session.id, MessageRole::User, "pageable")
            .expect("message saved");
        store
            .save_local_memory("D:/work/demo", "pageable note")
            .unwrap()
            .expect("note created");

        for limit in [0usize, 101] {
            assert!(matches!(
                store.list_messages_page(session.id, 0, limit),
                Err(StoreError::InvalidInput(_))
            ));
            assert!(matches!(
                store.search_session_messages(session.id, "pageable", 0, limit),
                Err(StoreError::InvalidInput(_))
            ));
            assert!(matches!(
                store.list_local_memories_page("D:/work/demo", 0, limit),
                Err(StoreError::InvalidInput(_))
            ));
            assert!(matches!(
                store.search_local_memories("D:/work/demo", "pageable", 0, limit),
                Err(StoreError::InvalidInput(_))
            ));
        }

        assert_eq!(
            store.list_messages_page(session.id, 0, 100).unwrap().len(),
            1
        );
        assert_eq!(
            store
                .search_session_messages(session.id, "pageable", 0, 100)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .list_local_memories_page("D:/work/demo", 0, 100)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .search_local_memories("D:/work/demo", "pageable", 0, 100)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn pages_history_and_notes_without_crossing_session_or_workspace() {
        let store = SessionStore::in_memory().expect("store starts");
        let session = store
            .create_session(CreateSessionParams {
                workspace_root: "D:/work/demo".to_owned(),
                mode: Mode::Ask,
                provider: "openai".to_owned(),
                model: "gpt-5.5".to_owned(),
                title: None,
            })
            .expect("session created");
        let other = store
            .create_session(CreateSessionParams {
                workspace_root: "D:/work/other".to_owned(),
                mode: Mode::Ask,
                provider: "openai".to_owned(),
                model: "gpt-5.5".to_owned(),
                title: None,
            })
            .expect("other session created");
        for content in ["one", "two", "three"] {
            store
                .append_message(session.id, MessageRole::User, content)
                .expect("message saved");
        }
        let other_message = store
            .append_message(other.id, MessageRole::User, "two")
            .expect("other message saved");

        let page = store.list_messages_page(session.id, 1, 1).unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].content, "two");
        let search_page = store
            .search_session_messages(session.id, "t", 1, 1)
            .unwrap();
        assert_eq!(search_page.len(), 1);
        assert_eq!(search_page[0].content, "three");
        assert!(
            store
                .get_session_message(other.id, page[0].id)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            store
                .search_session_messages(other.id, "two", 0, 10)
                .unwrap(),
            vec![other_message]
        );

        for content in ["alpha", "beta", "gamma"] {
            store
                .save_local_memory("D:/work/demo", content)
                .unwrap()
                .expect("note created");
        }
        store
            .save_local_memory("D:/work/other", "beta")
            .unwrap()
            .expect("other note created");
        let notes_page = store
            .list_local_memories_page("D:/work/demo", 1, 1)
            .unwrap();
        assert_eq!(notes_page.len(), 1);
        assert_eq!(notes_page[0].content, "beta");
        let notes_search = store
            .search_local_memories("D:/work/demo", "mm", 0, 10)
            .unwrap();
        assert_eq!(notes_search.len(), 1);
        assert_eq!(notes_search[0].content, "gamma");
        assert!(
            store
                .get_local_memory("D:/work/other", notes_page[0].id)
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .replace_local_memory("D:/work/demo", Uuid::new_v4(), "missing")
                .unwrap()
                .is_none()
        );

        let max_note = "a".repeat(MAX_LOCAL_MEMORY_CHARS);
        store
            .save_local_memory("D:/work/demo", &max_note)
            .unwrap()
            .expect("max note saved");
        assert!(
            store
                .save_local_memory("D:/work/demo", &"a".repeat(MAX_LOCAL_MEMORY_CHARS + 1))
                .is_err()
        );
        assert!(
            store
                .replace_local_memory(
                    "D:/work/demo",
                    notes_page[0].id,
                    &"a".repeat(MAX_LOCAL_MEMORY_CHARS + 1)
                )
                .is_err()
        );
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
            .append_message(target_a.id, MessageRole::User, "second window")
            .expect("second message saves");
        assert_eq!(store.create_context_window(target_a.id, 1).unwrap(), 1);
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
            assert!(
                store
                    .list_context_windows(session.id)
                    .expect("context windows")
                    .is_empty()
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
            .append_message(session.id, MessageRole::User, "second window")
            .expect("second message saves");
        assert_eq!(store.create_context_window(session.id, 1).unwrap(), 1);
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
        assert!(
            store
                .list_context_windows(session.id)
                .expect("context windows")
                .is_empty()
        );
        assert!(!store.delete_session(session.id).expect("second delete"));
    }

    #[test]
    fn deleting_session_reclaims_database_file_space() {
        let (directory, database) = temporary_store_path();
        let store = SessionStore::open(&database).expect("persistent store opens");
        let session = test_session(&store);
        store
            .append_message(session.id, MessageRole::User, "x".repeat(8 * 1024 * 1024))
            .expect("large session content saves");
        drop(store);

        let size_before = fs::metadata(&database)
            .expect("database metadata loads")
            .len();
        let store = SessionStore::open(&database).expect("persistent store reopens");
        assert!(store.delete_session(session.id).expect("session deletes"));
        drop(store);
        let size_after = fs::metadata(&database)
            .expect("compacted database metadata loads")
            .len();

        assert!(
            size_after < size_before / 2,
            "session deletion should reclaim database space: {size_before} -> {size_after}"
        );
        fs::remove_dir_all(directory).expect("temporary files clean up");
    }

    #[test]
    fn purges_only_expired_model_call_logs() {
        let store = SessionStore::in_memory().expect("in-memory database starts");
        let session = store
            .create_session(CreateSessionParams {
                workspace_root: "D:/work/purge".to_owned(),
                mode: Mode::Ask,
                provider: "provider-a".to_owned(),
                model: "model-x".to_owned(),
                title: None,
            })
            .expect("session saves");
        let expired_at = (Utc::now() - Duration::days(2)).to_rfc3339();
        let recent_at = (Utc::now() - Duration::hours(12)).to_rfc3339();

        insert_model_call_event(
            &store,
            session.id,
            "provider-a",
            "provider-a",
            "Provider A",
            "key-a",
            Some("...key-a"),
            "model-expired",
            1,
            true,
            &expired_at,
        );
        insert_model_call_event(
            &store,
            session.id,
            "provider-a",
            "provider-a",
            "Provider A",
            "key-a",
            Some("...key-a"),
            "model-recent",
            1,
            false,
            &recent_at,
        );
        store
            .connection
            .execute(
                "INSERT INTO session_events (id, session_id, event, created_at) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    Uuid::new_v4().to_string(),
                    session.id.to_string(),
                    serde_json::to_string(&SessionEvent::TextDelta {
                        session_id: session.id,
                        delta: "ordinary event".to_owned(),
                    })
                    .expect("ordinary event serializes"),
                    expired_at,
                ],
            )
            .expect("ordinary event saves");

        assert_eq!(store.purge_model_call_logs(1).expect("logs purge"), 1);
        assert_eq!(store.list_events(session.id).expect("events").len(), 2);
        let report = store.model_call_report(0).expect("report loads");
        assert_eq!(report.models.len(), 1);
        assert_eq!(report.models[0].model, "model-recent");
        assert_eq!(report.models[0].success_count, 0);
        assert_eq!(report.models[0].failure_count, 1);
    }

    #[test]
    fn reports_model_calls_by_provider_key_and_model_with_local_dates() {
        let store = SessionStore::in_memory().expect("in-memory database starts");
        let session = store
            .create_session(CreateSessionParams {
                workspace_root: "D:/work/report".to_owned(),
                mode: Mode::Ask,
                provider: "provider-a".to_owned(),
                model: "model-x".to_owned(),
                title: None,
            })
            .expect("session saves");

        // The first two calls represent one request and its retry. They remain
        // separate provider, key, and model counts because each event is an
        // actual HTTP attempt. Both timestamps are 2026-10-06 UTC, which is
        // 2026-10-07 in the requested UTC+08:00 report timezone.
        insert_model_call_event(
            &store,
            session.id,
            "provider-a",
            "provider-a",
            "Provider A",
            "key-a",
            Some("...key-a"),
            "model-x",
            1,
            true,
            "2026-10-06T23:30:00Z",
        );
        insert_model_call_event(
            &store,
            session.id,
            "provider-a",
            "provider-a",
            "Provider A",
            "key-a",
            Some("...key-a"),
            "model-x",
            2,
            false,
            "2026-10-06T23:31:00Z",
        );
        insert_model_call_event(
            &store,
            session.id,
            "provider-a",
            "provider-a",
            "Provider A",
            "key-b",
            Some("...key-b"),
            "model-x",
            1,
            true,
            "2026-10-06T23:32:00Z",
        );
        insert_model_call_event(
            &store,
            session.id,
            "provider-b",
            "provider-b",
            "Provider B",
            "key-c",
            Some("...key-c"),
            "model-y",
            1,
            false,
            "2026-10-06T23:33:00Z",
        );
        // 2026-10-07 UTC becomes 2026-10-08 in UTC+08:00.
        insert_model_call_event(
            &store,
            session.id,
            "provider-a",
            "provider-a",
            "Provider A",
            "key-a",
            Some("...key-a"),
            "model-z",
            1,
            true,
            "2026-10-07T23:30:00Z",
        );

        // Simulate an event written before provider and key identity fields
        // were added. It must fall back to the legacy provider value and must
        // not guess a key bucket.
        let legacy_event = SessionEvent::ModelCall {
            session_id: session.id,
            provider: "legacy-provider".to_owned(),
            provider_id: "legacy-id-that-will-be-removed".to_owned(),
            provider_name: "Legacy Provider".to_owned(),
            key_id: "legacy-key-that-will-be-removed".to_owned(),
            key_hint: Some("...legacy".to_owned()),
            model: "legacy-model".to_owned(),
            effective_model: "legacy-model".to_owned(),
            endpoint: "https://example.test/v1/chat/completions".to_owned(),
            purpose: "chat".to_owned(),
            round: 1,
            attempt: 1,
            max_attempts: 1,
            success: true,
            output_chars: 0,
            tool_calls: 0,
            error: None,
            model_reported: None,
            ttft_ms: None,
            total_ms: None,
        };
        let mut legacy_json =
            serde_json::to_value(legacy_event).expect("legacy model call serializes");
        let legacy_object = legacy_json
            .as_object_mut()
            .expect("legacy model call is an object");
        legacy_object.remove("provider_id");
        legacy_object.remove("provider_name");
        legacy_object.remove("key_id");
        store
            .connection
            .execute(
                "INSERT INTO session_events (id, session_id, event, created_at) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    Uuid::new_v4().to_string(),
                    session.id.to_string(),
                    serde_json::to_string(&legacy_json).expect("legacy event JSON serializes"),
                    "2026-10-06T23:34:00Z",
                ],
            )
            .expect("legacy model call event saves");

        let report = store
            .model_call_report(8 * 60)
            .expect("model call report loads");

        assert!(
            report
                .providers
                .windows(2)
                .all(|rows| rows[0].date >= rows[1].date)
        );
        assert!(
            report
                .keys
                .windows(2)
                .all(|rows| rows[0].date >= rows[1].date)
        );
        assert!(
            report
                .models
                .windows(2)
                .all(|rows| rows[0].date >= rows[1].date)
        );

        assert_eq!(report.providers.len(), 4);
        let provider_a_day_one = report
            .providers
            .iter()
            .find(|row| row.date == "2026-10-07" && row.provider_id == "provider-a")
            .expect("provider A day one report row");
        assert_eq!(provider_a_day_one.provider_name, "Provider A");
        assert_eq!(provider_a_day_one.success_count, 2);
        assert_eq!(provider_a_day_one.failure_count, 1);
        let provider_a_day_two = report
            .providers
            .iter()
            .find(|row| row.date == "2026-10-08" && row.provider_id == "provider-a")
            .expect("provider A day two report row");
        assert_eq!(provider_a_day_two.success_count, 1);
        assert_eq!(provider_a_day_two.failure_count, 0);
        let provider_b = report
            .providers
            .iter()
            .find(|row| row.provider_id == "provider-b")
            .expect("provider B report row");
        assert_eq!(provider_b.success_count, 0);
        assert_eq!(provider_b.failure_count, 1);
        let legacy_provider = report
            .providers
            .iter()
            .find(|row| row.provider_id == "legacy-provider")
            .expect("legacy provider report row");
        assert_eq!(legacy_provider.provider_name, "legacy-provider");
        assert_eq!(legacy_provider.success_count, 1);
        assert_eq!(legacy_provider.failure_count, 0);

        assert_eq!(report.keys.len(), 4);
        let key_a_day_one = report
            .keys
            .iter()
            .find(|row| row.date == "2026-10-07" && row.key_id == "key-a")
            .expect("key A day one report row");
        assert_eq!(key_a_day_one.success_count, 1);
        assert_eq!(key_a_day_one.failure_count, 1);
        assert_eq!(key_a_day_one.key_hint.as_deref(), Some("...key-a"));
        let key_b = report
            .keys
            .iter()
            .find(|row| row.key_id == "key-b")
            .expect("key B report row");
        assert_eq!(key_b.success_count, 1);
        assert_eq!(key_b.failure_count, 0);
        assert!(
            report
                .keys
                .iter()
                .all(|row| !row.key_id.starts_with("legacy"))
        );

        assert_eq!(report.models.len(), 4);
        let model_x = report
            .models
            .iter()
            .find(|row| row.date == "2026-10-07" && row.model == "model-x")
            .expect("model X report row");
        assert_eq!(model_x.success_count, 2);
        assert_eq!(model_x.failure_count, 1);
        let model_z = report
            .models
            .iter()
            .find(|row| row.date == "2026-10-08" && row.model == "model-z")
            .expect("model Z report row");
        assert_eq!(model_z.success_count, 1);
        assert_eq!(model_z.failure_count, 0);
    }

    #[test]
    fn redacts_historical_content_idempotently_without_touching_restore_points() {
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
        store
            .append_message(session.id, MessageRole::Tool, "api_key=secret-value")
            .expect("message saves");
        store
            .save_local_memory("D:/work/demo", "token=secret-value")
            .expect("memory saves");
        store
            .save_context_compaction(ContextCompaction {
                session_id: session.id,
                summary: "access_token: secret-value".to_owned(),
                compacted_message_count: 1,
                updated_at: Utc::now(),
            })
            .expect("compaction saves");
        let event_message = Message {
            id: Uuid::new_v4(),
            session_id: session.id,
            role: MessageRole::Tool,
            content: "client_secret=secret-value".to_owned(),
            created_at: Utc::now(),
        };
        store
            .record_event(&SessionEvent::MessageCompleted {
                session_id: session.id,
                message: event_message,
            })
            .expect("event saves");
        store
            .create_restore_point(
                session.id,
                "config.env",
                Some("api_key=secret-value"),
                "new",
            )
            .expect("restore point saves");

        let report = store
            .redact_historical_secrets(&|value| value.replace("secret-value", "[REDACTED]"))
            .expect("historical redaction succeeds");
        assert_eq!(report.messages_scanned, 1);
        assert_eq!(report.messages_redacted, 1);
        assert_eq!(report.events_redacted, 1);
        assert_eq!(report.compactions_redacted, 1);
        assert_eq!(report.memories_redacted, 1);

        assert_eq!(
            store.list_messages(session.id).expect("messages")[0].content,
            "api_key=[REDACTED]"
        );
        assert_eq!(
            store
                .get_context_compaction(session.id)
                .expect("compaction lookup")
                .expect("compaction exists")
                .summary,
            "access_token: [REDACTED]"
        );
        assert_eq!(
            store
                .list_restore_points(session.id)
                .expect("restore lookup")[0]
                .original_text
                .as_deref(),
            Some("api_key=secret-value")
        );

        let second = store
            .redact_historical_secrets(&|value| value.replace("secret-value", "[REDACTED]"))
            .expect("second redaction succeeds");
        assert_eq!(second.messages_redacted, 0);
        assert_eq!(second.events_redacted, 0);
        assert_eq!(second.compactions_redacted, 0);
        assert_eq!(second.memories_redacted, 0);
    }
}
