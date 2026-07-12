# StatefulBench Docker Runtime Design

## Status

Approved design. This replaces the host-executed real-world qualification and live-run policy. StatefulBench Lite remains unchanged unless a later design explicitly moves it.

## Problem

The real-world runner currently launches OMP under an agent-specific macOS `sandbox-exec` profile so agents cannot read the hidden corpus. The required outer `stateful sandbox run` also applies macOS Seatbelt. macOS rejects the nested profile before OMP starts:

```text
sandbox-exec: sandbox_apply: Operation not permitted
```

The failed scoped run consumed no model tokens, but it proves that the current host execution path cannot both follow the command policy and preserve hidden-corpus isolation.

## Decisions

1. Real-world qualification and every live arm run in Docker.
2. Qualification and live execution use the same dedicated runtime image.
3. The image uses the host-native Docker architecture. Runs record the resolved platform and immutable image ID/digest; results from different platforms are not pooled.
4. Each repository/arm/trial receives one fresh, persistent container.
5. All eleven agents in an arm share one `/workspace` checkout.
6. All eleven agents in an arm also share one `HOME` and one OMP runtime state.
7. Shared-HOME failures are benchmark evidence about OMP or Stateful concurrency, not infrastructure noise to hide with per-agent homes.
8. Containers never receive the frozen corpus, issue snapshots, reference patches, or canonical evaluators while task agents are running.
9. Docker is mandatory. Qualification and live commands fail closed if Docker, the requested image, required capabilities, or platform inspection is unavailable. There is no host fallback.
10. The container is the ordinary OMP filesystem sandbox boundary. Set `STATEFUL_OMP_SANDBOX=off` consistently for every arm. Stateful's own explicitly requested `stateful sandbox run` operations remain available through Linux `bwrap`, following the existing ProgramBench container pattern.

## Reused Patterns

The implementation reuses established behavior rather than creating a second Docker abstraction:

- DeNovoSWE: bind-mounted agent workspace, prompt mount, environment allowlist, native image execution, `SYS_ADMIN`, and unconfined seccomp/AppArmor/system paths so `bwrap` can run.
- ProgramBench: persistent container, `docker exec`, selective OAuth seeding, explicit cleanup, container-as-sandbox policy, and copy-in/copy-out error reporting.

StatefulBench differs in one essential way: all task agents intentionally edit the same checkout concurrently, so the persistent container belongs to an arm/trial rather than to an individual agent.

## Runtime Image

Add a dedicated Dockerfile under `crates/stateful-bench/docker/`. The image contains only runtime tools:

- the exact supported Python 3.14 patch version;
- Git and required build utilities;
- Rust/Cargo toolchain needed by the frozen corpora;
- `bubblewrap`;
- the pinned OMP version;
- the repository-built `stateful` binary.

The image MUST NOT copy the benchmark dataset, repository manifests, issue snapshots, evaluators, reference patches, qualification cache, or host credentials. Build arguments and resulting versions are verified during image construction. The runner records `docker image inspect` identity, architecture, OS, OMP version, Stateful version, Python version, Git version, and Rust version in every qualification and run summary.

A run accepts `--docker-bin` and `--docker-image`. The documented image tag is a convenience input; reports use the resolved immutable image ID and repository digest when available. Comparing rows built from different image IDs or platforms is prohibited.

## Host Coordinator

The host process remains the trusted coordinator. It may read the manifest and hidden dataset, prepare output paths, inject evaluators after the task phase, and write reports. It never executes repository setup, tests, evaluators, Stateful, or OMP directly on the host.

The coordinator is responsible for:

1. resolving and inspecting Docker and the image;
2. creating a fresh workspace and output directory;
3. starting and naming the qualification or arm container;
4. copying only approved credentials and runtime inputs;
5. launching and reaping container processes;
6. injecting final-phase specifications and canonical evaluators only at the allowed phase boundary;
7. collecting sanitized diagnostics;
8. removing the container on success, failure, interruption, or timeout;
9. atomically persisting results.

Container IDs are recorded as diagnostics but are never accepted as reusable inputs. A scheduled row always starts from a new container.

## Qualification Container

`qualify` starts a disposable container from the same image used for live arms.

Mounted inputs:

- the benchmark code and dataset, read-only, at a fixed container path;
- the content-addressed archive and pip cache, read-write;
- a qualification artifact directory, read-write.

All setup commands, isolated virtual environments, Git operations, base suites, task evaluators, integrated evaluators, overlap checks, and upstream suites run inside the container. Existing sanitized Git configuration and environment isolation remain mandatory.

Qualification reports bind the result to:

- manifest digest;
- corpus and frozen-source digests;
- repository commit and archive SHA-256;
- image ID/digest and platform;
- tool versions;
- exact qualification artifacts.

A live repository selection is valid only after qualification succeeds with the same manifest/corpus identity, image ID, and platform. `run` still does not silently invoke or bypass qualification; the user runs the explicit gate first.

## Live Arm Container

Each `repository / arm / trial` creates one persistent container with:

```text
/workspace                 shared repository checkout
/home/stateful             shared HOME and OMP/Stateful runtime
/runtime/prompts           approved prompt files
/runtime/logs              per-process stdout/stderr and OMP JSON streams
/runtime/diagnostics       sanitized diagnostic output
```

The host bind-mounts only the workspace and explicit output/input directories required for that row. The Docker socket, host HOME, repository root, dataset root, qualification cache, issue snapshots, evaluators, and reference patches are not mounted. The shared HOME lives in the container writable layer and is destroyed with the container; only sanitized diagnostics leave it.

The container uses bridge networking for model API access. Network policy is identical across arms. The runner does not mount the Docker socket into the container.

### Shared State

All agents in a row intentionally share:

- `/workspace`;
- `/home/stateful`;
- OMP configuration, session state, cache, and databases;
- the container process namespace;
- the arm-local Stateful server and store in `parallel-on`;
- network and tool versions.

No per-agent HOME, OMP profile, database, cache, or copied runtime tree is created.

Per-agent separation is limited to observational artifacts and process control:

- prompt file;
- stdout/stderr/JSON log files;
- launch and completion record;
- process group used for bounded timeout cleanup.

### Initialization

Initialization runs exactly once per arm container:

1. create `/workspace`, `/home/stateful`, and runtime directories;
2. copy only the selected `openai-codex` OAuth credential into the shared HOME;
3. prepare OMP once;
4. for `parallel-on`, install/enable Stateful once and start one enforcement-mode server inside the container;
5. record the initial diagnostic snapshot;
6. launch agents.

The coordinator never resets or copies HOME between agents or phases.

### Scheduling

- `sequential`: launch and reap each of the ten task agents in specification order.
- `parallel-off`: launch all ten task agents before waiting; no Stateful server.
- `parallel-on`: start one arm-local Stateful server, then launch all ten task agents before waiting.
- All arms: after every task process is fully reaped, inject the final specifications and canonical evaluator files, then launch one final review/fix agent using the same HOME and workspace.
- After the final agent is reaped, run canonical evaluators and the upstream suite inside the same container.

This preserves the existing shared-checkout experiment and changes only the operating-system isolation boundary.

## Process Lifecycle

`docker exec` returns a host-side Docker client PID, not the inner OMP process identity. Each launched command therefore starts an inner process group and writes its PID/PGID to a runtime record before `exec`-ing OMP.

On timeout or interruption:

1. send `SIGTERM` to the recorded inner process group;
2. wait a fixed grace interval;
3. send `SIGKILL` to that group;
4. wait a second fixed interval;
5. if the process still cannot be proven dead, stop and remove the entire arm container;
6. mark the row uncleared and preserve the cleanup evidence.

The final evaluator never runs while an agent or descendant process remains alive. Container removal failure is a row failure, not a warning.

## Shared-HOME Diagnostics

Shared-HOME concurrency is an explicit observation target. Capture diagnostics at these boundaries:

1. after container initialization;
2. immediately before task launch;
3. after all task agents are reaped;
4. after the final agent is reaped;
5. after canonical grading;
6. immediately before container removal.

Diagnostics include:

- agent ID, inner PID/PGID, start/end timestamps, exit code, timeout, and signal history;
- per-agent stdout, stderr, and OMP JSON events;
- container ID, image identity, platform, inspect summary, and lifecycle timestamps;
- tool versions;
- relative HOME path inventory with type, size, mtime, and SHA-256;
- files created, changed, and deleted between snapshots;
- leftover lock, WAL, SHM, journal, and temporary files;
- SQLite `PRAGMA integrity_check` results;
- SQLite schema names and per-table row counts where safe;
- Stateful server logs and coordination events;
- process snapshots and surviving descendants;
- final workspace diff and grading outcome.

The classifier recognizes evidence such as SQLite `locked`, `busy`, or `malformed` errors, partial configuration writes, session replacement, rename/unlink races, stale lock files, authentication-state loss, and cross-agent session takeover.

A confident classification uses:

```json
{
  "error_class": "shared_home_concurrency",
  "component": "omp",
  "phase": "task_agents",
  "affected_agents": ["task-a", "task-b"],
  "evidence": ["diagnostics/home-after-tasks.json", "logs/task-a.stderr.log"]
}
```

Ambiguous failures remain explicit rather than being guessed:

```json
{
  "error_class": "unclassified_runtime_failure",
  "shared_home_involved": true
}
```

Any such failure keeps `cleared=false`. Diagnostics explain the failure; they never convert it into a successful benchmark row.

## Secret Handling

The complete shared HOME is never copied into benchmark outputs. Diagnostics must exclude or redact:

- OAuth token values;
- credential rows from `agent.db` or other SQLite databases;
- API keys, cookies, and authorization headers;
- full environment-variable values;
- raw files known to contain credentials.

Safe observations include credential-file existence, type, size, mtime, digest, database integrity, schema names, non-credential row counts, and redacted error messages. If a database cannot be inspected without risking secret disclosure, record only its file metadata and integrity result.

## Result Schema and Reporting

Preserve existing completion and efficiency fields. Add runtime provenance and diagnostics without redefining clearance:

- Docker image ID/digest, platform, and tool versions;
- container lifecycle and setup wall time;
- qualification identity used for the row;
- process cleanup status;
- shared-HOME snapshot artifact paths;
- structured error classification;
- Docker command/setup/copy/exec/inspect/remove failures as distinct causes.

Agent wall time remains the primary time measurement. Container setup and teardown are reported separately so all arms remain auditable. Total elapsed row time remains available as an end-to-end metric.

Rows using different image IDs or platforms may be displayed but not aggregated into one comparison. One trial remains descriptive; comparative claims require the maintained three-trial rule.

## Failure Policy

Fail closed when any of the following occurs:

- Docker binary or daemon unavailable;
- image missing or inspection fails;
- runtime platform differs from the requested native platform;
- required container capability unavailable;
- qualification and live image identity differ;
- unexpected mount exposes a forbidden host path;
- credential seeding fails;
- Stateful initialization/server failure in `parallel-on`;
- inner process identity cannot be recorded;
- cleanup cannot prove agents are gone;
- diagnostics cannot be safely redacted;
- canonical evaluator or upstream suite fails;
- container removal fails.

Never fall back to host OMP, host qualification, macOS `sandbox-exec`, per-agent HOME, or a prior container.

## CLI and Documentation

Update real-world commands and operational guidance to require the Docker image. The documented flow is:

1. build the dedicated image for the host-native architecture;
2. inspect and record its immutable identity;
3. run all-ten qualification in Docker;
4. run a one-repository, all-arm, one-trial Docker gate;
5. proceed to the full three-trial run only when every scoped row clears;
6. report Docker provenance and shared-HOME diagnostics.

The README, benchmark guide, running-statefulbench skill, and CLI help must state that host real-world execution is removed rather than deprecated or retained as a fallback.

## Testing

Use TDD and focused tests before live spending.

Required automated contracts:

- Docker command construction uses the inspected image and host-native platform;
- qualification mounts the dataset read-only and executes all repository commands in the container;
- live containers do not mount the dataset, repository root, reference patches, evaluators, or Docker socket;
- all task and final commands use the same `/workspace` and `/home/stateful`;
- parallel arms launch ten `docker exec` commands before waits;
- sequential launch ordering remains serial;
- `parallel-on` starts exactly one in-container Stateful server;
- OAuth seeding occurs once per arm and exposes no other host credentials;
- final-phase files appear only after task processes are reaped;
- inner process-group timeout cleanup is bounded and fail closed;
- container removal occurs for success, setup failure, launch failure, timeout, interruption, and grading failure;
- HOME diagnostics detect file changes and SQLite integrity failures while redacting credentials;
- qualification/run image or platform mismatch is rejected;
- result JSON preserves prior metrics and records Docker provenance and error classifications.

Focused integration smoke tests use a local fake OMP executable inside the image and spend no model credits. The first credit-consuming test is one qualified repository, all three arms, one trial. A failed scoped row blocks the full run.

## Non-Goals

- No per-agent container or per-agent HOME.
- No Docker Compose, Kubernetes, or container scheduler.
- No Docker socket inside agent containers.
- No host execution fallback.
- No change to task prompts, corpus contracts, evaluators, reference patches, or clearance semantics.
- No claim that shared-HOME failures prove general model-quality differences.
- No Stateful graph protocol or OMP fork.
