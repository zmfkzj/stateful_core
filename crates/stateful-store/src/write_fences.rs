use super::*;

impl Store {
    pub fn acquire_write_fences(
        &self,
        agent_id: impl AsRef<str>,
        workspace_id: impl AsRef<str>,
        paths: &[String],
        action: impl AsRef<str>,
    ) -> StoreResult<()> {
        let agent_id = agent_id.as_ref().to_string();
        let workspace_id = workspace_id.as_ref().to_string();
        let paths = paths.to_vec();
        let action = action.as_ref().to_string();
        self.store_transaction(move |store| {
            store.acquire_write_fences_inner(&agent_id, &workspace_id, &paths, &action)
        })
    }

    fn acquire_write_fences_inner(
        &self,
        agent_id: &str,
        workspace_id: &str,
        paths: &[String],
        action: &str,
    ) -> StoreResult<()> {
        self.expire_stale_write_fences_inner()?;
        let paths: Vec<String> = paths
            .iter()
            .map(normalize_relative_path)
            .filter(|path| !path.is_empty())
            .collect();
        let now = now_timestamp();
        let expires_at = timestamp_after(&now, WRITE_FENCE_TTL_SECONDS)?;

        for path in &paths {
            if let Some(owner_agent_id) =
                self.active_write_fence_owner_except(workspace_id, path, agent_id)?
            {
                return Err(StoreError::WriteFenceConflict {
                    path: path.clone(),
                    owner_agent_id,
                });
            }
        }

        for path in paths {
            let updated = self.conn.execute(
                "UPDATE write_fences
                 SET expires_at = ?1, action = ?2
                 WHERE agent_id = ?3
                   AND workspace_id = ?4
                   AND relative_path = ?5
                   AND released_at IS NULL
                   AND expires_at > ?6",
                params![expires_at, action, agent_id, workspace_id, path, now],
            )?;
            if updated == 0 {
                self.conn.execute(
                    "INSERT INTO write_fences (
                        fence_id, agent_id, workspace_id, relative_path, action,
                        acquired_at, expires_at, released_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL)",
                    params![
                        Uuid::new_v4().to_string(),
                        agent_id,
                        workspace_id,
                        path,
                        action,
                        now,
                        expires_at
                    ],
                )?;
            }
        }
        Ok(())
    }

    pub fn active_write_fence_owner(
        &self,
        workspace_id: impl AsRef<str>,
        path: impl AsRef<str>,
    ) -> StoreResult<Option<String>> {
        self.expire_stale_write_fences_inner()?;
        self.active_write_fence_owner_inner(
            workspace_id.as_ref(),
            &normalize_relative_path(path.as_ref()),
        )
    }

    fn active_write_fence_owner_except(
        &self,
        workspace_id: &str,
        path: &str,
        except_agent_id: &str,
    ) -> StoreResult<Option<String>> {
        let now = now_timestamp();
        self.conn
            .query_row(
                "SELECT agent_id
                 FROM write_fences
                 WHERE workspace_id = ?1
                   AND relative_path = ?2
                   AND agent_id <> ?3
                   AND released_at IS NULL
                   AND expires_at > ?4
                 ORDER BY acquired_at
                 LIMIT 1",
                params![workspace_id, path, except_agent_id, now],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(StoreError::from)
    }

    fn active_write_fence_owner_inner(
        &self,
        workspace_id: &str,
        path: &str,
    ) -> StoreResult<Option<String>> {
        let now = now_timestamp();
        self.conn
            .query_row(
                "SELECT agent_id
                 FROM write_fences
                 WHERE workspace_id = ?1
                   AND relative_path = ?2
                   AND released_at IS NULL
                   AND expires_at > ?3
                 ORDER BY acquired_at
                 LIMIT 1",
                params![workspace_id, path, now],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn write_fence_owner_for_observation(
        &self,
        workspace_id: impl AsRef<str>,
        path: impl AsRef<str>,
        observed_at: impl AsRef<str>,
    ) -> StoreResult<Option<String>> {
        let workspace_id = workspace_id.as_ref();
        let path = normalize_relative_path(path.as_ref());
        let observed_at = observed_at.as_ref();
        let grace_start = timestamp_after(observed_at, -WRITE_FENCE_RELEASE_GRACE_SECONDS)?;
        self.conn
            .query_row(
                "SELECT agent_id
                 FROM write_fences
                 WHERE workspace_id = ?1
                   AND relative_path = ?2
                   AND acquired_at <= ?3
                   AND expires_at >= ?3
                   AND (released_at IS NULL OR released_at >= ?4)
                 ORDER BY acquired_at DESC
                 LIMIT 1",
                params![workspace_id, path, observed_at, grace_start],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn release_write_fences(
        &self,
        agent_id: impl AsRef<str>,
        workspace_id: impl AsRef<str>,
        path: impl AsRef<str>,
    ) -> StoreResult<u64> {
        let agent_id = agent_id.as_ref().to_string();
        let workspace_id = workspace_id.as_ref().to_string();
        let path = normalize_relative_path(path.as_ref());
        self.store_transaction(move |store| {
            store.release_write_fences_inner(&agent_id, &workspace_id, &path)
        })
    }

    pub(crate) fn release_write_fences_inner(
        &self,
        agent_id: &str,
        workspace_id: &str,
        path: &str,
    ) -> StoreResult<u64> {
        let released_at = now_timestamp();
        Ok(self.conn.execute(
            "UPDATE write_fences
             SET released_at = ?1
             WHERE agent_id = ?2
               AND workspace_id = ?3
               AND relative_path = ?4
               AND released_at IS NULL",
            params![released_at, agent_id, workspace_id, path],
        )? as u64)
    }

    pub fn release_session_write_fences(
        &self,
        agent_id: impl AsRef<str>,
        workspace_id: impl AsRef<str>,
    ) -> StoreResult<u64> {
        let agent_id = agent_id.as_ref().to_string();
        let workspace_id = workspace_id.as_ref().to_string();
        self.store_transaction(move |store| {
            store.release_session_write_fences_inner(&agent_id, &workspace_id)
        })
    }

    pub(crate) fn release_session_write_fences_inner(
        &self,
        agent_id: &str,
        workspace_id: &str,
    ) -> StoreResult<u64> {
        let released_at = now_timestamp();
        Ok(self.conn.execute(
            "UPDATE write_fences
             SET released_at = ?1
             WHERE agent_id = ?2
               AND workspace_id = ?3
               AND released_at IS NULL",
            params![released_at, agent_id, workspace_id],
        )? as u64)
    }

    pub fn fence_windows_for_path(
        &self,
        workspace_id: impl AsRef<str>,
        path: impl AsRef<str>,
        since: impl AsRef<str>,
    ) -> StoreResult<Vec<(String, String, Option<String>)>> {
        let workspace_id = workspace_id.as_ref();
        let path = normalize_relative_path(path.as_ref());
        let since = since.as_ref();
        let mut statement = self.conn.prepare(
            "SELECT agent_id, acquired_at, released_at
             FROM write_fences
             WHERE workspace_id = ?1
               AND relative_path = ?2
               AND acquired_at >= ?3
             ORDER BY acquired_at",
        )?;
        Ok(statement
            .query_map(params![workspace_id, path, since], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub(crate) fn live_write_fence_items(
        &self,
        workspace_filter: Option<&str>,
        identity_filter: CurrentStateIdentityFilter<'_>,
        resource_filter: Option<&str>,
    ) -> StoreResult<Vec<CurrentItem>> {
        let now = now_timestamp();
        let mut statement = self.conn.prepare(
            "SELECT agent_id, workspace_id, relative_path, action, acquired_at, expires_at
             FROM write_fences
             WHERE released_at IS NULL
               AND expires_at > ?1
             ORDER BY relative_path, acquired_at",
        )?;
        let rows = statement.query_map([&now], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;

        let mut items = Vec::new();
        for row in rows {
            let (agent_id, workspace_id, relative_path, action, acquired_at, expires_at) = row?;
            if workspace_filter.is_some_and(|filter| filter != workspace_id) {
                continue;
            }
            if identity_filter.exclude_agent_id == Some(agent_id.as_str()) {
                continue;
            }
            if !resource_matches_filter(&relative_path, resource_filter) {
                continue;
            }

            items.push(
                CurrentItem::new(
                    CurrentItemKind::Claim,
                    CurrentSeverity::Warn,
                    CurrentFreshness::Live,
                    relative_path.clone(),
                    action.clone(),
                    format!("write in flight by {agent_id}"),
                )
                .with_next_action(format!(
                    "Wait for the in-flight write on {relative_path} by {agent_id} to finish before writing."
                ))
                .with_agent(agent_id)
                .with_workspace(workspace_id)
                .with_evidence_kind(CurrentEvidenceKind::ClaimOnly)
                .with_source_ref(AGENT_CONTEXT_SCOPE_SOURCE_REF)
                .with_observed_at(acquired_at)
                .with_expires_at(Some(expires_at)),
            );
        }
        Ok(items)
    }

    fn expire_stale_write_fences_inner(&self) -> StoreResult<()> {
        let now = now_timestamp();
        self.conn.execute(
            "UPDATE write_fences
             SET released_at = expires_at
             WHERE released_at IS NULL
               AND expires_at <= ?1",
            [&now],
        )?;
        Ok(())
    }
}
