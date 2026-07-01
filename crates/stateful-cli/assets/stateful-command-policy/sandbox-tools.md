# Sandbox Tools

Choose the narrowest existing entry point before writing a command.

For OMP entries below, strict trusted bare `stateful` means session-start or per-tool preflight has hash-verified the first PATH `stateful` binary against the installed Stateful binary; commands using the installed absolute binary path remain trusted. Arbitrary raw Bash/eval remains denied.

| Need | Codex | OMP | Do not use |
| --- | --- | --- | --- |
| File inspection after native tools are insufficient | `sandbox run --fs read-only --network disabled` | built-in Bash with strict trusted `stateful sandbox run --fs read-only --network disabled ...` | raw Bash, eval tools, networked read-only |
| Process inspection | `stateful sandbox process find ...` | built-in Bash with strict trusted `stateful sandbox process find ...` | `ps`, `pgrep`, process checks inside sandbox commands |
| Build, test, package manager, disposable generator output | `sandbox run --fs build --network enabled --write-dir <scratch-purpose>` | built-in Bash with strict trusted `stateful sandbox run --fs build ...` | raw tests/builds, repo `tmp` scratch |
| Non-git repo write command | `sandbox run --fs write-targets` with exact targets after reservation/claims | built-in Bash with strict trusted `stateful sandbox run --fs write-targets ...` after reservation/claims | native edits without file claims, broad dirs, git commands |
| Local git | `sandbox run --fs git --network disabled --command 'git <args>'` | built-in Bash with strict trusted `stateful sandbox run --fs git --network disabled ...` | raw git, write-targets |
| Remote git | same git profile with `--network enabled` | same | raw git, config-mutating remote setup |
| GitHub PR list/view/status/create | `sandbox run --fs github-pr --network enabled --command 'gh pr ...'` | built-in Bash with strict trusted `stateful sandbox run --fs github-pr ...` | raw `gh`, browser/editor PR flows |
| Read-only external shell command | `sandbox run --fs external --purpose ... --command ...` | built-in Bash with strict trusted `stateful sandbox run --fs external ...` | raw Bash/eval |
| External write/socket/signal scope | `sandbox run --fs external` with explicit scope | built-in Bash with strict trusted `stateful sandbox run --fs external ...`; prompts unless `stateful.autoApprove: true` | repo-internal profiles for external writes |
| Nested Codex benchmark | `sandbox run-nested-codex-benchmark ...` | not a generic OMP command path | generic relaxed profiles or nested wrapping |

## Git And PR Rules

- Use the git profile for `status`, `diff`, `log`, `add`, `commit`, `switch`/`checkout`, `merge`, `rebase`, `tag`, and `push`.
- Use network only for remote git operations such as `fetch`, `pull`, or `push`.
- The git profile rejects explicit write targets and protects persistent config/hooks. It rejects shell-dispatching options, path/exec overrides, inline/config-env config, disallowed subcommands such as `init` and `submodule`, branch upstream persistence, `push -u`, `rebase --exec`, grep pager dispatch, archive/fetch/push exec overrides, and config-mutating remote subcommands such as `add`, `set-url`, `rename`, and `remove`.
- Use the `github-pr` profile only for `gh pr <list|view|status|create>`. Use external read-only commands for other read-only `gh` calls, such as `gh api` or Actions log inspection.

## Build And Temp Rules

- Build profile commands must provide exactly one scratch purpose with `--write-dir <scratch-purpose>`. Scratch is external: `/tmp/stateful/<session>/<scratch-purpose>/`.
- Configure non-Cargo caches/outputs under the scratch root when the tool ignores standard temp variables.
- `tmp/` in the repo is disposable only. Do not store plans, notes, baselines, installable binaries, review evidence, or user-requested deliverables there.
- If an artifact must survive cleanup, use a purpose-named repo path with exact target/create scopes and `.gitignore` when needed.
- In parallel/subagent work, treat repo `tmp/<purpose>/` as a claimed resource; use unique children per active session.

## Write-Targets Rules

- `write-targets` targets must be repo-relative.
- Do not target `.git`, symlinks, paths outside the repo, or paths with control characters.
- Use `--write-target` for existing files, `--create-target` for new files, and `--write-dir <repo-dir>` only after declaring and claiming the exact trailing-slashed directory scope.
- Repo `tmp/` has no special write-dir exemption.
- `/dev/null` is writable inside the sandbox; do not declare it as a target.

## External Rules

- External profile requires `--purpose` and `--command`.
- Read-only external commands may omit write/create/directory/socket/signal scope.
- Absolute external scopes must resolve outside the repo.
- Repo-relative external write scopes require matching Stateful reservation and same-reservation claims.
- On macOS, read-only Go-based GitHub API commands such as `gh api` belong in external read-only because the Seatbelt profile permits the system identity/trust Mach lookups Go TLS needs.

## Avoid In Bash Or Eval Tools

- Raw Bash/eval for repo-internal commands, including quick `rg`, `git status`, `sed`, or language snippets.
- Shell wrappers around sandbox commands: environment assignments, command substitutions, outer redirects/pipelines, multiple commands, duplicate `--command`, or untrusted executable paths.
- Session-repair probes such as `stateful hook codex session-start`, `stateful hook omp session-start`, `stateful current`, `stateful notifications`, `stateful resume`, `stateful reservation declare/request/claim`, `strings <stateful>`, shell snapshots, or manual legacy session environment variables.
- Shell writes outside sandbox `--command`: `>`, `>>`, heredocs, and `| tee`.
- Direct mutation through raw `rm`, `mv`, `cp`, `mkdir`, `touch`, `chmod`, or `chown`.
- Raw process inspection (`ps`, `pgrep`, `ps auxww`, `ps -ef`, `ps -eo ...`) inside sandbox commands.
- `sandbox run --fs read-only --network enabled`.
- `sandbox run --fs build` without exactly one write dir.
