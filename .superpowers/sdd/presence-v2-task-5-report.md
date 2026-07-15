# Task 5 Report

## Status
Implementation and package verification are complete; the final commit and push contain the lifecycle correction follow-up.

## Verification
The approved package command below is the current verification evidence.

## Behavioral coverage
The aggregate regression suite covers reservation heartbeat/release/expiry, FIFO wait grant/cancel and non-conflicting directory fanout, claim acquire/refresh/release, fences, human attribution, notification coalescing/expiry/delivery, outbox delivery, idempotent callbacks, terminal activity, replay, receipts, and rollback.

## Concerns
None.

## Aggregate lifecycle blocker follow-up

### RED
- The single approved command, `stateful sandbox run --fs build --network disabled --write-dir v2-aggregates-final --command 'cargo test -p stateful-store -p stateful-core'`, exited 101 before running tests. Rust reported four `E0596` fixture errors in the new replay regressions: `coordination_aggregates.rs:322`, `:358`, `:435`, and `:486` borrowed immutable `store` values mutably for `rebuild_projections`.
- Each fixture is now `mut`; the corrected command below established GREEN.

### GREEN
- `stateful sandbox run --fs build --network disabled --write-dir v2-aggregates-final --command 'cargo test -p stateful-store -p stateful-core'` — passed: 41 stateful-core tests and 69 stateful-store tests (110 total), 0 failed; both doc-test binaries also passed with 0 tests.
- Structural validation: all modified Rust files and `coordination_aggregates.rs` parsed successfully with ast-grep.

### Regressions added
- Candidate promotion checks all active reservation scopes; partial directory-claim release retains sibling claims/reservation; activity finalization passes the full release set; and all four replay paths are asserted.
- Empty Adopt/Reapply ACKs, non-clearing AskUser/Abandon ACKs, and unmatched Reapply ACKs persist dedicated replayable acknowledgement records with decision, normalized files, and summary.
- Duplicate/ancestor claim batches reject rather than silently omit a result. Dedicated behavioral tests also cover two-store duplicates, claimable cancellation promotion, queued-only grants, outbox identity collision, fence refresh action, released-fence owner grace, and expired callbacks.

## Lifecycle correction follow-up

### RED
- `stateful sandbox run --fs build --network disabled --write-dir v2-lifecycle-red-retry --command 'cargo test -p stateful-store --test coordination_aggregates'` — 22 passed and 3 failed as intended: submission ordering used random `wait_id`; activity finalization re-granted an owner wait it had planned to cancel; and a duplicate fence ID produced two response entries.

### GREEN
- `stateful sandbox run --fs build --network disabled --write-dir v2-lifecycle-green --command 'cargo test -p stateful-store --test coordination_aggregates'` — 25 passed, 0 failed.
- `stateful sandbox run --fs build --network disabled --write-dir v2-lifecycle-packages --command 'cargo test -p stateful-store -p stateful-core'` — 112 stateful-store tests and 41 stateful-core tests passed (153 total), 0 failed; both doc-test binaries passed with 0 tests.

### Corrections
- Wait promotion now uses journal submission sequence before UUID and activity finalization excludes every owner wait it plans to cancel from the same promotion pass.
- Duplicate fence IDs are de-duplicated during release planning, yielding one transition and one response record.
