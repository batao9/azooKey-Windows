import test from "node:test";
import assert from "node:assert/strict";

import {
    createDebouncedSaver,
    createSerialTaskQueue,
} from "../src/lib/config-save-controller.js";

test("serialized updates preserve changes from different settings", async () => {
    const enqueue = createSerialTaskQueue();
    let config = { profile: "", otherSetting: false };
    let loadCount = 0;
    let releaseFirst;
    const firstGate = new Promise((resolve) => {
        releaseFirst = resolve;
    });

    const update = (mutate, gate = Promise.resolve()) =>
        enqueue(async () => {
            loadCount += 1;
            const next = structuredClone(config);
            mutate(next);
            await gate;
            config = next;
        });

    const first = update((next) => {
        next.profile = "latest profile";
    }, firstGate);
    const second = update((next) => {
        next.otherSetting = true;
    });

    await Promise.resolve();
    assert.equal(loadCount, 1, "the second read must wait for the first write");

    releaseFirst();
    await Promise.all([first, second]);

    assert.deepEqual(config, {
        profile: "latest profile",
        otherSetting: true,
    });
    assert.equal(loadCount, 2);
});

test("a rejected serialized task does not block later updates", async () => {
    const enqueue = createSerialTaskQueue();

    await assert.rejects(
        enqueue(async () => {
            throw new Error("expected failure");
        }),
        /expected failure/,
    );

    assert.equal(await enqueue(async () => "continued"), "continued");
});

test("rapid profile edits are saved once with the latest value", async () => {
    const saved = [];
    const states = [];
    const saver = createDebouncedSaver({
        save: async (value) => {
            saved.push(value);
            return value;
        },
        onStateChange: (state) => states.push(state),
        delayMs: 60_000,
    });

    saver.schedule("p");
    saver.schedule("pr");
    saver.schedule("profile");
    await saver.flush();

    assert.deepEqual(saved, ["profile"]);
    assert.deepEqual(states.slice(-2), ["saving", "saved"]);
});

test("an older failed request cannot overwrite a newer completion state", async () => {
    const completions = new Map();
    const states = [];
    const saver = createDebouncedSaver({
        save: (value) =>
            new Promise((resolve) => {
                completions.set(value, resolve);
            }),
        onStateChange: (state) => states.push(state),
        delayMs: 60_000,
    });

    saver.schedule("old");
    const oldRequest = saver.flush();
    await Promise.resolve();

    saver.schedule("new");
    const newRequest = saver.flush();
    await Promise.resolve();

    completions.get("new")("new");
    await newRequest;
    completions.get("old")(null);
    await oldRequest;

    assert.equal(states.at(-1), "saved");
    assert.deepEqual(
        states.filter((state) => state === "error"),
        [],
    );
});

test("dispose synchronously queues the pending save before a remount load", async () => {
    const enqueue = createSerialTaskQueue();
    let profile = "old";
    const states = [];
    const saver = createDebouncedSaver({
        save: (value) =>
            enqueue(async () => {
                profile = value;
                return value;
            }),
        onStateChange: (state) => states.push(state),
        delayMs: 60_000,
    });

    saver.schedule("new");
    const unmountSave = saver.dispose();
    const remountLoad = enqueue(async () => profile);

    await unmountSave;
    assert.equal(await remountLoad, "new");
    assert.deepEqual(states, ["dirty"]);
});
