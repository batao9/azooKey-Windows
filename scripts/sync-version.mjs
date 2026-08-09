import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const repoRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const versionPath = join(repoRoot, "app-version.json");
const generatedRoot = join(repoRoot, "target", "azookey-build");
const versionPattern = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/;

const readJson = (path) => JSON.parse(readFileSync(path, "utf8"));
const jsonText = (value) => `${JSON.stringify(value, null, 2)}\n`;

export const textMatchesIgnoringLineEndings = (actual, expected) =>
  actual.replace(/\r\n?/g, "\n") === expected.replace(/\r\n?/g, "\n");

const runGit = (args, { optional = false } = {}) => {
  try {
    return execFileSync("git", args, {
      cwd: repoRoot,
      encoding: "utf8",
      stdio: ["ignore", "pipe", optional ? "ignore" : "inherit"],
    }).trim();
  } catch (error) {
    if (optional) {
      return "";
    }
    throw error;
  }
};

const assertVersion = (version, label) => {
  if (typeof version !== "string" || !versionPattern.test(version)) {
    throw new Error(`Invalid ${label}: ${String(version)}`);
  }
  return version;
};

export const validateAppVersionConfig = (value) => {
  const version = assertVersion(value?.version, "release version in app-version.json");
  const installerGeneration = value?.installerGeneration;
  if (!Number.isSafeInteger(installerGeneration) || installerGeneration < 1) {
    throw new Error(
      `Invalid installerGeneration in app-version.json: ${String(installerGeneration)}`,
    );
  }
  return { version, installerGeneration };
};

const validateBuildNumber = (value) => {
  const buildNumber = String(value);
  if (!/^(?:0|[1-9]\d*)$/.test(buildNumber)) {
    throw new Error(`Invalid validation build number: ${buildNumber}`);
  }
  return buildNumber;
};

const validateRevision = (value) => {
  const revision = String(value).trim().toLowerCase();
  if (!/^[0-9a-f]{7,64}$/.test(revision)) {
    throw new Error(`Invalid build revision: ${String(value)}`);
  }
  return revision;
};

const createValidationVersion = (baseVersion, buildNumber, revision) => {
  if (baseVersion.includes("-")) {
    return `${baseVersion}.dev.${buildNumber}.g${revision.slice(0, 8)}`;
  }

  const [major, minor, patch] = baseVersion.split(".");
  const nextPatch = (BigInt(patch) + 1n).toString();
  return `${major}.${minor}.${nextPatch}-dev.${buildNumber}.g${revision.slice(0, 8)}`;
};

export const createBuildInfo = ({
  channel,
  releaseVersion,
  baseVersion,
  buildNumber,
  revision,
  branch,
  installerGeneration,
}) => {
  if (channel !== "release" && channel !== "validation") {
    throw new Error(`Invalid build channel: ${String(channel)}`);
  }
  releaseVersion = assertVersion(releaseVersion, "release version");
  baseVersion = assertVersion(baseVersion, "validation base version");
  buildNumber = validateBuildNumber(buildNumber);
  revision = validateRevision(revision);
  if (typeof branch !== "string" || branch.trim() === "") {
    throw new Error(`Invalid build branch: ${String(branch)}`);
  }
  if (!Number.isSafeInteger(installerGeneration) || installerGeneration < 1) {
    throw new Error(`Invalid installer generation: ${String(installerGeneration)}`);
  }

  const version = channel === "release"
    ? releaseVersion
    : createValidationVersion(baseVersion, buildNumber, revision);
  assertVersion(version, "generated build version");

  return {
    version,
    releaseVersion,
    baseVersion: channel === "release" ? releaseVersion : baseVersion,
    channel,
    buildNumber,
    revision,
    branch,
    installerGeneration,
  };
};

const releaseMetadata = ({ version, installerGeneration }) => new Map([
  ["frontend/package.json", (data) => ({ ...data, version })],
  ["frontend/package-lock.json", (data) => ({
    ...data,
    version,
    packages: data.packages?.[""]
      ? {
          ...data.packages,
          "": { ...data.packages[""], version },
        }
      : data.packages,
  })],
  ["frontend/src-tauri/tauri.conf.json", (data) => ({ ...data, version })],
  ["installer/AppVersion.iss", () =>
    `; Generated from app-version.json by scripts/sync-version.mjs --sync-release.\n` +
    `#define MyReleaseVersion "${version}"\n` +
    `#define MyInstallerGeneration ${installerGeneration}\n`],
]);

const expectedReleaseMetadata = (config) => {
  const expected = new Map();
  for (const [relativePath, updater] of releaseMetadata(config)) {
    const path = join(repoRoot, relativePath);
    if (relativePath.endsWith(".json")) {
      expected.set(relativePath, jsonText(updater(readJson(path))));
    } else {
      expected.set(relativePath, updater());
    }
  }
  return expected;
};

const syncReleaseMetadata = (config) => {
  for (const [relativePath, content] of expectedReleaseMetadata(config)) {
    writeFileSync(join(repoRoot, relativePath), content);
  }
  process.stdout.write(`Synchronized release metadata for ${config.version}.\n`);
};

const checkReleaseMetadata = (config) => {
  const mismatches = [];
  for (const [relativePath, expected] of expectedReleaseMetadata(config)) {
    const actual = readFileSync(join(repoRoot, relativePath), "utf8");
    if (!textMatchesIgnoringLineEndings(actual, expected)) {
      mismatches.push(relativePath);
    }
  }
  if (mismatches.length > 0) {
    throw new Error(
      `Release version metadata is out of sync: ${mismatches.join(", ")}. ` +
      "Run node scripts/sync-version.mjs --sync-release.",
    );
  }
  process.stdout.write(`Release metadata is synchronized for ${config.version}.\n`);
};

const exactReleaseTagExists = (releaseVersion) => {
  const expected = `v${releaseVersion}`;
  return runGit(["tag", "--points-at", "HEAD"], { optional: true })
    .split(/\r?\n/)
    .some((tag) => tag === expected);
};

export const selectBuildChannel = ({ requested, exactReleaseTag }) => {
  const channel = requested?.trim().toLowerCase()
    || "validation";
  if (channel !== "release" && channel !== "validation") {
    throw new Error(`AZOOKEY_BUILD_CHANNEL must be release or validation: ${requested}`);
  }
  if (channel === "release" && !exactReleaseTag) {
    throw new Error("Release builds require an exact release tag.");
  }
  return channel;
};

const inferChannel = (releaseVersion) => {
  const requested = process.env.AZOOKEY_BUILD_CHANNEL?.trim().toLowerCase();
  const exactReleaseTag = exactReleaseTagExists(releaseVersion);
  try {
    return selectBuildChannel({ requested, exactReleaseTag });
  } catch (error) {
    if (requested === "release" && !exactReleaseTag) {
      throw new Error(
        `Release builds require HEAD to have the exact tag v${releaseVersion}.`,
      );
    }
    throw error;
  }
};

const inferValidationBaseVersion = (releaseVersion) => {
  const requested = process.env.AZOOKEY_BUILD_BASE_VERSION?.trim();
  if (requested) {
    return assertVersion(requested.replace(/^v/, ""), "AZOOKEY_BUILD_BASE_VERSION");
  }
  const tag = runGit(
    ["describe", "--tags", "--abbrev=0", "--match", "v[0-9]*", "HEAD"],
    { optional: true },
  );
  if (!tag) {
    return releaseVersion;
  }
  return assertVersion(tag.replace(/^v/, ""), "latest reachable release tag");
};

const inferBuildNumber = () => {
  const requested = process.env.AZOOKEY_BUILD_NUMBER?.trim()
    || process.env.GITHUB_RUN_NUMBER?.trim();
  if (requested) {
    return validateBuildNumber(requested);
  }
  const commitTimestamp = runGit(["show", "-s", "--format=%ct", "HEAD"], { optional: true });
  if (!commitTimestamp) {
    throw new Error("Cannot derive a validation build number; set AZOOKEY_BUILD_NUMBER.");
  }
  return validateBuildNumber(commitTimestamp);
};

const inferRevision = () => {
  const requested = process.env.AZOOKEY_BUILD_REVISION?.trim()
    || process.env.GITHUB_SHA?.trim();
  if (requested) {
    return validateRevision(requested);
  }
  const revision = runGit(["rev-parse", "HEAD"], { optional: true });
  if (!revision) {
    throw new Error("Cannot derive a build revision; set AZOOKEY_BUILD_REVISION.");
  }
  return validateRevision(revision);
};

const inferBranch = () => process.env.GITHUB_REF_NAME?.trim()
  || runGit(["branch", "--show-current"], { optional: true })
  || "detached";

const generateBuildMetadata = (config) => {
  const channel = inferChannel(config.version);
  const info = createBuildInfo({
    channel,
    releaseVersion: config.version,
    baseVersion: channel === "release"
      ? config.version
      : inferValidationBaseVersion(config.version),
    buildNumber: inferBuildNumber(),
    revision: inferRevision(),
    branch: inferBranch(),
    installerGeneration: config.installerGeneration,
  });

  mkdirSync(generatedRoot, { recursive: true });
  writeFileSync(join(generatedRoot, "build-info.json"), jsonText(info));
  writeFileSync(
    join(generatedRoot, "tauri.build.conf.json"),
    jsonText({ version: info.version }),
  );
  writeFileSync(
    join(generatedRoot, "AppVersion.iss"),
    "; Generated by scripts/sync-version.mjs.\n" +
    `#define MyAppVersion "${info.version}"\n` +
    `#define MyReleaseVersion "${info.releaseVersion}"\n` +
    `#define MyBuildChannel "${info.channel}"\n` +
    `#define MyBuildNumber "${info.buildNumber}"\n` +
    `#define MyBuildRevision "${info.revision}"\n` +
    `#define MyBuildShortRevision "${info.revision.slice(0, 8)}"\n` +
    `#define MyInstallerGeneration ${info.installerGeneration}\n`,
  );
  process.stdout.write(
    `Generated ${info.channel} build metadata for ${info.version} (${info.revision.slice(0, 8)}).\n`,
  );
};

const main = () => {
  const args = new Set(process.argv.slice(2));
  const supported = new Set(["--sync-release", "--check-release"]);
  for (const arg of args) {
    if (!supported.has(arg)) {
      throw new Error(`Unknown argument: ${arg}`);
    }
  }
  if (args.has("--sync-release") && args.has("--check-release")) {
    throw new Error("--sync-release and --check-release cannot be used together.");
  }

  const config = validateAppVersionConfig(readJson(versionPath));
  if (args.has("--sync-release")) {
    syncReleaseMetadata(config);
  } else if (args.has("--check-release")) {
    checkReleaseMetadata(config);
    return;
  }
  generateBuildMetadata(config);
};

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main();
}
