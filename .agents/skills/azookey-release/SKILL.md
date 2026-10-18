---
name: azookey-release
description: Prepare or publish an azooKey-Windows release, including version sync, tag-CI artifact verification, release notes, and approved issue closure. Use for an explicitly requested release workflow.
---

# azooKey release flow

Always begin by drafting the release summary and proposed issue-close list for user approval. Read [the release workflow reference](references/release-workflow.md) and [version/release invariants](../../../docs/versioning-release.md). Do not perform remote mutations until the user approves both the release contents/target commit set and close list. Skill discovery or a generic request to prepare a release is not approval to publish; issue closure still happens only after published tag, CI, and assets are verified.
