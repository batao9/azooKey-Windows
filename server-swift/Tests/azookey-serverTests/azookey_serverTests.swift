import Foundation
import ffi
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
    defer {
        try? FileManager.default.removeItem(at: fileURL)
    }
    let tableName = "azookey-windows-test-romaji-\(UUID().uuidString)"
    let table = try InputStyleManager.loadTable(from: fileURL)
    InputStyleManager.registerInputStyle(table: table, for: tableName)
    return .mapped(id: .tableName(tableName))
}

private func tableMap(_ rows: [RomajiTableRow]) -> [String: String] {
    Dictionary(
        uniqueKeysWithValues: buildCustomRomajiTableEntries(rows: rows).map { ($0.key, $0.value) }
    )
}

private func packageRootURL() -> URL {
    URL(filePath: #filePath)
        .deletingLastPathComponent()
        .deletingLastPathComponent()
        .deletingLastPathComponent()
}

@Test func engineRuntimeDirectoryUsesAppData() {
    let directory = engineRuntimeDirectoryURL(
        appDataPath: #"C:\Users\test\AppData\Roaming"#,
        temporaryDirectoryURL: URL(filePath: #"C:\Users\test\AppData\Local\Temp"#)
    )

    #expect(directory.lastPathComponent == "EngineRuntime")
    #expect(directory.deletingLastPathComponent().lastPathComponent == "Azookey")
    #expect(directory.deletingLastPathComponent().deletingLastPathComponent().lastPathComponent == "Roaming")
}

@Test func engineRuntimeDirectoryFallsBackOutsideInstallDirectory() {
    let installDirectory = URL(filePath: #"C:\Program Files\Azookey"#)
    let directory = engineRuntimeDirectoryURL(
        appDataPath: "  ",
        temporaryDirectoryURL: URL(filePath: #"C:\Users\test\AppData\Local\Temp"#)
    )

    #expect(directory.lastPathComponent == "EngineRuntime")
    #expect(directory.deletingLastPathComponent().deletingLastPathComponent().lastPathComponent == "Temp")
    #expect(directory.path != installDirectory.path)
    #expect(!directory.path.hasPrefix(installDirectory.path + "/"))
}

private func testConvertRequestOptions(memoryURL: URL) -> ConvertRequestOptions {
    let packageRoot = packageRootURL()
    return ConvertRequestOptions(
        requireJapanesePrediction: .disabled,
        requireEnglishPrediction: .disabled,
        keyboardLanguage: .ja_JP,
        learningType: .nothing,
        memoryDirectoryURL: memoryURL,
        sharedContainerURL: memoryURL,
        textReplacer: .init {
            packageRoot
                .appending(path: "azooKey_emoji_dictionary_storage")
                .appending(path: "EmojiDictionary")
                .appending(path: "emoji_all_E15.1.txt")
        },
        specialCandidateProviders: nil,
        zenzaiMode: .off,
        metadata: .init(versionString: "Azookey for Windows test")
    )
}

private func testLearningConvertRequestOptions(memoryURL: URL) -> ConvertRequestOptions {
    let packageRoot = packageRootURL()
    return ConvertRequestOptions(
        requireJapanesePrediction: .disabled,
        requireEnglishPrediction: .disabled,
        keyboardLanguage: .ja_JP,
        learningType: .inputAndOutput,
        memoryDirectoryURL: memoryURL,
        sharedContainerURL: memoryURL,
        textReplacer: .init {
            packageRoot
                .appending(path: "azooKey_emoji_dictionary_storage")
                .appending(path: "EmojiDictionary")
                .appending(path: "emoji_all_E15.1.txt")
        },
        specialCandidateProviders: nil,
        zenzaiMode: .off,
        metadata: .init(versionString: "Azookey for Windows learning test")
    )
}

private func testCandidate(
    word: String,
    ruby: String,
    composingCount: ComposingCount
) -> Candidate {
    Candidate(
        text: word,
        value: -1,
        composingCount: composingCount,
        lastMid: MIDData.一般.mid,
        data: [
            DicdataElement(
                word: word,
                ruby: ruby,
                cid: CIDData.一般名詞.cid,
                mid: MIDData.一般.mid,
                value: -1
            )
        ]
    )
}

@Test func learningCandidateCanOnlyBeConsumedOnce() async throws {
    let candidate = testCandidate(
        word: "今日",
        ruby: "きょう",
        composingCount: .inputCount(3)
    )

    await MainActor.run {
        let previousLearningType = currentLearningType
        currentLearningType = .inputAndOutput
        learningCandidateCache.removeAll()
        let batchFirstId = cacheLearningCandidates([candidate])
        let candidateId = learningCandidateId(at: 0, batchFirstId: batchFirstId)

        #expect(candidateId != 0)
        #expect(consumeLearningCandidate(candidateId) != nil)
        #expect(consumeLearningCandidate(candidateId) == nil)

        learningCandidateCache.removeAll()
        currentLearningType = previousLearningType
    }
}

@Test func learningCandidateIdsUseDenseSequentialSlots() async throws {
    let first = testCandidate(
        word: "今日",
        ruby: "きょう",
        composingCount: .inputCount(3)
    )
    let second = testCandidate(
        word: "教",
        ruby: "きょう",
        composingCount: .inputCount(3)
    )

    await MainActor.run {
        let previousLearningType = currentLearningType
        currentLearningType = .inputAndOutput
        learningCandidateCache.removeAll()

        let batchFirstId = cacheLearningCandidates([first, second])
        let firstId = learningCandidateId(at: 0, batchFirstId: batchFirstId)
        let secondId = learningCandidateId(at: 1, batchFirstId: batchFirstId)

        #expect(firstId != 0)
        #expect(secondId == firstId + 1)
        #expect(learningCandidateCache.slotCount == 2)

        learningCandidateCache.removeAll()
        currentLearningType = previousLearningType
    }
}

@Test func learningCandidateIdsRemainValidAcrossLaterBatches() {
    var cache = LearningCandidateCache()
    let first = testCandidate(
        word: "今日",
        ruby: "きょう",
        composingCount: .inputCount(3)
    )
    let second = testCandidate(
        word: "教",
        ruby: "きょう",
        composingCount: .inputCount(3)
    )

    let firstBatchId = cache.appendBatch([first, second])!
    let firstId = cache.candidateId(at: 0, batchFirstId: firstBatchId)
    let firstBatchLastId = cache.candidateId(at: 1, batchFirstId: firstBatchId)
    let secondBatchId = cache.appendBatch([second])!
    let secondId = cache.candidateId(at: 0, batchFirstId: secondBatchId)

    #expect(firstId != secondId)
    #expect(firstBatchLastId + 1 == secondId)
    #expect(cache.batchCount == 2)
    #expect(cache.consume(firstId)?.text == "今日")
    #expect(cache.consume(firstBatchLastId)?.text == "教")
    #expect(cache.consume(secondId)?.text == "教")
}

@Test func clearingLearningCandidateCacheInvalidatesAllBatches() {
    var cache = LearningCandidateCache()
    let candidate = testCandidate(
        word: "今日",
        ruby: "きょう",
        composingCount: .inputCount(3)
    )
    let batchFirstId = cache.appendBatch([candidate])!
    let oldCandidateId = cache.candidateId(at: 0, batchFirstId: batchFirstId)

    cache.removeAll()
    let newBatchFirstId = cache.appendBatch([candidate])!
    let newCandidateId = cache.candidateId(at: 0, batchFirstId: newBatchFirstId)

    #expect(oldCandidateId != newCandidateId)
    #expect(cache.consume(oldCandidateId) == nil)
    #expect(cache.consume(newCandidateId)?.text == "今日")
    #expect(cache.batchCount == 1)
}

@Test func emptyLearningCandidateBatchDoesNotAllocateIds() {
    var cache = LearningCandidateCache()

    #expect(cache.appendBatch([]) == nil)
    #expect(cache.batchCount == 0)
    #expect(cache.slotCount == 0)
}

@Test func learningCandidateCacheIsSkippedWhenLearningDoesNotAcceptInput() async throws {
    let candidate = testCandidate(
        word: "今日",
        ruby: "きょう",
        composingCount: .inputCount(3)
    )

    await MainActor.run {
        let previousLearningType = currentLearningType
        learningCandidateCache.removeAll()

        currentLearningType = .onlyOutput
        #expect(cacheLearningCandidates([candidate]) == nil)
        #expect(learningCandidateCache.slotCount == 0)

        currentLearningType = .nothing
        #expect(cacheLearningCandidates([candidate]) == nil)
        #expect(learningCandidateCache.slotCount == 0)

        learningCandidateCache.removeAll()
        currentLearningType = previousLearningType
    }
}

@Test func latestLearningSelectionIsPrioritizedOnlyForItsReading() async {
    let katouSugar = testCandidate(
        word: "果糖",
        ruby: "かとう",
        composingCount: .inputCount(3)
    )
    let today = testCandidate(
        word: "今日",
        ruby: "きょう",
        composingCount: .inputCount(3)
    )
    let katouSurname = testCandidate(
        word: "加藤",
        ruby: "かとう",
        composingCount: .inputCount(3)
    )
    let candidates = [katouSugar, today, katouSurname]

    await MainActor.run {
        let previousLearningType = currentLearningType
        let previousLearningSelectionOverrides = learningSelectionOverrides
        defer {
            currentLearningType = previousLearningType
            learningSelectionOverrides = previousLearningSelectionOverrides
        }

        currentLearningType = .inputAndOutput
        learningSelectionOverrides = ["カトウ": "加藤"]
        let prioritized = prioritizeLearningSelectionOverrides(candidates, ruby: "かとう") { $0 }
        #expect(prioritized.map(\.text) == ["加藤", "今日", "果糖"])

        var unrelatedCandidateAccessCount = 0
        let unrelated = prioritizeLearningSelectionOverrides(candidates, ruby: "しゅう") {
            unrelatedCandidateAccessCount += 1
            return $0
        }
        #expect(unrelated.map(\.text) == ["果糖", "今日", "加藤"])
        #expect(unrelatedCandidateAccessCount == 0)

        currentLearningType = .nothing
        let learningDisabled = prioritizeLearningSelectionOverrides(candidates, ruby: "かとう") { $0 }
        #expect(learningDisabled.map(\.text) == ["果糖", "今日", "加藤"])
    }
}

@Test func latestSelectionReplacesPreviouslyLearnedFirstCandidate() async throws {
    let packageRoot = packageRootURL()
    let dictionaryURL = packageRoot
        .appending(path: "azooKey_dictionary_storage")
        .appending(path: "Dictionary")
    let memoryURL = FileManager.default.temporaryDirectory
        .appending(path: "azookey-server-learning-test-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: memoryURL, withIntermediateDirectories: true)
    defer {
        try? FileManager.default.removeItem(at: memoryURL)
    }

    try await MainActor.run {
        let previousConverter = converter
        let previousSupplementConverter = normalNBestSupplementConverter
        let previousComposingText = composingText
        let previousComposingTextSnapshots = composingTextSnapshots
        let previousLearningType = currentLearningType
        let previousLearningMemoryDirectoryURL = currentLearningMemoryDirectoryURL
        let previousLearningCandidateCache = learningCandidateCache
        let previousLearningSelectionOverrides = learningSelectionOverrides
        defer {
            converter = previousConverter
            normalNBestSupplementConverter = previousSupplementConverter
            composingText = previousComposingText
            composingTextSnapshots = previousComposingTextSnapshots
            currentLearningType = previousLearningType
            currentLearningMemoryDirectoryURL = previousLearningMemoryDirectoryURL
            learningCandidateCache = previousLearningCandidateCache
            learningSelectionOverrides = previousLearningSelectionOverrides
        }

        var source = ComposingText()
        source.insertAtCursorPosition("かとう", inputStyle: .direct)
        let options = testLearningConvertRequestOptions(memoryURL: memoryURL)
        converter = KanaKanjiConverter(dictionaryURL: dictionaryURL, preloadDictionary: true)
        normalNBestSupplementConverter = KanaKanjiConverter(
            dictionaryURL: dictionaryURL,
            preloadDictionary: false
        )
        currentLearningType = .inputAndOutput
        currentLearningMemoryDirectoryURL = memoryURL
        learningCandidateCache.removeAll()
        learningSelectionOverrides.removeAll()

        @MainActor func candidates() -> [Candidate] {
            prioritizeLearningSelectionOverrides(
                converter.requestCandidates(source, options: options).mainResults,
                ruby: source.convertTarget
            ) { $0 }
        }

        @MainActor func commit(_ candidate: Candidate, from candidates: [Candidate]) throws {
            let index = try #require(candidates.firstIndex { $0.text == candidate.text })
            let batchFirstId = try #require(cacheLearningCandidates(candidates))
            let candidateId = learningCandidateId(at: index, batchFirstId: batchFirstId)
            #expect(commit_learning_candidate(candidateId: candidateId, commitKind: 1))
            clear_text()
        }

        let initial = candidates()
        let katouSugar = try #require(initial.first { $0.text == "果糖" })
        try commit(katouSugar, from: initial)

        let afterFirstCommit = candidates()
        #expect(afterFirstCommit.first?.text == "果糖")
        let katouSurname = try #require(afterFirstCommit.first { $0.text == "加藤" })
        try commit(katouSurname, from: afterFirstCommit)

        learningSelectionOverrides.removeAll()
        loadLearningSelectionOverrides()
        let afterSecondCommit = candidates()
        #expect(
            afterSecondCommit.first?.text == "加藤",
            "first candidates: \(afterSecondCommit.prefix(5).map(\.text))"
        )
    }
}

@Test func ffiFreeCStringAcceptsNullAndAllocatedStrings() async throws {
    free_c_string(nil)

    let text = try #require(_strdup("azookey"))
    free_c_string(text)
}

@Test func cursorOffsetsUseFullInt32RangeWithoutSnapshotCollisions() async {
    await MainActor.run {
        let previousComposingText = composingText
        let previousComposingTextSnapshots = composingTextSnapshots
        defer {
            composingText = previousComposingText
            composingTextSnapshots = previousComposingTextSnapshots
        }

        let inputLength = 2048
        let input = String(repeating: "あ", count: inputLength)
        for offset in [125, 126, 127, 128, 129, 1024] {
            composingText = ComposingText()
            composingText.insertAtCursorPosition(input, inputStyle: .direct)
            composingTextSnapshots.removeAll()

            var cursor: CInt = 0
            free_c_string(move_cursor(offset: -CInt(inputLength), cursorPtr: &cursor))
            #expect(cursor == -CInt(inputLength))
            #expect(composingText.convertTargetCursorPosition == 0)
            free_c_string(move_cursor(offset: CInt(offset), cursorPtr: &cursor))
            #expect(cursor == CInt(offset))
            #expect(composingText.convertTargetCursorPosition == offset)
            #expect(composingTextSnapshots.isEmpty)
        }

        composingText = ComposingText()
        composingText.insertAtCursorPosition(input, inputStyle: .direct)
        var extremeCursor: CInt = 0
        free_c_string(move_cursor(offset: CInt.min, cursorPtr: &extremeCursor))
        #expect(extremeCursor == -CInt(inputLength))
        #expect(composingText.convertTargetCursorPosition == 0)
        free_c_string(move_cursor(offset: CInt.max, cursorPtr: &extremeCursor))
        #expect(extremeCursor == CInt(inputLength))
        #expect(composingText.convertTargetCursorPosition == inputLength)

        composingText = ComposingText()
        composingText.insertAtCursorPosition(input, inputStyle: .direct)
        composingTextSnapshots.removeAll()
        push_composing_text_snapshot()
        #expect(composingTextSnapshots.count == 1)

        var cursor: CInt = 0
        free_c_string(move_cursor(offset: -1024, cursorPtr: &cursor))
        free_c_string(move_cursor(offset: 125, cursorPtr: &cursor))
        #expect(cursor == 125)
        #expect(composingText.convertTargetCursorPosition == 1149)
        #expect(composingTextSnapshots.count == 1)

        pop_composing_text_snapshot()
        #expect(composingText.convertTargetCursorPosition == inputLength)
        #expect(composingTextSnapshots.isEmpty)

        push_composing_text_snapshot()
        clear_composing_text_snapshots()
        #expect(composingTextSnapshots.isEmpty)
    }
}

@Test func shrinkTextSupportsLongOffsetsAndClampsDirectFfiInput() async {
    await MainActor.run {
        let previousComposingText = composingText
        defer {
            composingText = previousComposingText
        }

        let inputLength = 1100
        let input = String(repeating: "あ", count: inputLength)
        for offset in [127, 128, 129, 1024] {
            composingText = ComposingText()
            composingText.insertAtCursorPosition(input, inputStyle: .direct)

            let result = shrink_text(offset: CInt(offset))
            let remaining = String(cString: result)
            free_c_string(result)

            #expect(remaining.count == inputLength - offset)
            #expect(composingText.input.count == inputLength - offset)
        }

        composingText = ComposingText()
        composingText.insertAtCursorPosition(input, inputStyle: .direct)
        let negativeResult = shrink_text(offset: -1)
        let unchanged = String(cString: negativeResult)
        free_c_string(negativeResult)
        #expect(unchanged.count == inputLength)
        #expect(composingText.input.count == inputLength)
    }
}

@Test func ffiFreeCandidateListAcceptsNullEmptyAndPopulatedLists() async throws {
    free_candidate_list(nil, 0)

    let emptyList = to_list_pointer([])
    free_candidate_list(emptyList, 0)

    let text = try #require(_strdup("candidate"))
    let subtext = try #require(_strdup("remaining"))
    let hiragana = try #require(_strdup("かな"))
    let candidates = [
        FFICandidate(
            text: text,
            subtext: subtext,
            hiragana: hiragana,
            correspondingCount: 1,
            candidateId: 1
        )
    ]

    free_candidate_list(to_list_pointer(candidates), Int32(candidates.count))

    let nilHiraganaText = try #require(_strdup("candidate"))
    let nilHiraganaSubtext = try #require(_strdup("remaining"))
    let nilHiraganaCandidates = [
        FFICandidate(
            text: nilHiraganaText,
            subtext: nilHiraganaSubtext,
            hiragana: nil,
            correspondingCount: 1,
            candidateId: 2
        )
    ]

    free_candidate_list(to_list_pointer(nilHiraganaCandidates), Int32(nilHiraganaCandidates.count))

    let firstLegacyText = try #require(_strdup("legacy"))
    let firstLegacySubtext = try #require(_strdup("remaining"))
    let firstLegacyHiragana = try #require(_strdup("かな"))
    let firstLegacyCandidate = UnsafeMutablePointer<FFICandidate>.allocate(capacity: 1)
    firstLegacyCandidate.initialize(
        to: FFICandidate(
            text: firstLegacyText,
            subtext: firstLegacySubtext,
            hiragana: firstLegacyHiragana,
            correspondingCount: 1,
            candidateId: 3
        )
    )

    let secondLegacyText = try #require(_strdup("legacy-second"))
    let secondLegacySubtext = try #require(_strdup("remaining-second"))
    let secondLegacyCandidate = UnsafeMutablePointer<FFICandidate>.allocate(capacity: 1)
    secondLegacyCandidate.initialize(
        to: FFICandidate(
            text: secondLegacyText,
            subtext: secondLegacySubtext,
            hiragana: nil,
            correspondingCount: 1,
            candidateId: 4
        )
    )

    let legacyList = UnsafeMutablePointer<UnsafeMutablePointer<FFICandidate>?>.allocate(capacity: 3)
    legacyList.advanced(by: 0).initialize(to: firstLegacyCandidate)
    legacyList.advanced(by: 1).initialize(to: nil)
    legacyList.advanced(by: 2).initialize(to: secondLegacyCandidate)
    free_candidate_list(legacyList, 3)
}

@Test func constructCandidateStringAdvancesByRubyWithoutMutatingRemainder() async throws {
    let candidate = Candidate(
        text: "今日は",
        value: -1,
        composingCount: .inputCount(5),
        lastMid: MIDData.一般.mid,
        data: [
            DicdataElement(
                word: "今日",
                ruby: "きょう",
                cid: CIDData.一般名詞.cid,
                mid: MIDData.一般.mid,
                value: -1
            ),
            DicdataElement(
                word: "は",
                ruby: "は",
                cid: CIDData.一般名詞.cid,
                mid: MIDData.一般.mid,
                value: -1
            ),
        ]
    )

    #expect(constructCandidateString(candidate: candidate, hiragana: "きょうは") == "今日は")
}

@Test func constructCandidateStringFallsBackToRemainingHiraganaWhenRubyOverruns() async throws {
    let candidate = testCandidate(
        word: "今日",
        ruby: "きょう",
        composingCount: .inputCount(2)
    )

    #expect(constructCandidateString(candidate: candidate, hiragana: "きょ") == "きょ")
}

@Test func zenzaiNormalNBestSupplementKeepsZenzaiFirstAndDeduplicates() async throws {
    let hiragana = "ここではきものをぬいでください"
    let zenzaiTop = testCandidate(
        word: "ここでは着物を脱いでください",
        ruby: hiragana,
        composingCount: .inputCount(16)
    )
    let zenzaiRichSecond = testCandidate(
        word: "ここで履物を脱いでください",
        ruby: hiragana,
        composingCount: .inputCount(16)
    )
    let duplicatedTop = testCandidate(
        word: "ここでは着物を脱いでください",
        ruby: hiragana,
        composingCount: .inputCount(16)
    )
    let normalSecond = testCandidate(
        word: "ここでは着物を脱いでくださ異",
        ruby: hiragana,
        composingCount: .inputCount(16)
    )
    let normalThird = testCandidate(
        word: "ここでは着物を脱いでくださ偉",
        ruby: hiragana,
        composingCount: .inputCount(16)
    )

    let merged = mergeZenzaiMainResultsWithNormalNBest(
        zenzaiResults: [zenzaiTop, zenzaiRichSecond],
        normalNBestResults: [duplicatedTop, normalSecond, normalThird, zenzaiRichSecond],
        hiragana: hiragana
    )

    #expect(
        merged.map { constructCandidateString(candidate: $0, hiragana: hiragana) } == [
            "ここでは着物を脱いでください",
            "ここで履物を脱いでください",
            "ここでは着物を脱いでくださ異",
            "ここでは着物を脱いでくださ偉",
        ]
    )
}

@Test func zenzaiNormalNBestSupplementFiltersWeakRichCandidates() async throws {
    let hiragana = "ここではきものをぬいでください"
    let zenzaiTop = testCandidate(
        word: "ここでは着物を脱いでください",
        ruby: hiragana,
        composingCount: .inputCount(16)
    )
    let partialRich = testCandidate(
        word: "ここでは",
        ruby: "ここでは",
        composingCount: .inputCount(4)
    )
    let katakanaEchoRich = testCandidate(
        word: "ココデハキモノヲヌイデクダサイ",
        ruby: hiragana,
        composingCount: .inputCount(16)
    )
    let hiraganaEchoRich = testCandidate(
        word: hiragana,
        ruby: hiragana,
        composingCount: .inputCount(16)
    )
    let usefulRich = testCandidate(
        word: "ここで履物を脱いでください",
        ruby: hiragana,
        composingCount: .inputCount(16)
    )
    let normalSecond = testCandidate(
        word: "ここでは着物を抜いてください",
        ruby: hiragana,
        composingCount: .inputCount(16)
    )

    let merged = mergeZenzaiMainResultsWithNormalNBest(
        zenzaiResults: [zenzaiTop, partialRich, katakanaEchoRich, hiraganaEchoRich, usefulRich],
        normalNBestResults: [normalSecond],
        hiragana: hiragana
    )

    #expect(
        merged.map { constructCandidateString(candidate: $0, hiragana: hiragana) } == [
            "ここでは着物を脱いでください",
            "ここで履物を脱いでください",
            "ここでは着物を抜いてください",
        ]
    )
}

@Test func zenzaiNormalNBestSupplementCanKeepFirstClauseRichCandidates() async throws {
    let hiragana = "ここではきものをぬいでください"
    let firstClause = testCandidate(
        word: "ここでは",
        ruby: "ここでは",
        composingCount: .inputCount(4)
    )
    let alternativeFirstClause = testCandidate(
        word: "ここで",
        ruby: "ここで",
        composingCount: .inputCount(3)
    )
    let normalFirstClause = testCandidate(
        word: "此処では",
        ruby: "ここでは",
        composingCount: .inputCount(4)
    )

    let merged = mergeZenzaiMainResultsWithNormalNBest(
        zenzaiResults: [firstClause, alternativeFirstClause],
        normalNBestResults: [normalFirstClause],
        hiragana: hiragana,
        filterZenzaiAlternatives: false
    )

    #expect(
        merged.map { constructCandidateString(candidate: $0, hiragana: hiragana) } == [
            "ここでは",
            "ここで",
            "此処では",
        ]
    )
}

@Test func zenzaiNormalNBestSupplementUsesNormalCandidatesWhenZenzaiResultsAreEmpty() async throws {
    let hiragana = "あしたのてんきはあめです"
    let normal = testCandidate(
        word: "明日の天気は雨です",
        ruby: hiragana,
        composingCount: .inputCount(21)
    )

    let merged = mergeZenzaiMainResultsWithNormalNBest(
        zenzaiResults: [],
        normalNBestResults: [normal],
        hiragana: hiragana
    )

    #expect(merged.map { constructCandidateString(candidate: $0, hiragana: hiragana) } == ["明日の天気は雨です"])
}

@Test func cursorPrefixBoundarySelectionUsesZenzaiFirstClauseBeforeNormalFallback() async throws {
    let boundaryCounts = await MainActor.run {
        var source = ComposingText()
        source.insertAtCursorPosition("aruteidonagaibunsetsudemo", inputStyle: .roman2kana)
        let preview = makeCandidatePreviewComposingText(from: source).composingText
        let zenzaiFirstClause = testCandidate(
            word: "ある程度",
            ruby: "あるていど",
            composingCount: .inputCount(8)
        )
        let normalLongerClause = testCandidate(
            word: "ある程度長い",
            ruby: "あるていどながい",
            composingCount: .inputCount(13)
        )
        let mergedFirstClauseResults = mergeZenzaiMainResultsWithNormalNBest(
            zenzaiResults: [zenzaiFirstClause],
            normalNBestResults: [normalLongerClause],
            hiragana: preview.convertTarget
        )
        let boundaryFirstClauseResults = cursorPrefixBoundaryFirstClauseResults(
            zenzaiFirstClauseResults: [zenzaiFirstClause],
            mergedFirstClauseResults: mergedFirstClauseResults
        )

        return (
            selected: cursorPrefixFirstClauseCorrespondingCount(
                firstClauseResults: boundaryFirstClauseResults,
                originalComposingText: source,
                previewComposingText: preview
            ),
            merged: cursorPrefixFirstClauseCorrespondingCount(
                firstClauseResults: mergedFirstClauseResults,
                originalComposingText: source,
                previewComposingText: preview
            )
        )
    }

    #expect(boundaryCounts.selected == 8)
    #expect(boundaryCounts.merged == 13)
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
    #expect(map["n{any character}"] == "ん{any character}")
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

@Test func warmupUsesShortInputWhenZenzaiRuntimeIsDisabled() async throws {
    let metrics = await MainActor.run {
        let warmupComposingText = makeWarmupComposingText(
            zenzaiRuntimeEnabled: false,
            inputStyle: .roman2kana
        )
        return (
            inputCount: warmupComposingText.input.count,
            hiraganaCount: warmupComposingText.convertTarget.count,
            convertTarget: warmupComposingText.convertTarget
        )
    }

    #expect(metrics.inputCount == 1)
    #expect(metrics.hiraganaCount == 1)
    #expect(metrics.convertTarget == "あ")
}

@Test func warmupUsesZenzaiCandidatePathWhenRuntimeIsEnabled() async throws {
    let metrics = await MainActor.run {
        let warmupComposingText = makeWarmupComposingText(
            zenzaiRuntimeEnabled: true,
            inputStyle: .direct
        )
        return (
            inputCount: warmupComposingText.input.count,
            hiraganaCount: warmupComposingText.convertTarget.count,
            convertTarget: warmupComposingText.convertTarget
        )
    }

    #expect(metrics.inputCount == zenzaiWarmupRomanInput.count)
    #expect(metrics.inputCount >= minInputCountForZenzaiCandidates)
    #expect(metrics.hiraganaCount >= minHiraganaCountForZenzaiCandidates)
    #expect(metrics.convertTarget == "にほんご")
    #expect(
        effectiveZenzaiEnabledForCandidates(
            isConfigured: true,
            inputCount: metrics.inputCount,
            hiraganaCount: metrics.hiraganaCount
        )
    )
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

@Test func zenzaiBackendNormalizationIgnoresCaseAndWhitespace() async throws {
    #expect(normalizedZenzaiBackend(" Vulkan ") == "vulkan")
    #expect(normalizedZenzaiBackend(nil) == "cpu")
}

@Test func serverOptionsDisableJapanesePrediction() async throws {
    let predictionMode = await MainActor.run {
        getOptions(zenzaiEnabled: false).requireJapanesePrediction
    }

    #expect(predictionMode == .disabled)
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

@Test func customRomajiTableCommitsNBeforeConsonant() async throws {
    let rows = [
        row("n", "ん"),
        row("na", "な"),
        row("nn", "ん"),
        row("n'", "ん"),
        row("nya", "にゃ"),
        row("ta", "た"),
    ]
    let inputStyle = try makeTemporaryCustomInputStyle(rows)

    let convertTarget = await MainActor.run {
        var source = ComposingText()
        source.insertAtCursorPosition("nta", inputStyle: inputStyle)
        return source.convertTarget
    }

    #expect(convertTarget == "んた")
}

@Test func dictionaryCandidatesIncludeKanjiAfterRomanTrailingNPreview() async throws {
    let packageRoot = packageRootURL()
    let dictionaryURL = packageRoot
        .appending(path: "azooKey_dictionary_storage")
        .appending(path: "Dictionary")
    let memoryURL = FileManager.default.temporaryDirectory
        .appending(path: "azookey-server-test-\(UUID().uuidString)")
    defer {
        try? FileManager.default.removeItem(at: memoryURL)
    }

    let candidates = await MainActor.run {
        var source = ComposingText()
        source.insertAtCursorPosition("iikagenn", inputStyle: .roman2kana)
        let preview = makeCandidatePreviewComposingText(from: source)
        let previewHiragana = preview.composingText.convertTarget
        let testConverter = KanaKanjiConverter(dictionaryURL: dictionaryURL, preloadDictionary: true)
        return testConverter.requestCandidates(
            preview.composingText,
            options: testConvertRequestOptions(memoryURL: memoryURL)
        )
        .mainResults
        .map { constructCandidateString(candidate: $0, hiragana: previewHiragana) }
    }

    #expect(candidates.contains { $0.contains("加減") }, "candidates: \(candidates)")
}

@Test func singleWordKanjiCandidateBeatsHiraganaPrediction() async throws {
    let packageRoot = packageRootURL()
    let dictionaryURL = packageRoot
        .appending(path: "azooKey_dictionary_storage")
        .appending(path: "Dictionary")
    let memoryURL = FileManager.default.temporaryDirectory
        .appending(path: "azookey-server-test-\(UUID().uuidString)")
    defer {
        try? FileManager.default.removeItem(at: memoryURL)
    }

    let candidates = await MainActor.run {
        var source = ComposingText()
        source.insertAtCursorPosition("kannji", inputStyle: .roman2kana)
        let preview = makeCandidatePreviewComposingText(from: source)
        let previewHiragana = preview.composingText.convertTarget
        let testConverter = KanaKanjiConverter(dictionaryURL: dictionaryURL, preloadDictionary: true)
        return testConverter.requestCandidates(
            preview.composingText,
            options: testConvertRequestOptions(memoryURL: memoryURL)
        )
        .mainResults
        .prefix(5)
        .map { candidate in
            constructCandidateString(candidate: candidate, hiragana: previewHiragana)
        }
    }

    #expect(candidates.first == "感じ", "candidates: \(candidates)")
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

@Test func singleNBoundaryKeepsFollowingConsonantInRemainingText() async throws {
    let resolved = await MainActor.run {
        var source = ComposingText()
        source.insertAtCursorPosition("iikagentouitusiro", inputStyle: .roman2kana)
        return resolveCandidateComposition(
            composingText: source,
            candidateComposingCount: .inputCount(8)
        )
    }

    #expect(resolved.correspondingCount == 7)
    #expect(resolved.remainingConvertTarget == "とういつしろ")
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

    let preview = await MainActor.run {
        var source = ComposingText()
        source.insertAtCursorPosition("q", inputStyle: inputStyle)
        return makeCandidatePreviewComposingText(from: source)
    }

    #expect(preview.syntheticEndOfText == false)
    #expect(preview.composingText.convertTarget == "q")
}

@Test func cursorPrefixCandidatesSupplementFirstClauseWithMainResultsForSameBoundary() async throws {
    let resultTexts = await MainActor.run {
        var source = ComposingText()
        source.insertAtCursorPosition("aruteidonagai", inputStyle: .roman2kana)
        let preview = makeCandidatePreviewComposingText(from: source).composingText
        let firstClause = testCandidate(
            word: "ある程度",
            ruby: "あるていど",
            composingCount: .inputCount(8)
        )
        let hiragana = testCandidate(
            word: "あるていど",
            ruby: "あるていど",
            composingCount: .inputCount(8)
        )
        let katakana = testCandidate(
            word: "アルテイド",
            ruby: "あるていど",
            composingCount: .inputCount(8)
        )
        let fullSentence = Candidate(
            text: "ある程度長い",
            value: -1,
            composingCount: .inputCount(13),
            lastMid: MIDData.一般.mid,
            data: [
                DicdataElement(
                    word: "ある程度長い",
                    ruby: "あるていどながい",
                    cid: CIDData.一般名詞.cid,
                    mid: MIDData.一般.mid,
                    value: -1
                )
            ]
        )
        return cursorPrefixCandidateResults(
            mainResults: [fullSentence, hiragana, katakana],
            firstClauseResults: [firstClause],
            originalComposingText: source,
            previewComposingText: preview,
            previewHiragana: preview.convertTarget
        ).map { constructCandidateString(candidate: $0, hiragana: preview.convertTarget) }
    }

    #expect(resultTexts == ["ある程度", "あるていど", "アルテイド"])
}

@Test func cursorPrefixCandidatesDropFirstClauseResultsForDifferentBoundary() async throws {
    let resultTexts = await MainActor.run {
        var source = ComposingText()
        source.insertAtCursorPosition("iikagentouitusiro", inputStyle: .roman2kana)
        let preview = makeCandidatePreviewComposingText(from: source).composingText
        let firstClause = testCandidate(
            word: "いい加減",
            ruby: "いいかげん",
            composingCount: .inputCount(7)
        )
        let tooShort = testCandidate(
            word: "いい",
            ruby: "いい",
            composingCount: .inputCount(2)
        )
        let hiragana = testCandidate(
            word: "いいかげん",
            ruby: "いいかげん",
            composingCount: .inputCount(7)
        )
        return cursorPrefixCandidateResults(
            mainResults: [],
            firstClauseResults: [firstClause, tooShort, hiragana],
            originalComposingText: source,
            previewComposingText: preview,
            previewHiragana: preview.convertTarget
        ).map { constructCandidateString(candidate: $0, hiragana: preview.convertTarget) }
    }

    #expect(resultTexts == ["いい加減", "いいかげん"])
}

@Test func cursorPrefixCandidatesUseLongestFirstClauseBoundaryWhenShorterCandidateRanksFirst() async throws {
    let resultTexts = await MainActor.run {
        var source = ComposingText()
        source.insertAtCursorPosition("iikagentouitusiro", inputStyle: .roman2kana)
        let preview = makeCandidatePreviewComposingText(from: source).composingText
        let tooShort = testCandidate(
            word: "いい",
            ruby: "いい",
            composingCount: .inputCount(2)
        )
        let firstClause = testCandidate(
            word: "いい加減",
            ruby: "いいかげん",
            composingCount: .inputCount(7)
        )
        let hiragana = testCandidate(
            word: "いいかげん",
            ruby: "いいかげん",
            composingCount: .inputCount(7)
        )
        return cursorPrefixCandidateResults(
            mainResults: [],
            firstClauseResults: [tooShort, firstClause, hiragana],
            originalComposingText: source,
            previewComposingText: preview,
            previewHiragana: preview.convertTarget
        ).map { constructCandidateString(candidate: $0, hiragana: preview.convertTarget) }
    }

    #expect(resultTexts == ["いい加減", "いいかげん"])
}

@Test func cursorPrefixCandidatesPreferClauseTerminalBoundaryOverLongerNounPrefix() async throws {
    let resultTexts = await MainActor.run {
        var source = ComposingText()
        source.insertAtCursorPosition("wagahaihanekodearunamaehamadanai", inputStyle: .roman2kana)
        let preview = makeCandidatePreviewComposingText(from: source).composingText
        let badLongBoundary = Candidate(
            text: "吾輩は猫である名",
            value: -1,
            composingCount: .inputCount(20),
            lastMid: MIDData.一般.mid,
            data: [
                DicdataElement(
                    word: "吾輩は猫である名",
                    ruby: "わがはいはねこであるな",
                    cid: CIDData.一般名詞.cid,
                    mid: MIDData.一般.mid,
                    value: -1
                )
            ]
        )
        let sentenceBoundary = Candidate(
            text: "吾輩は猫である",
            value: -1,
            composingCount: .inputCount(18),
            lastMid: MIDData.一般.mid,
            data: [
                DicdataElement(
                    word: "吾輩は猫である",
                    ruby: "わがはいはねこである",
                    cid: CIDData.一般名詞.cid,
                    mid: MIDData.一般.mid,
                    value: -1
                )
            ]
        )
        return cursorPrefixCandidateDisplayResults(
            mainResults: [],
            firstClauseResults: [badLongBoundary, sentenceBoundary],
            originalComposingText: source,
            previewComposingText: preview,
            previewHiragana: preview.convertTarget
        ).map(\.displayText)
    }

    #expect(resultTexts == ["吾輩は猫である"])
}

@Test func cursorPrefixCandidatesPreferProperBoundaryOverFullPhraseCandidate() async throws {
    let resultTexts = await MainActor.run {
        var source = ComposingText()
        source.insertAtCursorPosition("touitusiro", inputStyle: .roman2kana)
        let preview = makeCandidatePreviewComposingText(from: source).composingText
        let fullPhrase = testCandidate(
            word: "統一しろ",
            ruby: "とういつしろ",
            composingCount: .inputCount(10)
        )
        let firstClause = testCandidate(
            word: "統一",
            ruby: "とういつ",
            composingCount: .inputCount(6)
        )
        let hiragana = testCandidate(
            word: "とういつ",
            ruby: "とういつ",
            composingCount: .inputCount(6)
        )
        let katakana = testCandidate(
            word: "トウイツ",
            ruby: "とういつ",
            composingCount: .inputCount(6)
        )
        return cursorPrefixCandidateDisplayResults(
            mainResults: [fullPhrase],
            firstClauseResults: [fullPhrase, firstClause],
            exactClauseResults: [hiragana, katakana],
            originalComposingText: source,
            previewComposingText: preview,
            previewHiragana: preview.convertTarget
        ).map(\.displayText)
    }

    #expect(resultTexts == ["統一", "とういつ", "トウイツ"])
}

@Test func cursorPrefixCandidatesSupplementWithExactClauseResultsWhenMainResultsLackSameBoundary() async throws {
    let resultTexts = await MainActor.run {
        var source = ComposingText()
        source.insertAtCursorPosition("aruteidonagaibunsetsudemo", inputStyle: .roman2kana)
        let preview = makeCandidatePreviewComposingText(from: source).composingText
        let firstClause = testCandidate(
            word: "ある程度",
            ruby: "あるていど",
            composingCount: .inputCount(8)
        )
        let fullSentence = Candidate(
            text: "ある程度長い文節でも",
            value: -1,
            composingCount: .inputCount(25),
            lastMid: MIDData.一般.mid,
            data: [
                DicdataElement(
                    word: "ある程度長い文節でも",
                    ruby: "あるていどながいぶんせつでも",
                    cid: CIDData.一般名詞.cid,
                    mid: MIDData.一般.mid,
                    value: -1
                )
            ]
        )
        let hiragana = testCandidate(
            word: "あるていど",
            ruby: "あるていど",
            composingCount: .inputCount(8)
        )
        let katakana = testCandidate(
            word: "アルテイド",
            ruby: "あるていど",
            composingCount: .inputCount(8)
        )
        return cursorPrefixCandidateResults(
            mainResults: [fullSentence],
            firstClauseResults: [firstClause],
            exactClauseResults: [hiragana, katakana],
            originalComposingText: source,
            previewComposingText: preview,
            previewHiragana: preview.convertTarget
        ).map { constructCandidateString(candidate: $0, hiragana: preview.convertTarget) }
    }

    #expect(resultTexts == ["ある程度", "あるていど", "アルテイド"])
}

@Test func cursorPrefixCandidatesSupplementParticleClauseWithExactClauseResults() async throws {
    let resultTexts = await MainActor.run {
        var source = ComposingText()
        source.insertAtCursorPosition("bunsetsudemofukusuunibunkatsusareru", inputStyle: .roman2kana)
        let preview = makeCandidatePreviewComposingText(from: source).composingText
        let firstClause = testCandidate(
            word: "文節でも",
            ruby: "ぶんせつでも",
            composingCount: .inputCount(12)
        )
        let alternative = testCandidate(
            word: "分節でも",
            ruby: "ぶんせつでも",
            composingCount: .inputCount(12)
        )
        let hiragana = testCandidate(
            word: "ぶんせつでも",
            ruby: "ぶんせつでも",
            composingCount: .inputCount(12)
        )
        let katakana = testCandidate(
            word: "ブンセツデモ",
            ruby: "ぶんせつでも",
            composingCount: .inputCount(12)
        )
        let fullSentence = Candidate(
            text: "文節でも複数に分割される",
            value: -1,
            composingCount: .inputCount(35),
            lastMid: MIDData.一般.mid,
            data: [
                DicdataElement(
                    word: "文節でも複数に分割される",
                    ruby: "ぶんせつでもふくすうにぶんかつされる",
                    cid: CIDData.一般名詞.cid,
                    mid: MIDData.一般.mid,
                    value: -1
                )
            ]
        )
        return cursorPrefixCandidateResults(
            mainResults: [fullSentence],
            firstClauseResults: [firstClause, alternative],
            exactClauseResults: [firstClause, alternative, hiragana, katakana],
            originalComposingText: source,
            previewComposingText: preview,
            previewHiragana: preview.convertTarget
        ).map { constructCandidateString(candidate: $0, hiragana: preview.convertTarget) }
    }

    #expect(resultTexts == ["文節でも", "分節でも", "ぶんせつでも", "ブンセツデモ"])
}

@Test func cursorPrefixExactClauseComposingTextPreservesSelectedClauseInput() async throws {
    let clause = await MainActor.run {
        var source = ComposingText()
        source.insertAtCursorPosition("aruteidonagaibunsetsudemo", inputStyle: .roman2kana)
        return makeCursorPrefixExactClauseComposingText(
            prefixComposingText: source,
            correspondingCount: 8
        )
    }

    #expect(clause.convertTarget == "あるていど")
    #expect(clause.input.count == 8)
}
