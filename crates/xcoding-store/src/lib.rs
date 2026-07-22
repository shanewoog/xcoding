//! SQLite persistence for XCoding sessions and future event traces.

use std::path::Path;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use thiserror::Error;
use uuid::Uuid;
use xcoding_protocol::{CreateSessionParams, Session, SessionStatus};

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("invalid stored session data: {0}")]
    InvalidData(#[from] serde_json::Error),
    #[error("invalid stored timestamp: {0}")]
    Timestamp(#[from] chrono::ParseError),
    #[error("invalid stored session id: {0}")]
    SessionId(#[from] uuid::Error),
}

pub struct SessionStore {
    connection: Connection,
}

impl SessionStore {
    pub fn in_memory() -> Result<Self, StoreError> {
        let connection = Connection::open_in_memory()?;
        let store = Self { connection };
        store.migrate()?;
        Ok(store)
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let connection = Connection::open(path)?;
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
                sessions.push(row?);
            }
        } else {
            let mut statement = self.connection.prepare(
                "SELECT id, workspace_root, mode, provider, model, status, created_at, updated_at, title
                 FROM sessions ORDER BY created_at DESC",
            )?;
            let rows = statement.query_map([], Self::row_to_session)?;
            for row in rows {
                sessions.push(row?);
            }
        }

        Ok(sessions)
    }

    pub fn get_session(&self, id: Uuid) -> Result<Option<Session>, StoreError> {
        self.connection
            .query_row(
                "SELECT id, workspace_root, mode, provider, model, status, created_at, updated_at, title
                 FROM sessions WHERE id = ?1",
                [id.to_string()],
                Self::row_to_session,
            )
            .optional()
            .map_err(StoreError::from)
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
            );",
        )?;
        Ok(())
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
            id: Uuid::parse_str(&id).map_err(|error| parse(StoreError::SessionId(error)))?,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use xcoding_protocol::Mode;

    #[test]
    fn persists_sessions() {
        let store = SessionStore::in_memory().expect("in-memory database starts");
        let session = store
            .create_session(CreateSessionParams {
                workspace_root: "D:/work/demo".to_owned(),
                mode: Mode::Ask,
                provider: "openai".to_owned(),
                model: "gpt-4.1".to_owned(),
                title: Some("First session".to_owned()),
            })
            .expect("session saves");

        let sessions = store
            .list_sessions(Some("D:/work/demo"))
            .expect("sessions load");

        assert_eq!(sessions, vec![session]);
    }
}
