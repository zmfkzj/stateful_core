use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use stateful_core::{
    AGENT_CONTEXT_SCOPE_SOURCE_REF, ActivityPhase, CurrentEvidenceKind, CurrentFreshness,
    CurrentItem, CurrentItemKind, CurrentSeverity, PolicyState, ReservationScope,
    normalize_relative_path,
};
use std::time::Duration as StdDuration;
use std::{fs, path::Path};
use thiserror::Error;
use time::{Date, Duration, Month, OffsetDateTime, Time};
use uuid::Uuid;

pub const CRATE_NAME: &str = "stateful-store";

const ACTIVE_CLAIMABLE_RESERVATION_TTL_SECONDS: i64 = 900;
const ACTIVE_RESERVATION_MAX_SECONDS: i64 = 3600;
const CLAIM_TTL_SECONDS: i64 = 300;
const CLAIMABLE_RESERVATION_TTL_SECONDS: i64 = 120;
const ACTIVITY_TTL_SECONDS: i64 = 900;
const EVENT_RETENTION_DAYS: i64 = 14;
const SQLITE_BUSY_TIMEOUT_MS: u64 = 5_000;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("reservation owner mismatch")]
    ReservationOwnerMismatch,
    #[error("reservation request not found")]
    ReservationRequestNotFound,
    #[error("reservation request owner mismatch")]
    ReservationRequestOwnerMismatch,
    #[error("reservation request is not cancelable")]
    ReservationRequestNotCancelable,
    #[error("active claim conflict")]
    ClaimConflict,
    #[error("claim is already held by this session")]
    ClaimAlreadyHeld,
    #[error("claim not found")]
    ClaimNotFound,
    #[error("claim owner mismatch")]
    ClaimOwnerMismatch,
    #[error("matching active reservation is required")]
    MissingReservation,
    #[error(
        "invalid claim path `{0}`: direct tmp claims are not allowed; claim a file or subdirectory under tmp instead"
    )]
    InvalidClaimPath(String),
    #[error("purpose is required")]
    MissingPurpose,
    #[error("reservation scope is required")]
    MissingScope,
    #[error("invalid timestamp: {0}")]
    InvalidTimestamp(String),
}

pub type StoreResult<T> = Result<T, StoreError>;

#[derive(Debug, Clone, Copy, Default)]
pub struct CurrentStateIdentityFilter<'a> {
    pub repo_id: Option<&'a str>,
    pub worktree_id: Option<&'a str>,
    pub root: Option<&'a str>,
    pub exclude_agent_id: Option<&'a str>,
}

impl CurrentStateIdentityFilter<'_> {
    fn is_empty(self) -> bool {
        self.repo_id.is_none() && self.worktree_id.is_none() && self.root.is_none()
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct WorkspaceIdentity<'a> {
    pub repo_id: Option<&'a str>,
    pub worktree_id: Option<&'a str>,
    pub root: Option<&'a str>,
    pub branch: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimObservation {
    pub exists: bool,
    pub content_hash: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaimBatchAcquireResult {
    pub acquired: usize,
    pub already_held: usize,
}

impl WorkspaceIdentity<'_> {
    fn is_empty(self) -> bool {
        self.repo_id.is_none()
            && self.worktree_id.is_none()
            && self.root.is_none()
            && self.branch.is_none()
    }
}

#[derive(Debug)]
pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> StoreResult<Self> {
        let path = path.as_ref();
        prepare_private_database_path(path)?;

        let conn = Connection::open(path)?;
        configure_file_connection(&conn)?;
        restrict_database_file_permissions(path)?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn transaction<T, E, F, M>(&self, operation: F, map_store_error: M) -> Result<T, E>
    where
        F: FnOnce(&Self) -> Result<T, E>,
        M: Fn(StoreError) -> E,
    {
        if !self.conn.is_autocommit() {
            return operation(self);
        }

        self.conn
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| map_store_error(StoreError::from(error)))?;

        let result = (|| -> Result<T, E> {
            let value = operation(self)?;
            self.conn
                .execute_batch("COMMIT")
                .map_err(|error| map_store_error(StoreError::from(error)))?;
            Ok(value)
        })();

        if result.is_err() {
            let _ = self.conn.execute_batch("ROLLBACK");
        }

        result
    }

    fn store_transaction<T>(
        &self,
        operation: impl FnOnce(&Self) -> StoreResult<T>,
    ) -> StoreResult<T> {
        self.transaction(operation, |error| error)
    }

    pub fn open_in_memory() -> StoreResult<Self> {
        let conn = Connection::open_in_memory()?;
        configure_connection(&conn)?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn append(&self, event: Event) -> StoreResult<()> {
        if !self.conn.is_autocommit() {
            return self.append_inner(&event);
        }

        self.conn.execute_batch("BEGIN IMMEDIATE")?;

        let result = (|| -> StoreResult<()> {
            self.append_inner(&event)?;
            self.conn.execute_batch("COMMIT")?;
            Ok(())
        })();

        if result.is_err() {
            let _ = self.conn.execute_batch("ROLLBACK");
        }

        result
    }

    fn append_inner(&self, event: &Event) -> StoreResult<()> {
        let inserted = self.conn.execute(
            "INSERT OR IGNORE INTO events (
                event_id,
                event_type,
                agent_id,
                workspace_id,
                repo_id,
                worktree_id,
                root,
                branch,
                payload_json,
                created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                &event.event_id,
                event.event_type.as_str(),
                &event.agent_id,
                &event.workspace_id,
                &event.repo_id,
                &event.worktree_id,
                &event.root,
                &event.branch,
                serde_json::to_string(&event.payload)
                    .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))?,
                &event.created_at,
            ],
        )?;

        if inserted == 1 {
            self.materialize(event)?;
        }

        Ok(())
    }

    pub fn agent(&self, agent_id: &str) -> StoreResult<Option<SessionRecord>> {
        self.conn
            .query_row(
                "SELECT agent_id, workspace_id FROM agents WHERE agent_id = ?1",
                [agent_id],
                |row| {
                    Ok(SessionRecord {
                        agent_id: row.get(0)?,
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
        self.current_summary_filtered(None, CurrentStateIdentityFilter::default())
    }

    pub fn current_summary_for_workspace(
        &self,
        workspace_id: impl AsRef<str>,
    ) -> StoreResult<CurrentSummary> {
        self.current_summary_filtered(
            Some(workspace_id.as_ref()),
            CurrentStateIdentityFilter::default(),
        )
    }

    fn current_summary_filtered(
        &self,
        workspace_filter: Option<&str>,
        identity_filter: CurrentStateIdentityFilter<'_>,
    ) -> StoreResult<CurrentSummary> {
        self.expire_stale()?;
        if !identity_filter.is_empty() {
            return self.current_summary_for_identity(workspace_filter, identity_filter);
        }
        let agent_count = match workspace_filter {
            Some(workspace_id) => self.conn.query_row(
                "SELECT COUNT(*) FROM agents WHERE workspace_id = ?1",
                [workspace_id],
                |row| row.get::<_, u64>(0),
            )?,
            None => self
                .conn
                .query_row("SELECT COUNT(*) FROM agents", [], |row| {
                    row.get::<_, u64>(0)
                })?,
        };
        let active_reservation_count = match workspace_filter {
            Some(workspace_id) => self.conn.query_row(
                "SELECT COUNT(*) FROM reservations WHERE status = 'active' AND workspace_id = ?1",
                [workspace_id],
                |row| row.get::<_, u64>(0),
            )?,
            None => self.conn.query_row(
                "SELECT COUNT(*) FROM reservations WHERE status = 'active'",
                [],
                |row| row.get::<_, u64>(0),
            )?,
        };
        let event_count = match workspace_filter {
            Some(workspace_id) => self.conn.query_row(
                "SELECT COUNT(*) FROM events WHERE workspace_id = ?1",
                [workspace_id],
                |row| row.get::<_, u64>(0),
            )?,
            None => self.event_count()?,
        };

        Ok(CurrentSummary {
            agent_count,
            active_reservation_count,
            event_count,
        })
    }

    fn current_summary_for_identity(
        &self,
        workspace_filter: Option<&str>,
        identity_filter: CurrentStateIdentityFilter<'_>,
    ) -> StoreResult<CurrentSummary> {
        let mut event_statement = self.conn.prepare(
            "SELECT agent_id, workspace_id, repo_id, worktree_id, root
             FROM events",
        )?;
        let event_rows = event_statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })?;
        let event_rows = event_rows.collect::<Result<Vec<_>, _>>()?;
        let mut agents = std::collections::BTreeSet::new();
        let mut event_count = 0u64;
        for (agent_id, workspace_id, repo_id, worktree_id, root) in event_rows {
            if workspace_filter.is_some_and(|filter| workspace_id != filter) {
                continue;
            }
            if !identity_filter_matches(
                identity_filter,
                repo_id.as_deref(),
                worktree_id.as_deref(),
                root.as_deref(),
            ) {
                continue;
            }
            event_count += 1;
            agents.insert(agent_id);
        }

        let mut reservation_statement = self.conn.prepare(
            "SELECT i.workspace_id, e.repo_id, e.worktree_id, e.root
             FROM reservations i
             LEFT JOIN events e ON e.event_id = i.reservation_id
             WHERE i.status = 'active'",
        )?;
        let reservation_rows = reservation_statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })?;
        let reservation_rows = reservation_rows.collect::<Result<Vec<_>, _>>()?;
        let active_reservation_count = reservation_rows
            .into_iter()
            .filter(|(workspace_id, repo_id, worktree_id, root)| {
                workspace_filter.is_none_or(|filter| workspace_id == filter)
                    && identity_filter_matches(
                        identity_filter,
                        repo_id.as_deref(),
                        worktree_id.as_deref(),
                        root.as_deref(),
                    )
            })
            .count() as u64;

        Ok(CurrentSummary {
            agent_count: agents.len() as u64,
            active_reservation_count,
            event_count,
        })
    }

    pub fn live_current_state(
        &self,
        resource_filter: Option<&str>,
    ) -> StoreResult<LiveCurrentState> {
        self.live_current_state_filtered(
            None,
            CurrentStateIdentityFilter::default(),
            resource_filter,
        )
    }

    pub fn live_current_state_for_workspace(
        &self,
        workspace_id: impl AsRef<str>,
        resource_filter: Option<&str>,
    ) -> StoreResult<LiveCurrentState> {
        self.live_current_state_filtered(
            Some(workspace_id.as_ref()),
            CurrentStateIdentityFilter::default(),
            resource_filter,
        )
    }

    pub fn live_current_state_for_workspace_identity(
        &self,
        workspace_id: impl AsRef<str>,
        identity_filter: CurrentStateIdentityFilter<'_>,
        resource_filter: Option<&str>,
    ) -> StoreResult<LiveCurrentState> {
        self.live_current_state_filtered(
            Some(workspace_id.as_ref()),
            identity_filter,
            resource_filter,
        )
    }

    fn live_current_state_filtered(
        &self,
        workspace_filter: Option<&str>,
        identity_filter: CurrentStateIdentityFilter<'_>,
        resource_filter: Option<&str>,
    ) -> StoreResult<LiveCurrentState> {
        self.refresh_live_current_state()?;
        let summary = self.current_summary_filtered(workspace_filter, identity_filter)?;
        let resource_filter = resource_filter.map(normalize_relative_path);
        let mut items = Vec::new();

        items.extend(self.live_intent_items(
            workspace_filter,
            identity_filter,
            resource_filter.as_deref(),
        )?);
        items.extend(self.live_claim_items(
            workspace_filter,
            identity_filter,
            resource_filter.as_deref(),
        )?);
        items.extend(self.live_wait_queue_items(
            workspace_filter,
            identity_filter,
            resource_filter.as_deref(),
        )?);

        Ok(LiveCurrentState { summary, items })
    }
    fn live_intent_items(
        &self,
        workspace_filter: Option<&str>,
        identity_filter: CurrentStateIdentityFilter<'_>,
        resource_filter: Option<&str>,
    ) -> StoreResult<Vec<CurrentItem>> {
        let mut statement = self.conn.prepare(
            "SELECT
                i.agent_id,
                i.workspace_id,
                i.scopes_json,
                i.purpose,
                i.declared_at,
                i.expires_at,
                e.repo_id,
                e.worktree_id,
                e.root
             FROM reservations i
             LEFT JOIN events e ON e.event_id = i.reservation_id
             WHERE i.status = 'active'
             ORDER BY i.declared_at DESC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
            ))
        })?;
        let rows = rows.collect::<Result<Vec<_>, _>>()?;
        let mut items = Vec::new();

        for (
            agent_id,
            workspace_id,
            scopes_json,
            purpose,
            declared_at,
            expires_at,
            repo_id,
            worktree_id,
            root,
        ) in rows
        {
            if workspace_filter.is_some_and(|filter| workspace_id != filter) {
                continue;
            }
            let current_agent_scope = identity_filter.exclude_agent_id == Some(agent_id.as_str());
            if !identity_filter_matches(
                identity_filter,
                repo_id.as_deref(),
                worktree_id.as_deref(),
                root.as_deref(),
            ) {
                continue;
            }
            let scopes: Vec<ReservationScope> =
                serde_json::from_str(&scopes_json).map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(err),
                    )
                })?;
            for scope in scopes {
                let resource = intent_scope_resource(&scope);
                if !resource_matches_filter(&resource, resource_filter) {
                    continue;
                }
                let summary = if current_agent_scope {
                    format!("This session declared reservation for {resource}.")
                } else {
                    format!("Agent {agent_id} declared reservation for {resource}.")
                };
                let next_action = if current_agent_scope {
                    format!(
                        "Before writing {resource}, keep an exact same-reservation file claim active."
                    )
                } else {
                    format!(
                        "Avoid overlapping edits to {resource} unless coordinating with agent {agent_id}."
                    )
                };
                let mut item = CurrentItem::new(
                    CurrentItemKind::Reservation,
                    CurrentSeverity::Info,
                    CurrentFreshness::Live,
                    resource.clone(),
                    purpose.clone(),
                    summary,
                )
                .with_next_action(next_action)
                .with_agent(agent_id.clone())
                .with_workspace(workspace_id.clone())
                .with_source_ref("ReservationDeclared")
                .with_evidence_kind(CurrentEvidenceKind::DeclaredReservation)
                .with_observed_at(declared_at.clone())
                .with_expires_at(expires_at.clone());
                if current_agent_scope {
                    item = item.with_source_ref(AGENT_CONTEXT_SCOPE_SOURCE_REF);
                }
                items.push(item);
            }
        }

        Ok(items)
    }

    fn active_reservation_purpose_for_lease(
        &self,
        agent_id: &str,
        workspace_id: &str,
        relative_path: &str,
        lease_is_directory: bool,
    ) -> StoreResult<Option<String>> {
        self.active_reservation_for_lease(
            agent_id,
            workspace_id,
            relative_path,
            lease_is_directory,
            None,
        )
        .map(|reservation| reservation.map(|(_, purpose)| purpose))
    }

    pub fn reservation_id_for_active_scope(
        &self,
        agent_id: &str,
        workspace_id: &str,
        relative_path: &str,
        lease_is_directory: bool,
    ) -> StoreResult<Option<String>> {
        self.expire_stale()?;
        let relative_path = normalize_relative_path(relative_path);
        self.active_reservation_for_lease(
            agent_id,
            workspace_id,
            &relative_path,
            lease_is_directory,
            None,
        )
        .map(|reservation| reservation.map(|(reservation_id, _)| reservation_id))
    }

    fn active_reservation_for_lease(
        &self,
        agent_id: &str,
        workspace_id: &str,
        relative_path: &str,
        lease_is_directory: bool,
        reservation_id: Option<&str>,
    ) -> StoreResult<Option<(String, String)>> {
        for (active_reservation_id, scopes, purpose) in
            self.active_reservation_scope_rows(agent_id, workspace_id)?
        {
            if let Some(reservation_id) = reservation_id {
                if reservation_id != active_reservation_id {
                    continue;
                }
            }
            let covers_lease = scopes.iter().any(|scope| {
                if lease_is_directory {
                    scope.allows_write_directory(relative_path)
                } else {
                    scope.allows_write(relative_path)
                }
            });

            if covers_lease {
                return Ok(Some((active_reservation_id, purpose)));
            }
        }

        let Some(reservation_id) = reservation_id else {
            return Ok(None);
        };
        self.wait_queue_reservation_purpose_for_lease(
            reservation_id,
            agent_id,
            workspace_id,
            relative_path,
            lease_is_directory,
        )
        .map(|purpose| purpose.map(|purpose| (reservation_id.to_string(), purpose)))
    }

    fn wait_queue_reservation_purpose_for_lease(
        &self,
        reservation_id: &str,
        agent_id: &str,
        workspace_id: &str,
        relative_path: &str,
        lease_is_directory: bool,
    ) -> StoreResult<Option<String>> {
        let reservation = self
            .conn
            .query_row(
                "SELECT relative_path, action, purpose
                 FROM wait_queue
                 WHERE wait_id = ?1
                    AND agent_id = ?2
                    AND workspace_id = ?3
                    AND status = 'reserved'",
                params![reservation_id, agent_id, workspace_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        let Some((wait_path, action, purpose)) = reservation else {
            return Ok(None);
        };
        let wait_path = normalize_relative_path(&wait_path);
        let covers_lease = if lease_is_directory {
            action == "write_directory" && wait_path == relative_path
        } else if action == "write_directory" {
            relative_path.starts_with(&format!("{wait_path}/"))
        } else {
            wait_path == relative_path
        };
        Ok(covers_lease.then_some(purpose))
    }

    fn active_reservation_scope_rows(
        &self,
        agent_id: &str,
        workspace_id: &str,
    ) -> StoreResult<Vec<(String, Vec<ReservationScope>, String)>> {
        let mut statement = self.conn.prepare(
            "SELECT reservation_id, scopes_json, purpose
             FROM reservations
             WHERE agent_id = ?1 AND workspace_id = ?2 AND status = 'active'
             ORDER BY declared_at DESC, rowid DESC",
        )?;
        let rows = statement
            .query_map(params![agent_id, workspace_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut parsed_rows = Vec::with_capacity(rows.len());
        for (reservation_id, scopes_json, purpose) in rows {
            let scopes: Vec<ReservationScope> =
                serde_json::from_str(&scopes_json).map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(err),
                    )
                })?;
            parsed_rows.push((reservation_id, scopes, purpose));
        }

        Ok(parsed_rows)
    }

    fn live_claim_items(
        &self,
        workspace_filter: Option<&str>,
        identity_filter: CurrentStateIdentityFilter<'_>,
        resource_filter: Option<&str>,
    ) -> StoreResult<Vec<CurrentItem>> {
        let mut statement = self.conn.prepare(
            "SELECT agent_id, workspace_id, relative_path, action, purpose, expires_at
             FROM claims
             WHERE status = 'active' AND relative_path IS NOT NULL
             ORDER BY rowid ASC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })?;
        let rows = rows.collect::<Result<Vec<_>, _>>()?;
        let mut items = Vec::new();

        for (agent_id, workspace_id, relative_path, action, purpose, expires_at) in rows {
            if workspace_filter.is_some_and(|filter| workspace_id != filter) {
                continue;
            }
            let current_agent_scope = identity_filter
                .exclude_agent_id
                .is_some_and(|excluded| agent_id.as_deref() == Some(excluded));
            if !identity_filter.is_empty() {
                let Some(agent_id) = agent_id.as_deref() else {
                    continue;
                };
                if !self.agent_active_reservation_matches_identity(
                    agent_id,
                    &workspace_id,
                    identity_filter,
                )? {
                    continue;
                }
            }
            if !resource_matches_filter(&relative_path, resource_filter) {
                continue;
            }
            let Some(purpose) = purpose.filter(|purpose| !purpose.trim().is_empty()) else {
                continue;
            };
            let session_summary = agent_id.as_deref().unwrap_or("unknown session");
            let resource = if action == "write_directory" {
                format!("{relative_path}/")
            } else {
                relative_path.clone()
            };
            let (severity, summary, next_action) = if current_agent_scope {
                (
                    CurrentSeverity::Info,
                    format!("This session has an active write claim on {resource}."),
                    format!(
                        "You can write {resource} while this same-reservation claim remains fresh."
                    ),
                )
            } else {
                (
                    CurrentSeverity::Block,
                    format!("{session_summary} has an active write claim on {resource}."),
                    format!("Wait for the claim to release, or coordinate with {session_summary}."),
                )
            };
            let mut item = CurrentItem::new(
                CurrentItemKind::Claim,
                severity,
                CurrentFreshness::Live,
                resource.clone(),
                purpose,
                summary,
            )
            .with_next_action(next_action)
            .with_workspace(workspace_id.clone())
            .with_source_ref("ClaimAcquired")
            .with_evidence_kind(CurrentEvidenceKind::ClaimOnly)
            .with_expires_at(expires_at);
            if let Some(agent_id) = agent_id {
                item = item.with_agent(agent_id);
            }
            if current_agent_scope {
                item = item.with_source_ref(AGENT_CONTEXT_SCOPE_SOURCE_REF);
            }
            items.push(item);
        }

        Ok(items)
    }

    fn live_wait_queue_items(
        &self,
        workspace_filter: Option<&str>,
        identity_filter: CurrentStateIdentityFilter<'_>,
        resource_filter: Option<&str>,
    ) -> StoreResult<Vec<CurrentItem>> {
        let mut statement = self.conn.prepare(
            "SELECT
                wait_id,
                agent_id,
                workspace_id,
                repo_id,
                worktree_id,
                root,
                branch,
                relative_path,
                action,
                status,
                requested_at,
                reservation_expires_at,
                blocking_agent_id,
                purpose
             FROM wait_queue
             WHERE status IN ('queued', 'reserved')
             ORDER BY rowid ASC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, String>(10)?,
                row.get::<_, Option<String>>(11)?,
                row.get::<_, Option<String>>(12)?,
                row.get::<_, String>(13)?,
            ))
        })?;
        let rows = rows.collect::<Result<Vec<_>, _>>()?;
        let mut items = Vec::new();

        for (
            wait_id,
            agent_id,
            workspace_id,
            repo_id,
            worktree_id,
            root,
            _branch,
            relative_path,
            action,
            status,
            requested_at,
            reservation_expires_at,
            blocking_agent_id,
            purpose,
        ) in rows
        {
            if workspace_filter.is_some_and(|filter| workspace_id != filter) {
                continue;
            }
            let current_agent_scope = identity_filter
                .exclude_agent_id
                .is_some_and(|excluded| agent_id == excluded);
            if !identity_filter_matches(
                identity_filter,
                repo_id.as_deref(),
                worktree_id.as_deref(),
                root.as_deref(),
            ) {
                continue;
            }
            if !resource_matches_filter(&relative_path, resource_filter) {
                continue;
            }
            let (kind, severity, summary, next_action) = if status == "reserved" {
                (
                    CurrentItemKind::ClaimableReservation,
                    CurrentSeverity::Info,
                    format!(
                        "Agent {agent_id} has a claimable reservation for {action} on {relative_path}."
                    ),
                    format!(
                        "Reread {relative_path}, then call state.reservation.claim with wait_id {wait_id}."
                    ),
                )
            } else {
                (
                    CurrentItemKind::WaitQueue,
                    CurrentSeverity::Warn,
                    format!("Agent {agent_id} is queued for {action} on {relative_path}."),
                    format!(
                        "Wait for the active blocker to release before assuming {relative_path} is writable."
                    ),
                )
            };
            let evidence_kind = if status == "reserved" {
                CurrentEvidenceKind::Reservation
            } else {
                CurrentEvidenceKind::WaitQueue
            };
            let evidence = blocking_agent_id.map(|blocking_agent_id| {
                format!("Blocked by agent {blocking_agent_id}; wait_id {wait_id}.")
            });
            let mut item = CurrentItem::new(
                kind,
                severity,
                CurrentFreshness::Live,
                relative_path.clone(),
                purpose,
                summary,
            )
            .with_next_action(next_action)
            .with_agent(agent_id)
            .with_workspace(workspace_id)
            .with_source_ref("ReservationRequested")
            .with_evidence_kind(evidence_kind)
            .with_observed_at(requested_at)
            .with_expires_at(reservation_expires_at);
            if let Some(evidence) = evidence {
                item = item.with_evidence(evidence);
            }
            if current_agent_scope {
                item = item.with_source_ref(AGENT_CONTEXT_SCOPE_SOURCE_REF);
            }
            items.push(item);
        }

        Ok(items)
    }

    fn agent_active_reservation_matches_identity(
        &self,
        agent_id: &str,
        workspace_id: &str,
        identity_filter: CurrentStateIdentityFilter<'_>,
    ) -> StoreResult<bool> {
        let row = self
            .conn
            .query_row(
                "SELECT e.repo_id, e.worktree_id, e.root
                 FROM reservations i
                 LEFT JOIN events e ON e.event_id = i.reservation_id
                 WHERE i.agent_id = ?1 AND i.workspace_id = ?2 AND i.status = 'active'
                 ORDER BY i.declared_at DESC
                 LIMIT 1",
                params![agent_id, workspace_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()?;

        Ok(row
            .map(|(repo_id, worktree_id, root)| {
                identity_filter_matches(
                    identity_filter,
                    repo_id.as_deref(),
                    worktree_id.as_deref(),
                    root.as_deref(),
                )
            })
            .unwrap_or(false))
    }

    pub fn recent_events(&self, limit: u64) -> StoreResult<Vec<EventRecord>> {
        let mut statement = self.conn.prepare(
            "SELECT
                event_id,
                event_type,
                agent_id,
                workspace_id,
                repo_id,
                worktree_id,
                root,
                branch,
                payload_json,
                created_at
             FROM events
             ORDER BY rowid DESC
             LIMIT ?1",
        )?;
        let rows = statement.query_map([limit], |row| {
            let payload_json: String = row.get(8)?;
            let payload = serde_json::from_str(&payload_json).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    8,
                    rusqlite::types::Type::Text,
                    Box::new(err),
                )
            })?;
            Ok(EventRecord {
                event_id: row.get(0)?,
                event_type: row.get(1)?,
                agent_id: row.get(2)?,
                workspace_id: row.get(3)?,
                repo_id: row.get(4)?,
                worktree_id: row.get(5)?,
                root: row.get(6)?,
                branch: row.get(7)?,
                payload,
                created_at: row.get(9)?,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn append_outbox(&self, entry: OutboxEntry) -> StoreResult<()> {
        let payload = serde_json::to_string(&entry.payload)
            .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))?;
        self.conn.execute(
            "INSERT OR IGNORE INTO outbox (
                outbox_id,
                agent_id,
                workspace_id,
                sequence,
                event_type,
                payload_json,
                sync_status
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                entry.outbox_id,
                entry.agent_id,
                entry.workspace_id,
                entry.sequence,
                entry.event_type,
                payload,
                entry.sync_status.as_str(),
            ],
        )?;

        Ok(())
    }

    pub fn outbox_entry(&self, outbox_id: impl AsRef<str>) -> StoreResult<Option<OutboxRecord>> {
        self.conn
            .query_row(
                "SELECT
                    outbox_id,
                    agent_id,
                    workspace_id,
                    sequence,
                    event_type,
                    payload_json,
                    sync_status
                 FROM outbox
                 WHERE outbox_id = ?1",
                [outbox_id.as_ref()],
                |row| {
                    let payload_json: String = row.get(5)?;
                    let payload = serde_json::from_str(&payload_json).map_err(|err| {
                        rusqlite::Error::FromSqlConversionFailure(
                            5,
                            rusqlite::types::Type::Text,
                            Box::new(err),
                        )
                    })?;
                    Ok(OutboxRecord {
                        outbox_id: row.get(0)?,
                        agent_id: row.get(1)?,
                        workspace_id: row.get(2)?,
                        sequence: row.get(3)?,
                        event_type: row.get(4)?,
                        payload,
                        sync_status: SyncStatus::from_str(row.get::<_, String>(6)?.as_str()),
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn outbox_count(&self) -> StoreResult<u64> {
        self.conn
            .query_row("SELECT COUNT(*) FROM outbox", [], |row| {
                row.get::<_, u64>(0)
            })
            .map_err(StoreError::from)
    }

    pub fn append_reconciliation_ack(&self, agent_id: impl AsRef<str>) -> StoreResult<()> {
        self.conn.execute(
            "INSERT INTO reconciliations (
                reconciliation_id,
                agent_id,
                created_at
            ) VALUES (?1, ?2, ?3)",
            params![
                Uuid::new_v4().to_string(),
                agent_id.as_ref(),
                now_timestamp(),
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

    pub fn acquire_claim(
        &self,
        agent_id: impl AsRef<str>,
        workspace_id: impl AsRef<str>,
        relative_path: impl AsRef<str>,
    ) -> StoreResult<()> {
        self.acquire_claim_with_observation(agent_id, workspace_id, relative_path, None)
    }

    pub fn acquire_claim_with_observation(
        &self,
        agent_id: impl AsRef<str>,
        workspace_id: impl AsRef<str>,
        relative_path: impl AsRef<str>,
        observation: Option<ClaimObservation>,
    ) -> StoreResult<()> {
        let agent_id = agent_id.as_ref().to_string();
        let workspace_id = workspace_id.as_ref().to_string();
        let relative_path = relative_path.as_ref().to_string();
        self.store_transaction(move |store| {
            store.acquire_claim_with_observation_inner(
                None,
                &agent_id,
                &workspace_id,
                &relative_path,
                observation,
            )
        })
    }

    pub fn acquire_claim_for_reservation(
        &self,
        reservation_id: impl AsRef<str>,
        agent_id: impl AsRef<str>,
        workspace_id: impl AsRef<str>,
        relative_path: impl AsRef<str>,
    ) -> StoreResult<()> {
        self.acquire_claim_for_reservation_with_observation_and_event(
            reservation_id,
            agent_id,
            workspace_id,
            relative_path,
            None,
        )
    }

    pub fn acquire_claim_for_reservation_with_observation_and_event(
        &self,
        reservation_id: impl AsRef<str>,
        agent_id: impl AsRef<str>,
        workspace_id: impl AsRef<str>,
        relative_path: impl AsRef<str>,
        observation: Option<ClaimObservation>,
    ) -> StoreResult<()> {
        let reservation_id = reservation_id.as_ref().to_string();
        let agent_id = agent_id.as_ref().to_string();
        let workspace_id = workspace_id.as_ref().to_string();
        let relative_path = relative_path.as_ref().to_string();
        self.store_transaction(move |store| {
            store.acquire_claim_with_observation_and_event_inner(
                Some(&reservation_id),
                &agent_id,
                &workspace_id,
                &relative_path,
                observation,
            )
        })
    }

    pub fn acquire_claim_with_observation_and_event(
        &self,
        agent_id: impl AsRef<str>,
        workspace_id: impl AsRef<str>,
        relative_path: impl AsRef<str>,
        observation: Option<ClaimObservation>,
    ) -> StoreResult<()> {
        let agent_id = agent_id.as_ref().to_string();
        let workspace_id = workspace_id.as_ref().to_string();
        let relative_path = relative_path.as_ref().to_string();
        self.store_transaction(move |store| {
            store.acquire_claim_with_observation_and_event_inner(
                None,
                &agent_id,
                &workspace_id,
                &relative_path,
                observation,
            )
        })
    }

    pub fn acquire_claims_with_observations_and_events(
        &self,
        agent_id: impl AsRef<str>,
        workspace_id: impl AsRef<str>,
        claims: Vec<(String, Option<ClaimObservation>)>,
    ) -> StoreResult<ClaimBatchAcquireResult> {
        let agent_id = agent_id.as_ref().to_string();
        let workspace_id = workspace_id.as_ref().to_string();
        self.store_transaction(move |store| {
            store.acquire_claims_with_observations_and_events_inner(
                None,
                &agent_id,
                &workspace_id,
                claims,
            )
        })
    }

    pub fn acquire_claims_for_reservation_with_observations_and_events(
        &self,
        reservation_id: impl AsRef<str>,
        agent_id: impl AsRef<str>,
        workspace_id: impl AsRef<str>,
        claims: Vec<(String, Option<ClaimObservation>)>,
    ) -> StoreResult<ClaimBatchAcquireResult> {
        let reservation_id = reservation_id.as_ref().to_string();
        let agent_id = agent_id.as_ref().to_string();
        let workspace_id = workspace_id.as_ref().to_string();
        self.store_transaction(move |store| {
            store.acquire_claims_with_observations_and_events_inner(
                Some(&reservation_id),
                &agent_id,
                &workspace_id,
                claims,
            )
        })
    }

    fn acquire_claims_with_observations_and_events_inner(
        &self,
        reservation_id: Option<&str>,
        agent_id: &str,
        workspace_id: &str,
        claims: Vec<(String, Option<ClaimObservation>)>,
    ) -> StoreResult<ClaimBatchAcquireResult> {
        let mut result = ClaimBatchAcquireResult {
            acquired: 0,
            already_held: 0,
        };

        for (relative_path, observation) in claims {
            match self.acquire_claim_with_observation_and_event_inner(
                reservation_id,
                agent_id,
                workspace_id,
                &relative_path,
                observation,
            ) {
                Ok(()) => result.acquired += 1,
                Err(StoreError::ClaimAlreadyHeld) => result.already_held += 1,
                Err(error) => return Err(error),
            }
        }

        Ok(result)
    }

    fn acquire_claim_with_observation_and_event_inner(
        &self,
        reservation_id: Option<&str>,
        agent_id: &str,
        workspace_id: &str,
        relative_path: &str,
        observation: Option<ClaimObservation>,
    ) -> StoreResult<()> {
        self.acquire_claim_with_observation_inner(
            reservation_id,
            agent_id,
            workspace_id,
            relative_path,
            observation,
        )?;
        self.append_inner(&Event::claim_acquired(
            agent_id.to_string(),
            workspace_id.to_string(),
            relative_path.to_string(),
        ))
    }

    fn acquire_claim_with_observation_inner(
        &self,
        reservation_id: Option<&str>,
        agent_id: &str,
        workspace_id: &str,
        relative_path: &str,
        observation: Option<ClaimObservation>,
    ) -> StoreResult<()> {
        self.expire_stale()?;
        let requested_relative_path = relative_path;
        let lease_is_directory = requested_relative_path.ends_with("/");
        let lease_action = if lease_is_directory {
            "write_directory"
        } else {
            "write_file"
        };
        let relative_path = normalize_relative_path(requested_relative_path);
        if direct_tmp_lease_path(&relative_path) {
            return Err(StoreError::InvalidClaimPath(relative_path));
        }
        if self.active_exact_lease_for_agent(
            reservation_id,
            agent_id,
            workspace_id,
            &relative_path,
            lease_action,
        )? {
            return Err(StoreError::ClaimAlreadyHeld);
        }
        let Some((claim_reservation_id, purpose)) = self.active_reservation_for_lease(
            agent_id,
            workspace_id,
            &relative_path,
            lease_is_directory,
            reservation_id,
        )?
        else {
            return Err(StoreError::MissingReservation);
        };
        let purpose = required_purpose(&purpose)?;
        if self.active_claim_conflicts_for_acquire(
            agent_id,
            workspace_id,
            &relative_path,
            lease_action,
            lease_is_directory,
        )? {
            return Err(StoreError::ClaimConflict);
        }
        let now = now_timestamp();
        let expires_at = timestamp_after(&now, CLAIM_TTL_SECONDS)?;
        let observed_exists = observation.as_ref().map(|observation| observation.exists);
        let observed_content_hash = observation
            .as_ref()
            .and_then(|observation| observation.content_hash.as_deref());
        self.conn.execute(
            "INSERT INTO claims (
                claim_id,
                reservation_id,
                agent_id,
                workspace_id,
                repo_id,
                relative_path,
                absolute_path,
                purpose,
                action,
                status,
                expires_at,
                observed_exists,
                observed_content_hash
            ) VALUES (?1, ?2, ?3, ?4, NULL, ?5, NULL, ?6, ?7, 'active', ?8, ?9, ?10)",
            params![
                Uuid::new_v4().to_string(),
                claim_reservation_id,
                agent_id,
                workspace_id,
                relative_path,
                purpose,
                lease_action,
                expires_at,
                observed_exists,
                observed_content_hash,
            ],
        )?;

        Ok(())
    }

    fn active_exact_lease_for_agent(
        &self,
        reservation_id: Option<&str>,
        agent_id: &str,
        workspace_id: &str,
        relative_path: &str,
        lease_action: &str,
    ) -> StoreResult<bool> {
        self.conn
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM claims
                    WHERE agent_id = ?1
                       AND workspace_id = ?2
                       AND relative_path = ?3
                       AND action = ?4
                       AND status = 'active'
                       AND (?5 IS NULL OR reservation_id = ?5)
                )",
                params![
                    agent_id,
                    workspace_id,
                    relative_path,
                    lease_action,
                    reservation_id
                ],
                |row| row.get::<_, bool>(0),
            )
            .map_err(StoreError::from)
    }

    fn active_claim_conflicts_for_acquire(
        &self,
        agent_id: &str,
        workspace_id: &str,
        relative_path: &str,
        lease_action: &str,
        lease_is_directory: bool,
    ) -> StoreResult<bool> {
        if lease_is_directory {
            let directory_prefix = format!("{relative_path}/");
            return self
                .conn
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM claims
                        WHERE workspace_id = ?1
                           AND status = 'active'
                           AND (agent_id != ?5 OR (agent_id = ?5 AND action = ?6 AND relative_path = ?2))
                           AND (
                               (action = 'write_directory' AND relative_path = ?2)
                               OR substr(relative_path, 1, ?3) = ?4
                               OR (action = 'write_directory'
                                  AND substr(?2, 1, length(relative_path) + 1) = relative_path || '/')
                           )
                    ) OR EXISTS(
                        SELECT 1 FROM wait_queue
                        WHERE workspace_id = ?1
                           AND status = 'reserved'
                           AND (
                               (action = 'write_directory' AND relative_path = ?2)
                               OR substr(relative_path, 1, ?3) = ?4
                               OR (action = 'write_directory'
                                  AND substr(?2, 1, length(relative_path) + 1) = relative_path || '/')
                           )
                    )",
                    params![
                        workspace_id,
                        relative_path,
                        directory_prefix.len() as i64,
                        directory_prefix,
                        agent_id,
                        lease_action,
                    ],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(StoreError::from);
        }

        self.conn
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM claims
                    WHERE workspace_id = ?1
                       AND status = 'active'
                       AND (agent_id != ?3 OR (agent_id = ?3 AND action = ?4 AND relative_path = ?2))
                       AND (
                           (action = 'write_file' AND relative_path = ?2)
                           OR (action = 'write_directory'
                              AND substr(?2, 1, length(relative_path) + 1) = relative_path || '/')
                       )
                ) OR EXISTS(
                    SELECT 1 FROM wait_queue
                    WHERE workspace_id = ?1
                       AND status = 'reserved'
                       AND (
                           (action = 'write_file' AND relative_path = ?2)
                           OR (action = 'write_directory'
                              AND substr(?2, 1, length(relative_path) + 1) = relative_path || '/')
                       )
                )",
                params![workspace_id, relative_path, agent_id, lease_action],
                |row| row.get::<_, bool>(0),
            )
            .map_err(StoreError::from)
    }

    pub fn release_claim(
        &self,
        agent_id: impl AsRef<str>,
        workspace_id: impl AsRef<str>,
        relative_path: impl AsRef<str>,
    ) -> StoreResult<()> {
        self.expire_stale()?;
        let requested_relative_path = relative_path.as_ref();
        let lease_action = if requested_relative_path.ends_with("/") {
            "write_directory"
        } else {
            "write_file"
        };
        let relative_path = normalize_relative_path(requested_relative_path);
        let workspace_id = workspace_id.as_ref().to_string();
        self.conn.execute_batch("BEGIN IMMEDIATE")?;

        let result = (|| -> StoreResult<()> {
            let released = self.conn.execute(
                "UPDATE claims
                 SET status = 'released'
                 WHERE agent_id = ?1 AND workspace_id = ?2 AND relative_path = ?3 AND action = ?4 AND status = 'active'",
                params![agent_id.as_ref(), workspace_id, relative_path, lease_action],
            )?;
            if released == 0 {
                let owner_exists = self.conn.query_row(
                    "SELECT EXISTS(
                            SELECT 1 FROM claims
                            WHERE agent_id != ?1
                              AND workspace_id = ?2
                              AND relative_path = ?3
                              AND action = ?4
                              AND status = 'active'
                        )",
                    params![agent_id.as_ref(), workspace_id, relative_path, lease_action],
                    |row| row.get::<_, bool>(0),
                )?;
                if owner_exists {
                    return Err(StoreError::ClaimOwnerMismatch);
                }
                return Err(StoreError::ClaimNotFound);
            }
            self.promote_waiters_after_lease_release(&workspace_id, &relative_path)?;
            self.append_inner(&Event::claim_released(
                agent_id.as_ref().to_string(),
                workspace_id.clone(),
                relative_path.clone(),
                lease_action,
            ))?;
            self.conn.execute_batch("COMMIT")?;

            Ok(())
        })();

        if result.is_err() {
            let _ = self.conn.execute_batch("ROLLBACK");
        }

        result
    }

    pub fn release_session_claims(
        &self,
        agent_id: impl AsRef<str>,
        workspace_id: impl AsRef<str>,
    ) -> StoreResult<u64> {
        let agent_id = agent_id.as_ref().to_string();
        let workspace_id = workspace_id.as_ref().to_string();
        if !self.conn.is_autocommit() {
            return self.release_session_claims_inner(&agent_id, &workspace_id);
        }

        self.conn.execute_batch("BEGIN IMMEDIATE")?;

        let result = (|| -> StoreResult<u64> {
            let released = self.release_session_claims_inner(&agent_id, &workspace_id)?;
            self.conn.execute_batch("COMMIT")?;
            Ok(released)
        })();

        if result.is_err() {
            let _ = self.conn.execute_batch("ROLLBACK");
        }

        result
    }

    fn release_session_claims_inner(&self, agent_id: &str, workspace_id: &str) -> StoreResult<u64> {
        self.expire_stale()?;
        let paths = {
            let mut statement = self.conn.prepare(
                "SELECT relative_path FROM claims
                 WHERE agent_id = ?1 AND workspace_id = ?2 AND status = 'active'",
            )?;
            let paths = statement.query_map(params![agent_id, workspace_id], |row| {
                row.get::<_, String>(0)
            })?;
            paths.collect::<Result<Vec<_>, _>>()?
        };
        let released = paths.len() as u64;

        self.conn.execute(
            "UPDATE claims
             SET status = 'released'
             WHERE agent_id = ?1 AND workspace_id = ?2 AND status = 'active'",
            params![agent_id, workspace_id],
        )?;

        for path in paths {
            self.promote_waiters_after_lease_release(workspace_id, &path)?;
        }

        Ok(released)
    }

    pub fn refresh_exact_file_claim_observation(
        &self,
        agent_id: impl AsRef<str>,
        workspace_id: impl AsRef<str>,
        relative_path: impl AsRef<str>,
        observation: ClaimObservation,
    ) -> StoreResult<()> {
        self.expire_stale()?;
        let agent_id = agent_id.as_ref().to_string();
        let workspace_id = workspace_id.as_ref().to_string();
        let relative_path = normalize_relative_path(relative_path.as_ref());
        if !self.conn.is_autocommit() {
            return self.refresh_exact_file_claim_observation_inner(
                &agent_id,
                &workspace_id,
                &relative_path,
                &observation,
            );
        }

        self.conn.execute_batch("BEGIN IMMEDIATE")?;

        let result = (|| -> StoreResult<()> {
            self.refresh_exact_file_claim_observation_inner(
                &agent_id,
                &workspace_id,
                &relative_path,
                &observation,
            )?;
            self.conn.execute_batch("COMMIT")?;
            Ok(())
        })();

        if result.is_err() {
            let _ = self.conn.execute_batch("ROLLBACK");
        }

        result
    }

    fn refresh_exact_file_claim_observation_inner(
        &self,
        agent_id: &str,
        workspace_id: &str,
        relative_path: &str,
        observation: &ClaimObservation,
    ) -> StoreResult<()> {
        let updated = self.conn.execute(
            "UPDATE claims
             SET observed_exists = ?1, observed_content_hash = ?2
             WHERE agent_id = ?3
               AND workspace_id = ?4
               AND relative_path = ?5
               AND action = 'write_file'
               AND status = 'active'",
            params![
                observation.exists,
                observation.content_hash.as_deref(),
                agent_id,
                workspace_id,
                relative_path,
            ],
        )?;
        if updated > 0 {
            return Ok(());
        }

        let owner_exists = self.conn.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM claims
                WHERE agent_id != ?1
                  AND workspace_id = ?2
                  AND relative_path = ?3
                  AND action = 'write_file'
                  AND status = 'active'
            )",
            params![agent_id, workspace_id, relative_path],
            |row| row.get::<_, bool>(0),
        )?;
        if owner_exists {
            return Err(StoreError::ClaimOwnerMismatch);
        }

        Err(StoreError::ClaimNotFound)
    }

    pub fn complete_session_reservations(
        &self,
        agent_id: impl AsRef<str>,
        workspace_id: impl AsRef<str>,
    ) -> StoreResult<u64> {
        self.complete_session_reservations_inner(agent_id.as_ref(), workspace_id.as_ref())
    }

    fn complete_session_reservations_inner(
        &self,
        agent_id: &str,
        workspace_id: &str,
    ) -> StoreResult<u64> {
        self.expire_stale()?;
        let completed = self.conn.execute(
            "UPDATE reservations
             SET status = 'completed'
             WHERE agent_id = ?1 AND workspace_id = ?2 AND status = 'active'",
            params![agent_id, workspace_id],
        )?;
        Ok(completed as u64)
    }

    pub fn lease_count(&self) -> StoreResult<u64> {
        self.conn
            .query_row("SELECT COUNT(*) FROM claims", [], |row| {
                row.get::<_, u64>(0)
            })
            .map_err(StoreError::from)
    }

    pub fn active_claim_owner(
        &self,
        workspace_id: impl AsRef<str>,
        relative_path: impl AsRef<str>,
    ) -> StoreResult<Option<String>> {
        self.expire_stale()?;
        let relative_path = normalize_relative_path(relative_path.as_ref());
        self.conn
            .query_row(
                "SELECT agent_id FROM claims
                 WHERE workspace_id = ?1 AND relative_path = ?2 AND status = 'active'
                 ORDER BY rowid DESC
                 LIMIT 1",
                params![workspace_id.as_ref(), relative_path],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn active_claim_conflict_owner_for_directory(
        &self,
        workspace_id: impl AsRef<str>,
        directory_path: impl AsRef<str>,
        agent_id: impl AsRef<str>,
    ) -> StoreResult<Option<String>> {
        self.expire_stale()?;
        let directory_path = normalize_relative_path(directory_path.as_ref());
        let directory_prefix = format!("{directory_path}/");
        self.conn
            .query_row(
                "SELECT agent_id FROM claims
                 WHERE workspace_id = ?1
                    AND agent_id != ?2
                    AND status = 'active'
                    AND (
                        (action = 'write_directory' AND relative_path = ?3)
                        OR substr(relative_path, 1, ?4) = ?5
                        OR (action = 'write_directory'
                           AND substr(?3, 1, length(relative_path) + 1) = relative_path || '/')
                    )
                 ORDER BY rowid DESC
                 LIMIT 1",
                params![
                    workspace_id.as_ref(),
                    agent_id.as_ref(),
                    directory_path,
                    directory_prefix.len() as i64,
                    directory_prefix,
                ],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn active_claim_covers_directory_by_agent(
        &self,
        workspace_id: impl AsRef<str>,
        directory_path: impl AsRef<str>,
        agent_id: impl AsRef<str>,
    ) -> StoreResult<bool> {
        self.expire_stale()?;
        let directory_path = normalize_relative_path(directory_path.as_ref());
        self.conn
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM claims
                    WHERE workspace_id = ?1
                       AND agent_id = ?2
                       AND status = 'active'
                       AND action = 'write_directory'
                       AND (
                           relative_path = ?3
                           OR substr(?3, 1, length(relative_path) + 1) = relative_path || '/'
                       )
                )",
                params![workspace_id.as_ref(), agent_id.as_ref(), directory_path],
                |row| row.get::<_, bool>(0),
            )
            .map_err(StoreError::from)
    }

    pub fn active_claim_covers_directory_by_reservation(
        &self,
        workspace_id: impl AsRef<str>,
        directory_path: impl AsRef<str>,
        reservation_id: impl AsRef<str>,
    ) -> StoreResult<bool> {
        self.expire_stale()?;
        let directory_path = normalize_relative_path(directory_path.as_ref());
        self.conn
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM claims
                    WHERE workspace_id = ?1
                       AND reservation_id = ?2
                       AND status = 'active'
                       AND action = 'write_directory'
                       AND (
                           relative_path = ?3
                           OR substr(?3, 1, length(relative_path) + 1) = relative_path || '/'
                       )
                )",
                params![
                    workspace_id.as_ref(),
                    reservation_id.as_ref(),
                    directory_path
                ],
                |row| row.get::<_, bool>(0),
            )
            .map_err(StoreError::from)
    }

    pub fn active_claim_conflict_owner_for_path(
        &self,
        workspace_id: impl AsRef<str>,
        relative_path: impl AsRef<str>,
        agent_id: impl AsRef<str>,
    ) -> StoreResult<Option<String>> {
        self.expire_stale()?;
        let relative_path = normalize_relative_path(relative_path.as_ref());
        self.conn
            .query_row(
                "SELECT agent_id FROM claims
                 WHERE workspace_id = ?1
                    AND agent_id != ?2
                    AND status = 'active'
                    AND (
                        (action = 'write_file' AND relative_path = ?3)
                        OR (action = 'write_directory'
                           AND substr(?3, 1, length(relative_path) + 1) = relative_path || '/')
                    )
                 ORDER BY rowid DESC
                 LIMIT 1",
                params![workspace_id.as_ref(), agent_id.as_ref(), relative_path],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn active_claim_covers_path_by_agent(
        &self,
        workspace_id: impl AsRef<str>,
        relative_path: impl AsRef<str>,
        agent_id: impl AsRef<str>,
    ) -> StoreResult<bool> {
        self.expire_stale()?;
        let relative_path = normalize_relative_path(relative_path.as_ref());
        self.conn
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM claims
                    WHERE workspace_id = ?1
                       AND agent_id = ?2
                       AND status = 'active'
                       AND (
                           (action = 'write_file' AND relative_path = ?3)
                           OR (action = 'write_directory'
                           AND substr(?3, 1, length(relative_path) + 1) = relative_path || '/')
                       )
                )",
                params![workspace_id.as_ref(), agent_id.as_ref(), relative_path],
                |row| row.get::<_, bool>(0),
            )
            .map_err(StoreError::from)
    }

    pub fn active_claim_covers_path_by_reservation(
        &self,
        workspace_id: impl AsRef<str>,
        relative_path: impl AsRef<str>,
        reservation_id: impl AsRef<str>,
    ) -> StoreResult<bool> {
        self.expire_stale()?;
        let relative_path = normalize_relative_path(relative_path.as_ref());
        self.conn
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM claims
                    WHERE workspace_id = ?1
                       AND reservation_id = ?2
                       AND status = 'active'
                       AND (
                           (action = 'write_file' AND relative_path = ?3)
                           OR (action = 'write_directory'
                           AND substr(?3, 1, length(relative_path) + 1) = relative_path || '/')
                       )
                )",
                params![
                    workspace_id.as_ref(),
                    reservation_id.as_ref(),
                    relative_path
                ],
                |row| row.get::<_, bool>(0),
            )
            .map_err(StoreError::from)
    }

    pub fn active_exact_file_lease_by_reservation(
        &self,
        workspace_id: impl AsRef<str>,
        relative_path: impl AsRef<str>,
        reservation_id: impl AsRef<str>,
    ) -> StoreResult<bool> {
        self.expire_stale()?;
        let relative_path = normalize_relative_path(relative_path.as_ref());
        self.conn
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM claims
                    WHERE workspace_id = ?1
                       AND reservation_id = ?2
                       AND status = 'active'
                       AND action = 'write_file'
                       AND relative_path = ?3
                )",
                params![
                    workspace_id.as_ref(),
                    reservation_id.as_ref(),
                    relative_path
                ],
                |row| row.get::<_, bool>(0),
            )
            .map_err(StoreError::from)
    }

    pub fn active_exact_file_lease_by_agent(
        &self,
        workspace_id: impl AsRef<str>,
        relative_path: impl AsRef<str>,
        agent_id: impl AsRef<str>,
    ) -> StoreResult<bool> {
        self.expire_stale()?;
        let relative_path = normalize_relative_path(relative_path.as_ref());
        self.conn
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM claims
                    WHERE workspace_id = ?1
                       AND agent_id = ?2
                       AND status = 'active'
                       AND action = 'write_file'
                       AND relative_path = ?3
                )",
                params![workspace_id.as_ref(), agent_id.as_ref(), relative_path],
                |row| row.get::<_, bool>(0),
            )
            .map_err(StoreError::from)
    }

    pub fn active_exact_file_claim_observation_by_agent(
        &self,
        workspace_id: impl AsRef<str>,
        relative_path: impl AsRef<str>,
        agent_id: impl AsRef<str>,
    ) -> StoreResult<Option<ClaimObservation>> {
        self.expire_stale()?;
        let relative_path = normalize_relative_path(relative_path.as_ref());
        self.conn
            .query_row(
                "SELECT observed_exists, observed_content_hash
                 FROM claims
                 WHERE workspace_id = ?1
                    AND agent_id = ?2
                    AND status = 'active'
                    AND action = 'write_file'
                    AND relative_path = ?3
                 ORDER BY rowid DESC
                 LIMIT 1",
                params![workspace_id.as_ref(), agent_id.as_ref(), relative_path],
                |row| {
                    let observed_exists = row.get::<_, Option<bool>>(0)?;
                    let observed_content_hash = row.get::<_, Option<String>>(1)?;
                    Ok(observed_exists.map(|exists| ClaimObservation {
                        exists,
                        content_hash: observed_content_hash,
                    }))
                },
            )
            .optional()
            .map(|row| row.flatten())
            .map_err(StoreError::from)
    }

    pub fn active_exact_file_claim_observation_by_reservation(
        &self,
        workspace_id: impl AsRef<str>,
        relative_path: impl AsRef<str>,
        reservation_id: impl AsRef<str>,
    ) -> StoreResult<Option<ClaimObservation>> {
        self.expire_stale()?;
        let relative_path = normalize_relative_path(relative_path.as_ref());
        self.conn
            .query_row(
                "SELECT observed_exists, observed_content_hash
                 FROM claims
                 WHERE workspace_id = ?1
                    AND reservation_id = ?2
                    AND status = 'active'
                    AND action = 'write_file'
                    AND relative_path = ?3
                 ORDER BY rowid DESC
                 LIMIT 1",
                params![
                    workspace_id.as_ref(),
                    reservation_id.as_ref(),
                    relative_path
                ],
                |row| {
                    let observed_exists = row.get::<_, Option<bool>>(0)?;
                    let observed_content_hash = row.get::<_, Option<String>>(1)?;
                    Ok(observed_exists.map(|exists| ClaimObservation {
                        exists,
                        content_hash: observed_content_hash,
                    }))
                },
            )
            .optional()
            .map(|row| row.flatten())
            .map_err(StoreError::from)
    }

    pub fn enqueue_waiter(
        &self,
        agent_id: impl AsRef<str>,
        workspace_id: impl AsRef<str>,
        relative_path: impl AsRef<str>,
        action: impl AsRef<str>,
        purpose: impl AsRef<str>,
        blocking_agent_id: Option<&str>,
    ) -> StoreResult<WaitRecord> {
        self.enqueue_waiter_with_identity(
            agent_id,
            workspace_id,
            relative_path,
            action,
            purpose,
            blocking_agent_id,
            WorkspaceIdentity::default(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn enqueue_waiter_with_identity(
        &self,
        agent_id: impl AsRef<str>,
        workspace_id: impl AsRef<str>,
        relative_path: impl AsRef<str>,
        action: impl AsRef<str>,
        purpose: impl AsRef<str>,
        blocking_agent_id: Option<&str>,
        identity: WorkspaceIdentity<'_>,
    ) -> StoreResult<WaitRecord> {
        self.expire_stale()?;
        let relative_path = normalize_relative_path(relative_path.as_ref());
        if relative_path.is_empty() {
            return Err(StoreError::MissingScope);
        }
        let purpose = required_purpose(purpose.as_ref())?;
        let existing = self
            .conn
            .query_row(
                "SELECT wait_id FROM wait_queue
                 WHERE agent_id = ?1
                    AND workspace_id = ?2
                    AND relative_path = ?3
                    AND action = ?4
                    AND status IN ('queued', 'reserved')
                 ORDER BY rowid DESC
                 LIMIT 1",
                params![
                    agent_id.as_ref(),
                    workspace_id.as_ref(),
                    relative_path,
                    action.as_ref(),
                ],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if let Some(wait_id) = existing {
            self.update_waiter_identity_if_missing(&wait_id, identity)?;
            return self
                .waiter(&wait_id)
                .map(|waiter| waiter.expect("existing waiter should load"));
        }

        let wait_id = Uuid::new_v4().to_string();
        self.conn.execute(
            "INSERT INTO wait_queue (
                wait_id,
                agent_id,
                workspace_id,
                repo_id,
                worktree_id,
                root,
                branch,
                relative_path,
                action,
                status,
                requested_at,
                reservation_expires_at,
                blocking_agent_id,
                purpose
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'queued', ?10, NULL, ?11, ?12)",
            params![
                wait_id,
                agent_id.as_ref(),
                workspace_id.as_ref(),
                identity.repo_id,
                identity.worktree_id,
                identity.root,
                identity.branch,
                relative_path,
                action.as_ref(),
                now_timestamp(),
                blocking_agent_id,
                purpose,
            ],
        )?;

        self.waiter(&wait_id)
            .map(|waiter| waiter.expect("inserted waiter should load"))
    }

    pub fn enqueue_reservation_request(
        &self,
        input: ReservationRequestInput<'_>,
    ) -> StoreResult<WaitRecord> {
        self.enqueue_reservation_request_with_identity(input, WorkspaceIdentity::default())
    }

    pub fn enqueue_reservation_request_with_identity(
        &self,
        input: ReservationRequestInput<'_>,
        identity: WorkspaceIdentity<'_>,
    ) -> StoreResult<WaitRecord> {
        self.expire_stale()?;
        let purpose = required_purpose(input.purpose)?;
        if let Some(waiter) = self.waiter_by_request_id(input.request_id)? {
            self.update_waiter_identity_if_missing(&waiter.wait_id, identity)?;
            if waiter.status == "expired" {
                self.conn.execute(
                    "UPDATE wait_queue
                     SET status = 'queued',
                         reservation_expires_at = NULL,
                         blocking_agent_id = ?1
                     WHERE wait_id = ?2 AND status = 'expired'",
                    params![input.blocking_agent_id, waiter.wait_id],
                )?;
            }
            let waiter = self
                .waiter(&waiter.wait_id)?
                .expect("existing waiter should load");
            return Ok(waiter);
        }

        let wait_id = Uuid::new_v4().to_string();
        let relative_path = normalize_relative_path(input.relative_path);
        if relative_path.is_empty() {
            return Err(StoreError::MissingScope);
        }
        self.conn.execute(
            "INSERT INTO wait_queue (
                wait_id,
                request_id,
                agent_id,
                workspace_id,
                repo_id,
                worktree_id,
                root,
                branch,
                relative_path,
                action,
                status,
                requested_at,
                reservation_expires_at,
                blocking_agent_id,
                purpose
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'queued', ?11, NULL, ?12, ?13)",
            params![
                wait_id,
                input.request_id,
                input.agent_id,
                input.workspace_id,
                identity.repo_id,
                identity.worktree_id,
                identity.root,
                identity.branch,
                relative_path,
                input.action,
                now_timestamp(),
                input.blocking_agent_id,
                purpose,
            ],
        )?;

        self.waiter(&wait_id)
            .map(|waiter| waiter.expect("inserted waiter should load"))
    }

    pub fn waiter_by_request_id(
        &self,
        request_id: impl AsRef<str>,
    ) -> StoreResult<Option<WaitRecord>> {
        self.expire_stale()?;
        let wait_id = self
            .conn
            .query_row(
                "SELECT wait_id FROM wait_queue
                 WHERE request_id = ?1
                 ORDER BY rowid DESC
                 LIMIT 1",
                params![request_id.as_ref()],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        match wait_id {
            Some(wait_id) => self.waiter(&wait_id),
            None => Ok(None),
        }
    }

    pub fn backfill_waiter_identity_if_missing(
        &self,
        wait_id: impl AsRef<str>,
        identity: WorkspaceIdentity<'_>,
    ) -> StoreResult<WaitRecord> {
        let wait_id = wait_id.as_ref();
        self.update_waiter_identity_if_missing(wait_id, identity)?;
        self.waiter(wait_id)?
            .ok_or(StoreError::ReservationRequestNotFound)
    }

    pub fn cancel_reservation_request(
        &self,
        request_id: impl AsRef<str>,
        agent_id: impl AsRef<str>,
        workspace_id: impl AsRef<str>,
    ) -> StoreResult<WaitRecord> {
        let request_id = request_id.as_ref().to_string();
        let agent_id = agent_id.as_ref().to_string();
        let workspace_id = workspace_id.as_ref().to_string();
        if !self.conn.is_autocommit() {
            return self.cancel_reservation_request_inner(&request_id, &agent_id, &workspace_id);
        }

        self.conn.execute_batch("BEGIN IMMEDIATE")?;

        let result = (|| -> StoreResult<WaitRecord> {
            let canceled =
                self.cancel_reservation_request_inner(&request_id, &agent_id, &workspace_id)?;
            self.conn.execute_batch("COMMIT")?;
            Ok(canceled)
        })();

        if result.is_err() {
            let _ = self.conn.execute_batch("ROLLBACK");
        }

        result
    }

    fn cancel_reservation_request_inner(
        &self,
        request_id: &str,
        agent_id: &str,
        workspace_id: &str,
    ) -> StoreResult<WaitRecord> {
        self.expire_stale()?;
        let wait_id = self
            .conn
            .query_row(
                "SELECT wait_id FROM wait_queue
                 WHERE request_id = ?1
                 ORDER BY rowid DESC
                 LIMIT 1",
                params![request_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .ok_or(StoreError::ReservationRequestNotFound)?;
        let waiter = self
            .waiter(&wait_id)?
            .ok_or(StoreError::ReservationRequestNotFound)?;
        if waiter.agent_id != agent_id || waiter.workspace_id != workspace_id {
            return Err(StoreError::ReservationRequestOwnerMismatch);
        }
        if waiter.status == "canceled" {
            return Ok(waiter);
        }
        if !matches!(waiter.status.as_str(), "queued" | "reserved") {
            return Err(StoreError::ReservationRequestNotCancelable);
        }

        self.conn.execute(
            "UPDATE wait_queue
             SET status = 'canceled', reservation_expires_at = NULL
             WHERE wait_id = ?1 AND status IN ('queued', 'reserved')",
            params![&waiter.wait_id],
        )?;

        self.promote_next_waiter_after_path_release(&waiter.workspace_id, &waiter.relative_path)?;

        let mut canceled = waiter;
        canceled.status = "canceled".to_string();
        canceled.reservation_expires_at = None;
        Ok(canceled)
    }

    fn cancel_session_waiters_inner(&self, agent_id: &str, workspace_id: &str) -> StoreResult<u64> {
        self.expire_stale()?;
        let waiters = {
            let mut statement = self.conn.prepare(
                "SELECT workspace_id, relative_path
                 FROM wait_queue
                 WHERE agent_id = ?1
                    AND workspace_id = ?2
                    AND status IN ('queued', 'reserved')
                 ORDER BY rowid ASC",
            )?;
            let waiters = statement.query_map(params![agent_id, workspace_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            waiters.collect::<Result<Vec<_>, _>>()?
        };
        let canceled = waiters.len() as u64;
        if canceled == 0 {
            return Ok(0);
        }

        self.conn.execute(
            "UPDATE wait_queue
             SET status = 'canceled', reservation_expires_at = NULL
             WHERE agent_id = ?1
                AND workspace_id = ?2
                AND status IN ('queued', 'reserved')",
            params![agent_id, workspace_id],
        )?;

        for (workspace_id, relative_path) in waiters {
            self.promote_next_waiter_after_path_release(&workspace_id, &relative_path)?;
        }

        Ok(canceled)
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
        let workspace_id = workspace_id.as_ref().to_string();
        let relative_path = normalize_relative_path(relative_path.as_ref());
        if !self.conn.is_autocommit() {
            return self.promote_next_waiter_exact(&workspace_id, &relative_path);
        }

        self.conn.execute_batch("BEGIN IMMEDIATE")?;

        let result = (|| -> StoreResult<Option<WaitRecord>> {
            let promoted = self.promote_next_waiter_exact(&workspace_id, &relative_path)?;
            self.conn.execute_batch("COMMIT")?;
            Ok(promoted)
        })();

        if result.is_err() {
            let _ = self.conn.execute_batch("ROLLBACK");
        }

        result
    }

    fn promote_next_waiter_exact(
        &self,
        workspace_id: &str,
        relative_path: &str,
    ) -> StoreResult<Option<WaitRecord>> {
        let active_reservation = self
            .conn
            .query_row(
                "SELECT wait_id FROM wait_queue
                 WHERE workspace_id = ?1 AND relative_path = ?2 AND status = 'reserved'
                 ORDER BY rowid ASC
                 LIMIT 1",
                params![workspace_id, relative_path],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if let Some(wait_id) = active_reservation {
            return self.waiter(&wait_id);
        }

        let wait_id = self
            .conn
            .query_row(
                "SELECT wait_id FROM wait_queue
                 WHERE workspace_id = ?1 AND relative_path = ?2 AND status = 'queued'
                 ORDER BY rowid ASC
                 LIMIT 1",
                params![workspace_id, relative_path],
                |row| row.get::<_, String>(0),
            )
            .optional()?;

        let Some(wait_id) = wait_id else {
            return Ok(None);
        };

        self.promote_waiter_by_id(&wait_id)
    }

    pub fn promote_next_waiter_for_path(
        &self,
        workspace_id: impl AsRef<str>,
        relative_path: impl AsRef<str>,
    ) -> StoreResult<Option<WaitRecord>> {
        let workspace_id = workspace_id.as_ref().to_string();
        let relative_path = relative_path.as_ref().to_string();
        let now = now_timestamp();
        if !self.conn.is_autocommit() {
            self.expire_stale_at_inner(&now)?;
            return self.promote_next_waiter_after_path_release(&workspace_id, &relative_path);
        }

        self.conn.execute_batch("BEGIN IMMEDIATE")?;

        let result = (|| -> StoreResult<Option<WaitRecord>> {
            self.expire_stale_at_inner(&now)?;
            let promoted =
                self.promote_next_waiter_after_path_release(&workspace_id, &relative_path)?;
            self.conn.execute_batch("COMMIT")?;
            Ok(promoted)
        })();

        if result.is_err() {
            let _ = self.conn.execute_batch("ROLLBACK");
        }

        result
    }

    pub fn active_reservation(
        &self,
        workspace_id: impl AsRef<str>,
        relative_path: impl AsRef<str>,
    ) -> StoreResult<Option<WaitRecord>> {
        self.expire_stale()?;
        let relative_path = normalize_relative_path(relative_path.as_ref());
        self.conn
            .query_row(
                "SELECT
                    wait_id,
                    agent_id,
                    workspace_id,
                    repo_id,
                    worktree_id,
                    root,
                    branch,
                    relative_path,
                    action,
                    status,
                    requested_at,
                    reservation_expires_at,
                    blocking_agent_id,
                    purpose
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

    pub fn active_reservation_conflict_for_directory(
        &self,
        workspace_id: impl AsRef<str>,
        directory_path: impl AsRef<str>,
        agent_id: impl AsRef<str>,
    ) -> StoreResult<Option<WaitRecord>> {
        self.expire_stale()?;
        let directory_path = normalize_relative_path(directory_path.as_ref());
        let directory_prefix = format!("{directory_path}/");
        self.conn
            .query_row(
                "SELECT
                    wait_id,
                    agent_id,
                    workspace_id,
                    repo_id,
                    worktree_id,
                    root,
                    branch,
                    relative_path,
                    action,
                    status,
                    requested_at,
                    reservation_expires_at,
                    blocking_agent_id,
                    purpose
                 FROM wait_queue
                 WHERE workspace_id = ?1
                    AND agent_id != ?2
                    AND status = 'reserved'
                    AND (
                        (action = 'write_directory' AND relative_path = ?3)
                        OR substr(relative_path, 1, ?4) = ?5
                        OR (action = 'write_directory'
                           AND substr(?3, 1, length(relative_path) + 1) = relative_path || '/')
                    )
                 ORDER BY rowid ASC
                 LIMIT 1",
                params![
                    workspace_id.as_ref(),
                    agent_id.as_ref(),
                    directory_path,
                    directory_prefix.len() as i64,
                    directory_prefix,
                ],
                wait_record_from_row,
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn active_reservation_for_directory_by_agent(
        &self,
        workspace_id: impl AsRef<str>,
        directory_path: impl AsRef<str>,
        agent_id: impl AsRef<str>,
    ) -> StoreResult<Option<WaitRecord>> {
        self.expire_stale()?;
        let directory_path = normalize_relative_path(directory_path.as_ref());
        let directory_prefix = format!("{directory_path}/");
        self.conn
            .query_row(
                "SELECT
                    wait_id,
                    agent_id,
                    workspace_id,
                    relative_path,
                    action,
                    status,
                    requested_at,
                    reservation_expires_at,
                    blocking_agent_id,
                    purpose
                 FROM wait_queue
                 WHERE workspace_id = ?1
                    AND agent_id = ?2
                    AND status = 'reserved'
                    AND (
                        (action = 'write_directory' AND relative_path = ?3)
                        OR substr(relative_path, 1, ?4) = ?5
                        OR (action = 'write_directory'
                           AND substr(?3, 1, length(relative_path) + 1) = relative_path || '/')
                    )
                 ORDER BY rowid ASC
                 LIMIT 1",
                params![
                    workspace_id.as_ref(),
                    agent_id.as_ref(),
                    directory_path,
                    directory_prefix.len() as i64,
                    directory_prefix,
                ],
                wait_record_from_row,
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn active_reservation_conflict_for_path(
        &self,
        workspace_id: impl AsRef<str>,
        relative_path: impl AsRef<str>,
        agent_id: impl AsRef<str>,
    ) -> StoreResult<Option<WaitRecord>> {
        self.expire_stale()?;
        let relative_path = normalize_relative_path(relative_path.as_ref());
        self.conn
            .query_row(
                "SELECT
                    wait_id,
                    agent_id,
                    workspace_id,
                    relative_path,
                    action,
                    status,
                    requested_at,
                    reservation_expires_at,
                    blocking_agent_id,
                    purpose
                 FROM wait_queue
                 WHERE workspace_id = ?1
                    AND agent_id != ?2
                    AND status = 'reserved'
                    AND (
                        (action = 'write_file' AND relative_path = ?3)
                        OR (action = 'write_directory'
                           AND substr(?3, 1, length(relative_path) + 1) = relative_path || '/')
                    )
                 ORDER BY rowid ASC
                 LIMIT 1",
                params![workspace_id.as_ref(), agent_id.as_ref(), relative_path],
                wait_record_from_row,
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn active_reservation_for_path_by_agent(
        &self,
        workspace_id: impl AsRef<str>,
        relative_path: impl AsRef<str>,
        agent_id: impl AsRef<str>,
    ) -> StoreResult<Option<WaitRecord>> {
        self.expire_stale()?;
        let relative_path = normalize_relative_path(relative_path.as_ref());
        self.conn
            .query_row(
                "SELECT
                    wait_id,
                    agent_id,
                    workspace_id,
                    relative_path,
                    action,
                    status,
                    requested_at,
                    reservation_expires_at,
                    blocking_agent_id,
                    purpose
                 FROM wait_queue
                 WHERE workspace_id = ?1
                    AND agent_id = ?2
                    AND status = 'reserved'
                    AND (
                        (action = 'write_file' AND relative_path = ?3)
                        OR (action = 'write_directory'
                           AND substr(?3, 1, length(relative_path) + 1) = relative_path || '/')
                    )
                 ORDER BY rowid ASC
                 LIMIT 1",
                params![workspace_id.as_ref(), agent_id.as_ref(), relative_path],
                wait_record_from_row,
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn active_waiter_for_directory_by_agent(
        &self,
        workspace_id: impl AsRef<str>,
        directory_path: impl AsRef<str>,
        agent_id: impl AsRef<str>,
    ) -> StoreResult<Option<WaitRecord>> {
        self.expire_stale()?;
        let directory_path = normalize_relative_path(directory_path.as_ref());
        let directory_prefix = format!("{directory_path}/");
        self.conn
            .query_row(
                "SELECT
                    wait_id,
                    agent_id,
                    workspace_id,
                    repo_id,
                    worktree_id,
                    root,
                    branch,
                    relative_path,
                    action,
                    status,
                    requested_at,
                    reservation_expires_at,
                    blocking_agent_id,
                    purpose
                 FROM wait_queue
                 WHERE workspace_id = ?1
                    AND agent_id = ?2
                    AND status IN ('queued', 'reserved')
                    AND (
                        (action = 'write_directory' AND relative_path = ?3)
                        OR substr(relative_path, 1, ?4) = ?5
                        OR (action = 'write_directory'
                           AND substr(?3, 1, length(relative_path) + 1) = relative_path || '/')
                    )
                 ORDER BY rowid ASC
                 LIMIT 1",
                params![
                    workspace_id.as_ref(),
                    agent_id.as_ref(),
                    directory_path,
                    directory_prefix.len() as i64,
                    directory_prefix,
                ],
                wait_record_from_row,
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn active_waiter_for_path_by_agent(
        &self,
        workspace_id: impl AsRef<str>,
        relative_path: impl AsRef<str>,
        agent_id: impl AsRef<str>,
    ) -> StoreResult<Option<WaitRecord>> {
        self.expire_stale()?;
        let relative_path = normalize_relative_path(relative_path.as_ref());
        self.conn
            .query_row(
                "SELECT
                    wait_id,
                    agent_id,
                    workspace_id,
                    repo_id,
                    worktree_id,
                    root,
                    branch,
                    relative_path,
                    action,
                    status,
                    requested_at,
                    reservation_expires_at,
                    blocking_agent_id,
                    purpose
                 FROM wait_queue
                 WHERE workspace_id = ?1
                    AND agent_id = ?2
                    AND status IN ('queued', 'reserved')
                    AND (
                        (action = 'write_file' AND relative_path = ?3)
                        OR (action = 'write_directory'
                           AND substr(?3, 1, length(relative_path) + 1) = relative_path || '/')
                    )
                 ORDER BY rowid ASC
                 LIMIT 1",
                params![workspace_id.as_ref(), agent_id.as_ref(), relative_path],
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
            .map(|reservation| reservation.agent_id))
    }

    pub fn next_reservation_for_agent(
        &self,
        agent_id: impl AsRef<str>,
        workspace_id: impl AsRef<str>,
    ) -> StoreResult<Option<WaitRecord>> {
        self.expire_stale()?;
        self.conn
            .query_row(
                "SELECT
                    wait_id,
                    agent_id,
                    workspace_id,
                    relative_path,
                    action,
                    status,
                    requested_at,
                    reservation_expires_at,
                    blocking_agent_id,
                    purpose
                 FROM wait_queue
                 WHERE agent_id = ?1 AND workspace_id = ?2 AND status = 'reserved'
                 ORDER BY rowid ASC
                 LIMIT 1",
                params![agent_id.as_ref(), workspace_id.as_ref()],
                wait_record_from_row,
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn reservation_by_id(&self, wait_id: impl AsRef<str>) -> StoreResult<Option<WaitRecord>> {
        self.expire_stale()?;
        self.waiter(wait_id.as_ref())
            .map(|waiter| waiter.filter(|waiter| waiter.status == "reserved"))
    }

    pub fn claim_reservation(
        &self,
        wait_id: impl AsRef<str>,
        agent_id: impl AsRef<str>,
    ) -> StoreResult<()> {
        self.expire_stale()?;
        let waiter = self.waiter(wait_id.as_ref())?;
        let Some(waiter) = waiter else {
            return Err(StoreError::ReservationOwnerMismatch);
        };
        if waiter.agent_id != agent_id.as_ref() || waiter.status != "reserved" {
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

    pub fn claim_reservation_with_intent_and_lease(
        &self,
        wait_id: impl AsRef<str>,
        agent_id: impl AsRef<str>,
        workspace_id: impl AsRef<str>,
        event: Event,
        lease_path: impl AsRef<str>,
        claim_observation: Option<ClaimObservation>,
    ) -> StoreResult<WaitRecord> {
        let wait_id = wait_id.as_ref().to_string();
        let agent_id = agent_id.as_ref().to_string();
        let workspace_id = workspace_id.as_ref().to_string();
        let lease_path = lease_path.as_ref().to_string();
        if !self.conn.is_autocommit() {
            return self.claim_reservation_with_intent_and_lease_inner(
                &wait_id,
                &agent_id,
                &workspace_id,
                &event,
                &lease_path,
                claim_observation.clone(),
            );
        }

        self.conn.execute_batch("BEGIN IMMEDIATE")?;

        let result = (|| -> StoreResult<WaitRecord> {
            let claimed = self.claim_reservation_with_intent_and_lease_inner(
                &wait_id,
                &agent_id,
                &workspace_id,
                &event,
                &lease_path,
                claim_observation.clone(),
            )?;
            self.conn.execute_batch("COMMIT")?;
            Ok(claimed)
        })();

        if result.is_err() {
            let _ = self.conn.execute_batch("ROLLBACK");
        }

        result
    }

    fn claim_reservation_with_intent_and_lease_inner(
        &self,
        wait_id: &str,
        agent_id: &str,
        workspace_id: &str,
        event: &Event,
        lease_path: &str,
        claim_observation: Option<ClaimObservation>,
    ) -> StoreResult<WaitRecord> {
        self.expire_stale()?;
        let reservation = self
            .waiter(wait_id)?
            .ok_or(StoreError::ReservationOwnerMismatch)?;
        if reservation.agent_id != agent_id
            || reservation.workspace_id != workspace_id
            || reservation.status != "reserved"
        {
            return Err(StoreError::ReservationOwnerMismatch);
        }

        self.conn.execute(
            "UPDATE wait_queue
             SET status = 'claimed'
             WHERE wait_id = ?1 AND status = 'reserved'",
            params![wait_id],
        )?;

        let mut event = event.clone();
        event.event_id = wait_id.to_string();
        self.append_inner(&event)?;
        self.acquire_claim_with_observation_and_event_inner(
            Some(wait_id),
            agent_id,
            workspace_id,
            lease_path,
            claim_observation,
        )?;

        let mut claimed = reservation;
        claimed.status = "claimed".to_string();
        Ok(claimed)
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
        target_agent_id: impl AsRef<str>,
        workspace_id: impl AsRef<str>,
    ) -> StoreResult<Vec<NotificationRecord>> {
        self.expire_stale()?;
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| -> StoreResult<Vec<NotificationRecord>> {
            let notifications = self.pending_notifications_after_in_transaction(
                target_agent_id.as_ref(),
                workspace_id.as_ref(),
                0,
            )?;

            for notification in &notifications {
                self.conn.execute(
                    "UPDATE notifications
                     SET status = 'delivered'
                     WHERE notification_id = ?1 AND status = 'pending'",
                    params![&notification.notification_id],
                )?;
            }

            self.conn.execute_batch("COMMIT")?;
            Ok(notifications)
        })();

        if result.is_err() {
            let _ = self.conn.execute_batch("ROLLBACK");
        }

        result
    }

    pub fn pending_notifications_after(
        &self,
        target_agent_id: impl AsRef<str>,
        workspace_id: impl AsRef<str>,
        after_sequence: u64,
    ) -> StoreResult<Vec<NotificationRecord>> {
        self.expire_stale()?;
        self.pending_notifications_after_in_transaction(
            target_agent_id.as_ref(),
            workspace_id.as_ref(),
            after_sequence,
        )
    }

    pub fn mark_notifications_delivered_through(
        &self,
        target_agent_id: impl AsRef<str>,
        workspace_id: impl AsRef<str>,
        sequence: u64,
    ) -> StoreResult<()> {
        if sequence == 0 {
            return Ok(());
        }
        let sequence = i64::try_from(sequence).unwrap_or(i64::MAX);
        self.conn.execute(
            "UPDATE notifications
             SET status = 'delivered'
             WHERE target_agent_id = ?1
                AND workspace_id = ?2
                AND status = 'pending'
                AND sequence <= ?3",
            params![target_agent_id.as_ref(), workspace_id.as_ref(), sequence],
        )?;
        Ok(())
    }

    fn pending_notifications_after_in_transaction(
        &self,
        target_agent_id: &str,
        workspace_id: &str,
        after_sequence: u64,
    ) -> StoreResult<Vec<NotificationRecord>> {
        let after_sequence = i64::try_from(after_sequence).unwrap_or(i64::MAX);
        let mut statement = self.conn.prepare(
            "SELECT
                notification_id,
                sequence,
                target_agent_id,
                workspace_id,
                kind,
                payload_json,
                status,
                created_at,
                expires_at
             FROM notifications
             WHERE target_agent_id = ?1
                AND workspace_id = ?2
                AND status = 'pending'
                AND sequence > ?3
             ORDER BY sequence ASC, rowid ASC",
        )?;
        let rows = statement.query_map(
            params![target_agent_id, workspace_id, after_sequence],
            notification_from_row,
        )?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    fn append_notification(
        &self,
        target_agent_id: &str,
        workspace_id: &str,
        kind: &str,
        payload: serde_json::Value,
    ) -> StoreResult<()> {
        let sequence = self.conn.query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1
             FROM notifications
             WHERE target_agent_id = ?1 AND workspace_id = ?2",
            params![target_agent_id, workspace_id],
            |row| row.get::<_, u64>(0),
        )?;
        let now = now_timestamp();
        let expires_at = timestamp_after(&now, CLAIMABLE_RESERVATION_TTL_SECONDS)?;
        self.conn.execute(
            "INSERT INTO notifications (
                notification_id,
                sequence,
                target_agent_id,
                workspace_id,
                kind,
                payload_json,
                status,
                created_at,
                expires_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7, ?8)",
            params![
                Uuid::new_v4().to_string(),
                sequence,
                target_agent_id,
                workspace_id,
                kind,
                payload.to_string(),
                now,
                expires_at,
            ],
        )?;

        Ok(())
    }

    pub fn append_activity(
        &self,
        agent_id: impl AsRef<str>,
        workspace_id: impl AsRef<str>,
    ) -> StoreResult<()> {
        self.append_activity_with_phase(agent_id, workspace_id, ActivityPhase::Exploring)
    }

    pub fn append_activity_with_phase(
        &self,
        agent_id: impl AsRef<str>,
        workspace_id: impl AsRef<str>,
        phase: ActivityPhase,
    ) -> StoreResult<()> {
        self.append_activity_inner(agent_id.as_ref(), workspace_id.as_ref(), phase)
    }

    fn append_activity_inner(
        &self,
        agent_id: &str,
        workspace_id: &str,
        phase: ActivityPhase,
    ) -> StoreResult<()> {
        let now = now_timestamp();
        let expires_at = timestamp_after(&now, ACTIVITY_TTL_SECONDS)?;
        self.conn.execute(
            "INSERT INTO activities (
                activity_id,
                agent_id,
                workspace_id,
                phase,
                expires_at
            ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                Uuid::new_v4().to_string(),
                agent_id,
                workspace_id,
                phase.as_str(),
                expires_at,
            ],
        )?;

        Ok(())
    }

    pub fn finalize_session_activity(
        &self,
        agent_id: impl AsRef<str>,
        workspace_id: impl AsRef<str>,
    ) -> StoreResult<(u64, u64)> {
        self.finalize_session_activity_with_phase(agent_id, workspace_id, ActivityPhase::Done)
    }

    pub fn finalize_session_activity_with_phase(
        &self,
        agent_id: impl AsRef<str>,
        workspace_id: impl AsRef<str>,
        _phase: ActivityPhase,
    ) -> StoreResult<(u64, u64)> {
        let agent_id = agent_id.as_ref().to_string();
        let workspace_id = workspace_id.as_ref().to_string();
        self.conn
            .execute_batch("SAVEPOINT stateful_finalize_activity")?;

        let result = self
            .finalize_session_activity_inner(&agent_id, &workspace_id)
            .and_then(|finalized| {
                self.conn
                    .execute_batch("RELEASE SAVEPOINT stateful_finalize_activity")?;
                Ok(finalized)
            });

        if result.is_err() {
            let _ = self
                .conn
                .execute_batch("ROLLBACK TO SAVEPOINT stateful_finalize_activity");
            let _ = self
                .conn
                .execute_batch("RELEASE SAVEPOINT stateful_finalize_activity");
        }

        result
    }

    fn finalize_session_activity_inner(
        &self,
        agent_id: &str,
        workspace_id: &str,
    ) -> StoreResult<(u64, u64)> {
        self.cancel_session_waiters_inner(agent_id, workspace_id)?;
        let released = self.release_session_claims_inner(agent_id, workspace_id)?;
        let completed = self.complete_session_reservations_inner(agent_id, workspace_id)?;
        self.append_inner(&Event::activity_finalized(
            agent_id.to_string(),
            workspace_id.to_string(),
            released,
            completed,
        ))?;
        Ok((released, completed))
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

    fn active_session_phase(
        &self,
        agent_id: &str,
        workspace_id: &str,
    ) -> StoreResult<Option<ActivityPhase>> {
        let now = now_timestamp();
        self.conn
            .query_row(
                "SELECT phase
                 FROM activities
                 WHERE agent_id = ?1
                    AND workspace_id = ?2
                    AND (expires_at IS NULL OR expires_at > ?3)
                 ORDER BY rowid DESC
                 LIMIT 1",
                params![agent_id, workspace_id, now],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map(|phase| phase.and_then(|phase| parse_activity_phase(&phase)))
            .map_err(StoreError::from)
    }

    pub fn policy_state_for_agent(
        &self,
        agent_id: &str,
        workspace_id: &str,
    ) -> StoreResult<PolicyState> {
        self.expire_stale()?;
        let phase = self.active_session_phase(agent_id, workspace_id)?;
        let scopes = self
            .active_reservation_scope_rows(agent_id, workspace_id)?
            .into_iter()
            .flat_map(|(_reservation_id, scopes, _purpose)| scopes)
            .collect::<Vec<_>>();
        if scopes.is_empty() {
            return Ok(PolicyState::default());
        }

        let mut state = PolicyState::default().with_active_reservation_scopes(scopes);
        if let Some(phase) = phase {
            state = state.with_activity_phase(phase);
        }

        Ok(state)
    }

    pub fn policy_state_for_reservation(
        &self,
        reservation_id: &str,
        workspace_id: &str,
    ) -> StoreResult<PolicyState> {
        self.expire_stale()?;
        let reservation = self
            .conn
            .query_row(
                "SELECT agent_id, scopes_json
                 FROM reservations
                 WHERE reservation_id = ?1
                    AND workspace_id = ?2
                    AND status = 'active'
                 ORDER BY declared_at DESC, rowid DESC
                 LIMIT 1",
                params![reservation_id, workspace_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;

        let (agent_id, scopes) = if let Some((agent_id, scopes_json)) = reservation {
            let scopes = serde_json::from_str(&scopes_json).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(err),
                )
            })?;
            (agent_id, scopes)
        } else {
            let wait_record = self
                .conn
                .query_row(
                    "SELECT agent_id, relative_path, action
                     FROM wait_queue
                     WHERE wait_id = ?1
                        AND workspace_id = ?2
                       AND status = 'reserved'
                     ORDER BY rowid DESC
                     LIMIT 1",
                    params![reservation_id, workspace_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .optional()?;
            let Some((agent_id, relative_path, action)) = wait_record else {
                return Ok(PolicyState::default());
            };
            let scope = if action == "write_directory" {
                ReservationScope::directory(relative_path)
            } else {
                ReservationScope::file(relative_path)
            };
            (agent_id, vec![scope])
        };

        if scopes.is_empty() {
            return Ok(PolicyState::default());
        }

        let mut state = PolicyState::default().with_active_reservation_scopes(scopes);
        if let Some(phase) = self.active_session_phase(&agent_id, workspace_id)? {
            state = state.with_activity_phase(phase);
        }

        Ok(state)
    }

    pub fn active_exact_file_intent_by_agent(
        &self,
        workspace_id: impl AsRef<str>,
        relative_path: impl AsRef<str>,
        agent_id: impl AsRef<str>,
    ) -> StoreResult<bool> {
        self.expire_stale()?;
        let relative_path = normalize_relative_path(relative_path.as_ref());

        for (_reservation_id, scopes, _purpose) in
            self.active_reservation_scope_rows(agent_id.as_ref(), workspace_id.as_ref())?
        {
            if scopes.iter().any(
                |scope| matches!(scope, ReservationScope::File(path) if path == &relative_path),
            ) {
                return Ok(true);
            }
        }

        Ok(false)
    }

    pub fn active_exact_file_intent_by_reservation(
        &self,
        workspace_id: impl AsRef<str>,
        relative_path: impl AsRef<str>,
        reservation_id: impl AsRef<str>,
    ) -> StoreResult<bool> {
        self.expire_stale()?;
        let workspace_id = workspace_id.as_ref();
        let reservation_id = reservation_id.as_ref();
        let relative_path = normalize_relative_path(relative_path.as_ref());
        let scopes_json = self
            .conn
            .query_row(
                "SELECT scopes_json
                 FROM reservations
                 WHERE reservation_id = ?1
                    AND workspace_id = ?2
                    AND status = 'active'
                 ORDER BY declared_at DESC, rowid DESC
                 LIMIT 1",
                params![reservation_id, workspace_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;

        if let Some(scopes_json) = scopes_json {
            let scopes: Vec<ReservationScope> =
                serde_json::from_str(&scopes_json).map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(err),
                    )
                })?;
            return Ok(scopes.iter().any(
                |scope| matches!(scope, ReservationScope::File(path) if path == &relative_path),
            ));
        }

        self.conn
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM wait_queue
                    WHERE wait_id = ?1
                       AND workspace_id = ?2
                       AND status IN ('reserved', 'claimed')
                       AND action = 'write_file'
                       AND relative_path = ?3
                )",
                params![reservation_id, workspace_id, relative_path],
                |row| row.get::<_, bool>(0),
            )
            .map_err(StoreError::from)
    }

    pub fn expire_stale(&self) -> StoreResult<()> {
        self.expire_stale_at(&now_timestamp())
    }

    pub fn prune_retention(&self) -> StoreResult<()> {
        let cutoff =
            format_timestamp(OffsetDateTime::now_utc() - Duration::days(EVENT_RETENTION_DAYS));
        self.prune_retention_before(&cutoff)
    }

    pub fn prune_retention_before(&self, cutoff: &str) -> StoreResult<()> {
        if !self.conn.is_autocommit() {
            return self.prune_retention_before_inner(cutoff);
        }

        self.conn.execute_batch("BEGIN IMMEDIATE")?;

        let result = (|| -> StoreResult<()> {
            self.prune_retention_before_inner(cutoff)?;
            self.conn.execute_batch("COMMIT")?;
            Ok(())
        })();

        if result.is_err() {
            let _ = self.conn.execute_batch("ROLLBACK");
        }

        result
    }

    pub fn expire_stale_at(&self, now: &str) -> StoreResult<()> {
        if !self.conn.is_autocommit() {
            return self.expire_stale_at_inner(now);
        }

        self.conn.execute_batch("BEGIN IMMEDIATE")?;

        let result = (|| -> StoreResult<()> {
            self.expire_stale_at_inner(now)?;
            self.conn.execute_batch("COMMIT")?;
            Ok(())
        })();

        if result.is_err() {
            let _ = self.conn.execute_batch("ROLLBACK");
        }

        result
    }

    fn refresh_live_current_state(&self) -> StoreResult<()> {
        let now = now_timestamp();
        if !self.conn.is_autocommit() {
            self.expire_stale_at_inner(&now)?;
            self.promote_unblocked_waiters()?;
            return Ok(());
        }

        self.conn.execute_batch("BEGIN IMMEDIATE")?;

        let result = (|| -> StoreResult<()> {
            self.expire_stale_at_inner(&now)?;
            self.promote_unblocked_waiters()?;
            self.conn.execute_batch("COMMIT")?;
            Ok(())
        })();

        if result.is_err() {
            let _ = self.conn.execute_batch("ROLLBACK");
        }

        result
    }

    fn expire_stale_at_inner(&self, now: &str) -> StoreResult<()> {
        self.conn.execute(
            "UPDATE reservations
             SET status = 'expired'
             WHERE status = 'active' AND expires_at IS NOT NULL AND expires_at <= ?1",
            [now],
        )?;

        let mut statement = self.conn.prepare(
            "SELECT DISTINCT workspace_id, relative_path FROM wait_queue
             WHERE status = 'reserved'
                AND reservation_expires_at IS NOT NULL
                AND reservation_expires_at <= ?1",
        )?;
        let expired_reservations = statement
            .query_map([now], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);

        self.conn.execute(
            "UPDATE wait_queue
             SET status = 'expired'
             WHERE status = 'reserved'
                AND reservation_expires_at IS NOT NULL
                AND reservation_expires_at <= ?1",
            [now],
        )?;
        for (workspace_id, relative_path) in expired_reservations {
            self.promote_next_waiter_after_path_release(&workspace_id, &relative_path)?;
        }

        let mut statement = self.conn.prepare(
            "SELECT DISTINCT workspace_id, relative_path FROM claims
             WHERE status = 'active' AND expires_at IS NOT NULL AND expires_at <= ?1",
        )?;
        let expired_claims = statement
            .query_map([now], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);

        self.conn.execute(
            "UPDATE claims
             SET status = 'expired'
             WHERE status = 'active' AND expires_at IS NOT NULL AND expires_at <= ?1",
            [now],
        )?;
        for (workspace_id, relative_path) in expired_claims {
            self.promote_waiters_after_lease_release(&workspace_id, &relative_path)?;
        }

        self.conn.execute(
            "UPDATE notifications
             SET status = 'expired'
             WHERE status = 'pending' AND expires_at IS NOT NULL AND expires_at <= ?1",
            [now],
        )?;

        Ok(())
    }

    fn prune_retention_before_inner(&self, cutoff: &str) -> StoreResult<()> {
        self.conn
            .execute("DELETE FROM events WHERE created_at < ?1", [cutoff])?;
        self.conn.execute(
            "DELETE FROM reconciliations WHERE created_at < ?1",
            [cutoff],
        )?;
        self.conn
            .execute("DELETE FROM conflicts WHERE checked_at < ?1", [cutoff])?;
        self.conn.execute(
            "DELETE FROM human_observations WHERE created_at < ?1",
            [cutoff],
        )?;
        self.conn.execute(
            "DELETE FROM notifications
             WHERE status IN ('expired', 'delivered') AND created_at < ?1",
            [cutoff],
        )?;
        Ok(())
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
            ",
        )?;
        self.migrate_agent_identity_schema()?;
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS events (
                event_id TEXT PRIMARY KEY,
                event_type TEXT NOT NULL,
                agent_id TEXT NOT NULL,
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

            CREATE INDEX IF NOT EXISTS idx_events_agent_created_at
                ON events(agent_id, created_at);

            CREATE INDEX IF NOT EXISTS idx_events_agent_sequence
                ON events(agent_id, sequence);

            CREATE TABLE IF NOT EXISTS agents (
                agent_id TEXT PRIMARY KEY,
                workspace_id TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_agents_workspace_agent
                ON agents(workspace_id, agent_id);

            CREATE TABLE IF NOT EXISTS activities (
                activity_id TEXT PRIMARY KEY,
                agent_id TEXT NOT NULL,
                workspace_id TEXT NOT NULL,
                phase TEXT NOT NULL DEFAULT 'exploring',
                expires_at TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_activities_workspace_expires_at
                ON activities(workspace_id, expires_at);

            CREATE INDEX IF NOT EXISTS idx_activities_agent_workspace_expires_at
                ON activities(agent_id, workspace_id, expires_at);

            CREATE TABLE IF NOT EXISTS reservations (
                reservation_id TEXT PRIMARY KEY,
                agent_id TEXT NOT NULL,
                workspace_id TEXT NOT NULL,
                purpose TEXT NOT NULL,
                scopes_json TEXT NOT NULL,
                status TEXT NOT NULL,
                declared_at TEXT NOT NULL,
                expires_at TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_reservations_agent_status_expires_at
                ON reservations(agent_id, status, expires_at);

            CREATE TABLE IF NOT EXISTS claims (
                claim_id TEXT PRIMARY KEY,
                reservation_id TEXT,
                agent_id TEXT,
                workspace_id TEXT NOT NULL,
                repo_id TEXT,
                relative_path TEXT,
                absolute_path TEXT,
                purpose TEXT,
                action TEXT NOT NULL DEFAULT 'write_file',
                status TEXT NOT NULL,
                expires_at TEXT,
                observed_exists INTEGER,
                observed_content_hash TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_claims_workspace_path_status
                ON claims(workspace_id, relative_path, status);

            CREATE INDEX IF NOT EXISTS idx_claims_workspace_absolute_status_expires_at
                ON claims(workspace_id, absolute_path, status, expires_at);

            CREATE INDEX IF NOT EXISTS idx_claims_repo_relative_status_expires_at
                ON claims(repo_id, relative_path, status, expires_at);

            CREATE TABLE IF NOT EXISTS wait_queue (
                wait_id TEXT PRIMARY KEY,
                request_id TEXT,
                agent_id TEXT NOT NULL,
                workspace_id TEXT NOT NULL,
                repo_id TEXT,
                worktree_id TEXT,
                root TEXT,
                branch TEXT,
                relative_path TEXT NOT NULL,
                action TEXT NOT NULL,
                status TEXT NOT NULL,
                requested_at TEXT NOT NULL,
                reservation_expires_at TEXT,
                blocking_agent_id TEXT,
                purpose TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_wait_queue_workspace_path_status
                ON wait_queue(workspace_id, relative_path, status);

            CREATE INDEX IF NOT EXISTS idx_wait_queue_agent_status
                ON wait_queue(agent_id, status);

            CREATE TABLE IF NOT EXISTS notifications (
                notification_id TEXT PRIMARY KEY,
                sequence INTEGER NOT NULL DEFAULT 0,
                target_agent_id TEXT NOT NULL,
                workspace_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                status TEXT NOT NULL,
                created_at TEXT NOT NULL,
                expires_at TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_notifications_agent_status
                ON notifications(target_agent_id, status);

            CREATE TABLE IF NOT EXISTS conflicts (
                conflict_id TEXT PRIMARY KEY,
                agent_id TEXT NOT NULL,
                checked_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_conflicts_agent_checked_at
                ON conflicts(agent_id, checked_at);

            CREATE TABLE IF NOT EXISTS overrides (
                override_id TEXT PRIMARY KEY,
                agent_id TEXT NOT NULL,
                status TEXT NOT NULL,
                expires_at TEXT
            );

            CREATE TABLE IF NOT EXISTS reconciliations (
                reconciliation_id TEXT PRIMARY KEY,
                agent_id TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_reconciliations_agent_created_at
                ON reconciliations(agent_id, created_at);

            CREATE TABLE IF NOT EXISTS human_observations (
                observation_id TEXT PRIMARY KEY,
                workspace_id TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS outbox (
                outbox_id TEXT PRIMARY KEY,
                agent_id TEXT NOT NULL,
                workspace_id TEXT NOT NULL DEFAULT '',
                sequence INTEGER NOT NULL,
                event_type TEXT NOT NULL DEFAULT '',
                payload_json TEXT NOT NULL DEFAULT '{}',
                sync_status TEXT NOT NULL
            );
            ",
        )?;

        self.add_column_if_missing(
            "events",
            "repo_id",
            "ALTER TABLE events ADD COLUMN repo_id TEXT;",
        )?;
        self.add_column_if_missing(
            "events",
            "worktree_id",
            "ALTER TABLE events ADD COLUMN worktree_id TEXT;",
        )?;
        self.add_column_if_missing("events", "root", "ALTER TABLE events ADD COLUMN root TEXT;")?;
        self.add_column_if_missing(
            "events",
            "branch",
            "ALTER TABLE events ADD COLUMN branch TEXT;",
        )?;
        self.add_column_if_missing(
            "wait_queue",
            "request_id",
            "ALTER TABLE wait_queue ADD COLUMN request_id TEXT;",
        )?;
        self.add_column_if_missing(
            "wait_queue",
            "repo_id",
            "ALTER TABLE wait_queue ADD COLUMN repo_id TEXT;",
        )?;
        self.add_column_if_missing(
            "wait_queue",
            "worktree_id",
            "ALTER TABLE wait_queue ADD COLUMN worktree_id TEXT;",
        )?;
        self.add_column_if_missing(
            "wait_queue",
            "root",
            "ALTER TABLE wait_queue ADD COLUMN root TEXT;",
        )?;
        self.add_column_if_missing(
            "wait_queue",
            "branch",
            "ALTER TABLE wait_queue ADD COLUMN branch TEXT;",
        )?;
        self.add_column_if_missing(
            "reservations",
            "purpose",
            "ALTER TABLE reservations ADD COLUMN purpose TEXT;",
        )?;
        self.add_column_if_missing(
            "wait_queue",
            "purpose",
            "ALTER TABLE wait_queue ADD COLUMN purpose TEXT;",
        )?;
        self.add_column_if_missing(
            "claims",
            "reservation_id",
            "ALTER TABLE claims ADD COLUMN reservation_id TEXT;",
        )?;
        self.conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_claims_reservation_path_status
                ON claims(reservation_id, workspace_id, relative_path, status);",
        )?;
        self.add_column_if_missing(
            "claims",
            "purpose",
            "ALTER TABLE claims ADD COLUMN purpose TEXT;",
        )?;
        self.add_column_if_missing(
            "claims",
            "action",
            "ALTER TABLE claims ADD COLUMN action TEXT NOT NULL DEFAULT 'write_file';",
        )?;
        self.add_column_if_missing(
            "claims",
            "observed_exists",
            "ALTER TABLE claims ADD COLUMN observed_exists INTEGER;",
        )?;
        self.add_column_if_missing(
            "claims",
            "observed_content_hash",
            "ALTER TABLE claims ADD COLUMN observed_content_hash TEXT;",
        )?;
        self.add_column_if_missing(
            "activities",
            "phase",
            "ALTER TABLE activities ADD COLUMN phase TEXT NOT NULL DEFAULT 'exploring';",
        )?;
        self.add_column_if_missing(
            "outbox",
            "workspace_id",
            "ALTER TABLE outbox ADD COLUMN workspace_id TEXT NOT NULL DEFAULT '';",
        )?;
        self.add_column_if_missing(
            "outbox",
            "event_type",
            "ALTER TABLE outbox ADD COLUMN event_type TEXT NOT NULL DEFAULT '';",
        )?;
        self.add_column_if_missing(
            "outbox",
            "payload_json",
            "ALTER TABLE outbox ADD COLUMN payload_json TEXT NOT NULL DEFAULT '{}';",
        )?;
        self.add_column_if_missing(
            "outbox",
            "sync_status",
            "ALTER TABLE outbox ADD COLUMN sync_status TEXT NOT NULL DEFAULT 'pending';",
        )?;
        self.conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_outbox_agent_sequence_sync_status
                ON outbox(agent_id, sequence, sync_status);",
        )?;
        self.add_column_if_missing(
            "notifications",
            "sequence",
            "ALTER TABLE notifications ADD COLUMN sequence INTEGER NOT NULL DEFAULT 0;",
        )?;
        self.conn.execute_batch(
            "
            UPDATE notifications
            SET sequence = rowid
            WHERE sequence = 0;

            CREATE INDEX IF NOT EXISTS idx_notifications_agent_workspace_status_sequence
                ON notifications(target_agent_id, workspace_id, status, sequence);
            ",
        )?;
        self.remove_legacy_rows_without_required_purpose()?;
        self.conn.execute_batch(
            "
            CREATE UNIQUE INDEX IF NOT EXISTS idx_wait_queue_request_id
                ON wait_queue(request_id)
                WHERE request_id IS NOT NULL;
            ",
        )?;

        Ok(())
    }

    fn migrate_agent_identity_schema(&self) -> StoreResult<()> {
        if self.has_table("sessions")? && !self.has_table("agents")? {
            self.conn
                .execute_batch("ALTER TABLE sessions RENAME TO agents;")?;
        }

        self.rename_legacy_identity_column("agents", "session_id", "agent_id")?;
        if self.has_table("sessions")? {
            self.copy_legacy_sessions_into_agents()?;
            self.conn.execute_batch("DROP TABLE sessions;")?;
        }

        for (table, old_column, new_column) in [
            ("events", "session_id", "agent_id"),
            ("activities", "session_id", "agent_id"),
            ("reservations", "session_id", "agent_id"),
            ("claims", "session_id", "agent_id"),
            ("wait_queue", "session_id", "agent_id"),
            ("wait_queue", "blocking_session_id", "blocking_agent_id"),
            ("notifications", "target_session_id", "target_agent_id"),
            ("conflicts", "session_id", "agent_id"),
            ("overrides", "session_id", "agent_id"),
            ("reconciliations", "session_id", "agent_id"),
            ("outbox", "session_id", "agent_id"),
        ] {
            self.rename_legacy_identity_column(table, old_column, new_column)?;
        }

        self.drop_legacy_agent_identity_index_names()?;

        Ok(())
    }

    fn drop_legacy_agent_identity_index_names(&self) -> StoreResult<()> {
        self.conn.execute_batch(
            "
            DROP INDEX IF EXISTS idx_events_session_created_at;
            DROP INDEX IF EXISTS idx_events_session_sequence;
            DROP INDEX IF EXISTS idx_agents_workspace_session;
            DROP INDEX IF EXISTS idx_activities_session_workspace_expires_at;
            DROP INDEX IF EXISTS idx_reservations_session_status_expires_at;
            DROP INDEX IF EXISTS idx_wait_queue_session_status;
            DROP INDEX IF EXISTS idx_notifications_session_status;
            DROP INDEX IF EXISTS idx_conflicts_session_checked_at;
            DROP INDEX IF EXISTS idx_reconciliations_session_created_at;
            DROP INDEX IF EXISTS idx_outbox_session_sequence_sync_status;
            ",
        )?;
        Ok(())
    }

    fn copy_legacy_sessions_into_agents(&self) -> StoreResult<()> {
        let session_columns = self.table_columns("sessions")?;
        let agent_columns = self.table_columns("agents")?;
        if !session_columns.iter().any(|name| name == "session_id")
            || !session_columns.iter().any(|name| name == "workspace_id")
            || !session_columns.iter().any(|name| name == "updated_at")
            || !agent_columns.iter().any(|name| name == "agent_id")
            || !agent_columns.iter().any(|name| name == "workspace_id")
            || !agent_columns.iter().any(|name| name == "updated_at")
        {
            return Ok(());
        }

        self.conn.execute_batch(
            "
            INSERT OR IGNORE INTO agents (agent_id, workspace_id, updated_at)
            SELECT session_id, workspace_id, updated_at
            FROM sessions
            WHERE session_id IS NOT NULL AND trim(session_id) <> '';
            ",
        )?;
        Ok(())
    }

    fn rename_legacy_identity_column(
        &self,
        table: &str,
        old_column: &str,
        new_column: &str,
    ) -> StoreResult<()> {
        let supported = matches!(
            (table, old_column, new_column),
            ("agents", "session_id", "agent_id")
                | ("events", "session_id", "agent_id")
                | ("activities", "session_id", "agent_id")
                | ("reservations", "session_id", "agent_id")
                | ("claims", "session_id", "agent_id")
                | ("wait_queue", "session_id", "agent_id")
                | ("wait_queue", "blocking_session_id", "blocking_agent_id")
                | ("notifications", "target_session_id", "target_agent_id")
                | ("conflicts", "session_id", "agent_id")
                | ("overrides", "session_id", "agent_id")
                | ("reconciliations", "session_id", "agent_id")
                | ("outbox", "session_id", "agent_id")
        );
        if !supported || !self.has_table(table)? {
            return Ok(());
        }

        let columns = self.table_columns(table)?;
        let has_old = columns.iter().any(|name| name == old_column);
        let has_new = columns.iter().any(|name| name == new_column);
        match (has_old, has_new) {
            (true, false) => {
                self.drop_indexes_referencing_column(table, old_column)?;
                self.conn.execute_batch(&format!(
                    "ALTER TABLE {table} RENAME COLUMN {old_column} TO {new_column};"
                ))?;
            }
            (true, true) => {
                self.conn.execute_batch(&format!(
                    "UPDATE {table}
                     SET {new_column} = {old_column}
                     WHERE ({new_column} IS NULL OR trim({new_column}) = '')
                        AND {old_column} IS NOT NULL
                        AND trim({old_column}) <> '';"
                ))?;
                self.drop_indexes_referencing_column(table, old_column)?;
                self.conn
                    .execute_batch(&format!("ALTER TABLE {table} DROP COLUMN {old_column};"))?;
            }
            _ => {}
        }

        Ok(())
    }

    fn table_columns(&self, table: &str) -> StoreResult<Vec<String>> {
        let mut statement = self.conn.prepare(&format!("PRAGMA table_info({table})"))?;
        Ok(statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?)
    }

    fn drop_indexes_referencing_column(&self, table: &str, column: &str) -> StoreResult<()> {
        let quoted_table = Self::quote_sql_identifier(table);
        let mut statement = self
            .conn
            .prepare(&format!("PRAGMA index_list({quoted_table})"))?;
        let indexes = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(1)?, row.get::<_, String>(3)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        for (index, origin) in indexes {
            if origin != "c" {
                continue;
            }

            let quoted_index = Self::quote_sql_identifier(&index);
            let mut index_statement = self
                .conn
                .prepare(&format!("PRAGMA index_info({quoted_index})"))?;
            let columns = index_statement
                .query_map([], |row| row.get::<_, String>(2))?
                .collect::<Result<Vec<_>, _>>()?;
            if columns.iter().any(|name| name == column) {
                self.conn
                    .execute_batch(&format!("DROP INDEX IF EXISTS {quoted_index};"))?;
            }
        }

        Ok(())
    }

    fn quote_sql_identifier(identifier: &str) -> String {
        format!("\"{}\"", identifier.replace('"', "\"\""))
    }

    fn remove_legacy_rows_without_required_purpose(&self) -> StoreResult<()> {
        self.conn.execute(
            "DELETE FROM reservations WHERE purpose IS NULL OR trim(purpose) = ''",
            [],
        )?;
        self.conn.execute(
            "DELETE FROM wait_queue WHERE purpose IS NULL OR trim(purpose) = ''",
            [],
        )?;
        let legacy_lease_paths = {
            let mut statement = self.conn.prepare(
                "SELECT DISTINCT workspace_id, relative_path
                 FROM claims
                 WHERE status = 'active'
                    AND relative_path IS NOT NULL
                    AND (purpose IS NULL OR trim(purpose) = '')",
            )?;
            statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        self.conn.execute(
            "UPDATE claims
             SET status = 'released'
             WHERE status = 'active' AND (purpose IS NULL OR trim(purpose) = '')",
            [],
        )?;
        for (workspace_id, relative_path) in legacy_lease_paths {
            self.promote_waiters_after_lease_release(&workspace_id, &relative_path)?;
        }
        Ok(())
    }

    fn add_column_if_missing(&self, table: &str, column: &str, ddl: &str) -> StoreResult<()> {
        let supported = (table == "events"
            && matches!(column, "repo_id" | "worktree_id" | "root" | "branch"))
            || (table == "reservations" && column == "purpose")
            || (table == "claims"
                && matches!(
                    column,
                    "reservation_id"
                        | "purpose"
                        | "action"
                        | "observed_exists"
                        | "observed_content_hash"
                ))
            || (table == "outbox"
                && matches!(
                    column,
                    "workspace_id" | "event_type" | "payload_json" | "sync_status"
                ))
            || (table == "wait_queue"
                && matches!(
                    column,
                    "request_id" | "purpose" | "repo_id" | "worktree_id" | "root" | "branch"
                ))
            || (table == "activities" && column == "phase")
            || (table == "notifications" && column == "sequence");
        if !supported {
            return Ok(());
        }

        let mut statement = self.conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        if !columns.iter().any(|name| name == column) {
            self.conn.execute_batch(ddl)?;
        }
        Ok(())
    }

    fn refresh_agent_state(
        &self,
        agent_id: &str,
        workspace_id: &str,
        heartbeat_at: &str,
    ) -> StoreResult<()> {
        self.expire_stale_at(heartbeat_at)?;

        let lease_expires_at = timestamp_after(heartbeat_at, CLAIM_TTL_SECONDS)?;
        let active_claims = {
            let mut statement = self.conn.prepare(
                "SELECT claim_id, relative_path, action
                 FROM claims
                 WHERE agent_id = ?1
                    AND workspace_id = ?2
                    AND status = ?3
                    AND (expires_at IS NULL OR expires_at < ?4)",
            )?;
            statement
                .query_map(
                    params![agent_id, workspace_id, "active", lease_expires_at],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )?
                .collect::<Result<Vec<_>, _>>()?
        };
        for (claim_id, relative_path, action) in active_claims {
            let lease_is_directory = action == "write_directory";
            if self
                .active_reservation_purpose_for_lease(
                    agent_id,
                    workspace_id,
                    &relative_path,
                    lease_is_directory,
                )?
                .is_none()
            {
                continue;
            }
            self.conn.execute(
                "UPDATE claims
                 SET expires_at = ?1
                 WHERE claim_id = ?2
                    AND status = ?3
                    AND (expires_at IS NULL OR expires_at < ?1)",
                params![lease_expires_at, claim_id, "active"],
            )?;
        }

        let activity_expires_at = timestamp_after(heartbeat_at, ACTIVITY_TTL_SECONDS)?;
        self.conn.execute(
            "UPDATE activities
             SET expires_at = ?1
             WHERE agent_id = ?2
                AND workspace_id = ?3
                AND phase IN ('exploring', 'editing', 'testing')
                AND (expires_at IS NULL OR expires_at < ?1)",
            params![activity_expires_at, agent_id, workspace_id],
        )?;

        let mut statement = self.conn.prepare(
            "SELECT reservation_id, declared_at, expires_at
             FROM reservations
             WHERE agent_id = ?1 AND workspace_id = ?2 AND status = ?3",
        )?;
        let reservations = statement
            .query_map(params![agent_id, workspace_id, "active"], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);

        for (reservation_id, declared_at, current_expires_at) in reservations {
            let refreshed =
                timestamp_after(heartbeat_at, ACTIVE_CLAIMABLE_RESERVATION_TTL_SECONDS)?;
            let max_expires_at = timestamp_after(&declared_at, ACTIVE_RESERVATION_MAX_SECONDS)?;
            let next_expires_at = if refreshed.as_str() < max_expires_at.as_str() {
                refreshed
            } else {
                max_expires_at
            };
            if current_expires_at
                .as_deref()
                .is_some_and(|current| current >= next_expires_at.as_str())
            {
                continue;
            }
            self.conn.execute(
                "UPDATE reservations SET expires_at = ?1 WHERE reservation_id = ?2",
                params![next_expires_at, reservation_id],
            )?;
        }

        Ok(())
    }

    fn materialize(&self, event: &Event) -> StoreResult<()> {
        match event.event_type {
            EventType::AgentRegistered => {
                self.conn.execute(
                    "INSERT INTO agents (agent_id, workspace_id, updated_at)
                     VALUES (?1, ?2, ?3)
                     ON CONFLICT(agent_id) DO UPDATE SET
                        workspace_id = excluded.workspace_id,
                        updated_at = excluded.updated_at",
                    params![event.agent_id, event.workspace_id, event.created_at],
                )?;
            }
            EventType::AgentHeartbeat => {
                self.conn.execute(
                    "INSERT INTO agents (agent_id, workspace_id, updated_at)
                     VALUES (?1, ?2, ?3)
                     ON CONFLICT(agent_id) DO UPDATE SET
                        workspace_id = excluded.workspace_id,
                        updated_at = excluded.updated_at",
                    params![event.agent_id, event.workspace_id, event.created_at],
                )?;
                self.refresh_agent_state(&event.agent_id, &event.workspace_id, &event.created_at)?;
            }
            EventType::ReservationDeclared => {
                let scopes = required_intent_scopes(&event.payload["scopes"])?;
                let scopes_json = serde_json::to_string(&scopes)
                    .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))?;
                let purpose = required_purpose(
                    event.payload["purpose"]
                        .as_str()
                        .ok_or(StoreError::MissingPurpose)?,
                )?;
                let expires_at =
                    timestamp_after(&event.created_at, ACTIVE_CLAIMABLE_RESERVATION_TTL_SECONDS)?;
                self.conn.execute(
                    "INSERT INTO reservations (
                        reservation_id,
                        agent_id,
                        workspace_id,
                        purpose,
                        scopes_json,
                        status,
                        declared_at,
                        expires_at
                    ) VALUES (?1, ?2, ?3, ?4, ?5, 'active', ?6, ?7)",
                    params![
                        event.event_id,
                        event.agent_id,
                        event.workspace_id,
                        purpose,
                        scopes_json,
                        event.created_at,
                        expires_at,
                    ],
                )?;
                self.append_activity_inner(
                    &event.agent_id,
                    &event.workspace_id,
                    ActivityPhase::Exploring,
                )?;
            }
            EventType::ClaimAcquired
            | EventType::ClaimReleased
            | EventType::ReservationRequested
            | EventType::ReservationClaimed
            | EventType::ReservationCanceled
            | EventType::ActivityFinalized
            | EventType::AuthorizationDenied => {}
        }

        Ok(())
    }

    fn waiter(&self, wait_id: &str) -> StoreResult<Option<WaitRecord>> {
        self.conn
            .query_row(
                "SELECT
                    wait_id,
                    agent_id,
                    workspace_id,
                    repo_id,
                    worktree_id,
                    root,
                    branch,
                    relative_path,
                    action,
                    status,
                    requested_at,
                    reservation_expires_at,
                    blocking_agent_id,
                    purpose
                 FROM wait_queue
                 WHERE wait_id = ?1",
                params![wait_id],
                wait_record_from_row,
            )
            .optional()
            .map_err(StoreError::from)
    }

    fn promote_waiters_after_lease_release(
        &self,
        workspace_id: &str,
        relative_path: &str,
    ) -> StoreResult<()> {
        while self
            .promote_next_waiter_after_path_release(workspace_id, relative_path)?
            .is_some()
        {}

        Ok(())
    }

    fn promote_unblocked_waiters(&self) -> StoreResult<()> {
        loop {
            let mut statement = self.conn.prepare(
                "SELECT
                    wait_id,
                    agent_id,
                    workspace_id,
                    relative_path,
                    action,
                    status,
                    requested_at,
                    reservation_expires_at,
                    blocking_agent_id,
                    purpose
                 FROM wait_queue
                 WHERE status = 'queued'
                 ORDER BY rowid ASC",
            )?;
            let waiters = statement
                .query_map([], wait_record_from_row)?
                .collect::<Result<Vec<_>, _>>()?;
            drop(statement);

            let mut promoted = false;
            for waiter in waiters {
                if self.waiter_has_active_conflict(&waiter)? {
                    continue;
                }
                if self.promote_waiter_by_id(&waiter.wait_id)?.is_some() {
                    promoted = true;
                    break;
                }
            }

            if !promoted {
                return Ok(());
            }
        }
    }

    fn promote_next_waiter_after_path_release(
        &self,
        workspace_id: &str,
        relative_path: &str,
    ) -> StoreResult<Option<WaitRecord>> {
        let relative_path = normalize_relative_path(relative_path);
        let mut statement = self.conn.prepare(
            "SELECT
                wait_id,
                agent_id,
                workspace_id,
                relative_path,
                action,
                status,
                requested_at,
                reservation_expires_at,
                blocking_agent_id,
                purpose
             FROM wait_queue
             WHERE workspace_id = ?1
                AND status = 'queued'
                AND (
                    relative_path = ?2
                    OR (
                        action = 'write_directory'
                        AND relative_path != ?2
                        AND substr(?2, 1, length(relative_path) + 1) = relative_path || '/'
                    )
                    OR substr(relative_path, 1, length(?2) + 1) = ?2 || '/'
                )
             ORDER BY rowid ASC",
        )?;
        let waiters = statement
            .query_map(params![workspace_id, relative_path], wait_record_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);

        for waiter in waiters {
            if self.waiter_has_active_conflict(&waiter)? {
                continue;
            }

            return self.promote_waiter_by_id(&waiter.wait_id);
        }

        Ok(None)
    }

    fn update_waiter_identity_if_missing(
        &self,
        wait_id: &str,
        identity: WorkspaceIdentity<'_>,
    ) -> StoreResult<()> {
        if identity.is_empty() {
            return Ok(());
        }
        self.conn.execute(
            "UPDATE wait_queue
             SET repo_id = COALESCE(repo_id, ?2),
                 worktree_id = COALESCE(worktree_id, ?3),
                 root = COALESCE(root, ?4),
                 branch = COALESCE(branch, ?5)
             WHERE wait_id = ?1",
            params![
                wait_id,
                identity.repo_id,
                identity.worktree_id,
                identity.root,
                identity.branch,
            ],
        )?;
        Ok(())
    }

    fn waiter_has_active_conflict(&self, waiter: &WaitRecord) -> StoreResult<bool> {
        if waiter.action == "write_directory" {
            let directory_prefix = format!("{}/", waiter.relative_path);
            let active_claim_conflict = self.conn.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM claims
                    WHERE workspace_id = ?1
                       AND agent_id != ?2
                       AND status = 'active'
                       AND (
                           (action = 'write_directory' AND relative_path = ?3)
                           OR substr(relative_path, 1, ?4) = ?5
                           OR (action = 'write_directory'
                              AND substr(?3, 1, length(relative_path) + 1) = relative_path || '/')
                       )
                )",
                params![
                    &waiter.workspace_id,
                    &waiter.agent_id,
                    &waiter.relative_path,
                    directory_prefix.len() as i64,
                    &directory_prefix,
                ],
                |row| row.get::<_, bool>(0),
            )?;
            let active_reservation_conflict = self.conn.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM wait_queue
                    WHERE workspace_id = ?1
                       AND wait_id != ?2
                       AND status = 'reserved'
                       AND (
                           (action = 'write_directory' AND relative_path = ?3)
                           OR substr(relative_path, 1, ?4) = ?5
                           OR (action = 'write_directory'
                              AND substr(?3, 1, length(relative_path) + 1) = relative_path || '/')
                       )
                )",
                params![
                    &waiter.workspace_id,
                    &waiter.wait_id,
                    &waiter.relative_path,
                    directory_prefix.len() as i64,
                    &directory_prefix,
                ],
                |row| row.get::<_, bool>(0),
            )?;
            return Ok(active_claim_conflict || active_reservation_conflict);
        }

        let active_claim_conflict = self.conn.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM claims
                WHERE workspace_id = ?1
                   AND agent_id != ?2
                   AND status = 'active'
                   AND (
                       (action = 'write_file' AND relative_path = ?3)
                       OR (action = 'write_directory'
                          AND substr(?3, 1, length(relative_path) + 1) = relative_path || '/')
                   )
            )",
            params![
                &waiter.workspace_id,
                &waiter.agent_id,
                &waiter.relative_path
            ],
            |row| row.get::<_, bool>(0),
        )?;
        let active_reservation_conflict = self.conn.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM wait_queue
                WHERE workspace_id = ?1
                   AND wait_id != ?2
                   AND status = 'reserved'
                   AND (
                       (action = 'write_file' AND relative_path = ?3)
                       OR (action = 'write_directory'
                          AND substr(?3, 1, length(relative_path) + 1) = relative_path || '/')
                   )
            )",
            params![&waiter.workspace_id, &waiter.wait_id, &waiter.relative_path],
            |row| row.get::<_, bool>(0),
        )?;
        Ok(active_claim_conflict || active_reservation_conflict)
    }

    fn promote_waiter_by_id(&self, wait_id: &str) -> StoreResult<Option<WaitRecord>> {
        let now = now_timestamp();
        let reservation_expires_at = timestamp_after(&now, CLAIMABLE_RESERVATION_TTL_SECONDS)?;
        self.conn.execute(
            "UPDATE wait_queue
             SET status = 'reserved', reservation_expires_at = ?1
             WHERE wait_id = ?2 AND status = 'queued'",
            params![reservation_expires_at, wait_id],
        )?;

        let waiter = self.waiter(wait_id)?;
        if let Some(waiter) = &waiter {
            self.append_notification(
                &waiter.agent_id,
                &waiter.workspace_id,
                "reservation_granted",
                serde_json::json!({
                    "wait_id": waiter.wait_id,
                    "reservation_id": waiter.wait_id,
                    "relative_path": waiter.relative_path,
                    "action": waiter.action,
                    "reservation_expires_at": waiter.reservation_expires_at,
                    "purpose": waiter.purpose,
                }),
            )?;
        }

        Ok(waiter)
    }
}

fn required_intent_scopes(scopes: &serde_json::Value) -> StoreResult<Vec<ReservationScope>> {
    let scopes: Vec<ReservationScope> = serde_json::from_value(scopes.clone()).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })?;
    let scopes = scopes
        .into_iter()
        .map(normalize_intent_scope)
        .collect::<Vec<_>>();
    if scopes.is_empty() || scopes.iter().any(intent_scope_is_empty) {
        return Err(StoreError::MissingScope);
    }
    Ok(scopes)
}

fn normalize_intent_scope(scope: ReservationScope) -> ReservationScope {
    match scope {
        ReservationScope::File(path) => ReservationScope::file(path),
        ReservationScope::Directory(path) => ReservationScope::directory(path),
    }
}

fn intent_scope_is_empty(scope: &ReservationScope) -> bool {
    match scope {
        ReservationScope::File(path) | ReservationScope::Directory(path) => path.trim().is_empty(),
    }
}

fn direct_tmp_lease_path(relative_path: &str) -> bool {
    relative_path == "tmp"
}

fn required_purpose(purpose: &str) -> StoreResult<String> {
    let purpose = purpose.trim().to_string();
    if purpose.is_empty() {
        return Err(StoreError::MissingPurpose);
    }
    Ok(purpose)
}

fn intent_scope_resource(scope: &ReservationScope) -> String {
    match scope {
        ReservationScope::File(path) => path.clone(),
        ReservationScope::Directory(path) => format!("{}/", path.trim_end_matches('/')),
    }
}

fn resource_matches_filter(resource: &str, filter: Option<&str>) -> bool {
    let Some(filter) = filter else {
        return true;
    };
    let resource = normalize_relative_path(resource.trim_end_matches('/'));
    let filter = normalize_relative_path(filter.trim_end_matches('/'));
    resource == filter
        || resource
            .strip_prefix(&format!("{filter}/"))
            .is_some_and(|rest| !rest.is_empty())
        || filter
            .strip_prefix(&format!("{resource}/"))
            .is_some_and(|rest| !rest.is_empty())
}

fn identity_filter_matches(
    filter: CurrentStateIdentityFilter<'_>,
    repo_id: Option<&str>,
    worktree_id: Option<&str>,
    root: Option<&str>,
) -> bool {
    if filter.is_empty() {
        return true;
    }
    if let Some(expected) = filter.repo_id
        && repo_id != Some(expected)
    {
        return false;
    }
    if let Some(expected) = filter.worktree_id
        && worktree_id != Some(expected)
    {
        return false;
    }
    if let Some(expected) = filter.root
        && root != Some(expected)
    {
        return false;
    }
    true
}

fn prepare_private_database_path(path: &Path) -> StoreResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        restrict_directory_permissions(parent)?;
    }
    reject_linked_database_path(path)?;
    if !path.exists() {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
    }
    restrict_database_file_permissions(path)
}

fn reject_linked_database_path(path: &Path) -> StoreResult<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(StoreError::Io(
            std::io::Error::other("state database path must not be a symlink"),
        )),
        Ok(metadata) if database_metadata_is_hard_linked(&metadata) => Err(StoreError::Io(
            std::io::Error::other("state database path must not be hard-linked"),
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(StoreError::Io(error)),
    }
}

fn restrict_database_file_permissions(path: &Path) -> StoreResult<()> {
    restrict_file_permissions(path)?;
    restrict_file_permissions(&path.with_extension("db-wal"))?;
    restrict_file_permissions(&path.with_extension("db-shm"))?;
    restrict_file_permissions(&path.with_extension("wal"))?;
    restrict_file_permissions(&path.with_extension("shm"))?;
    Ok(())
}

#[cfg(unix)]
fn restrict_directory_permissions(path: &Path) -> StoreResult<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_directory_permissions(_path: &Path) -> StoreResult<()> {
    Ok(())
}

#[cfg(unix)]
fn restrict_file_permissions(path: &Path) -> StoreResult<()> {
    use std::os::unix::fs::PermissionsExt;
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(StoreError::Io(
            std::io::Error::other("state database file must not be a symlink"),
        )),
        Ok(metadata) if database_metadata_is_hard_linked(&metadata) => Err(StoreError::Io(
            std::io::Error::other("state database file must not be hard-linked"),
        )),
        Ok(metadata) if metadata.is_file() => {
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
            Ok(())
        }
        Ok(_) => Err(StoreError::Io(std::io::Error::other(
            "state database path must be a regular file",
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(StoreError::Io(error)),
    }
}

#[cfg(not(unix))]
fn restrict_file_permissions(path: &Path) -> StoreResult<()> {
    reject_linked_database_path(path)
}

#[cfg(unix)]
fn database_metadata_is_hard_linked(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    metadata.is_file() && metadata.nlink() > 1
}

#[cfg(not(unix))]
fn database_metadata_is_hard_linked(_metadata: &fs::Metadata) -> bool {
    false
}

fn configure_file_connection(conn: &Connection) -> StoreResult<()> {
    configure_connection(conn)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    Ok(())
}

fn configure_connection(conn: &Connection) -> StoreResult<()> {
    conn.busy_timeout(StdDuration::from_millis(SQLITE_BUSY_TIMEOUT_MS))?;
    Ok(())
}

fn now_timestamp() -> String {
    format_timestamp(OffsetDateTime::now_utc())
}

fn parse_activity_phase(phase: &str) -> Option<ActivityPhase> {
    match phase {
        "exploring" => Some(ActivityPhase::Exploring),
        "editing" => Some(ActivityPhase::Editing),
        "testing" => Some(ActivityPhase::Testing),
        "blocked" => Some(ActivityPhase::Blocked),
        "done" => Some(ActivityPhase::Done),
        "failed" => Some(ActivityPhase::Failed),
        _ => None,
    }
}

fn timestamp_after(timestamp: &str, seconds: i64) -> StoreResult<String> {
    let base = parse_timestamp(timestamp)
        .ok_or_else(|| StoreError::InvalidTimestamp(timestamp.to_string()))?;
    Ok(format_timestamp(base + Duration::seconds(seconds)))
}

fn parse_timestamp(timestamp: &str) -> Option<OffsetDateTime> {
    let timestamp = timestamp.strip_suffix('Z')?;
    let (date, time) = timestamp.split_once('T')?;
    let mut date_parts = date.split('-');
    let year = date_parts.next()?.parse::<i32>().ok()?;
    let month = date_parts.next()?.parse::<u8>().ok()?;
    let day = date_parts.next()?.parse::<u8>().ok()?;
    if date_parts.next().is_some() {
        return None;
    }

    let mut time_parts = time.split(':');
    let hour = time_parts.next()?.parse::<u8>().ok()?;
    let minute = time_parts.next()?.parse::<u8>().ok()?;
    let second_text = time_parts.next()?.split('.').next()?;
    let second = second_text.parse::<u8>().ok()?;
    if time_parts.next().is_some() {
        return None;
    }

    let month = Month::try_from(month).ok()?;
    let date = Date::from_calendar_date(year, month, day).ok()?;
    let time = Time::from_hms(hour, minute, second).ok()?;
    Some(date.with_time(time).assume_utc())
}

fn format_timestamp(timestamp: OffsetDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        timestamp.year(),
        u8::from(timestamp.month()),
        timestamp.day(),
        timestamp.hour(),
        timestamp.minute(),
        timestamp.second()
    )
}

fn wait_record_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WaitRecord> {
    if row.as_ref().column_count() == 10 {
        return Ok(WaitRecord {
            wait_id: row.get(0)?,
            agent_id: row.get(1)?,
            workspace_id: row.get(2)?,
            repo_id: None,
            worktree_id: None,
            root: None,
            branch: None,
            relative_path: row.get(3)?,
            action: row.get(4)?,
            status: row.get(5)?,
            requested_at: row.get(6)?,
            reservation_expires_at: row.get(7)?,
            blocking_agent_id: row.get(8)?,
            purpose: row.get(9)?,
        });
    }

    Ok(WaitRecord {
        wait_id: row.get(0)?,
        agent_id: row.get(1)?,
        workspace_id: row.get(2)?,
        repo_id: row.get(3)?,
        worktree_id: row.get(4)?,
        root: row.get(5)?,
        branch: row.get(6)?,
        relative_path: row.get(7)?,
        action: row.get(8)?,
        status: row.get(9)?,
        requested_at: row.get(10)?,
        reservation_expires_at: row.get(11)?,
        blocking_agent_id: row.get(12)?,
        purpose: row.get(13)?,
    })
}

fn notification_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<NotificationRecord> {
    let payload_json: String = row.get(5)?;
    let payload: serde_json::Value = serde_json::from_str(&payload_json).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Text, Box::new(err))
    })?;
    Ok(NotificationRecord {
        notification_id: row.get(0)?,
        sequence: row.get(1)?,
        target_agent_id: row.get(2)?,
        workspace_id: row.get(3)?,
        kind: row.get(4)?,
        payload,
        status: row.get(6)?,
        created_at: row.get(7)?,
        expires_at: row.get(8)?,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub event_id: String,
    pub event_type: EventType,
    pub agent_id: String,
    pub workspace_id: String,
    pub repo_id: Option<String>,
    pub worktree_id: Option<String>,
    pub root: Option<String>,
    pub branch: Option<String>,
    pub payload: serde_json::Value,
    pub created_at: String,
}

impl Event {
    pub fn agent_registered(agent_id: impl Into<String>, workspace_id: impl Into<String>) -> Self {
        let agent_id = agent_id.into();
        let workspace_id = workspace_id.into();

        Self {
            event_id: Uuid::new_v4().to_string(),
            event_type: EventType::AgentRegistered,
            payload: serde_json::json!({
                "agent_id": agent_id,
                "workspace_id": workspace_id,
            }),
            agent_id,
            workspace_id,
            repo_id: None,
            worktree_id: None,
            root: None,
            branch: None,
            created_at: now_timestamp(),
        }
    }

    pub fn agent_heartbeat(agent_id: impl Into<String>, workspace_id: impl Into<String>) -> Self {
        let agent_id = agent_id.into();
        let workspace_id = workspace_id.into();

        Self {
            event_id: Uuid::new_v4().to_string(),
            event_type: EventType::AgentHeartbeat,
            payload: serde_json::json!({
                "agent_id": agent_id,
                "workspace_id": workspace_id,
            }),
            agent_id,
            workspace_id,
            repo_id: None,
            worktree_id: None,
            root: None,
            branch: None,
            created_at: now_timestamp(),
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

    pub fn reservation_declared<I, S>(
        agent_id: impl Into<String>,
        workspace_id: impl Into<String>,
        purpose: impl Into<String>,
        files_planned: I,
    ) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let agent_id = agent_id.into();
        let workspace_id = workspace_id.into();
        let purpose = purpose.into().trim().to_string();
        assert!(
            !purpose.is_empty(),
            "ReservationDeclared event should include non-empty purpose"
        );
        let scopes = files_planned
            .into_iter()
            .map(|path| {
                let path = path.as_ref();
                if path.ends_with('/') {
                    ReservationScope::directory(path)
                } else {
                    ReservationScope::file(path)
                }
            })
            .collect::<Vec<_>>();

        Self {
            event_id: Uuid::new_v4().to_string(),
            event_type: EventType::ReservationDeclared,
            payload: serde_json::json!({
                "agent_id": agent_id,
                "workspace_id": workspace_id,
                "purpose": purpose,
                "scopes": scopes,
            }),
            agent_id,
            workspace_id,
            repo_id: None,
            worktree_id: None,
            root: None,
            branch: None,
            created_at: now_timestamp(),
        }
    }

    pub fn claim_acquired(
        agent_id: impl Into<String>,
        workspace_id: impl Into<String>,
        path: impl Into<String>,
    ) -> Self {
        let agent_id = agent_id.into();
        let workspace_id = workspace_id.into();
        let path = path.into();
        let action = if path.ends_with('/') {
            "write_directory"
        } else {
            "write_file"
        };

        Self {
            event_id: Uuid::new_v4().to_string(),
            event_type: EventType::ClaimAcquired,
            payload: serde_json::json!({
                "agent_id": agent_id,
                "workspace_id": workspace_id,
                "path": path,
                "action": action,
            }),
            agent_id,
            workspace_id,
            repo_id: None,
            worktree_id: None,
            root: None,
            branch: None,
            created_at: now_timestamp(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn reservation_requested(
        agent_id: impl Into<String>,
        workspace_id: impl Into<String>,
        request_id: impl Into<String>,
        action: impl Into<String>,
        relative_path: impl Into<String>,
        purpose: impl Into<String>,
        request_state: impl Into<String>,
        wait_id: Option<String>,
        queue_position: Option<u64>,
        blocking_agent_id: Option<String>,
    ) -> Self {
        let agent_id = agent_id.into();
        let workspace_id = workspace_id.into();
        let request_id = request_id.into();
        let action = action.into();
        let relative_path = relative_path.into();
        let purpose = purpose.into();
        let request_state = request_state.into();

        Self {
            event_id: Uuid::new_v4().to_string(),
            event_type: EventType::ReservationRequested,
            payload: serde_json::json!({
                "agent_id": agent_id,
                "workspace_id": workspace_id,
                "request_id": request_id,
                "action": action,
                "relative_path": relative_path,
                "purpose": purpose,
                "request_state": request_state,
                "wait_id": wait_id,
                "queue_position": queue_position,
                "blocking_agent_id": blocking_agent_id,
            }),
            agent_id,
            workspace_id,
            repo_id: None,
            worktree_id: None,
            root: None,
            branch: None,
            created_at: now_timestamp(),
        }
    }

    pub fn claim_released(
        agent_id: impl Into<String>,
        workspace_id: impl Into<String>,
        path: impl Into<String>,
        action: impl Into<String>,
    ) -> Self {
        let agent_id = agent_id.into();
        let workspace_id = workspace_id.into();
        let path = path.into();
        let action = action.into();

        Self {
            event_id: Uuid::new_v4().to_string(),
            event_type: EventType::ClaimReleased,
            payload: serde_json::json!({
                "agent_id": agent_id,
                "workspace_id": workspace_id,
                "path": path,
                "action": action,
            }),
            agent_id,
            workspace_id,
            repo_id: None,
            worktree_id: None,
            root: None,
            branch: None,
            created_at: now_timestamp(),
        }
    }

    pub fn reservation_claimed(
        agent_id: impl Into<String>,
        workspace_id: impl Into<String>,
        wait_id: impl Into<String>,
        action: impl Into<String>,
        relative_path: impl Into<String>,
        purpose: impl Into<String>,
    ) -> Self {
        let agent_id = agent_id.into();
        let workspace_id = workspace_id.into();
        let wait_id = wait_id.into();
        let action = action.into();
        let relative_path = relative_path.into();
        let purpose = purpose.into();

        Self {
            event_id: Uuid::new_v4().to_string(),
            event_type: EventType::ReservationClaimed,
            payload: serde_json::json!({
                "agent_id": agent_id,
                "workspace_id": workspace_id,
                "wait_id": wait_id,
                "action": action,
                "relative_path": relative_path,
                "purpose": purpose,
                "request_state": "claimed",
            }),
            agent_id,
            workspace_id,
            repo_id: None,
            worktree_id: None,
            root: None,
            branch: None,
            created_at: now_timestamp(),
        }
    }

    pub fn reservation_canceled(
        agent_id: impl Into<String>,
        workspace_id: impl Into<String>,
        request_id: impl Into<String>,
        wait_id: impl Into<String>,
        action: impl Into<String>,
        relative_path: impl Into<String>,
        purpose: impl Into<String>,
    ) -> Self {
        let agent_id = agent_id.into();
        let workspace_id = workspace_id.into();
        let request_id = request_id.into();
        let wait_id = wait_id.into();
        let action = action.into();
        let relative_path = relative_path.into();
        let purpose = purpose.into();

        Self {
            event_id: Uuid::new_v4().to_string(),
            event_type: EventType::ReservationCanceled,
            payload: serde_json::json!({
                "agent_id": agent_id,
                "workspace_id": workspace_id,
                "request_id": request_id,
                "wait_id": wait_id,
                "action": action,
                "relative_path": relative_path,
                "purpose": purpose,
                "request_state": "canceled",
            }),
            agent_id,
            workspace_id,
            repo_id: None,
            worktree_id: None,
            root: None,
            branch: None,
            created_at: now_timestamp(),
        }
    }

    pub fn activity_finalized(
        agent_id: impl Into<String>,
        workspace_id: impl Into<String>,
        released_claims: u64,
        completed_reservations: u64,
    ) -> Self {
        let agent_id = agent_id.into();
        let workspace_id = workspace_id.into();

        Self {
            event_id: Uuid::new_v4().to_string(),
            event_type: EventType::ActivityFinalized,
            payload: serde_json::json!({
                "agent_id": agent_id,
                "workspace_id": workspace_id,
                "released_claims": released_claims,
                "completed_reservations": completed_reservations,
            }),
            agent_id,
            workspace_id,
            repo_id: None,
            worktree_id: None,
            root: None,
            branch: None,
            created_at: now_timestamp(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn authorization_denied(
        agent_id: impl Into<String>,
        workspace_id: impl Into<String>,
        action: impl Into<String>,
        path: impl Into<String>,
        old_path: Option<String>,
        new_path: Option<String>,
        reason_code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        let agent_id = agent_id.into();
        let workspace_id = workspace_id.into();
        let action = action.into();
        let path = path.into();
        let reason_code = reason_code.into();
        let message = message.into();

        Self {
            event_id: Uuid::new_v4().to_string(),
            event_type: EventType::AuthorizationDenied,
            payload: serde_json::json!({
                "agent_id": agent_id,
                "workspace_id": workspace_id,
                "action": action,
                "path": path,
                "old_path": old_path,
                "new_path": new_path,
                "reason_code": reason_code,
                "message": message,
            }),
            agent_id,
            workspace_id,
            repo_id: None,
            worktree_id: None,
            root: None,
            branch: None,
            created_at: now_timestamp(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventType {
    AgentRegistered,
    AgentHeartbeat,
    ReservationDeclared,
    ClaimAcquired,
    ClaimReleased,
    ReservationRequested,
    ReservationClaimed,
    ReservationCanceled,
    ActivityFinalized,
    AuthorizationDenied,
}

impl EventType {
    fn as_str(self) -> &'static str {
        match self {
            Self::AgentRegistered => "AgentRegistered",
            Self::AgentHeartbeat => "AgentHeartbeat",
            Self::ReservationDeclared => "ReservationDeclared",
            Self::ClaimAcquired => "ClaimAcquired",
            Self::ClaimReleased => "ClaimReleased",
            Self::ReservationRequested => "ReservationRequested",
            Self::ReservationClaimed => "ReservationClaimed",
            Self::ReservationCanceled => "ReservationCanceled",
            Self::ActivityFinalized => "ActivityFinalized",
            Self::AuthorizationDenied => "AuthorizationDenied",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord {
    pub agent_id: String,
    pub workspace_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CurrentSummary {
    pub agent_count: u64,
    pub active_reservation_count: u64,
    pub event_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LiveCurrentState {
    pub summary: CurrentSummary,
    pub items: Vec<CurrentItem>,
}

#[derive(Debug, Clone, Copy)]
pub struct ReservationRequestInput<'a> {
    pub request_id: &'a str,
    pub agent_id: &'a str,
    pub workspace_id: &'a str,
    pub relative_path: &'a str,
    pub action: &'a str,
    pub purpose: &'a str,
    pub blocking_agent_id: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EventRecord {
    pub event_id: String,
    pub event_type: String,
    pub agent_id: String,
    pub workspace_id: String,
    pub repo_id: Option<String>,
    pub worktree_id: Option<String>,
    pub root: Option<String>,
    pub branch: Option<String>,
    pub payload: serde_json::Value,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WaitRecord {
    pub wait_id: String,
    pub agent_id: String,
    pub workspace_id: String,
    pub repo_id: Option<String>,
    pub worktree_id: Option<String>,
    pub root: Option<String>,
    pub branch: Option<String>,
    pub relative_path: String,
    pub action: String,
    pub status: String,
    pub requested_at: String,
    pub reservation_expires_at: Option<String>,
    pub blocking_agent_id: Option<String>,
    pub purpose: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NotificationRecord {
    pub notification_id: String,
    pub sequence: u64,
    pub target_agent_id: String,
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
    pub agent_id: String,
    pub workspace_id: String,
    pub sequence: u64,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub sync_status: SyncStatus,
}

impl OutboxEntry {
    pub fn new(outbox_id: impl Into<String>, agent_id: impl Into<String>, sequence: u64) -> Self {
        Self {
            outbox_id: outbox_id.into(),
            agent_id: agent_id.into(),
            workspace_id: String::new(),
            sequence,
            event_type: String::new(),
            payload: serde_json::json!({}),
            sync_status: SyncStatus::Pending,
        }
    }

    pub fn synced(
        outbox_id: impl Into<String>,
        agent_id: impl Into<String>,
        sequence: u64,
    ) -> Self {
        Self {
            sync_status: SyncStatus::Synced,
            ..Self::new(outbox_id, agent_id, sequence)
        }
    }

    pub fn with_workspace_id(mut self, workspace_id: impl Into<String>) -> Self {
        self.workspace_id = workspace_id.into();
        self
    }

    pub fn with_event_type(mut self, event_type: impl Into<String>) -> Self {
        self.event_type = event_type.into();
        self
    }

    pub fn with_payload(mut self, payload: serde_json::Value) -> Self {
        self.payload = payload;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncStatus {
    Pending,
    Synced,
}

impl SyncStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Synced => "synced",
        }
    }

    fn from_str(value: &str) -> Self {
        match value {
            "synced" => Self::Synced,
            _ => Self::Pending,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxRecord {
    pub outbox_id: String,
    pub agent_id: String,
    pub workspace_id: String,
    pub sequence: u64,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub sync_status: SyncStatus,
}
