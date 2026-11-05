import { createHash } from "node:crypto";
import { cp, mkdir, readdir, readFile, rm, writeFile } from "node:fs/promises";
import { dirname, isAbsolute, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { generateReverseDictionary } from "./generate-reverse-dictionary.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const shardSize = 2048;
const resourcePath = join(root, "server-swift/Sources/azookey-server/Resources/english-vocabulary.bin");
export const identifier = (char) => `[${char.charCodeAt(0).toString(16).toUpperCase().padStart(4, "0")}]`;
export const indexIdentifier = (char) => /^[A-Za-z]$/u.test(char) ? identifier(char) : char;

// Preserve the original float32 bytes and every row (including equal surfaces
// with different grammatical IDs). The reverse-index parser deduplicates rows,
// so cannot be used for a lossless dictionary merge.
export function parseRows(data) {
  const slots = data.readUInt16LE(0);
  const rows = [];
  if (2 + slots * 4 > data.length) throw new Error("Truncated shard header");
  for (let i = 0; i < slots; i++) {
    const start = data.readUInt32LE(2 + i * 4);
    const end = i + 1 === slots ? data.length : data.readUInt32LE(6 + i * 4);
    if (start < 2 + slots * 4 || end < start + 2 || end > data.length) throw new Error("Invalid shard offsets");
    const count = data.readUInt16LE(start);
    if (!count) continue;
    if (start + 2 + count * 10 > end) throw new Error("Truncated row metadata");
    const fields = new TextDecoder("utf-8", { fatal: true }).decode(data.subarray(start + 2 + count * 10, end)).split("\t");
    if (fields.length !== count + 1 || !fields[0]) throw new Error("Invalid row text");
    for (let r = 0; r < count; r++) {
      rows.push({ reading: fields[0], word: fields[r + 1] || fields[0], numeric: Buffer.from(data.subarray(start + 2 + r * 10, start + 12 + r * 10)), allowUnknown: true });
    }
  }
  return rows;
}

export function newRow(reading, word, score = -12) {
  const numeric = Buffer.alloc(10);
  numeric.writeUInt16LE(1285, 0); // 一般名詞, not user-dictionary priority
  numeric.writeUInt16LE(1285, 2);
  numeric.writeUInt16LE(501, 4); // 一般
  numeric.writeFloatLE(score, 6);
  return { reading, word, numeric };
}

// Conservative short-form recognition, computed offline. Only a 2–4-character
// reading that is a substantially truncated prefix of another reading for the
// *same spelling* is demoted. Ordinary long-vowel/spelling variants are not.
export function abbreviatedKanaPairs(rows) {
  const readings = new Map();
  for (const [reading, word] of rows) {
    if (!readings.has(word)) readings.set(word, new Set());
    readings.get(word).add(reading);
  }
  const result = new Set();
  for (const [reading, word, source] of rows) {
    if (source !== "unidic" || reading.length < 2 || reading.length > 4) continue;
    if ([...readings.get(word)].some((full) => full.startsWith(reading)
      && full.length >= reading.length + 3 && reading.length * 5 <= full.length * 3)) {
      result.add(`${reading}\0${word}`);
    }
  }
  return result;
}

export function buildGroup(rows, charIDs) {
  const nodes = [{ children: new Map(), rows: [], label: 0 }];
  for (const row of rows) {
    let node = 0;
    for (const char of row.reading) {
      // The shipped dictionary uses 255 for characters absent from charID.chid.
      // Preserve those legacy paths; never silently admit new unmappable rows.
      const label = charIDs.get(char) ?? (row.allowUnknown ? 255 : undefined);
      if (label === undefined) throw new Error(`Unrepresentable reading: ${row.reading}`);
      if (!nodes[node].children.has(label)) {
        nodes[node].children.set(label, nodes.length);
        nodes.push({ children: new Map(), rows: [], label });
      }
      node = nodes[node].children.get(label);
    }
    nodes[node].rows.push(row);
  }
  const order = [0];
  const labels = [0, 0];
  const bits = [1, 0];
  const indexedRows = new Map();
  for (let i = 0; i < order.length; i++) {
    const node = nodes[order[i]];
    const children = [...node.children].sort((a, b) => a[0] - b[0]);
    bits.push(...children.map(() => 1), 0);
    for (const [label, child] of children) { order.push(child); labels.push(label); }
    if (node.rows.length) indexedRows.set(i + 1, node.rows);
  }
  const louds = Buffer.alloc(Math.ceil(bits.length / 64) * 8, 255);
  for (let i = 0; i < bits.length; i++) {
    if (!bits[i]) {
      const byte = Math.floor(i / 64) * 8 + 7 - Math.floor((i % 64) / 8);
      louds[byte] &= ~(1 << (7 - (i % 8)));
    }
  }
  const shards = new Map();
  for (const [index, entries] of indexedRows) {
    const shard = Math.floor(index / shardSize);
    if (!shards.has(shard)) shards.set(shard, Array.from({ length: shardSize }, () => Buffer.alloc(2)));
    const prefix = Buffer.alloc(2);
    if (entries.length > 65535) throw new Error("Too many rows per reading");
    prefix.writeUInt16LE(entries.length);
    const text = [entries[0].reading, ...entries.map((r) => r.word === r.reading ? "" : r.word)].join("\t");
    shards.get(shard)[index % shardSize] = Buffer.concat([prefix, ...entries.map((r) => r.numeric), Buffer.from(text)]);
  }
  const files = new Map([[".louds", louds], [".loudschars2", Buffer.from(labels)]]);
  for (const [shard, payloads] of shards) {
    const header = Buffer.alloc(2 + shardSize * 4);
    header.writeUInt16LE(shardSize);
    let offset = header.length;
    payloads.forEach((payload, index) => { header.writeUInt32LE(offset, 2 + index * 4); offset += payload.length; });
    files.set(`${shard}.loudstxt3`, Buffer.concat([header, ...payloads]));
  }
  return files;
}

export function makeRecognitionBinary(words) {
  const nodes = [{ children: new Map(), terminal: false, label: 0 }];
  for (const word of [...words].sort()) {
    if (!/^[a-z]{4,20}$/u.test(word)) throw new Error(`Invalid recognition word: ${word}`);
    let node = 0;
    for (const label of Buffer.from(word, "ascii")) {
      if (!nodes[node].children.has(label)) {
        nodes[node].children.set(label, nodes.length);
        nodes.push({ children: new Map(), terminal: false, label });
      }
      node = nodes[node].children.get(label);
    }
    nodes[node].terminal = true;
  }
  const result = Buffer.alloc(16 + nodes.length * 8);
  result.write("AZEV", 0, "ascii");
  result.writeUInt32LE(1, 4); result.writeUInt32LE(nodes.length, 8);
  result.writeUInt32LE(new Set(words).size, 12);
  const order = [0];
  for (let i = 0; i < order.length; i++) {
    const node = nodes[order[i]];
    const children = [...node.children].sort((a, b) => a[0] - b[0]);
    const offset = 16 + i * 8;
    result.writeUInt32LE(children.length ? order.length : 0, offset);
    result[offset + 4] = children.length; result[offset + 5] = node.label;
    result[offset + 6] = Number(node.terminal);
    order.push(...children.map(([, child]) => child));
  }
  return result;
}

export async function generateRecognition({ check = false } = {}) {
  const bytes = await readFile(join(root, "data/english-dictionaries/spelling.tsv"));
  const sources = JSON.parse(await readFile(join(root, "data/english-dictionaries/sources.json"), "utf8"));
  if (createHash("sha256").update(bytes).digest("hex") !== sources.spelling_sha256) throw new Error("Recognition source checksum mismatch");
  const words = bytes.toString("utf8").trim().split("\n").filter((line) => !line.startsWith("#")).map((line) => line.split("\t")[0]);
  if (new Set(words).size !== sources.spellings) throw new Error("Recognition source count mismatch");
  const binary = makeRecognitionBinary(words);
  if (check) {
    if (!(await readFile(resourcePath)).equals(binary)) throw new Error("Recognition resource is stale; regenerate it");
  } else {
    await mkdir(dirname(resourcePath), { recursive: true });
    await writeFile(resourcePath, binary);
  }
  console.log(`Recognition resource: words=${words.length} bytes=${binary.length} sha256=${createHash("sha256").update(binary).digest("hex")}`);
}

export async function generate({ source, output, kana = true, spelling = true, runtimeOnly = false }) {
  source = resolve(source); output = resolve(output);
  const inside = (parent, child) => { const path = relative(parent, child); return path === "" || (!path.startsWith("..") && !isAbsolute(path)); };
  if (inside(source, output) || inside(output, source)) throw new Error("Source/output must be separate trees");
  // Output is a derived build directory. Never mutate the dictionary submodule.
  const previousFiles = await readdir(output).catch((error) => { if (error.code === "ENOENT") return []; throw error; });
  if (previousFiles.length && !previousFiles.includes(".english-build-output") && !previousFiles.includes("english-manifest.json")) {
    throw new Error("Refusing to replace a non-generated output directory");
  }
  await rm(output, { recursive: true, force: true });
  await mkdir(output, { recursive: true });
  await writeFile(join(output, ".english-build-output"), "Generated by scripts/generate-english-dictionary.mjs\n");
  console.log(`English dictionary: copying immutable source kana=${kana} spelling=${spelling}`);
  let visited = 0;
  await cp(source, output, { recursive: true, filter: (path) => {
    if (++visited % 256 === 0) console.log(`English dictionary: copy visited=${visited} kana=${kana} spelling=${spelling}`);
    const part = relative(source, path).split(/[\\/]/u)[0];
    return !part || (runtimeOnly ? ["louds", "cb", "mm.binary"].includes(part) : !["Variants", "Reverse"].includes(part));
  } });
  console.log(`English dictionary: source copied visited=${visited}`);
  const dir = join(output, "louds");
  const charIDs = new Map([...(await readFile(join(dir, "charID.chid"), "utf8"))].map((c, i) => [c, i]));
  if (charIDs.size > 256) throw new Error("Character-ID overflow");
  const dataDir = join(root, "data", "english-dictionaries");
  const groups = new Map();
  let added = 0, duplicates = 0, preserved = 0, lowerPriorityKanaRows = 0;
  function add(row) {
    const first = [...row.reading][0];
    if (!groups.has(first)) groups.set(first, []);
    groups.get(first).push(row);
    added++;
  }
  const sources = JSON.parse(await readFile(join(dataDir, "sources.json"), "utf8"));
  for (const [name, enabled] of [["katakana", kana], ["spelling", spelling]]) {
    const bytes = await readFile(join(dataDir, `${name}.tsv`));
    if (createHash("sha256").update(bytes).digest("hex") !== sources[`${name}_sha256`]) throw new Error(`${name} source checksum mismatch`);
    if (!enabled) continue;
    const rows = bytes.toString("utf8").trim().split("\n").filter((line) => !line.startsWith("#")).map((line) => line.split("\t"));
    const abbreviated = name === "katakana" ? abbreviatedKanaPairs(rows) : new Set();
    for (const [reading, word] of rows) {
      if (name === "katakana") {
        if (!/^[ァ-ヺー]{2,20}$/u.test(reading) || !/^[a-z]{4,20}$/u.test(word)) throw new Error("Invalid kana entry");
        const lowerPriority = abbreviated.has(`${reading}\0${word}`);
        add({ ...newRow(reading, word, lowerPriority ? -16 : -12), lowerPriority });
      } else {
        if (!/^[a-z]{4,20}$/u.test(reading)) throw new Error("Invalid spelling entry");
        for (const form of new Set([reading, reading.toUpperCase(), reading[0].toUpperCase() + reading.slice(1)])) add(newRow(form, form, -10));
      }
    }
  }
  const names = await readdir(dir);
  let completedGroups = 0;
  for (const [first, additions] of [...groups].sort((a, b) => a[0] < b[0] ? -1 : 1)) {
    // NTFS normally treats a/A as the same filename. ASCII groups must use
    // escaped identifiers; kana can retain the original dictionary filenames.
    const id = indexIdentifier(first);
    const prefixes = new Set([first, identifier(first)]);
    const oldFiles = names.filter((name) => [...prefixes].some((prefix) => name === `${prefix}.louds` || name === `${prefix}.loudschars2` || (name.startsWith(prefix) && /^\d+\.loudstxt3$/u.test(name.slice(prefix.length)))));
    const original = [];
    for (const name of oldFiles.filter((n) => n.endsWith(".loudstxt3")).sort()) original.push(...parseRows(await readFile(join(dir, name))));
    const keys = new Set(original.map((r) => `${r.reading}\0${r.word}`));
    const selected = additions.filter((r) => { const key = `${r.reading}\0${r.word}`; if (keys.has(key)) { duplicates++; return false; } keys.add(key); return true; });
    lowerPriorityKanaRows += selected.filter((row) => row.lowerPriority).length;
    const files = buildGroup([...original, ...selected], charIDs);
    // Validate the complete multiset, not just totals. This catches accidental
    // reading collisions (including legacy unknown-ID paths) and score loss.
    const rowKey = (r) => `${r.reading}\0${r.word}\0${r.numeric.toString("hex")}`;
    const expected = new Map();
    for (const row of [...original, ...selected]) { const key = rowKey(row); expected.set(key, (expected.get(key) ?? 0) + 1); }
    for (const [name, bytes] of files) {
      if (!name.endsWith(".loudstxt3")) continue;
      for (const row of parseRows(bytes)) {
        const key = rowKey(row);
        if (!expected.get(key)) throw new Error(`Round-trip mismatch in ${first}: ${row.reading}`);
        expected.set(key, expected.get(key) - 1);
      }
    }
    if ([...expected.values()].some((n) => n !== 0)) throw new Error(`Lost dictionary rows in ${first}`);
    // A rebuilt trie can omit an empty old shard. Still overwrite that file on
    // upgrade with an empty shard, so no obsolete rows remain in the payload.
    for (const name of oldFiles.filter((n) => n.endsWith(".loudstxt3"))) {
      const prefix = [...prefixes].find((p) => name.startsWith(p) && /^\d+\.loudstxt3$/u.test(name.slice(p.length)));
      const suffix = name.slice(prefix.length);
      if (!files.has(suffix)) {
        const empty = Buffer.alloc(2 + shardSize * 6);
        empty.writeUInt16LE(shardSize);
        for (let i = 0; i < shardSize; i++) empty.writeUInt32LE(2 + shardSize * 4 + i * 2, 2 + i * 4);
        files.set(suffix, empty);
      }
    }
    preserved += original.length;
    for (const name of oldFiles) await rm(join(dir, name));
    for (const [suffix, bytes] of files) await writeFile(join(dir, id + suffix), bytes);
    if (++completedGroups % 16 === 0) console.log(`English dictionary: rebuilt_groups=${completedGroups}/${groups.size}`);
  }
  await cp(join(dataDir, "NOTICE.txt"), join(output, "English-NOTICE.txt"));
  const manifest = { format: 2, kana, spelling, source: sources, requestedRows: added, duplicates, addedRows: added - duplicates, lowerPriorityKanaRows, preservedRowsInRebuiltGroups: preserved, losslessRoundtripVerified: true };
  await writeFile(join(output, "english-manifest.json"), JSON.stringify(manifest, null, 2) + "\n");
  console.log(JSON.stringify(manifest));
  return manifest;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const options = { source: join(root, "server-swift/azooKey_dictionary_storage/Dictionary"), output: join(root, "target/azookey-english-dictionary/Dictionary") };
  let matrix = true, recognitionOnly = false, check = false;
  for (let i = 2; i < process.argv.length; i++) {
    const arg = process.argv[i];
    if (arg === "--source" || arg === "--output") options[arg.slice(2)] = process.argv[++i];
    else if (arg === "--no-kana") { options.kana = false; matrix = false; }
    else if (arg === "--no-spelling") { options.spelling = false; matrix = false; }
    else if (arg === "--recognition-only") recognitionOnly = true;
    else if (arg === "--single-mode") matrix = false;
    else if (arg === "--check") { check = true; recognitionOnly = true; }
    else throw new Error(`Unknown argument: ${arg}`);
  }
  await generateRecognition({ check });
  if (!recognitionOnly) {
    await generate(options);
    await generateReverseDictionary({ source: join(options.output, "louds"), output: join(options.output, "Reverse/reverse-v2.bin") });
    if (matrix) {
      for (const [mode, kana, spelling] of [["base", false, false], ["kana", true, false], ["spelling", false, true]]) {
        const output = join(options.output, "Variants", mode);
        await generate({ source: options.source, output, kana, spelling, runtimeOnly: true });
        await generateReverseDictionary({ source: join(output, "louds"), output: join(output, "Reverse/reverse-v2.bin") });
      }
    }
  }
}
