use super::*;

impl Store {
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

    pub(crate) fn append_activity_inner(
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
        self.release_session_write_fences_inner(agent_id, workspace_id)?;
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
}
