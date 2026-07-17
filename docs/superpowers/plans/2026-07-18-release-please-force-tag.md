# Release Please Force-Tag Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make release-please create component tags before GitHub Releases so historical release PR merge SHAs remain valid release targets.

**Architecture:** Enable release-please's built-in `force-tag-creation` option in the existing manifest configuration. Keep the approved `RELEASE_PLEASE_TOKEN`; no workflow or application code changes are needed. The live GitHub workflow and resulting tags/releases are the observable verification.

**Tech Stack:** release-please 17.3.0, GitHub Actions, GitHub Releases API, JSON

## Global Constraints

- All 0.1.1 component tags must resolve to `3bcc980b00bc61fd09d580de7d4970bc056899e1`.
- Keep the approved `RELEASE_PLEASE_TOKEN` repository secret.
- Do not retarget 0.1.1 to current `main`.
- Do not add a custom tag-creation workflow step.
- Do not change package versions or release notes.

---

### Task 1: Enable force-tag creation and verify the release

**Files:**
- Modify: `release-please-config.json:3-4`

**Interfaces:**
- Consumes: release-please manifest configuration and merged release PR #13.
- Produces: four `stateful-*-v0.1.1` tags and matching GitHub Releases.

- [ ] **Step 1: Add the built-in configuration option**

Change the top of `release-please-config.json` to:

```json
{
  "$schema": "https://raw.githubusercontent.com/googleapis/release-please/main/schemas/config.json",
  "release-type": "rust",
  "force-tag-creation": true,
  "bump-minor-pre-major": true,
```

- [ ] **Step 2: Validate the JSON configuration**

Run:

```bash
python3 -m json.tool release-please-config.json
```

Expected: exit code 0 and formatted JSON output.

- [ ] **Step 3: Commit and push only the configuration**

```bash
git add release-please-config.json
git commit -m "fix: create release tags before releases"
git push
```

Expected: one commit containing only `release-please-config.json` reaches `main`.

- [ ] **Step 4: Require the release workflow to pass**

Run:

```bash
gh run list --workflow release-please.yml --limit 1 --json databaseId,status,conclusion,url
gh run watch "$(gh run list --workflow release-please.yml --limit 1 --json databaseId --jq '.[0].databaseId')" --exit-status
```

Expected: the newest run completes with `conclusion: success`.

- [ ] **Step 5: Verify releases and tag targets**

Run:

```bash
gh release view stateful-core-v0.1.1 --json tagName,url
gh release view stateful-store-v0.1.1 --json tagName,url
gh release view stateful-server-v0.1.1 --json tagName,url
gh release view stateful-cli-v0.1.1 --json tagName,url

gh api repos/zmfkzj/stateful_core/git/ref/tags/stateful-core-v0.1.1 --jq .object.sha
gh api repos/zmfkzj/stateful_core/git/ref/tags/stateful-store-v0.1.1 --jq .object.sha
gh api repos/zmfkzj/stateful_core/git/ref/tags/stateful-server-v0.1.1 --jq .object.sha
gh api repos/zmfkzj/stateful_core/git/ref/tags/stateful-cli-v0.1.1 --jq .object.sha
```

Expected: all four releases exist and every SHA equals `3bcc980b00bc61fd09d580de7d4970bc056899e1`.
