(function (root, factory) {
    const api = factory();

    if (typeof module === "object" && module.exports) {
        module.exports = api;
    } else {
        root.CandidateScroller = api;
    }
})(typeof globalThis !== "undefined" ? globalThis : this, function () {
    const VISIBLE_ITEM_COUNT = 5;
    const OVERSCAN_ITEM_COUNT = 5;
    const MAX_RENDERED_ITEM_COUNT = VISIBLE_ITEM_COUNT + OVERSCAN_ITEM_COUNT * 2;

    function clampCandidateIndex(index, candidateCount) {
        if (candidateCount <= 0) {
            return 0;
        }

        const numericIndex = Number(index);
        return Number.isFinite(numericIndex)
            ? Math.min(Math.max(Math.trunc(numericIndex), 0), candidateCount - 1)
            : 0;
    }

    function candidatePageStart(index) {
        return Math.floor(index / VISIBLE_ITEM_COUNT) * VISIBLE_ITEM_COUNT;
    }

    function maxScrollTop(candidateCount, itemHeight) {
        if (candidateCount <= VISIBLE_ITEM_COUNT || itemHeight <= 0) {
            return 0;
        }

        return (candidateCount - VISIBLE_ITEM_COUNT) * itemHeight;
    }

    function clampScrollTop(scrollTop, candidateCount, itemHeight) {
        const numericScrollTop = Number(scrollTop);
        const safeScrollTop = Number.isFinite(numericScrollTop) ? numericScrollTop : 0;
        return Math.min(Math.max(safeScrollTop, 0), maxScrollTop(candidateCount, itemHeight));
    }

    function selectionPageScrollTop(index, candidateCount, itemHeight) {
        if (candidateCount <= 0 || itemHeight <= 0) {
            return 0;
        }

        const safeIndex = clampCandidateIndex(index, candidateCount);
        return clampScrollTop(
            candidatePageStart(safeIndex) * itemHeight,
            candidateCount,
            itemHeight
        );
    }

    function isSelectionFullyVisible(index, scrollTop, candidateCount, itemHeight) {
        if (candidateCount <= 0 || itemHeight <= 0) {
            return false;
        }

        const safeIndex = clampCandidateIndex(index, candidateCount);
        const viewportTop = clampScrollTop(scrollTop, candidateCount, itemHeight);
        const viewportBottom = viewportTop + VISIBLE_ITEM_COUNT * itemHeight;
        const itemTop = safeIndex * itemHeight;
        const itemBottom = itemTop + itemHeight;
        return itemTop >= viewportTop && itemBottom <= viewportBottom;
    }

    function calculateRenderRange(candidateCount, scrollTop, itemHeight) {
        if (candidateCount <= 0 || itemHeight <= 0) {
            return {
                start: 0,
                end: 0,
                topSpacerHeight: 0,
                bottomSpacerHeight: 0,
            };
        }

        const safeScrollTop = clampScrollTop(scrollTop, candidateCount, itemHeight);
        const visibleStart = Math.min(
            Math.floor(safeScrollTop / itemHeight),
            Math.max(candidateCount - 1, 0)
        );
        const visiblePageStart =
            Math.floor(visibleStart / VISIBLE_ITEM_COUNT) * VISIBLE_ITEM_COUNT;
        const renderedItemCount = Math.min(candidateCount, MAX_RENDERED_ITEM_COUNT);
        const maxStart = candidateCount - renderedItemCount;
        const start = Math.min(Math.max(visiblePageStart - OVERSCAN_ITEM_COUNT, 0), maxStart);
        const end = start + renderedItemCount;

        return {
            start,
            end,
            topSpacerHeight: start * itemHeight,
            bottomSpacerHeight: (candidateCount - end) * itemHeight,
        };
    }

    return {
        VISIBLE_ITEM_COUNT,
        OVERSCAN_ITEM_COUNT,
        MAX_RENDERED_ITEM_COUNT,
        clampCandidateIndex,
        clampScrollTop,
        selectionPageScrollTop,
        isSelectionFullyVisible,
        calculateRenderRange,
    };
});
