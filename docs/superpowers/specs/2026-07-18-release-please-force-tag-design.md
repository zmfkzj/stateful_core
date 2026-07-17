# Release Please 1.0.0 Bootstrap Design

## Problem

Merged release PR #13 is still labeled `autorelease: pending`. Release Please
therefore retries its obsolete 0.1.1 release on every push. GitHub rejects the
historical merge SHA when the Releases or Git Refs API tries to create tags.

The four crates and `.release-please-manifest.json` are at 0.1.1, but no
0.1.1 tags or GitHub Releases exist. The current `main` history already makes
Release Please calculate 1.0.0 for all four linked packages.

The custom group title `chore: release stateful workspace` is also static.
Release Please requires a named placeholder when parsing merged release PR
titles, so it can create the PR but cannot later turn that PR into releases.

## Decision

Skip 0.1.1 and make 1.0.0 the first GitHub release:

- Remove `autorelease: pending` from PR #13. No 0.1.1 tags or releases will
  exist.
- Set `force-tag-creation` to `true` in `release-please-config.json`.
- Remove the static `group-pull-request-title-pattern` and use Release Please's
  parseable default `chore: release ${branch}`.
- Let Release Please create the 1.0.0 release PR so Cargo manifests,
  `.release-please-manifest.json`, and changelogs stay synchronized.
- Merge only after confirming every managed package is 1.0.0.
- Keep the approved `RELEASE_PLEASE_TOKEN` repository secret.

## Flow

1. Remove the pending label from merged PR #13.
2. Push the force-tag configuration without the custom group title pattern.
3. Release Please opens `chore: release main` for 1.0.0.
4. Verify the four Cargo versions, release manifest, and changelogs.
5. Squash-merge the reviewed release PR.
6. Release Please parses the default title, creates four tags at the new merge
   SHA, then creates four GitHub Releases.

If GitHub rejects tag creation for the new merge SHA, create the same four tags
with the verified Git-over-SSH path and rerun Release Please. Existing tags make
release creation retry-safe.

## Verification

The configuration and post-merge release workflows must pass. The four releases
must exist, and every tag must resolve to release PR #22's merge SHA
`ff6deac19b06da82cb8dccc4bb43aca0cab4f0db`:

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
