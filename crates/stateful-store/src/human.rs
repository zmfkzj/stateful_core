use super::*;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HumanObservationKind {
    Save,
    Change,
    Delete,
    Presence,
    Dirty,
}

impl HumanObservationKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Save => "save",
            Self::Change => "change",
            Self::Delete => "delete",
            Self::Presence => "presence",
            Self::Dirty => "dirty",
        }
    }

    fn is_write(self) -> bool {
        matches!(self, Self::Save | Self::Change | Self::Delete)
    }
}

impl FromStr for HumanObservationKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "save" => Ok(Self::Save),
            "change" => Ok(Self::Change),
            "delete" => Ok(Self::Delete),
            "presence" => Ok(Self::Presence),
            "dirty" => Ok(Self::Dirty),
            other => Err(format!("unknown human observation kind: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HumanObservationConfidence {
    High,
    Low,
}

impl HumanObservationConfidence {
    fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Low => "low",
        }
    }
}

impl FromStr for HumanObservationConfidence {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "high" => Ok(Self::High),
            "low" => Ok(Self::Low),
            other => Err(format!("unknown human observation confidence: {other}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HumanObservationInput {
    pub workspace_id: String,
    pub relative_path: String,
    pub kind: HumanObservationKind,
    pub confidence: HumanObservationConfidence,
    pub source: String,
    pub observed_at: String,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconciliationAckInput {
    pub agent_id: String,
    pub workspace_id: String,
    pub reservation_id: Option<String>,
    pub decision: ReconciliationDecision,
    pub files_reread: Vec<String>,
    pub human_change_summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HumanObservationRecord {
    pub observation_id: String,
    pub workspace_id: String,
    pub relative_path: String,
    pub kind: HumanObservationKind,
    pub confidence: HumanObservationConfidence,
    pub source: String,
    pub observed_at: String,
    pub summary: String,
    pub reconciled_at: Option<String>,
}

impl Store {
    pub fn record_human_observation(&self, input: HumanObservationInput) -> StoreResult<String> {
        self.store_transaction(move |store| store.record_human_observation_inner(input))
    }

    fn record_human_observation_inner(&self, input: HumanObservationInput) -> StoreResult<String> {
        let observation_id = Uuid::new_v4().to_string();
        let relative_path = normalize_relative_path(&input.relative_path);
        let is_write =
            input.kind.is_write() && input.confidence == HumanObservationConfidence::High;
        let attributed_to_agent = is_write
            && self
                .write_fence_owner_for_observation(
                    &input.workspace_id,
                    &relative_path,
                    &input.observed_at,
                )?
                .is_some();
        let reconciled_at = attributed_to_agent.then_some(input.observed_at.clone());
        let expires_at = if matches!(
            input.kind,
            HumanObservationKind::Presence | HumanObservationKind::Dirty
        ) {
            Some(timestamp_after(
                &input.observed_at,
                WRITE_FENCE_TTL_SECONDS,
            )?)
        } else {
            None
        };

        self.conn.execute(
            "INSERT INTO human_observations (
                observation_id, workspace_id, relative_path, kind, source, confidence,
                observed_exists, observed_content_hash, observed_at, summary, expires_at,
                reconciled_at, reconcile_decision, reconciled_by_agent_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, NULL, ?7, ?8, ?9, ?10, NULL, NULL)",
            params![
                observation_id,
                input.workspace_id,
                relative_path,
                input.kind.as_str(),
                input.source,
                input.confidence.as_str(),
                input.observed_at,
                input.summary,
                expires_at,
                reconciled_at
            ],
        )?;

        if is_write && !attributed_to_agent {
            self.append_inner(&Event::human_write_observed(
                input.workspace_id,
                relative_path,
                input.kind.as_str(),
                input.source,
                input.summary,
            ))?;
        }

        Ok(observation_id)
    }

    pub fn unreconciled_human_observations(
        &self,
        workspace_id: impl AsRef<str>,
        paths: &[String],
    ) -> StoreResult<Vec<HumanObservationRecord>> {
        let paths = normalize_paths(paths);
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        self.unreconciled_human_observations_inner(workspace_id.as_ref(), &paths)
    }

    fn unreconciled_human_observations_inner(
        &self,
        workspace_id: &str,
        paths: &[String],
    ) -> StoreResult<Vec<HumanObservationRecord>> {
        let mut records = Vec::new();
        let mut statement = self.conn.prepare(
            "SELECT observation_id, workspace_id, relative_path, kind, confidence, source,
                    observed_at, summary, reconciled_at
             FROM human_observations
             WHERE workspace_id = ?1
               AND reconciled_at IS NULL
               AND kind IN ('save', 'change', 'delete')
               AND confidence = 'high'
             ORDER BY observed_at",
        )?;
        let rows = statement.query_map([workspace_id], human_observation_record_from_row)?;
        for row in rows {
            let record = row?;
            if paths.iter().any(|path| path == &record.relative_path) {
                records.push(record);
            }
        }
        Ok(records)
    }

    pub fn unreconciled_human_write_paths(
        &self,
        workspace_id: impl AsRef<str>,
        paths: &[String],
    ) -> StoreResult<Vec<String>> {
        let mut paths = self
            .unreconciled_human_observations(workspace_id, paths)?
            .into_iter()
            .map(|record| record.relative_path)
            .collect::<Vec<_>>();
        paths.sort();
        paths.dedup();
        Ok(paths)
    }

    pub fn acknowledge_human_reconciliation(
        &self,
        input: ReconciliationAckInput,
    ) -> StoreResult<u64> {
        self.store_transaction(move |store| store.acknowledge_human_reconciliation_inner(input))
    }

    fn acknowledge_human_reconciliation_inner(
        &self,
        input: ReconciliationAckInput,
    ) -> StoreResult<u64> {
        let now = now_timestamp();
        let files = normalize_paths(&input.files_reread);
        self.append_inner(&Event::reconciliation_acknowledged(
            input.agent_id.clone(),
            input.workspace_id.clone(),
            input.decision,
            files.clone(),
            input.human_change_summary,
        ))?;

        if !input.decision.clears_human_write_block() || files.is_empty() {
            return Ok(0);
        }

        let mut updated = 0;
        for path in files {
            updated += self.conn.execute(
                "UPDATE human_observations
                 SET reconciled_at = ?1,
                     reconcile_decision = ?2,
                     reconciled_by_agent_id = ?3
                 WHERE workspace_id = ?4
                   AND relative_path = ?5
                   AND reconciled_at IS NULL
                   AND kind IN ('save', 'change', 'delete')",
                params![
                    now,
                    reconciliation_decision_as_str(input.decision),
                    input.agent_id,
                    input.workspace_id,
                    path
                ],
            )? as u64;
        }
        Ok(updated)
    }

    pub(crate) fn live_human_observation_items(
        &self,
        workspace_filter: Option<&str>,
        identity_filter: CurrentStateIdentityFilter<'_>,
        resource_filter: Option<&str>,
    ) -> StoreResult<Vec<CurrentItem>> {
        let mut statement = self.conn.prepare(
            "SELECT observation_id, workspace_id, relative_path, kind, confidence, source,
                    observed_at, summary, reconciled_at
             FROM human_observations
             WHERE reconciled_at IS NULL
               AND kind IN ('save', 'change', 'delete')
               AND confidence = 'high'
             ORDER BY observed_at",
        )?;
        let rows = statement.query_map([], human_observation_record_from_row)?;
        let mut items = Vec::new();
        for row in rows {
            let record = row?;
            if workspace_filter.is_some_and(|workspace| workspace != record.workspace_id) {
                continue;
            }
            if identity_filter.exclude_agent_id.is_some() {
                // Human rows are not owned by an agent; keep them visible to every agent.
            }
            if !resource_matches_filter(&record.relative_path, resource_filter) {
                continue;
            }
            items.push(
                CurrentItem::new(
                    CurrentItemKind::Claim,
                    CurrentSeverity::Block,
                    CurrentFreshness::Live,
                    record.relative_path.clone(),
                    "Reconcile a human write before resuming agent edits.",
                    format!("{} has an unreconciled human write.", record.relative_path),
                )
                .with_workspace(record.workspace_id)
                .with_next_action(format!(
                    "Reread {}, summarize the human change, then call state.reconcile.ack.",
                    record.relative_path
                ))
                .with_evidence(record.summary)
                .with_evidence_kind(CurrentEvidenceKind::ObservedWrite)
                .with_source_ref("HumanWriteObserved")
                .with_observed_at(record.observed_at),
            );
        }
        Ok(items)
    }

    pub(crate) fn expire_stale_human_observations_inner(&self, now: &str) -> StoreResult<()> {
        self.conn.execute(
            "DELETE FROM human_observations
             WHERE kind IN ('presence', 'dirty')
               AND expires_at IS NOT NULL
               AND expires_at <= ?1",
            [now],
        )?;
        Ok(())
    }
}

fn normalize_paths(paths: &[String]) -> Vec<String> {
    let mut paths = paths
        .iter()
        .map(normalize_relative_path)
        .filter(|path| !path.is_empty())
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    paths
}

fn human_observation_record_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<HumanObservationRecord> {
    let kind = HumanObservationKind::from_str(&row.get::<_, String>(3)?).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            3,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::other(error)),
        )
    })?;
    let confidence =
        HumanObservationConfidence::from_str(&row.get::<_, String>(4)?).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                4,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::other(error)),
            )
        })?;
    Ok(HumanObservationRecord {
        observation_id: row.get(0)?,
        workspace_id: row.get(1)?,
        relative_path: row.get(2)?,
        kind,
        confidence,
        source: row.get(5)?,
        observed_at: row.get(6)?,
        summary: row.get(7)?,
        reconciled_at: row.get(8)?,
    })
}

fn reconciliation_decision_as_str(decision: ReconciliationDecision) -> &'static str {
    match decision {
        ReconciliationDecision::Adopt => "adopt",
        ReconciliationDecision::Reapply => "reapply",
        ReconciliationDecision::AskUser => "ask_user",
        ReconciliationDecision::Abandon => "abandon",
    }
}
