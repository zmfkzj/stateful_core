use serde_json::{Value, json};
use stateful_core::{AuthorizationInput, Decision, DecisionKind};
use stateful_store::{Store, WaitRecord};

#[derive(Debug, Clone)]
pub struct WriteAuthorizationRequest {
    pub session_id: String,
    pub workspace_id: Option<String>,
    pub action: String,
    pub path: String,
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub queue_on_conflict: bool,
    pub allow_queue_side_effects: bool,
}

#[derive(Debug, Clone)]
pub struct PolicyOutcome {
    pub decision: Decision,
    pub wait: Option<WaitQueueOutcome>,
    pub reservation: Option<WaitRecord>,
}

#[derive(Debug, Clone)]
pub struct WaitQueueOutcome {
    pub record: WaitRecord,
    pub queue_position: Option<u64>,
}

pub struct PolicyService<'a> {
    store: &'a Store,
}

impl<'a> PolicyService<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    pub fn authorize_write(
        &self,
        input: WriteAuthorizationRequest,
    ) -> Result<PolicyOutcome, String> {
        let authorization_input = match authorization_input(&input) {
            Ok(authorization_input) => authorization_input,
            Err(decision) => {
                return Ok(PolicyOutcome {
                    decision,
                    wait: None,
                    reservation: None,
                });
            }
        };

        if let Some(workspace_id) = &input.workspace_id {
            if let Some(reservation) = self
                .store
                .active_reservation(workspace_id, &input.path)
                .map_err(|error| error.to_string())?
            {
                if reservation.session_id != input.session_id {
                    return Ok(PolicyOutcome {
                        decision: Decision::deny(
                            "reservation_conflict",
                            "Write target is reserved for the next waiting session.",
                            "Call state.intent.claim for the active reservation before writing.",
                        ),
                        wait: None,
                        reservation: Some(reservation),
                    });
                }

                return Ok(PolicyOutcome {
                    decision: Decision::deny(
                        "reservation_requires_claim",
                        "Reserved writes require an explicit claim before authorization.",
                        "Reread the target, then call state.intent.claim for the reservation.",
                    ),
                    wait: None,
                    reservation: Some(reservation),
                });
            }

            if let Some(owner) = self
                .store
                .active_lease_owner(workspace_id, &input.path)
                .map_err(|error| error.to_string())?
                && owner != input.session_id
            {
                let wait = if input.allow_queue_side_effects && input.queue_on_conflict {
                    let waiter = self
                        .store
                        .enqueue_waiter(
                            &input.session_id,
                            workspace_id,
                            &input.path,
                            &input.action,
                            Some(&owner),
                        )
                        .map_err(|error| error.to_string())?;
                    let queue_position = self
                        .store
                        .queue_position(&waiter.wait_id)
                        .map_err(|error| error.to_string())?;
                    Some(WaitQueueOutcome {
                        record: waiter,
                        queue_position,
                    })
                } else {
                    None
                };

                return Ok(PolicyOutcome {
                    decision: Decision::deny(
                        "active_lease_conflict",
                        "Write target is covered by another active session lease.",
                        "Refresh current state, coordinate with the lease owner, or wait for the lease to release.",
                    ),
                    wait,
                    reservation: None,
                });
            }
        }

        let policy_state = self
            .store
            .policy_state_for_session(&input.session_id)
            .map_err(|error| error.to_string())?;
        let decision = stateful_core::authorize_action(&policy_state, authorization_input);

        Ok(PolicyOutcome {
            decision,
            wait: None,
            reservation: None,
        })
    }
}

fn authorization_input(input: &WriteAuthorizationRequest) -> Result<AuthorizationInput, Decision> {
    match input.action.as_str() {
        "write_file" => Ok(AuthorizationInput::write_file(&input.path)),
        "delete_file" => Ok(AuthorizationInput::delete_file(&input.path)),
        "rename_file" => Ok(AuthorizationInput::rename_file(
            input.old_path.as_deref().unwrap_or(input.path.as_str()),
            input.new_path.as_deref().unwrap_or(input.path.as_str()),
        )),
        "move_file" => Ok(AuthorizationInput::move_file(
            input.old_path.as_deref().unwrap_or(input.path.as_str()),
            input.new_path.as_deref().unwrap_or(input.path.as_str()),
        )),
        _ => Err(Decision::deny(
            "unsupported_action",
            "Action is not supported by the v1 authorization API.",
            "Use a supported action such as write_file.",
        )),
    }
}

pub fn policy_outcome_json(outcome: PolicyOutcome) -> Value {
    let decision = outcome.decision;
    let mut value = json!({
        "decision": match decision.decision {
            DecisionKind::Allow => "allow",
            DecisionKind::Warn => "warn",
            DecisionKind::Deny => "deny",
            DecisionKind::Error => "error",
        },
        "reason_code": decision.reason_code,
        "message": decision.message,
        "required_next_action": decision.required_next_action,
    });

    if let Some(wait) = outcome.wait {
        value["wait"] = json!({
            "wait_id": wait.record.wait_id,
            "session_id": wait.record.session_id,
            "workspace_id": wait.record.workspace_id,
            "relative_path": wait.record.relative_path,
            "action": wait.record.action,
            "status": wait.record.status,
            "queue_position": wait.queue_position,
            "blocking_session_id": wait.record.blocking_session_id,
        });
    }

    if let Some(reservation) = outcome.reservation {
        value["reservation"] = json!({
            "wait_id": reservation.wait_id,
            "session_id": reservation.session_id,
            "workspace_id": reservation.workspace_id,
            "relative_path": reservation.relative_path,
            "action": reservation.action,
            "status": reservation.status,
            "reservation_expires_at": reservation.reservation_expires_at,
        });
    }

    value
}
