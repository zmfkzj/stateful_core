### Task 9: CLI V2 request identity repair

**Status:** Complete for the CLI/runtime/lifecycle scope. Server-side PID proof remains separately owned.

#### RED evidence

- `stateful sandbox run --fs build --network enabled --write-dir task9-cli-red-runtime --command 'cargo test -p stateful-cli --test runtime'` exited 101: **20 passed, 5 failed**. The failures proved regenerated request IDs for request/cancel, a per-file declaration returning the last reservation identity, missing workspace status fields, and `current` using unknown repo identity.
- `stateful sandbox run --fs build --network enabled --write-dir task9-cli-red-lifecycle --command 'cargo test -p stateful-cli --test server_lifecycle'` exited 101: **20 passed, 2 failed**. The failures proved restart discarded `enforcement` mode and evaluated a non-HTTP runtime as remote before refusing its scheme.
- `stateful sandbox run --fs build --network enabled --write-dir task9-cli-red-directory --command 'cargo test -p stateful-cli --test runtime declare_reservation_via_http_posts_one_task_envelope_and_returns_its_identity'` exited 101: **0 passed, 1 failed**. The declaration incorrectly collapsed a planned `src/generated/` directory scope into a file scope.

#### Implementation

- Reservation declare now sends one V2 envelope with all normalized file and directory scopes, preserves its caller UUID, and returns that single server response.
- Reservation claim/request/cancel and their protocol-body helpers retain supplied caller UUIDs; cancellation carries its server wait ID separately from its envelope UUID.
- `current` and `events` reuse enabled-repository discovery when available, producing complete repo/worktree/root/branch query identity.
- Runtime status includes server workspace ID and workspace version while preserving the V2 schema/mode/capability handshake.
- Runtime files retain coordination mode; restart derives host, port, token, workspace, and mode from that runtime, and rejects unsupported schemes before PID handling.

#### GREEN evidence

- `stateful sandbox run --fs build --network enabled --write-dir task9-cli-final --command 'cargo test -p stateful-cli --test runtime --test cli --test server_lifecycle'`: **runtime 25 passed; cli 56 passed; server_lifecycle 22 passed**.

#### Concerns

- No endpoint-to-PID proof was added; server ownership covers that separately.
- Hooks, install/OMP, watcher/outbox, documentation, benchmarks, and server integration tests were intentionally untouched.

### Task 9: Journal diagnostic footprint and growth

**Files:** `crates/stateful-cli/src/lib.rs`, `crates/stateful-cli/tests/cli.rs`.

#### RED evidence

- `stateful sandbox run --fs build --network disabled --write-dir task9-journal-red-rerun --command 'cargo test -p stateful-cli --test cli doctor_'` exited 101: **2 passed, 4 failed** (53 filtered), proving main-only footprint and permanent `baseline_unavailable`.
- `stateful sandbox run --fs build --network disabled --write-dir task9-baseline-write-red --command 'cargo test -p stateful-cli --test cli doctor_does_not_claim_a_baseline_when_atomic_replacement_fails'` exited 101: **0 passed, 1 failed** (59 filtered), proving a failed atomic replacement was reported as captured.

#### GREEN evidence

- `stateful sandbox run --fs build --network disabled --write-dir task9-journal-green-final --command 'cargo test -p stateful-cli --test cli doctor_'`: **7 passed, 0 failed** (53 filtered). It covers exact main+WAL footprint, bounded physical delta after a real persistent-store mutation, first/corrupt/future/expired recapture, atomic-replace failure truthfulness, byte-preserving doctor reads, sanitized output, and inclusive 512 MiB warning.

#### Concerns

- Existing Task 9 regressions were attempted: CLI passed **60/60**; runtime had **24 passed, 2 failed** in PID-identity assertions; standalone lifecycle had **22 passed, 1 failed** in mismatched-identity signaling. [INFERENCE] These failures are outside the changed files and journal diagnostic path; the disabled-network attempt also blocked all listener-binding runtime tests.
