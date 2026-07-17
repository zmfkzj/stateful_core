# Release Please 1.0.0 Bootstrap Design

## Problem

Merged release PR #13 is still labeled `autorelease: pending`. Release Please
therefore retries its obsolete 0.1.1 release on every push. GitHub rejects the
historical merge SHA when the Releases or Git Refs API tries to create tags.

The four crates and `.release-please-manifest.json` are at 0.1.1, but no
0.1.1 tags or GitHub Releases exist. The current `main` history already makes
Release Please calculate 1.0.0 for all four linked packages.

## Decision

Skip 0.1.1 and make 1.0.0 the first GitHub release:

- Remove `autorelease: pending` from PR #13. No 0.1.1 tags or releases will
  exist.
- Set `force-tag-creation` to `true` in `release-please-config.json`.
- Let Release Please create the 1.0.0 release PR so Cargo manifests,
  `.release-please-manifest.json`, and changelogs stay synchronized.
- Merge only after confirming every managed package is 1.0.0.
- Keep the approved `RELEASE_PLEASE_TOKEN` repository secret.

## Flow

1. Remove the pending label from merged PR #13.
2. Push the force-tag configuration.
3. Release Please opens a new 1.0.0 PR from current `main`.
4. Verify the four Cargo versions, release manifest, and changelogs.
5. Merge the reviewed release PR.
6. Release Please creates four tags at the new merge SHA, then four GitHub
   Releases.

If GitHub rejects tag creation for the new merge SHA, create the same four tags
with the verified Git-over-SSH path and rerun Release Please. Existing tags make
release creation retry-safe.

## Verification

Require the release workflow to pass after both the configuration push and the
release PR merge. Confirm these releases exist and all tags resolve to the new
release PR merge SHA:

- `stateful-core-v1.0.0`
- `stateful-store-v1.0.0`
- `stateful-server-v1.0.0`
- `stateful-cli-v1.0.0`

Confirm no `stateful-*-v0.1.1` tags or releases were created.

## Non-goals

- Publishing 0.1.1.
- Retargeting an old release to current `main`.
- Adding a custom tag-creation workflow step unless the built-in option fails.
- Publishing crates to an external package registry.
