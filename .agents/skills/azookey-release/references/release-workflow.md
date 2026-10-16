# Release workflow

The approval gate is mandatory. In phase 1, present concise release notes, proposed issue-close list, and exact content commit SHA. Do not push branches/tags, create or merge PRs, publish/update releases, comment, or close issues until the user explicitly approves the contents and close list. If the approved `origin/master` content changes, regenerate the summary/list and obtain approval again.

## Phase 1: prepare the proposal

1. Read `AGENTS.md`, `docs/versioning-release.md`, and current status/remotes. Inspect any existing release branch, bump, PR, tag, release, workflows, and assets; resume from verified state. When available, use `gh release list --repo batao9/azooKey-Windows --limit 10` and `git tag --sort=-creatordate` to identify the prior published release. Read recent bodies with `gh release view <previous-tag> --repo batao9/azooKey-Windows --json name,body,publishedAt`.
2. Bind the proposal to `git rev-parse origin/master` as `<approved-content-sha>`. Review the change set with:

   ```bash
   git log --reverse --format='%h%x09%s%n%b' <previous-tag>..origin/master
   git diff --stat <previous-tag>..origin/master
   git log --oneline --merges <previous-tag>..origin/master
   ```

   Inspect relevant merged PRs with `gh pr view <number> --json title,body,mergedAt,headRefName` where useful.
3. Extract `Issue: #<number>` references from feature/fix PRs since the prior release. Verify each with `gh issue view <number> --json number,title,state,url`. Include only open issues whose completed change is in the release and has no remaining acceptance criteria. Exclude partial/tracking work or explain why it remains open.
4. Present a brief draft with `## Summary`, `## User-facing changes`, `## Stability and internal improvements`, `## Assets`, and `## Notes`, plus a separate `## Issues to close after publication` list. State explicitly when the list is empty. Record the exact content SHA and accurately state which bump/branch/PR/tag/release/comment/closure actions already exist. Ask for approval of the notes, commit set, and issue list. Do no phase-2 remote mutation until approved.

## Phase 2: execute after approval

### Release branch, version bump, and PR

1. Ensure the worktree is clean or understand all existing changes. Update and verify `master` still equals `<approved-content-sha>`; if not, return to phase 1 and get approval again. Create `release/<tag>` at that SHA.
2. Set `app-version.json` to the requested version (without `v`) and run:

   ```bash
   node scripts/sync-version.mjs --sync-release
   git diff --check
   node scripts/sync-version.mjs --check-release
   ```

   Verify `app-version.json`, `frontend/package.json`, `frontend/package-lock.json`, `frontend/src-tauri/tauri.conf.json`, and `installer/AppVersion.iss` agree. Run available focused checks, including TypeScript (`node node_modules/typescript/bin/tsc` from `frontend/`) when dependencies are installed. Commit `chore(release): prepare <tag>`.
3. Push the release branch and create a PR to `master` with the version-sync and local check details. Treat this branch/PR artifact as `validation`; it is never a release asset. Wait for PR CI. Merge only after checks pass and immediately recheck that `origin/master` still matches the approved content SHA. Record the PR's exact `mergeCommit` as `<merge-sha>` and verify `<previous-tag>..<merge-sha>` contains only approved changes plus the expected version-sync commit. If other feature/fix content entered, return to phase 1.

### Verify master CI and tag CI

1. Confirm `<merge-sha>` is contained in `origin/master`. Select and inspect the master push workflow for that exact SHA, for example:

   ```bash
   gh run list --repo batao9/azooKey-Windows --workflow Rust --commit <merge-sha> --branch master --event push --limit 10 --json databaseId,headSha,headBranch,status,conclusion,url
   gh run watch <run-id> --repo batao9/azooKey-Windows
   ```

   Assert `headSha == <merge-sha>` before watching/downloading. Master CI must produce a `validation` identity even though the version file has been bumped; do not stage that artifact for publication.
2. Only after successful master CI, create and push `<tag>` at `<merge-sha>`. Select the tag push run and verify its exact SHA and branch:

   ```bash
   gh run list --repo batao9/azooKey-Windows --workflow Rust --commit <merge-sha> --branch <tag> --event push --limit 10 --json databaseId,headSha,headBranch,status,conclusion,url
   gh run watch <tag-run-id> --repo batao9/azooKey-Windows
   ```

   Require the build and tag-only installer smoke job to succeed. Download the `azookey-setup` artifact to `.local/release/<tag>/`:

   ```bash
   gh run download <tag-run-id> --repo batao9/azooKey-Windows --name azookey-setup --dir .local/release/<tag>
   ```

3. Verify `.local/release/<tag>/build-info.json`: `channel` is `release`; `version` and `releaseVersion` equal `<tag>` without `v`; `revision` is `<merge-sha>`; `installerGeneration` matches the approved version metadata. From the artifact directory, run `sha256sum -c SHA256SUMS.txt` and ensure the manifest names exactly `azookey-setup.exe`.

### Publish, then close approved issues

1. Write `.local/release/<tag>/RELEASE_NOTES.md` from the approved proposal, changing only for verified execution facts. Create the GitHub Release from the verified tag and attach only the tag-CI `azookey-setup.exe` and `SHA256SUMS.txt`:

   ```bash
   gh release create <tag> .local/release/<tag>/azookey-setup.exe .local/release/<tag>/SHA256SUMS.txt --repo batao9/azooKey-Windows --target <merge-sha> --title "azooKey Windows <tag>" --notes-file .local/release/<tag>/RELEASE_NOTES.md --latest
   gh release view <tag> --repo batao9/azooKey-Windows --json tagName,name,isDraft,isPrerelease,publishedAt,url,assets,targetCommitish
   git ls-remote --tags origin '<tag>'
   ```

   Verify it is published, targets the intended commit, and has the required assets. Check the latest-release designation when appropriate.
2. Run this step only after the release, tag, required CI, and assets are verified. Use only the issue list approved in phase 1. Reconfirm each issue remains open and the completed change is included in the release target; close as completed with the verified URL, then verify state:

   ```bash
   gh issue close <number> --repo batao9/azooKey-Windows --reason completed --comment "<tag> でリリースしました: <release-url>"
   gh issue view <number> --repo batao9/azooKey-Windows --json number,state,closedAt,url
   ```

   Leave excluded/partial issues open. Do not add newly inferred issues at this stage.

## Release body shape

```markdown
## Summary
- batao9/azooKey-Windows の fork release `<tag>`
- こちらは fkunn1326/azooKey-Windows の fork です

## User-facing changes
- ...

## Stability and internal improvements
- ...
- アプリ内バージョンとインストーラー版数を `<version>` に更新

## Assets
- azookey-setup.exe
- SHA256SUMS.txt

## Notes
- このリリースは fork 版です
- 配布 installer は Inno Setup 生成の `azookey-setup.exe` です
- 不具合報告はこのリポジトリの Issues へお願いします
```

Keep change sections limited to differences from the prior release. In phase 1, omit or mark the version update pending until merged. Follow [`docs/versioning-release.md`](../../../../docs/versioning-release.md) for build-channel, version identity, installer trust, and issue lifecycle invariants.

## Completion checks

Verify the approved version in all mirrors; release PR merge and successful validation CI; exact successful tag CI and installer smoke run; tag points to the intended merge; release build identity and checksums; published release URL/assets; and each approved issue closure/comment. Report URL, tag/commit, CI run results, asset names/SHA256, closed issues, and excluded issues intentionally left open. Do not claim an issue closed or artifact verified without checking current state.
