# StatefulBench Real-World Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build and run a reproducible three-arm StatefulBench corpus containing ten substantial overlapping issue-derived tasks for each of ten pinned DeNovo repositories.

**Architecture:** Keep `statefulbench-lite` unchanged and add a data-driven real-world runner that reuses its proven OMP launch, usage, timeout, and Stateful server functions. A versioned manifest pins source archives; one corpus file per repository owns prompts, evaluator paths, reference patches, setup, suite command, and overlap anchors. Evaluators remain outside task checkouts until the final-agent stage.

**Tech Stack:** Python 3.14 standard library, OMP 16.4.2, Stateful enforcement server, upstream Python test runners, GitHub source archives.

## Global Constraints

- Repositories are exactly `psf/requests`, `python-jsonschema/jsonschema`, `pytest-dev/pytest-asyncio`, `pytest-dev/pytest-xdist`, `pallets/click`, `jschneier/django-storages`, `python-attrs/attrs`, `gorakhargosh/watchdog`, `python-pendulum/pendulum`, and `authlib/authlib`.
- Freeze each default-branch source at the SHA and archive SHA-256 listed in Task 1; do not silently refresh.
- Each repository has exactly five bug tasks and five feature tasks plus one final review/fix agent.
- Every task has at least three behavioral acceptance criteria covering normal, boundary, and error behavior.
- Every task evaluator fails on the pinned base and passes its reference patch.
- Every task overlaps at least one other task at the same qualified production symbol or named production block; tests, docs, generated files, and lockfiles do not count.
- The integrated ten-task reference patch passes all ten evaluators and the upstream suite.
- Task agents cannot access evaluator tests or reference patches before the final stage.
- Arms are exactly `sequential`, `parallel-off`, and `parallel-on`; each repository/arm/trial uses a fresh checkout.
- `parallel-on` uses one arm-local enforcement server shared by that arm's eleven agents.
- `cleared` requires eleven zero exits, zero timeouts, no arm error, and a passing post-final evaluator plus upstream suite.
- Report only tokens, tool calls, timing, completion/failure, provenance, and descriptive aggregates; make no behavioral-quality, causal, safety, or superiority claim.
- No new Python dependency is permitted for the harness.

---

### Task 1: Pin the source manifest and validate its schema

**Files:**
- Create: `datasets/statefulbench-realworld/manifest.json`
- Create: `crates/stateful-bench/scripts/tests/test_statefulbench_realworld.py`
- Create: `crates/stateful-bench/scripts/statefulbench_realworld.py`

**Interfaces:**
- Produces: `load_manifest(path: Path) -> dict`, `repo_entries(manifest: dict) -> tuple[dict, ...]`
- Manifest repo fields: `key`, `requested_url`, `canonical_url`, `commit`, `archive_url`, `archive_sha256`, `python`, `setup`, `suite`, `corpus`

Pinned records, in order:

| key | commit | archive SHA-256 |
|---|---|---|
| requests | `f361ead047be5cb873174218582f7d8b9fcd9f49` | `7f60df8524d7a042f604a4176cc64777f6543037ab96dc4adaaabff55ada28fd` |
| jsonschema | `97c044c48d6c6c08f88142ad27edc590f2a2cb07` | `1d5bef7a24de2bec70a7840fef22cbaa5b169cf5159a0194635a7720eaa19a75` |
| pytest-asyncio | `66253978d8518925d3f5d2c12615fd7005b63080` | `6715e3e9991cce7fb56ab50e19ceee46c5528ed4817ff9375eab8ab23612cf1d` |
| pytest-xdist | `f63b6a25b4eb932385c6ee4651eac5c08fbd3a20` | `d035858bc41d5aa126e54a3edf8af4a4e871d0b7e5383211da767ab50ad6d511` |
| click | `b67832c2167e5b0ff6764a8c04a0a9087e697b5a` | `bc2f89f9b4687d51ca6ff592f6de34a9f8f97c49b4637c84eabd6a8df16ed1d2` |
| django-storages | `ca89a94a7462a2423df460e7bfd5f847457042ca` | `e0a0a36d3b1470776b6463e5dcd44c805fd31ccd3090110ea16936c176d90fab` |
| attrs | `45de9beb093d2142517ab7d1ebda6522e3d3c4ac` | `b330a639611e08fcfd54baaf3780d364c5e9bec44ab33fda961f0fe6956daffd` |
| watchdog | `c9edf3296d9edb9afded6adfaf3987e87ca8f928` | `a6e12fd17e2706161733cf111e7cd899a15b7e8a8f66540239c57e1a60d3d40d` |
| pendulum | `5ad098bc7b74d660679f0606673728042b9d4aca` | `d49ad8f8c6f43a18c3744dec61730fb369cf91dc77bb1c23df3360ae76d11397` |
| authlib | `5cb26721a39f74a196304e90fa5ae8d31925fd4a` | `c7e7818a31fd3ee7be4e370786974f92eb6c4f90f24efeeb6d3ea02fb0aca6e2` |

- [ ] **Step 1: Write manifest-validation tests**

Add tests that load a temporary JSON manifest and assert: exactly ten unique keys; SHA fields match `^[0-9a-f]{40}$` and `^[0-9a-f]{64}$`; URLs use HTTPS; setup and suite are non-empty argv arrays; corpus paths are relative and remain under the manifest directory. Add a failing duplicate-key case and a failing `../escape.json` corpus case.

```python
class ManifestTests(unittest.TestCase):
    def test_load_manifest_accepts_ten_unique_pinned_repositories(self):
        manifest = self.mod.load_manifest(self.manifest_path)
        self.assertEqual(len(self.mod.repo_entries(manifest)), 10)

    def test_load_manifest_rejects_duplicate_keys(self):
        data = json.loads(self.manifest_path.read_text())
        data["repositories"][1]["key"] = data["repositories"][0]["key"]
        self.manifest_path.write_text(json.dumps(data))
        with self.assertRaisesRegex(ValueError, "duplicate repository key"):
            self.mod.load_manifest(self.manifest_path)

    def test_load_manifest_rejects_corpus_escape(self):
        data = json.loads(self.manifest_path.read_text())
        data["repositories"][0]["corpus"] = "../escape.json"
        self.manifest_path.write_text(json.dumps(data))
        with self.assertRaisesRegex(ValueError, "corpus path"):
            self.mod.load_manifest(self.manifest_path)
```

- [ ] **Step 2: Run RED**

Run: `python3 -m unittest discover -s crates/stateful-bench/scripts/tests -t crates/stateful-bench/scripts -p 'test_statefulbench_realworld.py' -v`

Expected: import/file failure because `statefulbench_realworld.py` and the manifest do not exist.

- [ ] **Step 3: Write the exact manifest and minimal loader**

Use schema version `1`, `generated_at` equal to the source-freeze UTC timestamp, the table values above, GitHub archive URLs of the form `https://github.com/<owner>/<repo>/archive/<commit>.tar.gz`, Python `3.14.6`, repository-specific argv arrays, and corpus paths `repos/<key>.json`.

Implement strict type checks and return the parsed dictionary without introducing dataclasses or a schema dependency.

- [ ] **Step 4: Run GREEN**

Run the command from Step 2.

Expected: all manifest tests pass.

- [ ] **Step 5: Commit**

```bash
git add datasets/statefulbench-realworld/manifest.json crates/stateful-bench/scripts/statefulbench_realworld.py crates/stateful-bench/scripts/tests/test_statefulbench_realworld.py
git commit -m "bench: pin real-world corpus sources"
```

### Task 2: Add verified archive caching and fresh checkout extraction

**Files:**
- Modify: `crates/stateful-bench/scripts/statefulbench_realworld.py`
- Modify: `crates/stateful-bench/scripts/tests/test_statefulbench_realworld.py`

**Interfaces:**
- Consumes: validated repository dictionaries from Task 1
- Produces: `ensure_archive(repo: dict, cache_dir: Path, opener=urllib.request.urlopen) -> Path`
- Produces: `extract_workspace(archive: Path, expected_sha256: str, destination: Path) -> None`

- [ ] **Step 1: Write RED tests**

Create an in-memory `tar.gz` containing one top-level directory and `pyproject.toml`. Assert first use downloads to `<cache>/<sha256>.tar.gz`, second use makes no network call, checksum mismatch deletes the temporary download and raises `ValueError`, and extraction rejects `../escape` and symlink members. Assert two extractions produce byte-identical fresh workspaces and no shared inode for a regular file.

- [ ] **Step 2: Run RED**

Run the focused unittest command. Expected: missing `ensure_archive` and `extract_workspace`.

- [ ] **Step 3: Implement cache and extraction**

Download to `<sha>.tmp`, stream SHA-256 while writing, `os.replace` only after a match, and use `tarfile.extractall(..., filter="data")`. Require exactly one archive root directory and move only its children into an absent destination. Do not add retry, mirror, or cache-eviction policy.

- [ ] **Step 4: Run GREEN and commit**

Expected: archive tests pass.

```bash
git add crates/stateful-bench/scripts/statefulbench_realworld.py crates/stateful-bench/scripts/tests/test_statefulbench_realworld.py
git commit -m "bench: verify real-world source archives"
```

### Task 3: Define and validate repository corpus files

**Files:**
- Modify: `crates/stateful-bench/scripts/statefulbench_realworld.py`
- Modify: `crates/stateful-bench/scripts/tests/test_statefulbench_realworld.py`
- Create: `datasets/statefulbench-realworld/repos/requests.json`
- Create: `datasets/statefulbench-realworld/repos/jsonschema.json`
- Create: `datasets/statefulbench-realworld/repos/pytest-asyncio.json`
- Create: `datasets/statefulbench-realworld/repos/pytest-xdist.json`
- Create: `datasets/statefulbench-realworld/repos/click.json`
- Create: `datasets/statefulbench-realworld/repos/django-storages.json`
- Create: `datasets/statefulbench-realworld/repos/attrs.json`
- Create: `datasets/statefulbench-realworld/repos/watchdog.json`
- Create: `datasets/statefulbench-realworld/repos/pendulum.json`
- Create: `datasets/statefulbench-realworld/repos/authlib.json`

**Interfaces:**
- Produces: `load_corpus(path: Path) -> dict`
- Corpus fields: `repository`, `issue_snapshot`, `tasks`, `final_prompt`, `evaluators`, `integrated_reference_patch`
- Task fields: `key`, `kind`, `sources`, `source_hash`, `prompt`, `acceptance`, `overlap_anchors`, `evaluator`, `reference_patch`

- [ ] **Step 1: Write RED schema and overlap tests**

Assert every corpus has ten unique tasks, kinds count to `{"bug": 5, "feature": 5}`, every acceptance list has at least three non-empty strings, sources use GitHub HTTPS URLs, source hashes are SHA-256, evaluator/reference paths remain below the dataset root, overlap anchors contain `path` plus qualified `symbol`, and every task has nonzero overlap degree with another task on an identical `(path, symbol)` pair. Reject duplicate task keys, a 6/4 kind split, and an isolated task.

- [ ] **Step 2: Run RED**

Expected: missing `load_corpus` and corpus files.

- [ ] **Step 3: Implement strict loader and create corpus records from Tasks 4-13**

The loader performs only validation and returns JSON. It must not infer missing fields, rewrite paths, or access GitHub.

- [ ] **Step 4: Run GREEN and commit after Tasks 4-13 have supplied all records**

```bash
git add datasets/statefulbench-realworld/repos crates/stateful-bench/scripts/statefulbench_realworld.py crates/stateful-bench/scripts/tests/test_statefulbench_realworld.py
git commit -m "bench: add validated real-world corpus schema"
```

### Task 4: Curate the Requests ten-task cell

**Files:**
- Create: `datasets/statefulbench-realworld/repos/requests.json`
- Create: `datasets/statefulbench-realworld/issues/requests.json`
- Create: `datasets/statefulbench-realworld/evaluators/requests/`
- Create: `datasets/statefulbench-realworld/references/requests/`

**Issue families and required pairing anchors:**

| kind | source | behavior family | overlap anchor |
|---|---|---|---|
| bug | #6992 | multipart body with session Content-Type | `requests.models.PreparedRequest.prepare_body` |
| feature | #6294 | explicit zero-byte upload framing | `requests.models.PreparedRequest.prepare_body` |
| bug | #7040 | custom SSLContext regression | `requests.adapters.HTTPAdapter.cert_verify` |
| feature | #7564 | structured FileNotFoundError for TLS material | `requests.adapters.HTTPAdapter.cert_verify` |
| bug | #6885 | replay 307/308 bodies safely | `requests.sessions.SessionRedirectMixin.resolve_redirects` |
| feature | #7574 | RFC 10008 QUERY redirect semantics | `requests.sessions.SessionRedirectMixin.resolve_redirects` |
| bug | #6890 | escaped quotes in cookie values | `requests.models.PreparedRequest.prepare_cookies` |
| feature | #7122 | caller-provided CookiePolicy propagation | `requests.models.PreparedRequest.prepare_cookies` |
| bug | #6205 | mounted transport adapter through proxies | `requests.adapters.HTTPAdapter.send` |
| feature | #6900 | custom SNI and CA through proxies | `requests.adapters.HTTPAdapter.send` |

- [ ] Freeze the ten issue bodies and SHA-256 hashes.
- [ ] For each row, write three or more acceptance criteria and an evaluator that fails on the pinned base for the named behavior.
- [ ] Implement the smallest reference patch that passes that evaluator and the Requests focused tests.
- [ ] Build one integrated patch from all ten references; run all evaluators and `python -m pytest -q`.
- [ ] Record actual changed production anchors from the integrated diff; reject or revise any row that misses its required pair.
- [ ] Commit with message `bench: curate requests real-world cell`.

### Task 5: Curate the Jsonschema ten-task cell

**Files:**
- Create: `datasets/statefulbench-realworld/repos/jsonschema.json`
- Create: `datasets/statefulbench-realworld/issues/jsonschema.json`
- Create: `datasets/statefulbench-realworld/evaluators/jsonschema/`
- Create: `datasets/statefulbench-realworld/references/jsonschema/`

| kind | source | behavior family | overlap anchor |
|---|---|---|---|
| bug | #1511 | duration overflow becomes validation failure | `jsonschema._format.FormatChecker` |
| feature | #1496 | multiple checkers per format | `jsonschema._format.FormatChecker` |
| bug | #1465 | RFC3339 year zero handling | `jsonschema._format.FormatChecker` |
| feature | #1142 | injected regex implementation | `jsonschema._format.FormatChecker` |
| bug | #191 | consistent schema and relative paths | `jsonschema.exceptions.ValidationError` |
| feature | #1218 | instance-free safe error message | `jsonschema.exceptions.ValidationError` |
| bug | #442 | multiple errors per validator in ErrorTree | `jsonschema.exceptions.ErrorTree` |
| feature | #1363 | duplicate-array-item context | `jsonschema.exceptions.ErrorTree` |
| bug | #1159 | float `multipleOf` precision | `jsonschema.validators` named validator block |
| feature | #1170 | deprecated-keyword warning propagation | `jsonschema.validators` named validator block |

- [ ] Freeze the ten issue bodies and SHA-256 hashes.
- [ ] Write at least three acceptance criteria and one base-failing evaluator per row.
- [ ] Implement each reference patch and pass its evaluator plus focused upstream tests.
- [ ] Integrate all ten references and pass all evaluators plus `python -m pytest -q`; verify actual identical production anchors and zero isolated tasks.
- [ ] Commit `bench: curate jsonschema real-world cell`.

### Task 6: Curate the pytest-asyncio ten-task cell

**Files:**
- Create: `datasets/statefulbench-realworld/repos/pytest-asyncio.json`
- Create: `datasets/statefulbench-realworld/issues/pytest-asyncio.json`
- Create: `datasets/statefulbench-realworld/evaluators/pytest-asyncio/`
- Create: `datasets/statefulbench-realworld/references/pytest-asyncio/`

| kind | source | behavior family | overlap anchor |
|---|---|---|---|
| bug | #1501 | loop-factory fixture teardown ordering | `pytest_asyncio.plugin` fixture wrapper block |
| feature | #118 | concurrent async fixture setup/teardown | `pytest_asyncio.plugin` fixture wrapper block |
| bug | #810 | dynamically-added asyncio marker recognition | `pytest_asyncio.plugin.pytest_pyfunc_call` |
| feature | #215 | timeout marker kwargs | `pytest_asyncio.plugin.pytest_pyfunc_call` |
| bug | #1033 | nested-config false-positive warning | `pytest_asyncio.plugin.pytest_configure` |
| feature | #924 | finalized default-loop-scope deprecation | `pytest_asyncio.plugin.pytest_configure` |
| bug | #1083 | TaskGroup exception during fixture yield | `pytest_asyncio.plugin` async-generator fixture block |
| feature | #1044 | optional failure on remaining tasks | `pytest_asyncio.plugin` async-generator fixture block |
| bug | #796 | event-loop-policy over-parametrization | `pytest_asyncio.plugin.event_loop_policy` |
| feature | #1032 | loop configuration API | `pytest_asyncio.plugin.event_loop_policy` |

- [ ] Freeze the ten issue bodies and SHA-256 hashes.
- [ ] Write at least three acceptance criteria and one base-failing evaluator per row.
- [ ] Implement each reference patch and pass its evaluator plus focused pytest-asyncio tests.
- [ ] Integrate all ten references and pass all evaluators plus `python -m pytest -q`; verify actual identical production anchors and zero isolated tasks.
- [ ] Commit `bench: curate pytest-asyncio real-world cell`.

### Task 7: Curate the pytest-xdist ten-task cell

**Files:**
- Create: `datasets/statefulbench-realworld/repos/pytest-xdist.json`
- Create: `datasets/statefulbench-realworld/issues/pytest-xdist.json`
- Create: `datasets/statefulbench-realworld/evaluators/pytest-xdist/`
- Create: `datasets/statefulbench-realworld/references/pytest-xdist/`

| kind | source | behavior family | overlap anchor |
|---|---|---|---|
| bug | #1335 | parameter `::` does not split load scope | `xdist.scheduler.loadscope.LoadScopeScheduling._split_scope` |
| feature | #1008 | secondary load-group key | `xdist.scheduler.loadscope.LoadScopeScheduling._split_scope` |
| bug | #1323 | restarted loadgroup worker does not hang | `xdist.scheduler.loadgroup.LoadGroupScheduling` |
| feature | #1314 | idle-worker early shutdown | `xdist.scheduler.loadgroup.LoadGroupScheduling` |
| bug | #1278 | nonzero worker exit fails session | `xdist.dsession.DSession` |
| feature | #1219 | configurable worker ramping | `xdist.dsession.DSession` |
| bug | #1218 | dist/numprocesses conflict resolution | `xdist.plugin.pytest_cmdline_main` |
| feature | #1221 | `-n auto-1` worker expression | `xdist.plugin.pytest_cmdline_main` |
| bug | #1083 | loadscope preserves required order | `xdist.scheduler.loadscope.LoadScopeScheduling` |
| feature | #1040 | opt-out tests run on controller | `xdist.scheduler.loadscope.LoadScopeScheduling` |

- [ ] Freeze the ten issue bodies and SHA-256 hashes.
- [ ] Write at least three acceptance criteria and one base-failing evaluator per row.
- [ ] Implement each reference patch and pass its evaluator plus focused pytest-xdist tests.
- [ ] Integrate all ten references and pass all evaluators plus `python -m pytest -q`; verify actual identical production anchors and zero isolated tasks.
- [ ] Commit `bench: curate pytest-xdist real-world cell`.

### Task 8: Curate the Click ten-task cell

**Files:**
- Create: `datasets/statefulbench-realworld/repos/click.json`
- Create: `datasets/statefulbench-realworld/issues/click.json`
- Create: `datasets/statefulbench-realworld/evaluators/click/`
- Create: `datasets/statefulbench-realworld/references/click/`

| kind | source | behavior family | overlap anchor |
|---|---|---|---|
| bug | #3362 | usage wrapping does not split long options at hyphen | `click.formatting.HelpFormatter` |
| feature | #3652 | ellipsis for repeated option metavars | `click.formatting.HelpFormatter` |
| bug | #2847 | long-option completion after equals | `click.parser.OptionParser` |
| feature | #2771 | option `nargs=-1` with explicit separator | `click.parser.OptionParser` |
| bug | #2753 | parameter source survives invoke/forward | `click.core.Context` |
| feature | #2783 | dynamic parameters on active context | `click.core.Context` |
| bug | #2402 | missing subcommand name returns usage error | `click.core.Group` |
| feature | #2033 | awaitable command callback support | `click.core.Group` |
| bug | #1370 | confirm handles piped input deterministically | `click.termui.confirm` |
| feature | #1411 | parameter type custom prompt conversion | `click.termui.confirm` |

- [ ] Freeze the ten issue bodies and SHA-256 hashes.
- [ ] Write at least three acceptance criteria and one base-failing evaluator per row.
- [ ] Implement each reference patch and pass its evaluator plus focused Click tests.
- [ ] Integrate all ten references and pass all evaluators plus `python -m pytest -q`; verify actual identical production anchors and zero isolated tasks.
- [ ] Commit `bench: curate click real-world cell`.

### Task 9: Curate the django-storages ten-task cell

**Files:**
- Create: `datasets/statefulbench-realworld/repos/django-storages.json`
- Create: `datasets/statefulbench-realworld/issues/django-storages.json`
- Create: `datasets/statefulbench-realworld/evaluators/django-storages/`
- Create: `datasets/statefulbench-realworld/references/django-storages/`

All rows overlap `storages.backends.s3.S3Storage`.

| kind | source | behavior family |
|---|---|---|
| bug | #1558 | empty-location path normalization |
| bug | #1553 | unauthenticated URL performance path |
| bug | #1551 | string CA bundle verification |
| bug | #1526 | gzip text-mode reads |
| bug | #1524 | object-parameter name handling |
| feature | #1561 | ETag-based incremental collectstatic |
| feature | #1481 | presigned direct-upload URLs |
| feature | #1490 | request-level multi-region selection |
| feature | #1373 | optional metadata preload |
| feature | #1222 | per-call URL expiration |

- [ ] Freeze the ten issue bodies and SHA-256 hashes.
- [ ] Write at least three acceptance criteria and one base-failing evaluator per row.
- [ ] Implement each reference patch and pass its evaluator plus focused S3 backend tests.
- [ ] Integrate all ten references and pass all evaluators plus `python -m pytest -q`; verify every task actually modifies `storages.backends.s3.S3Storage`.
- [ ] Commit `bench: curate django-storages real-world cell`.

### Task 10: Curate the attrs ten-task cell

**Files:**
- Create: `datasets/statefulbench-realworld/repos/attrs.json`
- Create: `datasets/statefulbench-realworld/issues/attrs.json`
- Create: `datasets/statefulbench-realworld/evaluators/attrs/`
- Create: `datasets/statefulbench-realworld/references/attrs/`

All rows overlap the class-generation block in `attr._make` (`_ClassBuilder` and its generated-method helpers).

| kind | source | behavior family |
|---|---|---|
| bug | #1575 | Python 3.14 unimported ClassVar detection |
| bug | #1549 | decorated method inheritance |
| bug | #1462 | custom equality/hash interaction |
| bug | #1333 | overridden cached_property in slotted child |
| bug | #1038 | zero-argument super in decorated methods |
| feature | #1543 | stable third-party extension hook |
| feature | #1532 | public slotted cached-property helpers |
| feature | #1452 | generated equality extension |
| feature | #1437 | post-assignment `on_change` hook |
| feature | #1335 | `__attrs_init_subclass__` keyword forwarding |

- [ ] Freeze the ten issue bodies and SHA-256 hashes.
- [ ] Write at least three acceptance criteria and one base-failing evaluator per row.
- [ ] Implement each reference patch and pass its evaluator plus focused attrs class-generation tests.
- [ ] Integrate all ten references and pass all evaluators plus `python -m pytest -q`; verify every task has an actual paired class-generation anchor.
- [ ] Commit `bench: curate attrs real-world cell`.

### Task 11: Curate the Watchdog ten-task cell

**Files:**
- Create: `datasets/statefulbench-realworld/repos/watchdog.json`
- Create: `datasets/statefulbench-realworld/issues/watchdog.json`
- Create: `datasets/statefulbench-realworld/evaluators/watchdog/`
- Create: `datasets/statefulbench-realworld/references/watchdog/`

| kind | source | behavior family | overlap anchor |
|---|---|---|---|
| bug | #1044 | start/schedule race | `watchdog.observers.api.BaseObserver` |
| feature | #1039 | observed watch without handlers | `watchdog.observers.api.BaseObserver` |
| bug | #1110 | symlink creation event classification | `watchdog.observers.polling.PollingEmitter` |
| feature | #1010 | emit existing files at startup | `watchdog.observers.polling.PollingEmitter` |
| bug | #1065 | polling survives unavailable mount | `watchdog.observers.polling.PollingEmitter` |
| feature | #1071 | optional flush event | `watchdog.observers.polling.PollingEmitter` |
| bug | #1000 | debouncer stop preserves queued events | `watchdog.utils.event_debouncer.EventDebouncer` |
| feature | #1043 | callable event handlers | `watchdog.events.FileSystemEventHandler` |
| bug | #999 | repeated debounce windows respect delay | `watchdog.utils.event_debouncer.EventDebouncer` |
| feature | #1100 | optional event-origin PID | `watchdog.events.FileSystemEvent` |

- [ ] Freeze the ten issue bodies and SHA-256 hashes.
- [ ] Write at least three acceptance criteria and one base-failing evaluator per row.
- [ ] Implement each reference patch and pass its evaluator plus focused observer/event tests.
- [ ] Integrate all ten references and pass all evaluators plus `python -m pytest -q`; if the last four rows lack an actual identical event-dispatch anchor, replace those rows before committing rather than weakening the gate.
- [ ] Commit `bench: curate watchdog real-world cell`.

### Task 12: Curate the Pendulum ten-task cell

**Files:**
- Create: `datasets/statefulbench-realworld/repos/pendulum.json`
- Create: `datasets/statefulbench-realworld/issues/pendulum.json`
- Create: `datasets/statefulbench-realworld/evaluators/pendulum/`
- Create: `datasets/statefulbench-realworld/references/pendulum/`

| kind | source | behavior family | overlap anchor |
|---|---|---|---|
| bug | #935 | subsecond precision beyond nine digits | `pendulum.parser.parse` block |
| feature | #917 | `parse_dt` DateTime-only API | `pendulum.parser.parse` block |
| bug | #797 | ordinal-day `DDDD` parsing | `pendulum.from_format` block |
| feature | #795 | strict `from_format` mode | `pendulum.from_format` block |
| bug | #761 | `from_format` honors explicit timezone | `pendulum.from_format` block |
| feature | #754 | construct from ISO year/week/day | `pendulum.DateTime` construction block |
| bug | #956 | negative pre-epoch timestamp conversion | `pendulum.from_timestamp` block |
| feature | #856 | calendar conversion extension point | `pendulum.DateTime` construction block |
| bug | #880 | naive DateTime diff remains naive | `pendulum.DateTime.diff` |
| feature | #788 | configurable `in_words` units | `pendulum.DateTime.diff` |

- [ ] Freeze the ten issue bodies and SHA-256 hashes.
- [ ] Write at least three acceptance criteria and one base-failing evaluator per row.
- [ ] Implement each reference patch and pass its evaluator plus focused parser/DateTime tests.
- [ ] Integrate all ten references and pass all evaluators plus `python -m pytest -q`; replace any proposed pair whose reference patches do not modify an identical production anchor.
- [ ] Commit `bench: curate pendulum real-world cell`.

### Task 13: Curate the Authlib ten-task cell

**Files:**
- Create: `datasets/statefulbench-realworld/repos/authlib.json`
- Create: `datasets/statefulbench-realworld/issues/authlib.json`
- Create: `datasets/statefulbench-realworld/evaluators/authlib/`
- Create: `datasets/statefulbench-realworld/references/authlib/`

| kind | source | behavior family | overlap anchor |
|---|---|---|---|
| bug | #780 | client-credentials refresh grant | `authlib.oauth2.client.OAuth2Client` |
| feature | #822 | reusable OAuth session | `authlib.oauth2.client.OAuth2Client` |
| bug | #783 | refresh uses access-token parameters | `authlib.oauth2.client.OAuth2Client` |
| feature | #632 | disable automatic expired-token refresh | `authlib.oauth2.client.OAuth2Client` |
| bug | #699 | fetch_token uses configured client credentials | `authlib.oauth2.client.OAuth2Client` |
| feature | #819 | async convenience methods | `authlib.integrations.httpx_client.AsyncOAuth2Client` |
| bug | #650 | async client-credentials refresh | `authlib.integrations.httpx_client.AsyncOAuth2Client` |
| feature | #619 | async compliance hooks | `authlib.integrations.httpx_client.AsyncOAuth2Client` |
| bug | #609 | JWT validator forwards now and leeway | `authlib.oauth2.rfc7523.JWTBearerTokenValidator` |
| feature | #756 | token lookup outside Authorization header | `authlib.oauth2.rfc6750.BearerTokenValidator` block |

- [ ] Freeze the ten issue bodies and SHA-256 hashes.
- [ ] Write at least three acceptance criteria and one base-failing evaluator per row.
- [ ] Implement each reference patch and pass its evaluator plus focused OAuth/JWT tests.
- [ ] Integrate all ten references and pass all evaluators plus `python -m pytest -q`; replace the last pair unless both references modify one identical resource-protector/token-validator production block.
- [ ] Commit `bench: curate authlib real-world cell`.

### Task 14: Add corpus qualification command

**Files:**
- Modify: `crates/stateful-bench/scripts/statefulbench_realworld.py`
- Modify: `crates/stateful-bench/scripts/tests/test_statefulbench_realworld.py`

**Interfaces:**
- Produces CLI: `python3 statefulbench_realworld.py qualify --manifest <path> --cache <dir> [--repo <key>]`
- Produces per-task fields: `base_red`, `reference_green`, `changed_anchors`
- Produces per-repo fields: `integrated_green`, `upstream_green`, `isolated_tasks`

- [ ] Write fake-repository RED tests proving qualification rejects an evaluator green on base, a reference still red, an integrated patch failure, an upstream regression, and an isolated overlap node.
- [ ] Run RED and confirm each rejection is absent because `qualify` does not exist.
- [ ] Implement subprocess execution with captured stdout/stderr artifacts and no shell strings; apply patches with `git apply --index`; create a fresh extraction for every individual task and one for integration.
- [ ] Run GREEN; then qualify all ten real corpora. Every row must show base RED/reference GREEN and every repository must show integrated/upstream GREEN with `isolated_tasks: []`.
- [ ] Commit `bench: qualify real-world task corpus`.

### Task 15: Add the three-arm real-world runner

**Files:**
- Modify: `crates/stateful-bench/scripts/statefulbench_realworld.py`
- Modify: `crates/stateful-bench/scripts/tests/test_statefulbench_realworld.py`

**Interfaces:**
- Reuses from `statefulbench_lite.py`: `AgentHandle`, `RunConfig`, `arm_stateful_server`, `launch_agent`, `_wait_agent`, `_empty_arm_result`
- Produces CLI: `run --manifest <path> --cache <dir> --out <dir> [--repos k1,k2] [--arms sequential,parallel-off,parallel-on] [--trials N]`
- Produces: `run_repo_arm(repo: dict, corpus: dict, arm: str, out_dir: Path, cfg: RunConfig, ...) -> dict`

- [ ] Write fake-launch RED tests proving sequential launch/wait ordering, concurrent task launch before waits, evaluator injection only after all ten task waits, final launch after injection, one shared server only for `parallel-on`, post-suite failure clears `cleared`, and result requires exactly eleven successful records.
- [ ] Run RED.
- [ ] Implement dynamic import of the lite runner and the minimal repository-specific orchestration. Preserve its timeout, usage parser, isolated OMP homes, and server lifecycle. Do not refactor the lite runner.
- [ ] Run GREEN and the existing `test_statefulbench_lite.py` suite.
- [ ] Commit `bench: run real-world three-arm corpus`.

### Task 16: Add aggregate reporting and provenance

**Files:**
- Modify: `crates/stateful-bench/scripts/statefulbench_realworld.py`
- Modify: `crates/stateful-bench/scripts/tests/test_statefulbench_realworld.py`

**Interfaces:**
- Produces per-arm `results.json`, aggregate `summary.json`, and terminal table
- Summary contains `model`, `thinking`, `trials`, `repositories`, `generated_at`, and arm rows with timing, tokens, tool calls, cleared count, and exact failures

- [ ] Write RED tests with two repos where one arm fails; assert failed rows remain present, aggregate cleared counts are exact, metrics are never averaged across missing rows, and source SHAs/checksums are copied into provenance.
- [ ] Run RED.
- [ ] Implement direct sums and explicit row counts; no confidence intervals or quality score.
- [ ] Run GREEN.
- [ ] Commit `bench: report real-world corpus efficiency`.

### Task 17: Document and validate the maintained workflow

**Files:**
- Modify: `docs/statefulbench-lite.md`
- Modify: `README.md`
- Modify: `~/.agents/skills/running-statefulbench/SKILL.md`
- Test: focused command/help and documentation consistency checks

- [ ] Document that `statefulbench-lite` remains the cheap synthetic smoke and `statefulbench-realworld` is the 100-task issue-derived corpus.
- [ ] Document freeze/qualification/run commands, 330-agent cost warning, evaluator isolation, source/result artifact paths, completion contract, and interpretation boundary.
- [ ] Update the personal skill with exact commands and prohibit reporting an unqualified corpus or partial arm set as a full result.
- [ ] Run both runner `--help` commands and the focused unittest suites.
- [ ] Commit `docs: add real-world statefulbench workflow`.

### Task 18: Run smoke, full benchmark, review, and deliver

**Files:**
- No source file is created until verification identifies a defect.
- Result artifacts live under `/private/tmp/statefulbench-realworld-<UTC>/`.

- [ ] Run `qualify` for all ten repositories and preserve its summary.
- [ ] Run one selected repository through all three arms with one trial; require three cleared rows before the full run.
- [ ] Run all ten repositories through all three arms with one trial.
- [ ] Inspect every non-cleared row and preserve exact logs; fix harness/runtime defects with a failing regression test, requalify, and rerun the affected full cell from a fresh output directory.
- [ ] Run final focused harness tests and the real-world manifest/corpus qualification check.
- [ ] Request task-scoped and whole-branch review; fix every Critical or Important finding and re-run affected checks.
- [ ] Commit only current-turn files, push `dev`, wait for GitHub checks when a PR exists, and report final CI state.
- [ ] Report configuration, completion/failures, efficiency rows, source artifacts, and the descriptive-only interpretation boundary exactly as defined in the design.
