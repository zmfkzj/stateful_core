# Documentation / Implementation Conformance Audit — 2026-07-04

Audit fixed at commit `d3183cd3ab5774a8f21a63406c62278f61c673db`.

Scope: normative docs and implementation conformance for the presence-first/current-state coordination reframe. This report is evidence only: no code changes, no documentation fixes, no commit.

## Baseline

| Gate | Result | Evidence |
| --- | --- | --- |
| `git rev-parse HEAD` | pass | `d3183cd3ab5774a8f21a63406c62278f61c673db` |
| `cargo fmt --all --check` | pass | no output, exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | pass | `Finished dev profile` with no warnings-as-errors |
| `env -u STATEFUL_CODEX_RUN_ID -u CODEX_THREAD_ID cargo test --workspace` | pass | workspace test run completed with no failures; output included crate/unit/integration/doc tests |
| `cargo test -p stateful-cli --features codex-benchmark --test hook` | pass | `151 passed; 0 failed` |
| `python3 -m pytest crates/stateful-bench/scripts/tests` in scratch venv | pass | `42 passed`; pytest cache warning only because repo cache write was sandbox-denied |
| VS Code helper test probe | pass | `node --test integrations/vscode/test/*.test.js`: `6 pass; 0 fail` |

The live HTTP server was not left running. Server probes used focused Axum route tests, which instantiate `build_router` with isolated in-memory/temp state and avoid default port `43873`.

## Verdict Rules Used

- Code and targeted probes decide current behavior (`IS`).
- Documentation decides stated behavior (`SHOULD`).
- Shipped docs require code/probe evidence for `conforms`; agreement between docs alone is insufficient.
- Target/planning docs do not create shipped defects unless they contradict a shipped row or are presented as current operator behavior.
- Finding severity:
  - **P0**: shipped auth/data-loss/auth-boundary docs are unsafe if followed. None found.
  - **P1**: shipped surface or state semantics are wrong, missing, or internally contradictory.
  - **P2**: drift is real but lower-risk, mainly duplicate debt, test/docs verification mismatch, or benchmark/operator hygiene.
  - **P3**: typo/dead link/cosmetic. One seeded docs-meta typo promoted after review.

## Executive Summary

| Result class | Count | Notes |
| --- | ---: | --- |
| Sweep claim records | 193 | 10 clusters, capped per plan. |
| Verified findings | 20 | 8 P1, 11 P2, 1 P3. |
| P0 findings | 0 | No unsafe auth/data-loss doc defect found after verification. |
| Conforming sample rechecked | 14 | All 14 survived three independent verifier passes. |
| Refuted candidate findings | 1 | `directory_scope_depth: 2` is documented as target/informational and not a shipped defect. |
| Honesty / not-run items | 8 | Mostly true wall-clock, native registry alias, or end-to-end GUI/availability checks. |

### Seed Coverage

The plan's seeded drift set is covered as follows:

| Seed | Final disposition |
| --- | --- |
| Phase-aware authorization contradiction | Finding F04, verified 3/3. |
| 14 minimum SQLite tables vs 11 shipped tables | Finding F01, verified 3/3. |
| Legacy `intent_*` runtime config/loading concern | Matrix B-022/C-023: verified as conforming target-only docs; runtime uses Rust constants today. |
| CLI surface documentation divergence | Finding F06, verified 3/3. |
| Native `state_*` tool list divergence | Finding F06 / matrix G1-021..G1-025; runtime registry aliases remain in honesty appendix. |
| OCC/base-observation shipped semantics | Matrix D-012..D-015 / S09: verified conforming, including stale hard stops. |
| IDE save gate / human coordination status | Finding F09 plus F19 for the v1-hardening ledger split; matrix E-001..E-015 verified server/static VS Code behavior, real VS Code host noted in honesty appendix. |
| `stateful watch` documentation reversal/omission | Finding F06, verified 3/3. |
| Core concept / phase enum overstatement | Covered by F04 and D-002..D-003; shipped code implements phase checks. |
| Removed surface negative checks | Matrix G1-024 verified removed `state_file_write`/`state_bash_write`; no shipped removed-tool drift found. |
| Test-contract and CI coverage deltas | Findings F14 and F16, verified 3/3. |
| TTL/default duplication | Finding F18, verified as hygiene debt; values agree today. |
| Identity guidance duplication | Finding F18, verified as hygiene debt; OMP extension agrees today. |
| Availability/fail-open/fail-closed split | Finding F12 plus probe log; write auth fail-closed verified, actual GUI availability remains limited as noted. |
| Benchmark command/path claims | Findings F08 and F15, verified 3/3. |
| Docs-meta typo/dead-link candidates | Finding F20: the original duplicate-word grep patterns missed the seeded `Reservation, claim, reservation` typo. |

### Top Findings

1. **SQLite schema docs are stale**: docs claim 14 minimum tables and dead coordination indexes; migration ships 11 tables and drops conflicts/overrides/reconciliations.
2. **Retention pruning docs overclaim current behavior**: code prunes old events and expired/delivered notifications only.
3. **Protocol envelope docs drift**: docs say enforcement is limited to authorize/reservation routes and use top-level `identity`; code also envelopes `/v1/human/observe` and uses top-level `agent`.
4. **Phase-aware authorization docs contradict themselves**: shipped table says implemented; later text says target/not implemented; code implements phase denial/awareness softening.
5. **Reconcile ack contract is inconsistent**: docs/CLI mention `resources`, `conflict_with_plan`, and required `--reservation-id`; server request/store shape omits some fields and CLI parser accepts missing reservation id before server rejection.
6. **CLI/native surface inventory is stale**: implementation-contract and usage-reference omit shipped commands (`watch run`, `tools`, `codex`, reservation request/claim/cancel) or list native tools not backed by HTTP routes.
7. **Write-fence context items violate the item-field contract**: state-model says warn/block items require `next_action`; active write-fence warnings lack one.
8. **DeNovo benchmark defaults drift**: docs say official-style default prompt version is v2; Clap default remains v1.
9. **Hook timeout docs are only suggestions but read like implementation values**: docs name 750/1500/500ms; shared HTTP client uses 5s.
10. **Verification docs omit CI gates**: README development checks omit the codex-benchmark hook test and stateful-bench pytest run that CI executes.

## Findings

### P1 — Shipped Contract Drift

#### F01 — SQLite schema/table/index contract is stale

- Key: `store-schema/minimum-table-count`, `store-schema/dead-coordination-indexes`
- Docs: `docs/implementation-contract.md` SQLite Storage minimum tables and required indexes list `sessions`, `conflicts`, `overrides`, `reconciliations`, `conflicts(agent_id, checked_at)`, and `reconciliations(agent_id, created_at)`.
- Code truth: `crates/stateful-store/src/lib.rs::Store::migrate` creates 11 shipped tables: `schema_migrations`, `events`, `agents`, `activities`, `reservations`, `claims`, `write_fences`, `human_observations`, `wait_queue`, `notifications`, `outbox`. Migration renames/copies legacy `sessions` into `agents` and drops `conflicts`, `overrides`, and `reconciliations`; legacy conflict/reconciliation indexes are dropped.
- Verdict: `drift-docs-ahead` / `drift-code-ahead` after cutover.
- Severity: P1.
- Verification: 3/3 verifier passes confirmed. Seed 2 satisfied.
- Fix direction: Make `Store::migrate`/fresh schema the single source for the table/index list. Remove dead table/index rows or move them to a target-model note.

#### F02 — Retention pruning docs overclaim shipped scope

- Key: `lifecycle-ttl/retention-prune-scope`
- Docs: `docs/implementation-contract.md` and `docs/architecture.md` say pruning deletes old events, reconciliations, conflicts, human observations, and expired notifications.
- Code truth: `Store::prune_retention_before_inner` deletes old `events` and notifications with `status IN ('expired', 'delivered')`. It does not delete `human_observations`; conflict/reconciliation tables are not current shipped tables.
- Verdict: `drift-docs-ahead`.
- Severity: P1.
- Verification: 3/3 verifier passes confirmed.
- Fix direction: Document current retention exactly. If human-observation pruning is desired, add code/tests first; otherwise remove it from shipped retention language.

#### F03 — Protocol envelope docs drift from HTTP implementation

- Key: `http-api/protocol-envelope-enforcement`, `http-api/envelope-field-name`
- Docs: `docs/implementation-contract.md` says current envelope enforcement is limited to `/v1/authorize` and reservation declare/request/claim/cancel. Its common envelope example names top-level `identity`.
- Code truth: `crates/stateful-server/src/lib.rs::human_observe` also calls `protocol::require_v1_envelope`. Actual `RequestEnvelope` uses top-level `agent`, and server handlers destructure `envelope.request.agent`.
- Verdict: `drift-code-ahead` and `doc-defect`.
- Severity: P1.
- Verification: 3/3 verifier passes confirmed. Focused route tests verified normal envelope acceptance and protocol-mismatch error shape.
- Fix direction: Update the common envelope schema to `agent`; update the enforcement list to include `/v1/human/observe`, or relax the “limited to” wording.

#### F04 — Phase-aware authorization status contradicts itself

- Key: `policy/phase-aware-docs-contradiction`
- Docs: `docs/current-state-coordination.md` row 23 says phase-aware authorization is shipped; later text says phase-aware authorization and blocked-phase denies are target/not implemented. `docs/core-concept.md` also says `phase` includes `idle` and `expired`, which are current/context statuses rather than supported activity phases.
- Code truth: store reads active session phase; core policy defines `exploring`, `editing`, `testing`, `blocked`, `done`, and `failed`, then denies inactive/blocked/finalized phases in enforcement; policy service softens phase denials in awareness.
- Verdict: `contradiction-docs`; code supports shipped semantics.
- Severity: P1.
- Verification: 3/3 verifier passes confirmed.
- Fix direction: Keep the shipped row, delete or revise the later target/not-implemented text, and point to the current policy tests.

#### F05 — Reconcile ack CLI/API/store contract is inconsistent

- Key: `human/reconcile-ack-payload-fields`, `cli/reconcile-ack-reservation-id`
- Docs: `docs/state-model.md`, `docs/usage-reference.md`, and `docs/implementation-contract.md` describe/require `resources`, `files_reread`, `conflict_with_plan`, and `--reservation-id` for reconcile ack.
- Code truth: `crates/stateful-server/src/lib.rs::ReconcileAckRequest` has `agent_id`, `workspace_id`, optional `reservation_id`, `decision`, `files_reread`, and `human_change_summary`; no `resources` or `conflict_with_plan`. CLI accepts `resources` and `conflict_with_plan`, but server request deserialization ignores extra fields. CLI parser makes `reservation_id` optional, then server rejects missing reservation id.
- Verdict: `drift-docs-ahead` / CLI-server shape mismatch.
- Severity: P1.
- Verification: 3/3 verifier passes confirmed.
- Fix direction: Either persist/accept the documented fields or remove them from shipped API docs. Make CLI parser require `--reservation-id` if the server requires it.

#### F06 — CLI and native tool surface inventories are stale/divergent

- Key: `cli/root-surface-watch-docs`, `native/activity-conflicts-list-divergence`, `cli/server-join-allow-plain-http-docs`
- Docs: implementation-contract CLI surface omits shipped root commands and reservation subcommands; usage-reference omits `watch`; README names bare `stateful watch`; architecture native list includes `state_activity_observe` and `state_conflicts_check` while usage-reference omits them. The implementation-contract `server join` synopsis omits shipped `--allow-plain-http`.
- Code truth: root help and `Command` enum expose `install`, `server`, `status`, `current`, `events`, `doctor`, `codex`, `sandbox`, `enable`, `disable`, `repos`, `tools`, `notifications`, `resume`, `reservation`, `human`, `reconcile`, `watch`, `sync-outbox`, and `hook`. `WatchCommand` is `run`. `server join` exposes `--allow-plain-http`, and `join_server_runtime` validates the base URL with that flag. Server routes have no `/v1/conflicts/check`.
- Verdict: `drift-code-ahead` and `contradiction-docs`.
- Severity: P1.
- Verification: 3/3 verifier passes confirmed. Phase 0 root help matched the code enum; review spot-check confirmed `server join --allow-plain-http`.
- Fix direction: Generate or manually synchronize one CLI/native surface table from the Clap/server route truth; update README to `stateful watch run`; document `server join --allow-plain-http` with an explicit transport-security warning.

#### F07 — Context item contract is violated for active write fences

- Key: `rendering/fence-next-action`
- Docs: `docs/state-model.md` says `next_action` is required for `block` and `warn` current/context items.
- Code truth: `crates/stateful-store/src/write_fences.rs::live_write_fence_items` emits active write-fence items with `severity = Warn` but no `with_next_action(...)`; human block items do set a reconciliation next action.
- Verdict: `drift-docs-ahead` or implementation gap.
- Severity: P1.
- Verification: 3/3 verifier passes confirmed.
- Fix direction: Add a concise next action for write-fence warnings, or change the state-model field rule to exempt informational in-flight write warnings.

#### F08 — DeNovo prompt-version default is documented as v2 but implemented as v1

- Key: `bench/denovo-prompt-v2-default`
- Docs: `docs/denovo-benchmark-guide.md` and commands docs say official-style DeNovo defaults use `--prompt-version v2`.
- Code truth: `crates/stateful-bench/src/denovo.rs` Clap default for `denovo run --prompt-version` is `v1`. Example commands explicitly pass `v2`, so copied commands are safe; default wording is wrong.
- Verdict: `drift-docs-ahead`.
- Severity: P1.
- Verification: 3/3 verifier passes confirmed.
- Fix direction: Change the Clap default to `v2` or rewrite docs to say examples pass `v2` while the CLI default remains `v1` for compatibility.

### P2 — Lower-Risk Drift and Documentation Debt

#### F09 — IDE observation implementation is ahead of architecture docs

- Key: `human/vscode-observation-docs-conflict`
- Docs: `docs/architecture.md` says richer IDE observation for opened, dirty, selected, and save-complete telemetry remains future work.
- Code truth: `integrations/vscode/extension.js` already posts low-confidence `presence` on open, low-confidence `dirty` on change, and high-confidence `save` on did-save. No selected-file sensor was found.
- Verdict: `drift-code-ahead`.
- Severity: P2.
- Verification: 3/3 verifier passes confirmed; Node tests passed 6/6 for helper behavior.
- Fix direction: Split implemented telemetry from the remaining selected-file future work.

#### F10 — `write-targets` docs omit shipped repo-relative `--write-dir`

- Key: `sandbox/write-targets-write-dir-docs`
- Docs: `docs/implementation-contract.md` describes `--fs write-targets` with `--write-target` and `--create-target` only.
- Code truth: CLI and sandbox validation also support repo-relative `--write-dir` for `write_directory` authorization under `write-targets`; tests cover it.
- Verdict: `doc-defect`.
- Severity: P2.
- Verification: 3/3 verifier passes confirmed.
- Fix direction: Add `--write-dir <repo-dir>` to the implementation contract with the exact directory-claim requirement.

#### F11 — Hook timeout values are documented as suggested but not implemented as stated values

- Key: `hooks/timeout-values`
- Docs: implementation-contract suggests authorization/context/observation hook timeouts of 750/1500/500ms.
- Code truth: shared runtime HTTP helpers use a 5s read timeout; no per-hook constants were found in the verified paths.
- Verdict: documentation ambiguity / lower-risk drift.
- Severity: P2.
- Verification: 2/3 verifier passes retained; one verifier refuted hard drift because the docs say “suggested.”
- Fix direction: Either implement the suggested per-hook values or explicitly label them as non-binding design guidance.

#### F12 — OMP unavailable behavior has two fail-closed shapes

- Key: `hooks/omp-unavailable-structured-block`
- Docs: architecture says unavailable Stateful maps to a hard OMP block.
- Probe truth: if runtime identity succeeds and `/v1/authorize` fails, CLI prints structured `{"decision":"block",...}`. If `STATEFUL_SERVER_URL` runtime identity itself is refused, CLI exits non-zero with no stdout; the OMP JS extension maps that non-zero status to `{ decision: "block" }`.
- Verdict: behaviorally fail-closed at extension boundary, but docs should not imply structured CLI JSON for all unavailable cases.
- Severity: P2.
- Verification: 3/3 verifier passes confirmed the split behavior.
- Fix direction: Document the two adapter paths: structured CLI block after runtime discovery, extension-level block for hook subprocess failure.

#### F13 — Human observe CLI parser does not enforce documented enum values client-side

- Key: `cli/human-observe-option-values`
- Docs: CLI surface shows `--kind save|change|delete|presence|dirty` and `--confidence high|low`.
- Code truth: Clap stores both as unconstrained `String`; server-side parsing rejects invalid values later.
- Verdict: client-side parser drift, not server-policy drift.
- Severity: P2.
- Verification: 3/3 verifier passes confirmed.
- Fix direction: Use Clap value enums or make docs say invalid values fail at the server/API layer.

#### F14 — Prompt renderer “golden tests” are actually assertion tests

- Key: `docs-meta/prompt-renderer-golden-tests`
- Docs: implementation-contract claims prompt renderer golden output tests.
- Code truth: `crates/stateful-core/tests/context.rs` and route tests use assertions/substrings; no golden/snapshot/insta fixture was found.
- Verdict: `drift-docs-ahead`.
- Severity: P2.
- Verification: 3/3 verifier passes confirmed.
- Fix direction: Either add a real golden fixture or rename the test-contract row to assertion coverage.

#### F15 — DeNovo output path documentation mixes roots

- Key: `bench/denovo-output-paths`
- Docs: benchmark docs reference `.stateful_bench/denovo/run-control`, `target/stateful_bench_runs/denovo/runs`, and historical `target/stateful-bench/denovo/...` paths.
- Code truth: DeNovo code defaults to `.stateful_bench/denovo/runs` / `.stateful_bench/denovo/extracts`; command docs override output roots in places.
- Verdict: documentation path hygiene drift.
- Severity: P2.
- Verification: 3/3 verifier passes confirmed.
- Fix direction: Pick one default/output-root story and label historical target paths as historical or scratch examples.

#### F16 — README development verification omits CI gates

- Key: `docs-meta/ci-verification-docs`
- Docs: README Development lists fmt, clippy, and workspace cargo tests. `docs/implementation-contract.md` §Verification lists only fmt and workspace tests.
- CI truth: `.github/workflows/rust.yml` also runs `cargo test -p stateful-cli --features codex-benchmark --test hook` and `python -m pytest crates/stateful-bench/scripts/tests`.
- Verdict: `drift-code-ahead` for verification docs.
- Severity: P2.
- Verification: 3/3 verifier passes confirmed. Phase 0 ran both omitted gates successfully.
- Fix direction: Add the two CI-only gates to README and implementation-contract verification text, or link both to one canonical verification section.

#### F17 — Nested Codex benchmark sandbox surface is visible but underdocumented

- Key: `sandbox/nested-codex-benchmark-visible-undocumented`
- Docs: implementation-contract mentions nested Codex benchmark sandbox hook authorization as feature-gated test coverage.
- Code truth: `SandboxCommand::RunNestedCodexBenchmark` and `codex_benchmark.rs` implement a visible nested runtime command and macOS profile behavior.
- Verdict: `drift-code-ahead` / underdocumented internal surface.
- Severity: P2.
- Verification: 3/3 verifier passes confirmed.
- Fix direction: Either hide/internalize the subcommand or document it as benchmark-only with feature/sandbox constraints.

#### F18 — Duplicate TTL and OMP identity guidance is hygiene debt

- Key: `docs-meta/duplicate-ttl-tables`, `docs-meta/omp-agent-id-duplication`
- Docs: TTL/defaults and OMP `agent_id` derivation are repeated across multiple docs.
- Code truth: values currently agree with Rust constants; OMP identity docs agree with the generated extension and tests.
- Verdict: `doc-defect` hygiene, not behavior drift.
- Severity: P2.
- Verification: 3/3 verifier passes confirmed consistency.
- Fix direction: Keep one canonical table/paragraph and link to it from other docs.

#### F19 — v1-hardening implementation-order item 7 is stale on the shipped save-gate half

- Key: `docs-meta/v1-hardening-item-7-stale`
- Docs: `docs/v1-hardening-scope-decisions.md` item 7 says “Remaining: add IDE save gate API and harden native edit hook target extraction.”
- Code truth: this report already verified the save-check API and extension/static save gate as shipped in E-004, E-011, E-012, and related probes. Native edit hook target extraction hardening remains separate.
- Verdict: `drift-docs-ahead` for the first half of a compound implementation-order item.
- Severity: P2.
- Verification: review spot-check confirmed the ledger contradiction against the report's verified save-gate records.
- Fix direction: Split item 7: mark the IDE save gate API as Done, and leave native edit hook target extraction hardening as Remaining.

### P3 — Low-Risk Text Hygiene

#### F20 — Runtime-config window sentence has a duplicated noun from rename residue

- Key: `docs-meta/runtime-config-window-typo`
- Docs: `docs/implementation-contract.md` line 750 says “Reservation, claim, reservation, directory-scope, and retention windows,” making it ambiguous which window class maps to a built-in Rust constant.
- Code truth: the report's Phase 0 duplicate-word probe only checked `the the` and `to to`, so it did not cover the seeded `Reservation, claim, reservation` location.
- Verdict: `doc-defect` typo/hygiene.
- Severity: P3.
- Verification: review spot-check confirmed the exact sentence at `docs/implementation-contract.md:750`.
- Fix direction: Replace the duplicate `reservation` with the intended runtime-config window name, or rewrite the list from the actual constants.

## Improvement Recommendations

| Recommendation | Effort | Findings | Action |
| --- | --- | --- | --- |
| Regenerate the shipped storage schema table from `Store::migrate` | S | F01, F02 | Replace 14-table/dead-index docs with the 11-table schema and current indexes. |
| Narrow retention docs to current code | S | F02 | State events + expired/delivered notifications only; move human-observation pruning to target if desired. |
| Make protocol envelope examples match structs/tests | S | F03 | Use `agent`, include `/v1/human/observe`, link to protocol tests. |
| Resolve phase-aware authorization status in one place | S | F04 | Delete target-era contradiction and cite shipped tests. |
| Align reconcile ack CLI/API/store shape | M | F05 | Either persist `resources`/`conflict_with_plan` or remove them; make CLI require server-required args. |
| Generate CLI/native surface docs from source or help dumps | M | F06, F13, F17 | Avoid hand-maintained command lists; include `server join --allow-plain-http` and its transport warning. |
| Add write-fence `next_action` or relax the item invariant | S | F07 | Small code or doc change; test current/context rendering. |
| Clarify benchmark defaults and paths | S | F08, F15 | Pick one default prompt/path story; mark compatibility defaults explicitly. |
| Move repeated constants/identity text to canonical anchors | S | F18 | Single-source docs; other pages link. |
| Split stale v1-hardening implementation-order item 7 | S | F19 (`docs-meta/v1-hardening-item-7-stale`) | Mark IDE save gate API Done; keep native edit hook target extraction hardening Remaining. |
| Fix seeded runtime-config window typo | S | F20 (`docs-meta/runtime-config-window-typo`) | Correct the duplicated `reservation` noun in the built-in window sentence. |
| Add omitted CI gates to development docs | S | F16 | Include feature hook test and bench pytest in README and implementation-contract verification text. |

## Claim Matrix Appendix

Legend: `V` verified conforming, `F` verified finding, `H` honesty/not-run/untestable, `R` refuted as finding.

| Cluster | Claims | Matrix |
| --- | ---: | --- |
| A HTTP | 19 | A-001 V route-table; A-002 V health-public; A-003 V protected-routes-auth; A-004 V bearer-token-format; A-005 V unauthorized-error-shape; A-006 F protocol-envelope-enforcement; A-007 F envelope-field-name; A-008 V protocol-mismatch-error; A-009 V authorize-decision-output; A-010 V current-success-envelope; A-011 V generic-error-envelope; A-012 V declare-purpose-files-planned; A-013 V authorize-queue-purpose; A-014 V authorize-reservation-claim-boundary; A-015 V authorize-lazy-claim-boundary; A-016 V authorize-supported-actions after probe; A-017 V write-auth fail-closed after hook probe; A-018 H human-save fail-open availability; A-019 H/partial OMP unavailable boundary split. |
| B Store | 22 | B-001 F minimum-table-count; B-002 V schema-migrations; B-003 V events; B-004 F sessions-vs-agents-table; B-005 V current-state-tables; B-006 F dead-coordination-tables; B-007 V events-indexes; B-008 V current-state-indexes; B-009 F dead-coordination-indexes; B-010 V outbox-legacy-migration; B-011 V notification-sequence-migration; B-012 V outbox-index; B-013 V retention-window; B-014 F retention-prune-scope; B-015 V retention-preserve-current-state; B-016 V outbox-idempotence; B-017 V outbox-sequence-and-status; B-018 V event-materialization-transaction; B-019 V mutation-rollback; B-020 V generic-transaction-rollback; B-021 V maintenance-transaction-equivalence; B-022 V runtime-config-retention. |
| C Lifecycle | 24 | C-001 V reservation-declare-scope; C-002 V active-reservation-ttl; C-003 V heartbeat-refresh-caps; C-004 V activity-ttl; C-005 V claim-acquire-authority; C-006 V claim-conflict-reserved-waiter; C-007 V claim-release-promotion; C-008 H claim-expiry wall-clock; C-009 V wait-request-idempotency; C-010 V wait-fifo-promotion; C-011 V wait-allowed-actions after probe; C-012 V awareness-queue-suppression after probe; C-013 V notification-poll-delivery; C-014 V/H SSE primitives verified, live frame not run; C-015 V notification-payload-purpose; C-016 V reservation-cancel; C-017 V finalization-release-cancel; C-018 V write-fence-conflict; C-019 H write-fence wall-clock expiry; C-020 F retention-pruning; C-021 V notification-sequence-backfill; C-022 F duplicate-ttl-tables; C-023 V config-runtime-loading; C-024 H resume native surface not run. |
| D Policy | 22 | D-001 F phase-aware-docs-contradiction; D-002 V phase-blocked-enforcement; D-003 V phase-blocked-awareness; D-004 V exact-file-scope-write; D-005 V directory-scope-exactness; D-006 V delete-exact-file-scope; D-007 V rename-move-exact-source-destination; D-008 V rename-move-missing-paths; D-009 V same-workspace-relative-hard-conflict; D-010 V directory-claim-subtree-hard-conflict; D-011 V coordination-mode-soft-denials; D-012 V stale-base-observation-enforcement; D-013 V stale-base-observation-awareness; D-014 V missing-base-observation-hard-stop; D-015 V stale-claim-observation-hard-stop; D-016 V unreconciled-human-write-stop; D-017 V write-fence-hard-stop; D-018 V absolute-path-domain-target; D-019 V repo-relative-soft-warning-target; D-020 V rename-move-no-queue; D-021 V unsupported-actions-both-modes; D-022 R depth-2-directory-collision-warning. |
| E Human/IDE | 17 | E-001 V human-observe-route; E-002 V human-observe-kinds-confidence; E-003 V high-confidence-write-block; E-004 V human-save-check-route; E-005 V human-commands; E-006 V reconcile-ack-route; E-007 V reconciliation-decisions; E-008 F reconcile-ack-payload-fields; E-009 V unreconciled-write-denial; E-010 V current-state-human-block; E-011 V save-gate-events; E-012 V vscode-static-save-gate; E-013 V vscode-save-continue; E-014 F vscode-observation-docs-conflict; E-015 V/H node helper tests pass, real VS Code host not run; E-016 H static-base-observation delegated to policy/hook; E-017 V shadow-guard-role. |
| F Sandbox | 18 | F-001 V raw-bash-full-deny; F-002 V raw-bash-outer-wrapper-deny; F-003 V canonical-read-only; F-004 V sequence-behavior; F-005 V write-targets-authorization; F-006 F write-targets-write-dir-docs; F-007 V build-scratch-scope; F-008 V build-rejects-explicit-targets; F-009 V external-scope; F-010 V omp-bash-preflight; F-011 V process-find; F-012 V network-policy; F-013 V git-profile; F-014 V github-pr-profile; F-015 V artifact-temp-scope; F-016 F nested-codex-benchmark-runtime; F-017 V codex-benchmark-feature-tests; F-018 V/H runtime-availability split probed. |
| G1 CLI/native | 25 | G1-001 F root-surface-matrix; G1-002 V root-help-vs-command-enum; G1-003 F implementation-contract-root-omissions; G1-004 F usage-reference-root-overview; G1-005 F watch-run-usage-omission; G1-006 F watch-readme-reversal; G1-007 F watch-contract-omission; G1-008 V watch-behavior; G1-009 V notifications-poll; G1-010 V resume-next; G1-011 V reservation-declare; G1-012 F reservation-request docs gap; G1-013 F reservation-claim docs gap; G1-014 F reservation-cancel docs gap; G1-015 V human-observe-save-check; G1-016 F human-observe-option-values; G1-017 F reconcile-ack-reservation-id; G1-018 V/H reconcile multiple resources is benign code-ahead; G1-019 F sandbox-run-nested-codex-benchmark; G1-020 F server-join-allow-plain-http docs gap; G1-021 H native tool list runtime not verified; G1-022 F native activity/conflicts list divergence; G1-023 H native reconcile aliases not verified; G1-024 V removed-write-tools; G1-025 H native claim acquire runtime not verified. |
| G2 Packaging/runtime | 22 | G2-001 V global-install-files; G2-002 V codex-install-assets; G2-003 V codex-hook-config; G2-004 V omp-install-assets; G2-005 V omp-config-merge; G2-006 V omp-agent-id; G2-007 V codex-session-id; G2-008 F omp-agent-id-duplication; G2-009 V runtime-env-discovery; G2-010 V runtime-file-order; G2-011 V runtime-file-schema-perms; G2-012 V stale-malformed-hardening planning; G2-013 V default-workspace; G2-014 V repo-registry; G2-015 V adapter-command-set; G2-016 V native-edit-authorization; G2-017 V bash-eval-adapter-policy; G2-018 V codex-bash-adapter-policy; G2-019 F timeout-values; G2-020 V fail-closed-authorization; G2-021 H fail-open observation/context not run; G2-022 V managed-hook-plugin-ux planning. |
| H Rendering | 10 | H-001 V context-render-route; H-002 V current-view-and-cli; H-003 V native-context-render-api by hook/HTTP evidence; H-004 V sections-caps-evidence; H-005 V active-scope; H-006 V wait-queue-and-claimable; H-007 V resource-workspace-identity-filtering; H-008 V notifications-resume-visibility; H-009 F fence-and-human-content-rendering; H-010 F prompt-renderer-golden-tests. |
| J Bench | 14 | J-001 V top-level-bench-subcommands; J-002 V denovo-run-command-flags; J-003 F denovo-prompt-v2-default; J-004 V denovo-report-compare; J-005 F denovo-output-paths; J-006 H denovo-shard-control-paths; J-007 V programbench-run-command-flags; J-008 V programbench-default-matrix; J-009 V programbench-eval-report-compare; J-010 V programbench-official-eval-steps; J-011 H programbench-python310-caveat; J-012 V artifact-status; J-013 F script-pytest-existence/docs; J-014 F ci-verification-docs. |

Matrix cross-reference notes: G1-020 `server-join-allow-plain-http` maps to F06; E-004/E-012 save-gate shipped records map to F19 (`docs-meta/v1-hardening-item-7-stale`); B-022 runtime-config retention wording maps to F20 (`docs-meta/runtime-config-window-typo`). These notes do not add claim rows, so the cluster sum remains 193.

## Probe Log Appendix

### Server probes

Focused route tests run through `stateful sandbox run --fs build --network enabled`:

- `health_is_public_but_authorize_requires_token`: verified public `/health` and protected `/v1/authorize` no-token 401.
- `runtime_identity_requires_token_and_returns_process_identity`: verified protected normal bearer response envelope.
- `side_effecting_routes_reservation_declare_accepts_protocol_envelope`: verified normal v1 protocol envelope acceptance.
- `authorize_rejects_legacy_body_after_protocol_enforcement`: verified 400 `decision:error` / `reason_code:protocol_mismatch`.
- `queue_on_conflict_without_intent_enqueues_wait_record`: verified enforcement active-claim conflict queue side effect.
- `concurrent_codex_agents_transfer_native_edit_access_through_request_claim_and_lease`: verified full same-path transfer lifecycle.
- `awareness_mode_warns_and_records_audit_event_for_soft_denial`: verified awareness warning for broad denial.
- `authorize_denies_when_target_changed_since_base_observation`: verified stale target observation deny.
- `awareness_mode_still_denies_unreconciled_human_write_without_reservation`: verified awareness retains hard safety stops.
- Action-gap exact tests verified `write_file`, `write_directory`, `delete_file`, `rename_file`, and `move_file` support, plus reservation request validation.

### Hook / VS Code / sandbox probes

- Codex normal hook path: `session_start_registers_explicit_agent_without_current_file` passed.
- OMP normal hook path: `run_hook_omp_pre_tool_use_prints_extension_decision` passed.
- Codex malformed pre-tool stdin: minimal CLI probe exited non-zero on JSON parse error.
- OMP malformed raw CLI stdin: minimal probe exited non-zero on JSON parse error; real extension constructs JSON payload itself and maps non-zero hook status to block.
- Read-only sandbox unavailable runtime: `pre_tool_use_allows_read_only_sandbox_when_runtime_unreachable` passed.
- Native write unavailable runtime: `pre_tool_use_denies_native_write_when_runtime_unreachable` passed.
- Authorization connection drop: `pre_tool_use_edit_denies_when_authorize_connection_drops` passed.
- OMP server denial under yolo: `omp_yolo_does_not_downgrade_server_denial` passed.
- OMP unavailable split: valid runtime identity + `/v1/authorize` drop returned `decision:block`; env runtime identity connection refused exited `1` and extension maps that to block.
- VS Code helper tests: `node --test integrations/vscode/test/*.test.js` passed 6 tests.
- Sandbox smoke: canonical read-only allowed; read-only network enabled denied; raw read-only Bash denied; OMP Bash preflight only allows trusted sandbox request.

## Honesty Appendix

- No long-running live state server was started; route tests provided router-level runtime evidence.
- True wall-clock TTL expiry and 14-day retention were not time-traveled; code paths and targeted tests were read.
- Native `state_*` registry/alias exposure was not fully verified in a running harness; docs and HTTP/CLI/hook surfaces were compared.
- Human save-check fail-open under an actually unavailable server was not fully end-to-end probed; VS Code helper tests and static extension code support the behavior.
- Observation/context-render fail-open on timeout was not end-to-end probed.
- Real VS Code host behavior was not run; Node tests cover helper behavior, body shape, hashing, and conflict message rendering.
- OMP malformed raw CLI stdin is not a normal extension path because the extension sends `JSON.stringify(payload)`; CLI parse failure and extension nonzero-to-block behavior were separately verified.
- `directory_scope_depth: 2` was refuted as a shipped finding: docs label config defaults as informational/target and exact directory semantics match shipped code.
- Duplicate-word grep probes for `the the` and `to to` found no matches in `docs`/`README.md`, but those patterns did not cover the seeded `Reservation, claim, reservation` typo at `docs/implementation-contract.md:750`; F20 records the confirmed P3 hygiene finding.
