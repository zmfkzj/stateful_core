use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use stateful_core::{IntentScope, PolicyState};
use std::path::Path;
use thiserror::Error;
use uuid::Uuid;

pub const CRATE_NAME: &str = "stateful-store";

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("reservation owner mismatch")]
    ReservationOwnerMismatch,
}

pub type StoreResult<T> = Result<T, StoreError>;

#[derive(Debug)]
pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> StoreResult<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(path)?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_in_memory() -> StoreResult<Self> {
        let conn = Connection::open_in_memory()?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn append(&self, event: Event) -> StoreResult<()> {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;

        let result = (|| -> StoreResult<()> {
            let inserted = self.conn.execute(
                "INSERT OR IGNORE INTO events (
                    event_id,
                    event_type,
                    session_id,
                    workspace_id,
                    repo_id,
                    worktree_id,
                    root,
                    branch,
                    payload_json,
                    created_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    event.event_id,
                    event.event_type.as_str(),
                    event.session_id,
                    event.workspace_id,
                    event.repo_id,
                    event.worktree_id,
                    event.root,
                    event.branch,
                    serde_json::to_string(&event.payload)
                        .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))?,
                    event.created_at,
                ],
            )?;

            if inserted == 1 {
                self.materialize(&event)?;
            }

            self.conn.execute_batch("COMMIT")?;
            Ok(())
        })();

        if result.is_err() {
            let _ = self.conn.execute_batch("ROLLBACK");
        }

        result
    }

    pub fn session(&self, session_id: &str) -> StoreResult<Option<SessionRecord>> {
        self.conn
            .query_row(
                "SELECT session_id, workspace_id FROM sessions WHERE session_id = ?1",
                [session_id],
                |row| {
                    Ok(SessionRecord {
                        session_id: row.get(0)?,
                        workspace_id: row.get(1)?,
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn event_count(&self) -> StoreResult<u64> {
        self.conn
            .query_row("SELECT COUNT(*) FROM events", [], |row| {
                row.get::<_, u64>(0)
            })
            .map_err(StoreError::from)
    }

    pub fn current_summary(&self) -> StoreResult<CurrentSummary> {
        let session_count = self
            .conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |row| {
                row.get::<_, u64>(0)
            })?;
        let active_intent_count = self.conn.query_row(
            "SELECT COUNT(*) FROM intents WHERE status = 'active'",
            [],
            |row| row.get::<_, u64>(0),
        )?;
        let event_count = self.event_count()?;

        Ok(CurrentSummary {
            session_count,
            active_intent_count,
            event_count,
        })
    }

    pub fn recent_events(&self, limit: u64) -> StoreResult<Vec<EventRecord>> {
        let mut statement = self.conn.prepare(
            "SELECT
                event_id,
                event_type,
                session_id,
                workspace_id,
                repo_id,
                worktree_id,
                root,
                branch,
                created_at
             FROM events
             ORDER BY rowid ASC
             LIMIT ?1",
        )?;
        let rows = statement.query_map([limit], |row| {
            Ok(EventRecord {
                event_id: row.get(0)?,
                event_type: row.get(1)?,
                session_id: row.get(2)?,
                workspace_id: row.get(3)?,
                repo_id: row.get(4)?,
                worktree_id: row.get(5)?,
                root: row.get(6)?,
                branch: row.get(7)?,
                created_at: row.get(8)?,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn append_outbox(&self, entry: OutboxEntry) -> StoreResult<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO outbox (
                outbox_id,
                session_id,
                sequence,
                sync_status
            ) VALUES (?1, ?2, ?3, ?4)",
            params![
                entry.outbox_id,
                entry.session_id,
                entry.sequence,
                entry.sync_status.as_str(),
            ],
        )?;

        Ok(())
    }

    pub fn outbox_count(&self) -> StoreResult<u64> {
        self.conn
            .query_row("SELECT COUNT(*) FROM outbox", [], |row| {
                row.get::<_, u64>(0)
            })
            .map_err(StoreError::from)
    }

    pub fn append_reconciliation_ack(&self, session_id: impl AsRef<str>) -> StoreResult<()> {
        self.conn.execute(
            "INSERT INTO reconciliations (
                reconciliation_id,
                session_id,
                created_at
            ) VALUES (?1, ?2, ?3)",
            params![
                Uuid::new_v4().to_string(),
                session_id.as_ref(),
                "2026-05-31T00:00:00Z",
            ],
        )?;

        Ok(())
    }

    pub fn reconciliation_count(&self) -> StoreResult<u64> {
        self.conn
            .query_row("SELECT COUNT(*) FROM reconciliations", [], |row| {
                row.get::<_, u64>(0)
            })
            .map_err(StoreError::from)
    }

    pub fn append_validation_result(
        &self,
        workspace_id: impl AsRef<str>,
        profile_id: impl AsRef<str>,
        status: impl AsRef<str>,
    ) -> StoreResult<()> {
        self.conn.execute(
            "INSERT INTO validations (
                validation_id,
                workspace_id,
                profile_id,
                status
            ) VALUES (?1, ?2, ?3, ?4)",
            params![
                Uuid::new_v4().to_string(),
                workspace_id.as_ref(),
                profile_id.as_ref(),
                status.as_ref(),
            ],
        )?;

        Ok(())
    }

    pub fn validation_count(&self) -> StoreResult<u64> {
        self.conn
            .query_row("SELECT COUNT(*) FROM validations", [], |row| {
                row.get::<_, u64>(0)
            })
            .map_err(StoreError::from)
    }

    pub fn acquire_lease(
        &self,
        session_id: impl AsRef<str>,
        workspace_id: impl AsRef<str>,
        relative_path: impl AsRef<str>,
    ) -> StoreResult<()> {
        let relative_path = normalize_relative_path(relative_path.as_ref());
        self.conn.execute(
            "INSERT INTO leases (
                lease_id,
                session_id,
                workspace_id,
                repo_id,
                relative_path,
                absolute_path,
                status,
                expires_at
            ) VALUES (?1, ?2, ?3, NULL, ?4, NULL, 'active', ?5)",
            params![
                Uuid::new_v4().to_string(),
                session_id.as_ref(),
                workspace_id.as_ref(),
                relative_path,
                "2026-05-31T00:15:00Z",
            ],
        )?;

        Ok(())
    }

    pub fn release_lease(
        &self,
        session_id: impl AsRef<str>,
        workspace_id: impl AsRef<str>,
        relative_path: impl AsRef<str>,
    ) -> StoreResult<()> {
        let relative_path = normalize_relative_path(relative_path.as_ref());
        let workspace_id = workspace_id.as_ref().to_string();
        self.conn.execute(
            "UPDATE leases
             SET status = 'released'
             WHERE session_id = ?1 AND workspace_id = ?2 AND relative_path = ?3 AND status = 'active'",
            params![session_id.as_ref(), workspace_id, relative_path],
        )?;
        self.promote_next_waiter(&workspace_id, &relative_path)?;

        Ok(())
    }

    pub fn release_session_leases(
        &self,
        session_id: impl AsRef<str>,
        workspace_id: impl AsRef<str>,
    ) -> StoreResult<u64> {
        let mut statement = self.conn.prepare(
            "SELECT relative_path FROM leases
             WHERE session_id = ?1 AND workspace_id = ?2 AND status = 'active'",
        )?;
        let paths = statement
            .query_map(params![session_id.as_ref(), workspace_id.as_ref()], |row| {
                row.get::<_, String>(0)
            })?;
        let paths = paths.collect::<Result<Vec<_>, _>>()?;
        let released = paths.len() as u64;

        for path in paths {
            self.release_lease(session_id.as_ref(), workspace_id.as_ref(), path)?;
        }

        Ok(released)
    }

    pub fn lease_count(&self) -> StoreResult<u64> {
        self.conn
            .query_row("SELECT COUNT(*) FROM leases", [], |row| {
                row.get::<_, u64>(0)
            })
            .map_err(StoreError::from)
    }

    pub fn active_lease_owner(
        &self,
        workspace_id: impl AsRef<str>,
        relative_path: impl AsRef<str>,
    ) -> StoreResult<Option<String>> {
        let relative_path = normalize_relative_path(relative_path.as_ref());
        self.conn
            .query_row(
                "SELECT session_id FROM leases
                 WHERE workspace_id = ?1 AND relative_path = ?2 AND status = 'active'
                 ORDER BY rowid DESC
                 LIMIT 1",
                params![workspace_id.as_ref(), relative_path],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn create_intent_request(
        &self,
        request_id: impl AsRef<str>,
        session_id: impl AsRef<str>,
        workspace_id: impl AsRef<str>,
        resources: &[String],
        action: impl AsRef<str>,
    ) -> StoreResult<IntentRequestRecord> {
        let request_id = request_id.as_ref();
        let session_id = session_id.as_ref();
        let workspace_id = workspace_id.as_ref();
        let action = action.as_ref();
        let resources_json = serde_json::to_string(resources)
            .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))?;

        self.conn.execute(
            "INSERT OR IGNORE INTO intent_requests (
                request_id,
                session_id,
                workspace_id,
                resources_json,
                action,
                status,
                created_at,
                updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, 'requested', ?6, ?6)",
            params![
                request_id,
                session_id,
                workspace_id,
                resources_json,
                action,
                "2026-05-31T00:00:00Z",
            ],
        )?;

        self.intent_request(request_id)?
            .ok_or_else(|| StoreError::Sqlite(rusqlite::Error::QueryReturnedNoRows))
    }

    pub fn intent_request(
        &self,
        request_id: impl AsRef<str>,
    ) -> StoreResult<Option<IntentRequestRecord>> {
        self.conn
            .query_row(
                "SELECT
                    request_id,
                    session_id,
                    workspace_id,
                    resources_json,
                    action,
                    status,
                    created_at,
                    updated_at
                 FROM intent_requests
                 WHERE request_id = ?1",
                params![request_id.as_ref()],
                intent_request_from_row,
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn intent_request_count(&self) -> StoreResult<u64> {
        self.conn
            .query_row("SELECT COUNT(*) FROM intent_requests", [], |row| {
                row.get::<_, u64>(0)
            })
            .map_err(StoreError::from)
    }

    pub fn mark_intent_request_status(
        &self,
        request_id: impl AsRef<str>,
        status: impl AsRef<str>,
    ) -> StoreResult<()> {
        let updated = self.conn.execute(
            "UPDATE intent_requests
             SET status = ?1, updated_at = ?2
             WHERE request_id = ?3",
            params![status.as_ref(), "2026-05-31T00:00:00Z", request_id.as_ref(),],
        )?;
        if updated == 0 {
            return Err(StoreError::ReservationOwnerMismatch);
        }

        Ok(())
    }

    pub fn enqueue_waiter(
        &self,
        session_id: impl AsRef<str>,
        workspace_id: impl AsRef<str>,
        relative_path: impl AsRef<str>,
        action: impl AsRef<str>,
        blocking_session_id: Option<&str>,
    ) -> StoreResult<WaitRecord> {
        self.enqueue_waiter_inner(
            None,
            session_id.as_ref(),
            workspace_id.as_ref(),
            relative_path.as_ref(),
            action.as_ref(),
            blocking_session_id,
        )
    }

    pub fn enqueue_waiter_for_request(
        &self,
        request_id: impl AsRef<str>,
        session_id: impl AsRef<str>,
        workspace_id: impl AsRef<str>,
        relative_path: impl AsRef<str>,
        action: impl AsRef<str>,
        blocking_session_id: Option<&str>,
    ) -> StoreResult<WaitRecord> {
        self.enqueue_waiter_inner(
            Some(request_id.as_ref()),
            session_id.as_ref(),
            workspace_id.as_ref(),
            relative_path.as_ref(),
            action.as_ref(),
            blocking_session_id,
        )
    }

    fn enqueue_waiter_inner(
        &self,
        request_id: Option<&str>,
        session_id: &str,
        workspace_id: &str,
        relative_path: &str,
        action: &str,
        blocking_session_id: Option<&str>,
    ) -> StoreResult<WaitRecord> {
        let relative_path = normalize_relative_path(relative_path);
        let existing = if let Some(request_id) = request_id {
            self.conn
                .query_row(
                    "SELECT wait_id FROM wait_queue
                     WHERE request_id = ?1
                        AND session_id = ?2
                        AND workspace_id = ?3
                        AND relative_path = ?4
                        AND action = ?5
                        AND status IN ('queued', 'reserved')
                     ORDER BY rowid DESC
                     LIMIT 1",
                    params![request_id, session_id, workspace_id, relative_path, action],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
        } else {
            self.conn
                .query_row(
                    "SELECT wait_id FROM wait_queue
                 WHERE session_id = ?1
                    AND workspace_id = ?2
                    AND relative_path = ?3
                    AND action = ?4
                    AND status IN ('queued', 'reserved')
                 ORDER BY rowid DESC
                 LIMIT 1",
                    params![session_id, workspace_id, relative_path, action],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
        };
        if let Some(wait_id) = existing {
            return self
                .waiter(&wait_id)
                .map(|waiter| waiter.expect("existing waiter should load"));
        }

        let wait_id = Uuid::new_v4().to_string();
        self.conn.execute(
            "INSERT INTO wait_queue (
                wait_id,
                session_id,
                workspace_id,
                relative_path,
                action,
                status,
                requested_at,
                reservation_expires_at,
                blocking_session_id,
                request_id
            ) VALUES (?1, ?2, ?3, ?4, ?5, 'queued', ?6, NULL, ?7, ?8)",
            params![
                wait_id,
                session_id,
                workspace_id,
                relative_path,
                action,
                "2026-05-31T00:00:00Z",
                blocking_session_id,
                request_id,
            ],
        )?;

        self.waiter(&wait_id)
            .map(|waiter| waiter.expect("inserted waiter should load"))
    }

    pub fn queue_position(&self, wait_id: impl AsRef<str>) -> StoreResult<Option<u64>> {
        let waiter = self.waiter(wait_id.as_ref())?;
        let Some(waiter) = waiter else {
            return Ok(None);
        };

        self.conn
            .query_row(
                "SELECT COUNT(*)
                 FROM wait_queue
                 WHERE workspace_id = ?1
                    AND relative_path = ?2
                    AND status = 'queued'
                    AND rowid <= (SELECT rowid FROM wait_queue WHERE wait_id = ?3)",
                params![waiter.workspace_id, waiter.relative_path, wait_id.as_ref()],
                |row| row.get::<_, u64>(0),
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn promote_next_waiter(
        &self,
        workspace_id: impl AsRef<str>,
        relative_path: impl AsRef<str>,
    ) -> StoreResult<Option<WaitRecord>> {
        let relative_path = normalize_relative_path(relative_path.as_ref());
        let wait_id = self
            .conn
            .query_row(
                "SELECT wait_id FROM wait_queue
                 WHERE workspace_id = ?1 AND relative_path = ?2 AND status = 'queued'
                 ORDER BY rowid ASC
                 LIMIT 1",
                params![workspace_id.as_ref(), relative_path],
                |row| row.get::<_, String>(0),
            )
            .optional()?;

        let Some(wait_id) = wait_id else {
            return Ok(None);
        };

        self.conn.execute(
            "UPDATE wait_queue
             SET status = 'reserved', reservation_expires_at = ?1
             WHERE wait_id = ?2 AND status = 'queued'",
            params!["2026-05-31T00:02:00Z", wait_id],
        )?;

        let waiter = self.waiter(&wait_id)?;
        if let Some(waiter) = &waiter {
            self.append_notification(
                &waiter.session_id,
                &waiter.workspace_id,
                "reservation_granted",
                serde_json::json!({
                    "wait_id": waiter.wait_id,
                    "relative_path": waiter.relative_path,
                    "action": waiter.action,
                    "reservation_expires_at": waiter.reservation_expires_at,
                }),
            )?;
        }

        Ok(waiter)
    }

    pub fn active_reservation(
        &self,
        workspace_id: impl AsRef<str>,
        relative_path: impl AsRef<str>,
    ) -> StoreResult<Option<WaitRecord>> {
        let relative_path = normalize_relative_path(relative_path.as_ref());
        self.conn
            .query_row(
                "SELECT
                    wait_id,
                    session_id,
                    workspace_id,
                    relative_path,
                    action,
                    status,
                    requested_at,
                    reservation_expires_at,
                    blocking_session_id,
                    request_id
                 FROM wait_queue
                 WHERE workspace_id = ?1 AND relative_path = ?2 AND status = 'reserved'
                 ORDER BY rowid ASC
                 LIMIT 1",
                params![workspace_id.as_ref(), relative_path],
                wait_record_from_row,
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn active_reservation_owner(
        &self,
        workspace_id: impl AsRef<str>,
        relative_path: impl AsRef<str>,
    ) -> StoreResult<Option<String>> {
        Ok(self
            .active_reservation(workspace_id, relative_path)?
            .map(|reservation| reservation.session_id))
    }

    pub fn next_reservation_for_session(
        &self,
        session_id: impl AsRef<str>,
        workspace_id: impl AsRef<str>,
    ) -> StoreResult<Option<WaitRecord>> {
        self.conn
            .query_row(
                "SELECT
                    wait_id,
                    session_id,
                    workspace_id,
                    relative_path,
                    action,
                    status,
                    requested_at,
                    reservation_expires_at,
                    blocking_session_id,
                    request_id
                 FROM wait_queue
                 WHERE session_id = ?1 AND workspace_id = ?2 AND status = 'reserved'
                 ORDER BY rowid ASC
                 LIMIT 1",
                params![session_id.as_ref(), workspace_id.as_ref()],
                wait_record_from_row,
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn claim_reservation(
        &self,
        wait_id: impl AsRef<str>,
        session_id: impl AsRef<str>,
    ) -> StoreResult<()> {
        let waiter = self.waiter(wait_id.as_ref())?;
        let Some(waiter) = waiter else {
            return Err(StoreError::ReservationOwnerMismatch);
        };
        if waiter.session_id != session_id.as_ref() || waiter.status != "reserved" {
            return Err(StoreError::ReservationOwnerMismatch);
        }

        self.conn.execute(
            "UPDATE wait_queue
             SET status = 'claimed'
             WHERE wait_id = ?1 AND status = 'reserved'",
            params![wait_id.as_ref()],
        )?;

        Ok(())
    }

    pub fn cancel_intent_request(
        &self,
        request_id: impl AsRef<str>,
        session_id: impl AsRef<str>,
    ) -> StoreResult<()> {
        let request_id = request_id.as_ref();
        let session_id = session_id.as_ref();
        let Some(request) = self.intent_request(request_id)? else {
            return Err(StoreError::ReservationOwnerMismatch);
        };
        if request.session_id != session_id {
            return Err(StoreError::ReservationOwnerMismatch);
        }

        let updated = self.conn.execute(
            "UPDATE intent_requests
             SET status = 'cancelled', updated_at = ?1
             WHERE request_id = ?2
                AND session_id = ?3
                AND status IN ('requested', 'queued', 'reserved')",
            params!["2026-05-31T00:00:00Z", request_id, session_id],
        )?;
        if updated == 0 {
            return Err(StoreError::ReservationOwnerMismatch);
        }

        self.conn.execute(
            "UPDATE wait_queue
             SET status = 'cancelled'
             WHERE request_id = ?1
                AND session_id = ?2
                AND status IN ('queued', 'reserved')",
            params![request_id, session_id],
        )?;

        Ok(())
    }

    pub fn expire_reservation(&self, wait_id: impl AsRef<str>) -> StoreResult<()> {
        self.conn.execute(
            "UPDATE wait_queue
             SET status = 'expired'
             WHERE wait_id = ?1 AND status = 'reserved'",
            params![wait_id.as_ref()],
        )?;

        Ok(())
    }

    pub fn waiter_status(&self, wait_id: impl AsRef<str>) -> StoreResult<Option<String>> {
        self.conn
            .query_row(
                "SELECT status FROM wait_queue WHERE wait_id = ?1",
                params![wait_id.as_ref()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn pending_notifications(
        &self,
        target_session_id: impl AsRef<str>,
    ) -> StoreResult<Vec<NotificationRecord>> {
        let mut statement = self.conn.prepare(
            "SELECT
                notification_id,
                target_session_id,
                workspace_id,
                kind,
                payload_json,
                status,
                created_at,
                expires_at
             FROM notifications
             WHERE target_session_id = ?1 AND status = 'pending'
             ORDER BY rowid ASC",
        )?;
        let rows = statement.query_map([target_session_id.as_ref()], notification_from_row)?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    fn append_notification(
        &self,
        target_session_id: &str,
        workspace_id: &str,
        kind: &str,
        payload: serde_json::Value,
    ) -> StoreResult<()> {
        self.conn.execute(
            "INSERT INTO notifications (
                notification_id,
                target_session_id,
                workspace_id,
                kind,
                payload_json,
                status,
                created_at,
                expires_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6, ?7)",
            params![
                Uuid::new_v4().to_string(),
                target_session_id,
                workspace_id,
                kind,
                payload.to_string(),
                "2026-05-31T00:00:00Z",
                "2026-05-31T00:02:00Z",
            ],
        )?;

        Ok(())
    }

    pub fn append_activity(
        &self,
        session_id: impl AsRef<str>,
        workspace_id: impl AsRef<str>,
    ) -> StoreResult<()> {
        self.conn.execute(
            "INSERT INTO activities (
                activity_id,
                session_id,
                workspace_id,
                expires_at
            ) VALUES (?1, ?2, ?3, ?4)",
            params![
                Uuid::new_v4().to_string(),
                session_id.as_ref(),
                workspace_id.as_ref(),
                "2026-05-31T00:15:00Z",
            ],
        )?;

        Ok(())
    }

    pub fn activity_count(&self) -> StoreResult<u64> {
        self.conn
            .query_row("SELECT COUNT(*) FROM activities", [], |row| {
                row.get::<_, u64>(0)
            })
            .map_err(StoreError::from)
    }

    pub fn has_table(&self, table_name: &str) -> StoreResult<bool> {
        self.schema_object_exists("table", table_name)
    }

    pub fn has_index(&self, index_name: &str) -> StoreResult<bool> {
        self.schema_object_exists("index", index_name)
    }

    pub fn has_column(&self, table_name: &str, column_name: &str) -> StoreResult<bool> {
        self.conn
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2
                 )",
                params![table_name, column_name],
                |row| row.get::<_, bool>(0),
            )
            .map_err(StoreError::from)
    }

    fn schema_object_exists(&self, object_type: &str, name: &str) -> StoreResult<bool> {
        self.conn
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM sqlite_master WHERE type = ?1 AND name = ?2
                 )",
                params![object_type, name],
                |row| row.get::<_, bool>(0),
            )
            .map_err(StoreError::from)
    }

    pub fn policy_state_for_session(&self, session_id: &str) -> StoreResult<PolicyState> {
        let scopes = self
            .conn
            .query_row(
                "SELECT scopes_json FROM intents
                 WHERE session_id = ?1 AND status = 'active'
                 ORDER BY declared_at DESC
                 LIMIT 1",
                [session_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;

        let Some(scopes_json) = scopes else {
            return Ok(PolicyState::default());
        };

        let scopes: Vec<IntentScope> = serde_json::from_str(&scopes_json).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
        })?;

        Ok(PolicyState::default().with_active_intent_scopes(scopes))
    }

    fn migrate(&self) -> StoreResult<()> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS schema_migrations (
                version TEXT PRIMARY KEY,
                applied_at TEXT NOT NULL
            );

            INSERT OR IGNORE INTO schema_migrations (version, applied_at)
            VALUES ('stateful.v1.initial', '2026-05-31T00:00:00Z');

            CREATE TABLE IF NOT EXISTS events (
                event_id TEXT PRIMARY KEY,
                event_type TEXT NOT NULL,
                session_id TEXT NOT NULL,
                workspace_id TEXT NOT NULL,
                sequence INTEGER,
                repo_id TEXT,
                worktree_id TEXT,
                root TEXT,
                branch TEXT,
                payload_json TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_events_workspace_created_at
                ON events(workspace_id, created_at);

            CREATE INDEX IF NOT EXISTS idx_events_session_created_at
                ON events(session_id, created_at);

            CREATE INDEX IF NOT EXISTS idx_events_session_sequence
                ON events(session_id, sequence);

            CREATE TABLE IF NOT EXISTS sessions (
                session_id TEXT PRIMARY KEY,
                workspace_id TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_sessions_workspace_session
                ON sessions(workspace_id, session_id);

            CREATE TABLE IF NOT EXISTS activities (
                activity_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                workspace_id TEXT NOT NULL,
                expires_at TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_activities_workspace_expires_at
                ON activities(workspace_id, expires_at);

            CREATE TABLE IF NOT EXISTS intents (
                intent_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                workspace_id TEXT NOT NULL,
                scopes_json TEXT NOT NULL,
                status TEXT NOT NULL,
                declared_at TEXT NOT NULL,
                expires_at TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_intents_session_status_expires_at
                ON intents(session_id, status, expires_at);

            CREATE TABLE IF NOT EXISTS leases (
                lease_id TEXT PRIMARY KEY,
                session_id TEXT,
                workspace_id TEXT NOT NULL,
                repo_id TEXT,
                relative_path TEXT,
                absolute_path TEXT,
                status TEXT NOT NULL,
                expires_at TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_leases_workspace_path_status
                ON leases(workspace_id, relative_path, status);

            CREATE INDEX IF NOT EXISTS idx_leases_workspace_absolute_status_expires_at
                ON leases(workspace_id, absolute_path, status, expires_at);

            CREATE INDEX IF NOT EXISTS idx_leases_repo_relative_status_expires_at
                ON leases(repo_id, relative_path, status, expires_at);

            CREATE TABLE IF NOT EXISTS intent_requests (
                request_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                workspace_id TEXT NOT NULL,
                resources_json TEXT NOT NULL,
                action TEXT NOT NULL,
                status TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_intent_requests_session_status
                ON intent_requests(session_id, status);

            CREATE TABLE IF NOT EXISTS wait_queue (
                wait_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                workspace_id TEXT NOT NULL,
                relative_path TEXT NOT NULL,
                action TEXT NOT NULL,
                status TEXT NOT NULL,
                requested_at TEXT NOT NULL,
                reservation_expires_at TEXT,
                blocking_session_id TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_wait_queue_workspace_path_status
                ON wait_queue(workspace_id, relative_path, status);

            CREATE INDEX IF NOT EXISTS idx_wait_queue_session_status
                ON wait_queue(session_id, status);

            CREATE TABLE IF NOT EXISTS notifications (
                notification_id TEXT PRIMARY KEY,
                target_session_id TEXT NOT NULL,
                workspace_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                status TEXT NOT NULL,
                created_at TEXT NOT NULL,
                expires_at TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_notifications_session_status
                ON notifications(target_session_id, status);

            CREATE TABLE IF NOT EXISTS conflicts (
                conflict_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                checked_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_conflicts_session_checked_at
                ON conflicts(session_id, checked_at);

            CREATE TABLE IF NOT EXISTS overrides (
                override_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                status TEXT NOT NULL,
                expires_at TEXT
            );

            CREATE TABLE IF NOT EXISTS validations (
                validation_id TEXT PRIMARY KEY,
                workspace_id TEXT NOT NULL,
                profile_id TEXT NOT NULL,
                status TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_validations_workspace_profile_status
                ON validations(workspace_id, profile_id, status);

            CREATE TABLE IF NOT EXISTS reconciliations (
                reconciliation_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_reconciliations_session_created_at
                ON reconciliations(session_id, created_at);

            CREATE TABLE IF NOT EXISTS human_observations (
                observation_id TEXT PRIMARY KEY,
                workspace_id TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS outbox (
                outbox_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                sequence INTEGER NOT NULL,
                sync_status TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_outbox_session_sequence_sync_status
                ON outbox(session_id, sequence, sync_status);
            ",
        )?;

        self.add_column_if_missing("events", "repo_id", "TEXT")?;
        self.add_column_if_missing("events", "worktree_id", "TEXT")?;
        self.add_column_if_missing("events", "root", "TEXT")?;
        self.add_column_if_missing("events", "branch", "TEXT")?;
        self.add_column_if_missing("wait_queue", "request_id", "TEXT")?;

        Ok(())
    }

    fn add_column_if_missing(
        &self,
        table: &str,
        column: &str,
        column_type: &str,
    ) -> StoreResult<()> {
        if !matches!(
            (table, column, column_type),
            ("events", "repo_id", "TEXT")
                | ("events", "worktree_id", "TEXT")
                | ("events", "root", "TEXT")
                | ("events", "branch", "TEXT")
                | ("wait_queue", "request_id", "TEXT")
        ) {
            return Ok(());
        }

        let mut statement = self.conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        if !columns.iter().any(|name| name == column) {
            self.conn.execute_batch(&format!(
                "ALTER TABLE {table} ADD COLUMN {column} {column_type};"
            ))?;
        }
        Ok(())
    }

    fn materialize(&self, event: &Event) -> StoreResult<()> {
        match event.event_type {
            EventType::SessionRegistered | EventType::SessionHeartbeat => {
                self.conn.execute(
                    "INSERT INTO sessions (session_id, workspace_id, updated_at)
                     VALUES (?1, ?2, ?3)
                     ON CONFLICT(session_id) DO UPDATE SET
                        workspace_id = excluded.workspace_id,
                        updated_at = excluded.updated_at",
                    params![event.session_id, event.workspace_id, event.created_at],
                )?;
            }
            EventType::IntentDeclared => {
                let scopes_json = event.payload["scopes"].to_string();
                self.conn.execute(
                    "UPDATE intents SET status = 'superseded'
                     WHERE session_id = ?1 AND status = 'active'",
                    params![event.session_id],
                )?;
                self.conn.execute(
                    "INSERT INTO intents (
                        intent_id,
                        session_id,
                        workspace_id,
                        scopes_json,
                        status,
                        declared_at,
                        expires_at
                    ) VALUES (?1, ?2, ?3, ?4, 'active', ?5, ?6)",
                    params![
                        event.event_id,
                        event.session_id,
                        event.workspace_id,
                        scopes_json,
                        event.created_at,
                        "2026-05-31T00:15:00Z",
                    ],
                )?;
            }
        }

        Ok(())
    }

    fn waiter(&self, wait_id: &str) -> StoreResult<Option<WaitRecord>> {
        self.conn
            .query_row(
                "SELECT
                    wait_id,
                    session_id,
                    workspace_id,
                    relative_path,
                    action,
                    status,
                    requested_at,
                    reservation_expires_at,
                    blocking_session_id,
                    request_id
                 FROM wait_queue
                 WHERE wait_id = ?1",
                params![wait_id],
                wait_record_from_row,
            )
            .optional()
            .map_err(StoreError::from)
    }
}

fn normalize_relative_path(path: &str) -> String {
    path.replace('\\', "/")
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .fold(Vec::new(), |mut segments, segment| {
            if segment == ".." {
                segments.pop();
            } else {
                segments.push(segment);
            }
            segments
        })
        .join("/")
}

fn wait_record_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WaitRecord> {
    Ok(WaitRecord {
        wait_id: row.get(0)?,
        session_id: row.get(1)?,
        workspace_id: row.get(2)?,
        relative_path: row.get(3)?,
        action: row.get(4)?,
        status: row.get(5)?,
        requested_at: row.get(6)?,
        reservation_expires_at: row.get(7)?,
        blocking_session_id: row.get(8)?,
        request_id: row.get(9)?,
    })
}

fn intent_request_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<IntentRequestRecord> {
    let resources_json: String = row.get(3)?;
    let resources = serde_json::from_str(&resources_json).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(err))
    })?;

    Ok(IntentRequestRecord {
        request_id: row.get(0)?,
        session_id: row.get(1)?,
        workspace_id: row.get(2)?,
        resources,
        action: row.get(4)?,
        status: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

fn notification_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<NotificationRecord> {
    let payload_json: String = row.get(4)?;
    let payload = serde_json::from_str(&payload_json).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(err))
    })?;
    Ok(NotificationRecord {
        notification_id: row.get(0)?,
        target_session_id: row.get(1)?,
        workspace_id: row.get(2)?,
        kind: row.get(3)?,
        payload,
        status: row.get(5)?,
        created_at: row.get(6)?,
        expires_at: row.get(7)?,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub event_id: String,
    pub event_type: EventType,
    pub session_id: String,
    pub workspace_id: String,
    pub repo_id: Option<String>,
    pub worktree_id: Option<String>,
    pub root: Option<String>,
    pub branch: Option<String>,
    pub payload: serde_json::Value,
    pub created_at: String,
}

impl Event {
    pub fn session_registered(
        session_id: impl Into<String>,
        workspace_id: impl Into<String>,
    ) -> Self {
        let session_id = session_id.into();
        let workspace_id = workspace_id.into();

        Self {
            event_id: Uuid::new_v4().to_string(),
            event_type: EventType::SessionRegistered,
            payload: serde_json::json!({
                "session_id": session_id,
                "workspace_id": workspace_id,
            }),
            session_id,
            workspace_id,
            repo_id: None,
            worktree_id: None,
            root: None,
            branch: None,
            created_at: "2026-05-31T00:00:00Z".to_string(),
        }
    }

    pub fn session_heartbeat(
        session_id: impl Into<String>,
        workspace_id: impl Into<String>,
    ) -> Self {
        let session_id = session_id.into();
        let workspace_id = workspace_id.into();

        Self {
            event_id: Uuid::new_v4().to_string(),
            event_type: EventType::SessionHeartbeat,
            payload: serde_json::json!({
                "session_id": session_id,
                "workspace_id": workspace_id,
            }),
            session_id,
            workspace_id,
            repo_id: None,
            worktree_id: None,
            root: None,
            branch: None,
            created_at: "2026-05-31T00:00:00Z".to_string(),
        }
    }

    pub fn with_event_id(mut self, event_id: impl Into<String>) -> Self {
        self.event_id = event_id.into();
        self
    }

    pub fn with_workspace_identity(
        mut self,
        repo_id: impl Into<String>,
        worktree_id: impl Into<String>,
        root: impl Into<String>,
        branch: impl Into<String>,
    ) -> Self {
        self.repo_id = Some(repo_id.into());
        self.worktree_id = Some(worktree_id.into());
        self.root = Some(root.into());
        self.branch = Some(branch.into());
        self
    }

    pub fn intent_declared<I, S>(
        session_id: impl Into<String>,
        workspace_id: impl Into<String>,
        files_planned: I,
    ) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let session_id = session_id.into();
        let workspace_id = workspace_id.into();
        let scopes = files_planned
            .into_iter()
            .map(|path| {
                let path = path.as_ref();
                if path.ends_with('/') {
                    IntentScope::directory(path)
                } else {
                    IntentScope::file(path)
                }
            })
            .collect::<Vec<_>>();

        Self {
            event_id: Uuid::new_v4().to_string(),
            event_type: EventType::IntentDeclared,
            payload: serde_json::json!({
                "session_id": session_id,
                "workspace_id": workspace_id,
                "scopes": scopes,
            }),
            session_id,
            workspace_id,
            repo_id: None,
            worktree_id: None,
            root: None,
            branch: None,
            created_at: "2026-05-31T00:00:00Z".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventType {
    SessionRegistered,
    SessionHeartbeat,
    IntentDeclared,
}

impl EventType {
    fn as_str(self) -> &'static str {
        match self {
            Self::SessionRegistered => "SessionRegistered",
            Self::SessionHeartbeat => "SessionHeartbeat",
            Self::IntentDeclared => "IntentDeclared",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord {
    pub session_id: String,
    pub workspace_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CurrentSummary {
    pub session_count: u64,
    pub active_intent_count: u64,
    pub event_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EventRecord {
    pub event_id: String,
    pub event_type: String,
    pub session_id: String,
    pub workspace_id: String,
    pub repo_id: Option<String>,
    pub worktree_id: Option<String>,
    pub root: Option<String>,
    pub branch: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WaitRecord {
    pub wait_id: String,
    pub session_id: String,
    pub workspace_id: String,
    pub relative_path: String,
    pub action: String,
    pub status: String,
    pub requested_at: String,
    pub reservation_expires_at: Option<String>,
    pub blocking_session_id: Option<String>,
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntentRequestRecord {
    pub request_id: String,
    pub session_id: String,
    pub workspace_id: String,
    pub resources: Vec<String>,
    pub action: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NotificationRecord {
    pub notification_id: String,
    pub target_session_id: String,
    pub workspace_id: String,
    pub kind: String,
    pub payload: serde_json::Value,
    pub status: String,
    pub created_at: String,
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxEntry {
    pub outbox_id: String,
    pub session_id: String,
    pub sequence: u64,
    pub sync_status: SyncStatus,
}

impl OutboxEntry {
    pub fn new(outbox_id: impl Into<String>, session_id: impl Into<String>, sequence: u64) -> Self {
        Self {
            outbox_id: outbox_id.into(),
            session_id: session_id.into(),
            sequence,
            sync_status: SyncStatus::Pending,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncStatus {
    Pending,
}

impl SyncStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
        }
    }
}
