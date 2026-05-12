import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const versionPath = join(repoRoot, "app-version.json");

const versionConfig = JSON.parse(readFileSync(versionPath, "utf8"));
const version = versionConfig.version;

if (typeof version !== "string" || !/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(version)) {
  throw new Error(`Invalid app version in app-version.json: ${String(version)}`);
}

const updateJson = (relativePath, updater) => {
  const path = join(repoRoot, relativePath);
  const data = JSON.parse(readFileSync(path, "utf8"));
  updater(data);
  writeFileSync(path, `${JSON.stringify(data, null, 2)}\n`);
};

updateJson("frontend/package.json", (data) => {
  data.version = version;
});

updateJson("frontend/package-lock.json", (data) => {
  data.version = version;
  if (data.packages?.[""]) {
    data.packages[""].version = version;
  }
});

updateJson("frontend/src-tauri/tauri.conf.json", (data) => {
  data.version = version;
});

writeFileSync(
  join(repoRoot, "installer/AppVersion.iss"),
  `; Generated from app-version.json by scripts/sync-version.mjs.\n#define MyAppVersion "${version}"\n`,
);
