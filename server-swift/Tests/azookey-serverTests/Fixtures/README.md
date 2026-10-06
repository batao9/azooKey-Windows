# Jev explicit-conversion evaluation fixture

`jev-evaluation.json` is a fixed, manually authored, exploratory corpus for
comparing normal KanaKanjiConverter (KKC) N-best output with a Jev-selected
candidate. It is intentionally small and must not be represented as a general
Japanese conversion benchmark.

Each case has stable fields:

| Field | Meaning |
| --- | --- |
| `id` | Stable machine-readable case identifier. Do not renumber or reuse it. |
| `category` | `contextual_homophone`, `common_sentence`, `external_context`, or `public_proper_name`. |
| `reading` | Complete hiragana reading supplied to KKC. |
| `external_context` | Optional left context supplied to the scorer as a distinct field. It is not part of `reading`. |
| `acceptable_outputs` | One or more exact full-conversion surfaces counted as correct. |

The evaluator should generate KKC candidates once for each `reading`, then use
the same deduplicated N-best prefix for both the baseline and Jev runs. For
each N (initially 8, 16, and 32), report at least:

- KKC Top-1 accuracy;
- KKC oracle coverage: any `acceptable_outputs` member in the N-best list;
- Jev Top-1 accuracy among that same N-best list;
- candidate-generation, Jev request, and end-to-end latency;
- counts grouped by `category`, plus the total excluding `public_proper_name`.

This fixture is fixed before collecting output. Do not alter a case or append
an acceptable surface after seeing a KKC or Jev result; make a separately
versioned corpus revision when a correction is necessary.

Exact matching deliberately uses an allow-list because ordinary Japanese
orthography has legitimate variants, such as `良い` / `よい` and `行う` /
`行なう`. Add a variant only when it is equally acceptable for the case; do
not use normalization or a fuzzy metric that could make a different word look
correct.

The 40 cases are authored examples, not sampled user text. They cover common
sentences and deliberately disambiguated homophones, but cannot estimate
production accuracy, domain coverage, long-text behavior, or personalization
effects. The four proper-name cases use only well-known public place names;
there are no person, organization, product, or private names. Report that
category separately and do not generalize its result to proper-name conversion.

The fixture contains no secrets or user-originated text. Keep evaluation runs
opt-in: a real Jev call transmits the selected candidate strings and
`external_context` to TypeSafe AI.
