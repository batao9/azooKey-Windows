# Composition debugging

Treat final visible text as an outcome, not a complete reproduction. IME state may depend on edit history even when sessions end with the same text.

## Reconstruct the event history

Record in order:

- Every raw key and romanization variant.
- Every Backspace/Delete and cursor position; whether automatic clause splitting began with Left or Right.
- Shift+Left/Right, candidate selection, commit/cancel, click, focus/application changes, and server restart/recovery.
- Client raw input, server canonical raw input or input count, and visible candidates.

Correlate client `process_key` records (including `config_snapshot` and `wparam`) with server Append, Remove, MoveCursor, and AdjustClauseBoundary events. Find the earliest client/server divergence, not only the later operation that exposes it. A compact trace helps:

| Step | Event | Client raw | Server count | Visible state | Result |
| --- | --- | --- | --- | --- | --- |
| 1 | key or edit | value | value | text/candidates | applied/rejected |

## Mapped deletion and regression tests

One Backspace need not remove one raw character: romaji conversion can rewrite or consume multiple composing elements. Exercise relevant mappings such as `n`, `nn`, `tya`/`cha`, sokuon, and multi-input mappings.

- Treat server composition and canonical raw identity as authoritative for mutating IPC.
- Do not assume a local `raw_input.pop()` matches server removal.
- Retry a non-idempotent edit only when candidate composition and raw identity both prove it was not applied.
- If an older server omits canonical raw input, recover explicitly; an absent optional field is not an empty value.

Add the exact event sequence first, feeding keys one at a time through the real romaji table. Assert input counts, canonical raw input, composition, clauses, and candidates where relevant; then add only the general invariant established by the failure. Cover corrected/uncorrected input, Left/Right-started splits, and first/middle/last clause boundaries when they share the changed path. Keep production per-key logging disabled; detailed traces belong in a diagnostic configuration.

## Issue #76 reproduction

1. Enter `aruteidonageibunsyoudemofukusuubnun`.
2. Press Backspace three times.
3. Enter `bunsetunibunkatusareru`.
4. Split automatically with Left, move to the first clause, and adjust it.

Server input-element counts changed `35 -> 34 -> 32 -> 31` during deletion. A one-character client pop diverged from server canonical input; later clause adjustment was rejected as `raw_input_mismatch`. Preserve the entire correction history in regression tests and VM verification; the final visible sentence alone misses the cause.
