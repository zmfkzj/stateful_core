# Release Please Force-Tag Design

## Problem

`release-please` 17.3.0 finds merged release PR #13 and sends its merge SHA
`3bcc980b00bc61fd09d580de7d4970bc056899e1` as `target_commitish` to GitHub's
Create Release API. GitHub returns 404 for that historical SHA even though it
is an ancestor of `main`. Using `main` succeeds but would make the 0.1.1 tags
point at 372 later commits.

## Decision

Set `force-tag-creation` to `true` in `release-please-config.json`.
Release Please will create each component tag at the release PR merge SHA before
creating its GitHub Release. The Releases API then uses the existing tag instead
of resolving the historical SHA.

Keep the approved `RELEASE_PLEASE_TOKEN` repository secret. The workflow already
prefers it and requires no YAML change.

## Flow

1. Release Please finds a merged release PR.
2. It creates each component tag at that PR's merge SHA.
3. It creates each GitHub Release from the existing tag.
4. It applies the normal release labels and continues managing release PRs.

A failed partial run remains safe to retry because existing tags and releases
are reused by Release Please.

## Verification

Push the configuration change and require the `release-please` workflow to pass.
Confirm these releases exist and each tag resolves to `3bcc980b00bc61fd09d580de7d4970bc056899e1`:

- `stateful-core-v0.1.1`
- `stateful-store-v0.1.1`
- `stateful-server-v0.1.1`
- `stateful-cli-v0.1.1`

No product tests are needed; the observable contract is the live GitHub release
and tag state.

## Non-goals

- Retargeting 0.1.1 to current `main`.
- Adding a custom tag-creation workflow step.
- Changing package versions or release notes.
