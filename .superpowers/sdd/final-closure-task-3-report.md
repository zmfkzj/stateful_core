# Final Closure Task 3 Report

## Files
- `crates/stateful-store/src/journal.rs`
- `crates/stateful-store/src/schema.rs`
- `crates/stateful-store/src/migration.rs`
- `crates/stateful-store/src/projector.rs`
- `crates/stateful-store/tests/journal_v2.rs`
- `crates/stateful-store/tests/migration_v2.rs`

## RED / GREEN

RED behavior was observed before each implementation change:
- deterministic rejection: receipt count remained `0` after `ClaimConflict`;
- legacy human migration seed omitted its fingerprint (`Null` rather than `{exists, content_hash}`);
- unexpected V2/internal error created a receipt;
- `WriteFenceConflict` replayed as `ClaimConflict` rather than preserving its fields;
- corrupted eventless rejection receipt passed projection rebuild.

GREEN verification:
- `cargo test -p stateful-store --test journal_v2` — 12 passed.
- `cargo test -p stateful-store --test migration_v2 existing_v2_upgrade_repairs_omitted_terminal_seed_projections` — passed.
- `cargo test -p stateful-store --test migration_v2 failed_terminal_projection_repair_rolls_back_canonical_tables` — passed.
- `cargo test -p stateful-store --test task9_migration_authority` — 3 passed.

## Frozen rejection receipts

Receipts persist and reconstruct these exact deterministic `StoreError` variants: `V2` (only recognized domain codes), `ClaimConflict`, `ClaimAlreadyHeld`, `WriteFenceConflict { path, owner_agent_id }`, `StaleAuthorization`, `MissingReservation`, reservation/claim/write-intent owner and not-found variants, `ReservationRequestNotCancelable`, `MissingPurpose`, `MissingScope`, `InvalidClaimPath`, `InvalidReadOperation`, and `InvalidWriteIntent`.

SQLite, JSON, journal, projector, migration, timestamp, idempotency-reuse, and unrecognized V2 failures remain unpersisted and retryable. Eventless rejection receipts have null event bounds; replay validates their UUID, bounds, typed rejection JSON, and stored status. Event-bearing receipts retain their range and event envelope metadata validation.

## Schema and migration evidence

`create_v2_schema` adds `command_receipts.rejection_json` to new databases and upgrades old V2 tables in place. `prechange_v2_receipts_upgrade_and_replay` rebuilds a pre-column receipt table, reopens it, verifies duplicate replay, and confirms the upgraded column.

Human snapshot seeds retain the immutable legacy `{exists, content_hash}` evidence. Terminal claim and fence snapshot seeds now project their original IDs and terminal statuses; typed/current queries retain them while active filters allow a new claim/fence on the same path. Existing migration ordering and atomic rollback proof remain covered.

## Self-review

- Rejection writes and successful event writes commit through the same immediate transaction; every internal failure exits before receipt commit.
- UUID envelope matching checks route, normalized request hash, agent, actor, and workspace before either response or rejection decoding.
- Eventless receipts are accepted only with no journal events and are fully validated during rebuild; event-bearing receipts still validate actor/workspace metadata for every event.
- Migration seed ordering/provenance was unchanged. Terminal rows are retained in projection but active checks continue to require `status == "active"`.
- Independent re-review after the two receipt findings: no findings.

## Post-closure corrective review

The receipt decoder now rejects a syntactically valid but nonpersistable V2 internal error in both duplicate handling and rebuild validation. This keeps the frozen receipt boundary identical at every replay path.

Existing checkpointed V2 databases repair omitted terminal claim/fence seed rows in one immediate transaction. Terminal events are applied through the canonical projector; a full journal replay then verifies all non-derived projections. Logical projection snapshots sort encoded rows, so storage row insertion order is not semantic. Only `workspace_version` and `agent_context_cursor` are replaced from the full replay when their journal-order provenance differs. A forced retained-row mismatch occurs after terminal application and proves the transaction rolls back the newly inserted terminal projection.

Independent corrective re-review found no Critical, Important, or Minor findings.

## Commit / push

Implementation commit `ea15822` (`fix: freeze rejections and preserve migration terminals`) was pushed to `origin/presence-first-event-journal-v2`.
Corrective receipt validation and terminal-repair commit `18d2eae` (`fix: validate frozen receipts and repair terminal projections`) is ready to push on the same branch.

## Concerns

`cargo test -p stateful-store --test migration_v2` has two pre-existing failures unrelated to this change: `migrated_presence_and_handoff_project_to_typed_records_before_commands` and `finalization_cleans_migrated_coordination_rows_by_journal_owner`. Both fail with `ReservationOwnerMismatch` in unchanged presence/handoff ownership guards. A clean detached clone at `bad9574` reproduces the same two failures (9 passed, 2 failed); the current expanded binary has 13 passed, 2 failed.
