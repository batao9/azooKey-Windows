import assert from "node:assert/strict";
import test from "node:test";
import { abbreviatedKanaPairs, buildGroup, generateRecognition, indexIdentifier, makeRecognitionBinary, newRow, parseRows } from "./generate-english-dictionary.mjs";

test("only clear truncated aliases are lower priority; full readings and variants stay normal", () => {
  const rows = [
    ["インスト", "instrumental", "unidic"], ["インストルメンタル", "instrumental", "unidic"],
    ["アプリ", "application", "unidic"], ["アプリケーション", "application", "unidic"],
    ["サーバ", "server", "unidic"], ["サーバー", "server", "unidic"],
    ["インタフェース", "interface", "unidic"], ["インターフェース", "interface", "unidic"],
    ["プロ", "production", "unidic"], ["プロダクション", "production", "unidic"],
    ["プロ", "projectoverride", "project"], ["プロジェクト", "projectoverride", "unidic"],
    ["キャッシュ", "cache", "project"],
  ];
  assert.deepEqual(abbreviatedKanaPairs(rows), new Set([
    "インスト\0instrumental", "アプリ\0application", "プロ\0production",
  ]));
});

test("ASCII index names do not collide on a case-insensitive filesystem", () => {
  const letters = [..."ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz"];
  assert.equal(new Set(letters.map((c) => indexIdentifier(c).toLowerCase())).size, letters.length);
  assert.equal(indexIdentifier("A"), "[0041]");
  assert.equal(indexIdentifier("a"), "[0061]");
  assert.equal(indexIdentifier("ア"), "ア");
});

test("recognition resource is reproducible and independent of executable source", async () => {
  await generateRecognition({ check: true });
  const data = makeRecognitionBinary(["approach", "algorithm", "repository", "approach"]);
  assert.equal(data.subarray(0, 4).toString("ascii"), "AZEV");
  assert.equal(data.readUInt32LE(4), 1);
  assert.equal(data.readUInt32LE(12), 3);
  assert.equal(data.length, 16 + data.readUInt32LE(8) * 8);
  function contains(word) {
    let node = 0;
    for (const label of Buffer.from(word)) {
      const offset = 16 + node * 8;
      const first = data.readUInt32LE(offset), count = data[offset + 4];
      const child = Array.from({ length: count }, (_, i) => first + i).find((i) => data[16 + i * 8 + 5] === label);
      if (child === undefined) return false;
      node = child;
    }
    return data[16 + node * 8 + 6] === 1;
  }
  for (const word of ["approach", "algorithm", "repository"]) assert.ok(contains(word));
  assert.equal(contains("appro"), false);
  assert.equal(contains("approaches"), false);
});

function lookup(files, reading, ids) {
  const bytes = files.get(".louds");
  const bits = [];
  for (let i = 0; i < bytes.length * 8; i++) {
    const byte = Math.floor(i / 64) * 8 + 7 - Math.floor((i % 64) / 8);
    bits.push((bytes[byte] >> (7 - i % 8)) & 1);
  }
  const zeros = bits.flatMap((b, i) => b ? [] : [i]);
  let node = 1;
  for (const char of reading) {
    let ones = bits.slice(0, zeros[node - 1] + 1).filter(Boolean).length;
    let next;
    for (let i = zeros[node - 1] + 1; bits[i] === 1; i++) {
      ones++;
      if (files.get(".loudschars2")[ones] === ids.get(char)) next = ones;
    }
    if (next === undefined) return [];
    node = next;
  }
  const shard = files.get(`${Math.floor(node / 2048)}.loudstxt3`);
  if (!shard) return [];
  const slot = node % 2048;
  const begin = shard.readUInt32LE(2 + slot * 4);
  const end = slot === 2047 ? shard.length : shard.readUInt32LE(6 + slot * 4);
  // Parse this slot with a one-slot wrapper, independent of node numbering.
  const header = Buffer.alloc(6);
  header.writeUInt16LE(1); header.writeUInt32LE(6, 2);
  return parseRows(Buffer.concat([header, shard.subarray(begin, end)]));
}

test("merged LOUDS lookup preserves grammatical rows, scores and prefix topology", () => {
  const ids = new Map([..."\0アプローチabc"].map((c, i) => [c, i]));
  const existing = newRow("アプローチ", "アプローチ", -6.12345);
  const otherGrammar = newRow("アプローチ", "アプローチ", -8.25);
  otherGrammar.numeric.writeUInt16LE(1288, 0);
  const rows = [existing, otherGrammar, newRow("アプローチ", "approach"), newRow("ab", "ab"), newRow("abc", "ABC")];
  const files = buildGroup(rows, ids);
  const result = lookup(files, "アプローチ", ids);
  assert.deepEqual(result.map((r) => r.word), ["アプローチ", "アプローチ", "approach"]);
  assert.ok(result[0].numeric.equals(existing.numeric));
  assert.ok(result[1].numeric.equals(otherGrammar.numeric));
  assert.equal(lookup(files, "ab", ids)[0].word, "ab");
  assert.equal(lookup(files, "abc", ids)[0].word, "ABC");
  assert.deepEqual(lookup(files, "ac", ids), []);
});

test("shard boundaries retain every entry and reject unrepresentable additions", () => {
  const alphabet = "abcdefgh";
  const ids = new Map([...alphabet].map((c, i) => [c, i]));
  const rows = [];
  for (const a of alphabet) for (const b of alphabet) for (const c of alphabet) for (const d of alphabet) {
    const word = a + b + c + d;
    rows.push(newRow(word, word));
  }
  const files = buildGroup(rows, ids);
  const roundtrip = [...files].filter(([name]) => name.endsWith(".loudstxt3")).flatMap(([, bytes]) => parseRows(bytes));
  assert.equal(roundtrip.length, rows.length);
  assert.deepEqual(new Set(roundtrip.map((r) => r.word)), new Set(rows.map((r) => r.word)));
  for (const word of ["aaaa", "dhhg", "hhhh"]) assert.equal(lookup(files, word, ids)[0].word, word);
  assert.throws(() => buildGroup([newRow("z", "z")], ids), /Unrepresentable/);
  assert.throws(() => parseRows(Buffer.from([2, 0])), /Truncated/);
});
