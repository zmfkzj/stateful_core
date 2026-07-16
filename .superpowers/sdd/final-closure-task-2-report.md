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

## Review repair RED/GREEN
- RED: `cargo test -p stateful-store --test freshness` compiled and ran 26 tests; the three new regression tests failed before the repair: legacy sentinel identity could complete/renew, and a same-agent different actor could project read/write presence state. `cargo test -p stateful-store --test presence_handoff finalization_keeps_same_agent_fence_owned_by_a_different_actor` also failed: finalization released the other actor's fence.
- GREEN: `cargo test -p stateful-store --test freshness` — 26 passed; `cargo test -p stateful-store --test presence_handoff` — 30 passed; `cargo test -p stateful-server --test v2_coordination` — 4 passed.
- Repair: persisted `initiating_actor_known` distinguishes legacy missing attribution from a real `ActorType::Unknown` initiator. Intent and fence mutations reject legacy records. Presence resource updates check full actor lineage before planning. Finalization and generic cleanup use the finalizing actor lineage, preserving another actor's same-agent fence. The server captures the workspace version immediately after maintenance and before policy/thin-safety evaluation, so its transaction check rejects an interleaving change.

## Event and transaction review
- Exact stable reads append `read_observation.stabilized` then `presence.resources_updated(Read)`; structural, failed, and unstable completions append no read relation. The resource projector writes the presence row's origin from that real resource event sequence.
- Intent start orders `authorization.warned` (when applicable), `write_intent.started`, one `ResourcesUpdated(Planned)` per normalized target, then fences. Commit orders the intent result, peer invalidations, one `ResourcesUpdated(Changed)` per target, `presence.tool_completed`, then releases. Receipt lookup still precedes planning, preserving frozen duplicate responses and event sequences.
- The server snapshots workspace version after maintenance but before thin-safety and authority evaluation. The store repeats that version read inside its `BEGIN IMMEDIATE` command transaction before planning events; a mismatch yields retryable `stale_authorization` and rolls back intent, fence, receipt, and projections.
- Write intent and fence payloads persist the initiating actor, type, owner, parent lineage, and an attribution-presence marker. Complete, recover, reconcile, fence renewal, and fence release require exact identity; legacy payloads lack the marker and are permanently non-actionable, including to a literal `unknown` caller.

## Commit and push
- Implementation commit: `8ecdf01` (`fix: atomically project write lifecycle presence`).
- Review repair commit: `0b2f937` (`fix: preserve write freshness ownership`).
- This implementation commit and the following report commit are pushed together.

## Concerns
- Legacy in-flight write intents and fences intentionally remain non-actionable after upgrade because their initiating identity is unknown; a new exact authorization is required rather than transferring ownership.
