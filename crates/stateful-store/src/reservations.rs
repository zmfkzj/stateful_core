use super::*;

impl Store {
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

    pub(crate) fn cancel_session_waiters_inner(
        &self,
        agent_id: &str,
        workspace_id: &str,
    ) -> StoreResult<u64> {
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
}
