# Final Closure Task 7 Report

## Status

DONE for Task 7 Rust-owned scope. Task 8 must update the installed OMP JavaScript caller to pass the now-required claim `--path`; that asset and its JavaScript test are explicitly outside Task 7 ownership.

## Implementation

- Structured native commits replay retained write lifecycles before their first authorization; replay errors stop the commit before an authorization request.
- Every sandbox profile that can mutate repository files (`write-targets`, `git`, `github-pr`, and external with repository targets) replays retained lifecycles before work begins. `read-only`, build scratch, and external-only scopes do not replay.
- `RuntimeOrigin` is a persisted, serde-compatible binding API: authorization can fail-closed classify environment override (with captured URL), global runtime, or canonical repo-local runtime; replay resolution has no cross-scope fallback and permits same-origin restarts.
- Reservation claim carries distinct `wait_id` and normalized `relative_path`. The HTTP request sends the path required by `/v2/reservation/claim`, then validates that the returned queued grant has the supplied wait ID and path. The Rust native protocol payload preserves both fields. The CLI has no wait-ID-only claim compatibility path.
- Server reuse requires compatible V2 identity, the persisted PID matching that identity, and an exact canonical live executable path match with the current Stateful executable. An unproved persisted runtime file is removed without signaling its PID; only a fully proved runtime can be terminated.
- Watcher candidates remain queued until a successful response. Timeout failures are logged and retried by the live loop, failed observations retain their immutable request envelope and request ID, and permanently excluded/outside-root/directory candidates are removed rather than rescanned.
- `current` and `events` derive their workspace with `effective_workspace_id_for_repo` and retain repo/worktree identity in enabled repositories. Outside a repository they retain the runtime workspace.

## RED Evidence

- `cargo test -p stateful-cli --test cli reservation_claim_command_requires_granted_path` — failed before the schema change because a wait-ID-only claim parsed successfully.
- `cargo test -p stateful-cli server_lifecycle::tests::mismatched_identity_runtime_is_retired_without_reuse_or_signal` — failed before hardening because a compatible endpoint with a different identity PID was treated as reusable.
- `cargo test -p stateful-cli watcher_retries_failed_and_unsent_paths_with_exact_request_envelopes` — failed before the flush change because pending length was `0`, not `2`, after the first failed post.

## GREEN Evidence

Final focused CLI-only verification (all passed):

```text
cargo test -p stateful-cli --lib server_lifecycle::tests::mismatched_identity_runtime_is_retired_without_reuse_or_signal
cargo test -p stateful-cli --lib watcher_retries_failed_and_unsent_paths_with_exact_request_envelopes
cargo test -p stateful-cli --lib sandbox_replay
cargo test -p stateful-cli --lib sandbox_replays_only_profiles_that_can_mutate_repo_files
cargo test -p stateful-cli --lib current_and_events_query_the_enabled_repo_workspace_identity
cargo test -p stateful-cli --lib claim_validation_rejects_uuid_as_path_or_a_different_queued_wait
cargo test -p stateful-cli --lib native_claim_protocol_preserves_wait_identity_and_granted_path
cargo test -p stateful-cli --test cli reservation_claim_command
cargo test -p stateful-cli --test runtime claim_reservation_via_http_posts_granted_path_and_validates_wait_identity
cargo test -p stateful-cli --test commit structured_commit_replay_failure_prevents_new_authorization
cargo test -p stateful-cli --lib runtime_origin_binds_global_or_canonical_repo_without_fallback
cargo test -p stateful-cli --lib executable_proof_rejects_different_absolute_binary_with_same_basename
cargo test -p stateful-cli --lib watcher_discards_permanently_undeliverable_paths
```

Results: 1/1, 1/1, 2/2, 1/1, 1/1, 1/1, 1/1, 3/3, 1/1, and 1/1 passed, respectively.
Follow-up reviewer fixes passed 1/1, 1/1, 1/1, and 1/1 for runtime-origin binding, exact executable proof, permanent watcher filtering, and watcher retained-envelope retry, respectively.

## Ordering and Identity Review

- Replay occurs before a new native commit authorization and before every repository-mutating sandbox profile; replay errors are propagated rather than logged or ignored.
- Claim wait identity is never serialized as a path. The response must return the exact wait ID and normalized path supplied by the caller.
- Runtime retirement signals only after both endpoint PID identity and executable ownership proof; unproved PIDs receive no signal.
- Watch retry preserves the first request ID and removes only paths whose request received a successful response.
- Current/events use the same effective workspace derivation and full repo/worktree identity supplied to hook requests.
- `RuntimeOrigin` captures authorization source without persisting a token: environment replay requires the same current override URL; global and repo-local replay load only their own runtime file and may adopt a restarted PID/token/port. `hook/write_lifecycle.rs` must persist and consume this API per pending intent.

## Files

- `crates/stateful-cli/src/commit.rs`
- `crates/stateful-cli/src/sandbox.rs`
- `crates/stateful-cli/src/runtime.rs`
- `crates/stateful-cli/src/server_lifecycle.rs`
- `crates/stateful-cli/src/watch.rs`
- `crates/stateful-cli/src/lib.rs`
- `crates/stateful-cli/tests/cli.rs`
- `crates/stateful-cli/tests/commit.rs`
- `crates/stateful-cli/tests/runtime.rs`

## Commit and Push

Implementation commits `12d5900` (`fix: close CLI lifecycle identity gaps`), `4d8a2e5` (`fix: retain watcher retries and verify executable path`), and `7823486` (`fix: bind lifecycle replay to runtime origin`) were pushed to `origin/presence-first-event-journal-v2`.

## Pending Consumer Dependencies

- Task 6 must update `crates/stateful-cli/src/hook/write_lifecycle.rs` to persist `RuntimeOrigin` for each pending intent and resolve replay through `resolve_runtime_origin`; until then its global scan still cannot safely select each record's source runtime.
- `assets/stateful-omp-extension.js` still invokes `stateful reservation claim` with only `--wait-id`. Task 8 must pass the granted normalized `--path` and update its installed-dispatch JavaScript test. Task 7 intentionally leaves no Rust shim for that obsolete invocation.
