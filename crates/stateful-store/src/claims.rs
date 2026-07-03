use super::*;

impl Store {
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

    pub(crate) fn acquire_claim_with_observation_and_event_inner(
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
        let agent_id = agent_id.as_ref().to_string();
        let workspace_id = workspace_id.as_ref().to_string();
        let requested_relative_path = relative_path.as_ref();
        let lease_action = if requested_relative_path.ends_with("/") {
            "write_directory"
        } else {
            "write_file"
        };
        let relative_path = normalize_relative_path(requested_relative_path);
        self.store_transaction(move |store| {
            store.expire_stale()?;
            let released = store.conn.execute(
                "UPDATE claims
                 SET status = 'released'
                 WHERE agent_id = ?1 AND workspace_id = ?2 AND relative_path = ?3 AND action = ?4 AND status = 'active'",
                params![&agent_id, &workspace_id, &relative_path, lease_action],
            )?;
            if released == 0 {
                let owner_exists = store.conn.query_row(
                    "SELECT EXISTS(
                            SELECT 1 FROM claims
                            WHERE agent_id != ?1
                              AND workspace_id = ?2
                              AND relative_path = ?3
                              AND action = ?4
                              AND status = 'active'
                        )",
                    params![&agent_id, &workspace_id, &relative_path, lease_action],
                    |row| row.get::<_, bool>(0),
                )?;
                if owner_exists {
                    return Err(StoreError::ClaimOwnerMismatch);
                }
                return Err(StoreError::ClaimNotFound);
            }
            store.promote_waiters_after_lease_release(&workspace_id, &relative_path)?;
            store.append_inner(&Event::claim_released(
                agent_id,
                workspace_id,
                relative_path,
                lease_action,
            ))
        })
    }

    pub fn release_session_claims(
        &self,
        agent_id: impl AsRef<str>,
        workspace_id: impl AsRef<str>,
    ) -> StoreResult<u64> {
        let agent_id = agent_id.as_ref().to_string();
        let workspace_id = workspace_id.as_ref().to_string();
        self.store_transaction(move |store| {
            store.release_session_claims_inner(&agent_id, &workspace_id)
        })
    }

    pub(crate) fn release_session_claims_inner(
        &self,
        agent_id: &str,
        workspace_id: &str,
    ) -> StoreResult<u64> {
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
        let agent_id = agent_id.as_ref().to_string();
        let workspace_id = workspace_id.as_ref().to_string();
        let relative_path = normalize_relative_path(relative_path.as_ref());
        self.store_transaction(move |store| {
            store.expire_stale()?;
            store.refresh_exact_file_claim_observation_inner(
                &agent_id,
                &workspace_id,
                &relative_path,
                &observation,
            )
        })
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

    pub(crate) fn complete_session_reservations_inner(
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
}
