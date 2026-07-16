# Final Closure Task 7 Report

## Status

DONE for Task 7 Rust-owned scope. Task 8 must update the installed OMP JavaScript caller to pass the now-required claim `--path`; that asset and its JavaScript test are explicitly outside Task 7 ownership.

## Implementation

- Structured native commits replay retained write lifecycles before their first authorization; replay errors stop the commit before an authorization request.
- Every sandbox profile that can mutate repository files (`write-targets`, `git`, `github-pr`, and external with repository targets) replays retained lifecycles before work begins. `read-only`, build scratch, and external-only scopes do not replay.
- Reservation claim carries distinct `wait_id` and normalized `relative_path`. The HTTP request sends the path required by `/v2/reservation/claim`, then validates that the returned queued grant has the supplied wait ID and path. The Rust native protocol payload preserves both fields. The CLI has no wait-ID-only claim compatibility path.
- Server reuse requires compatible V2 identity, the persisted PID matching that identity, and proof that the PID belongs to the current Stateful executable. An unproved persisted runtime file is removed without signaling its PID; only a fully proved runtime can be terminated.
- Watcher candidates remain queued until a successful response. Failed observations retain their immutable request envelope and request ID; retry resends it, while acknowledged paths are removed exactly once.
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
```

Results: 1/1, 1/1, 2/2, 1/1, 1/1, 1/1, 1/1, 3/3, 1/1, and 1/1 passed, respectively.

## Ordering and Identity Review

- Replay occurs before a new native commit authorization and before every repository-mutating sandbox profile; replay errors are propagated rather than logged or ignored.
- Claim wait identity is never serialized as a path. The response must return the exact wait ID and normalized path supplied by the caller.
- Runtime retirement signals only after both endpoint PID identity and executable ownership proof; unproved PIDs receive no signal.
- Watch retry preserves the first request ID and removes only paths whose request received a successful response.
- Current/events use the same effective workspace derivation and full repo/worktree identity supplied to hook requests.

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

Implementation commit `12d5900` (`fix: close CLI lifecycle identity gaps`) was pushed to `origin/presence-first-event-journal-v2`.

## Concern / Task 8 Dependency

`assets/stateful-omp-extension.js` still invokes `stateful reservation claim` with only `--wait-id`. Task 7 intentionally leaves no Rust shim for that obsolete invocation. Task 8 must pass the granted normalized `--path` and update its installed-dispatch JavaScript test before installed OMP lazy resume can use the clean-cutover CLI contract.
