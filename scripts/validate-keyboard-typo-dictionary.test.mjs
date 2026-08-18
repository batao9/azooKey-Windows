import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import test from "node:test";

import {
  applyEnabledRewrites,
  findBundledReadingCollisions,
  hiragana,
  katakana,
  mechanismMatchesKeys,
  parseRewriteTsv,
  parseTsv,
  validateRewriteRules,
  validateRows,
} from "./validate-keyboard-typo-dictionary.mjs";
import {
  generateSwift,
  lookupReading,
  scoreFor,
} from "./generate-keyboard-typo-dictionary-swift.mjs";

const repoRoot = join(import.meta.dirname, "..");
const dictionaryFile = join(repoRoot, "data", "keyboard-typo-dictionary.tsv");
const rewriteFile = join(repoRoot, "data", "keyboard-typo-rewrite-rules.tsv");
const generatedSwiftFile = join(
  repoRoot,
  "server-swift",
  "Sources",
  "azookey-server",
  "KeyboardTypoDictionary.swift",
);

async function loadSelection() {
  const dictionaryContents = await readFile(dictionaryFile, "utf8");
  const rewriteContents = await readFile(rewriteFile, "utf8");
  return {
    dictionaryContents,
    rows: parseTsv(dictionaryContents),
    rewriteRules: parseRewriteTsv(rewriteContents),
  };
}

test("curated keyboard typo dictionary satisfies its rewrite-aware contract", async () => {
  const { rows, rewriteRules } = await loadSelection();

  assert.deepEqual(validateRewriteRules(rewriteRules), []);
  assert.deepEqual(validateRows(rows, rewriteRules), []);
  assert.equal(rows.length, 96);
  assert.equal(rows.filter((row) => row.source === "product_seed").length, 2);
  assert.equal(rows.filter((row) => row.source === "JWTD_v2_train").length, 55);
  assert.equal(rows.filter((row) => row.source === "curated_seed").length, 39);
  assert.ok(rows.every(mechanismMatchesKeys));

  const originCounts = new Map();
  for (const row of rows) {
    originCounts.set(row.origin_rule, (originCounts.get(row.origin_rule) ?? 0) + 1);
  }
  assert.equal(originCounts.get("none"), 39);
  assert.equal(originCounts.get("NN"), 10);
  assert.equal(originCounts.get("M"), 10);
  assert.equal(originCounts.get("Yu"), 28);
  assert.equal(originCounts.get("NI"), 9);
});

test("generated Swift dictionary stays synchronized with the reviewed TSV", async () => {
  const { rows } = await loadSelection();
  const generated = generateSwift(rows);
  assert.equal(await readFile(generatedSwiftFile, "utf8"), generated);
  assert.match(generated, /let keyboardTypoDictionaryEntryCount = 96/u);
  assert.match(generated, /word: "しました",\n\s+ruby: "シマスタ"/u);
  assert.match(generated, /word: "ご確認",\n\s+ruby: "ゴカクニ"/u);
  assert.match(generated, /word: "ファッション",\n\s+ruby: "ファショn"/u);
  assert.match(generated, /func disableLearningForKeyboardTypoDictionaryCandidates/u);
  assert.equal(scoreFor({ selection_tier: "required" }, 0), "-6.00");
  assert.equal(scoreFor({ selection_tier: "core" }, 0), "-8.00");
  assert.equal(scoreFor({ selection_tier: "candidate" }, 0), "-10.00");
});

test("terminal n entries use the unresolved roman-composer surface", () => {
  assert.equal(lookupReading({ id: "test", typed_reading: "しんぶん", typed_keys: "sinbun" }), "シンブn");
  assert.equal(lookupReading({ id: "test", typed_reading: "ところ", typed_keys: "tokoro" }), "トコロ");
  assert.throws(
    () => lookupReading({ id: "test", typed_reading: "ところ", typed_keys: "tokoron" }),
    /terminal n key must correspond to a terminal ん/u,
  );
});

test("long-vowel entries use the unresolved physical key", () => {
  assert.equal(lookupReading({ id: "test", typed_reading: "ヨーロパ", typed_keys: "yo-ropa" }), "ヨ-ロパ");
  assert.throws(
    () => lookupReading({ id: "test", typed_reading: "ヨロパ", typed_keys: "yo-ropa" }),
    /long-vowel keys must correspond to long-vowel marks/u,
  );
});

test("SmallTSU and DoubleNN own their exact overlaps instead of the dictionary", async () => {
  const { rows, rewriteRules } = await loadSelection();
  const readings = new Set(rows.map((row) => row.typed_reading));

  assert.equal(applyEnabledRewrites("きっって", rewriteRules), "きって");
  assert.equal(applyEnabledRewrites("きっっって", rewriteRules), "きっっって");
  assert.equal(applyEnabledRewrites("こんんにちは", rewriteRules), "こんにちは");
  assert.equal(applyEnabledRewrites("こんんいちは", rewriteRules), "こんにちは");
  assert.equal(applyEnabledRewrites("あんんんたい", rewriteRules), "あんんんたい");
  assert.equal(applyEnabledRewrites("オープニンング", rewriteRules), "おーぷにんぐ");

  for (const reading of [
    "なっって",
    "なっった",
    "よっって",
    "であっった",
    "しまっって",
    "なかっった",
    "あっった",
    "だっった",
    "ほとんんど",
    "オープニンング",
  ]) {
    assert.equal(readings.has(reading), false, `${reading} must be owned by an enabled rewrite`);
  }
});

test("disabled broad rewrites contribute only fixed candidate entries", async () => {
  const { rows, rewriteRules } = await loadSelection();
  const status = new Map(rewriteRules.map((row) => [row.rule, row.status]));

  assert.equal(status.get("NN"), "experiment");
  assert.equal(status.get("M"), "experiment");
  assert.equal(status.get("Yu"), "deferred");
  assert.equal(status.get("NI"), "deferred");
  for (const row of rows.filter((entry) => entry.origin_rule !== "none")) {
    assert.notEqual(status.get(row.origin_rule), "enabled");
    assert.equal(row.selection_tier, "candidate");
  }

  const candidates = new Map(rows.map((row) => [row.typed_reading, row.expected_candidate]));
  assert.equal(candidates.get("みんあ"), "みんな");
  assert.equal(candidates.get("しmばし"), "新橋");
  assert.equal(candidates.get("ちゅごく"), "中国");
  assert.equal(candidates.get("せにょう"), "専用");
});

test("bundled reading collisions are explicit and high-risk entries are excluded", async () => {
  const { rows } = await loadSelection();
  const dictionaryDirectory = join(
    repoRoot,
    "server-swift",
    "azooKey_dictionary_storage",
    "Dictionary",
    "louds",
  );
  const collisions = await findBundledReadingCollisions(rows, dictionaryDirectory);

  for (const row of rows) {
    assert.equal(
      collisions.get(row.typed_reading)?.size ?? 0,
      Number(row.builtin_collision_count),
      row.typed_reading,
    );
  }
  assert.deepEqual([...collisions.get("ちゅおう")].sort(), ["中欧"]);
  assert.deepEqual(
    [...collisions.get("いすれ")].sort(),
    ["いすれ", "医すれ", "委すれ", "慰すれ"].sort(),
  );
  const readings = new Set(rows.map((row) => row.typed_reading));
  for (const excluded of ["タクス", "ちゅもん", "しゅちゅう", "しにゅう", "せにゅう"]) {
    assert.equal(readings.has(excluded), false, `${excluded} has a valid bundled reading`);
  }
});

test("kana readings are normalized for rewrite and bundled dictionary comparison", () => {
  assert.equal(katakana("ごかくに"), "ゴカクニ");
  assert.equal(katakana("ファション"), "ファション");
  assert.equal(hiragana("オープニンング"), "おーぷにんんぐ");
});

test("validator rejects duplicate readings and unsupported matching", async () => {
  const { dictionaryContents, rewriteRules } = await loadSelection();
  const [header, firstRow] = dictionaryContents.trimEnd().split("\n");
  const columns = header.split("\t");
  const values = firstRow.split("\t");
  values[columns.indexOf("match_policy")] = "prefix";
  const invalidRow = values.join("\t");
  const rows = parseTsv(`${header}\n${invalidRow}\n${invalidRow}\n`);

  const errors = validateRows(rows, rewriteRules);
  assert.ok(errors.some((error) => error.includes("duplicate typed_reading")));
  assert.ok(errors.some((error) => error.includes("match_policy must be lattice_span_exact")));
});
