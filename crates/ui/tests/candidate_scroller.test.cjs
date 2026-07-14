const test = require("node:test");
const assert = require("node:assert/strict");

const {
    MAX_RENDERED_ITEM_COUNT,
    clampCandidateIndex,
    clampScrollTop,
    selectionPageScrollTop,
    isSelectionFullyVisible,
    calculateRenderRange,
} = require("../src/candidate_scroller.js");

test("empty candidates produce an empty render range", () => {
    assert.deepEqual(calculateRenderRange(0, 100, 32), {
        start: 0,
        end: 0,
        topSpacerHeight: 0,
        bottomSpacerHeight: 0,
    });
});

test("short candidate lists render every item without spacers", () => {
    assert.deepEqual(calculateRenderRange(4, 0, 32), {
        start: 0,
        end: 4,
        topSpacerHeight: 0,
        bottomSpacerHeight: 0,
    });
});

test("long candidate lists render at most fifteen items near the viewport", () => {
    const range = calculateRenderRange(100, 50 * 32, 32);

    assert.equal(range.end - range.start, MAX_RENDERED_ITEM_COUNT);
    assert.deepEqual(range, {
        start: 45,
        end: 60,
        topSpacerHeight: 45 * 32,
        bottomSpacerHeight: 40 * 32,
    });
});

test("render ranges advance only at five-row page boundaries", () => {
    const beforeBoundary = calculateRenderRange(100, 54.9 * 32, 32);
    const atBoundary = calculateRenderRange(100, 55 * 32, 32);

    assert.deepEqual(beforeBoundary, {
        start: 45,
        end: 60,
        topSpacerHeight: 45 * 32,
        bottomSpacerHeight: 40 * 32,
    });
    assert.deepEqual(atBoundary, {
        start: 50,
        end: 65,
        topSpacerHeight: 50 * 32,
        bottomSpacerHeight: 35 * 32,
    });
});

test("render range is clamped at the beginning and end", () => {
    assert.deepEqual(calculateRenderRange(20, -100, 32), {
        start: 0,
        end: 15,
        topSpacerHeight: 0,
        bottomSpacerHeight: 5 * 32,
    });
    assert.deepEqual(calculateRenderRange(20, Number.POSITIVE_INFINITY, 32), {
        start: 0,
        end: 15,
        topSpacerHeight: 0,
        bottomSpacerHeight: 5 * 32,
    });
    assert.deepEqual(calculateRenderRange(20, 1_000_000, 32), {
        start: 5,
        end: 20,
        topSpacerHeight: 5 * 32,
        bottomSpacerHeight: 0,
    });
});

test("selection indices and scroll positions clamp when candidates shrink", () => {
    assert.equal(clampCandidateIndex(99, 3), 2);
    assert.equal(clampCandidateIndex(-2, 3), 0);
    assert.equal(clampCandidateIndex(Number.NaN, 3), 0);
    assert.equal(clampScrollTop(1_000, 3, 32), 0);
    assert.equal(clampScrollTop(1_000, 8, 32), 3 * 32);
});

test("selection scrolling keeps visible rows and reveals hidden pages", () => {
    assert.equal(isSelectionFullyVisible(6, 5 * 32, 30, 32), true);
    assert.equal(isSelectionFullyVisible(11, 5 * 32, 30, 32), false);
    assert.equal(selectionPageScrollTop(11, 30, 32), 10 * 32);
    assert.equal(selectionPageScrollTop(29, 30, 32), 25 * 32);
});
