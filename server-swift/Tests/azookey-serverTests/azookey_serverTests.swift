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

private func serverSwiftRootURL() -> URL {
    URL(filePath: #filePath)
        .deletingLastPathComponent()
        .deletingLastPathComponent()
        .deletingLastPathComponent()
}

private struct TestClausePayload: Equatable {
    let text: String
    let rawHiragana: String
    let correspondingCount: Int
}

private func makeTestConvertOptions() -> ConvertRequestOptions {
    let root = serverSwiftRootURL()
    let dictionaryURL = root
        .appendingPathComponent("azooKey_dictionary_storage")
        .appendingPathComponent("Dictionary")
    let emojiURL = root
        .appendingPathComponent("azooKey_emoji_dictionary_storage")
        .appendingPathComponent("EmojiDictionary")
        .appendingPathComponent("emoji_all_E15.1.txt")
    let tempRoot = FileManager.default.temporaryDirectory
        .appendingPathComponent("azookey-server-tests")

    return ConvertRequestOptions(
        requireJapanesePrediction: true,
        requireEnglishPrediction: false,
        keyboardLanguage: .ja_JP,
        learningType: .nothing,
        dictionaryResourceURL: dictionaryURL,
        memoryDirectoryURL: tempRoot,
        sharedContainerURL: tempRoot,
        textReplacer: .init {
            emojiURL
        },
        zenzaiMode: .off,
        preloadDictionary: true,
        metadata: .init(versionString: "azookey-server-tests")
    )
}

@MainActor private func debugRealClausePayloads(
    hiragana: String
) throws -> [TestClausePayload] {
    var source = ComposingText()
    source.insertAtCursorPosition(hiragana, inputStyle: .direct)
    let preview = makeCandidatePreviewComposingText(from: source)
    let converted = converter.requestCandidates(
        preview.composingText,
        options: makeTestConvertOptions()
    )
    let candidate = try #require(converted.mainResults.first)
    return debugClausePayloads(
        candidate: candidate,
        originalComposingText: source
    )
    .map {
        TestClausePayload(
            text: $0.text,
            rawHiragana: $0.rawHiragana,
            correspondingCount: $0.correspondingCount
        )
    }
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

@Test func customRowsAreUsedWhenZenzaiIsEnabled() async throws {
    let selection = resolveRomajiInputStyleSelection(
        rows: [row("qa", "くぁ")]
    )

    #expect(selection == .custom)
}

@Test func customRowsAreUsedWhenZenzaiIsDisabled() async throws {
    let selection = resolveRomajiInputStyleSelection(
        rows: [row("qa", "くぁ")]
    )

    #expect(selection == .custom)
}

@Test func builtinRoman2KanaIsUsedWhenCustomRowsAreMissing() async throws {
    let selection = resolveRomajiInputStyleSelection(rows: nil)

    #expect(selection == .roman2kana)
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

@Test func deleteBackwardDropsWholeRomanChunkForConvertedKana() async throws {
    let result = await MainActor.run {
        var composingText = ComposingText()
        composingText.insertAtCursorPosition("aru", inputStyle: .roman2kana)
        composingText.deleteBackwardFromCursorPosition(count: 1)
        return (
            convertTarget: composingText.convertTarget,
            inputCount: composingText.input.count
        )
    }

    #expect(result.convertTarget == "あ")
    #expect(result.inputCount == 1)
}

@Test func clausePayloadsPreserveRomanInputCountsPerClause() async throws {
    let clauses = await MainActor.run {
        var composingText = ComposingText()
        composingText.insertAtCursorPosition("katoujunnichi", inputStyle: .roman2kana)
        let candidate = Candidate(
            text: "加藤純一",
            value: 0,
            composingCount: .surfaceCount(7),
            lastMid: 0,
            data: [
                DicdataElement(word: "加藤", ruby: "かとう", cid: 1, mid: 0, value: 0),
                DicdataElement(word: "純一", ruby: "じゅんいち", cid: 1, mid: 0, value: 0),
            ]
        )
        return debugClausePayloads(
            candidate: candidate,
            originalComposingText: composingText
        )
    }

    #expect(clauses.count == 2)
    #expect(clauses[0].text == "加藤")
    #expect(clauses[0].rawHiragana == "かとう")
    #expect(clauses[0].correspondingCount == 5)
    #expect(clauses[1].text == "純一")
    #expect(clauses[1].rawHiragana == "じゅんいち")
    #expect(clauses[1].correspondingCount == 8)
}

@Test func clausePayloadsKeepMultipleBootstrapClauses() async throws {
    let clauses = await MainActor.run {
        var composingText = ComposingText()
        composingText.insertAtCursorPosition("aaabbbcccdddeee", inputStyle: .direct)
        let candidate = Candidate(
            text: "文節モード見選択時の左右キー操作を初回だけ自動で設定しそのまま分節移動できるようにしました",
            value: 0,
            composingCount: .surfaceCount(15),
            lastMid: 0,
            data: [
                DicdataElement(word: "文節", ruby: "aaa", cid: 1, mid: 0, value: 0),
                DicdataElement(word: "モード", ruby: "bbb", cid: 1, mid: 0, value: 0),
                DicdataElement(word: "見選択時の", ruby: "ccc", cid: 1, mid: 0, value: 0),
                DicdataElement(word: "左右キー操作を", ruby: "ddd", cid: 1, mid: 0, value: 0),
                DicdataElement(
                    word: "初回だけ自動で設定しそのまま分節移動できるようにしました",
                    ruby: "eee",
                    cid: 1,
                    mid: 0,
                    value: 0
                ),
            ]
        )
        return debugClausePayloads(
            candidate: candidate,
            originalComposingText: composingText
        )
    }

    #expect(clauses.map(\.text) == [
        "文節",
        "モード",
        "見選択時の",
        "左右キー操作を",
        "初回だけ自動で設定しそのまま分節移動できるようにしました",
    ])
    #expect(clauses.map(\.rawHiragana) == ["aaa", "bbb", "ccc", "ddd", "eee"])
    #expect(clauses.map(\.correspondingCount) == [3, 3, 3, 3, 3])
}

@Test func clausePayloadsNormalizeUiFriendlyMultiClauses() async throws {
    let clauses = await MainActor.run {
        let coreCID = 561
        let postCID = 54
        var composingText = ComposingText()
        composingText.insertAtCursorPosition(
            "aabbbccdddeeffghhhijjkkllmm",
            inputStyle: .direct
        )
        let candidate = Candidate(
            text: "ある程度長い文章でも最大2文節にしか分割されない",
            value: 0,
            composingCount: .surfaceCount(27),
            lastMid: 0,
            data: [
                DicdataElement(word: "ある", ruby: "aa", cid: coreCID, mid: 0, value: 0),
                DicdataElement(word: "程度", ruby: "bbb", cid: coreCID, mid: 0, value: 0),
                DicdataElement(word: "長い", ruby: "cc", cid: coreCID, mid: 0, value: 0),
                DicdataElement(word: "文章", ruby: "ddd", cid: coreCID, mid: 0, value: 0),
                DicdataElement(word: "でも", ruby: "ee", cid: postCID, mid: 0, value: 0),
                DicdataElement(word: "最大", ruby: "ff", cid: coreCID, mid: 0, value: 0),
                DicdataElement(word: "2", ruby: "g", cid: coreCID, mid: 0, value: 0),
                DicdataElement(word: "文節", ruby: "hhh", cid: coreCID, mid: 0, value: 0),
                DicdataElement(word: "に", ruby: "i", cid: postCID, mid: 0, value: 0),
                DicdataElement(word: "しか", ruby: "jj", cid: postCID, mid: 0, value: 0),
                DicdataElement(word: "分割", ruby: "kk", cid: coreCID, mid: 0, value: 0),
                DicdataElement(word: "され", ruby: "ll", cid: coreCID, mid: 0, value: 0),
                DicdataElement(word: "ない", ruby: "mm", cid: postCID, mid: 0, value: 0),
            ]
        )
        return debugClausePayloads(
            candidate: candidate,
            originalComposingText: composingText
        )
    }

    #expect(clauses.map(\.text) == [
        "ある程度",
        "長い文章でも",
        "最大2文節にしか",
        "分割されない",
    ])
    #expect(clauses.map(\.rawHiragana) == [
        "aabbb",
        "ccdddee",
        "ffghhhijj",
        "kkllmm",
    ])
    #expect(clauses.map(\.correspondingCount) == [5, 7, 9, 6])
}

@Test func clausePayloadNormalizationIsDeterministic() async throws {
    let first = await MainActor.run {
        let coreCID = 561
        let postCID = 54
        var composingText = ComposingText()
        composingText.insertAtCursorPosition(
            "aabbbccdddeeffghhhijjkkllmm",
            inputStyle: .direct
        )
        let candidate = Candidate(
            text: "ある程度長い文章でも最大2文節にしか分割されない",
            value: 0,
            composingCount: .surfaceCount(27),
            lastMid: 0,
            data: [
                DicdataElement(word: "ある", ruby: "aa", cid: coreCID, mid: 0, value: 0),
                DicdataElement(word: "程度", ruby: "bbb", cid: coreCID, mid: 0, value: 0),
                DicdataElement(word: "長い", ruby: "cc", cid: coreCID, mid: 0, value: 0),
                DicdataElement(word: "文章", ruby: "ddd", cid: coreCID, mid: 0, value: 0),
                DicdataElement(word: "でも", ruby: "ee", cid: postCID, mid: 0, value: 0),
                DicdataElement(word: "最大", ruby: "ff", cid: coreCID, mid: 0, value: 0),
                DicdataElement(word: "2", ruby: "g", cid: coreCID, mid: 0, value: 0),
                DicdataElement(word: "文節", ruby: "hhh", cid: coreCID, mid: 0, value: 0),
                DicdataElement(word: "に", ruby: "i", cid: postCID, mid: 0, value: 0),
                DicdataElement(word: "しか", ruby: "jj", cid: postCID, mid: 0, value: 0),
                DicdataElement(word: "分割", ruby: "kk", cid: coreCID, mid: 0, value: 0),
                DicdataElement(word: "され", ruby: "ll", cid: coreCID, mid: 0, value: 0),
                DicdataElement(word: "ない", ruby: "mm", cid: postCID, mid: 0, value: 0),
            ]
        )
        return debugClausePayloads(
            candidate: candidate,
            originalComposingText: composingText
        )
    }

    let second = await MainActor.run {
        let coreCID = 561
        let postCID = 54
        var composingText = ComposingText()
        composingText.insertAtCursorPosition(
            "aabbbccdddeeffghhhijjkkllmm",
            inputStyle: .direct
        )
        let candidate = Candidate(
            text: "ある程度長い文章でも最大2文節にしか分割されない",
            value: 0,
            composingCount: .surfaceCount(27),
            lastMid: 0,
            data: [
                DicdataElement(word: "ある", ruby: "aa", cid: coreCID, mid: 0, value: 0),
                DicdataElement(word: "程度", ruby: "bbb", cid: coreCID, mid: 0, value: 0),
                DicdataElement(word: "長い", ruby: "cc", cid: coreCID, mid: 0, value: 0),
                DicdataElement(word: "文章", ruby: "ddd", cid: coreCID, mid: 0, value: 0),
                DicdataElement(word: "でも", ruby: "ee", cid: postCID, mid: 0, value: 0),
                DicdataElement(word: "最大", ruby: "ff", cid: coreCID, mid: 0, value: 0),
                DicdataElement(word: "2", ruby: "g", cid: coreCID, mid: 0, value: 0),
                DicdataElement(word: "文節", ruby: "hhh", cid: coreCID, mid: 0, value: 0),
                DicdataElement(word: "に", ruby: "i", cid: postCID, mid: 0, value: 0),
                DicdataElement(word: "しか", ruby: "jj", cid: postCID, mid: 0, value: 0),
                DicdataElement(word: "分割", ruby: "kk", cid: coreCID, mid: 0, value: 0),
                DicdataElement(word: "され", ruby: "ll", cid: coreCID, mid: 0, value: 0),
                DicdataElement(word: "ない", ruby: "mm", cid: postCID, mid: 0, value: 0),
            ]
        )
        return debugClausePayloads(
            candidate: candidate,
            originalComposingText: composingText
        )
    }

    #expect(first.map(\.text) == second.map(\.text))
    #expect(first.map(\.rawHiragana) == second.map(\.rawHiragana))
    #expect(first.map(\.correspondingCount) == second.map(\.correspondingCount))
}

@Test func realLongSentenceProducesMoreThanTwoClauses() async throws {
    let reading = "あるていどながいぶんしょうでもさいだい2ぶんせつにしかぶんかつされない"
    let clauses = try await MainActor.run {
        try debugRealClausePayloads(hiragana: reading)
    }

    #expect(clauses.count > 2)
    #expect(clauses.map(\.correspondingCount).reduce(0, +) == reading.count)
    #expect(clauses.map(\.text).joined() != reading)
}

@Test func realLongSentenceClausePayloadsStayDeterministic() async throws {
    let reading = "あるていどながいぶんしょうでもさいだい2ぶんせつにしかぶんかつされない"
    let first = try await MainActor.run {
        try debugRealClausePayloads(hiragana: reading)
    }
    let second = try await MainActor.run {
        try debugRealClausePayloads(hiragana: reading)
    }

    #expect(first.map(\.text) == second.map(\.text))
    #expect(first.map(\.rawHiragana) == second.map(\.rawHiragana))
    #expect(first.map(\.correspondingCount) == second.map(\.correspondingCount))
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
        row("ka", "か"),
        row("ge", "げ"),
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
