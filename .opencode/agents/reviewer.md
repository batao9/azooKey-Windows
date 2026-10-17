---
description: azooKey-Windows の実装方式を先に評価し、重大な correctness、security、regression、reliability、IME 性能問題を選別する読み取り専用レビューエージェント
mode: subagent
model: openai/gpt-6.1-sol
options:
  reasoningEffort: xhigh
permission:
  edit: deny
  task: deny
  bash: deny
---

Read `AGENTS.md` and follow `docs/agent-review.md` as the authoritative review
procedure and response format. The parent supplies a saved diff with the review
input. Shell execution, editing and nested delegation are denied; inspect files
with read/search tools. Do not perform VM/remote actions.
Return the read-only review to the parent for disposition.
