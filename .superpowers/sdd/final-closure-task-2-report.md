# Final Closure Task 2 Report

## Files
- `crates/stateful-core/src/freshness.rs`
- `crates/stateful-store/src/{lib,observations,presence,write_fences,write_intents}.rs`
- `crates/stateful-server/src/{commands,protocol}.rs`
- `crates/stateful-store/tests/freshness.rs`
- `crates/stateful-server/tests/v2_coordination.rs`

## RED
- `cargo test -p stateful-store --test freshness`: failed as intended before implementation: exact completion had no `Read` presence resource and intent start omitted `Planned` resource events.
- After adding the authorization-snapshot contract test, the same focused binary failed to compile as intended because the version-bearing authorized-start API and `StaleAuthorization` result did not yet exist.

## GREEN
- `cargo test -p stateful-store --test freshness`: 23 passed.
- `cargo test -p stateful-server --test v2_coordination`: 4 passed.

## Event and transaction review
- Exact stable reads append `read_observation.stabilized` then `presence.resources_updated(Read)`; structural, failed, and unstable completions append no read relation. The resource projector writes the presence row's origin from that real resource event sequence.
- Intent start orders `authorization.warned` (when applicable), `write_intent.started`, one `ResourcesUpdated(Planned)` per normalized target, then fences. Commit orders the intent result, peer invalidations, one `ResourcesUpdated(Changed)` per target, `presence.tool_completed`, then releases. Receipt lookup still precedes planning, preserving frozen duplicate responses and event sequences.
- The server snapshots workspace version only after thin-safety and authority evaluation. The store repeats that version read inside its `BEGIN IMMEDIATE` command transaction before planning events; a mismatch yields retryable `stale_authorization` and rolls back intent, fence, receipt, and projections.
- Write intent and fence payloads persist the initiating actor, type, owner, and parent lineage. Complete, recover, reconcile, and fence release require exact identity. Legacy payloads deserialize deterministically as `unknown` and are not silently transferable. Payload-backed projections need no schema migration; focused tests rebuild projections after read and write mutations.

## Commit and push
- Implementation commit: `8ecdf01` (`fix: atomically project write lifecycle presence`).
- This implementation commit and the following report commit are pushed together.

## Concerns
- Legacy in-flight write intents and fences intentionally remain non-actionable after upgrade because their initiating identity is unknown; a new exact authorization is required rather than transferring ownership.
