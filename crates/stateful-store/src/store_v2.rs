use std::collections::BTreeSet;

use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use stateful_core::{
    CoordinationSettings, DirectoryTreeState, EntryState, MutationOperation, ObjectState,
    ResourceKey, ResourceKind, ResourceObservation, TaskStatus, digest_canonical_json,
    resource_keys_overlap, validate_operation_start, validate_operation_transition,
};
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

use crate::{
    AuditRecord, CommandContext, LeaseActivateInput, LeaseActivateResult, LeaseReleaseInput,
    LeaseReleaseResult, LeaseReleaseStatus, LeaseRequestState, LeaseRequestStatus,
    ReadCommandResult, ReadCompleteInput, ReadResultStatus, ReadStartInput, StatusSnapshot, Store,
    StoreError, StoreResult, TaskCommandResult, TaskEndInput, TaskHeartbeatInput, TaskStartInput,
    WriteCompleteInput, WriteCompleteResult, WritePrepareInput, WritePrepareResult,
    WriteResultStatus, WriteTerminal,
};

const CONTRACT_REVISION: &str = "lease-1";
const AUDIT_RETENTION_DAYS: i64 = 14;

impl Store {
    pub fn task_start(
        &mut self,
        context: &CommandContext,
        input: &TaskStartInput,
    ) -> StoreResult<TaskCommandResult> {
        self.accepted_command(context, "task.start", input, |transaction| {
            validate_context(context)?;
            if input.next_action.trim().is_empty() {
                return Err(StoreError::InvalidInput(
                    "next_action is required".to_string(),
                ));
            }
            input
                .settings
                .validate()
                .map_err(StoreError::InvalidInput)?;
            validate_future_window(
                &context.observed_at,
                &input.expires_at,
                setting_seconds(input.settings.inactivity_timeout_seconds)?,
                "task expires_at",
            )?;
            if let Some((agent_id, status)) = task_row(transaction, context)? {
                if agent_id != context.agent_id {
                    return Err(StoreError::Ownership("task owner mismatch".to_string()));
                }
                return Ok(TaskCommandResult {
                    task_id: context.task_id.clone(),
                    status,
                    draining: status == TaskStatus::Draining,
                });
            }
            transaction.execute(
                "INSERT INTO tasks (
                    workspace_id, task_id, agent_id, status, next_action, settings_json,
                    handoff, heartbeat_at, expires_at, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, 'active', ?4, ?5, NULL, ?6, ?7, ?6, ?6)",
                params![
                    &context.workspace_id,
                    &context.task_id,
                    &context.agent_id,
                    input.next_action.trim(),
                    serialize_json(&input.settings)?,
                    &context.observed_at,
                    &input.expires_at,
                ],
            )?;
            if let Some(process) = &input.runtime_process {
                if process.process_start_identity.trim().is_empty() {
                    return Err(StoreError::InvalidInput(
                        "process_start_identity is required".to_string(),
                    ));
                }
                transaction.execute(
                    "INSERT INTO runtime_processes (
                        workspace_id, agent_id, pid, process_start_identity, status, heartbeat_at
                     ) VALUES (?1, ?2, ?3, ?4, 'active', ?5)
                     ON CONFLICT(workspace_id, agent_id) DO UPDATE SET
                        pid = excluded.pid,
                        process_start_identity = excluded.process_start_identity,
                        status = 'active', heartbeat_at = excluded.heartbeat_at",
                    params![
                        &context.workspace_id,
                        &context.agent_id,
                        i64::from(process.pid),
                        &process.process_start_identity,
                        &context.observed_at,
                    ],
                )?;
            }
            append_audit(
                transaction,
                context,
                "task.started",
                json!({
                    "next_action": input.next_action,
                    "settings": input.settings,
                    "expires_at": input.expires_at,
                }),
            )?;
            Ok(TaskCommandResult {
                task_id: context.task_id.clone(),
                status: TaskStatus::Active,
                draining: false,
            })
        })
    }

    pub fn task_heartbeat(
        &mut self,
        context: &CommandContext,
        input: &TaskHeartbeatInput,
    ) -> StoreResult<TaskCommandResult> {
        self.accepted_command(context, "task.heartbeat", input, |transaction| {
            require_task_owner(transaction, context, true)?;
            let settings = task_coordination_settings(transaction, context)?;
            validate_future_window(
                &context.observed_at,
                &input.expires_at,
                setting_seconds(settings.inactivity_timeout_seconds)?,
                "task expires_at",
            )?;
            if input.next_action.trim().is_empty() {
                return Err(StoreError::InvalidInput(
                    "next_action is required".to_string(),
                ));
            }
            transaction.execute(
                "UPDATE tasks SET next_action = ?3, heartbeat_at = ?4,
                    expires_at = ?5, updated_at = ?4
                 WHERE workspace_id = ?1 AND task_id = ?2",
                params![
                    &context.workspace_id,
                    &context.task_id,
                    input.next_action.trim(),
                    &context.observed_at,
                    &input.expires_at,
                ],
            )?;
            transaction.execute(
                "UPDATE runtime_processes SET heartbeat_at = ?3
                 WHERE workspace_id = ?1 AND agent_id = ?2 AND status = 'active'",
                params![
                    &context.workspace_id,
                    &context.agent_id,
                    &context.observed_at
                ],
            )?;
            let status = task_status(transaction, context)?;
            append_audit(
                transaction,
                context,
                "task.heartbeat",
                json!({
                    "next_action": input.next_action,
                    "expires_at": input.expires_at,
                }),
            )?;
            Ok(TaskCommandResult {
                task_id: context.task_id.clone(),
                status,
                draining: status == TaskStatus::Draining,
            })
        })
    }

    pub fn task_finalize(
        &mut self,
        context: &CommandContext,
        input: &TaskEndInput,
    ) -> StoreResult<TaskCommandResult> {
        self.end_task(context, input, TaskStatus::Completed, "task.finalize")
    }

    pub fn task_cancel(
        &mut self,
        context: &CommandContext,
        input: &TaskEndInput,
    ) -> StoreResult<TaskCommandResult> {
        self.end_task(context, input, TaskStatus::Cancelled, "task.cancel")
    }

    fn end_task(
        &mut self,
        context: &CommandContext,
        input: &TaskEndInput,
        terminal: TaskStatus,
        command_kind: &'static str,
    ) -> StoreResult<TaskCommandResult> {
        self.accepted_command(context, command_kind, input, |transaction| {
            let current = require_task_owner(transaction, context, false)?;
            if current.is_terminal() {
                return Ok(TaskCommandResult {
                    task_id: context.task_id.clone(),
                    status: current,
                    draining: false,
                });
            }
            transaction.execute(
                "UPDATE tasks SET status = 'draining', terminal_status = ?3,
                    handoff = ?4, updated_at = ?5
                 WHERE workspace_id = ?1 AND task_id = ?2",
                params![
                    &context.workspace_id,
                    &context.task_id,
                    task_status_name(terminal),
                    input.handoff.as_deref(),
                    &context.observed_at,
                ],
            )?;
            transaction.execute(
                "UPDATE read_intents SET status = 'released'
                 WHERE workspace_id = ?1 AND task_id = ?2 AND status = 'active'",
                params![&context.workspace_id, &context.task_id],
            )?;
            transaction.execute(
                "UPDATE resource_evidence SET valid = 0
                 WHERE workspace_id = ?1 AND task_id = ?2 AND valid = 1",
                params![&context.workspace_id, &context.task_id],
            )?;
            transaction.execute(
                "UPDATE lease_requests SET state = 'cancelled', updated_at = ?3
                 WHERE workspace_id = ?1 AND task_id = ?2 AND state IN ('queued', 'offered')",
                params![
                    &context.workspace_id,
                    &context.task_id,
                    &context.observed_at
                ],
            )?;
            let in_flight = task_in_flight_count(transaction, context)?;
            let status = if in_flight == 0 {
                release_all_task_leases(transaction, context)?;
                update_task_terminal(transaction, context, terminal)?;
                promote_waiters(transaction, &context.observed_at)?;
                terminal
            } else {
                transaction.execute(
                    "UPDATE active_leases SET state = 'draining', release_pending = 1
                     WHERE workspace_id = ?1 AND task_id = ?2",
                    params![&context.workspace_id, &context.task_id],
                )?;
                TaskStatus::Draining
            };
            append_audit(
                transaction,
                context,
                command_kind,
                json!({
                    "status": task_status_name(status),
                    "in_flight": in_flight,
                    "handoff": input.handoff,
                }),
            )?;
            Ok(TaskCommandResult {
                task_id: context.task_id.clone(),
                status,
                draining: status == TaskStatus::Draining,
            })
        })
    }
}

impl Store {
    pub fn read_start(
        &mut self,
        context: &CommandContext,
        input: &ReadStartInput,
    ) -> StoreResult<ReadCommandResult> {
        self.accepted_command(context, "read.start", input, |transaction| {
            require_task_owner(transaction, context, true)?;
            if input.read_id.trim().is_empty() || input.invocation_id.trim().is_empty() {
                return Err(StoreError::InvalidInput(
                    "read_id and invocation_id are required".to_string(),
                ));
            }
            let resources = sync_observations(transaction, context, &input.resources)?;
            if resources.is_empty() {
                return Err(StoreError::InvalidInput(
                    "read resource set is required".to_string(),
                ));
            }
            let inserted = transaction.execute(
                "INSERT INTO read_attempts (
                    read_id, workspace_id, task_id, invocation_id, resources_json,
                    status, started_at, completed_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 'started', ?6, NULL)
                 ON CONFLICT(read_id) DO NOTHING",
                params![
                    &input.read_id,
                    &context.workspace_id,
                    &context.task_id,
                    &input.invocation_id,
                    serialize_json(&resources)?,
                    &context.observed_at,
                ],
            )?;
            if inserted != 1 {
                return Err(StoreError::InvalidState(
                    "read attempt already exists; only its original command receipt may replay"
                        .to_string(),
                ));
            }
            append_audit(
                transaction,
                context,
                "read.started",
                json!({
                    "read_id": input.read_id,
                    "invocation_id": input.invocation_id,
                    "resources": resources,
                }),
            )?;
            Ok(ReadCommandResult {
                read_id: input.read_id.clone(),
                status: ReadResultStatus::Started,
                evidence_id: None,
            })
        })
    }

    pub fn read_complete(
        &mut self,
        context: &CommandContext,
        input: &ReadCompleteInput,
    ) -> StoreResult<ReadCommandResult> {
        self.accepted_command(context, "read.complete", input, |transaction| {
            require_task_owner(transaction, context, true)?;
            let attempt = transaction
                .query_row(
                    "SELECT invocation_id, resources_json, status, started_at
                     FROM read_attempts
                     WHERE read_id = ?1 AND workspace_id = ?2 AND task_id = ?3",
                    params![&input.read_id, &context.workspace_id, &context.task_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    },
                )
                .optional()?
                .ok_or_else(|| StoreError::NotFound("read attempt not found".to_string()))?;
            if attempt.0 != input.invocation_id {
                return Err(StoreError::Ownership(
                    "read invocation does not match start".to_string(),
                ));
            }
            if attempt.2 != "started" {
                return Err(StoreError::InvalidState(
                    "read attempt is terminal; only its original completion receipt may replay"
                        .to_string(),
                ));
            }
            let eligible = input.terminal_success && input.complete && input.stable && input.exact;
            if !eligible {
                transition_read_attempt(
                    transaction,
                    &input.read_id,
                    "failed",
                    &context.observed_at,
                )?;
                append_audit(
                    transaction,
                    context,
                    "read.failed",
                    json!({
                        "read_id": input.read_id,
                        "terminal_success": input.terminal_success,
                        "complete": input.complete,
                        "stable": input.stable,
                        "exact": input.exact,
                    }),
                )?;
                return Ok(ReadCommandResult {
                    read_id: input.read_id.clone(),
                    status: ReadResultStatus::Failed,
                    evidence_id: None,
                });
            }
            let started: Vec<ResourceObservation> = deserialize_json(&attempt.1)?;
            let reported = canonicalize_observations(context, &input.resources)?;
            let resources = started
                .iter()
                .map(|observation| observation.resource().clone())
                .collect::<Vec<_>>();
            let completed = current_observations(transaction, &resources)?;
            if started != completed || !observations_same_state(&started, &reported)? {
                transition_read_attempt(
                    transaction,
                    &input.read_id,
                    "failed",
                    &context.observed_at,
                )?;
                append_audit(
                    transaction,
                    context,
                    "read.unstable",
                    json!({
                        "read_id": input.read_id,
                    }),
                )?;
                return Ok(ReadCommandResult {
                    read_id: input.read_id.clone(),
                    status: ReadResultStatus::Failed,
                    evidence_id: None,
                });
            }
            transition_read_attempt(
                transaction,
                &input.read_id,
                "completed",
                &context.observed_at,
            )?;
            let evidence_id = Uuid::new_v4().to_string();
            for observation in &completed {
                insert_evidence(
                    transaction,
                    context,
                    &evidence_id,
                    &input.read_id,
                    observation,
                    &attempt.3,
                )?;
            }
            append_audit(
                transaction,
                context,
                "read.completed",
                json!({
                    "read_id": input.read_id,
                    "evidence_id": evidence_id,
                    "resources": completed,
                }),
            )?;
            Ok(ReadCommandResult {
                read_id: input.read_id.clone(),
                status: ReadResultStatus::Completed,
                evidence_id: Some(evidence_id),
            })
        })
    }

    pub fn lease_request_status(
        &self,
        workspace_id: &str,
        task_id: &str,
        batch_id: &str,
        now: &str,
    ) -> StoreResult<LeaseRequestStatus> {
        validate_timestamp(now)?;
        let row = self
            .conn
            .query_row(
                "SELECT state, version, offer_id, offer_expires_at, superseded_by
                 FROM lease_requests
                 WHERE workspace_id = ?1 AND task_id = ?2 AND batch_id = ?3",
                params![workspace_id, task_id, batch_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, u64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| StoreError::NotFound("lease request not found".to_string()))?;
        Ok(LeaseRequestStatus {
            batch_id: batch_id.to_string(),
            state: parse_request_state(&row.0)?,
            version: row.1,
            offer_id: row.2,
            offer_expires_at: row.3,
            superseded_by: row.4,
        })
    }

    pub fn status(&self) -> StoreResult<StatusSnapshot> {
        Ok(StatusSnapshot {
            active_tasks: count_where(&self.conn, "tasks", "status = 'active'")?,
            draining_tasks: count_where(&self.conn, "tasks", "status = 'draining'")?,
            active_leases: count_where(&self.conn, "active_leases", "state = 'active'")?,
            draining_leases: count_where(&self.conn, "active_leases", "state = 'draining'")?,
            queued_requests: count_where(&self.conn, "lease_requests", "state = 'queued'")?,
            offered_requests: count_where(&self.conn, "lease_requests", "state = 'offered'")?,
            executing_writes: count_where(&self.conn, "write_attempts", "status = 'executing'")?,
            uncertain_writes: count_where(&self.conn, "write_attempts", "status = 'uncertain'")?,
        })
    }

    pub fn audit_events(&self, limit: usize) -> StoreResult<Vec<AuditRecord>> {
        let limit = i64::try_from(limit.min(1_000))
            .map_err(|_| StoreError::InvalidInput("invalid audit limit".to_string()))?;
        let mut statement = self.conn.prepare(
            "SELECT event_id, workspace_id, task_id, agent_id, event_type, payload_json, created_at
             FROM audit_events ORDER BY rowid DESC LIMIT ?1",
        )?;
        let rows = statement.query_map([limit], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
            ))
        })?;
        let mut records = Vec::new();
        for row in rows {
            let row = row?;
            records.push(AuditRecord {
                event_id: row.0,
                workspace_id: row.1,
                task_id: row.2,
                agent_id: row.3,
                event_type: row.4,
                payload: deserialize_json(&row.5)?,
                created_at: row.6,
            });
        }
        Ok(records)
    }

    pub fn maintain(&mut self, now: &str) -> StoreResult<()> {
        validate_timestamp(now)?;
        let transaction = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        expire_offers(&transaction, now)?;
        expire_tasks_and_leases(&transaction, now)?;
        transaction.execute(
            "UPDATE read_attempts SET status = 'failed', completed_at = ?1
             WHERE status = 'started'
               AND EXISTS (
                   SELECT 1 FROM tasks
                   WHERE tasks.workspace_id = read_attempts.workspace_id
                     AND tasks.task_id = read_attempts.task_id
                     AND tasks.status <> 'active'
               )",
            [now],
        )?;
        transaction.execute(
            "UPDATE read_intents SET status = 'released'
             WHERE status = 'active'
               AND EXISTS (
                   SELECT 1 FROM tasks
                   WHERE tasks.workspace_id = read_intents.workspace_id
                     AND tasks.task_id = read_intents.task_id
                     AND tasks.status <> 'active'
               )",
            [],
        )?;
        transaction.execute(
            "UPDATE resource_evidence SET valid = 0
             WHERE valid = 1
               AND EXISTS (
                   SELECT 1 FROM tasks
                   WHERE tasks.workspace_id = resource_evidence.workspace_id
                     AND tasks.task_id = resource_evidence.task_id
                     AND tasks.status <> 'active'
               )",
            [],
        )?;
        transaction.execute(
            "UPDATE active_leases SET state = 'draining', release_pending = 1
             WHERE state = 'active' AND batch_id IN (
                SELECT batch_id FROM write_attempts
                WHERE status = 'executing' AND deadline <= ?1
             )",
            [now],
        )?;
        let cutoff =
            format_timestamp(parse_timestamp(now)? - Duration::days(AUDIT_RETENTION_DAYS))?;
        transaction.execute("DELETE FROM audit_events WHERE created_at < ?1", [&cutoff])?;
        promote_waiters(&transaction, now)?;
        transaction.commit()?;
        Ok(())
    }

    fn accepted_command<P, R>(
        &mut self,
        context: &CommandContext,
        command_kind: &'static str,
        payload: &P,
        operation: impl FnOnce(&Transaction<'_>) -> StoreResult<R>,
    ) -> StoreResult<R>
    where
        P: Serialize,
        R: Serialize + DeserializeOwned,
    {
        validate_context(context)?;
        let payload = serde_json::to_value(payload)?;
        let payload_digest = digest_canonical_json(&payload).value;
        let transaction = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let receipt = transaction
            .query_row(
                "SELECT contract_revision, command_kind, payload_digest, response_json
                 FROM command_receipts
                 WHERE workspace_id = ?1 AND agent_id = ?2 AND request_id = ?3",
                params![
                    &context.workspace_id,
                    &context.agent_id,
                    &context.request_id,
                ],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?;
        if let Some(receipt) = receipt {
            if receipt.0 != CONTRACT_REVISION
                || receipt.1 != command_kind
                || receipt.2 != payload_digest
            {
                return Err(StoreError::IdempotencyMismatch);
            }
            let response = deserialize_json(&receipt.3)?;
            transaction.commit()?;
            return Ok(response);
        }
        expire_tasks_and_leases(&transaction, &context.observed_at)?;
        promote_waiters(&transaction, &context.observed_at)?;
        let response = operation(&transaction)?;
        let response_json = serialize_json(&response)?;
        transaction.execute(
            "INSERT INTO command_events (
                event_id, workspace_id, task_id, agent_id, request_id, contract_revision,
                command_kind, payload_json, response_json, recorded_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                Uuid::new_v4().to_string(),
                &context.workspace_id,
                &context.task_id,
                &context.agent_id,
                &context.request_id,
                CONTRACT_REVISION,
                command_kind,
                serialize_json(&payload)?,
                &response_json,
                &context.observed_at,
            ],
        )?;
        transaction.execute(
            "INSERT INTO command_receipts (
                workspace_id, agent_id, request_id, contract_revision, command_kind,
                payload_digest, response_json, recorded_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                &context.workspace_id,
                &context.agent_id,
                &context.request_id,
                CONTRACT_REVISION,
                command_kind,
                payload_digest,
                &response_json,
                &context.observed_at,
            ],
        )?;
        transaction.commit()?;
        Ok(response)
    }
}

fn validate_context(context: &CommandContext) -> StoreResult<()> {
    for (field, value) in [
        ("request_id", context.request_id.as_str()),
        ("task_id", context.task_id.as_str()),
        ("agent_id", context.agent_id.as_str()),
        ("workspace_id", context.workspace_id.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(StoreError::InvalidInput(format!("{field} is required")));
        }
    }
    validate_timestamp(&context.observed_at)?;
    Ok(())
}

fn task_row(
    transaction: &Transaction<'_>,
    context: &CommandContext,
) -> StoreResult<Option<(String, TaskStatus)>> {
    let row = transaction
        .query_row(
            "SELECT agent_id, status FROM tasks WHERE workspace_id = ?1 AND task_id = ?2",
            params![&context.workspace_id, &context.task_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    row.map(|(agent, status)| Ok((agent, parse_task_status(&status)?)))
        .transpose()
}

fn task_status(transaction: &Transaction<'_>, context: &CommandContext) -> StoreResult<TaskStatus> {
    task_row(transaction, context)?
        .map(|row| row.1)
        .ok_or_else(|| StoreError::NotFound("task not found".to_string()))
}

fn task_coordination_settings(
    transaction: &Transaction<'_>,
    context: &CommandContext,
) -> StoreResult<CoordinationSettings> {
    let settings = transaction.query_row(
        "SELECT settings_json FROM tasks WHERE workspace_id = ?1 AND task_id = ?2",
        params![&context.workspace_id, &context.task_id],
        |row| row.get::<_, String>(0),
    )?;
    let settings: CoordinationSettings = deserialize_json(&settings)?;
    settings.validate().map_err(StoreError::Corrupt)?;
    Ok(settings)
}

fn require_task_owner(
    transaction: &Transaction<'_>,
    context: &CommandContext,
    require_active: bool,
) -> StoreResult<TaskStatus> {
    let (agent_id, status) = task_row(transaction, context)?
        .ok_or_else(|| StoreError::NotFound("task not found".to_string()))?;
    if agent_id != context.agent_id {
        return Err(StoreError::Ownership("task owner mismatch".to_string()));
    }
    if require_active && status != TaskStatus::Active {
        return Err(StoreError::InvalidState("task is not active".to_string()));
    }
    Ok(status)
}

fn parse_task_status(value: &str) -> StoreResult<TaskStatus> {
    match value {
        "active" => Ok(TaskStatus::Active),
        "draining" => Ok(TaskStatus::Draining),
        "completed" => Ok(TaskStatus::Completed),
        "failed" => Ok(TaskStatus::Failed),
        "cancelled" => Ok(TaskStatus::Cancelled),
        _ => Err(StoreError::Corrupt(format!("unknown task status: {value}"))),
    }
}

fn task_status_name(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Active => "active",
        TaskStatus::Draining => "draining",
        TaskStatus::Completed => "completed",
        TaskStatus::Failed => "failed",
        TaskStatus::Cancelled => "cancelled",
    }
}

fn update_task_terminal(
    transaction: &Transaction<'_>,
    context: &CommandContext,
    status: TaskStatus,
) -> StoreResult<()> {
    transaction.execute(
        "UPDATE tasks SET status = ?3, updated_at = ?4
         WHERE workspace_id = ?1 AND task_id = ?2",
        params![
            &context.workspace_id,
            &context.task_id,
            task_status_name(status),
            &context.observed_at,
        ],
    )?;
    Ok(())
}

fn transition_read_attempt(
    transaction: &Transaction<'_>,
    read_id: &str,
    status: &str,
    completed_at: &str,
) -> StoreResult<()> {
    let updated = transaction.execute(
        "UPDATE read_attempts SET status = ?2, completed_at = ?3
         WHERE read_id = ?1 AND status = 'started'",
        params![read_id, status, completed_at],
    )?;
    if updated != 1 {
        return Err(StoreError::InvalidState(
            "read attempt terminal transition lost its one-shot CAS".to_string(),
        ));
    }
    Ok(())
}

fn sync_observations(
    transaction: &Transaction<'_>,
    context: &CommandContext,
    input: &[ResourceObservation],
) -> StoreResult<Vec<ResourceObservation>> {
    let mut ordered = input.to_vec();
    ordered.sort_by(|left, right| left.resource().cmp(right.resource()));
    let mut seen = BTreeSet::new();
    let mut synced = Vec::with_capacity(ordered.len());
    for observation in ordered {
        if !observation.has_matching_resource_kind()
            || observation.resource().workspace_id != context.workspace_id
        {
            return Err(StoreError::InvalidInput(
                "resource observation kind or workspace mismatch".to_string(),
            ));
        }
        let identity = (
            observation.resource().workspace_id.clone(),
            observation.resource().kind,
            observation.resource().resource_id.clone(),
        );
        if !seen.insert(identity) {
            return Err(StoreError::InvalidInput(
                "duplicate resource observation".to_string(),
            ));
        }
        let resource = observation.resource();
        let state_json = observation_state_json(&observation)?;
        let current = transaction
            .query_row(
                "SELECT kind, state_json, generation
                 FROM resources WHERE workspace_id = ?1 AND resource_id = ?2",
                params![&resource.workspace_id, &resource.resource_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, u64>(2)?,
                    ))
                },
            )
            .optional()?;
        let generation = match current {
            Some((kind, current_state, generation)) => {
                if kind != resource_kind_name(resource.kind) || current_state != state_json {
                    let generation = generation.checked_add(1).ok_or_else(|| {
                        StoreError::Corrupt("resource generation overflow".to_string())
                    })?;
                    revoke_overlapping(transaction, context, resource, None)?;
                    transaction.execute(
                        "UPDATE resources SET kind = ?3, state_json = ?4,
                            generation = ?5, updated_at = ?6
                         WHERE workspace_id = ?1 AND resource_id = ?2",
                        params![
                            &resource.workspace_id,
                            &resource.resource_id,
                            resource_kind_name(resource.kind),
                            &state_json,
                            generation,
                            &context.observed_at,
                        ],
                    )?;
                    generation
                } else {
                    generation
                }
            }
            None => {
                revoke_overlapping(transaction, context, resource, None)?;
                transaction.execute(
                    "INSERT INTO resources (
                        workspace_id, resource_id, kind, canonical_path,
                        state_json, generation, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)",
                    params![
                        &resource.workspace_id,
                        &resource.resource_id,
                        resource_kind_name(resource.kind),
                        &resource.canonical_path,
                        &state_json,
                        &context.observed_at,
                    ],
                )?;
                1
            }
        };
        transaction.execute(
            "INSERT OR IGNORE INTO resource_aliases (workspace_id, resource_id, canonical_path)
             VALUES (?1, ?2, ?3)",
            params![
                &resource.workspace_id,
                &resource.resource_id,
                &resource.canonical_path,
            ],
        )?;
        synced.push(with_generation(observation, generation));
    }
    Ok(synced)
}

fn observation_state_json(observation: &ResourceObservation) -> StoreResult<String> {
    match observation {
        ResourceObservation::Object { observed, .. } => serialize_json(observed),
        ResourceObservation::Entry { observed, .. } => serialize_json(observed),
        ResourceObservation::DirectoryTree { observed, .. } => serialize_json(observed),
    }
}

fn with_generation(observation: ResourceObservation, generation: u64) -> ResourceObservation {
    match observation {
        ResourceObservation::Object {
            resource, observed, ..
        } => ResourceObservation::Object {
            resource,
            observed,
            generation,
        },
        ResourceObservation::Entry {
            resource, observed, ..
        } => ResourceObservation::Entry {
            resource,
            observed,
            generation,
        },
        ResourceObservation::DirectoryTree {
            resource, observed, ..
        } => ResourceObservation::DirectoryTree {
            resource,
            observed,
            generation,
        },
    }
}

fn insert_evidence(
    transaction: &Transaction<'_>,
    context: &CommandContext,
    evidence_id: &str,
    read_id: &str,
    observation: &ResourceObservation,
    read_started_at: &str,
) -> StoreResult<()> {
    let resource = observation.resource();
    let newer_started_at = transaction
        .query_row(
            "SELECT read_started_at FROM read_intents
             WHERE workspace_id = ?1 AND task_id = ?2 AND resource_id = ?3 AND status = 'active'",
            params![
                &context.workspace_id,
                &context.task_id,
                &resource.resource_id,
            ],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let valid = newer_started_at
        .as_deref()
        .is_none_or(|existing| existing <= read_started_at)
        && !peer_write_in_flight(transaction, context, resource)?;
    transaction.execute(
        "INSERT INTO resource_evidence (
            evidence_id, workspace_id, task_id, source_kind, source_id, resource_id,
            resource_kind, canonical_path, observation_json, generation, complete,
            stable, exact, valid, recorded_at, read_started_at
         ) VALUES (?1, ?2, ?3, 'read', ?4, ?5, ?6, ?7, ?8, ?9,
                   1, 1, 1, ?10, ?11, ?12)",
        params![
            evidence_id,
            &context.workspace_id,
            &context.task_id,
            read_id,
            &resource.resource_id,
            resource_kind_name(resource.kind),
            &resource.canonical_path,
            serialize_json(observation)?,
            observation.generation(),
            i64::from(valid),
            &context.observed_at,
            read_started_at,
        ],
    )?;
    if valid {
        transaction.execute(
            "INSERT INTO read_intents (
                workspace_id, task_id, resource_id, resource_kind, canonical_path,
                evidence_id, status, created_at, read_started_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'active', ?7, ?8)
             ON CONFLICT(workspace_id, task_id, resource_id) DO UPDATE SET
                resource_kind = excluded.resource_kind,
                canonical_path = excluded.canonical_path,
                evidence_id = excluded.evidence_id,
                status = 'active',
                created_at = excluded.created_at,
                read_started_at = excluded.read_started_at
             WHERE excluded.read_started_at >= read_intents.read_started_at",
            params![
                &context.workspace_id,
                &context.task_id,
                &resource.resource_id,
                resource_kind_name(resource.kind),
                &resource.canonical_path,
                evidence_id,
                &context.observed_at,
                read_started_at,
            ],
        )?;
    }
    Ok(())
}

fn insert_own_write_evidence(
    transaction: &Transaction<'_>,
    context: &CommandContext,
    attempt_id: &str,
    observations: &[ResourceObservation],
) -> StoreResult<()> {
    let evidence_id = Uuid::new_v4().to_string();
    for observation in observations {
        let resource = observation.resource();
        transaction.execute(
            "INSERT INTO resource_evidence (
                evidence_id, workspace_id, task_id, source_kind, source_id, resource_id,
                resource_kind, canonical_path, observation_json, generation, complete,
                stable, exact, valid, recorded_at, read_started_at
             ) VALUES (?1, ?2, ?3, 'own_write', ?4, ?5, ?6, ?7, ?8, ?9,
                       1, 1, 1, 1, ?10, NULL)",
            params![
                &evidence_id,
                &context.workspace_id,
                &context.task_id,
                attempt_id,
                &resource.resource_id,
                resource_kind_name(resource.kind),
                &resource.canonical_path,
                serialize_json(observation)?,
                observation.generation(),
                &context.observed_at,
            ],
        )?;
    }
    Ok(())
}

fn peer_write_in_flight(
    transaction: &Transaction<'_>,
    context: &CommandContext,
    resource: &ResourceKey,
) -> StoreResult<bool> {
    let mut statement = transaction.prepare(
        "SELECT lr.resource_id, lr.resource_kind, lr.canonical_path
         FROM lease_resources lr
         JOIN active_leases al ON al.batch_id = lr.batch_id
         WHERE al.workspace_id = ?1 AND al.task_id <> ?2
           AND lr.in_flight_attempt_id IS NOT NULL",
    )?;
    let rows = statement.query_map(params![&context.workspace_id, &context.task_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    for row in rows {
        let row = row?;
        let candidate = ResourceKey {
            workspace_id: context.workspace_id.clone(),
            resource_id: row.0,
            kind: parse_resource_kind(&row.1)?,
            canonical_path: row.2,
        };
        if resource_keys_overlap(resource, &candidate) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn revoke_overlapping(
    transaction: &Transaction<'_>,
    context: &CommandContext,
    resource: &ResourceKey,
    exclude_task: Option<&str>,
) -> StoreResult<usize> {
    let mut statement = transaction.prepare(
        "SELECT evidence_id, task_id, resource_id, resource_kind, canonical_path
         FROM resource_evidence WHERE workspace_id = ?1 AND valid = 1",
    )?;
    let rows = statement.query_map([&context.workspace_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;
    let mut revoked = Vec::new();
    for row in rows {
        let row = row?;
        if exclude_task.is_some_and(|task| task == row.1) {
            continue;
        }
        let candidate = ResourceKey {
            workspace_id: context.workspace_id.clone(),
            resource_id: row.2,
            kind: parse_resource_kind(&row.3)?,
            canonical_path: row.4,
        };
        if resource_keys_overlap(resource, &candidate) {
            revoked.push((row.0, candidate.resource_id));
        }
    }
    drop(statement);
    for (evidence_id, resource_id) in &revoked {
        transaction.execute(
            "UPDATE read_intents SET status = 'revoked'
             WHERE evidence_id = ?1 AND resource_id = ?2 AND status = 'active'",
            params![evidence_id, resource_id],
        )?;
        transaction.execute(
            "UPDATE resource_evidence SET valid = 0
             WHERE evidence_id = ?1 AND resource_id = ?2 AND valid = 1",
            params![evidence_id, resource_id],
        )?;
    }
    Ok(revoked.len())
}

fn append_audit(
    transaction: &Transaction<'_>,
    context: &CommandContext,
    event_type: &str,
    payload: Value,
) -> StoreResult<()> {
    transaction.execute(
        "INSERT INTO audit_events (
            event_id, workspace_id, task_id, agent_id, event_type, payload_json, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            Uuid::new_v4().to_string(),
            &context.workspace_id,
            &context.task_id,
            &context.agent_id,
            event_type,
            serialize_json(&payload)?,
            &context.observed_at,
        ],
    )?;
    Ok(())
}

fn serialize_json(value: &(impl Serialize + ?Sized)) -> StoreResult<String> {
    Ok(serde_json::to_string(value)?)
}

fn deserialize_json<T: DeserializeOwned>(value: &str) -> StoreResult<T> {
    Ok(serde_json::from_str(value)?)
}

fn resource_kind_name(kind: ResourceKind) -> &'static str {
    match kind {
        ResourceKind::Object => "object",
        ResourceKind::Entry => "entry",
        ResourceKind::DirectoryTree => "directory_tree",
    }
}

fn parse_resource_kind(value: &str) -> StoreResult<ResourceKind> {
    match value {
        "object" => Ok(ResourceKind::Object),
        "entry" => Ok(ResourceKind::Entry),
        "directory_tree" => Ok(ResourceKind::DirectoryTree),
        _ => Err(StoreError::Corrupt(format!(
            "unknown resource kind: {value}"
        ))),
    }
}

fn parse_request_state(value: &str) -> StoreResult<LeaseRequestState> {
    match value {
        "queued" => Ok(LeaseRequestState::Queued),
        "offered" => Ok(LeaseRequestState::Offered),
        "activated" => Ok(LeaseRequestState::Activated),
        "superseded" => Ok(LeaseRequestState::Superseded),
        "expired" => Ok(LeaseRequestState::Expired),
        "cancelled" => Ok(LeaseRequestState::Cancelled),
        _ => Err(StoreError::Corrupt(format!(
            "unknown lease request state: {value}"
        ))),
    }
}

fn count_where(
    connection: &rusqlite::Connection,
    table: &str,
    predicate: &str,
) -> StoreResult<u64> {
    let sql = format!("SELECT COUNT(*) FROM {table} WHERE {predicate}");
    Ok(connection.query_row(&sql, [], |row| row.get::<_, u64>(0))?)
}

fn parse_timestamp(value: &str) -> StoreResult<OffsetDateTime> {
    OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|_| StoreError::InvalidTimestamp(value.to_string()))
}

fn validate_timestamp(value: &str) -> StoreResult<()> {
    parse_timestamp(value).map(|_| ())
}

fn setting_seconds(value: u64) -> StoreResult<i64> {
    i64::try_from(value)
        .map_err(|_| StoreError::InvalidInput("coordination duration is too large".to_string()))
}

fn validate_future(observed_at: &str, future: &str, field: &str) -> StoreResult<()> {
    if parse_timestamp(future)? <= parse_timestamp(observed_at)? {
        return Err(StoreError::InvalidInput(format!(
            "{field} must be after observed_at"
        )));
    }
    Ok(())
}

fn validate_future_window(
    observed_at: &str,
    expires_at: &str,
    maximum_seconds: i64,
    field: &str,
) -> StoreResult<()> {
    let observed_at = parse_timestamp(observed_at)?;
    let expires_at = parse_timestamp(expires_at)?;
    if expires_at <= observed_at || expires_at - observed_at > Duration::seconds(maximum_seconds) {
        return Err(StoreError::InvalidInput(format!(
            "{field} must be after observed_at and at most {maximum_seconds} seconds later"
        )));
    }
    Ok(())
}

fn format_timestamp(value: OffsetDateTime) -> StoreResult<String> {
    value
        .format(&Rfc3339)
        .map_err(|error| StoreError::Corrupt(format!("timestamp format failed: {error}")))
}

fn task_in_flight_count(
    transaction: &Transaction<'_>,
    context: &CommandContext,
) -> StoreResult<u64> {
    Ok(transaction.query_row(
        "SELECT COUNT(*)
         FROM write_attempts attempt
         JOIN active_leases lease ON lease.batch_id = attempt.batch_id
         WHERE attempt.workspace_id = ?1 AND attempt.task_id = ?2
           AND attempt.status IN ('executing', 'uncertain')",
        params![&context.workspace_id, &context.task_id],
        |row| row.get::<_, u64>(0),
    )?)
}

fn release_all_task_leases(
    transaction: &Transaction<'_>,
    context: &CommandContext,
) -> StoreResult<()> {
    transaction.execute(
        "DELETE FROM active_leases WHERE workspace_id = ?1 AND task_id = ?2",
        params![&context.workspace_id, &context.task_id],
    )?;
    Ok(())
}

fn expire_offers(transaction: &Transaction<'_>, now: &str) -> StoreResult<()> {
    transaction.execute(
        "UPDATE lease_requests SET state = 'expired', updated_at = ?1
         WHERE state = 'offered' AND offer_expires_at <= ?1",
        [now],
    )?;
    Ok(())
}

fn expire_tasks_and_leases(transaction: &Transaction<'_>, now: &str) -> StoreResult<()> {
    let mut statement = transaction.prepare(
        "SELECT workspace_id, task_id FROM tasks
         WHERE status = 'active' AND expires_at <= ?1",
    )?;
    let rows = statement.query_map([now], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let expired = rows.collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    for (workspace_id, task_id) in expired {
        let in_flight = transaction.query_row(
            "SELECT COUNT(*) FROM write_attempts attempt
             JOIN active_leases lease ON lease.batch_id = attempt.batch_id
             WHERE attempt.workspace_id = ?1 AND attempt.task_id = ?2
               AND attempt.status IN ('executing', 'uncertain')",
            params![&workspace_id, &task_id],
            |row| row.get::<_, u64>(0),
        )?;
        transaction.execute(
            "UPDATE tasks SET status = ?3, terminal_status = 'failed', updated_at = ?4
             WHERE workspace_id = ?1 AND task_id = ?2",
            params![
                &workspace_id,
                &task_id,
                if in_flight == 0 { "failed" } else { "draining" },
                now,
            ],
        )?;
        transaction.execute(
            "UPDATE lease_requests SET state = 'cancelled', updated_at = ?3
             WHERE workspace_id = ?1 AND task_id = ?2 AND state IN ('queued', 'offered')",
            params![&workspace_id, &task_id, now],
        )?;
        if in_flight == 0 {
            transaction.execute(
                "DELETE FROM active_leases WHERE workspace_id = ?1 AND task_id = ?2",
                params![&workspace_id, &task_id],
            )?;
        } else {
            transaction.execute(
                "UPDATE active_leases SET state = 'draining', release_pending = 1
                 WHERE workspace_id = ?1 AND task_id = ?2",
                params![&workspace_id, &task_id],
            )?;
        }
    }
    let mut lease_statement = transaction.prepare(
        "SELECT batch_id FROM active_leases
         WHERE expires_at <= ?1 AND state = 'active'",
    )?;
    let expired_batches = lease_statement
        .query_map([now], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(lease_statement);
    for batch_id in expired_batches {
        let in_flight = transaction.query_row(
            "SELECT COUNT(*) FROM lease_resources
             WHERE batch_id = ?1 AND in_flight_attempt_id IS NOT NULL",
            [&batch_id],
            |row| row.get::<_, u64>(0),
        )?;
        if in_flight == 0 {
            transaction.execute("DELETE FROM active_leases WHERE batch_id = ?1", [&batch_id])?;
        } else {
            transaction.execute(
                "UPDATE active_leases SET state = 'draining', release_pending = 1
                 WHERE batch_id = ?1",
                [&batch_id],
            )?;
        }
    }
    Ok(())
}

impl Store {
    pub fn write_prepare(
        &mut self,
        context: &CommandContext,
        input: &WritePrepareInput,
    ) -> StoreResult<WritePrepareResult> {
        self.accepted_command(context, "write.prepare", input, |transaction| {
            require_task_owner(transaction, context, true)?;
            let settings = task_coordination_settings(transaction, context)?;
            validate_future_window(
                &context.observed_at,
                &input.request_expires_at,
                setting_seconds(settings.offer_ttl_seconds)?,
                "request_expires_at",
            )?;
            validate_future_window(
                &context.observed_at,
                &input.lease_expires_at,
                setting_seconds(settings.lease_expiry_seconds)?,
                "lease_expires_at",
            )?;
            validate_future(
                &context.observed_at,
                &input.attempt_deadline,
                "attempt_deadline",
            )?;
            if input.invocation_id.trim().is_empty() {
                return Err(StoreError::InvalidInput(
                    "invocation_id is required".to_string(),
                ));
            }

            let reported = canonicalize_observations(context, &input.current)?;
            if reported.is_empty() {
                return Err(StoreError::InvalidInput(
                    "write resource set is required".to_string(),
                ));
            }
            validate_operation_start(&input.operation, &reported)
                .map_err(|error| StoreError::InvalidInput(error.to_string()))?;
            let requested = reported
                .iter()
                .map(|observation| observation.resource().clone())
                .collect::<Vec<_>>();
            let active = active_task_lease(transaction, context)?;
            let pending = pending_task_request(transaction, context)?;
            let mut union = active
                .as_ref()
                .map(|lease| lease.resources.clone())
                .unwrap_or_default();
            if let Some((_, resources)) = &pending {
                merge_resource_keys(&mut union, resources);
            }
            merge_resource_keys(&mut union, &requested);
            let other_agent_task_has_lease =
                agent_slot_occupied(transaction, context, Some(&context.task_id))?;

            let mut conflicts = conflicting_owners(
                transaction,
                &context.workspace_id,
                &context.task_id,
                &union,
                None,
            )?;
            conflicts.extend(queued_conflicting_owners(
                transaction,
                &context.workspace_id,
                &context.task_id,
                &union,
            )?);
            if pending.is_some() || other_agent_task_has_lease || !conflicts.is_empty() {
                if !has_historical_evidence(transaction, context, &reported)? {
                    let mut lease_batch_ids = active
                        .as_ref()
                        .map(|lease| vec![lease.batch_id.clone()])
                        .unwrap_or_default();
                    if let Some((batch_id, _)) = &pending {
                        lease_batch_ids.push(batch_id.clone());
                    }
                    return Ok(WritePrepareResult::RereadRequired { lease_batch_ids });
                }
                if active
                    .as_ref()
                    .is_some_and(|lease| lease.in_flight_count != 0)
                {
                    return Ok(WritePrepareResult::Denied {
                        reason_code: "lease_growth_while_write_in_flight".to_string(),
                    });
                }
                if active.is_some() {
                    release_all_task_leases(transaction, context)?;
                    promote_waiters(transaction, &context.observed_at)?;
                }
                if active.is_none()
                    && let Some((batch_id, resources)) = &pending
                    && *resources == union
                {
                    append_audit(
                        transaction,
                        context,
                        "lease.queue_reused",
                        json!({
                            "batch_id": batch_id,
                            "resources": union,
                        }),
                    )?;
                    return Ok(WritePrepareResult::Queued {
                        batch_id: batch_id.clone(),
                    });
                }
                let queued =
                    enqueue_request(transaction, context, &union, &input.request_expires_at)?;
                promote_waiters(transaction, &context.observed_at)?;
                append_audit(
                    transaction,
                    context,
                    "lease.queued",
                    json!({
                        "batch_id": queued.batch_id,
                        "queue_position": queued.queue_position,
                        "resources": union,
                        "conflicts": conflicts,
                    }),
                )?;
                return Ok(WritePrepareResult::Queued {
                    batch_id: queued.batch_id,
                });
            }

            let current = current_observations(transaction, &requested)?;
            if !observations_same_state(&current, &reported)?
                || !has_current_evidence(transaction, context, &current, None)?
            {
                return Ok(WritePrepareResult::RereadRequired {
                    lease_batch_ids: active
                        .as_ref()
                        .map(|lease| vec![lease.batch_id.clone()])
                        .unwrap_or_default(),
                });
            }
            let lease = ensure_active_lease(
                transaction,
                context,
                active,
                &union,
                &input.lease_expires_at,
            )?;
            for resource in &union {
                revoke_overlapping(transaction, context, resource, Some(&context.task_id))?;
            }
            let (attempt_id, permit_id) =
                prepare_write_attempt(transaction, context, input, &current, &lease)?;
            append_audit(
                transaction,
                context,
                "write.prepared",
                json!({
                    "batch_id": lease.batch_id,
                    "lease_version": lease.version,
                    "write_attempt_id": attempt_id,
                    "resources": requested,
                }),
            )?;
            Ok(WritePrepareResult::Ready {
                attempt_id,
                permit_id,
                lease_batch_ids: vec![lease.batch_id],
            })
        })
    }

    pub fn lease_activate(
        &mut self,
        context: &CommandContext,
        input: &LeaseActivateInput,
    ) -> StoreResult<LeaseActivateResult> {
        self.accepted_command(context, "lease.activate", input, |transaction| {
            require_task_owner(transaction, context, true)?;
            let settings = task_coordination_settings(transaction, context)?;
            validate_future_window(
                &context.observed_at,
                &input.lease_expires_at,
                setting_seconds(settings.lease_expiry_seconds)?,
                "lease_expires_at",
            )?;
            let request = lease_request_row(transaction, context, &input.batch_id)?
                .ok_or_else(|| StoreError::NotFound("lease request not found".to_string()))?;
            if request.state != LeaseRequestState::Offered
                || request.offer_id.as_deref() != Some(input.offer_id.as_str())
                || request.version != input.version
            {
                return Ok(LeaseActivateResult {
                    batch_id: input.batch_id.clone(),
                    active: false,
                });
            }
            let offered_at = request.offered_at.as_deref().ok_or_else(|| {
                StoreError::Corrupt("offered request has no offered_at".to_string())
            })?;
            let observations = current_observations(transaction, &request.resources)?;
            if !has_current_evidence(transaction, context, &observations, Some(offered_at))? {
                return Ok(LeaseActivateResult {
                    batch_id: input.batch_id.clone(),
                    active: false,
                });
            }
            let conflicts = conflicting_owners(
                transaction,
                &context.workspace_id,
                &context.task_id,
                &request.resources,
                Some(&input.batch_id),
            )?;
            if agent_slot_occupied(transaction, context, None)? {
                return Ok(LeaseActivateResult {
                    batch_id: input.batch_id.clone(),
                    active: false,
                });
            }
            if !conflicts.is_empty() || active_task_lease(transaction, context)?.is_some() {
                return Ok(LeaseActivateResult {
                    batch_id: input.batch_id.clone(),
                    active: false,
                });
            }
            let lease_version = request
                .version
                .checked_add(1)
                .ok_or_else(|| StoreError::Corrupt("lease request version overflow".to_string()))?;
            insert_active_lease(
                transaction,
                context,
                &input.batch_id,
                lease_version,
                &request.resources,
                &input.lease_expires_at,
            )?;
            for resource in &request.resources {
                revoke_overlapping(transaction, context, resource, Some(&context.task_id))?;
            }
            let updated = transaction.execute(
                "UPDATE lease_requests
                 SET state = 'activated', version = ?4, updated_at = ?5
                 WHERE workspace_id = ?1 AND task_id = ?2 AND batch_id = ?3
                   AND state = 'offered' AND version = ?6",
                params![
                    &context.workspace_id,
                    &context.task_id,
                    &input.batch_id,
                    lease_version,
                    &context.observed_at,
                    input.version,
                ],
            )?;
            if updated != 1 {
                return Err(StoreError::InvalidState(
                    "offer activation lost its version CAS".to_string(),
                ));
            }
            append_audit(
                transaction,
                context,
                "lease.activated",
                json!({
                    "batch_id": input.batch_id,
                    "lease_version": lease_version,
                }),
            )?;
            Ok(LeaseActivateResult {
                batch_id: input.batch_id.clone(),
                active: true,
            })
        })
    }

    pub fn lease_release(
        &mut self,
        context: &CommandContext,
        input: &LeaseReleaseInput,
    ) -> StoreResult<LeaseReleaseResult> {
        self.accepted_command(context, "lease.release", input, |transaction| {
            require_task_owner(transaction, context, false)?;
            let lease = active_task_lease(transaction, context)?
                .filter(|lease| lease.batch_id == input.batch_id)
                .ok_or_else(|| StoreError::NotFound("active lease not found".to_string()))?;
            let status = if lease.in_flight_count == 0 {
                transaction.execute(
                    "DELETE FROM active_leases WHERE batch_id = ?1",
                    [&input.batch_id],
                )?;
                promote_waiters(transaction, &context.observed_at)?;
                LeaseReleaseStatus::Released
            } else {
                transaction.execute(
                    "UPDATE active_leases SET state = 'draining', release_pending = 1
                     WHERE batch_id = ?1",
                    [&input.batch_id],
                )?;
                LeaseReleaseStatus::Deferred
            };
            append_audit(
                transaction,
                context,
                "lease.release",
                json!({
                    "batch_id": input.batch_id,
                    "status": status,
                }),
            )?;
            Ok(LeaseReleaseResult {
                batch_id: input.batch_id.clone(),
                status,
            })
        })
    }
}

#[derive(Debug, Clone)]
struct ActiveLeaseView {
    batch_id: String,
    version: u64,
    state: String,
    expires_at: String,
    resources: Vec<ResourceKey>,
    in_flight_count: u64,
}

#[derive(Debug)]
struct QueuedRequest {
    batch_id: String,
    queue_position: u64,
}

#[derive(Debug)]
struct LeaseRequestView {
    state: LeaseRequestState,
    version: u64,
    offer_id: Option<String>,
    offered_at: Option<String>,
    resources: Vec<ResourceKey>,
}

fn canonicalize_observations(
    context: &CommandContext,
    observations: &[ResourceObservation],
) -> StoreResult<Vec<ResourceObservation>> {
    let mut ordered = observations.to_vec();
    ordered.sort_by(|left, right| left.resource().cmp(right.resource()));
    let mut seen = BTreeSet::new();
    for observation in &ordered {
        if !observation.has_matching_resource_kind()
            || observation.resource().workspace_id != context.workspace_id
        {
            return Err(StoreError::InvalidInput(
                "resource observation kind or workspace mismatch".to_string(),
            ));
        }
        let resource = observation.resource();
        if !seen.insert((
            resource.workspace_id.clone(),
            resource.kind,
            resource.resource_id.clone(),
        )) {
            return Err(StoreError::InvalidInput(
                "duplicate resource observation".to_string(),
            ));
        }
    }
    Ok(ordered)
}

fn observations_same_state(
    left: &[ResourceObservation],
    right: &[ResourceObservation],
) -> StoreResult<bool> {
    if left.len() != right.len() {
        return Ok(false);
    }
    for (left, right) in left.iter().zip(right) {
        if left.resource() != right.resource()
            || observation_state_json(left)? != observation_state_json(right)?
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn has_current_evidence(
    transaction: &Transaction<'_>,
    context: &CommandContext,
    observations: &[ResourceObservation],
    minimum_read_started_at: Option<&str>,
) -> StoreResult<bool> {
    for observation in observations {
        let resource = observation.resource();
        let exists = transaction.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM resource_evidence
                WHERE workspace_id = ?1 AND task_id = ?2 AND resource_id = ?3
                  AND valid = 1 AND complete = 1 AND stable = 1 AND exact = 1
                  AND observation_json = ?4
                  AND (
                    ?5 IS NULL
                    OR (source_kind = 'read' AND read_started_at >= ?5)
                  )
            )",
            params![
                &context.workspace_id,
                &context.task_id,
                &resource.resource_id,
                serialize_json(observation)?,
                minimum_read_started_at,
            ],
            |row| row.get::<_, bool>(0),
        )?;
        if !exists {
            return Ok(false);
        }
    }
    Ok(true)
}

fn has_historical_evidence(
    transaction: &Transaction<'_>,
    context: &CommandContext,
    observations: &[ResourceObservation],
) -> StoreResult<bool> {
    for observation in observations {
        let resource = observation.resource();
        let mut statement = transaction.prepare(
            "SELECT observation_json FROM resource_evidence
             WHERE workspace_id = ?1 AND task_id = ?2 AND resource_id = ?3
               AND complete = 1 AND stable = 1 AND exact = 1",
        )?;
        let rows = statement.query_map(
            params![
                &context.workspace_id,
                &context.task_id,
                &resource.resource_id,
            ],
            |row| row.get::<_, String>(0),
        )?;
        let mut matched = false;
        for stored in rows {
            let stored: ResourceObservation = deserialize_json(&stored?)?;
            if observations_same_state(
                std::slice::from_ref(&stored),
                std::slice::from_ref(observation),
            )? {
                matched = true;
                break;
            }
        }
        if !matched {
            return Ok(false);
        }
    }
    Ok(true)
}

fn current_observations(
    transaction: &Transaction<'_>,
    resources: &[ResourceKey],
) -> StoreResult<Vec<ResourceObservation>> {
    let mut observations = Vec::with_capacity(resources.len());
    for requested in resources {
        let row = transaction
            .query_row(
                "SELECT kind, canonical_path, state_json, generation
                 FROM resources WHERE workspace_id = ?1 AND resource_id = ?2",
                params![&requested.workspace_id, &requested.resource_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, u64>(3)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| StoreError::NotFound("leased resource not found".to_string()))?;
        let kind = parse_resource_kind(&row.0)?;
        if kind != requested.kind {
            return Err(StoreError::Corrupt(
                "leased resource kind changed".to_string(),
            ));
        }
        let resource = ResourceKey {
            workspace_id: requested.workspace_id.clone(),
            resource_id: requested.resource_id.clone(),
            kind,
            canonical_path: requested.canonical_path.clone(),
        };
        let observation = match kind {
            ResourceKind::Object => ResourceObservation::Object {
                resource,
                observed: deserialize_json::<ObjectState>(&row.2)?,
                generation: row.3,
            },
            ResourceKind::Entry => ResourceObservation::Entry {
                resource,
                observed: deserialize_json::<EntryState>(&row.2)?,
                generation: row.3,
            },
            ResourceKind::DirectoryTree => ResourceObservation::DirectoryTree {
                resource,
                observed: deserialize_json::<DirectoryTreeState>(&row.2)?,
                generation: row.3,
            },
        };
        observations.push(observation);
    }
    observations.sort_by(|left, right| left.resource().cmp(right.resource()));
    Ok(observations)
}

fn merge_resource_keys(target: &mut Vec<ResourceKey>, additions: &[ResourceKey]) {
    for addition in additions {
        if !target.iter().any(|resource| {
            resource.workspace_id == addition.workspace_id
                && resource.kind == addition.kind
                && resource.resource_id == addition.resource_id
        }) {
            target.push(addition.clone());
        }
    }
    target.sort();
}

fn active_task_lease(
    transaction: &Transaction<'_>,
    context: &CommandContext,
) -> StoreResult<Option<ActiveLeaseView>> {
    let row = transaction
        .query_row(
            "SELECT batch_id, version, state, expires_at
             FROM active_leases
             WHERE workspace_id = ?1 AND task_id = ?2",
            params![&context.workspace_id, &context.task_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()?;
    let Some((batch_id, version, state, expires_at)) = row else {
        return Ok(None);
    };
    let resources = lease_resources(transaction, &batch_id)?;
    let in_flight_count = transaction.query_row(
        "SELECT COUNT(*) FROM lease_resources
         WHERE batch_id = ?1 AND in_flight_attempt_id IS NOT NULL",
        [&batch_id],
        |row| row.get::<_, u64>(0),
    )?;
    Ok(Some(ActiveLeaseView {
        batch_id,
        version,
        state,
        expires_at,
        resources,
        in_flight_count,
    }))
}

fn agent_slot_occupied(
    transaction: &Transaction<'_>,
    context: &CommandContext,
    excluding_task_id: Option<&str>,
) -> StoreResult<bool> {
    let occupied = transaction.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM active_leases
            WHERE workspace_id = ?1 AND agent_id = ?2
              AND (?3 IS NULL OR task_id <> ?3)
        )",
        params![&context.workspace_id, &context.agent_id, excluding_task_id],
        |row| row.get::<_, bool>(0),
    )?;
    Ok(occupied)
}

fn lease_resources(transaction: &Transaction<'_>, batch_id: &str) -> StoreResult<Vec<ResourceKey>> {
    let mut statement = transaction.prepare(
        "SELECT workspace_id, resource_id, resource_kind, canonical_path
         FROM lease_resources WHERE batch_id = ?1",
    )?;
    let rows = statement.query_map([batch_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    let mut resources = Vec::new();
    for row in rows {
        let row = row?;
        resources.push(ResourceKey {
            workspace_id: row.0,
            resource_id: row.1,
            kind: parse_resource_kind(&row.2)?,
            canonical_path: row.3,
        });
    }
    resources.sort();
    Ok(resources)
}

fn conflicting_owners(
    transaction: &Transaction<'_>,
    workspace_id: &str,
    task_id: &str,
    resources: &[ResourceKey],
    exclude_offer_batch: Option<&str>,
) -> StoreResult<Vec<String>> {
    let mut conflicts = BTreeSet::new();
    let mut lease_statement = transaction.prepare(
        "SELECT al.task_id, lr.resource_id, lr.resource_kind, lr.canonical_path
         FROM active_leases al
         JOIN lease_resources lr ON lr.batch_id = al.batch_id
         WHERE al.workspace_id = ?1 AND al.task_id <> ?2",
    )?;
    let lease_rows = lease_statement.query_map(params![workspace_id, task_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    for row in lease_rows {
        let row = row?;
        let candidate = ResourceKey {
            workspace_id: workspace_id.to_string(),
            resource_id: row.1,
            kind: parse_resource_kind(&row.2)?,
            canonical_path: row.3,
        };
        if resources
            .iter()
            .any(|resource| resource_keys_overlap(resource, &candidate))
        {
            conflicts.insert(format!("task:{}", row.0));
        }
    }
    drop(lease_statement);

    let mut offer_statement = transaction.prepare(
        "SELECT request.task_id, request.batch_id, resource.resource_id,
                resource.resource_kind, resource.canonical_path
         FROM lease_requests request
         JOIN lease_request_resources resource ON resource.batch_id = request.batch_id
         WHERE request.workspace_id = ?1 AND request.task_id <> ?2
           AND request.state = 'offered'",
    )?;
    let offer_rows = offer_statement.query_map(params![workspace_id, task_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;
    for row in offer_rows {
        let row = row?;
        if exclude_offer_batch.is_some_and(|batch| batch == row.1) {
            continue;
        }
        let candidate = ResourceKey {
            workspace_id: workspace_id.to_string(),
            resource_id: row.2,
            kind: parse_resource_kind(&row.3)?,
            canonical_path: row.4,
        };
        if resources
            .iter()
            .any(|resource| resource_keys_overlap(resource, &candidate))
        {
            conflicts.insert(format!("offer:{}", row.0));
        }
    }
    Ok(conflicts.into_iter().collect())
}

fn pending_task_request(
    transaction: &Transaction<'_>,
    context: &CommandContext,
) -> StoreResult<Option<(String, Vec<ResourceKey>)>> {
    let batch_id = transaction
        .query_row(
            "SELECT batch_id FROM lease_requests
             WHERE workspace_id = ?1 AND task_id = ?2 AND state IN ('queued', 'offered')",
            params![&context.workspace_id, &context.task_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    batch_id
        .map(|batch_id| {
            request_resources(transaction, &batch_id).map(|resources| (batch_id, resources))
        })
        .transpose()
}

fn queued_conflicting_owners(
    transaction: &Transaction<'_>,
    workspace_id: &str,
    task_id: &str,
    resources: &[ResourceKey],
) -> StoreResult<Vec<String>> {
    let mut statement = transaction.prepare(
        "SELECT request.task_id, resource.resource_id, resource.resource_kind,
                resource.canonical_path
         FROM lease_requests request
         JOIN lease_request_resources resource ON resource.batch_id = request.batch_id
         WHERE request.workspace_id = ?1 AND request.task_id <> ?2
           AND request.state = 'queued'",
    )?;
    let rows = statement.query_map(params![workspace_id, task_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    let mut conflicts = BTreeSet::new();
    for row in rows {
        let row = row?;
        let candidate = ResourceKey {
            workspace_id: workspace_id.to_string(),
            resource_id: row.1,
            kind: parse_resource_kind(&row.2)?,
            canonical_path: row.3,
        };
        if resources
            .iter()
            .any(|resource| resource_keys_overlap(resource, &candidate))
        {
            conflicts.insert(format!("queue:{}", row.0));
        }
    }
    Ok(conflicts.into_iter().collect())
}

fn ensure_active_lease(
    transaction: &Transaction<'_>,
    context: &CommandContext,
    active: Option<ActiveLeaseView>,
    resources: &[ResourceKey],
    expires_at: &str,
) -> StoreResult<ActiveLeaseView> {
    if let Some(active) = active {
        if active.state != "active" || active.expires_at <= context.observed_at {
            return Err(StoreError::InvalidState(
                "lease is draining or expired".to_string(),
            ));
        }
        let additions = resources
            .iter()
            .filter(|resource| {
                !active.resources.iter().any(|existing| {
                    existing.workspace_id == resource.workspace_id
                        && existing.kind == resource.kind
                        && existing.resource_id == resource.resource_id
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        if !additions.is_empty() && active.in_flight_count != 0 {
            return Err(StoreError::InvalidState(
                "cannot expand a lease while a write is in flight".to_string(),
            ));
        }
        let mut version = active.version;
        for resource in &additions {
            insert_lease_resource(transaction, &active.batch_id, resource)?;
        }
        if !additions.is_empty() || active.expires_at != expires_at {
            version = version
                .checked_add(1)
                .ok_or_else(|| StoreError::Corrupt("lease version overflow".to_string()))?;
            transaction.execute(
                "UPDATE active_leases SET version = ?2, expires_at = ?3
                 WHERE batch_id = ?1",
                params![&active.batch_id, version, expires_at],
            )?;
        }
        return Ok(ActiveLeaseView {
            batch_id: active.batch_id,
            version,
            state: active.state,
            expires_at: expires_at.to_string(),
            resources: resources.to_vec(),
            in_flight_count: active.in_flight_count,
        });
    }
    let batch_id = Uuid::new_v4().to_string();
    insert_active_lease(transaction, context, &batch_id, 1, resources, expires_at)?;
    Ok(ActiveLeaseView {
        batch_id,
        version: 1,
        state: "active".to_string(),
        expires_at: expires_at.to_string(),
        resources: resources.to_vec(),
        in_flight_count: 0,
    })
}

fn insert_active_lease(
    transaction: &Transaction<'_>,
    context: &CommandContext,
    batch_id: &str,
    version: u64,
    resources: &[ResourceKey],
    expires_at: &str,
) -> StoreResult<()> {
    let mode = if resources
        .iter()
        .any(|resource| resource.kind == ResourceKind::DirectoryTree)
    {
        "exclusive_directory"
    } else {
        "exclusive_write"
    };
    transaction.execute(
        "INSERT INTO active_leases (
            batch_id, workspace_id, task_id, agent_id, mode, state, version,
            acquired_at, expires_at, release_pending
         ) VALUES (?1, ?2, ?3, ?4, ?5, 'active', ?6, ?7, ?8, 0)",
        params![
            batch_id,
            &context.workspace_id,
            &context.task_id,
            &context.agent_id,
            mode,
            version,
            &context.observed_at,
            expires_at,
        ],
    )?;
    for resource in resources {
        insert_lease_resource(transaction, batch_id, resource)?;
    }
    Ok(())
}

fn insert_lease_resource(
    transaction: &Transaction<'_>,
    batch_id: &str,
    resource: &ResourceKey,
) -> StoreResult<()> {
    let generation = transaction
        .query_row(
            "SELECT generation FROM resources
             WHERE workspace_id = ?1 AND resource_id = ?2",
            params![&resource.workspace_id, &resource.resource_id],
            |row| row.get::<_, u64>(0),
        )
        .optional()?
        .ok_or_else(|| StoreError::NotFound("lease resource not found".to_string()))?;
    transaction.execute(
        "INSERT INTO lease_resources (
            batch_id, workspace_id, resource_id, resource_kind, canonical_path,
            acquired_generation, in_flight_attempt_id
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL)",
        params![
            batch_id,
            &resource.workspace_id,
            &resource.resource_id,
            resource_kind_name(resource.kind),
            &resource.canonical_path,
            generation,
        ],
    )?;
    Ok(())
}

fn enqueue_request(
    transaction: &Transaction<'_>,
    context: &CommandContext,
    resources: &[ResourceKey],
    expires_at: &str,
) -> StoreResult<QueuedRequest> {
    let batch_id = Uuid::new_v4().to_string();
    transaction.execute(
        "UPDATE lease_requests SET state = 'superseded', superseded_by = ?3,
            version = version + 1, updated_at = ?4
         WHERE workspace_id = ?1 AND task_id = ?2 AND state IN ('queued', 'offered')",
        params![
            &context.workspace_id,
            &context.task_id,
            &batch_id,
            &context.observed_at,
        ],
    )?;
    let sequence = transaction
        .query_row(
            "SELECT value FROM coordination_sequences WHERE name = 'lease_queue'",
            [],
            |row| row.get::<_, u64>(0),
        )?
        .checked_add(1)
        .ok_or_else(|| StoreError::Corrupt("lease queue sequence overflow".to_string()))?;
    transaction.execute(
        "UPDATE coordination_sequences SET value = ?1 WHERE name = 'lease_queue'",
        [sequence],
    )?;
    let mode = if resources
        .iter()
        .any(|resource| resource.kind == ResourceKind::DirectoryTree)
    {
        "exclusive_directory"
    } else {
        "exclusive_write"
    };
    transaction.execute(
        "INSERT INTO lease_requests (
            batch_id, workspace_id, task_id, agent_id, mode, state, version,
            queue_sequence, offer_id, offered_at, offer_expires_at, expires_at,
            superseded_by, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, 'queued', 1, ?6,
                   NULL, NULL, NULL, ?7, NULL, ?8, ?8)",
        params![
            &batch_id,
            &context.workspace_id,
            &context.task_id,
            &context.agent_id,
            mode,
            sequence,
            expires_at,
            &context.observed_at,
        ],
    )?;
    for resource in resources {
        transaction.execute(
            "INSERT INTO lease_request_resources (
                batch_id, workspace_id, resource_id, resource_kind, canonical_path
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                &batch_id,
                &resource.workspace_id,
                &resource.resource_id,
                resource_kind_name(resource.kind),
                &resource.canonical_path,
            ],
        )?;
    }
    Ok(QueuedRequest {
        batch_id,
        queue_position: sequence,
    })
}

fn lease_request_row(
    transaction: &Transaction<'_>,
    context: &CommandContext,
    batch_id: &str,
) -> StoreResult<Option<LeaseRequestView>> {
    let row = transaction
        .query_row(
            "SELECT state, version, offer_id, offered_at
             FROM lease_requests
             WHERE workspace_id = ?1 AND task_id = ?2 AND batch_id = ?3",
            params![&context.workspace_id, &context.task_id, batch_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .optional()?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(LeaseRequestView {
        state: parse_request_state(&row.0)?,
        version: row.1,
        offer_id: row.2,
        offered_at: row.3,
        resources: request_resources(transaction, batch_id)?,
    }))
}

fn request_resources(
    transaction: &Transaction<'_>,
    batch_id: &str,
) -> StoreResult<Vec<ResourceKey>> {
    let mut statement = transaction.prepare(
        "SELECT workspace_id, resource_id, resource_kind, canonical_path
         FROM lease_request_resources WHERE batch_id = ?1",
    )?;
    let rows = statement.query_map([batch_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    let mut resources = Vec::new();
    for row in rows {
        let row = row?;
        resources.push(ResourceKey {
            workspace_id: row.0,
            resource_id: row.1,
            kind: parse_resource_kind(&row.2)?,
            canonical_path: row.3,
        });
    }
    resources.sort();
    Ok(resources)
}

fn promote_waiters(transaction: &Transaction<'_>, now: &str) -> StoreResult<()> {
    expire_offers(transaction, now)?;
    let now_value = parse_timestamp(now)?;
    let mut statement = transaction.prepare(
        "SELECT request.batch_id, request.workspace_id, request.task_id, request.agent_id,
                request.expires_at, task.status, task.settings_json
         FROM lease_requests AS request
         JOIN tasks AS task
           ON task.workspace_id = request.workspace_id AND task.task_id = request.task_id
         WHERE request.state = 'queued'
         ORDER BY request.queue_sequence",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, String>(6)?,
        ))
    })?;
    let queued = rows.collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    let mut blocked_resources = Vec::<ResourceKey>::new();
    for (batch_id, workspace_id, task_id, agent_id, expires_at, task_status, settings_json) in
        queued
    {
        let resources = request_resources(transaction, &batch_id)?;
        if task_status != "active" || parse_timestamp(&expires_at)? <= now_value {
            transaction.execute(
                "UPDATE lease_requests SET state = 'expired', updated_at = ?2
                 WHERE batch_id = ?1 AND state = 'queued'",
                params![&batch_id, now],
            )?;
            continue;
        }
        let earlier_overlap = resources.iter().any(|resource| {
            blocked_resources
                .iter()
                .any(|earlier| resource_keys_overlap(resource, earlier))
        });
        let agent_slot_full = transaction.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM active_leases
                WHERE workspace_id = ?1 AND agent_id = ?2
            )",
            params![&workspace_id, &agent_id],
            |row| row.get::<_, bool>(0),
        )?;
        let task_has_lease = transaction.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM active_leases
                WHERE workspace_id = ?1 AND task_id = ?2
            )",
            params![&workspace_id, &task_id],
            |row| row.get::<_, bool>(0),
        )?;
        let agent_offer_pending = transaction.query_row(
            "SELECT EXISTS(
                    SELECT 1 FROM lease_requests
                    WHERE workspace_id = ?1 AND agent_id = ?2 AND state = 'offered'
                )",
            params![&workspace_id, &agent_id],
            |row| row.get::<_, bool>(0),
        )?;
        let conflicts = conflicting_owners(
            transaction,
            &workspace_id,
            &task_id,
            &resources,
            Some(&batch_id),
        )?;
        if earlier_overlap
            || agent_slot_full
            || task_has_lease
            || agent_offer_pending
            || !conflicts.is_empty()
        {
            blocked_resources.extend(resources);
            continue;
        }
        let settings: CoordinationSettings = deserialize_json(&settings_json)?;
        settings.validate().map_err(StoreError::Corrupt)?;
        let offer_id = Uuid::new_v4().to_string();
        let offer_limit =
            now_value + Duration::seconds(setting_seconds(settings.offer_ttl_seconds)?);
        let request_limit = parse_timestamp(&expires_at)?;
        let offer_expires_at = format_timestamp(offer_limit.min(request_limit))?;
        transaction.execute(
            "UPDATE lease_requests
             SET state = 'offered', version = version + 1, offer_id = ?2,
                 offered_at = ?3, offer_expires_at = ?4, updated_at = ?3
             WHERE batch_id = ?1 AND state = 'queued'",
            params![&batch_id, &offer_id, now, &offer_expires_at],
        )?;
    }
    Ok(())
}

fn prepare_write_attempt(
    transaction: &Transaction<'_>,
    context: &CommandContext,
    input: &WritePrepareInput,
    current: &[ResourceObservation],
    lease: &ActiveLeaseView,
) -> StoreResult<(String, String)> {
    for observation in current {
        let resource = observation.resource();
        if !lease.resources.iter().any(|leased| {
            leased.workspace_id == resource.workspace_id
                && leased.kind == resource.kind
                && leased.resource_id == resource.resource_id
        }) {
            return Err(StoreError::InvalidState(
                "write target is not covered by the active lease".to_string(),
            ));
        }
        let slot = transaction.query_row(
            "SELECT in_flight_attempt_id FROM lease_resources
             WHERE batch_id = ?1 AND resource_id = ?2",
            params![&lease.batch_id, &resource.resource_id],
            |row| row.get::<_, Option<String>>(0),
        )?;
        if slot.is_some() {
            return Err(StoreError::InvalidState(
                "write target already has an in-flight operation".to_string(),
            ));
        }
    }
    for observation in current {
        revoke_overlapping(
            transaction,
            context,
            observation.resource(),
            Some(&context.task_id),
        )?;
    }
    let attempt_id = Uuid::new_v4().to_string();
    let permit_id = Uuid::new_v4().to_string();
    transaction.execute(
        "INSERT INTO write_attempts (
            attempt_id, permit_id, workspace_id, task_id, invocation_id, batch_id,
            operation_json, start_observations_json, status,
            started_at, deadline, completed_at, terminal_result_json
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                   'executing', ?9, ?10, NULL, NULL)",
        params![
            &attempt_id,
            &permit_id,
            &context.workspace_id,
            &context.task_id,
            &input.invocation_id,
            &lease.batch_id,
            serialize_json(&input.operation)?,
            serialize_json(current)?,
            &context.observed_at,
            &input.attempt_deadline,
        ],
    )?;
    for observation in current {
        let updated = transaction.execute(
            "UPDATE lease_resources SET in_flight_attempt_id = ?3
             WHERE batch_id = ?1 AND resource_id = ?2
               AND in_flight_attempt_id IS NULL",
            params![
                &lease.batch_id,
                &observation.resource().resource_id,
                &attempt_id,
            ],
        )?;
        if updated != 1 {
            return Err(StoreError::InvalidState(
                "write slot acquisition lost its CAS".to_string(),
            ));
        }
    }
    Ok((attempt_id, permit_id))
}

impl Store {
    pub fn write_complete(
        &mut self,
        context: &CommandContext,
        input: &WriteCompleteInput,
    ) -> StoreResult<WriteCompleteResult> {
        self.accepted_command(context, "write.complete", input, |transaction| {
            require_task_owner(transaction, context, false)?;
            let attempt = transaction
                .query_row(
                    "SELECT invocation_id, permit_id, batch_id, start_observations_json, status,
                            operation_json
                     FROM write_attempts
                     WHERE attempt_id = ?1 AND workspace_id = ?2 AND task_id = ?3",
                    params![&input.attempt_id, &context.workspace_id, &context.task_id,],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, String>(5)?,
                        ))
                    },
                )
                .optional()?
                .ok_or_else(|| StoreError::NotFound("write attempt not found".to_string()))?;
            if attempt.0 != input.invocation_id || attempt.1 != input.permit_id {
                return Err(StoreError::Ownership(
                    "write invocation or permit does not match prepare".to_string(),
                ));
            }
            if attempt.4 != "executing" {
                return Err(StoreError::InvalidState(
                    "write attempt is terminal; only its original completion receipt may replay"
                        .to_string(),
                ));
            }
            let start: Vec<ResourceObservation> = deserialize_json(&attempt.3)?;
            let actual = canonicalize_observations(context, &input.post_resources)?;
            let expected = canonicalize_observations(context, &input.expected_post_resources)?;
            let operation: MutationOperation = deserialize_json(&attempt.5)?;
            let success_matches = !expected.is_empty()
                && validate_operation_transition(&operation, &start, &expected, &actual).is_ok();
            let (status, own_write_evidence) =
                if input.terminal == WriteTerminal::Success && success_matches {
                    (
                        WriteResultStatus::Completed,
                        Some(sync_observations(transaction, context, &actual)?),
                    )
                } else if input.terminal == WriteTerminal::FailedKnown
                    && observations_same_state(&start, &actual)?
                {
                    (WriteResultStatus::Failed, None)
                } else {
                    invalidate_uncertain_write(transaction, context, &start)?;
                    (WriteResultStatus::Uncertain, None)
                };
            let updated = transaction.execute(
                "UPDATE write_attempts
                 SET status = ?2, completed_at = ?3, terminal_result_json = ?4
                 WHERE attempt_id = ?1 AND status = 'executing'",
                params![
                    &input.attempt_id,
                    write_result_name(status),
                    &context.observed_at,
                    serialize_json(input)?,
                ],
            )?;
            if updated != 1 {
                return Err(StoreError::InvalidState(
                    "write completion lost its one-shot CAS".to_string(),
                ));
            }
            if let Some(observations) = own_write_evidence {
                insert_own_write_evidence(transaction, context, &input.attempt_id, &observations)?;
            }
            transaction.execute(
                "UPDATE lease_resources SET in_flight_attempt_id = NULL
                 WHERE in_flight_attempt_id = ?1",
                [&input.attempt_id],
            )?;

            let mut released = false;
            if status == WriteResultStatus::Uncertain {
                transaction.execute(
                    "UPDATE active_leases SET state = 'draining', release_pending = 1
                     WHERE batch_id = ?1",
                    [&attempt.2],
                )?;
            } else {
                let release_pending = transaction
                    .query_row(
                        "SELECT release_pending FROM active_leases WHERE batch_id = ?1",
                        [&attempt.2],
                        |row| row.get::<_, bool>(0),
                    )
                    .optional()?
                    .unwrap_or(false);
                let remaining = transaction.query_row(
                    "SELECT COUNT(*) FROM lease_resources
                     WHERE batch_id = ?1 AND in_flight_attempt_id IS NOT NULL",
                    [&attempt.2],
                    |row| row.get::<_, u64>(0),
                )?;
                if release_pending && remaining == 0 {
                    transaction.execute(
                        "DELETE FROM active_leases WHERE batch_id = ?1",
                        [&attempt.2],
                    )?;
                    released = true;
                }
            }
            if finish_draining_task(transaction, context)? {
                released = true;
            }
            if released {
                promote_waiters(transaction, &context.observed_at)?;
            }
            append_audit(
                transaction,
                context,
                "write.completed",
                json!({
                    "attempt_id": input.attempt_id,
                    "batch_id": attempt.2,
                    "status": write_result_name(status),
                    "error": input.error,
                }),
            )?;
            Ok(WriteCompleteResult {
                attempt_id: input.attempt_id.clone(),
                status,
            })
        })
    }
}

fn write_result_name(status: WriteResultStatus) -> &'static str {
    match status {
        WriteResultStatus::Completed => "completed",
        WriteResultStatus::Failed => "failed",
        WriteResultStatus::Uncertain => "uncertain",
    }
}

fn invalidate_uncertain_write(
    transaction: &Transaction<'_>,
    context: &CommandContext,
    resources: &[ResourceObservation],
) -> StoreResult<()> {
    for observation in resources {
        revoke_overlapping(transaction, context, observation.resource(), None)?;
    }
    Ok(())
}

fn finish_draining_task(
    transaction: &Transaction<'_>,
    context: &CommandContext,
) -> StoreResult<bool> {
    let terminal = transaction
        .query_row(
            "SELECT terminal_status FROM tasks
             WHERE workspace_id = ?1 AND task_id = ?2 AND status = 'draining'",
            params![&context.workspace_id, &context.task_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    let Some(terminal) = terminal else {
        return Ok(false);
    };
    if task_in_flight_count(transaction, context)? != 0 {
        return Ok(false);
    }
    transaction.execute(
        "DELETE FROM active_leases WHERE workspace_id = ?1 AND task_id = ?2",
        params![&context.workspace_id, &context.task_id],
    )?;
    transaction.execute(
        "UPDATE tasks SET status = ?3, updated_at = ?4
         WHERE workspace_id = ?1 AND task_id = ?2 AND status = 'draining'",
        params![
            &context.workspace_id,
            &context.task_id,
            terminal,
            &context.observed_at,
        ],
    )?;
    Ok(true)
}
