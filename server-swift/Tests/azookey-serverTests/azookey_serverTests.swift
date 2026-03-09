import Foundation
import KanaKanjiConverterModule
import Testing
@testable import azookey_server

private func row(_ input: String, _ output: String, _ next: String = "") -> RomajiTableRow {
    RomajiTableRow(input: input, output: output, next_input: next)
}

private func makeTemporaryCustomInputStyle(_ rows: [RomajiTableRow]) throws -> InputStyle {
    let fileURL = FileManager.default.temporaryDirectory
        .appendingPathComponent("azookey-romaji-test-\(UUID().uuidString).tsv")
    let content = try #require(buildCustomRomajiTableContent(rows: rows))
    try content.write(to: fileURL, atomically: true, encoding: .utf8)
    return .mapped(id: .custom(fileURL))
}

private func customInputStyleURL(_ inputStyle: InputStyle) -> URL? {
    guard case .mapped(id: .custom(let url)) = inputStyle else {
        return nil
    }
    return url
}

private func tableMap(_ rows: [RomajiTableRow]) -> [String: String] {
    Dictionary(
        uniqueKeysWithValues: buildCustomRomajiTableEntries(rows: rows).map { ($0.key, $0.value) }
    )
}

@Test func supportsNextInputCarryForTsuRules() async throws {
    let map = tableMap([
        row("tt", "っ", "t"),
        row("ta", "た"),
    ])

    #expect(map["tt"] == "っt")
    #expect(map["tta"] == "った")
}

@Test func keepsWwOverlapRulesStable() async throws {
    let map = tableMap([
        row("ww", "っ", "w"),
        row("www", "w", "ww"),
        row("wa", "わ"),
    ])

    #expect(map["ww"] == "っw")
    #expect(map["www"] == "www")
    #expect(map["っww"] == "www")
    #expect(map["wwa"] == "っわ")
}

@Test func delaysPrefixCommitForNRow() async throws {
    let map = tableMap([
        row("n", "ん"),
        row("na", "な"),
        row("nn", "ん"),
        row("n'", "ん"),
        row("nya", "にゃ"),
        row("-", "ー"),
    ])

    #expect(map["n"] == nil)
    #expect(map["n{composition-separator}"] == "ん")
    #expect(map["n{any-0x00}"] == "ん{any-0x00}")
    #expect(map["ny"] == "ny")
    #expect(map["na"] == "な")
    #expect(map["nn"] == "ん")
    #expect(map["n'"] == "ん")
    #expect(map["n-"] == "んー")
}

@Test func explicitRowsOverrideGeneratedRules() async throws {
    let map = tableMap([
        row("ww", "っ", "w"),
        row("wa", "わ"),
        row("wwa", "ゔぁ"),
    ])

    #expect(map["wwa"] == "ゔぁ")
}

@Test func bracesAreEscapedForInputTableTokens() async throws {
    let map = tableMap([
        row("{a", "}", ""),
    ])

    #expect(map["{lbracket}a"] == "{rbracket}")
}

@Test func zenzaiForcesBuiltinRoman2KanaEvenWithCustomRows() async throws {
    let selection = resolveRomajiInputStyleSelection(
        rows: [row("qa", "くぁ")],
        isZenzaiEnabled: true
    )

    #expect(selection == .roman2kana)
}

@Test func customRowsAreUsedWhenZenzaiIsDisabled() async throws {
    let selection = resolveRomajiInputStyleSelection(
        rows: [row("qa", "くぁ")],
        isZenzaiEnabled: false
    )

    #expect(selection == .custom)
}

@Test func zenzaiCandidateGateRejectsShortInput() async throws {
    let useZenzai = effectiveZenzaiEnabledForCandidates(
        isConfigured: true,
        inputCount: 2,
        hiraganaCount: 1
    )

    #expect(useZenzai == false)
}

@Test func zenzaiCandidateGateAcceptsLongEnoughInput() async throws {
    let useZenzai = effectiveZenzaiEnabledForCandidates(
        isConfigured: true,
        inputCount: 4,
        hiraganaCount: 2
    )

    #expect(useZenzai)
}

@Test func cpuBackendIsDisabledWhenAvxIsUnavailable() async throws {
    let enabled = effectiveZenzaiRuntimeEnabled(
        isConfigured: true,
        backend: "cpu",
        cpuBackendSupported: false
    )

    #expect(enabled == false)
}

@Test func nonCpuBackendRemainsAvailableWithoutCpuAvx() async throws {
    let enabled = effectiveZenzaiRuntimeEnabled(
        isConfigured: true,
        backend: "vulkan",
        cpuBackendSupported: false
    )

    #expect(enabled)
}

@Test func surfaceCountTracksUnderlyingRomanInputLength() async throws {
    let resolved = await MainActor.run {
        var composingText = ComposingText()
        composingText.insertAtCursorPosition("kato", inputStyle: .roman2kana)
        return resolveCandidateComposition(
            composingText: composingText,
            candidateComposingCount: .surfaceCount(1)
        )
    }

    #expect(resolved.correspondingCount == 2)
    #expect(resolved.remainingConvertTarget == "と")
}

@Test func compositeSurfaceCountPreservesClauseOffset() async throws {
    let resolved = await MainActor.run {
        var composingText = ComposingText()
        composingText.insertAtCursorPosition("kato", inputStyle: .roman2kana)
        return resolveCandidateComposition(
            composingText: composingText,
            candidateComposingCount: .composite(lhs: .inputCount(0), rhs: .surfaceCount(1))
        )
    }

    #expect(resolved.correspondingCount == 2)
    #expect(resolved.remainingConvertTarget == "と")
}

@Test func trailingNPreviewFinalizesRoman2KanaOnlyInPreview() async throws {
    let result = await MainActor.run {
        var source = ComposingText()
        source.insertAtCursorPosition("kagen", inputStyle: .roman2kana)
        let preview = makeCandidatePreviewComposingText(from: source)
        return (source: source, preview: preview)
    }

    #expect(result.source.convertTarget == "かげn")
    #expect(result.source.input.count == 5)
    #expect(result.preview.syntheticEndOfText)
    #expect(result.preview.composingText.convertTarget == "かげん")
    #expect(result.preview.composingText.input.count == 6)
}

@Test func trailingNPreviewSkipsDirectInput() async throws {
    let preview = await MainActor.run {
        var source = ComposingText()
        source.insertAtCursorPosition("n", inputStyle: .direct)
        return makeCandidatePreviewComposingText(from: source)
    }

    #expect(preview.syntheticEndOfText == false)
    #expect(preview.composingText.convertTarget == "n")
    #expect(preview.composingText.input.count == 1)
}

@Test func trailingNPreviewKeepsCommittedRomanSequencesUntouched() async throws {
    let preview = await MainActor.run {
        var source = ComposingText()
        source.insertAtCursorPosition("kann", inputStyle: .roman2kana)
        return makeCandidatePreviewComposingText(from: source)
    }

    #expect(preview.syntheticEndOfText == false)
    #expect(preview.composingText.convertTarget == "かん")
}

@Test func trailingNPreviewSupportsCustomRomajiTable() async throws {
    let rows = [
        row("n", "ん"),
        row("na", "な"),
        row("nn", "ん"),
        row("n'", "ん"),
        row("nya", "にゃ"),
        row("-", "ー"),
    ]
    let inputStyle = try makeTemporaryCustomInputStyle(rows)
    let fileURL = try #require(customInputStyleURL(inputStyle))
    defer {
        try? FileManager.default.removeItem(at: fileURL)
    }

    let result = await MainActor.run {
        var source = ComposingText()
        source.insertAtCursorPosition("kagen", inputStyle: inputStyle)
        let preview = makeCandidatePreviewComposingText(from: source)
        return (source: source, preview: preview)
    }

    #expect(result.source.convertTarget == "かげn")
    #expect(result.preview.syntheticEndOfText)
    #expect(result.preview.composingText.convertTarget == "かげん")
}

@Test func trailingNPreviewUsesPreviewSuffixForDisplaySubtext() async throws {
    let resolved = await MainActor.run {
        var source = ComposingText()
        source.insertAtCursorPosition("kagen", inputStyle: .roman2kana)
        let preview = makeCandidatePreviewComposingText(from: source)
        return resolveCandidateCompositionForDisplay(
            originalComposingText: source,
            previewComposingText: preview.composingText,
            candidateComposingCount: .surfaceCount(2)
        )
    }

    #expect(resolved.correspondingCount == 4)
    #expect(resolved.remainingConvertTarget == "ん")
}

@Test func trailingNPreviewForCursorPrefixOnlyAppliesAtCompositionEnd() async throws {
    let result = await MainActor.run {
        var source = ComposingText()
        source.insertAtCursorPosition("kagen", inputStyle: .roman2kana)
        let endPreview = makeCandidatePreviewComposingTextForCursorPrefix(
            prefixComposingText: source.prefixToCursorPosition(),
            suffixAfterCursor: ""
        )

        _ = source.moveCursorFromCursorPosition(count: -1)
        let midPrefix = source.prefixToCursorPosition()
        let midSuffix = String(source.convertTarget.dropFirst(source.convertTargetCursorPosition))
        let midPreview = makeCandidatePreviewComposingTextForCursorPrefix(
            prefixComposingText: midPrefix,
            suffixAfterCursor: midSuffix
        )

        return (endPreview: endPreview, midPreview: midPreview, midSuffix: midSuffix)
    }

    #expect(result.endPreview.syntheticEndOfText)
    #expect(result.endPreview.composingText.convertTarget == "かげん")
    #expect(result.midPreview.syntheticEndOfText == false)
    #expect(result.midPreview.composingText.convertTarget == "かげ")
    #expect(result.midSuffix == "n")
}

@Test func trailingNPreviewDoesNotGeneralizeToOtherDelayedPrefixes() async throws {
    let rows = [
        row("q", "く"),
        row("qa", "くぁ"),
    ]
    let inputStyle = try makeTemporaryCustomInputStyle(rows)
    let fileURL = try #require(customInputStyleURL(inputStyle))
    defer {
        try? FileManager.default.removeItem(at: fileURL)
    }

    let preview = await MainActor.run {
        var source = ComposingText()
        source.insertAtCursorPosition("q", inputStyle: inputStyle)
        return makeCandidatePreviewComposingText(from: source)
    }

    #expect(preview.syntheticEndOfText == false)
    #expect(preview.composingText.convertTarget == "q")
}
