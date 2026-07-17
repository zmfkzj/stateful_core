# Final Closure Task 8 Report

## Status
Implemented and verified at base `f86ae6d`.

## RED/GREEN evidence

- RED — `node --test crates/stateful-cli/assets/stateful-omp-extension.test.mjs`: new lazy edit/write tests failed when the claim runner rejected a wait-only claim; the resumed hooks lacked the captured original ID.
- RED — `node --test integrations/vscode/test/core.test.js`: enabled-repository identity tests failed with global runtime identity (`unknown` repo/worktree), disabled/mismatched folders posted, and multi-root presence coalesced.
- RED — focused regressions also caught raw subdirectory claim paths, quoted YAML booleans, and explicit-workspace multi-root actor collisions.
- GREEN — `node --test crates/stateful-cli/assets/stateful-omp-extension.test.mjs`: 14 passing, 0 failing.
- GREEN — `node --test integrations/vscode/test/core.test.js`: 17 passing, 0 failing.

## Identity invariants

- An IDE folder is active only when its canonical Git root exactly matches an enabled entry in installed `$STATEFUL_HOME/config.yml`; disabled, malformed, stale, and mismatched entries create no runtime or mutation envelope.
- The emitted root is the canonical Git root. `repo_id` and `worktree_id` are the enabled metadata `repo_id`; the branch is read from normal or worktree `.git` `HEAD` metadata, otherwise `unknown`.
- Runtime `local`, `shared`, and `unknown` map to Rust's `workspace-<repo suffix>` form. Explicit runtime workspace IDs are preserved exactly.
- Actor IDs include the derived workspace identity; when an explicit runtime workspace is shared by multiple roots they additionally include the worktree identity, keeping folders and low-confidence throttling distinct.
- Runtime query, save-check, observe, and reconcile all consume this one derived workspace object.

## OMP and Task 7 closure

- Lazy edit/write stores the original OMP `toolCallId`, reservation/wait identity, and normalized repository-relative claim path. Both resumed Rust hooks receive that same original `tool_call_id`; PostToolUse records completion/error result metadata.
- Resume claims use the clean Task 7 interface only: `stateful reservation claim --agent-id … --wait-id … --path <normalized-repo-relative-path>`. There is no wait-only compatibility call. A queued wait without one unambiguous normalized target is not resumed with a fabricated claim path.

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
- Pushed `f86ae6d..3a6689c` to `origin/presence-first-event-journal-v2`.

## Concerns

The VS Code package script currently invokes `node --test test`, which Node 26 treats as a missing module. Focused verification used the working Node-native file command above; no package or dependency change was made.
