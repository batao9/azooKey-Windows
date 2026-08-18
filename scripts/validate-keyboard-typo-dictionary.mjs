import { readFile, readdir } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { parseLoudstxt3 } from "./generate-reverse-dictionary.mjs";

// The rewrite contract model follows Mozc KeyCorrector. Its BSD-3-Clause
// notice is retained in data/keyboard-typo-rewrite-NOTICE.md.

const EXPECTED_COLUMNS = [
  "id",
  "selection_tier",
  "origin_rule",
  "typed_reading",
  "corrected_reading",
  "expected_candidate",
  "typed_keys",
  "corrected_keys",
  "mechanism",
  "source",
  "train_count",
  "page_count",
  "reverse_count",
  "test_count",
  "match_policy",
  "builtin_collision_count",
];

const REWRITE_COLUMNS = [
  "rule",
  "status",
  "input_scope",
  "dictionary_policy",
  "example_input",
  "example_output",
  "jwtd_exact",
  "jwtd_different",
  "post_triggered",
  "builtin_collision_readings",
  "required_negative_examples",
];

const MECHANISMS = new Set([
  "adjacent_key_substitution",
  "extra_key",
  "geminate_shift",
  "key_repeat",
  "missing_key",
  "mora_repeat",
  "mora_transposition",
  "rewrite_m_fallback",
  "rewrite_ni_fallback",
  "rewrite_nn_fallback",
  "rewrite_yu_fallback",
]);

const SOURCES = new Set(["JWTD_v2_train", "curated_seed", "product_seed"]);
const SELECTION_TIERS = new Set(["candidate", "core", "required"]);
const ORIGIN_RULES = new Set(["none", "NN", "M", "Yu", "NI"]);
const COUNT_COLUMNS = ["train_count", "page_count", "reverse_count", "test_count"];
const REWRITE_COUNT_COLUMNS = [
  "jwtd_exact",
  "jwtd_different",
  "post_triggered",
  "builtin_collision_readings",
];
const QWERTY_NEIGHBORS = new Set(["iu", "rt", "sz", "bh"]);
const FALLBACK_MECHANISM_BY_RULE = new Map([
  ["NN", "rewrite_nn_fallback"],
  ["M", "rewrite_m_fallback"],
  ["Yu", "rewrite_yu_fallback"],
  ["NI", "rewrite_ni_fallback"],
]);

function parseTable(contents, expectedColumns, label) {
  const lines = contents.replace(/\r\n/gu, "\n").split("\n").filter(Boolean);
  if (lines.length === 0) throw new Error(`${label} is empty`);
  const columns = lines[0].split("\t");
  if (columns.join("\t") !== expectedColumns.join("\t")) {
    throw new Error(`unexpected ${label} header: ${columns.join(", ")}`);
  }
  return lines.slice(1).map((line, index) => {
    const values = line.split("\t");
    if (values.length !== columns.length) {
      throw new Error(`line ${index + 2}: expected ${columns.length} columns, got ${values.length}`);
    }
    return Object.fromEntries(columns.map((column, columnIndex) => [column, values[columnIndex]]));
  });
}

function parseTsv(contents) {
  return parseTable(contents, EXPECTED_COLUMNS, "dictionary");
}

function parseRewriteTsv(contents) {
  return parseTable(contents, REWRITE_COLUMNS, "rewrite manifest");
}

function katakana(reading) {
  return [...reading].map((character) => {
    const codePoint = character.codePointAt(0);
    return codePoint >= 0x3041 && codePoint <= 0x3096
      ? String.fromCodePoint(codePoint + 0x60)
      : character;
  }).join("");
}

function hiragana(reading) {
  return [...reading].map((character) => {
    const codePoint = character.codePointAt(0);
    return codePoint >= 0x30a1 && codePoint <= 0x30f6
      ? String.fromCodePoint(codePoint - 0x60)
      : character;
  }).join("");
}

function isHiragana(character) {
  return character >= "ぁ" && character <= "ゖ";
}

function rewriteSmallTsu(reading) {
  const input = [...hiragana(reading)];
  const output = [];
  for (let index = 0; index < input.length;) {
    const first = input[index];
    const last = input[index + 3];
    if (
      index + 3 < input.length
      && isHiragana(first)
      && first !== "っ"
      && input[index + 1] === "っ"
      && input[index + 2] === "っ"
      && isHiragana(last)
      && last !== "っ"
    ) {
      output.push(first, "っ", last);
      index += 4;
    } else {
      output.push(first);
      index += 1;
    }
  }
  return output.join("");
}

function rewriteDoubleNn(reading) {
  const input = [...hiragana(reading)];
  const output = [];
  const vowelContinuation = new Map([
    ["あ", "な"],
    ["い", "に"],
    ["う", "ぬ"],
    ["え", "ね"],
    ["お", "の"],
  ]);
  for (let index = 0; index < input.length;) {
    const first = input[index];
    const continuation = input[index + 3];
    if (
      index + 3 < input.length
      && isHiragana(first)
      && first !== "ん"
      && input[index + 1] === "ん"
      && input[index + 2] === "ん"
      && continuation !== "ん"
    ) {
      output.push(first, "ん", vowelContinuation.get(continuation) ?? continuation);
      index += 4;
    } else {
      output.push(first);
      index += 1;
    }
  }
  return output.join("");
}

function applyEnabledRewrites(reading, rewriteRules) {
  const enabled = new Set(
    rewriteRules.filter((row) => row.status === "enabled").map((row) => row.rule),
  );
  let corrected = hiragana(reading);
  if (enabled.has("DoubleNN")) corrected = rewriteDoubleNn(corrected);
  if (enabled.has("SmallTSU")) corrected = rewriteSmallTsu(corrected);
  return corrected;
}

function canDeleteOne(source, result) {
  if (source.length !== result.length + 1) return false;
  for (let index = 0; index < source.length; index += 1) {
    if (source.slice(0, index) + source.slice(index + 1) === result) return true;
  }
  return false;
}

function isRepeatedKeyInsertion(typed, corrected) {
  if (typed.length !== corrected.length + 1) return false;
  for (let index = 0; index < typed.length; index += 1) {
    if (typed.slice(0, index) + typed.slice(index + 1) !== corrected) continue;
    if (typed[index] === typed[index - 1] || typed[index] === typed[index + 1]) return true;
  }
  return false;
}

function isRepeatedBlockInsertion(typed, corrected) {
  for (let start = 0; start < typed.length; start += 1) {
    for (let end = start + 1; end <= typed.length; end += 1) {
      if (typed.slice(0, start) + typed.slice(end) !== corrected) continue;
      const block = typed.slice(start, end);
      const left = typed.slice(Math.max(0, start - block.length), start);
      const right = typed.slice(end, end + block.length);
      if (left === block || right === block) return true;
    }
  }
  return false;
}

function isAdjacentBlockTransposition(typed, corrected) {
  if (typed.length !== corrected.length) return false;
  for (let start = 0; start < typed.length - 1; start += 1) {
    for (let middle = start + 1; middle < typed.length; middle += 1) {
      for (let end = middle + 1; end <= typed.length; end += 1) {
        const swapped = typed.slice(0, start)
          + typed.slice(middle, end)
          + typed.slice(start, middle)
          + typed.slice(end);
        if (swapped === corrected) return true;
      }
    }
  }
  return false;
}

function onlyAddsLetter(typed, corrected, insertedLetter) {
  let typedIndex = 0;
  let inserted = 0;
  for (const character of corrected) {
    if (character === typed[typedIndex]) {
      typedIndex += 1;
    } else if (character === insertedLetter) {
      inserted += 1;
    } else {
      return false;
    }
  }
  return typedIndex === typed.length && inserted > 0;
}

function mechanismMatchesKeys(row) {
  const typed = row.typed_keys;
  const corrected = row.corrected_keys;
  switch (row.mechanism) {
    case "adjacent_key_substitution": {
      if (typed.length !== corrected.length) return false;
      const differences = [...typed].flatMap((character, index) => (
        character === corrected[index] ? [] : [[character, corrected[index]]]
      ));
      if (differences.length !== 1) return false;
      return QWERTY_NEIGHBORS.has(differences[0].sort().join(""));
    }
    case "extra_key":
      return canDeleteOne(typed, corrected);
    case "key_repeat":
      return isRepeatedKeyInsertion(typed, corrected);
    case "missing_key":
      return canDeleteOne(corrected, typed);
    case "mora_repeat":
      return isRepeatedBlockInsertion(typed, corrected);
    case "mora_transposition":
      return isAdjacentBlockTransposition(typed, corrected);
    case "geminate_shift":
      return typed.length === corrected.length;
    case "rewrite_nn_fallback":
    case "rewrite_m_fallback":
    case "rewrite_ni_fallback":
      return typed === corrected;
    case "rewrite_yu_fallback":
      return onlyAddsLetter(typed, corrected, "u");
    default:
      return false;
  }
}

function validateRewriteRules(rows) {
  const errors = [];
  const expected = new Map([
    ["SmallTSU", ["enabled", "exclude_exact_overlap"]],
    ["DoubleNN", ["enabled", "exclude_exact_overlap"]],
    ["NN", ["experiment", "fixed_dictionary_candidates"]],
    ["M", ["experiment", "raw_dictionary_candidates"]],
    ["Yu", ["deferred", "fixed_dictionary_candidates"]],
    ["NI", ["deferred", "fixed_dictionary_candidates"]],
  ]);
  const names = new Set();
  for (const [index, row] of rows.entries()) {
    const line = index + 2;
    for (const column of REWRITE_COLUMNS) {
      if (row[column] === "") errors.push(`rewrite line ${line}: ${column} must not be empty`);
    }
    if (names.has(row.rule)) errors.push(`rewrite line ${line}: duplicate rule ${row.rule}`);
    names.add(row.rule);
    const contract = expected.get(row.rule);
    if (!contract) {
      errors.push(`rewrite line ${line}: unknown rule ${row.rule}`);
      continue;
    }
    if (row.status !== contract[0]) errors.push(`rewrite line ${line}: ${row.rule} must be ${contract[0]}`);
    if (row.dictionary_policy !== contract[1]) {
      errors.push(`rewrite line ${line}: unexpected dictionary policy for ${row.rule}`);
    }
    if (row.input_scope !== "roman2kana_only") {
      errors.push(`rewrite line ${line}: input_scope must be roman2kana_only`);
    }
    for (const column of REWRITE_COUNT_COLUMNS) {
      if (!/^\d+$/u.test(row[column])) errors.push(`rewrite line ${line}: ${column} must be an integer`);
    }
  }
  for (const rule of expected.keys()) {
    if (!names.has(rule)) errors.push(`rewrite manifest is missing ${rule}`);
  }
  return errors;
}

function validateRows(rows, rewriteRules = []) {
  const errors = [];
  const typedReadings = new Set();
  const ids = new Set();
  const rewriteStatus = new Map(rewriteRules.map((row) => [row.rule, row.status]));
  if (rows.length === 0) errors.push("dictionary must not be empty");

  rows.forEach((row, index) => {
    const line = index + 2;
    if (!/^\d+$/u.test(row.id)) errors.push(`line ${line}: id must be an integer`);
    if (ids.has(row.id)) errors.push(`line ${line}: duplicate id ${row.id}`);
    ids.add(row.id);
    for (const column of EXPECTED_COLUMNS) {
      if (row[column] === "") errors.push(`line ${line}: ${column} must not be empty`);
    }
    if (typedReadings.has(row.typed_reading)) {
      errors.push(`line ${line}: duplicate typed_reading ${row.typed_reading}`);
    }
    typedReadings.add(row.typed_reading);
    if (row.typed_reading === row.corrected_reading) {
      errors.push(`line ${line}: correction does not change the reading`);
    }
    if (!/^[a-z-]+$/u.test(row.typed_keys) || !/^[a-z-]+$/u.test(row.corrected_keys)) {
      errors.push(`line ${line}: key sequences must contain lowercase ASCII letters or a long-vowel key`);
    }
    if (!MECHANISMS.has(row.mechanism)) errors.push(`line ${line}: unknown mechanism ${row.mechanism}`);
    else if (!mechanismMatchesKeys(row)) {
      errors.push(`line ${line}: key sequences do not match ${row.mechanism}`);
    }
    if (!SOURCES.has(row.source)) errors.push(`line ${line}: unknown source ${row.source}`);
    if (!SELECTION_TIERS.has(row.selection_tier)) {
      errors.push(`line ${line}: unknown selection_tier ${row.selection_tier}`);
    }
    if (!ORIGIN_RULES.has(row.origin_rule)) errors.push(`line ${line}: unknown origin_rule ${row.origin_rule}`);

    const fallbackMechanism = FALLBACK_MECHANISM_BY_RULE.get(row.origin_rule);
    if (row.origin_rule === "none") {
      if (row.selection_tier === "candidate") errors.push(`line ${line}: candidate must identify its origin rule`);
    } else {
      if (row.selection_tier !== "candidate") errors.push(`line ${line}: rewrite fallback must be a candidate`);
      if (row.mechanism !== fallbackMechanism) {
        errors.push(`line ${line}: ${row.origin_rule} must use ${fallbackMechanism}`);
      }
      if (!rewriteStatus.has(row.origin_rule)) errors.push(`line ${line}: origin rule is absent from manifest`);
      if (rewriteStatus.get(row.origin_rule) === "enabled") {
        errors.push(`line ${line}: enabled rewrite ${row.origin_rule} must not own dictionary entries`);
      }
    }

    const expectedMatchPolicy = row.origin_rule === "M"
      || row.typed_keys.endsWith("n")
      || row.typed_keys.includes("-")
      ? "raw_lattice_span_exact"
      : "lattice_span_exact";
    if (row.match_policy !== expectedMatchPolicy) {
      errors.push(`line ${line}: match_policy must be ${expectedMatchPolicy}`);
    }

    for (const column of COUNT_COLUMNS) {
      if (!/^\d+$/u.test(row[column])) errors.push(`line ${line}: ${column} must be an integer`);
    }
    if (!/^\d+$/u.test(row.builtin_collision_count)) {
      errors.push(`line ${line}: builtin_collision_count must be an integer`);
    }
    const trainCount = Number(row.train_count);
    const pageCount = Number(row.page_count);
    const reverseCount = Number(row.reverse_count);
    if (row.source === "product_seed" || row.source === "curated_seed") {
      if (COUNT_COLUMNS.some((column) => row[column] !== "0")) {
        errors.push(`line ${line}: ${row.source} must not claim corpus counts`);
      }
    } else if (row.selection_tier === "core") {
      if (trainCount < 5 || pageCount < 5) {
        errors.push(`line ${line}: core JWTD evidence must cover at least 5 edits and 5 pages`);
      }
      const dominance = trainCount / (trainCount + reverseCount);
      if (dominance < 0.98) errors.push(`line ${line}: core correction direction is below 98% dominance`);
    } else if (trainCount < 1 || pageCount < 1) {
      errors.push(`line ${line}: candidate JWTD evidence must cover at least one edit and page`);
    }

    if (row.selection_tier === "required" && row.source !== "product_seed") {
      errors.push(`line ${line}: required rows must be product_seed`);
    }
    if (row.selection_tier === "core" && row.source !== "JWTD_v2_train") {
      errors.push(`line ${line}: core rows must be JWTD_v2_train`);
    }

    const rewritten = applyEnabledRewrites(row.typed_reading, rewriteRules);
    if (rewritten !== hiragana(row.typed_reading) && rewritten === hiragana(row.corrected_reading)) {
      errors.push(`line ${line}: ${row.typed_reading} is already owned by an enabled rewrite`);
    }
  });

  if (rows.filter((row) => row.source === "product_seed").length !== 2) {
    errors.push("expected exactly 2 product_seed rows");
  }
  return errors;
}

async function findBundledReadingCollisions(rows, dictionaryDirectory) {
  const wanted = new Map();
  for (const row of rows) {
    const reading = katakana(row.typed_reading);
    const typedReadings = wanted.get(reading) ?? [];
    typedReadings.push(row.typed_reading);
    wanted.set(reading, typedReadings);
  }
  const collisions = new Map(rows.map((row) => [row.typed_reading, new Set()]));
  const files = (await readdir(dictionaryDirectory))
    .filter((file) => file.endsWith(".loudstxt3"))
    .sort();
  const readingsBySurface = new Map();
  for (const file of files) {
    parseLoudstxt3(await readFile(join(dictionaryDirectory, file)), file, readingsBySurface);
  }
  for (const [surface, readings] of readingsBySurface) {
    for (const reading of readings.keys()) {
      for (const typedReading of wanted.get(reading) ?? []) {
        collisions.get(typedReading).add(surface);
      }
    }
  }
  return collisions;
}

async function main() {
  const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
  const dictionaryFile = resolve(process.argv[2] ?? join(repoRoot, "data", "keyboard-typo-dictionary.tsv"));
  const rewriteFile = resolve(process.argv[3] ?? join(repoRoot, "data", "keyboard-typo-rewrite-rules.tsv"));
  const rows = parseTsv(await readFile(dictionaryFile, "utf8"));
  const rewriteRules = parseRewriteTsv(await readFile(rewriteFile, "utf8"));
  const errors = [...validateRewriteRules(rewriteRules), ...validateRows(rows, rewriteRules)];
  const bundledDirectory = join(repoRoot, "server-swift", "azooKey_dictionary_storage", "Dictionary", "louds");
  const collisions = await findBundledReadingCollisions(rows, bundledDirectory);
  for (const row of rows) {
    const actualCount = collisions.get(row.typed_reading)?.size ?? 0;
    if (actualCount !== Number(row.builtin_collision_count)) {
      errors.push(
        `${row.typed_reading}: declared bundled collisions=${row.builtin_collision_count}, actual=${actualCount}`,
      );
    }
  }
  if (errors.length > 0) throw new Error(errors.join("\n"));

  const corpusRows = rows.filter((row) => row.source === "JWTD_v2_train");
  const trainCount = corpusRows.reduce((sum, row) => sum + Number(row.train_count), 0);
  const testCount = corpusRows.reduce((sum, row) => sum + Number(row.test_count), 0);
  const collisionCount = [...collisions.values()].reduce((sum, surfaces) => sum + surfaces.size, 0);
  const originCounts = new Map();
  for (const row of rows) originCounts.set(row.origin_rule, (originCounts.get(row.origin_rule) ?? 0) + 1);
  process.stdout.write(
    `Validated ${rows.length} entries (JWTD train edits=${trainCount}, test edits=${testCount}, bundled collision surfaces=${collisionCount})\n`,
  );
  process.stdout.write(
    `${[...originCounts].sort().map(([rule, count]) => `${rule}=${count}`).join(", ")}\n`,
  );
}

export {
  applyEnabledRewrites,
  findBundledReadingCollisions,
  hiragana,
  katakana,
  mechanismMatchesKeys,
  parseRewriteTsv,
  parseTsv,
  rewriteDoubleNn,
  rewriteSmallTsu,
  validateRewriteRules,
  validateRows,
};

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  await main();
}
