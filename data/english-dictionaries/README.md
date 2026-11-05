# Bundled English dictionaries

Two offline-generated inputs are kept separate:

- `katakana.tsv`: 14,195 reading/output pairs, selected from official UniDic
  2.1.2 and an independently authored technical supplement.
- `spelling.tsv`: 61,050 ASCII spellings, selected from ESDB US size50 and the
  named CSpell software-terms lists. ASCII letters only, length4–20; existing
  short technical seeds remain in `MixedInputConversion.swift`.

The exact revisions, SHA256s, counts and ESDB extraction command are in
`sources.json`. Redistribution notices are in `NOTICE.txt`, copied into the
installed immutable dictionary as `English-NOTICE.txt`.

The checksummed TSV inputs use canonical UTF-8/LF bytes. Local Git attributes
retain LF even with Windows `core.autocrlf=true`, and the importer explicitly
writes LF on every platform. Checksums validate those exact bytes.

## Regeneration

Use the exact ESDB revision from `sources.json` and run its `make` first. Obtain
the official `unidic-mecab-2.1.2_src.zip` with the recorded hash, then run:

```sh
python3 scripts/import-english-dictionaries.py --esdb <esdb-checkout> --unidic <official-source.zip>
node scripts/generate-english-dictionary.mjs
```

The importer writes data only. The Node generator creates the immutable packed
BFS recognition trie as
`server-swift/Sources/azookey-server/Resources/english-vocabulary.bin`.
That tracked SwiftPM resource supports direct `swift build` as well as the full
build pipeline. `EnglishVocabulary.swift` contains only loading/validation code,
not Base64 or a generated vocabulary literal. `--check` verifies that the
resource exactly matches the canonical TSV without changing files:

```sh
node scripts/generate-english-dictionary.mjs --check
```

The AZEV v1 binary has a version/count header and fixed8-byte trie nodes. The
loader checks length, child ranges, BFS topology, sorted labels and terminal
counts once, caching both success and failure. New words are checked against the
actual romaji table only when encountered, with a bounded per-table cache.
There is no resource reading, decoding or validation per key.

The dictionary generator copies the original dictionary to
`target/azookey-english-dictionary/Dictionary`, then merges additions into the
normal LOUDS index. It also independently generates `Variants/base`,
`Variants/kana` and `Variants/spelling` from the original submodule, not from the
merged dictionary. Each root contains its own reverse index; variants omit the
unused `p` source data. It never rewrites the dictionary submodule. It preserves
every existing reading, surface, grammatical ID and float32 score byte, checks
the complete row multiset after serialization, and rejects unmappable new
readings. The fixed converter uses 2048-slot shards.

Kana entries are ordinary low-priority noun entries (score -12), not high-priority
user registrations. The engine's existing input/surface count resolution is
used for partial commits. User entries and typo-correction entries retain their
separate dynamic dictionary path and priority.

Clear shortened aliases receive score -16 instead of -12. The offline generator
marks only UniDic readings of2–4 kana that are a prefix of another reading for
the same English spelling, omit at least3 kana, and are at most60% of that full
reading. For example インスト→instrumental is lower priority than
インストルメンタル→instrumental. Long-vowel variants such as サーバ/サーバー
and project-authored overrides are not demoted. The candidate is retained;
learned/user-registered preferences are separate. Current data yields149 added
lower-priority rows, recorded in each generated manifest. Original dictionary
rows and scores are preserved. This conservative rule does not identify every
possible abbreviation, and does not run during typing.

ASCII entries have lowercase, uppercase and title-case forms (score -10).
This produces 183,150 rows; deduplication with the original kana dictionary
leaves 196,034 additions in the combined dictionary. Output spelling preserves
the actual input's case. Arbitrary mixed case still uses the literal path.

UniDic is not English-only and includes abbreviations and etymological
spellings. The importer intersects with the English vocabulary and excludes
known traps such as コンセント/マンション; this is spelling assistance, not
unrestricted translation. The data has not been exhaustively semantically
reviewed. `cache`, `rebase`, `deploy`, `backend`, `frontend` have explicit
project-authored supplementary readings. Newer UniDic versions need their own
schema, license, coverage and performance verification before replacing this
pinned source.

## Packaging and verification

`cargo make build` generates all four dictionary combinations and reverse
indexes. post_build replaces the build output's Dictionary tree and copies the
SwiftPM `azookey-server_azookey-server.resources` directory next to the DLL and
Rust executable. The installed-layout guard does not accept a source-tree
resource fallback. Native probes also exercise a separate distribution
directory with a different CWD, and graceful failure when the bundle is missing.

Kana groups retain their original filenames. ASCII groups use escaped UTF-16
identifiers (`[0041]` / `[0061]`), because `A` and `a` collide on typical Windows
filesystems. The installer replaces the immutable `Dictionary/louds` and
`Dictionary/Variants` trees so old indexes cannot be preloaded alongside the new
ones. These are separate from user settings and learning data.

## Settings and application boundary

The conversion page exposes `general.english_dictionary` (英単語辞書) and
`general.katakana_english_dictionary` (カタカナ英語辞書), both default ON,
including settings files that omit the new fields. `experimental_mixed_input`
remains independently default OFF.

English OFF disables the added ASCII rows and automatic dictionary recognition
(including the small technical seed). Direct/F10 input, the original dictionary,
user registrations and learned candidates remain usable. Kana OFF removes only
the added kana-to-English rows. Main/supplement/reconversion/warmup use the same
effective root.

Changing either switch prepares and validates the next converter pair on the
explicit LoadConfig action. During active input, it retains the old effective
mode, raw input, candidate IDs and snapshots. ClearText atomically swaps the
already-prepared pair for the next composition. Repeated changes keep only the
last desired configuration. Recovery resets/ReplaceComposition keep the pending
choice without applying it while the old composition is replayed. An absolute
empty replay is an idle boundary. New reconversion uses the prepared reverse
index and then changes the effective root once the selection is supported.
Preparation failures preserve the active state and
are reported as server-apply failures to the settings UI. Learning files and
selection overrides stay in their original user directory.

Fresh Initialize failures are returned to the Rust startup path and do not
serve as a successfully initialized engine. Existing loaded Zenzai models are
also prepared in the replacement pair during the settings action, rather than
loading that replacement model on the next key. This potentially slow model
work owns an isolated, unpublished converter and runs outside the input mutex.
Configuration requests are serialized by a separate lock. The completion task
retains ownership even if the caller disconnects, and publishes under the input
mutex only after preparation finishes. The settings RPC has a30-second bounded
preparation budget; typing retains its normal budget. If input starts before
preparation finishes, that composition stays on the old mode. Preparation can
temporarily retain two model contexts; settings-action and first-input costs
need separate evaluation.

This read-only prebuilt-root approach uses additional installed disk space
(roughly156MiB for the three variants, including reverse indexes), rather than
generating/copying dictionaries during typing or scanning a dynamic wordlist.
Only the selected root is normally preloaded. A settings change during active
input temporarily retains both the current and prepared pair until commit/cancel.

```sh
node --test scripts/generate-english-dictionary.test.mjs
python3 scripts/vm.py --profile build test <branch> skip '.|englishDictionary'
```

The owned child-server RPC regression additionally checks real Zenzai model
preparation against concurrent typing with the normal2-second input budget,
and rejects a missing resource in real Rust-server startup:

```sh
SWIFT_TEST_REAL_ZENZAI=1 CARGO_TEST_PACKAGE=azookey-server SWIFT_TEST_JOBS=1 python3 scripts/vm.py --profile build test <branch> dictionary_settings_rpc '.|englishDictionary'
```

The fixture verifies the named-pipe server PID before sending any request,
uses private APPDATA and terminates only its own child server. The VM helper
builds Swift first and provides the exact DLL/resource/dictionary/model layout
for this opt-in Rust integration test. Ordinary cargo test leaves it ignored.

The VM test helper accepts `SWIFT_TEST_JOBS=1` through `16` (default2) to bound
compiler concurrency on memory-constrained guests. This changes build
parallelism, not optimization level or benchmark process conditions.

Focused tests that need only the combined dictionary can set
`SWIFT_TEST_SINGLE_DICTIONARY=1`; this uses the generator's `--single-mode`
option and omits the other three roots. Do not use this option for setting-mode
or reconversion matrix tests. Example:

```sh
SWIFT_TEST_SINGLE_DICTIONARY=1 SWIFT_TEST_REAL_ZENZAI=1 SWIFT_TEST_JOBS=1 python3 scripts/vm.py --profile build test <branch> skip katakanaAbbreviationsPreferKanaToExpandedEnglish
```

The explicit `englishDictionary` VM filter also checks the distribution resource
layout and launches fresh benchmark processes for
base/kana/spelling/both in forward and reverse order. Ordinary all-tests runs
skip the opt-in benchmark. The measurement test requires
`AZOOKEY_DICTIONARY_BENCHMARK_MODE=base|kana|spelling|both` and an appropriately
generated variant directory. The default generator prepares all variants.

Each process preloads two converters, uses the bundled Windows romaji table,
and records initialization, working set, first-key latency, and warmed
append+normalize+candidate latency by input category. The benchmark uses
ordinary KKC without Zenzai, learning, or IPC/UI. It must not be reported as
end-to-end Windows/TSF latency or a Zenzai performance result. Timing values are
evidence, not flaky wall-clock test assertions.

Benchmark base/kana use English OFF including the small seed. These are the new
settings combinations, not the historical seed-enabled pre-expansion baseline.
