# PR workflow

Use only after implementation and relevant local verification are ready. An explicit skill invocation/direct PR request authorizes the ordinary PR workflow without a second confirmation; skill auto-discovery does not. Do not merge, release, force-push, or close issues.

## Audit and preflight

1. Read `AGENTS.md` and session instructions. Inspect branch/status, diff/stat/check, remote branch and matching PR, commits, current checks, reviews, and inline threads. Resume an existing PR from its first incomplete verified step; do not duplicate it.
2. Confirm the branch is not `master`; identify unrelated changes and stage only intended files. Ask only if the target change set is ambiguous or unrelated work blocks safe staging.
3. Run any final focused formatting/tests needed because files changed after previous verification. `git diff --check` must pass.

## Commit and create/resume PR

Use a concise English Conventional Commit subject appropriate to the area, such as `fix(client): ...`, `feat(ui): ...`, or `test(vm): ...`. Verify staged files and branch/log before committing.

Follow `.github/pull_request_template.md`. Use an English title and Japanese body; complete Summary, Background, Changes, Verification, and Checklist. Include `Issue: #<number>` or `Issue: N/A`; never include closing keywords (`Closes`, `Fixes`, `Resolves`, etc.) in the PR body or commit. Do not use a GitHub Development link that would auto-close an issue unless auto-close is disabled. Mention actual local/VM checks. Leave the CI checkbox unchecked until CI passes, then add its run URL if practical.

Push the current branch and create a review-ready PR to `master` when no matching PR exists. Do not ask for another confirmation for these ordinary PR steps after explicit invocation. Opening a PR may trigger review; request review with `@codex review` when appropriate and missing.

## CI and review follow-up

- Check `gh pr view <number> --json statusCheckRollup,mergeStateStatus,reviewDecision,reviews,comments,url`, inline comments/threads, and `gh pr checks`. Confirm serious inline findings apply to the current PR head SHA.
- If a check fails, read its log, fix the root cause, run focused checks, commit and push the correction, then continue monitoring.
- Treat blocking/high/major, `[must]`, correctness, regression, crash, data-loss, CI, and security findings as serious. Fix substantiated serious issues and add focused regression tests when useful. For installer/updater/security/privilege-boundary changes, request another read-only review when available. Do not chase nits or unsupported suggestions; explain a false positive with code evidence.
- Before follow-up commits/pushes run `git diff --check` and relevant focused checks. Update the PR body with CI/review/VM facts when they change.

Report PR URL, branch/commits, current CI/review result, fixes, related issue still open pending release, and remaining risks. Never claim success without checking live GitHub state.
