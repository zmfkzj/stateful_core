# Final Closure Task 8 Report

## Status
Implemented and verified at base `f86ae6d`.

## RED/GREEN evidence

- RED — `node --test crates/stateful-cli/assets/stateful-omp-extension.test.mjs`: lazy resumes attempted wait-only/redundant claims before a grant; grant-path, original-ID, and completion-identity regressions were added first.
- RED — `node --test integrations/vscode/test/core.test.js`: enabled-repository identity initially emitted global unknown metadata; disabled/mismatched folders posted, multi-root presence coalesced, symlink document paths were dropped, and an outer root won over a nested enabled root.
- RED — focused regressions also caught raw subdirectory claim paths, quoted YAML booleans, explicit-workspace actor collisions, missing grant binding, grant-path mismatch acceptance, and unsupported raw hook identity fields.
- GREEN — `node --test crates/stateful-cli/assets/stateful-omp-extension.test.mjs`: 15 passing, 0 failing.
- GREEN — `node --test integrations/vscode/test/core.test.js`: 19 passing, 0 failing.

## Identity invariants

- An IDE folder is active only when its canonical Git root exactly matches an enabled entry in installed `$STATEFUL_HOME/config.yml`; disabled, malformed, stale, and mismatched entries create no runtime or mutation envelope.
- The emitted root is the canonical Git root. `repo_id` and `worktree_id` are the enabled metadata `repo_id`; the branch is read from normal or worktree `.git` `HEAD` metadata, otherwise `unknown`.
- Runtime `local`, `shared`, and `unknown` map to Rust's `workspace-<repo suffix>` form. Explicit runtime workspace IDs are preserved exactly.
- Actor IDs include the derived workspace identity; when an explicit runtime workspace is shared by multiple roots they additionally include the worktree identity, keeping folders and low-confidence throttling distinct.
- Runtime query, save-check, observe, and reconcile all consume this one derived workspace object.
- Document containment accepts the opened workspace-folder spelling while events retain the canonical root; nested enabled roots select the longest matching canonical root rather than workspace-folder order.

## OMP and Task 7 closure

- The reservation record owns `wait_id`. A `reservation_granted` notification binds its exact wait to the actual reservation ID; resumed PreToolUse sends only its supported real `reservation_id` and original tool-call ID, so Rust can claim the reservation before the original operation completes. Unmatched or ungranted waits fail closed.
- PostToolUse sends the original operation ID and completion outcome; wait/reservation IDs remain only in `result_metadata` diagnostics, not unsupported top-level fields. A granted wait is never claimed again and no wait-only compatibility claim exists.

## Files

- `crates/stateful-cli/assets/stateful-omp-extension.js`
- `crates/stateful-cli/assets/stateful-omp-extension.test.mjs`
- `integrations/vscode/extension.js`
- `integrations/vscode/lib/core.js`
- `integrations/vscode/test/core.test.js`
- `.superpowers/sdd/final-closure-task-8-report.md`

## Commits and push

- Implementation: `da1770b` — `fix: preserve integration repository identities`
- Review fixes: `3a6689c` — `fix: scope resumed claims and IDE actors`
- Independent review closure: `9dcb3d8` — `fix: bind granted lazy reservations`
- Pushed `0bd6496..9dcb3d8` to `origin/presence-first-event-journal-v2`.

## Concerns

The VS Code package script currently invokes `node --test test`, which Node 26 treats as a missing module. Focused verification used the working Node-native file command above; no package or dependency change was made.
