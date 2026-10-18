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
python3 scripts/vm.py --profile build|manual status|start|shutdown|doctor|prepare|build|test|stage|exec|upload|download|screenshot|restore|snapshot|handoff
```

Machine-specific profile values live in ignored `.local/vm.json`; the public shape is `scripts/vm-config.example.json`. `build <branch>` and `test <branch> [cargo-filter|skip] [swift-filter|all|skip]` wrap the repository build/test helpers. `stage <installer>` updates in place by default and shuts down. For native UI verification use `stage <installer> --leave-running`, then `handoff` to shut down while recording the installed identity. `exec --script <file>` runs a normal SSH PowerShell script; add `--interactive` only for work that requires the logged-on UI. `restore <snapshot>` and `snapshot <new-name>` are explicit maintenance actions. See the [VM operations guide](vm-operations.md) for config, exact identity checks, logging, recovery, and baseline maintenance.

After verification, keep the installed current disk intact and normally leave the VM shut down. Never restore or clean automatically after validation. Restore, leave-running, or baseline refresh only when requested; baseline refresh is explicit maintenance, not a per-run snapshot cycle. Report the final power/disk state and installed version, channel, revision, and installer generation when available. See the VM operations guide for the finalized command details.

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
