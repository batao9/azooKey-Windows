import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import { join } from "node:path";
import test from "node:test";

import { makeBinary, parseLoudstxt3 } from "./generate-reverse-dictionary.mjs";

function makeSlot(reading, rows) {
  const text = Buffer.from([reading, ...rows.map(({ surface }) => surface)].join("\t"), "utf8");
  const slot = Buffer.alloc(2 + rows.length * 10 + text.length);
  slot.writeUInt16LE(rows.length, 0);
  rows.forEach(({ score }, index) => {
    slot.writeFloatLE(score, 2 + index * 10 + 6);
  });
  text.copy(slot, 2 + rows.length * 10);
  return slot;
}

function makeDictionary(slots) {
  const headerSize = 2 + slots.length * 4;
  const data = Buffer.alloc(headerSize + slots.reduce((sum, slot) => sum + slot.length, 0));
  data.writeUInt16LE(slots.length, 0);
  let offset = headerSize;
  slots.forEach((slot, index) => {
    data.writeUInt32LE(offset, 2 + index * 4);
    slot.copy(data, offset);
    offset += slot.length;
  });
  return data;
}

test("loudstxt3 parser preserves distinct readings and their best scores", () => {
  const source = makeDictionary([
    makeSlot("ニホン", [
      { surface: "日本", score: -6 },
      { surface: "", score: -1 },
    ]),
    makeSlot("ニッポン", [{ surface: "日本", score: -3 }]),
  ]);
  const entries = new Map();

  parseLoudstxt3(source, "fixture.loudstxt3", entries);

  assert.deepEqual([...entries.get("日本").values()], [
    { reading: "ニホン", score: -6 },
    { reading: "ニッポン", score: -3 },
  ]);
  assert.deepEqual([...entries.get("ニホン").values()], [
    { reading: "ニホン", score: -1 },
  ]);
});

test("bundled dictionary preserves common ambiguous readings", async () => {
  const directory = join(
    import.meta.dirname,
    "..",
    "server-swift",
    "azooKey_dictionary_storage",
    "Dictionary",
    "louds",
  );
  const readingsBySurface = new Map();
  const files = (await readdir(directory)).filter((file) => file.endsWith(".loudstxt3"));
  for (const file of files) {
    parseLoudstxt3(await readFile(join(directory, file)), file, readingsBySurface);
  }

  for (const [surface, expectedReading] of [
    ["橋", "ハシ"],
    ["上手", "ジョウズ"],
    ["行った", "イッタ"],
    ["空く", "アク"],
  ]) {
    assert.ok(
      readingsBySurface.get(surface)?.has(expectedReading),
      `${surface} must preserve ${expectedReading}`,
    );
  }
});

test("reverse dictionary binary stores bounded records and terminal offset", () => {
  const binary = makeBinary([
    ["今日", { reading: "キョウ", score: -2 }],
    ["日本", { reading: "ニホン", score: -3 }],
  ]);

  assert.equal(binary.subarray(0, 4).toString("ascii"), "AZR2");
  assert.equal(binary.readUInt32LE(4), 2);
  assert.equal(binary.readUInt32LE(8 + 2 * 4), binary.length);

  const firstOffset = binary.readUInt32LE(8);
  const surfaceLength = binary.readUInt16LE(firstOffset);
  const readingLength = binary.readUInt16LE(firstOffset + 2);
  assert.equal(binary.subarray(firstOffset + 8, firstOffset + 8 + surfaceLength).toString("utf8"), "今日");
  assert.equal(
    binary
      .subarray(firstOffset + 8 + surfaceLength, firstOffset + 8 + surfaceLength + readingLength)
      .toString("utf8"),
    "キョウ",
  );
});

test("loudstxt3 parser rejects a truncated row table", () => {
  const source = makeDictionary([Buffer.from([1, 0])]);

  assert.throws(
    () => parseLoudstxt3(source, "truncated.loudstxt3", new Map()),
    /truncated row table/,
  );
});
