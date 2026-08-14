import { createHash } from "node:crypto";
import { mkdir, readdir, readFile, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const MAGIC = Buffer.from("AZR2", "ascii");
const RECORD_FIXED_SIZE = 8;

function parseArguments(argv) {
  const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
  let source = join(repoRoot, "server-swift", "azooKey_dictionary_storage", "Dictionary", "louds");
  let output = join(repoRoot, "target", "azookey-reverse-dictionary", "reverse-v2.bin");
  for (let index = 0; index < argv.length; index += 1) {
    if (argv[index] === "--source" && argv[index + 1]) {
      source = resolve(argv[++index]);
    } else if (argv[index] === "--output" && argv[index + 1]) {
      output = resolve(argv[++index]);
    } else {
      throw new Error(`Unknown argument: ${argv[index]}`);
    }
  }
  return { source, output };
}

function readUInt16LE(data, offset, file) {
  if (offset < 0 || offset + 2 > data.length) {
    throw new Error(`${file}: UInt16 offset is out of range: ${offset}`);
  }
  return data.readUInt16LE(offset);
}

function readUInt32LE(data, offset, file) {
  if (offset < 0 || offset + 4 > data.length) {
    throw new Error(`${file}: UInt32 offset is out of range: ${offset}`);
  }
  return data.readUInt32LE(offset);
}

function parseLoudstxt3(data, file, readingsBySurface) {
  const slotCount = readUInt16LE(data, 0, file);
  const headerEnd = 2 + slotCount * 4;
  if (headerEnd > data.length) {
    throw new Error(`${file}: truncated offset table`);
  }

  for (let slot = 0; slot < slotCount; slot += 1) {
    const start = readUInt32LE(data, 2 + slot * 4, file);
    const end = slot === slotCount - 1
      ? data.length
      : readUInt32LE(data, 2 + (slot + 1) * 4, file);
    if (start < headerEnd || end < start + 2 || end > data.length) {
      throw new Error(`${file}: invalid slot range ${start}..<${end}`);
    }

    const rowCount = readUInt16LE(data, start, file);
    if (rowCount === 0) continue;
    const textStart = start + 2 + rowCount * 10;
    if (textStart > end) {
      throw new Error(`${file}: truncated row table`);
    }
    let fields;
    try {
      fields = new TextDecoder("utf-8", { fatal: true })
        .decode(data.subarray(textStart, end))
        .split("\t");
    } catch (error) {
      throw new Error(`${file}: invalid UTF-8 in slot ${slot}`, { cause: error });
    }
    if (fields.length !== rowCount + 1) {
      throw new Error(`${file}: expected ${rowCount + 1} text fields, got ${fields.length}`);
    }
    const reading = fields[0];
    if (!reading || /[\t\r\n]/u.test(reading)) {
      throw new Error(`${file}: empty reading in slot ${slot}`);
    }
    for (let row = 0; row < rowCount; row += 1) {
      const surface = fields[row + 1] || reading;
      const score = data.readFloatLE(start + 2 + row * 10 + 6);
      if (!Number.isFinite(score)) {
        throw new Error(`${file}: invalid score in slot ${slot}, row ${row}`);
      }
      let readings = readingsBySurface.get(surface);
      if (!readings) {
        readings = new Map();
        readingsBySurface.set(surface, readings);
      }
      const previous = readings.get(reading);
      if (!previous || score > previous.score || (score === previous.score && reading < previous.reading)) {
        readings.set(reading, { reading, score });
      }
    }
  }
}

function makeBinary(entries) {
  const records = entries.map(([surface, { reading, score }]) => {
    const surfaceBytes = Buffer.from(surface, "utf8");
    const readingBytes = Buffer.from(reading, "utf8");
    if (surfaceBytes.length > 0xffff || readingBytes.length > 0xffff) {
      throw new Error(`Reverse dictionary record is too long: ${surface}`);
    }
    const record = Buffer.allocUnsafe(RECORD_FIXED_SIZE + surfaceBytes.length + readingBytes.length);
    record.writeUInt16LE(surfaceBytes.length, 0);
    record.writeUInt16LE(readingBytes.length, 2);
    record.writeFloatLE(score, 4);
    surfaceBytes.copy(record, RECORD_FIXED_SIZE);
    readingBytes.copy(record, RECORD_FIXED_SIZE + surfaceBytes.length);
    return record;
  });

  const headerSize = MAGIC.length + 4 + (records.length + 1) * 4;
  const totalSize = headerSize + records.reduce((sum, record) => sum + record.length, 0);
  if (totalSize > 0xffff_ffff) {
    throw new Error(`Reverse dictionary exceeds the v1 offset limit: ${totalSize}`);
  }
  const result = Buffer.allocUnsafe(totalSize);
  MAGIC.copy(result, 0);
  result.writeUInt32LE(records.length, MAGIC.length);
  let offset = headerSize;
  for (let index = 0; index < records.length; index += 1) {
    result.writeUInt32LE(offset, MAGIC.length + 4 + index * 4);
    records[index].copy(result, offset);
    offset += records[index].length;
  }
  result.writeUInt32LE(offset, MAGIC.length + 4 + records.length * 4);
  return result;
}

async function main() {
  const { source, output } = parseArguments(process.argv.slice(2));
  const files = (await readdir(source))
    .filter((name) => name.endsWith(".loudstxt3"))
    .sort();
  if (files.length === 0) {
    throw new Error(`No loudstxt3 dictionary files found under ${source}`);
  }

  const readingsBySurface = new Map();
  const sourceHash = createHash("sha256");
  for (const file of files) {
    const data = await readFile(join(source, file));
    sourceHash.update(file, "utf8");
    sourceHash.update(data);
    parseLoudstxt3(data, file, readingsBySurface);
  }
  const entries = [...readingsBySurface.entries()]
    .flatMap(([surface, readings]) => [...readings.values()].map((entry) => [surface, entry]))
    .sort((left, right) => {
      const surfaceOrder = Buffer.compare(Buffer.from(left[0], "utf8"), Buffer.from(right[0], "utf8"));
      if (surfaceOrder !== 0) return surfaceOrder;
      if (left[1].score !== right[1].score) return right[1].score - left[1].score;
      return Buffer.compare(Buffer.from(left[1].reading, "utf8"), Buffer.from(right[1].reading, "utf8"));
    });
  const binary = makeBinary(entries);
  await mkdir(dirname(output), { recursive: true });
  await writeFile(output, binary);
  process.stdout.write(
    `Generated ${output} (${readingsBySurface.size} surfaces, ${entries.length} readings, ${binary.length} bytes, source_sha256=${sourceHash.digest("hex")})\n`,
  );
}

export { makeBinary, parseLoudstxt3 };

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  await main();
}
