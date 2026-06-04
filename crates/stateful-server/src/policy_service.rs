use stateful_core::{AuthorizationInput, Decision};
use stateful_store::{Store, WaitRecord};

#[derive(Debug, Clone)]
pub struct AuthorizationOutcome {
    pub decision: Decision,
    pub wait: Option<WaitQueueInfo>,
    pub reservation: Option<WaitRecord>,
}

#[derive(Debug, Clone)]
pub struct WaitQueueInfo {
    pub record: WaitRecord,
    pub queue_position: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct AuthorizeWriteInput {
    pub session_id: String,
    pub workspace_id: Option<String>,
    pub queue_on_conflict: bool,
    pub action: String,
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub path: String,
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
        input: AuthorizeWriteInput,
        allow_queue_side_effects: bool,
    ) -> Result<AuthorizationOutcome, String> {
        let path = input.path.clone();
        let authorization_input = match input.action.as_str() {
            "write_file" => AuthorizationInput::write_file(&path),
            "delete_file" => AuthorizationInput::delete_file(&path),
            "rename_file" => AuthorizationInput::rename_file(
                input.old_path.as_deref().unwrap_or(path.as_str()),
                input.new_path.as_deref().unwrap_or(path.as_str()),
            ),
            "move_file" => AuthorizationInput::move_file(
                input.old_path.as_deref().unwrap_or(path.as_str()),
                input.new_path.as_deref().unwrap_or(path.as_str()),
            ),
            _ => {
                return Ok(AuthorizationOutcome {
                    decision: Decision::deny(
                        "unsupported_action",
                        "Action is not supported by the v1 authorization API.",
                        "Use a supported action such as write_file.",
                    ),
                    wait: None,
                    reservation: None,
                });
            }
        };

        let mut active_reservation = None;
        if let Some(workspace_id) = &input.workspace_id {
            if let Some(reservation) = self
                .store
                .active_reservation(workspace_id, &path)
                .map_err(|error| error.to_string())?
            {
                if reservation.session_id != input.session_id {
                    return Ok(AuthorizationOutcome {
                        decision: Decision::deny(
                            "reservation_conflict",
                            "Write target is reserved for the next waiting session.",
                            "Wait for the active reservation to be claimed or expire.",
                        ),
                        wait: None,
                        reservation: Some(reservation),
                    });
                }
                active_reservation = Some(reservation);
            }

            if let Some(owner) = self
                .store
                .active_lease_owner(workspace_id, &path)
                .map_err(|error| error.to_string())?
                && owner != input.session_id
            {
                let wait = if allow_queue_side_effects && input.queue_on_conflict {
                    let waiter = self
                        .store
                        .enqueue_waiter(
                            &input.session_id,
                            workspace_id,
                            &path,
                            &input.action,
                            Some(&owner),
                        )
                        .map_err(|error| error.to_string())?;
                    let queue_position = self
                        .store
                        .queue_position(&waiter.wait_id)
                        .map_err(|error| error.to_string())?;
                    Some(WaitQueueInfo {
                        record: waiter,
                        queue_position,
                    })
                } else {
                    None
                };

                return Ok(AuthorizationOutcome {
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
        let mut reservation = active_reservation;
        if matches!(decision.decision, stateful_core::DecisionKind::Allow)
            && let Some(active) = &reservation
        {
            self.store
                .claim_reservation(&active.wait_id, &input.session_id)
                .map_err(|error| error.to_string())?;
            if let Some(workspace_id) = &input.workspace_id {
                self.store
                    .acquire_lease(&input.session_id, workspace_id, &path)
                    .map_err(|error| error.to_string())?;
            }
            if let Some(claimed) = &mut reservation {
                claimed.status = "claimed".to_string();
            }
        }

        Ok(AuthorizationOutcome {
            decision,
            wait: None,
            reservation,
        })
    }
}
