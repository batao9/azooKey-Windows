import assert from "node:assert/strict";
import test from "node:test";

import {
    createBuildInfo,
    selectBuildChannel,
    textMatchesIgnoringLineEndings,
    validateAppVersionConfig,
} from "../../scripts/sync-version.mjs";

const REVISION = "d1525a6123456789abcdef0123456789abcdef01";

test("validation builds use the latest released version as their semver base", () => {
    const info = createBuildInfo({
        channel: "validation",
        releaseVersion: "0.1.0-batao.12",
        baseVersion: "0.1.0-batao.11",
        buildNumber: "1842",
        revision: REVISION,
        branch: "feature/example",
        installerGeneration: 2,
    });

    assert.deepEqual(info, {
        version: "0.1.0-batao.11.dev.1842.gd1525a61",
        releaseVersion: "0.1.0-batao.12",
        baseVersion: "0.1.0-batao.11",
        channel: "validation",
        buildNumber: "1842",
        revision: REVISION,
        branch: "feature/example",
        installerGeneration: 2,
    });
});

test("validation builds increment a stable base patch before adding a prerelease", () => {
    const info = createBuildInfo({
        channel: "validation",
        releaseVersion: "1.1.0",
        baseVersion: "1.0.0",
        buildNumber: "1842",
        revision: REVISION,
        branch: "feature/example",
        installerGeneration: 2,
    });

    assert.equal(info.version, "1.0.1-dev.1842.gd1525a61");
    assert.equal(info.baseVersion, "1.0.0");
});

test("release builds use the declared release version without a suffix", () => {
    const info = createBuildInfo({
        channel: "release",
        releaseVersion: "0.1.0-batao.12",
        baseVersion: "0.1.0-batao.12",
        buildNumber: "1842",
        revision: REVISION,
        branch: "v0.1.0-batao.12",
        installerGeneration: 2,
    });

    assert.equal(info.version, "0.1.0-batao.12");
    assert.equal(info.channel, "release");
});

test("app version config requires an explicit installer generation", () => {
    assert.deepEqual(
        validateAppVersionConfig({
            version: "0.1.0-batao.12",
            installerGeneration: 2,
        }),
        {
            version: "0.1.0-batao.12",
            installerGeneration: 2,
        },
    );

    assert.throws(
        () => validateAppVersionConfig({ version: "0.1.0-batao.12" }),
        /installerGeneration/,
    );
});

test("validation build numbers must be canonical numeric semver identifiers", () => {
    assert.throws(
        () => createBuildInfo({
            channel: "validation",
            releaseVersion: "0.1.0-batao.12",
            baseVersion: "0.1.0-batao.11",
            buildNumber: "01842",
            revision: REVISION,
            branch: "feature/example",
            installerGeneration: 2,
        }),
        /build number/,
    );
});

test("release channel requires an explicit request, exact tag, and tag CI context", () => {
    assert.equal(
        selectBuildChannel({ requested: undefined, exactReleaseTag: false }),
        "validation",
    );
    assert.equal(
        selectBuildChannel({ requested: undefined, exactReleaseTag: true }),
        "validation",
    );
    assert.equal(
        selectBuildChannel({
            requested: "release",
            exactReleaseTag: true,
            trustedReleaseContext: true,
        }),
        "release",
    );
    assert.throws(
        () => selectBuildChannel({ requested: "release", exactReleaseTag: false }),
        /exact release tag/,
    );
    assert.throws(
        () => selectBuildChannel({
            requested: "release",
            exactReleaseTag: true,
            trustedReleaseContext: false,
        }),
        /GitHub Actions tag context/,
    );
});

test("release metadata checks ignore only platform line endings", () => {
    assert.equal(
        textMatchesIgnoringLineEndings("first\r\nsecond\r\n", "first\nsecond\n"),
        true,
    );
    assert.equal(
        textMatchesIgnoringLineEndings("first\r\nchanged\r\n", "first\nsecond\n"),
        false,
    );
});
