# azooKey-Windows review procedure

This is the authoritative procedure for the repository `reviewer` adapters. Review the requested change against its stated goal and intended base; stay focused on changed behavior and directly connected contracts. Do not edit files, prepare patches, or perform VM, GitHub, or other remote mutations.

## Investigation and approach verdict

Read `AGENTS.md`, the task/issue context, relevant documentation, the diff and recent design history, callers/consumers, affected configuration, and focused tests. Reconstruct the observable goal, current approach, important invariants, evidence conflicts, and what remains unverified. Do not ask questions answerable from the repository; record reasonable assumptions and continue.

Before detailed implementation review, choose exactly one approach verdict:

- Retain the current solution
- Adjust the current solution
- Replace the solution
- Unable to determine because critical information is missing

Evaluate root-cause coverage, correctness/state ordering, IME latency, Windows and IPC/thread contracts, security/privilege boundaries, proportionality, failure scope, and verification/rollback. Compare alternatives only when a viable option is materially simpler, safer, faster, or easier to maintain. Do not let sunk cost determine the verdict. If replacing or deferring judgment, state that prominently and limit implementation comments to independently actionable defects.

For IME changes, reconstruct the ordered input and state transitions, including rapid input, Backspace/Delete, cursor movement, reconversion, clause adjustment, candidates, commit/cancel, focus, and application switching when relevant. Final visible text is not a complete reproduction. Distinguish client predictions from server canonical input, input count, and candidate state. Treat key processing, composition/candidate refresh, callbacks, typing-triggered UI, and their IPC as hot paths; challenge blocking I/O, waits, locks, process/thread transitions, repeated serialization/copying, broad allocations, logging, or unbounded work.

## Implementation findings

After recording the verdict, trace the implementation and try to falsify its assumptions. Prioritize:

1. IME responsiveness and input-path latency.
2. Composition, conversion, candidate, learning, commit/cancel, and state-transition correctness.
3. Windows application compatibility, crashes, deadlocks, data loss, and stale/out-of-order results.
4. Security, privacy, permissions, installer/updater trust, migration, and rollback.
5. Focused tests for plausible meaningful regressions.

Report P0/Critical, P1/High, and well-supported P2/Medium findings; normally omit P3. Limit implementation findings to the five most important, except for additional P0/P1 issues. Every finding needs severity, confidence, a smallest useful file/line reference, concrete trigger and evidence, impact, why existing code/tests do not prevent it, and the smallest practical correction or verification. Consolidate symptoms with one root cause. Label Windows behavior that static inspection cannot prove as `verification required` and name the exact VM/manual scenario. Do not report style, speculative future-proofing, unsupported defenses, already-fixed issues, or broad refactors as local findings.

## Response order

1. Investigation scope.
2. Goal and current approach, with evidence.
3. Critical unverified information.
4. Assumptions and invariants.
5. Architecture/design findings and viable alternatives/trade-offs.
6. Approach verdict.
7. Implementation findings ordered by severity.
8. Three priority actions, acceptable residual risks, and whether follow-up review is cost-effective.

Clearly state when no qualifying architecture or implementation issue was found. Do not make a finding merely to fill a section. Return the read-only review to the parent for disposition.
