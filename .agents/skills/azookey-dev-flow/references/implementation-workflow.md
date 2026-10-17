# Implementation and verification workflow

Use this reference for a requested issue or real-use report. Keep work limited to the reported behavior and preserve unrelated worktree changes.

## Understand and implement

1. Read `AGENTS.md`, applicable task docs, and `git status --short --branch`. Identify the intended base, changed state, and any user changes before acting. For an issue URL/number, inspect the issue and comments when GitHub access is available.
2. Establish observed versus intended behavior, application/environment, exact reproduction, and whether correctness, responsiveness, or both are affected. For composition history, follow `composition-debugging.md`.
3. Use a feature branch for new behavior/performance work and a fix branch for regressions; do not work directly on `master`. Do not switch branches if that would strand or overwrite uncommitted work.
4. Reproduce or inspect current behavior when useful. For real-time work, trace the input event to visible state and note composition, candidate ordering, commit/cancel, rapid input, cancellation, and stale-result behavior as relevant.
5. Add focused regression coverage close to the changed behavior. Prefer exact bug sequences, client stateful tests, server candidate tests, frontend checks, and stale/cancellation tests as appropriate. Tests should establish meaningful observable behavior and normally fail without the fix. Avoid coverage-only tests and fragile wall-clock thresholds.
6. Implement the smallest coherent change following nearby architecture. Before adding hot-path work, check frequency, thread, blocking/IPC/lock behavior, allocation/copying, cancellation, and result ordering. Avoid unrelated refactors, broad defensive state, excessive logging, and abstractions without a concrete benefit.

## Checks and review

Run focused tests first, then checks appropriate to changed areas. Run `git diff --check` and `node scripts/sync-version.mjs --check-release`; use relevant formatters/checks (for example, `cargo fmt --all --check`). If a required tool is missing, record the exact command and error; never describe an unrun check as passing. Inspect performance-sensitive diffs for new waits, locks, per-key logging, copies, unbounded work, and unnecessary per-candidate computation.

When an independent review is available and useful, ask the repository `reviewer` to follow `docs/agent-review.md`. Give the goal, changed paths, relevant execution paths, tests/evidence, known verification limits, and read-only constraint. Supply a saved diff including intended new files to shell-free reviewers. Review the approach verdict before implementation findings; assess each finding against the actual code/scenario before making changes. Resolve confirmed P0/P1 and realistic meaningful P2 issues, then rerun focused checks. For a resumed Codex/OpenCode subagent use its actual session ID; Pi workers start fresh, so provide a self-contained request each time.

## Windows verification

Use `python3 scripts/vm.py` for VM operations; do not run ad-hoc VBox/SSH commands. Route build/test to the `build` profile and installer/UI checks to `manual`. For example:

```text
python3 scripts/vm.py --profile build build <branch>
python3 scripts/vm.py --profile build test <branch> [cargo-filter|skip] [swift-filter|all|skip]
python3 scripts/vm.py --profile manual stage <installer>
```

Use `stage <installer> --leave-running`, then `handoff`, when native logged-on UI verification is required. Use `exec --script <file>` for normal guest PowerShell and `--interactive` only for work requiring the logged-on desktop. `restore` and `snapshot` are explicit maintenance operations; never clean/restore the VM automatically after validation. The default is to preserve the installed current disk and shut down. If the user requests leave-running or restore, do that explicitly and report the exact final VM state and installed identity.

IME/TSF/COM/IMM32/Win32, candidate/UI, installer/updater, or Windows filesystem/process changes warrant VM checks when feasible. Verify the reported sequence and a nearby normal path; exercise rapid input, focus loss, cancellation, or application switching when affected. Inspect screenshots/logs and build identity rather than only command completion. Do not claim a performance improvement without evidence.

## Completion

Report branch, changed files, behavior, test/check commands and results, VM evidence or blocker, version/channel/revision/installer generation for any built installer, performance evidence or reasoning, review disposition, and unverified behavior. Do not commit, push, or create a PR as part of development-flow skill use; report whether ready for `azookey-pr`.
