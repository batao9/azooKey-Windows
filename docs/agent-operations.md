# Agent operations

## Scope and discovery

`AGENTS.md` is the repository authority for product-specific constraints and applies to Codex, OpenCode, Pi, and other agents that honor agent instructions. `.agents/skills/` contains the portable project skills; Codex, OpenCode, and Pi all discover this project location. Keep one authoritative project copy there rather than duplicating skills under harness-specific directories.

User-wide defaults, personal roles, tool permissions, and extra skills belong in the user's agent configuration. Repository guidance must not depend on a personal home directory, plugin cache, credentials, session store, or private permission file. Project adapters under `.codex/agents/` and `.opencode/agents/` are limited to the azooKey reviewer; `.codex/config.toml` and machine-local OpenCode files remain ignored.

Skills are automatically discoverable, but discovery alone never authorizes external or irreversible actions. The PR skill's explicit invocation authorizes its ordinary commit/push/PR/CI flow. A release always begins with a proposed summary and issue-close list and proceeds to remote mutations only after the user approves both.

## Delegation

Use `gpt-6-luna` with `xhigh` reasoning as the minimum delegated model/reasoning level. The project reviewer may use `gpt-6.1-sol`, also at `xhigh`. Scope each request to one outcome, pass the relevant paths and evidence, state edit/side-effect limits, and give enough context for the selected harness.

Codex and OpenCode support continuing a delegated session when the harness returns a real agent/session ID; retain that ID and use it for a related follow-up rather than starting over. Pi's shipped subagent extension launches each worker as a new `pi --mode json -p --no-session` process. It has no prior worker session to resume: every invocation and retry must include the complete task, constraints, needed context, and expected result. Do not assume identical tool names, permissions, or resume semantics across harnesses.

Read-only review is both an instruction and a harness capability. Use actual read-only tool/sandbox permissions when dispatching reviewers. Codex custom-agent configuration cannot override the caller's sandbox; OpenCode's reviewer denies `edit` and `task`; Pi's subagent tool list/extension must enforce the permissions available in that invocation. A prompt by itself does not restrict tools.
The OpenCode and Pi read-only adapters also omit shell execution. Include a saved diff in their review input; Git command-prefix globs do not enforce read-only access because options such as `--output` write files.

## Windows VM verification

Use the repository VM interface, not ad-hoc VirtualBox or SSH commands. The command shape is:

```text
python3 scripts/vm.py --profile build|manual status|start|shutdown|reset|reboot|doctor|prepare|build|test|stage|exec|upload|download|screenshot|restore|snapshot|handoff
```

Machine-specific profile values live in ignored `.local/vm.json`; the public shape is `scripts/vm-config.example.json`. `build <branch>` and `test <branch> [cargo-filter|skip] [swift-filter|all|skip]` wrap the repository helpers with a baseline reset before and after work. `stage <installer>` also starts clean, records the installed identity, then restores the baseline. For native UI verification use `stage <installer> --leave-running`, run probes, recover evidence and `handoff` to record identity and reset. `exec --script <file>` runs a normal SSH PowerShell script; add `--interactive` only for work that requires the logged-on UI. Standalone exec/doctor runs reset before/after; commands within an open session continue that installed state. `reset` immediately powers off/discards saved memory and restores the configured baseline without SSH. `snapshot <new-name>` remains explicit maintenance. See the [VM operations guide](vm-operations.md) for details.

Treat both build and manual VMs as disposable. Recover artifacts/evidence to the host before resetting. On SSH failure, timeout or stalled helper output, record failure and reset promptly; do not perform repeated wake/SSH/shutdown investigations. Automatic recovery never changes a failed test into success or replays an install/build. Do not interrupt another live owner. Report the tested identity separately from the final restored baseline/power state. Capturing/promoting a new baseline is still explicit maintenance, using `shutdown --keep-current` only for that purpose.

Explicit baseline/OS-update maintenance is the exception to disposable recovery. Start it with `start --keep-current` or `prepare`; keep the current disk even when a command fails. Known completed failures may continue as maintenance; uncertain operations retain their evidence and block subsequent ordinary dispatch until explicitly resolved. Preserving shutdown/reboot issues one normal Windows shutdown and observes power-off with its own bound, without force/reset or replay on lost SSH acknowledgment. A maintenance `handoff` preserves the disk and stops normally. Snapshot capture reserves ownership before mutation and verifies a new powered-off identity before release. VM display names, UUIDs, local snapshot inventory and promotion receipts stay under ignored `.local/`; public profiles describe roles, not a particular machine.

## Local workspace layout

`.local/` はGit管理しない作業領域であり、実行手順の正本ではありません。

- `vm.json`: VM controllerの機械固有設定。既定パスを維持し、認証情報を共有しない。
- `cache/`: 再利用する依存source・データ・ツールのキャッシュ。
- `artifacts/`、`release/<tag>/`: 検証成果物と正式リリースの検証記録。公開スクリプト・release workflowの既定パスを維持する。
- `logs/`: VM controller等が出力する実行ログ。操作中・未解決の証拠は削除しない。
- `reports/<topic>/`: 調査結果・測定値・必要な証拠。課題番号は `reports/issues/<number>/`、関連する測定生データは各topic配下にまとめる。
- `private/`: 設定・認証関連のバックアップ。内容をログへ出したり、Gitへ追加したりしない。
- `tmp/<task>/`: 単発probe・PSスクリプト・一時checkout。作業終了時に結果をreportへ回収して削除する。`.local/` 直下へ増殖させない。

共有する仕様・列定義は `docs/`、配布データ・NOTICEは `data/`、再利用する自動化は `scripts/` に置く。過去のベンチマークや実行メモを `data/` へ置かない。整理時は稼働中の作業・Git worktree・認証情報・controllerの状態/lockを保護し、実行時に参照される既定パスを移さない。

## References

- [Repository review procedure](agent-review.md)
- [Version and release invariants](versioning-release.md)
- [Codex `AGENTS.md` guide](https://developers.openai.com/codex/agent-configuration/agents-md.md)
- [Codex subagents guide](https://developers.openai.com/codex/agent-configuration/subagents.md)
- [Codex skills guide](https://developers.openai.com/codex/build-skills.md)
- [OpenCode agents](https://opencode.ai/docs/agents/)
- [OpenCode skills](https://opencode.ai/docs/skills/)
- [OpenCode rules](https://opencode.ai/docs/rules/)
- Pi's versioned practical references: `docs/skills.md` and `examples/extensions/subagent/index.ts` in the installed Pi source. The latter shows fresh `--no-session` subagent processes and project-agent discovery.
