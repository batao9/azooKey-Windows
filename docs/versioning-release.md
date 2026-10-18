# Versioning and release invariants

## Version and build identity

- `app-version.json` is the source of truth: `version` is the formal release version and `installerGeneration` is the installer layout/migration contract generation. Do not write validation build numbers there.
- For a formal metadata sync, run `node scripts/sync-version.mjs --sync-release`; do not hand-edit its generated mirrors (`frontend/package.json`, `frontend/package-lock.json`, `frontend/src-tauri/tauri.conf.json`, or `installer/AppVersion.iss`).
- Branch, PR, `master`, VM, and untagged local builds use the `validation` channel. The build derives its version from the latest reachable release tag: prerelease tags produce `<base>.dev.<build-number>.g<short-sha>`, stable tags produce the next patch with `-dev.<build-number>.g<short-sha>`. With no reachable release tag, use `app-version.json` as the base.
- Only tag CI may select `release` with `AZOOKEY_BUILD_CHANNEL=release`, and `HEAD` must have the exact `v<app-version>` tag. A local build defaults to `validation` even on a tag. Publish only the `azookey-setup` artifact from tag CI, never a branch/PR/master artifact.
- Build output goes under `target/azookey-build/`; embedded identity is `build/build-info.json`. Verify `version`, `channel`, `revision`, and `installerGeneration` when testing or publishing an installer.
- Validation and stable builds intentionally share the Inno `AppId` for replacement testing. The old AppData install version/hash allowlist is a privileged migration trust policy; never relax it based on the validation channel or suffix.
- Installer/version changes must cover stable-to-validation, validation-to-validation, and validation-to-next-stable update paths.

## PR and issue lifecycle

- Feature/fix PR bodies include `Issue: #<number>` when related, but neither PR bodies nor commit messages use `Closes`, `Fixes`, `Resolves`, or equivalent closing keywords. Do not create an auto-closing Development link unless repository auto-close is disabled.
- Keep issues open when a PR merges. Only close a completed issue as `completed` after the containing GitHub Release is published and its tag, CI, and distribution assets are verified. Build the close list from PR issue references since the previous release, exclude partial work or remaining acceptance criteria, and comment with the release URL.
- Release work first presents the proposed release notes, exact target commit set, and issue-close list. Do not push, create/merge PRs, tag, publish, comment, or close issues until the user approves the release contents and close list. Reconfirm if the approved master commit advances.
