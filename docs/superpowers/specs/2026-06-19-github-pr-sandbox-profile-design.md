# GitHub PR Sandbox Profile Design

Date: 2026-06-19

## Problem

Stateful's `git` sandbox profile intentionally accepts only one validated
`git ...` command. That boundary works for committed-history operations such as
status, diff, commit, fetch, and push. Pull request creation is different: it is
a GitHub API operation, usually performed through `gh pr create`, not a git
protocol command.

Using the GitHub connector remains possible, but in enabled repositories
unclassified connector tools are blocked until the repo allowlist is updated.
That makes the common local workflow awkward: commit and push can run through
the git profile, but PR creation needs a separate connector allowlist step or a
raw `gh` command that the hook correctly denies.

## Decision

Add a separate GitHub PR sandbox profile instead of expanding the existing git
profile. The profile should cover only the small `gh pr` surface needed for
read-only PR inspection and draft PR creation:

- `gh pr list`
- `gh pr view`
- `gh pr status`
- `gh pr create`

This keeps the git profile's invariant intact while giving PR workflows a
policy-compliant local path.

## Non-Goals

- Do not allow general `gh api`.
- Do not allow repository administration commands such as `gh repo`.
- Do not allow merge, close, edit, review submission, checks rerun, secret,
  workflow, release, gist, or extension management.
- Do not replace the GitHub connector. Connector use remains valid when the
  repo tool allowlist explicitly permits it.
- Do not add broad GitHub automation beyond opening or inspecting pull
  requests.

## CLI Shape

Extend `SandboxFsProfile` with a new value:

```text
stateful sandbox run --fs github-pr --network enabled --command 'gh pr create ...'
```

The profile requires `--network enabled` because `gh pr` talks to GitHub. It
rejects explicit `--write-target`, `--create-target`, and `--write-dir` inputs,
matching the git profile's automatic scope style.

## Command Validation

The validator accepts one simple command with no shell control syntax. The first
word must be `gh`, the second word must be `pr`, and the third word must be one
of the allowed PR subcommands.

The validator denies:

- any command not starting with `gh pr`
- `gh api`
- `gh auth`
- `gh config`
- `gh extension`
- `gh repo`
- `gh secret`
- `gh workflow`
- `gh run`
- `gh release`
- `gh gist`
- `gh pr merge`
- `gh pr close`
- `gh pr edit`
- `gh pr review`
- `gh pr checks`
- `gh pr ready`
- shell control syntax, command substitution, redirects, pipes, and escapes
- environment assignment wrappers and nested shell launchers
- browser/editor launching flags such as `--web` where present

For `gh pr create`, the initial implementation should support the normal
non-interactive path: `--title`, `--body` or `--body-file`, `--base`, `--head`,
and `--draft`. Interactive prompting is disabled.

## Sandbox Scope

The command should run from the repo root with source files readable but not
writable. PR creation does not need to mutate the worktree or `.git`.

Writable scope should be limited to a private transient directory under the
repo's existing stateful sandbox area, for example:

```text
.git/stateful/github-pr/.stateful-tmp
```

For linked worktrees where `.git` is a file, use the existing fallback style
under `.stateful-git/`.

The sandbox should not write to persistent git metadata such as `.git/config`,
`.git/config.worktree`, or `.git/hooks`.

## Environment

Set deterministic non-interactive environment values:

- `GH_PROMPT_DISABLED=1`
- `GH_NO_UPDATE_NOTIFIER=1`
- `GH_FORCE_TTY=0`
- `GIT_TERMINAL_PROMPT=0`
- `GIT_EDITOR=:`
- `GIT_PAGER=cat`
- `PAGER=cat`

Remove inherited `GIT_*` variables using the same helper used by the git
profile, then inject only safe values needed for deterministic git behavior.
Do not copy or rewrite GitHub authentication files. The profile can read normal
`gh` auth configuration through the sandbox's read-only filesystem access.

## Hook Authorization

`authorize_sandbox_run_bash` should accept the new `github-pr` profile only
when:

- the executable is the trusted absolute `stateful` binary
- the request has `--fs github-pr`
- `--network enabled` is present
- there are no explicit write targets or write dirs
- the inner command passes the `gh pr` validator

Denied requests should point users to the new profile rather than suggesting the
git profile for PR creation.

## Documentation

Update the installed command-policy skill and README guidance:

- keep `git` profile guidance for git protocol operations
- state that PR creation uses `--fs github-pr`
- keep connector use documented as an alternate API path when allowlisted

## Testing

Add focused tests for:

- CLI parsing accepts `--fs github-pr`
- hook authorization allows `gh pr list`, `view`, `status`, and `create`
- hook authorization denies non-PR `gh` commands and dangerous PR subcommands
- the sandbox command builder invokes `gh` rather than `git`
- network disabled is rejected for the profile
- explicit write targets and write dirs are rejected
- persistent git metadata remains protected or unwritable
- command-policy text mentions the new PR profile

Existing git profile tests should remain unchanged except where shared helper
names are generalized.

## Migration And Compatibility

The change is additive. Existing `--fs git` commands keep their current
behavior, and existing GitHub connector workflows continue to require explicit
stateful tool classification in enabled repositories.

After implementing the profile, the trusted installed `stateful` binary must be
updated before Codex hooks can use the new profile in this checkout.
