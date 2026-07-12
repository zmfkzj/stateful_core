# StatefulBench Docker Runtime Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace host real-world qualification and live execution with a fail-closed, host-native Docker runtime where each repository/arm/trial shares one container, one checkout, and one HOME.

**Architecture:** The trusted host coordinator retains the hidden corpus and drives Docker. Qualification runs wholly in a disposable container; each live row uses one persistent container and launches all eleven OMP processes with `docker exec` against the same `/workspace` and `/home/stateful`. The container image contains runtime tools but no corpus, and sanitized shared-HOME diagnostics make OMP/Stateful concurrency failures observable.

**Tech Stack:** Python 3.14 `unittest`, Docker CLI/Engine, Debian Bookworm, Bun/OMP, Rust/Stateful, Linux process groups, SQLite, existing StatefulBench JSON reporting.

## Global Constraints

- Real-world `qualify` and `run` require Docker; no host or macOS `sandbox-exec` fallback.
- Qualification and live rows must use the same inspected image ID and `os/architecture` platform.
- Use the Docker host's native architecture; do not force `linux/amd64` on Apple Silicon.
- Each repository/arm/trial gets one fresh persistent container.
- All task and final agents in a row share exactly `/workspace` and `/home/stateful`; no per-agent HOME/profile/database/cache/container.
- Task agents must not receive the dataset root, issue snapshots, reference patches, canonical evaluators, host HOME, qualification cache, or Docker socket.
- Seed only the selected `openai-codex` OAuth credential, once per arm container.
- Set `STATEFUL_OMP_SANDBOX=off` for all arms; retain Linux `bwrap` support for explicit Stateful sandbox commands.
- Preserve current task prompts, corpus contracts, evaluator/reference contents, arm scheduling, clearance semantics, and efficiency metrics.
- Shared-HOME runtime failures remain uncleared rows and gain sanitized diagnostics; never copy raw HOME or secret values into results.
- Use existing dependencies only. Do not add Docker SDK, Compose, an orchestration framework, or a host fallback.
- Documentation cleanup is intentionally deferred until a credit-free Docker smoke proves the implementation, per maintainer workflow.

---

### Task 1: Dedicated Runtime Image and Docker Identity

**Files:**
- Create: `crates/stateful-bench/docker/statefulbench-realworld.Dockerfile`
- Create: `crates/stateful-bench/scripts/statefulbench_container_entry.py`
- Create: `crates/stateful-bench/scripts/statefulbench_docker.py`
- Create: `crates/stateful-bench/scripts/tests/test_statefulbench_docker.py`
- Modify: `crates/stateful-bench/scripts/tests/conftest.py`

**Interfaces:**
- Consumes: existing `stateful` workspace build, OMP installation pattern from `denovo-omp-agent.Dockerfile`, and `subprocess.run`.
- Produces: `DockerRuntime`, `inspect_runtime()`, `docker_command()`, `resolve_binary()`, and `/usr/local/bin/statefulbench-container-entry` for all subsequent tasks.

- [ ] **Step 1: Write Docker identity failure tests**

Add tests that load `statefulbench_docker.py` through the existing path-based test loader and exercise the public data contract:

```python
class DockerRuntimeTests(unittest.TestCase):
    def test_inspect_runtime_records_immutable_native_identity(self):
        completed = subprocess.CompletedProcess(
            ["docker"],
            0,
            stdout=json.dumps([{
                "Id": "sha256:abc",
                "RepoDigests": ["statefulbench@sha256:def"],
                "Os": "linux",
                "Architecture": "arm64",
            }]),
            stderr="",
        )
        runtime = self.mod.inspect_runtime(
            "docker", "statefulbench-realworld:local", runner=Mock(return_value=completed)
        )
        self.assertEqual(runtime.image_id, "sha256:abc")
        self.assertEqual(runtime.repo_digests, ("statefulbench@sha256:def",))
        self.assertEqual(runtime.platform, "linux/arm64")
        self.assertTrue(Path(runtime.binary).is_absolute())

    def test_inspect_runtime_fails_closed_on_missing_daemon_or_non_linux_image(self):
        with self.assertRaisesRegex(RuntimeError, "Docker image inspection failed"):
            self.mod.inspect_runtime(
                "docker", "missing", runner=Mock(return_value=subprocess.CompletedProcess([], 1, "", "daemon unavailable"))
            )
```

Also assert that `docker_command(runtime, ...)` always starts with the resolved Docker binary and `--platform=<runtime.platform>` where the Docker subcommand accepts it.

- [ ] **Step 2: Run the focused tests and observe RED**

Run:

```sh
python3 -m unittest crates.stateful-bench.scripts.tests.test_statefulbench_docker.DockerRuntimeTests -v
```

Expected: failure because `statefulbench_docker.py` and its interfaces do not exist.

- [ ] **Step 3: Implement immutable Docker runtime inspection**

Implement the minimal module contract:

```python
@dataclass(frozen=True)
class DockerRuntime:
    binary: str
    image: str
    image_id: str
    repo_digests: tuple[str, ...]
    platform: str


def resolve_binary(binary: str) -> str:
    resolved = shutil.which(binary)
    if resolved is None:
        raise RuntimeError(f"Docker binary is not executable: {binary}")
    return str(Path(resolved).resolve())


def inspect_runtime(docker_bin: str, image: str, *, runner=subprocess.run) -> DockerRuntime:
    binary = resolve_binary(docker_bin)
    completed = runner(
        [binary, "image", "inspect", image],
        capture_output=True,
        text=True,
        check=False,
    )
    if completed.returncode != 0:
        raise RuntimeError(f"Docker image inspection failed: {completed.stderr.strip()}")
    rows = json.loads(completed.stdout)
    if len(rows) != 1 or rows[0].get("Os") != "linux":
        raise RuntimeError("Docker image must resolve to exactly one Linux image")
    row = rows[0]
    return DockerRuntime(
        binary=binary,
        image=image,
        image_id=row["Id"],
        repo_digests=tuple(sorted(row.get("RepoDigests") or ())),
        platform=f"{row['Os']}/{row['Architecture']}",
    )
```

Keep command construction as plain list-returning functions. Do not add a Docker client class.

- [ ] **Step 4: Add the process-group entrypoint**

The entrypoint accepts a PID-record path followed by the real command, creates a new session, atomically records PID/PGID, then `exec`s without a shell:

```python
def main(argv: list[str]) -> int:
    if len(argv) < 3:
        raise SystemExit("usage: statefulbench-container-entry PID_FILE COMMAND [ARG ...]")
    pid_file = Path(argv[1])
    os.setsid()
    record = {"pid": os.getpid(), "pgid": os.getpgrp()}
    temporary = pid_file.with_suffix(".tmp")
    temporary.write_text(json.dumps(record) + "\n", encoding="utf-8")
    os.replace(temporary, pid_file)
    os.execvpe(argv[2], argv[2:], os.environ)
    return 127
```

Add an assert-based unit test that patches `os.setsid`, `os.execvpe`, and `os.replace` and verifies the exact PID record and argv. Do not invoke a shell or interpolate prompt text.

- [ ] **Step 5: Add the dedicated image**

Use a multi-stage Dockerfile patterned after the existing DeNovo image:

```dockerfile
FROM rust:1.90-bookworm AS stateful-builder
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release -p stateful-cli

FROM python:3.14.6-slim-bookworm
ARG OMP_VERSION=16.4.2
ENV DEBIAN_FRONTEND=noninteractive \
    BUN_INSTALL=/opt/bun \
    CARGO_HOME=/usr/local/cargo \
    RUSTUP_HOME=/usr/local/rustup \
    PATH=/opt/bun/bin:/usr/local/cargo/bin:/usr/local/bin:/usr/bin:/bin \
    PYTHONDONTWRITEBYTECODE=1
RUN apt-get update \
    && apt-get install -y --no-install-recommends bash bubblewrap ca-certificates curl git build-essential pkg-config \
    && rm -rf /var/lib/apt/lists/* \
    && curl -fsSL https://bun.sh/install | bash \
    && bun install -g "@oh-my-pi/pi-coding-agent@${OMP_VERSION}" \
    && ln -s /opt/bun/bin/omp /usr/local/bin/omp
COPY --from=stateful-builder /usr/local/cargo /usr/local/cargo
COPY --from=stateful-builder /usr/local/rustup /usr/local/rustup
COPY --from=stateful-builder /src/target/release/stateful /usr/local/bin/stateful
COPY crates/stateful-bench/scripts/statefulbench_container_entry.py /usr/local/bin/statefulbench-container-entry
RUN chmod 0755 /usr/local/bin/statefulbench-container-entry \
    && python3 --version \
    && omp --version \
    && stateful --help >/dev/null \
    && git --version \
    && rustc --version \
    && cargo --version \
    && command -v bwrap
WORKDIR /workspace
```

Do not copy `datasets/`, evaluator files, reference patches, host configuration, or credentials.

- [ ] **Step 6: Run Task 1 tests and build the image**

Run the focused unit tests, then:

```sh
docker build --platform "$(docker version --format '{{.Server.Os}}/{{.Server.Arch}}')" \
  -f crates/stateful-bench/docker/statefulbench-realworld.Dockerfile \
  -t statefulbench-realworld:local .
```

Expected: all focused tests pass; image build succeeds; `docker image inspect statefulbench-realworld:local` reports the native Linux architecture.

- [ ] **Step 7: Commit Task 1**

```sh
git add crates/stateful-bench/docker/statefulbench-realworld.Dockerfile \
  crates/stateful-bench/scripts/statefulbench_container_entry.py \
  crates/stateful-bench/scripts/statefulbench_docker.py \
  crates/stateful-bench/scripts/tests/test_statefulbench_docker.py \
  crates/stateful-bench/scripts/tests/conftest.py
git commit -m "feat: add statefulbench Docker runtime"
```

---

### Task 2: Docker-Only Qualification Gate

**Files:**
- Modify: `crates/stateful-bench/scripts/statefulbench_docker.py`
- Modify: `crates/stateful-bench/scripts/statefulbench_realworld.py`
- Modify: `crates/stateful-bench/scripts/tests/test_statefulbench_docker.py`
- Modify: `crates/stateful-bench/scripts/tests/test_statefulbench_realworld.py`

**Interfaces:**
- Consumes: `DockerRuntime` and `inspect_runtime()` from Task 1; existing qualification functions remain unchanged inside the container.
- Produces: `qualification_command()`, `run_qualification_container()`, `write_qualification_receipt()`, `load_qualification_receipt()`, `STATEFULBENCH_DOCKER_INNER=qualification`, Docker CLI options shared by `qualify` and `run`, and qualification provenance output.

- [ ] **Step 1: Write qualification command tests**

Use temporary absolute paths and assert the exact security boundary:

```python
def test_qualification_command_mounts_repo_read_only_and_cache_artifacts_rw(self):
    command = self.mod.qualification_command(
        runtime=self.runtime,
        repo_root=Path("/repo"),
        manifest=Path("/repo/datasets/statefulbench-realworld/manifest.json"),
        cache=Path("/runs/cache"),
        repositories=("requests",),
    )
    self.assertIn("type=bind,source=/repo,target=/benchmark,readonly", command)
    self.assertIn("type=bind,source=/runs/cache,target=/cache", command)
    self.assertNotIn("/Users/arthur", " ".join(command))
    self.assertIn("STATEFULBENCH_DOCKER_INNER=qualification", command)
```

Add tests that reject a manifest outside the repository, reject cache inside the read-only repository, preserve repeatable `--repo`, propagate Docker exit status, reject an inner invocation whose inspected image/platform provenance does not match the outer values, atomically persist one successful receipt per repository, omit receipts for failed repositories, and reject stale manifest/corpus/image/platform receipts.

- [ ] **Step 2: Run qualification tests and observe RED**

Run:

```sh
python3 -m unittest crates.stateful-bench.scripts.tests.test_statefulbench_docker.DockerQualificationTests -v
```

Expected: failures for missing command and coordinator functions.

- [ ] **Step 3: Build the qualification Docker command**

Implement a deterministic command with these mounts and environment values:

```text
/benchmark                         repository root, read-only
/cache                             content-addressed archive/pip cache, read-write
/cache/qualification               retained artifacts, read-write
STATEFULBENCH_DOCKER_INNER          qualification
STATEFULBENCH_IMAGE_ID              inspected immutable ID
STATEFULBENCH_IMAGE_PLATFORM        inspected linux/<native-arch>
PYTHONDONTWRITEBYTECODE             1
```

The inner command is:

```text
python3 /benchmark/crates/stateful-bench/scripts/statefulbench_realworld.py qualify \
  --manifest /benchmark/datasets/statefulbench-realworld/manifest.json \
  --cache /cache \
  [--repo <key> ...] \
  --docker-image <same input image> \
  --docker-bin docker
```

The environment sentinel, not a public `--inside` switch, prevents recursion. The inner path validates `STATEFULBENCH_IMAGE_ID` and `STATEFULBENCH_IMAGE_PLATFORM` before loading the corpus.

- [ ] **Step 4: Make Docker mandatory in the CLI**

Add the same arguments to `qualify` and `run`:

```python
for command in (qualify, run):
    command.add_argument("--docker-bin", default="docker")
    command.add_argument("--docker-image", required=True)
```

At outer `qualify`, inspect the runtime and execute `run_qualification_container()`. Only the sentinel-bearing inner qualification may call the existing host-shaped qualification implementation, because it is then running inside the approved image. Remove any path that qualifies directly on macOS.

Add immutable runtime metadata to the qualification JSON result under:

```json
{
  "runtime": {
    "image": "statefulbench-realworld:local",
    "image_id": "sha256:...",
    "repo_digests": [],
    "platform": "linux/arm64"
  }
}
```

For every qualified repository, atomically write `/cache/qualification/receipts/<repo-key>.json` containing the repository key, manifest SHA-256, corpus SHA-256, archive/commit identity, image ID, repo digests, platform, qualification timestamp, and `qualified: true`. Failed or partial qualification must not create or preserve a successful receipt for that repository. Outer `run` loads the selected repositories' receipts and fails before creating any row unless every identity matches the current manifest, corpus, archive, inspected image ID, and platform. This enforces an explicit prior gate without silently running qualification.

- [ ] **Step 5: Run focused qualification tests**

Run:

```sh
python3 -m unittest \
  crates.stateful-bench.scripts.tests.test_statefulbench_docker.DockerQualificationTests \
  crates.stateful-bench.scripts.tests.test_statefulbench_realworld.ManifestTests \
  crates.stateful-bench.scripts.tests.test_statefulbench_realworld.QualificationTests -v
```

Expected: all tests pass without contacting Docker because subprocess runners are injected.

- [ ] **Step 6: Run one real Docker qualification**

Run Requests qualification with the built image and a dedicated cache. Expected: exit `0`, every Requests task base-red/reference-green, integrated/upstream green, and runtime metadata equal to `docker image inspect`.

- [ ] **Step 7: Commit Task 2**

```sh
git add crates/stateful-bench/scripts/statefulbench_docker.py \
  crates/stateful-bench/scripts/statefulbench_realworld.py \
  crates/stateful-bench/scripts/tests/test_statefulbench_docker.py \
  crates/stateful-bench/scripts/tests/test_statefulbench_realworld.py
git commit -m "feat: qualify real-world corpus in Docker"
```

---

### Task 3: Persistent Arm Container and Shared Runtime

**Files:**
- Modify: `crates/stateful-bench/scripts/statefulbench_docker.py`
- Modify: `crates/stateful-bench/scripts/statefulbench_realworld.py`
- Modify: `crates/stateful-bench/scripts/tests/test_statefulbench_docker.py`
- Modify: `crates/stateful-bench/scripts/tests/test_statefulbench_realworld.py`

**Interfaces:**
- Consumes: `DockerRuntime`; existing archive extraction and prompt rendering on the trusted host.
- Produces: `ArmContainer`, `start_arm_container()`, `exec_in_container()`, `copy_to_container()`, `remove_arm_container()`, `prepare_arm_runtime()`, and container-aware repository setup/evaluator/suite execution.

- [ ] **Step 1: Write mount-boundary and shared-HOME tests**

Assert one container per row and only approved mounts:

```python
def test_arm_container_has_one_shared_workspace_and_home_without_hidden_mounts(self):
    command = self.mod.arm_container_command(
        self.runtime,
        name="statefulbench-requests-parallel-on-1",
        workspace=Path("/runs/requests/parallel-on/trial-1/workspace"),
        runtime_dir=Path("/runs/requests/parallel-on/trial-1/runtime"),
    )
    text = " ".join(command)
    self.assertIn("target=/workspace", text)
    self.assertIn("target=/runtime", text)
    self.assertNotIn("datasets/statefulbench-realworld", text)
    self.assertNotIn("docker.sock", text)
    self.assertNotIn("/home/agents", text)
```
Add tests proving outer `run` refuses to start a container without a matching successful qualification receipt; all exec environments contain the same `HOME=/home/stateful`, `PI_CODING_AGENT_DIR=/home/stateful/.omp/profiles/stateful/agent`, and `STATEFUL_OMP_SANDBOX=off`; credential seeding is invoked once; and container removal occurs on initialization failure.

- [ ] **Step 2: Run arm-container tests and observe RED**

Run the new `DockerArmContainerTests`; expect missing interface failures.

- [ ] **Step 3: Implement the persistent container lifecycle**

Define the concrete state carrier:

```python
@dataclass(frozen=True)
class ArmContainer:
    runtime: DockerRuntime
    container_id: str
    name: str
    workspace: Path
    runtime_dir: Path

    @property
    def home(self) -> str:
        return "/home/stateful"
```

Start with `docker run -d --init`, native `--platform`, bridge network, the same ProgramBench capability/security flags, and only the workspace and row-local runtime-directory bind mounts. The runtime directory contains prompts introduced at their allowed phase, logs, PID records, and sanitized diagnostics; it never contains corpus or reference material. The image command is `sleep infinity`. Validate a nonempty container ID. Removal uses bounded `docker rm -f` and raises on failure.

- [ ] **Step 4: Prepare the shared runtime once**

`prepare_arm_runtime()` must:

1. create shared HOME and runtime directories inside the container;
2. run Stateful `install --agent omp --yes` only for `parallel-on`;
3. copy the selectively seeded OAuth `agent.db` once into the shared OMP directory;
4. run Stateful `enable --repo /workspace` only for `parallel-on`;
5. start exactly one enforcement-mode server inside the same container for `parallel-on`;
6. return the common environment used by every agent.

Do not call `omp_environment()`, create agent-specific directories, or copy the host HOME.

- [ ] **Step 5: Move repository commands into the arm container**

Replace live `_fresh_workspace()` usage with:

1. host-only checksum-verified archive extraction into the row workspace;
2. arm container start;
3. `git init/add/commit`, venv creation, and repository setup through `exec_in_container()`;
4. container paths `/workspace` and `/workspace/.statefulbench-venv/bin/python` for setup, evaluators, and suite;
5. the existing sanitized repository environment translated to container paths.

Keep qualification `_fresh_workspace()` unchanged because Task 2 already runs that entire code path inside Docker.

- [ ] **Step 6: Run focused arm and repository tests**

Run `DockerArmContainerTests` and `RealWorldRunnerTests`. Expected: setup/evaluator/suite subprocesses are Docker exec commands; no direct OMP, Python, Git, or Stateful live subprocess remains.

- [ ] **Step 7: Commit Task 3**

```sh
git add crates/stateful-bench/scripts/statefulbench_docker.py \
  crates/stateful-bench/scripts/statefulbench_realworld.py \
  crates/stateful-bench/scripts/tests/test_statefulbench_docker.py \
  crates/stateful-bench/scripts/tests/test_statefulbench_realworld.py
git commit -m "feat: isolate real-world arms in Docker"
```

---

### Task 4: Docker Exec Agents, Scheduling, and Cleanup

**Files:**
- Modify: `crates/stateful-bench/scripts/statefulbench_docker.py`
- Modify: `crates/stateful-bench/scripts/statefulbench_realworld.py`
- Modify: `crates/stateful-bench/scripts/tests/test_statefulbench_docker.py`
- Modify: `crates/stateful-bench/scripts/tests/test_statefulbench_realworld.py`

**Interfaces:**
- Consumes: `ArmContainer`, shared environment, and image entrypoint.
- Produces: `DockerAgentHandle`, `launch_agent()`, `wait_agent()`, `terminate_agent_group()`, phase-safe evaluator injection, and unchanged sequential/parallel/final scheduling semantics.

- [ ] **Step 1: Write process and scheduling tests**

Define observable contracts:

```python
def test_all_agents_use_same_home_and_workspace(self):
    handles = [self.launch("task-a"), self.launch("task-b"), self.launch("final")]
    for command in self.commands:
        self.assertIn("HOME=/home/stateful", command)
        self.assertIn("-w", command)
        self.assertIn("/workspace", command)
    self.assertEqual({handle.container_id for handle in handles}, {"container-1"})


def test_parallel_launches_ten_execs_before_first_wait(self):

    self.run_arm("parallel-off")
    self.assertEqual(self.events[:10], ["launch"] * 10)
    self.assertEqual(self.events[10], "wait")
```

Treat `--omp-bin` and `--stateful-binary` as container paths, defaulting to `/usr/local/bin/omp` and `/usr/local/bin/stateful`. Verify both with `docker exec test -x` during initialization; never resolve them with host `shutil.which()`.

Add tests for sequential interleaving, one final agent after ten task reaps, evaluator injection only after no task handle remains, inner PID/PGID recording, TERM/KILL escalation, whole-container removal when death cannot be proven, and no grading after cleanup failure.

- [ ] **Step 2: Run agent lifecycle tests and observe RED**

Run `DockerAgentLifecycleTests` plus `RealWorldRunnerTests`; expect failures until the host launcher is replaced.

- [ ] **Step 3: Implement Docker agent handles**

Use a compatible handle with explicit inner identity:

```python
@dataclass
class DockerAgentHandle:
    popen: subprocess.Popen
    agent_id: str
    container_id: str
    pid_record: Path
    started_monotonic: float
```

Launch `docker exec` with the common environment and:

```text
statefulbench-container-entry /runtime/pids/<agent>.json \
  omp --cwd /workspace --mode json --model <model> --thinking <thinking> \
  --approval-mode yolo --no-title @<approved-prompt>
```

Copy only the current task prompt before its launch. Do not copy final specifications until every task is reaped. Store stdout/stderr in the row's per-agent logs.

- [ ] **Step 4: Implement bounded inner-group cleanup**

Read the PID record from the runtime bind mount. On timeout or final cleanup, call `docker exec <id> kill -TERM -<pgid>`, wait five seconds, then `kill -KILL -<pgid>`, wait five seconds, and verify `/proc/<pid>` is absent. If identity is missing or survival cannot be disproved, mark cleanup failure and forcibly remove the arm container. Never grade after that path.

- [ ] **Step 5: Cut over `run_repo_arm()`**

Remove live dependencies on `_LITE.launch_agent`, `_LITE._wait_agent`, `_LITE.arm_stateful_server`, `denied_read_paths`, host `omp_bin` resolution, and macOS `sandbox-exec`. Preserve dependency injection points for unit tests, but default them to the concrete Docker functions.

Keep task and final usage parsing from the same JSON logs so token/tool-call metrics do not change.

- [ ] **Step 6: Run focused scheduling and lifecycle tests**

Run all Docker tests plus `RealWorldRunnerTests`. Expected: all pass; command captures contain no direct host OMP launch and no per-agent HOME.

- [ ] **Step 7: Commit Task 4**

```sh
git add crates/stateful-bench/scripts/statefulbench_docker.py \
  crates/stateful-bench/scripts/statefulbench_realworld.py \
  crates/stateful-bench/scripts/tests/test_statefulbench_docker.py \
  crates/stateful-bench/scripts/tests/test_statefulbench_realworld.py
git commit -m "feat: run shared agents through Docker exec"
```

---

### Task 5: Sanitized Shared-HOME Diagnostics and Reporting

**Files:**
- Create: `crates/stateful-bench/scripts/statefulbench_container_diagnostics.py`
- Modify: `crates/stateful-bench/docker/statefulbench-realworld.Dockerfile`
- Modify: `crates/stateful-bench/scripts/statefulbench_docker.py`
- Modify: `crates/stateful-bench/scripts/statefulbench_realworld.py`
- Modify: `crates/stateful-bench/scripts/tests/test_statefulbench_docker.py`
- Modify: `crates/stateful-bench/scripts/tests/test_statefulbench_realworld.py`

**Interfaces:**
- Consumes: `ArmContainer`, lifecycle phase names, existing result/summary builders.
- Produces: `capture_home_snapshot()`, `classify_runtime_failure()`, redacted diagnostic JSON, Docker provenance fields, setup/teardown timing, and structured error evidence.

- [ ] **Step 1: Write redaction, snapshot, and classification tests**

Create a temporary HOME fixture containing ordinary files, `agent.db`, `agent.db-wal`, a malformed SQLite file, and literal token strings. Assert:

```python
snapshot = diagnostics.snapshot_home(home)
encoded = json.dumps(snapshot)
self.assertNotIn("secret-token-value", encoded)
self.assertIn("agent.db", encoded)
self.assertEqual(snapshot["databases"]["agent.db"]["integrity"], "ok")
self.assertNotIn("rows", snapshot["databases"]["agent.db"])
```

Also test before/after created/changed/deleted diffs, detection of lock/WAL/SHM/journal/temp files, SQLite locked/malformed classifications, ambiguous `unclassified_runtime_failure`, and exact artifact paths in `results.json` and `summary.json`.

- [ ] **Step 2: Run diagnostic tests and observe RED**

Run `DockerDiagnosticTests` and `RealWorldReportingTests`; expect missing helper/schema failures.

- [ ] **Step 3: Implement the in-container diagnostic helper**

The helper accepts only a HOME path, phase, and output path. It emits:

- relative path, type, size, mtime-ns, and SHA-256;
- SQLite integrity result, schema names, and safe table counts without row values;
- lock/WAL/SHM/journal/temp file list;
- process snapshot containing PID/PPID/PGID/command basename, not full secret-bearing argv or environment.

Hard-code secret-bearing filename/table/column patterns (`auth`, `credential`, `token`, `secret`, `cookie`, `header`) and omit values. On any uncertain database shape, emit file metadata and integrity only. Copy this helper into the image; do not mount it from the repository during live execution.

- [ ] **Step 4: Capture every approved lifecycle boundary**

Call the helper at:

```text
initialized
before-tasks
after-tasks
after-final
after-grading
before-remove
```

Compute snapshot deltas on the host from sanitized JSON. Capture Docker inspect summary, tool versions, lifecycle timestamps, Stateful logs/events, cleanup signals, and surviving-process evidence. A diagnostic capture/redaction failure makes the row uncleared.

- [ ] **Step 5: Extend results without changing clearance**

Add these result fields:

```json
{
  "runtime": {
    "image_id": "sha256:...",
    "repo_digests": [],
    "platform": "linux/arm64",
    "versions": {}
  },
  "container": {
    "id": "...",
    "setup_wall_time_s": 0.0,
    "teardown_wall_time_s": 0.0,
    "removed": true
  },
  "diagnostics": {
    "snapshots": {},
    "home_changes": [],
    "error_classification": null
  }
}
```

Keep `cleared` dependent on all existing gates plus successful cleanup, safe diagnostics, and container removal. Keep `arm_wall_time_s` agent-only; report setup/teardown separately and retain end-to-end row elapsed time.

Reject aggregate comparisons when scheduled rows differ in image ID or platform. Preserve the raw rows as diagnostics.

- [ ] **Step 6: Run focused diagnostics and reporting tests**

Run Docker diagnostics, real-world reporting, and runner test classes. Expected: all pass; test output contains no fixture secret value.

- [ ] **Step 7: Commit Task 5**

```sh
git add crates/stateful-bench/scripts/statefulbench_container_diagnostics.py \
  crates/stateful-bench/docker/statefulbench-realworld.Dockerfile \
  crates/stateful-bench/scripts/statefulbench_docker.py \
  crates/stateful-bench/scripts/statefulbench_realworld.py \
  crates/stateful-bench/scripts/tests/test_statefulbench_docker.py \
  crates/stateful-bench/scripts/tests/test_statefulbench_realworld.py
git commit -m "feat: capture shared-home Docker diagnostics"
```

---

### Task 6: Credit-Free End-to-End Docker Gate

**Files:**
- Modify: `crates/stateful-bench/scripts/tests/test_statefulbench_docker.py`
- Modify: `crates/stateful-bench/scripts/tests/test_statefulbench_realworld.py`
- Modify only if a defect is demonstrated: implementation files from Tasks 1–5

**Interfaces:**
- Consumes: the built image and complete Docker qualification/live runtime.
- Produces: one deterministic, credit-free proof that shared workspace/HOME, scheduling, Stateful server, cleanup, diagnostics, and grading work before documentation cleanup or a model-backed run.

- [ ] **Step 1: Add an opt-in real-Docker integration test**

Gate it with `STATEFULBENCH_DOCKER_TEST_IMAGE`. The test creates a tiny fixture repository and fake OMP executable that:

- records `$HOME` and `$PWD`;
- appends one agent ID to a shared HOME file;
- edits its assigned shared-workspace file;
- sleeps behind a barrier so parallel overlap is observable;
- emits one OMP JSON usage event;
- exits with a configured code.

Run sequential, parallel-off, and parallel-on through the production container path. Assert all agents report `/home/stateful`, all edits coexist, parallel task intervals overlap, sequential intervals do not, `parallel-on` starts one server, final grading passes, snapshots contain shared HOME changes, and all containers are removed.

- [ ] **Step 2: Run the integration test and observe the first failure**

Run:

```sh
STATEFULBENCH_DOCKER_TEST_IMAGE=statefulbench-realworld:local \
python3 -m unittest \
  crates.stateful-bench.scripts.tests.test_statefulbench_docker.DockerEndToEndTests -v
```

Expected before final corrections: at least one concrete integration assertion fails; record the exact observed boundary rather than weakening the assertion.

- [ ] **Step 3: Correct only demonstrated integration defects**

Apply the smallest root-cause fix in the relevant concrete Docker function. Do not add retries, alternate runtimes, per-agent HOME, host execution, or hidden-data mounts.

- [ ] **Step 4: Rerun the end-to-end gate**

Expected: all three fake-agent arms clear, diagnostics are sanitized, and `docker ps -a` contains no named test container.

- [ ] **Step 5: Run all focused benchmark tests**

Run:

```sh
python3 -m unittest discover \
  -s crates/stateful-bench/scripts/tests \
  -t crates/stateful-bench/scripts \
  -p 'test_statefulbench_docker.py' -v
python3 -m unittest discover \
  -s crates/stateful-bench/scripts/tests \
  -t crates/stateful-bench/scripts \
  -p 'test_statefulbench_realworld.py' -v
python3 -m unittest discover \
  -s crates/stateful-bench/scripts/tests \
  -t crates/stateful-bench/scripts \
  -p 'test_statefulbench_lite.py' -v
```

Expected: every focused test passes. Lite behavior remains unchanged.

- [ ] **Step 6: Commit the verified integration gate**

```sh
git add crates/stateful-bench/scripts/tests/test_statefulbench_docker.py \
  crates/stateful-bench/scripts/tests/test_statefulbench_realworld.py \
  crates/stateful-bench/scripts/statefulbench_docker.py \
  crates/stateful-bench/scripts/statefulbench_realworld.py \
  crates/stateful-bench/scripts/statefulbench_container_diagnostics.py \
  crates/stateful-bench/docker/statefulbench-realworld.Dockerfile
git commit -m "test: verify Docker real-world runtime"
```

Post-smoke maintainer cleanup and credit-consuming qualification/run gates are deliberately outside this pre-smoke plan. The full three-trial run remains blocked until the separately reviewed scoped gate clears every row.
