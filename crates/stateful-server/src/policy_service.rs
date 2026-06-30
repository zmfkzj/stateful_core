use stateful_core::{
    AuthorizationInput, Decision, DecisionKind, SourceKind, normalize_relative_path,
    normalized_relative_path_is_empty,
};
use stateful_store::{
    ClaimObservation, Event, ReservationRequestInput, Store, WaitRecord, WorkspaceIdentity,
};
use std::path::{Component, Path, PathBuf};

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
    pub reservation_id: Option<String>,
    pub workspace_id: Option<String>,
    pub repo_id: Option<String>,
    pub worktree_id: Option<String>,
    pub root: Option<String>,
    pub branch: Option<String>,
    pub source_kind: Option<SourceKind>,
    pub source_event: Option<String>,
    pub queue_on_conflict: bool,
    pub queue_purpose: Option<String>,
    pub action: String,
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub path: String,
    pub base_observations: Vec<BaseObservation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseObservation {
    pub path: String,
    pub exists: bool,
    pub content_hash: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ClaimReservationInput {
    pub session_id: String,
    pub workspace_id: String,
    pub wait_id: String,
    pub repo_id: Option<String>,
    pub worktree_id: Option<String>,
    pub root: Option<String>,
    pub branch: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ClaimReservationOutcome {
    pub reservation: WaitRecord,
}

#[derive(Debug, Clone)]
pub struct RequestReservationInput {
    pub session_id: String,
    pub workspace_id: String,
    pub request_id: String,
    pub repo_id: Option<String>,
    pub worktree_id: Option<String>,
    pub root: Option<String>,
    pub branch: Option<String>,
    pub action: String,
    pub path: String,
    pub purpose: String,
}

#[derive(Debug, Clone)]
pub struct RequestReservationOutcome {
    pub request_id: String,
    pub request_state: String,
    pub wait: Option<WaitQueueInfo>,
    pub reservation: Option<WaitRecord>,
}

#[derive(Debug, Clone)]
pub struct CancelReservationInput {
    pub session_id: String,
    pub workspace_id: String,
    pub request_id: String,
}

#[derive(Debug, Clone)]
pub struct CancelReservationOutcome {
    pub request_id: String,
    pub wait: WaitRecord,
}

fn workspace_identity<'a>(
    repo_id: &'a Option<String>,
    worktree_id: &'a Option<String>,
    root: &'a Option<String>,
    branch: &'a Option<String>,
) -> WorkspaceIdentity<'a> {
    WorkspaceIdentity {
        repo_id: repo_id.as_deref(),
        worktree_id: worktree_id.as_deref(),
        root: root.as_deref(),
        branch: branch.as_deref(),
    }
}

pub struct PolicyService<'a> {
    store: &'a Store,
}

fn is_multi_path_action(action: &str) -> bool {
    matches!(action, "rename_file" | "move_file")
}

fn missing_rename_or_move_paths() -> AuthorizationOutcome {
    AuthorizationOutcome {
        decision: Decision::deny(
            "missing_rename_paths",
            "Rename or move authorization requires non-empty old_path and new_path.",
            "Provide both old_path and new_path, add exact scopes for both paths to the task reservation, and acquire matching same-reservation claims before writing.",
        ),
        wait: None,
        reservation: None,
    }
}

fn can_queue_after_policy_denial(decision: &Decision) -> bool {
    matches!(
        decision.reason_code.as_str(),
        "missing_reservation" | "scope_mismatch"
    )
}

fn active_claim_conflict_decision() -> Decision {
    Decision::deny(
        "active_claim_conflict",
        "Write target is covered by another active session claim.",
        "Refresh current state, coordinate with the claim owner, or wait for the claim to release. Do not redeclare reservation or change session_id; that does not release another session's claim.",
    )
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
            "write_directory" => AuthorizationInput::write_directory(&path),
            "delete_file" => AuthorizationInput::delete_file(&path),
            "rename_file" => {
                let Some((old_path, new_path)) = self.rename_or_move_paths(&input) else {
                    return Ok(missing_rename_or_move_paths());
                };
                AuthorizationInput::rename_file(old_path, new_path)
            }
            "move_file" => {
                let Some((old_path, new_path)) = self.rename_or_move_paths(&input) else {
                    return Ok(missing_rename_or_move_paths());
                };
                AuthorizationInput::move_file(old_path, new_path)
            }
            _ => {
                return Ok(AuthorizationOutcome {
                    decision: Decision::deny(
                        "unsupported_action",
                        "Action is not supported by the v1 authorization API.",
                        "Use a supported action such as write_file or write_directory.",
                    ),
                    wait: None,
                    reservation: None,
                });
            }
        };

        let mut lazy_claimed_reservation = None;
        if let Some(workspace_id) = &input.workspace_id {
            let current_session_reservation =
                if let Some(reservation_id) = input.reservation_id.as_deref() {
                    self.supplied_session_reservation(&input, workspace_id, reservation_id)?
                } else {
                    self.current_session_reservation(&input, workspace_id)?
                };
            if let Some(reservation) = current_session_reservation {
                if self.allows_lazy_claim_on_authorize(&input)
                    && matches!(input.action.as_str(), "write_file" | "write_directory")
                    && reservation.action == input.action
                {
                    let claimed = self.claim_intent(ClaimReservationInput {
                        session_id: input.session_id.clone(),
                        workspace_id: workspace_id.clone(),
                        wait_id: reservation.wait_id.clone(),
                        repo_id: input.repo_id.clone(),
                        worktree_id: input.worktree_id.clone(),
                        root: input.root.clone(),
                        branch: input.branch.clone(),
                    })?;
                    lazy_claimed_reservation = Some(claimed.reservation);
                } else {
                    return Ok(AuthorizationOutcome {
                        decision: Decision::deny(
                            "reservation_claim_required",
                            "Write target is inside active reservation scope, but the reservation has not been claimed.",
                            "Reread the target, then call state.reservation.claim for the reservation to create the same-reservation claim before writing.",
                        ),
                        wait: None,
                        reservation: Some(reservation),
                    });
                }
            }
        }

        let policy_state = if let (Some(workspace_id), Some(reservation_id)) =
            (&input.workspace_id, input.reservation_id.as_deref())
        {
            self.store
                .policy_state_for_reservation(reservation_id, workspace_id)
                .map_err(|error| error.to_string())?
        } else if let Some(workspace_id) = &input.workspace_id {
            self.store
                .policy_state_for_session(&input.session_id, workspace_id)
                .map_err(|error| error.to_string())?
        } else {
            Default::default()
        };

        let decision = stateful_core::authorize_action(&policy_state, authorization_input);
        if decision.decision != DecisionKind::Allow {
            if can_queue_after_policy_denial(&decision) {
                if let Some(outcome) = self.queue_active_claim_conflict(
                    &input,
                    input.workspace_id.as_deref(),
                    allow_queue_side_effects,
                )? {
                    self.release_lazy_claimed_lease_if_needed(
                        &input,
                        lazy_claimed_reservation.as_ref(),
                    )?;
                    return Ok(outcome);
                }
            }
            self.release_lazy_claimed_lease_if_needed(&input, lazy_claimed_reservation.as_ref())?;
            return Ok(AuthorizationOutcome {
                decision,
                wait: None,
                reservation: None,
            });
        }

        let Some(workspace_id) = &input.workspace_id else {
            return Ok(AuthorizationOutcome {
                decision: Decision::deny(
                    "missing_claim",
                    "Write target is inside active reservation scope, but workspace is missing so claim ownership cannot be checked.",
                    "Include workspace_id and acquire the relevant same-reservation file or directory claim successfully before writing. Do not change reservation_id; that does not create same-reservation claim ownership.",
                ),
                wait: None,
                reservation: None,
            });
        };

        let reservation_conflict = self.reservation_conflict(&input, workspace_id)?;
        if let Some(reservation) = reservation_conflict {
            self.release_lazy_claimed_lease_if_needed(&input, lazy_claimed_reservation.as_ref())?;
            return Ok(AuthorizationOutcome {
                decision: Decision::deny(
                    "reservation_conflict",
                    "Write target is reserved for the next waiting session.",
                    "Wait for the active reservation to be claimed or expire. Do not redeclare reservation or change session_id; that does not release another session's reservation.",
                ),
                wait: None,
                reservation: Some(reservation),
            });
        }

        if self.requires_exact_hook_file_scope(&input)
            && !self.has_exact_hook_file_intent(&input, workspace_id)?
        {
            if let Some(outcome) = self.queue_active_claim_conflict(
                &input,
                Some(workspace_id),
                allow_queue_side_effects,
            )? {
                self.release_lazy_claimed_lease_if_needed(
                    &input,
                    lazy_claimed_reservation.as_ref(),
                )?;
                return Ok(outcome);
            }
            self.release_lazy_claimed_lease_if_needed(&input, lazy_claimed_reservation.as_ref())?;
            return Ok(AuthorizationOutcome {
                decision: Decision::deny(
                    "scope_mismatch",
                    "Hook file targets require active task reservation exact file scope for every affected path.",
                    "Add exact file scope for every affected path to the task reservation and acquire matching same-reservation file claims before writing.",
                ),
                wait: None,
                reservation: None,
            });
        }

        let claim_owner = self.claim_conflict_owner(&input, workspace_id)?;
        if let Some(owner) = claim_owner {
            let wait = self.enqueue_waiter_for_active_claim_conflict(
                &input,
                workspace_id,
                &owner,
                allow_queue_side_effects,
            )?;

            self.release_lazy_claimed_lease_if_needed(&input, lazy_claimed_reservation.as_ref())?;
            return Ok(AuthorizationOutcome {
                decision: active_claim_conflict_decision(),
                wait,
                reservation: None,
            });
        }

        let requires_exact_hook_file_scope = self.requires_exact_hook_file_scope(&input);
        let has_required_lease = if requires_exact_hook_file_scope {
            self.has_exact_hook_file_lease(&input, workspace_id)?
        } else {
            self.has_required_lease(&input, workspace_id)?
        };
        if !has_required_lease {
            let decision = if requires_exact_hook_file_scope {
                Decision::deny(
                    "missing_claim",
                    "Hook file targets require exact active same-reservation file claims for every affected path.",
                    "Acquire matching same-reservation file claims for every affected path before writing. Do not change reservation_id; that does not create same-reservation claim ownership.",
                )
            } else {
                Decision::deny(
                    "missing_claim",
                    "Write target is inside active reservation scope, but no active same-reservation claim matches it.",
                    "Acquire exact same-reservation file claims for file actions, or exact same-reservation directory claims for write-directory actions. Do not change reservation_id; that does not create same-reservation claim ownership.",
                )
            };
            self.release_lazy_claimed_lease_if_needed(&input, lazy_claimed_reservation.as_ref())?;
            return Ok(AuthorizationOutcome {
                decision,
                wait: None,
                reservation: None,
            });
        }

        if let Some(decision) = self.claim_observation_decision(&input, workspace_id)? {
            self.release_lazy_claimed_lease_if_needed(&input, lazy_claimed_reservation.as_ref())?;
            return Ok(AuthorizationOutcome {
                decision,
                wait: None,
                reservation: None,
            });
        }

        if let Some(decision) = self.base_observation_decision(&input)? {
            self.release_lazy_claimed_lease_if_needed(&input, lazy_claimed_reservation.as_ref())?;
            return Ok(AuthorizationOutcome {
                decision,
                wait: None,
                reservation: None,
            });
        }

        Ok(AuthorizationOutcome {
            decision,
            wait: None,
            reservation: lazy_claimed_reservation,
        })
    }

    fn release_lazy_claimed_lease_if_needed(
        &self,
        input: &AuthorizeWriteInput,
        reservation: Option<&WaitRecord>,
    ) -> Result<(), String> {
        let Some(reservation) = reservation else {
            return Ok(());
        };
        let _ = self.store.release_claim(
            &input.session_id,
            &reservation.workspace_id,
            &reservation.relative_path,
        );
        Ok(())
    }

    fn queue_active_claim_conflict(
        &self,
        input: &AuthorizeWriteInput,
        workspace_id: Option<&str>,
        allow_queue_side_effects: bool,
    ) -> Result<Option<AuthorizationOutcome>, String> {
        let Some(workspace_id) = workspace_id else {
            return Ok(None);
        };
        let Some(owner) = self.claim_conflict_owner(input, workspace_id)? else {
            return Ok(None);
        };
        let wait = self.enqueue_waiter_for_active_claim_conflict(
            input,
            workspace_id,
            &owner,
            allow_queue_side_effects,
        )?;
        let Some(wait) = wait else {
            return Ok(None);
        };
        Ok(Some(AuthorizationOutcome {
            decision: active_claim_conflict_decision(),
            wait: Some(wait),
            reservation: None,
        }))
    }

    fn enqueue_waiter_for_active_claim_conflict(
        &self,
        input: &AuthorizeWriteInput,
        workspace_id: &str,
        blocking_session_id: &str,
        allow_queue_side_effects: bool,
    ) -> Result<Option<WaitQueueInfo>, String> {
        if !allow_queue_side_effects
            || !input.queue_on_conflict
            || is_multi_path_action(&input.action)
        {
            return Ok(None);
        }

        let purpose = input
            .queue_purpose
            .as_deref()
            .ok_or_else(|| "queue purpose is required".to_string())?;
        let waiter = self
            .store
            .enqueue_waiter_with_identity(
                &input.session_id,
                workspace_id,
                &input.path,
                &input.action,
                purpose,
                Some(blocking_session_id),
                workspace_identity(
                    &input.repo_id,
                    &input.worktree_id,
                    &input.root,
                    &input.branch,
                ),
            )
            .map_err(|error| error.to_string())?;
        let queue_position = self
            .store
            .queue_position(&waiter.wait_id)
            .map_err(|error| error.to_string())?;
        Ok(Some(WaitQueueInfo {
            record: waiter,
            queue_position,
        }))
    }

    fn requires_exact_hook_file_scope(&self, input: &AuthorizeWriteInput) -> bool {
        if !matches!(input.source_kind.as_ref(), Some(SourceKind::Hook)) {
            return false;
        }
        matches!(
            input.action.as_str(),
            "write_file" | "delete_file" | "rename_file" | "move_file"
        )
    }

    fn requires_file_freshness_scope(&self, input: &AuthorizeWriteInput) -> bool {
        if self.requires_exact_hook_file_scope(input) {
            return true;
        }
        input.source_event.as_deref() == Some("sandbox_run")
            && matches!(
                input.action.as_str(),
                "write_file" | "delete_file" | "rename_file" | "move_file"
            )
    }

    fn allows_lazy_claim_on_authorize(&self, input: &AuthorizeWriteInput) -> bool {
        matches!(input.source_kind.as_ref(), Some(SourceKind::Hook))
            || input.source_event.as_deref() == Some("sandbox_run")
    }

    fn has_exact_hook_file_intent(
        &self,
        input: &AuthorizeWriteInput,
        workspace_id: &str,
    ) -> Result<bool, String> {
        if let Some(reservation_id) = input.reservation_id.as_deref() {
            for path in self.affected_paths(input) {
                if !self
                    .store
                    .active_exact_file_intent_by_reservation(workspace_id, path, reservation_id)
                    .map_err(|error| error.to_string())?
                {
                    return Ok(false);
                }
            }
            return Ok(true);
        }

        for path in self.affected_paths(input) {
            if !self
                .store
                .active_exact_file_intent_by_session(workspace_id, path, &input.session_id)
                .map_err(|error| error.to_string())?
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn has_exact_hook_file_lease(
        &self,
        input: &AuthorizeWriteInput,
        workspace_id: &str,
    ) -> Result<bool, String> {
        if let Some(reservation_id) = input.reservation_id.as_deref() {
            for path in self.affected_paths(input) {
                if !self
                    .store
                    .active_exact_file_lease_by_reservation(workspace_id, path, reservation_id)
                    .map_err(|error| error.to_string())?
                {
                    return Ok(false);
                }
            }
            return Ok(true);
        }

        for path in self.affected_paths(input) {
            if !self
                .store
                .active_exact_file_lease_by_session(workspace_id, path, &input.session_id)
                .map_err(|error| error.to_string())?
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn claim_observation_decision(
        &self,
        input: &AuthorizeWriteInput,
        workspace_id: &str,
    ) -> Result<Option<Decision>, String> {
        if !self.requires_file_freshness_scope(input) {
            return Ok(None);
        }

        for path in self.affected_paths(input) {
            let observation = if let Some(reservation_id) = input.reservation_id.as_deref() {
                self.store
                    .active_exact_file_claim_observation_by_reservation(
                        workspace_id,
                        path,
                        reservation_id,
                    )
                    .map_err(|error| error.to_string())?
            } else {
                self.store
                    .active_exact_file_claim_observation_by_session(
                        workspace_id,
                        path,
                        &input.session_id,
                    )
                    .map_err(|error| error.to_string())?
            };
            let Some(observation) = observation else {
                continue;
            };
            let Some(root) = input.root.as_deref().filter(|root| !root.is_empty()) else {
                return Ok(Some(stale_claim_observation_decision(
                    "Claim observations require workspace.root so target freshness can be checked.",
                )));
            };
            let current = current_target_observation(root, path)?;
            if current.exists != observation.exists {
                return Ok(Some(stale_claim_observation_decision(
                    "Target existence changed since the active claim was acquired.",
                )));
            }
            if observation.exists && current.content_hash != observation.content_hash {
                return Ok(Some(stale_claim_observation_decision(
                    "Target content changed since the active claim was acquired.",
                )));
            }
        }

        Ok(None)
    }

    fn has_required_lease(
        &self,
        input: &AuthorizeWriteInput,
        workspace_id: &str,
    ) -> Result<bool, String> {
        if let Some(reservation_id) = input.reservation_id.as_deref() {
            return match input.action.as_str() {
                "write_directory" => self
                    .store
                    .active_claim_covers_directory_by_reservation(
                        workspace_id,
                        &input.path,
                        reservation_id,
                    )
                    .map_err(|error| error.to_string()),
                "write_file" | "delete_file" => self
                    .store
                    .active_exact_file_lease_by_reservation(
                        workspace_id,
                        &input.path,
                        reservation_id,
                    )
                    .map_err(|error| error.to_string()),
                "rename_file" | "move_file" => {
                    let Some((old_path, new_path)) = self.rename_or_move_paths(input) else {
                        return Ok(false);
                    };
                    let old_lease = self
                        .store
                        .active_exact_file_lease_by_reservation(
                            workspace_id,
                            old_path,
                            reservation_id,
                        )
                        .map_err(|error| error.to_string())?;
                    if !old_lease {
                        return Ok(false);
                    }
                    self.store
                        .active_exact_file_lease_by_reservation(
                            workspace_id,
                            new_path,
                            reservation_id,
                        )
                        .map_err(|error| error.to_string())
                }
                _ => Ok(false),
            };
        }

        match input.action.as_str() {
            "write_directory" => self
                .store
                .active_claim_covers_directory_by_session(
                    workspace_id,
                    &input.path,
                    &input.session_id,
                )
                .map_err(|error| error.to_string()),
            "write_file" | "delete_file" => self
                .store
                .active_exact_file_lease_by_session(workspace_id, &input.path, &input.session_id)
                .map_err(|error| error.to_string()),
            "rename_file" | "move_file" => {
                let Some((old_path, new_path)) = self.rename_or_move_paths(input) else {
                    return Ok(false);
                };
                let old_lease = self
                    .store
                    .active_exact_file_lease_by_session(workspace_id, old_path, &input.session_id)
                    .map_err(|error| error.to_string())?;
                if !old_lease {
                    return Ok(false);
                }
                self.store
                    .active_exact_file_lease_by_session(workspace_id, new_path, &input.session_id)
                    .map_err(|error| error.to_string())
            }
            _ => Ok(false),
        }
    }

    fn rename_or_move_paths<'input>(
        &self,
        input: &'input AuthorizeWriteInput,
    ) -> Option<(&'input str, &'input str)> {
        let old_path = input.old_path.as_deref()?.trim();
        let new_path = input.new_path.as_deref()?.trim();
        if old_path.is_empty() || new_path.is_empty() {
            return None;
        }
        Some((old_path, new_path))
    }

    fn affected_paths<'input>(&self, input: &'input AuthorizeWriteInput) -> Vec<&'input str> {
        match input.action.as_str() {
            "rename_file" | "move_file" => {
                if let Some((old_path, new_path)) = self.rename_or_move_paths(input) {
                    if old_path == new_path {
                        vec![old_path]
                    } else {
                        vec![old_path, new_path]
                    }
                } else {
                    Vec::new()
                }
            }
            _ => vec![input.path.as_str()],
        }
    }

    fn base_observation_decision(
        &self,
        input: &AuthorizeWriteInput,
    ) -> Result<Option<Decision>, String> {
        if input.base_observations.is_empty() {
            return Ok(None);
        }

        let Some(root) = input.root.as_deref() else {
            return Ok(Some(stale_observation_decision(
                "Base observations require workspace.root so target freshness can be checked.",
            )));
        };

        let affected_paths = self
            .affected_paths(input)
            .into_iter()
            .map(normalize_relative_path)
            .collect::<Vec<_>>();

        for observation in &input.base_observations {
            let observed_path = normalize_relative_path(&observation.path);
            if !affected_paths.iter().any(|path| path == &observed_path) {
                continue;
            }

            let current = current_target_observation(root, &observed_path)?;
            if current.exists != observation.exists {
                return Ok(Some(stale_observation_decision(
                    "Target existence changed since the supplied base observation.",
                )));
            }
            if observation.exists
                && observation
                    .content_hash
                    .as_ref()
                    .is_some_and(|expected| current.content_hash.as_ref() != Some(expected))
            {
                return Ok(Some(stale_observation_decision(
                    "Target content changed since the supplied base observation.",
                )));
            }
        }

        Ok(None)
    }

    fn reservation_conflict(
        &self,
        input: &AuthorizeWriteInput,
        workspace_id: &str,
    ) -> Result<Option<WaitRecord>, String> {
        if input.action == "write_directory" {
            return self
                .store
                .active_reservation_conflict_for_directory(
                    workspace_id,
                    &input.path,
                    &input.session_id,
                )
                .map_err(|error| error.to_string());
        }

        for path in self.affected_paths(input) {
            if let Some(reservation) = self
                .store
                .active_reservation_conflict_for_path(workspace_id, path, &input.session_id)
                .map_err(|error| error.to_string())?
            {
                return Ok(Some(reservation));
            }
        }
        Ok(None)
    }

    fn supplied_session_reservation(
        &self,
        input: &AuthorizeWriteInput,
        workspace_id: &str,
        reservation_id: &str,
    ) -> Result<Option<WaitRecord>, String> {
        let Some(reservation) = self
            .store
            .reservation_by_id(reservation_id)
            .map_err(|error| error.to_string())?
        else {
            return Ok(None);
        };

        if reservation.session_id != input.session_id
            || reservation.workspace_id != workspace_id
            || reservation.action != input.action
        {
            return Ok(None);
        }

        let reserved_path = normalize_relative_path(&reservation.relative_path);
        let covers_target = if input.action == "write_directory" {
            reserved_path == normalize_relative_path(&input.path)
        } else {
            self.affected_paths(input)
                .iter()
                .any(|path| reserved_path == normalize_relative_path(path))
        };

        Ok(covers_target.then_some(reservation))
    }

    fn current_session_reservation(
        &self,
        input: &AuthorizeWriteInput,
        workspace_id: &str,
    ) -> Result<Option<WaitRecord>, String> {
        if input.action == "write_directory" {
            return self
                .store
                .active_reservation_for_directory_by_session(
                    workspace_id,
                    &input.path,
                    &input.session_id,
                )
                .map_err(|error| error.to_string());
        }

        for path in self.affected_paths(input) {
            if let Some(reservation) = self
                .store
                .active_reservation_for_path_by_session(workspace_id, path, &input.session_id)
                .map_err(|error| error.to_string())?
            {
                return Ok(Some(reservation));
            }
        }
        Ok(None)
    }

    fn claim_conflict_owner(
        &self,
        input: &AuthorizeWriteInput,
        workspace_id: &str,
    ) -> Result<Option<String>, String> {
        if input.action == "write_directory" {
            return self
                .store
                .active_claim_conflict_owner_for_directory(
                    workspace_id,
                    &input.path,
                    &input.session_id,
                )
                .map_err(|error| error.to_string());
        }

        for path in self.affected_paths(input) {
            if let Some(owner) = self
                .store
                .active_claim_conflict_owner_for_path(workspace_id, path, &input.session_id)
                .map_err(|error| error.to_string())?
            {
                return Ok(Some(owner));
            }
        }
        Ok(None)
    }

    pub fn claim_intent(
        &self,
        input: ClaimReservationInput,
    ) -> Result<ClaimReservationOutcome, String> {
        let reservation = self
            .store
            .reservation_by_id(&input.wait_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "reservation not found".to_string())?;
        if reservation.session_id != input.session_id
            || reservation.workspace_id != input.workspace_id
        {
            return Err("reservation owner mismatch".to_string());
        }
        if normalized_relative_path_is_empty(&reservation.relative_path) {
            return Err("reservation scope is required".to_string());
        }

        let scope = if reservation.action == "write_directory" {
            format!("{}/", reservation.relative_path.trim_end_matches('/'))
        } else {
            reservation.relative_path.clone()
        };
        let lease_path = scope.clone();
        let mut event = Event::reservation_declared(
            &input.session_id,
            &input.workspace_id,
            reservation.purpose.clone(),
            [scope],
        );
        event.repo_id = input
            .repo_id
            .clone()
            .or_else(|| reservation.repo_id.clone());
        event.worktree_id = input
            .worktree_id
            .clone()
            .or_else(|| reservation.worktree_id.clone());
        event.root = input.root.clone().or_else(|| reservation.root.clone());
        event.branch = input.branch.clone().or_else(|| reservation.branch.clone());
        let claim_observation = input
            .root
            .as_deref()
            .filter(|root| !root.is_empty())
            .map(|root| claim_observation_for_path(root, &lease_path))
            .transpose()?;
        let claimed = self
            .store
            .claim_reservation_with_intent_and_lease(
                &input.wait_id,
                &input.session_id,
                &input.workspace_id,
                event,
                &lease_path,
                claim_observation,
            )
            .map_err(|error| error.to_string())?;

        Ok(ClaimReservationOutcome {
            reservation: claimed,
        })
    }

    pub fn request_intent(
        &self,
        input: RequestReservationInput,
    ) -> Result<RequestReservationOutcome, String> {
        if !matches!(input.action.as_str(), "write_file" | "write_directory") {
            return Err("unsupported reservation request action".to_string());
        }
        if normalized_relative_path_is_empty(&input.path) {
            return Err("reservation scope is required".to_string());
        }

        if let Some(existing) = self
            .store
            .waiter_by_request_id(&input.request_id)
            .map_err(|error| error.to_string())?
        {
            if existing.session_id != input.session_id
                || existing.workspace_id != input.workspace_id
            {
                return Err("reservation request owner mismatch".to_string());
            }
            let existing = self
                .store
                .backfill_waiter_identity_if_missing(
                    &existing.wait_id,
                    workspace_identity(
                        &input.repo_id,
                        &input.worktree_id,
                        &input.root,
                        &input.branch,
                    ),
                )
                .map_err(|error| error.to_string())?;
            return self.request_outcome(input.request_id, existing);
        }

        let reservation_conflict = if input.action == "write_directory" {
            self.store
                .active_reservation_conflict_for_directory(
                    &input.workspace_id,
                    &input.path,
                    &input.session_id,
                )
                .map_err(|error| error.to_string())?
        } else {
            self.store
                .active_reservation_conflict_for_path(
                    &input.workspace_id,
                    &input.path,
                    &input.session_id,
                )
                .map_err(|error| error.to_string())?
        };
        let blocking_session_id = reservation_conflict
            .as_ref()
            .map(|reservation| reservation.session_id.as_str());

        let claim_owner = if reservation_conflict.is_none() {
            if input.action == "write_directory" {
                self.store
                    .active_claim_conflict_owner_for_directory(
                        &input.workspace_id,
                        &input.path,
                        &input.session_id,
                    )
                    .map_err(|error| error.to_string())?
            } else {
                self.store
                    .active_claim_conflict_owner_for_path(
                        &input.workspace_id,
                        &input.path,
                        &input.session_id,
                    )
                    .map_err(|error| error.to_string())?
            }
        } else {
            None
        };
        let blocking_session_id = blocking_session_id.or(claim_owner.as_deref());

        let waiter = self
            .store
            .enqueue_reservation_request_with_identity(
                ReservationRequestInput {
                    request_id: &input.request_id,
                    session_id: &input.session_id,
                    workspace_id: &input.workspace_id,
                    relative_path: &input.path,
                    action: &input.action,
                    purpose: &input.purpose,
                    blocking_session_id,
                },
                workspace_identity(
                    &input.repo_id,
                    &input.worktree_id,
                    &input.root,
                    &input.branch,
                ),
            )
            .map_err(|error| error.to_string())?;

        if reservation_conflict.is_none() && claim_owner.is_none() {
            self.store
                .promote_next_waiter_for_path(&input.workspace_id, &input.path)
                .map_err(|error| error.to_string())?;
        }

        let waiter = self
            .store
            .waiter_by_request_id(&input.request_id)
            .map_err(|error| error.to_string())?
            .unwrap_or(waiter);
        self.request_outcome(input.request_id, waiter)
    }

    pub fn cancel_intent(
        &self,
        input: CancelReservationInput,
    ) -> Result<CancelReservationOutcome, String> {
        let wait = self
            .store
            .cancel_reservation_request(&input.request_id, &input.session_id, &input.workspace_id)
            .map_err(|error| error.to_string())?;
        Ok(CancelReservationOutcome {
            request_id: input.request_id,
            wait,
        })
    }

    fn request_outcome(
        &self,
        request_id: String,
        waiter: WaitRecord,
    ) -> Result<RequestReservationOutcome, String> {
        let request_state = waiter.status.clone();
        let queue_position = if waiter.status == "queued" {
            self.store
                .queue_position(&waiter.wait_id)
                .map_err(|error| error.to_string())?
        } else {
            None
        };

        let wait = if matches!(waiter.status.as_str(), "queued" | "canceled" | "expired") {
            Some(WaitQueueInfo {
                record: waiter.clone(),
                queue_position,
            })
        } else {
            None
        };
        let reservation = if matches!(waiter.status.as_str(), "reserved" | "claimed") {
            Some(waiter)
        } else {
            None
        };

        Ok(RequestReservationOutcome {
            request_id,
            request_state,
            wait,
            reservation,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CurrentTargetObservation {
    exists: bool,
    content_hash: Option<String>,
}

fn stale_observation_decision(message: &str) -> Decision {
    Decision::deny(
        "stale_target_observation",
        message,
        "Reread target, retry same edit with fresh base observation.",
    )
}

fn stale_claim_observation_decision(message: &str) -> Decision {
    Decision::deny(
        "stale_claim_observation",
        message,
        "Reread target, reacquire claim, retry same edit.",
    )
}

pub(crate) fn claim_observation_for_path(
    root: &str,
    relative_path: &str,
) -> Result<ClaimObservation, String> {
    let current = current_target_observation(root, relative_path)?;
    Ok(ClaimObservation {
        exists: current.exists,
        content_hash: current.content_hash,
    })
}

fn current_target_observation(
    root: &str,
    relative_path: &str,
) -> Result<CurrentTargetObservation, String> {
    let path = workspace_relative_path(root, relative_path)?;
    let metadata = match std::fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CurrentTargetObservation {
                exists: false,
                content_hash: None,
            });
        }
        Err(error) => {
            return Err(format!(
                "failed to read target observation metadata for {}: {error}",
                path.display()
            ));
        }
    };
    if metadata.is_dir() && relative_path.ends_with('/') {
        return Ok(CurrentTargetObservation {
            exists: true,
            content_hash: None,
        });
    }

    match std::fs::read(&path) {
        Ok(bytes) => Ok(CurrentTargetObservation {
            exists: true,
            content_hash: Some(content_hash(&bytes)),
        }),
        Err(error) => Err(format!(
            "failed to read target observation for {}: {error}",
            path.display()
        )),
    }
}

fn workspace_relative_path(root: &str, relative_path: &str) -> Result<PathBuf, String> {
    let root = Path::new(root);
    if root.as_os_str().is_empty() {
        return Err("workspace.root is required for base observation checks".to_string());
    }

    let mut path = root.to_path_buf();
    for component in Path::new(relative_path).components() {
        match component {
            Component::Normal(segment) => path.push(segment),
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) | Component::ParentDir => {
                return Err("base observation paths must stay inside the workspace".to_string());
            }
        }
    }
    Ok(path)
}

fn content_hash(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("fnv1a64:{hash:016x}")
}
