# OMP Tools

## Use OMP-Native Stateful Tools

- If Stateful MCP tools are missing from the active OMP tool list, use `search_tool_bm25` once with a query such as `stateful state current read`, then call the activated runtime-specific tool names.
- Never fall back to Bash for Stateful coordination.
- OMP raw Bash and Python/JavaScript/JS/Ruby/Julia eval tools are denied before command text matters, including attempts to wrap valid `stateful sandbox run` commands.
- Use `process_find` for process inspection.
- Use `sandbox_bash` for non-external sandbox profiles: `read-only`, `write-targets`, `build`, `git`, and `github-pr`.
- Use `ext_ro_bash` for read-only `--fs external` commands with only `purpose` and `command`; it runs without an OMP UI confirmation.
- Use `ext_rw_bash` for external writes or external socket/signal scope. It prompts for a scoped OMP UI grant unless `stateful.autoApprove: true` or per-call `auto_approve: true` is set. Auto-approval skips only the Stateful-owned UI prompt; sandbox validation, hooks, reservation/claim checks, and grant limits still apply.

## Installed OMP Profile

OMP installs the integration into the live OMP profile agent directory, defaulting to `~/.omp/profiles/stateful/agent`, with:

- the Stateful extension,
- `rules/stateful-required.md`,
- `skills/stateful-command-policy/SKILL.md` plus support files,
- `sandbox_bash`, `ext_ro_bash`, `ext_rw_bash`, and `process_find` tools.

The installer merges an existing `config.yml` and rejects invalid YAML. Without `stateful install --agent omp --update`, existing OMP scalar config values are preserved and only missing Stateful keys are inserted. With `--update`, targeted OMP scalar values are overwritten to delegate safety to Stateful hooks while raw Bash/eval denials and sandbox confirmations remain hook-enforced.

## Generated Tool Behavior

- `sandbox_bash`, `ext_ro_bash`, and `ext_rw_bash` run in the background by default. With `async` omitted or `true`, they return a job id immediately and later post stdout, stderr, and exit status. Set `async: false` for awaited foreground behavior.
- Generated sandbox tools expand unquoted, single-quoted, and double-quoted `skill://<name>` and `skill://<name>/<relative-path>` references in the command argument to files under the installed OMP agent `skills/` directory. Unknown skills, query/hash suffixes, and traversal paths are rejected.
- Generated `process_find` invokes `stateful sandbox process find` directly. By default it includes safe metadata fields such as `pid`, `ppid`, `pgid`, `user`, `uid`, `stat`, `start`, `etime`, `time`, `pcpu`, `pmem`, `rss`, `vsz`, `nice`, `pri`, `tty`, and `comm`; command, argv, and env are not exposed by default.
- The OMP extension subscribes to Stateful SSE notifications after `session-start`. When a queued reservation is claimable, it injects a next-turn message with the `wait_id`, action/path, and purpose. Agents must still reread the target and call `state_reservation_claim` or rely on an authorized lazy-claim write boundary.
- Blocked OMP `edit` calls with safe repo-relative line targets can be stored as live-session lazy edit operations. Wait-queue denials use the `wait_id`; `missing_reservation` and `missing_claim` denials use a generated operation id.

## DeNovo OMP Runs

Stateful-bench DeNovo OMP runs use isolated per-instance OMP homes. `stateful:on` conditions install the integration into that isolated home. Docker-backed stateful OMP runs use `/home/stateful` as the container runtime home so the mounted integration path is visible inside the agent. `stateful:off` / no-state conditions may still use an isolated OMP home, but must not be treated as Stateful sessions or repaired with Stateful MCP/sandbox guidance unless the benchmark prompt explicitly enables Stateful.
