# Release Please 1.0.0 Bootstrap Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Skip the blocked 0.1.1 release and create synchronized 1.0.0 tags and GitHub Releases for the four managed crates.

**Architecture:** Remove the stale pending label from release PR #13, then enable Release Please's built-in force-tag option. Release Please remains responsible for updating Cargo manifests, `.release-please-manifest.json`, and changelogs in a reviewed 1.0.0 release PR. Keep the approved `RELEASE_PLEASE_TOKEN`.

**Tech Stack:** release-please 17.3.0, GitHub Actions, GitHub CLI, Git, JSON

## Global Constraints

- Do not create any `stateful-*-v0.1.1` tags or releases.
- All four managed crates must start their GitHub release history at 1.0.0.
- Keep the approved `RELEASE_PLEASE_TOKEN` repository secret.
- Do not hand-edit package versions or changelogs; review Release Please's PR.
- Do not publish crates to an external package registry.

---

### Task 1: Retire the blocked 0.1.1 candidate

**Files:** None.

**Interfaces:**
- Consumes: merged PR #13 with `autorelease: pending`.
- Produces: no pending merged release PR, allowing a new release PR.

- [ ] **Step 1: Remove the pending label**

```bash
gh pr edit 13 --remove-label "autorelease: pending"
```

- [ ] **Step 2: Verify 0.1.1 is skipped**

```bash
gh pr view 13 --json labels --jq '[.labels[].name]'
gh release list --limit 100
```

Expected: PR #13 has no `autorelease: pending` label and no
`stateful-*-v0.1.1` release exists.

### Task 2: Enable force-tag creation

**Files:**
- Modify: `release-please-config.json:3-4`

**Interfaces:**
- Consumes: the existing release-please manifest configuration.
- Produces: tag creation before release creation for future release PRs.

- [ ] **Step 1: Add the built-in option**

```json
{
  "$schema": "https://raw.githubusercontent.com/googleapis/release-please/main/schemas/config.json",
  "release-type": "rust",
  "force-tag-creation": true,
  "bump-minor-pre-major": true,
```

- [ ] **Step 2: Validate the JSON**

```bash
python3 -m json.tool release-please-config.json
```

Expected: exit code 0 and formatted JSON output.

- [ ] **Step 3: Commit and push the configuration**

```bash
git add release-please-config.json
git commit -m "fix: create release tags before releases"
git push
```

Expected: the configuration reaches `main` and triggers release-please.

### Task 3: Review the generated 1.0.0 release PR

**Files:** Release Please must update these on its branch:
- `.release-please-manifest.json`
- `crates/stateful-core/Cargo.toml`
- `crates/stateful-store/Cargo.toml`
- `crates/stateful-server/Cargo.toml`
- `crates/stateful-cli/Cargo.toml`
- Four managed crate changelogs

**Interfaces:**
- Consumes: current `main` and the force-tag configuration.
- Produces: a reviewed release PR with all managed versions at 1.0.0.

- [ ] **Step 1: Require the workflow to pass**

```bash
gh run watch "$(gh run list --workflow release-please.yml --limit 1 --json databaseId --jq '.[0].databaseId')" --exit-status
```

Expected: success and one open Release Please PR.

- [ ] **Step 2: Identify and inspect the PR**

```bash
RELEASE_PR="$(gh pr list --state open --json number,headRefName --jq '.[] | select(.headRefName | contains("release-please")) | .number')"
gh pr view "$RELEASE_PR" --json number,title,files,url
gh pr diff "$RELEASE_PR"
```

Expected: title and diff describe 1.0.0; all four Cargo versions and all four
manifest entries are 1.0.0; changelogs contain the intended current history.

- [ ] **Step 3: Obtain explicit approval before merge**

Present the PR URL and verified version files. Do not merge until the user
chooses the merge strategy.

### Task 4: Merge and verify 1.0.0

**Files:** None locally.

**Interfaces:**
- Consumes: the approved 1.0.0 release PR.
- Produces: four 1.0.0 tags and four GitHub Releases.

- [ ] **Step 1: Merge with the user-selected strategy**

Use `gh pr merge` with the explicitly selected merge strategy.

- [ ] **Step 2: Require the post-merge workflow to pass**

```bash
gh run watch "$(gh run list --workflow release-please.yml --limit 1 --json databaseId --jq '.[0].databaseId')" --exit-status
```

- [ ] **Step 3: Verify releases and tag targets**

```bash
RELEASE_SHA="$(gh pr view "$RELEASE_PR" --json mergeCommit --jq '.mergeCommit.oid')"
gh release view stateful-core-v1.0.0 --json tagName,url
gh release view stateful-store-v1.0.0 --json tagName,url
gh release view stateful-server-v1.0.0 --json tagName,url
gh release view stateful-cli-v1.0.0 --json tagName,url
gh api repos/zmfkzj/stateful_core/git/ref/tags/stateful-core-v1.0.0 --jq .object.sha
gh api repos/zmfkzj/stateful_core/git/ref/tags/stateful-store-v1.0.0 --jq .object.sha
gh api repos/zmfkzj/stateful_core/git/ref/tags/stateful-server-v1.0.0 --jq .object.sha
gh api repos/zmfkzj/stateful_core/git/ref/tags/stateful-cli-v1.0.0 --jq .object.sha
```

Expected: all four releases exist and every tag SHA equals `$RELEASE_SHA`.

If built-in tag creation fails, fetch the merge commit, create the four
`stateful-*-v1.0.0` tags locally at `$RELEASE_SHA`, push those tags over SSH,
and rerun the failed release-please workflow. Do not retarget the tags.
