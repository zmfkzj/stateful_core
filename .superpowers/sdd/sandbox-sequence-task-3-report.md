# Task 3 Report: OMP extension sandbox sequence preflight

## Status

Complete. Task 3 modified only the requested production/test targets and created this report. The generated OMP extension preflight now accepts trusted `stateful sandbox run ... --sequence ...` wrappers and validates command/sequence combinations before external grant handling.

## Commit

- `32dffd3` — `feat: allow OMP sandbox run sequences`

## RED

Command:

```bash
stateful sandbox run --fs build --network enabled --write-dir sandbox-sequence-tests --command 'CARGO_HOME="$TMPDIR/cargo" CARGO_TARGET_DIR="$TMPDIR/target" cargo test --locked -p stateful-cli omp_extension_allows_sandbox_run_sequence_preflight -- --nocapture'
```

Result: failed before the JavaScript implementation change, as expected for unsupported `--sequence` preflight. The generated helper returned a denial, so the new test hit `TypeError: Cannot read properties of undefined (reading 'includes')` when it tried to inspect `decision.words`; the pre-change parser only had the `unsupported stateful sandbox run argument` fallback for `--sequence`.

## GREEN

Command:

```bash
stateful sandbox run --fs build --network enabled --write-dir sandbox-sequence-tests --command 'CARGO_HOME="$TMPDIR/cargo" CARGO_TARGET_DIR="$TMPDIR/target" cargo test --locked -p stateful-cli omp_extension_allows_sandbox_run_sequence_preflight -- --nocapture'
```

Result: passed. Output included `test install::tests::omp_extension_allows_sandbox_run_sequence_preflight ... ok` and `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 115 filtered out` for the targeted unit test.

## Self-review notes

- Added the requested generated extension test in `crates/stateful-cli/src/install.rs` before production changes and observed RED.
- Updated `crates/stateful-cli/assets/stateful-omp-extension.js` only in the sandbox run preflight parser: added `--sequence`, `--sequence-shell`, command-vs-sequence validation, sequence-shell dependency, and absolute shell path validation.
- Preserved the existing outer wrapper syntax checks in `splitStatefulCommandWords`; no shell separators, redirects, pipelines, command substitution, or outer wrappers were loosened.
- Kept external grant handling after the new command/sequence validation.
- Skipped formatters, linters, project-wide tests, timeout/help work, and unrelated code as requested.

## Review fix: reject git/github-pr sequences

Status: Complete. The Task 3 review finding is resolved: generated OMP extension preflight now rejects `--sequence` when `--fs git` or `--fs github-pr` is selected, before the external-grant/final-allow path.

Commit:

- `fix: reject OMP sequences for direct sandbox profiles` (final hash reported in handoff)

RED command:

```bash
stateful sandbox run --fs build --network enabled --write-dir sandbox-sequence-task-3-fix --command 'CARGO_HOME="$TMPDIR/cargo" CARGO_TARGET_DIR="$TMPDIR/target" cargo test --locked -p stateful-cli omp_extension_denies_git_profiles_with_sequence_preflight -- --nocapture'
```

RED result: failed as expected before the JavaScript denial. The generated helper returned `[[true,""],[true,""]]` for the git and github-pr sequence commands, but the test expected both to be denied with the direct-profile invariant reasons.

GREEN commands:

```bash
stateful sandbox run --fs build --network enabled --write-dir sandbox-sequence-task-3-fix --command 'CARGO_HOME="$TMPDIR/cargo" CARGO_TARGET_DIR="$TMPDIR/target" cargo test --locked -p stateful-cli omp_extension_denies_git_profiles_with_sequence_preflight -- --nocapture'
stateful sandbox run --fs build --network enabled --write-dir sandbox-sequence-task-3-fix --command 'CARGO_HOME="$TMPDIR/cargo" CARGO_TARGET_DIR="$TMPDIR/target" cargo test --locked -p stateful-cli omp_extension_ -- --nocapture'
```

GREEN result: the focused denial test passed (`1 passed; 0 failed; 116 filtered out`), then the targeted extension tests passed (`5 passed; 0 failed` across the matching install/integration filters).

Self-review notes:

- Added generated extension coverage for both `stateful sandbox run --fs git --sequence 'git status'` and `--fs github-pr --sequence 'gh pr status'`.
- Added the smallest parser guard after command/sequence validation and before external-grant/final allow.
- Used the existing direct-profile reason strings: `git profile requires a single git command` and `github-pr profile requires a single gh pr command`.
