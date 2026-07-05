use super::*;
use std::collections::HashSet;

const SCOPE_OVERLAP_DEDUP_SECONDS: i64 = 120;

impl Store {
    /// On a reservation declaration, notify other agents whose active
    /// reservation scopes overlap any newly declared resource. Advisory only.
    pub(crate) fn notify_scope_overlap_for_declaration(
        &self,
        workspace_id: &str,
        by_agent_id: &str,
        purpose: &str,
        actor_resources: &[(String, bool)],
    ) -> StoreResult<()> {
        if actor_resources.is_empty() {
            return Ok(());
        }

        let candidates = self.scope_overlap_candidates(workspace_id, by_agent_id)?;
        if candidates.is_empty() {
            return Ok(());
        }

        let now = now_timestamp();
        let since = timestamp_after(&now, -SCOPE_OVERLAP_DEDUP_SECONDS)?;
        let mut emitted = HashSet::new();

        for (holder_agent_id, holder_path, holder_is_dir) in candidates {
            for (actor_path, actor_is_dir) in actor_resources {
                if !resource_paths_overlap(actor_path, *actor_is_dir, &holder_path, holder_is_dir) {
                    continue;
                }

                if !emitted.insert((holder_agent_id.clone(), actor_path.clone())) {
                    continue;
                }

                if self.recent_scope_overlap_exists(
                    &holder_agent_id,
                    workspace_id,
                    by_agent_id,
                    actor_path,
                    &since,
                )? {
                    continue;
                }

                let payload = serde_json::json!({
                    "relative_path": actor_path,
                    "action": if *actor_is_dir { "write_directory" } else { "write_file" },
                    "by_agent_id": by_agent_id,
                    "purpose": purpose,
                    "source": "reservation_declared",
                    "overlaps_your": holder_path,
                });
                self.append_notification(&holder_agent_id, workspace_id, "scope_overlap", payload)?;
            }
        }

        Ok(())
    }

    fn scope_overlap_candidates(
        &self,
        workspace_id: &str,
        by_agent_id: &str,
    ) -> StoreResult<Vec<(String, String, bool)>> {
        let mut statement = self.conn.prepare(
            "SELECT agent_id, scopes_json
             FROM reservations
             WHERE workspace_id = ?1 AND status = 'active' AND agent_id != ?2",
        )?;
        let rows = statement
            .query_map(params![workspace_id, by_agent_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut candidates = Vec::new();
        for (agent_id, scopes_json) in rows {
            let scopes: Vec<ReservationScope> = serde_json::from_str(&scopes_json).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    1,
                    rusqlite::types::Type::Text,
                    Box::new(err),
                )
            })?;
            for scope in scopes {
                let (path, is_dir) = scope_resource_and_kind(&scope);
                candidates.push((agent_id.clone(), path, is_dir));
            }
        }

        Ok(candidates)
    }

    fn recent_scope_overlap_exists(
        &self,
        target_agent_id: &str,
        workspace_id: &str,
        by_agent_id: &str,
        relative_path: &str,
        since: &str,
    ) -> StoreResult<bool> {
        let mut statement = self.conn.prepare(
            "SELECT payload_json
             FROM notifications
             WHERE target_agent_id = ?1
               AND workspace_id = ?2
               AND kind = 'scope_overlap'
               AND created_at >= ?3",
        )?;
        let rows = statement.query_map(params![target_agent_id, workspace_id, since], |row| {
            row.get::<_, String>(0)
        })?;

        for row in rows {
            let payload_json = row?;
            let payload: serde_json::Value =
                serde_json::from_str(&payload_json).unwrap_or(serde_json::Value::Null);
            if payload.get("by_agent_id").and_then(|value| value.as_str()) == Some(by_agent_id)
                && payload
                    .get("relative_path")
                    .and_then(|value| value.as_str())
                    == Some(relative_path)
            {
                return Ok(true);
            }
        }

        Ok(false)
    }
}

fn scope_resource_and_kind(scope: &ReservationScope) -> (String, bool) {
    match scope {
        ReservationScope::File(path) => (normalize_relative_path(path), false),
        ReservationScope::Directory(path) => (
            normalize_relative_path(path.trim_end_matches('/')),
            true,
        ),
    }
}

fn resource_paths_overlap(a: &str, a_dir: bool, b: &str, b_dir: bool) -> bool {
    a == b || (a_dir && path_is_under(b, a)) || (b_dir && path_is_under(a, b))
}

fn path_is_under(path: &str, directory: &str) -> bool {
    path.strip_prefix(directory)
        .is_some_and(|rest| rest.starts_with('/'))
}
