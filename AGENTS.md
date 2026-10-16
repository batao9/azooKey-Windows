# azooKey-Windows agent guidance

These repository rules apply to Codex, OpenCode, Pi, and other coding agents. Keep product procedures here; user-wide defaults and roles belong in the user's agent configuration, not this repository. See [agent operations](docs/agent-operations.md).

## Routing

- Use `.agents/skills/azookey-dev-flow` for implementation and verification, `.agents/skills/azookey-pr` for an explicitly requested ordinary PR flow, and `.agents/skills/azookey-release` for release work.
- The authoritative high-signal reviewer procedure is [docs/agent-review.md](docs/agent-review.md); native adapters are in `.codex/agents/` and `.opencode/agents/`.
- Version, release-channel, installer trust, and issue-close rules are in [docs/versioning-release.md](docs/versioning-release.md). VM operations use `python3 scripts/vm.py`; profiles and local values are documented by that interface and `.local/vm.json`.

## Repository invariants

- Do not develop or commit directly on `master`. Use `feature/<slug>` or `fix/<issue>-<slug>` as appropriate; preserve unrelated worktree changes.
- Treat key handling, composition updates, candidate work, text-service callbacks, and typing-triggered IPC/UI as latency-sensitive. Avoid blocking I/O, waits, unnecessary locks/copies/allocations, and unbounded work on those paths.
- For IME regressions, reconstruct the ordered key/edit/focus history, including deletions, cursor movement, clause adjustment, candidate actions, and commit/cancel. Compare client predictions with server canonical input/count and candidate state; add a test for the exact sequence before generalizing.
- Run checks that cover changed behavior. GitHub CI is the formal build result; Windows-specific behavior should be checked on the VM when feasible.
- VM commands go through `scripts/vm.py` and [the VM operations guide](docs/vm-operations.md), not ad-hoc VBox/SSH commands. After verification, leave the installed current disk intact and normally shut down. Do not restore, clean, or refresh a baseline unless requested; report final VM state and installed build identity.
- Skill discovery is not authorization to commit, push, publish, or close issues. An explicit `azookey-pr` invocation or direct request authorizes the ordinary PR workflow; release publication still requires prior approval of the release contents and issue-close list.
- Never use PR/commit closing keywords for issues. Close an included issue as `completed` only after the containing release, tag, CI, and assets are verified; comment with the release URL.
- Delegated work must use at least `gpt-6-luna` with `xhigh` reasoning. The repository reviewer may use `gpt-6.1-sol` with `xhigh`.
