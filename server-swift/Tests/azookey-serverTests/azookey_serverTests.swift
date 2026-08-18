import Foundation
import ffi
import KanaKanjiConverterModule
import Testing
@testable import azookey_server

private func row(_ input: String, _ output: String, _ next: String = "") -> RomajiTableRow {
    RomajiTableRow(input: input, output: output, next_input: next)
}

// InputStyleManager uses a process-wide unsynchronized dictionary. Keep test
// registrations on the same actor as every ComposingText lookup.
@MainActor private func makeTemporaryCustomInputStyle(_ rows: [RomajiTableRow]) throws -> InputStyle {
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

private func defaultWindowsRomajiRows() throws -> [RomajiTableRow] {
    let tableURL = packageRootURL()
        .deletingLastPathComponent()
        .appending(path: "crates")
        .appending(path: "shared")
        .appending(path: "src")
        .appending(path: "default_romaji_table.txt")
    let content = try String(contentsOf: tableURL, encoding: .utf8)

    return content.split(whereSeparator: \.isNewline).compactMap { line in
        let trimmed = String(line).trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty, !trimmed.hasPrefix("#") else {
            return nil
        }
        let columns = trimmed.split(separator: "\t", omittingEmptySubsequences: false)
        guard columns.count >= 2, !columns[0].isEmpty, !columns[1].isEmpty else {
            return nil
        }
        return row(
            String(columns[0]),
            String(columns[1]),
            columns.count >= 3 ? String(columns[2]) : ""
        )
    }
}

private struct ReverseDictionaryFixtureEntry {
    let surface: String
    let reading: String
    let score: Float
}

private func appendUInt16LE(_ value: UInt16, to data: inout Data) {
    var value = value.littleEndian
    withUnsafeBytes(of: &value) { data.append(contentsOf: $0) }
}

private func appendUInt32LE(_ value: UInt32, to data: inout Data) {
    var value = value.littleEndian
    withUnsafeBytes(of: &value) { data.append(contentsOf: $0) }
}

private func makeReverseDictionaryFixture(_ entries: [ReverseDictionaryFixtureEntry]) -> Data {
    let records = entries.sorted {
        Array($0.surface.utf8).lexicographicallyPrecedes(Array($1.surface.utf8))
    }.map { entry in
        let surface = Data(entry.surface.utf8)
        let reading = Data(entry.reading.utf8)
        var record = Data()
        appendUInt16LE(UInt16(surface.count), to: &record)
        appendUInt16LE(UInt16(reading.count), to: &record)
        appendUInt32LE(entry.score.bitPattern, to: &record)
        record.append(surface)
        record.append(reading)
        return record
    }

    let headerSize = 8 + (records.count + 1) * 4
    var data = Data("AZR2".utf8)
    appendUInt32LE(UInt32(records.count), to: &data)
    var offset = headerSize
    for record in records {
        appendUInt32LE(UInt32(offset), to: &data)
        offset += record.count
    }
    appendUInt32LE(UInt32(offset), to: &data)
    for record in records {
        data.append(record)
    }
    return data
}

private func writeReverseDictionaryFixture(
    _ data: Data,
    under temporaryDirectory: URL
) throws -> URL {
    let dictionaryURL = temporaryDirectory.appendingPathComponent("Dictionary", isDirectory: true)
    let reverseURL = dictionaryURL.appendingPathComponent("Reverse", isDirectory: true)
    try FileManager.default.createDirectory(at: reverseURL, withIntermediateDirectories: true)
    try data.write(to: reverseURL.appendingPathComponent("reverse-v2.bin"))
    return dictionaryURL
}

@Test func reconversionDictionaryPrefersExactReadingsOverSegmentedReadings() throws {
    let temporaryDirectory = FileManager.default.temporaryDirectory
        .appendingPathComponent("azookey-reconversion-test-\(UUID().uuidString)", isDirectory: true)
    defer { try? FileManager.default.removeItem(at: temporaryDirectory) }
    let dictionaryURL = try writeReverseDictionaryFixture(
        makeReverseDictionaryFixture([
            .init(surface: "今日", reading: "キョウ", score: -1),
            .init(surface: "加", reading: "カ", score: -1),
            .init(surface: "減", reading: "ヘ", score: -1),
            .init(surface: "減", reading: "ゲン", score: -2),
            .init(surface: "加減", reading: "カゲン", score: -3),
            .init(surface: "統", reading: "ミツル", score: -1),
            .init(surface: "一", reading: "ハジメ", score: -1),
            .init(surface: "統一", reading: "トウイツ", score: -3),
            .init(surface: "日本", reading: "ニッポン", score: -1),
            .init(surface: "日本", reading: "ニホン", score: -2),
            .init(surface: "日本語", reading: "ニホンゴ", score: -0.5),
            .init(surface: "語", reading: "ゴ", score: -1),
        ]),
        under: temporaryDirectory
    )
    var dictionary = ReconversionDictionary()
    try dictionary.load(from: dictionaryURL)

    #expect(dictionary.recordCount == 12)
    #expect(dictionary.inferReadings(for: "加減") == ["カゲン"])
    #expect(dictionary.inferReadings(for: "統一") == ["トウイツ"])
    #expect(dictionary.inferReadings(for: "日本") == ["ニッポン", "ニホン"])
    #expect(dictionary.inferReadings(for: "日本", limit: 1) == ["ニッポン"])
    #expect(dictionary.inferReadings(for: "日本語") == ["ニホンゴ"])
    #expect(dictionary.inferReadings(for: "今日は") == ["キョウハ"])
    #expect(
        dictionary.inferReadings(for: "今日\n日本")
            == ["キョウ\nニッポン", "キョウ\nニホン"]
    )
    #expect(dictionary.inferReadings(for: "😀") == ["😀"])
    #expect(dictionary.inferReadings(for: "龘").isEmpty)
}

@Test func reconversionDictionaryRejectsTruncatedIndex() throws {
    let temporaryDirectory = FileManager.default.temporaryDirectory
        .appendingPathComponent("azookey-reconversion-corrupt-test-\(UUID().uuidString)", isDirectory: true)
    defer { try? FileManager.default.removeItem(at: temporaryDirectory) }
    var data = Data("AZR2".utf8)
    appendUInt32LE(1, to: &data)
    appendUInt32LE(12, to: &data)
    let dictionaryURL = try writeReverseDictionaryFixture(data, under: temporaryDirectory)
    var dictionary = ReconversionDictionary()
    var rejected = false

    do {
        try dictionary.load(from: dictionaryURL)
    } catch {
        rejected = true
    }

    #expect(rejected)
    #expect(dictionary.recordCount == 0)
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

private func testConvertRequestOptions(
    memoryURL: URL,
    experimentalKeyboardTypoCorrection: Bool = false
) -> ConvertRequestOptions {
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
        experimentalKeyboardTypoCorrection: experimentalKeyboardTypoCorrection,
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

@MainActor private func keyboardTypoCandidateTexts(
    keys: String,
    inputStyle: InputStyle = .roman2kana,
    enabled: Bool,
    memoryURL: URL,
    converter: KanaKanjiConverter? = nil
) -> [String] {
    var composingText = ComposingText()
    for key in keys {
        composingText.insertAtCursorPosition(String(key), inputStyle: inputStyle)
    }
    return keyboardTypoCandidateTexts(
        composingText: composingText,
        enabled: enabled,
        memoryURL: memoryURL,
        converter: converter
    )
}

@MainActor private func keyboardTypoCandidateTexts(
    composingText: ComposingText,
    enabled: Bool,
    memoryURL: URL,
    converter: KanaKanjiConverter? = nil
) -> [String] {
    let packageRoot = packageRootURL()
    let dictionaryURL = packageRoot
        .appending(path: "azooKey_dictionary_storage")
        .appending(path: "Dictionary")
    let converter = converter ?? KanaKanjiConverter(
        dictionaryURL: dictionaryURL,
        preloadDictionary: true
    )
    converter.importDynamicUserDictionary(makeKeyboardTypoDictionaryEntries())
    return converter.requestCandidates(
        composingText,
        options: testConvertRequestOptions(
            memoryURL: memoryURL,
            experimentalKeyboardTypoCorrection: enabled
        )
    ).mainResults.map(\.text)
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

@Test func learningCandidateCacheEvictsOldestBatchesAtConfiguredLimit() {
    var cache = LearningCandidateCache(maxBatchCount: 2, maxSlotCount: 8)
    let candidate = testCandidate(
        word: "今日",
        ruby: "きょう",
        composingCount: .inputCount(3)
    )

    let firstBatchId = cache.appendBatch([candidate])!
    let firstId = cache.candidateId(at: 0, batchFirstId: firstBatchId)
    let secondBatchId = cache.appendBatch([candidate])!
    let secondId = cache.candidateId(at: 0, batchFirstId: secondBatchId)
    let thirdBatchId = cache.appendBatch([candidate])!
    let thirdId = cache.candidateId(at: 0, batchFirstId: thirdBatchId)

    #expect(firstId != secondId)
    #expect(secondId != thirdId)
    #expect(cache.batchCount == 2)
    #expect(cache.slotCount == 2)
    #expect(cache.consume(firstId) == nil)
    #expect(cache.consume(secondId)?.text == "今日")
    #expect(cache.consume(thirdId)?.text == "今日")
}

@Test func pinnedLearningCandidateSurvivesBatchEvictionUntilConsumed() {
    var cache = LearningCandidateCache(maxBatchCount: 2, maxSlotCount: 2)
    let first = testCandidate(
        word: "今日",
        ruby: "きょう",
        composingCount: .inputCount(3)
    )
    let later = testCandidate(
        word: "教",
        ruby: "きょう",
        composingCount: .inputCount(3)
    )

    let firstBatchId = cache.appendBatch([first])!
    let firstId = cache.candidateId(at: 0, batchFirstId: firstBatchId)
    let pinnedFirst = cache.pin(firstId)
    #expect(pinnedFirst)

    _ = cache.appendBatch([later])
    _ = cache.appendBatch([later])

    #expect(cache.batchCount == 2)
    #expect(cache.slotCount == 2)
    #expect(cache.protectedSlotCount == 1)
    #expect(cache.consume(firstId)?.text == "今日")
    #expect(cache.consume(firstId) == nil)
    #expect(cache.protectedSlotCount == 1)
    cache.removeAllPins()
    #expect(cache.protectedSlotCount == 0)
}

@Test func protectedLearningCandidateBatchesShareTheExplicitSlotLimit() {
    var cache = LearningCandidateCache(maxBatchCount: 4, maxSlotCount: 3)
    let first = testCandidate(
        word: "今日",
        ruby: "きょう",
        composingCount: .inputCount(3)
    )
    let alternate = testCandidate(
        word: "教",
        ruby: "きょう",
        composingCount: .inputCount(3)
    )
    let later = testCandidate(
        word: "京",
        ruby: "きょう",
        composingCount: .inputCount(3)
    )

    let firstBatchId = cache.appendBatch([first, alternate])!
    let firstId = cache.candidateId(at: 0, batchFirstId: firstBatchId)
    let alternateId = cache.candidateId(at: 1, batchFirstId: firstBatchId)
    let pinnedFirst = cache.pin(firstId)
    #expect(pinnedFirst)
    #expect(cache.protectedSlotCount == 2)

    let laterBatchId = cache.appendBatch([later])!
    let laterId = cache.candidateId(at: 0, batchFirstId: laterBatchId)
    let pinnedLater = cache.pin(laterId)
    #expect(pinnedLater)
    #expect(cache.slotCount == 3)
    #expect(cache.protectedSlotCount == 3)
    #expect(cache.appendBatch([later]) == nil)

    // Protecting one selection preserves alternate nonzero IDs from the same
    // client snapshot instead of copying only the selected candidate.
    #expect(cache.consume(alternateId)?.text == "教")
    #expect(cache.consume(laterId)?.text == "京")
    cache.removeAllPins()
    #expect(cache.protectedSlotCount == 0)
    #expect(cache.appendBatch([later]) != nil)
}

@Test func compositionEditDiscardsFutureOnlyPinsWithoutExtraRPC() async {
    await MainActor.run {
        let previousLearningCandidateCache = learningCandidateCache
        let previousComposingTextSnapshots = composingTextSnapshots
        defer {
            learningCandidateCache = previousLearningCandidateCache
            composingTextSnapshots = previousComposingTextSnapshots
        }

        learningCandidateCache = LearningCandidateCache(
            maxBatchCount: 1,
            maxSlotCount: 1
        )
        let candidate = testCandidate(
            word: "今日",
            ruby: "きょう",
            composingCount: .inputCount(3)
        )
        let batchFirstId = learningCandidateCache.appendBatch([candidate])!
        let candidateId = learningCandidateCache.candidateId(
            at: 0,
            batchFirstId: batchFirstId
        )
        let pinnedFuture = learningCandidateCache.pin(candidateId)
        #expect(pinnedFuture)
        _ = learningCandidateCache.appendBatch([candidate])

        composingTextSnapshots.removeAll()
        discardPinnedLearningCandidatesBeforeCompositionEdit()

        #expect(learningCandidateCache.protectedSlotCount == 0)
        _ = learningCandidateCache.appendBatch([candidate])
        #expect(learningCandidateCache.consume(candidateId) == nil)

        learningCandidateCache.removeAll()
        let activeBatchFirstId = learningCandidateCache.appendBatch([candidate])!
        let activeCandidateId = learningCandidateCache.candidateId(
            at: 0,
            batchFirstId: activeBatchFirstId
        )
        let pinnedActive = learningCandidateCache.pin(activeCandidateId)
        #expect(pinnedActive)
        composingTextSnapshots = [ComposingText()]
        discardPinnedLearningCandidatesBeforeCompositionEdit()
        #expect(learningCandidateCache.protectedSlotCount == 1)
    }
}

@Test func learningCandidateCacheEvictsWholeBatchesToStayWithinSlotLimit() {
    var cache = LearningCandidateCache(maxBatchCount: 4, maxSlotCount: 3)
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

    let oldestBatchId = cache.appendBatch([first, second])!
    let oldestId = cache.candidateId(at: 0, batchFirstId: oldestBatchId)
    let middleBatchId = cache.appendBatch([first])!
    let middleId = cache.candidateId(at: 0, batchFirstId: middleBatchId)
    let newestBatchId = cache.appendBatch([second])!
    let newestId = cache.candidateId(at: 0, batchFirstId: newestBatchId)

    #expect(cache.batchCount == 2)
    #expect(cache.slotCount == 2)
    #expect(cache.consume(oldestId) == nil)
    #expect(cache.consume(middleId)?.text == "今日")
    #expect(cache.consume(newestId)?.text == "教")
}

@Test func oversizedLearningCandidateBatchCachesOnlyBoundedPrefix() {
    var cache = LearningCandidateCache(maxBatchCount: 2, maxSlotCount: 2)
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
    let third = testCandidate(
        word: "京",
        ruby: "きょう",
        composingCount: .inputCount(3)
    )

    let batchFirstId = cache.appendBatch([first, second, third])!
    let firstId = cache.candidateId(at: 0, batchFirstId: batchFirstId)
    let secondId = cache.candidateId(at: 1, batchFirstId: batchFirstId)

    #expect(firstId != 0)
    #expect(secondId == firstId + 1)
    #expect(cache.candidateId(at: 2, batchFirstId: batchFirstId) == 0)
    #expect(cache.batchCount == 1)
    #expect(cache.slotCount == 2)
    #expect(cache.consume(firstId)?.text == "今日")
    #expect(cache.consume(secondId)?.text == "教")
}

@Test func defaultLearningCandidateCacheRetainsPreparedFutureClauseWindow() {
    var cache = LearningCandidateCache()
    let candidate = testCandidate(
        word: "今日",
        ruby: "きょう",
        composingCount: .inputCount(3)
    )
    let observedHighWaterCandidateCount = 201
    let candidates = Array(
        repeating: candidate,
        count: observedHighWaterCandidateCount
    )
    var oldestId: UInt64 = 0
    var newestId: UInt64 = 0

    // The client retains the current clause and up to 16 prepared advances.
    for generation in 0..<17 {
        let batchFirstId = cache.appendBatch(candidates)!
        if generation == 0 {
            oldestId = cache.candidateId(at: 0, batchFirstId: batchFirstId)
        }
        newestId = cache.candidateId(
            at: observedHighWaterCandidateCount - 1,
            batchFirstId: batchFirstId
        )
    }

    #expect(cache.batchCount == 17)
    #expect(cache.slotCount == 17 * observedHighWaterCandidateCount)
    #expect(cache.consume(oldestId)?.text == "今日")
    #expect(cache.consume(newestId)?.text == "今日")
}

@Test func protectedClauseCandidateBatchSurvivesBulkCacheEviction() {
    var cache = LearningCandidateCache()
    let candidate = testCandidate(
        word: "今日",
        ruby: "きょう",
        composingCount: .inputCount(3)
    )
    let alternate = testCandidate(
        word: "教",
        ruby: "きょう",
        composingCount: .inputCount(3)
    )
    let candidates = Array(repeating: candidate, count: 201)
    let protectedBatchId = cache.appendBatch([candidate, alternate])!
    let selectedId = cache.candidateId(at: 0, batchFirstId: protectedBatchId)
    let alternateId = cache.candidateId(at: 1, batchFirstId: protectedBatchId)
    let pinnedSelected = cache.pin(selectedId)
    #expect(pinnedSelected)
    var newestId: UInt64 = 0

    // More than 40 high-water candidate generations exceed the 8,192-slot
    // ring. Unprotected batches are evicted around the protected source batch.
    for _ in 0..<48 {
        let batchFirstId = cache.appendBatch(candidates)!
        newestId = cache.candidateId(at: 0, batchFirstId: batchFirstId)
    }

    #expect(cache.slotCount <= maxLearningCandidateCacheSlotCount)
    #expect(cache.protectedSlotCount == 2)
    #expect(cache.consume(selectedId)?.text == "今日")
    #expect(cache.consume(alternateId)?.text == "教")
    #expect(cache.consume(newestId)?.text == "今日")
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

@Test func learningCandidateBatchCommitsAllSelectionsAndPersistsOverridesOnce() async throws {
    let packageRoot = packageRootURL()
    let dictionaryURL = packageRoot
        .appending(path: "azooKey_dictionary_storage")
        .appending(path: "Dictionary")
    let memoryURL = FileManager.default.temporaryDirectory
        .appending(path: "azookey-server-learning-batch-test-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: memoryURL, withIntermediateDirectories: true)
    defer {
        try? FileManager.default.removeItem(at: memoryURL)
    }

    try await MainActor.run {
        let previousConverter = converter
        let previousSupplementConverter = normalNBestSupplementConverter
        let previousLearningType = currentLearningType
        let previousLearningMemoryDirectoryURL = currentLearningMemoryDirectoryURL
        let previousLearningCandidateCache = learningCandidateCache
        let previousLearningSelectionOverrides = learningSelectionOverrides
        defer {
            converter = previousConverter
            normalNBestSupplementConverter = previousSupplementConverter
            currentLearningType = previousLearningType
            currentLearningMemoryDirectoryURL = previousLearningMemoryDirectoryURL
            learningCandidateCache = previousLearningCandidateCache
            learningSelectionOverrides = previousLearningSelectionOverrides
        }

        converter = KanaKanjiConverter(dictionaryURL: dictionaryURL, preloadDictionary: true)
        normalNBestSupplementConverter = KanaKanjiConverter(
            dictionaryURL: dictionaryURL,
            preloadDictionary: false
        )
        currentLearningType = .inputAndOutput
        currentLearningMemoryDirectoryURL = memoryURL
        learningCandidateCache.removeAll()
        learningSelectionOverrides.removeAll()
        let options = testLearningConvertRequestOptions(memoryURL: memoryURL)

        @MainActor func candidateId(reading: String, output: String) throws -> UInt64 {
            var source = ComposingText()
            source.insertAtCursorPosition(reading, inputStyle: .direct)
            let candidates = converter.requestCandidates(source, options: options).mainResults
            let index = try #require(candidates.firstIndex { $0.text == output })
            let firstId = try #require(cacheLearningCandidates(candidates))
            return learningCandidateId(at: index, batchFirstId: firstId)
        }

        let candidateIds = try [
            candidateId(reading: "かとう", output: "加藤"),
            candidateId(reading: "はしる", output: "走る"),
        ]
        let commitKinds = [Int32(1), Int32(1)]
        let committedCount = candidateIds.withUnsafeBufferPointer { candidateIdsPointer in
            commitKinds.withUnsafeBufferPointer { commitKindsPointer in
                commit_learning_candidates(
                    candidateIds: candidateIdsPointer.baseAddress,
                    commitKinds: commitKindsPointer.baseAddress,
                    count: Int32(candidateIds.count)
                )
            }
        }

        #expect(committedCount == 2)
        learningSelectionOverrides.removeAll()
        loadLearningSelectionOverrides()
        #expect(learningSelectionOverrides["カトウ"] == "加藤")
        #expect(learningSelectionOverrides["ハシル"] == "走る")
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
        push_composing_text_snapshot(selectedCandidateId: 0)
        #expect(composingTextSnapshots.count == 1)

        var cursor: CInt = 0
        free_c_string(move_cursor(offset: -1024, cursorPtr: &cursor))
        free_c_string(move_cursor(offset: 125, cursorPtr: &cursor))
        #expect(cursor == 125)
        #expect(composingText.convertTargetCursorPosition == 1149)
        #expect(composingTextSnapshots.count == 1)

        pop_composing_text_snapshot(selectedCandidateId: 0)
        #expect(composingText.convertTargetCursorPosition == inputLength)
        #expect(composingTextSnapshots.isEmpty)

        push_composing_text_snapshot(selectedCandidateId: 0)
        clear_composing_text_snapshots()
        #expect(composingTextSnapshots.isEmpty)
    }
}

@Test func clauseBoundaryAdjustmentKeepsAlternateRomajiSegmentsAtomic() async {
    await MainActor.run {
        let previousComposingText = composingText
        defer {
            composingText = previousComposingText
        }

        let cases = [
            (raw: "tya", kana: "ちゃ"),
            (raw: "cha", kana: "ちゃ"),
            (raw: "tyu", kana: "ちゅ"),
            (raw: "chu", kana: "ちゅ"),
        ]
        for testCase in cases {
            composingText = ComposingText()
            composingText.insertAtCursorPosition(
                "\(testCase.raw)ibu",
                inputStyle: .roman2kana
            )
            #expect(composingText.convertTarget == "\(testCase.kana)いぶ")

            var applied: CInt = 0
            var adjustedInputCount: CInt = -1
            var cursorOffset: CInt = 0
            free_c_string(
                adjust_clause_boundary(
                    currentInputCount: 4,
                    direction: -1,
                    appliedPtr: &applied,
                    adjustedInputCountPtr: &adjustedInputCount,
                    cursorOffsetPtr: &cursorOffset
                )
            )

            #expect(applied == 1)
            #expect(adjustedInputCount == 3)
            #expect(composingText.convertTargetCursorPosition == testCase.kana.count)
            #expect(cursorOffset < 0)

            applied = 0
            adjustedInputCount = -1
            cursorOffset = 0
            free_c_string(
                adjust_clause_boundary(
                    currentInputCount: 3,
                    direction: -1,
                    appliedPtr: &applied,
                    adjustedInputCountPtr: &adjustedInputCount,
                    cursorOffsetPtr: &cursorOffset
                )
            )

            #expect(applied == 0)
            #expect(adjustedInputCount == 3)
            #expect(cursorOffset == 0)
            #expect(composingText.convertTargetCursorPosition == testCase.kana.count)

            free_c_string(
                adjust_clause_boundary(
                    currentInputCount: 3,
                    direction: 1,
                    appliedPtr: &applied,
                    adjustedInputCountPtr: &adjustedInputCount,
                    cursorOffsetPtr: &cursorOffset
                )
            )
            #expect(applied == 1)
            #expect(adjustedInputCount == 4)
            #expect(composingText.convertTargetCursorPosition == testCase.kana.count + 1)
        }
    }
}

@Test func clauseBoundaryAdjustmentIncludesDelayedSingleNBoundary() async {
    await MainActor.run {
        let previousComposingText = composingText
        defer {
            composingText = previousComposingText
        }

        composingText = ComposingText()
        composingText.insertAtCursorPosition(
            "iikagentouitusiro",
            inputStyle: .roman2kana
        )
        #expect(composingText.convertTarget == "いいかげんとういつしろ")

        _ = composingText.moveCursorFromCursorPosition(
            count: 6 - composingText.convertTargetCursorPosition
        )

        @MainActor func adjust(
            currentInputCount: CInt,
            direction: CInt,
            expectedInputCount: CInt,
            expectedCursorPosition: CInt
        ) {
            var applied: CInt = 0
            var adjustedInputCount: CInt = -1
            var cursorOffset: CInt = 0
            free_c_string(
                adjust_clause_boundary(
                    currentInputCount: currentInputCount,
                    direction: direction,
                    appliedPtr: &applied,
                    adjustedInputCountPtr: &adjustedInputCount,
                    cursorOffsetPtr: &cursorOffset
                )
            )

            #expect(applied == 1)
            #expect(adjustedInputCount == expectedInputCount)
            #expect(composingText.convertTargetCursorPosition == expectedCursorPosition)
        }

        adjust(
            currentInputCount: 9,
            direction: -1,
            expectedInputCount: 7,
            expectedCursorPosition: 5
        )
        adjust(
            currentInputCount: 7,
            direction: 1,
            expectedInputCount: 9,
            expectedCursorPosition: 6
        )
        adjust(
            currentInputCount: 9,
            direction: -1,
            expectedInputCount: 7,
            expectedCursorPosition: 5
        )
        adjust(
            currentInputCount: 7,
            direction: -1,
            expectedInputCount: 6,
            expectedCursorPosition: 4
        )
    }
}

@Test func clauseBoundaryAdjustmentSupportsDelayedSingleNInCustomInputStyle() async throws {
    try await MainActor.run {
        let previousComposingText = composingText
        defer {
            composingText = previousComposingText
        }

        let inputStyle = try makeTemporaryCustomInputStyle([
            row("n", "ん"),
            row("na", "な"),
            row("nn", "ん"),
            row("n'", "ん"),
            row("nya", "にゃ"),
            row("ta", "た"),
        ])
        composingText = ComposingText()
        composingText.insertAtCursorPosition("nta", inputStyle: inputStyle)
        #expect(composingText.convertTarget == "んた")

        var applied: CInt = 0
        var adjustedInputCount: CInt = -1
        var cursorOffset: CInt = 0
        free_c_string(
            adjust_clause_boundary(
                currentInputCount: 3,
                direction: -1,
                appliedPtr: &applied,
                adjustedInputCountPtr: &adjustedInputCount,
                cursorOffsetPtr: &cursorOffset
            )
        )

        #expect(applied == 1)
        #expect(adjustedInputCount == 1)
        #expect(composingText.convertTargetCursorPosition == 1)

        free_c_string(
            adjust_clause_boundary(
                currentInputCount: 1,
                direction: 1,
                appliedPtr: &applied,
                adjustedInputCountPtr: &adjustedInputCount,
                cursorOffsetPtr: &cursorOffset
            )
        )

        #expect(applied == 1)
        #expect(adjustedInputCount == 3)
        #expect(composingText.convertTargetCursorPosition == 2)
    }
}

@Test func clauseBoundaryAdjustmentKeepsAtomicCustomNSequence() async throws {
    try await MainActor.run {
        let previousComposingText = composingText
        defer {
            composingText = previousComposingText
        }

        let inputStyle = try makeTemporaryCustomInputStyle([
            row("nq", "んや"),
        ])
        composingText = ComposingText()
        composingText.insertAtCursorPosition("nq", inputStyle: inputStyle)
        #expect(composingText.convertTarget == "んや")

        var applied: CInt = 0
        var adjustedInputCount: CInt = -1
        var cursorOffset: CInt = 0
        free_c_string(
            adjust_clause_boundary(
                currentInputCount: 2,
                direction: -1,
                appliedPtr: &applied,
                adjustedInputCountPtr: &adjustedInputCount,
                cursorOffsetPtr: &cursorOffset
            )
        )

        #expect(applied == 0)
        #expect(adjustedInputCount == 2)
        #expect(composingText.convertTargetCursorPosition == 2)
    }
}

@Test func rawInputIdentitySurvivesClauseShrinkWithoutNormalizingRomaji() async {
    await MainActor.run {
        let previousComposingText = composingText
        defer {
            composingText = previousComposingText
        }

        let input = "aruteidonagaibunsyoudemofukusuuni"
        composingText = ComposingText()
        composingText.insertAtCursorPosition(input, inputStyle: .roman2kana)
        free_c_string(shrink_text(offset: 8))

        let rawInput = get_raw_input()
        defer {
            free_c_string(rawInput)
        }
        #expect(rawInput.map { String(cString: $0) } == String(input.dropFirst(8)))
    }
}

@Test func removeTextReturnsCanonicalInputAfterMappedRomajiDeletion() async throws {
    try await MainActor.run {
        let previousComposingText = composingText
        defer {
            composingText = previousComposingText
        }

        let inputStyle = try makeTemporaryCustomInputStyle(defaultWindowsRomajiRows())
        composingText = ComposingText()
        for character in "buns" {
            composingText.insertAtCursorPosition(String(character), inputStyle: inputStyle)
        }

        var cursor: CInt = 0
        free_c_string(remove_text(cursorPtr: &cursor))
        var rawInput = get_raw_input()
        // The converter materializes the terminal `n` as kana while deleting
        // the pending `s`; this cannot be reproduced with a client-side pop.
        #expect(rawInput.map { String(cString: $0) } == "buん")
        free_c_string(rawInput)

        free_c_string(remove_text(cursorPtr: &cursor))
        rawInput = get_raw_input()
        #expect(rawInput.map { String(cString: $0) } == "bu")
        free_c_string(rawInput)
    }
}

@Test func removeTextCanConsumeMultipleRawElementsInLongComposition() async throws {
    try await MainActor.run {
        let previousComposingText = composingText
        defer {
            composingText = previousComposingText
        }

        // Exact key sequence recovered from the failing VM trace. The user
        // corrected the terminal `bnun` with three Backspaces; the mapped input
        // table removes two internal elements on the second deletion.
        let input = "aruteidonageibunsyoudemofukusuubnun"
        let inputStyle = try makeTemporaryCustomInputStyle(defaultWindowsRomajiRows())
        composingText = ComposingText()
        for character in input {
            composingText.insertAtCursorPosition(String(character), inputStyle: inputStyle)
        }
        #expect(composingText.input.count == 35)

        var cursor: CInt = 0
        free_c_string(remove_text(cursorPtr: &cursor))
        #expect(composingText.input.count == 34)
        free_c_string(remove_text(cursorPtr: &cursor))
        #expect(composingText.input.count == 32)
        free_c_string(remove_text(cursorPtr: &cursor))
        #expect(composingText.input.count == 31)

        let rawInput = get_raw_input()
        defer {
            free_c_string(rawInput)
        }
        #expect(rawInput.map { String(cString: $0).count } == 31)
        #expect(rawInput.map { String(cString: $0) } != String(input.dropLast(3)))
    }
}

@Test func clauseBoundaryAdjustmentSkipsEdgesAndUnknownInputBoundaries() async {
    await MainActor.run {
        let previousComposingText = composingText
        defer {
            composingText = previousComposingText
        }

        composingText = ComposingText()
        composingText.insertAtCursorPosition("tyaibu", inputStyle: .roman2kana)
        let endInputCount = CInt(composingText.input.count)
        let endSurfaceIndex = composingText.convertTargetCursorPosition

        for (currentInputCount, direction) in [
            (endInputCount, CInt(1)),
            (CInt(2), CInt(-1)),
            (CInt(-1), CInt(-1)),
        ] {
            var applied: CInt = 1
            var adjustedInputCount: CInt = -1
            var cursorOffset: CInt = 99
            free_c_string(
                adjust_clause_boundary(
                    currentInputCount: currentInputCount,
                    direction: direction,
                    appliedPtr: &applied,
                    adjustedInputCountPtr: &adjustedInputCount,
                    cursorOffsetPtr: &cursorOffset
                )
            )

            #expect(applied == 0)
            #expect(adjustedInputCount == currentInputCount)
            #expect(cursorOffset == 0)
            #expect(composingText.convertTargetCursorPosition == endSurfaceIndex)
        }

        var applied: CInt = 1
        var adjustedInputCount: CInt = -1
        var cursorOffset: CInt = 99
        "different-input".withCString { expectedRawInputPointer in
            free_c_string(
                adjust_clause_boundary(
                    currentInputCount: 3,
                    direction: 1,
                    expectedRawInput: expectedRawInputPointer,
                    appliedPtr: &applied,
                    adjustedInputCountPtr: &adjustedInputCount,
                    cursorOffsetPtr: &cursorOffset
                )
            )
        }
        #expect(applied == 0)
        #expect(adjustedInputCount == 3)
        #expect(cursorOffset == 0)
        #expect(composingText.convertTargetCursorPosition == endSurfaceIndex)

        var length: CInt = -1
        let candidates = get_composed_text_for_cursor_prefix(
            requiredInputCount: endInputCount - 1,
            lengthPtr: &length
        )
        free_candidate_list(candidates, length)
        #expect(length == 0)
    }
}

@Test func clauseBoundaryCandidatesPreserveTheAdjustedInputBoundary() async throws {
    let packageRoot = packageRootURL()
    let dictionaryURL = packageRoot
        .appending(path: "azooKey_dictionary_storage")
        .appending(path: "Dictionary")

    await MainActor.run {
        let previousConverter = converter
        let previousSupplementConverter = normalNBestSupplementConverter
        let previousComposingText = composingText
        let previousLearningType = currentLearningType
        let previousExecURL = execURL
        defer {
            converter = previousConverter
            normalNBestSupplementConverter = previousSupplementConverter
            composingText = previousComposingText
            currentLearningType = previousLearningType
            execURL = previousExecURL
        }

        converter = KanaKanjiConverter(dictionaryURL: dictionaryURL, preloadDictionary: true)
        normalNBestSupplementConverter = KanaKanjiConverter(
            dictionaryURL: dictionaryURL,
            preloadDictionary: false
        )
        currentLearningType = .nothing
        execURL = packageRoot

        @MainActor func candidateCounts(requiredInputCount: CInt) -> [CInt] {
            var length: CInt = 0
            let list = get_composed_text_for_cursor_prefix(
                requiredInputCount: requiredInputCount,
                lengthPtr: &length
            )
            defer {
                free_candidate_list(list, length)
            }
            return (0..<Int(length)).compactMap { index in
                list.advanced(by: index).pointee?.pointee.correspondingCount
            }
        }

        @MainActor func adjust(
            rawInput: String,
            shrinkOffset: CInt = 0,
            currentInputCount: CInt,
            direction: CInt,
            expectedInputCount: CInt,
            expectedCursorPosition: CInt? = nil
        ) {
            composingText = ComposingText()
            composingText.insertAtCursorPosition(rawInput, inputStyle: .roman2kana)
            if shrinkOffset > 0 {
                free_c_string(shrink_text(offset: shrinkOffset))
            }

            var applied: CInt = 0
            var adjustedInputCount: CInt = -1
            var cursorOffset: CInt = 0
            let expectedRawInput = String(rawInput.dropFirst(Int(shrinkOffset)))
            expectedRawInput.withCString { expectedRawInputPointer in
                free_c_string(
                    adjust_clause_boundary(
                        currentInputCount: currentInputCount,
                        direction: direction,
                        expectedRawInput: expectedRawInputPointer,
                        appliedPtr: &applied,
                        adjustedInputCountPtr: &adjustedInputCount,
                        cursorOffsetPtr: &cursorOffset
                    )
                )
            }

            #expect(applied == 1)
            #expect(adjustedInputCount == expectedInputCount)
            #expect(get_cursor_position() == composingText.convertTargetCursorPosition)
            if let expectedCursorPosition {
                #expect(get_cursor_position() == expectedCursorPosition)
            }
            let counts = candidateCounts(requiredInputCount: expectedInputCount)
            #expect(
                counts.contains(expectedInputCount),
                "expected boundary \(expectedInputCount), candidates: \(counts)"
            )
        }

        let input = "aruteidonagaibunsyoudemofukusuuni"
        adjust(
            rawInput: input,
            currentInputCount: 8,
            direction: -1,
            expectedInputCount: 6,
            expectedCursorPosition: 4
        )
        adjust(
            rawInput: input,
            currentInputCount: 8,
            direction: 1,
            expectedInputCount: 10
        )
        adjust(
            rawInput: input,
            currentInputCount: 10,
            direction: 1,
            expectedInputCount: 12
        )
        adjust(
            rawInput: input,
            shrinkOffset: 8,
            currentInputCount: 16,
            direction: -1,
            expectedInputCount: 14
        )
        adjust(
            rawInput: input,
            shrinkOffset: 8,
            currentInputCount: 16,
            direction: 1,
            expectedInputCount: 18
        )
    }
}

@Test func cursorPrefixActualConverterSelectsTheDisplayedFirstClauseBoundary() async {
    let packageRoot = packageRootURL()
    let dictionaryURL = packageRoot
        .appending(path: "azooKey_dictionary_storage")
        .appending(path: "Dictionary")

    await MainActor.run {
        let previousConverter = converter
        let previousSupplementConverter = normalNBestSupplementConverter
        let previousComposingText = composingText
        let previousLearningType = currentLearningType
        let previousLearningCandidateCache = learningCandidateCache
        let previousConfig = config
        let previousExecURL = execURL
        defer {
            converter = previousConverter
            normalNBestSupplementConverter = previousSupplementConverter
            composingText = previousComposingText
            currentLearningType = previousLearningType
            learningCandidateCache = previousLearningCandidateCache
            config = previousConfig
            execURL = previousExecURL
        }

        converter = KanaKanjiConverter(dictionaryURL: dictionaryURL, preloadDictionary: true)
        normalNBestSupplementConverter = KanaKanjiConverter(
            dictionaryURL: dictionaryURL,
            preloadDictionary: false
        )
        currentLearningType = .nothing
        learningCandidateCache.removeAll()
        config["enable"] = false
        config["backend"] = "cpu"
        config["context"] = ""
        execURL = packageRoot
        composingText = ComposingText()
        composingText.insertAtCursorPosition(
            "aruteidonagaibunsyoudemofukusuubunsetunibunkatusareru",
            inputStyle: .roman2kana
        )

        var length: CInt = 0
        let list = get_composed_text_for_cursor_prefix(
            requiredInputCount: -1,
            lengthPtr: &length
        )
        defer {
            free_candidate_list(list, length)
        }
        let candidates = (0..<Int(length)).compactMap { index -> (String, CInt)? in
            guard let candidate = list.advanced(by: index).pointee?.pointee else {
                return nil
            }
            return (String(cString: candidate.text), candidate.correspondingCount)
        }

        #expect(!candidates.isEmpty)
        #expect(
            Set(candidates.map(\.1)) == [8],
            "first-clause candidates: \(candidates)"
        )
        #expect(candidates.contains { $0.0 == "ある程度" })
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

@Test func zenzaiMergePromotesConfidentDictionaryCorrectionOverRawTypoSpan() async throws {
    let hiragana = "しますた"
    let typoEntry = makeKeyboardTypoDictionaryEntries()[0]
    let correction = Candidate(
        text: typoEntry.word,
        value: -1,
        composingCount: .surfaceCount(typoEntry.ruby.count),
        lastMid: typoEntry.mid,
        data: [typoEntry]
    )
    let zenzaiRaw = testCandidate(
        word: hiragana,
        ruby: typoEntry.ruby,
        composingCount: .surfaceCount(typoEntry.ruby.count)
    )
    var normalRaw = zenzaiRaw
    normalRaw.value = -2

    let merged = mergeZenzaiMainResultsWithNormalNBest(
        zenzaiResults: [zenzaiRaw],
        normalNBestResults: [correction, normalRaw],
        hiragana: hiragana
    )

    #expect(
        merged.map { constructCandidateString(candidate: $0, hiragana: hiragana) } == [
            "しました",
            "しますた",
        ]
    )
}

@Test func zenzaiMergeKeepsModelTopWhenCorrectionIsNotNormalTopOrIsConvertedAlternative() async throws {
    let hiragana = "しますた"
    let typoEntry = makeKeyboardTypoDictionaryEntries()[0]
    let correction = Candidate(
        text: typoEntry.word,
        value: -1,
        composingCount: .surfaceCount(typoEntry.ruby.count),
        lastMid: typoEntry.mid,
        data: [typoEntry]
    )
    let zenzaiRaw = testCandidate(
        word: hiragana,
        ruby: typoEntry.ruby,
        composingCount: .surfaceCount(typoEntry.ruby.count)
    )
    var normalRawTop = zenzaiRaw
    normalRawTop.value = -0.5
    let convertedAlternative = testCandidate(
        word: "済ました",
        ruby: typoEntry.ruby,
        composingCount: .surfaceCount(typoEntry.ruby.count)
    )

    let nonTopCorrectionMerged = mergeZenzaiMainResultsWithNormalNBest(
        zenzaiResults: [zenzaiRaw],
        normalNBestResults: [normalRawTop, correction],
        hiragana: hiragana
    )
    let convertedMerged = mergeZenzaiMainResultsWithNormalNBest(
        zenzaiResults: [convertedAlternative],
        normalNBestResults: [correction, zenzaiRaw],
        hiragana: hiragana
    )

    #expect(constructCandidateString(candidate: nonTopCorrectionMerged[0], hiragana: hiragana) == "しますた")
    #expect(constructCandidateString(candidate: convertedMerged[0], hiragana: hiragana) == "済ました")
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

@Test func keyboardTypoCorrectionDefaultsOffAndFollowsRuntimeConfig() async throws {
    let result = await MainActor.run {
        let previous = config["experimentalTypoCorrection"]
        defer {
            config["experimentalTypoCorrection"] = previous
        }

        config["experimentalTypoCorrection"] = false
        let disabled = getOptions(zenzaiEnabled: false).experimentalKeyboardTypoCorrection
        config["experimentalTypoCorrection"] = true
        let enabled = getOptions(zenzaiEnabled: false).experimentalKeyboardTypoCorrection
        return (disabled, enabled)
    }

    #expect(result.0 == false)
    #expect(result.1)
}

@Test func keyboardTypoCorrectionDictionaryIsOnlyMergedWhenEnabled() {
    let entries = makeKeyboardTypoDictionaryEntries()
    #expect(entries.count == keyboardTypoDictionaryEntryCount)
    #expect(keyboardTypoDictionarySelectionCount == 92)
    #expect(entries.count == 104)
    #expect(entries.contains { $0.ruby == "シマスタ" && $0.word == "しました" })
    #expect(entries.contains { $0.ruby == "ゴカクニ" && $0.word == "ご確認" })
    #expect(Set(entries.map { $0.ruby + "\u{0}" + $0.word }).count == entries.count)
    #expect(entries[0].lcid == 610)
    #expect(entries[0].rcid == 435)
    #expect(entries[0].mid == 17)
    #expect(abs(entries[0].value() - (-8.4169)) < 0.0001)
    #expect(entries[1].lcid == 560)
    #expect(entries[1].rcid == 1283)
    #expect(entries[1].mid == 17)
    #expect(abs(entries[1].value() - (-11.0794)) < 0.0001)

    let userEntry = DicdataElement(
        word: "ユーザー語",
        ruby: "ユーザーゴ",
        cid: CIDData.固有名詞.cid,
        mid: MIDData.一般.mid,
        value: -5
    )
    let disabled = makeConversionDictionaryEntries(
        userEntries: [userEntry],
        experimentalTypoCorrectionEnabled: false
    )
    let enabled = makeConversionDictionaryEntries(
        userEntries: [userEntry],
        experimentalTypoCorrectionEnabled: true
    )
    #expect(disabled.count == 1)
    #expect(enabled.count == 105)
}

@Test func keyboardTypoCorrectionDictionaryCandidatesAreNotLearnedWhileEnabled() {
    let typoEntry = makeKeyboardTypoDictionaryEntries()[0]
    let typoCandidate = Candidate(
        text: typoEntry.word,
        value: typoEntry.value(),
        composingCount: .surfaceCount(typoEntry.ruby.count),
        lastMid: typoEntry.mid,
        data: [typoEntry]
    )
    let ordinaryCandidate = testCandidate(
        word: "通常候補",
        ruby: "ツウジョウコウホ",
        composingCount: .surfaceCount(7)
    )

    let disabled = disableLearningForKeyboardTypoDictionaryCandidates(
        [typoCandidate],
        experimentalTypoCorrectionEnabled: false
    )
    let enabled = disableLearningForKeyboardTypoDictionaryCandidates(
        [typoCandidate, ordinaryCandidate],
        experimentalTypoCorrectionEnabled: true
    )

    #expect(disabled[0].isLearningTarget)
    #expect(!enabled[0].isLearningTarget)
    #expect(enabled[1].isLearningTarget)
}

@Test func keyboardTypoCorrectionProducesDictionaryAndRewriteCandidates() async throws {
    let memoryURL = FileManager.default.temporaryDirectory
        .appending(path: "azookey-typo-correction-test-\(UUID().uuidString)")
    defer { try? FileManager.default.removeItem(at: memoryURL) }

    let results = await MainActor.run {
        (
            dictionary: keyboardTypoCandidateTexts(
                keys: "simasuta",
                enabled: true,
                memoryURL: memoryURL
            ),
            phrase: keyboardTypoCandidateTexts(
                keys: "gokakunionegaisimasu",
                enabled: true,
                memoryURL: memoryURL
            ),
            sentenceShimasuta: keyboardTypoCandidateTexts(
                keys: "iikagentouitusimasuta",
                enabled: true,
                memoryURL: memoryURL
            ),
            sentenceGokakuni: keyboardTypoCandidateTexts(
                keys: "syoruinogokakunionegaisimasu",
                enabled: true,
                memoryURL: memoryURL
            ),
            unresolvedN: keyboardTypoCandidateTexts(
                keys: "fashon",
                enabled: true,
                memoryURL: memoryURL
            ),
            smallTsu: keyboardTypoCandidateTexts(
                keys: "kixtuxtute",
                enabled: true,
                memoryURL: memoryURL
            ),
            doubleNn: keyboardTypoCandidateTexts(
                keys: "konnnnitiha",
                enabled: true,
                memoryURL: memoryURL
            )
        )
    }

    #expect(results.dictionary.contains("しました"), "candidates: \(results.dictionary)")
    #expect(results.phrase.contains("ご確認お願いします"), "candidates: \(results.phrase)")
    #expect(results.sentenceShimasuta.first == "いい加減統一しました", "candidates: \(results.sentenceShimasuta)")
    #expect(results.sentenceGokakuni.first == "書類のご確認お願いします", "candidates: \(results.sentenceGokakuni)")
    #expect(results.unresolvedN.contains("ファッション"), "candidates: \(results.unresolvedN)")
    #expect(results.smallTsu.contains("切って"), "candidates: \(results.smallTsu)")
    #expect(results.doubleNn.contains("こんにちは"), "candidates: \(results.doubleNn)")
}

@Test func keyboardTypoCorrectionCanBeDisabledWithoutReusingEnabledLattice() async throws {
    let memoryURL = FileManager.default.temporaryDirectory
        .appending(path: "azookey-typo-toggle-test-\(UUID().uuidString)")
    defer { try? FileManager.default.removeItem(at: memoryURL) }
    let packageRoot = packageRootURL()
    let dictionaryURL = packageRoot
        .appending(path: "azooKey_dictionary_storage")
        .appending(path: "Dictionary")

    let results = await MainActor.run {
        let testConverter = KanaKanjiConverter(
            dictionaryURL: dictionaryURL,
            preloadDictionary: true
        )
        let enabled = keyboardTypoCandidateTexts(
            keys: "kixtuxtute",
            enabled: true,
            memoryURL: memoryURL,
            converter: testConverter
        )
        let disabled = keyboardTypoCandidateTexts(
            keys: "kixtuxtute",
            enabled: false,
            memoryURL: memoryURL,
            converter: testConverter
        )
        return (enabled, disabled)
    }

    #expect(results.0.contains("切って"), "enabled: \(results.0)")
    #expect(!results.1.contains("切って"), "disabled: \(results.1)")
}

@Test func keyboardTypoCorrectionDoesNotApplyToDirectKanaInput() async throws {
    let memoryURL = FileManager.default.temporaryDirectory
        .appending(path: "azookey-typo-direct-test-\(UUID().uuidString)")
    defer { try? FileManager.default.removeItem(at: memoryURL) }

    let candidates = await MainActor.run {
        let direct = keyboardTypoCandidateTexts(
            keys: "きっって",
            inputStyle: .direct,
            enabled: true,
            memoryURL: memoryURL
        )
        let directDictionary = keyboardTypoCandidateTexts(
            keys: "しますた",
            inputStyle: .direct,
            enabled: true,
            memoryURL: memoryURL
        )
        var romanThenDirect = ComposingText()
        for key in "kixtuxtute" {
            romanThenDirect.insertAtCursorPosition(String(key), inputStyle: .roman2kana)
        }
        romanThenDirect.insertAtCursorPosition("。", inputStyle: .direct)

        var mixedTypo = ComposingText()
        for key in "ki" {
            mixedTypo.insertAtCursorPosition(String(key), inputStyle: .roman2kana)
        }
        mixedTypo.insertAtCursorPosition("っって", inputStyle: .direct)
        return (
            direct,
            directDictionary,
            keyboardTypoCandidateTexts(
                composingText: romanThenDirect,
                enabled: true,
                memoryURL: memoryURL
            ),
            keyboardTypoCandidateTexts(
                composingText: mixedTypo,
                enabled: true,
                memoryURL: memoryURL
            )
        )
    }

    #expect(!candidates.0.contains("切って"), "direct candidates: \(candidates.0)")
    #expect(!candidates.1.contains("しました"), "direct dictionary candidates: \(candidates.1)")
    #expect(candidates.2.contains("切って。"), "mixed suffix candidates: \(candidates.2)")
    #expect(!candidates.3.contains("切って"), "mixed typo candidates: \(candidates.3)")
}

@Test func keyboardTypoCorrectionRewriteEditHistoryBackspace() async throws {
    let memoryURL = FileManager.default.temporaryDirectory
        .appending(path: "azookey-typo-backspace-test-\(UUID().uuidString)")
    defer { try? FileManager.default.removeItem(at: memoryURL) }
    let packageRoot = packageRootURL()
    let dictionaryURL = packageRoot
        .appending(path: "azooKey_dictionary_storage")
        .appending(path: "Dictionary")

    let results = await MainActor.run {
        let testConverter = KanaKanjiConverter(
            dictionaryURL: dictionaryURL,
            preloadDictionary: true
        )
        testConverter.importDynamicUserDictionary(makeKeyboardTypoDictionaryEntries())
        let options = testConvertRequestOptions(
            memoryURL: memoryURL,
            experimentalKeyboardTypoCorrection: true
        )

        func candidateTexts(_ composingText: ComposingText) -> [String] {
            testConverter.requestCandidates(
                composingText,
                options: options
            ).mainResults.map(\.text)
        }

        var smallTsu = ComposingText()
        for key in "kixtuxtute" {
            smallTsu.insertAtCursorPosition(String(key), inputStyle: .roman2kana)
        }
        let smallTsuBefore = candidateTexts(smallTsu)
        smallTsu.deleteBackwardFromCursorPosition(count: 1)
        let smallTsuAfter = candidateTexts(smallTsu)
        return (smallTsuBefore, smallTsuAfter, smallTsu)
    }

    #expect(results.0.contains("切って"), "before Backspace: \(results.0)")
    #expect(results.2.convertTarget == "きっっ")
    #expect(results.2.convertTargetCursorPosition == 3)
    #expect(results.2.input.count == 8)
    #expect(
        results.2.input == "kixtuxtu".map {
            ComposingText.InputElement(character: $0, inputStyle: .roman2kana)
        }
    )
    #expect(!results.1.contains("切って"), "after Backspace: \(results.1)")
}

@Test func keyboardTypoCorrectionRewriteEditHistoryCursorDelete() async throws {
    let memoryURL = FileManager.default.temporaryDirectory
        .appending(path: "azookey-typo-cursor-delete-test-\(UUID().uuidString)")
    defer { try? FileManager.default.removeItem(at: memoryURL) }
    let packageRoot = packageRootURL()
    let dictionaryURL = packageRoot
        .appending(path: "azooKey_dictionary_storage")
        .appending(path: "Dictionary")

    let results = await MainActor.run {
        let testConverter = KanaKanjiConverter(
            dictionaryURL: dictionaryURL,
            preloadDictionary: true
        )
        testConverter.importDynamicUserDictionary(makeKeyboardTypoDictionaryEntries())
        let options = testConvertRequestOptions(
            memoryURL: memoryURL,
            experimentalKeyboardTypoCorrection: true
        )

        func candidateTexts(_ composingText: ComposingText) -> [String] {
            testConverter.requestCandidates(
                composingText,
                options: options
            ).mainResults.map(\.text)
        }

        var doubleNn = ComposingText()
        for key in "konnnnitiha" {
            doubleNn.insertAtCursorPosition(String(key), inputStyle: .roman2kana)
        }
        let before = candidateTexts(doubleNn)
        let cursorDelta = doubleNn.moveCursorFromCursorPosition(count: -4)
        doubleNn.deleteForwardFromCursorPosition(count: 1)
        // Cursor-prefix conversion is the production path while the caret is
        // inside the composition; the suffix remains visible but unconverted.
        let after = candidateTexts(doubleNn.prefixToCursorPosition())
        return (before, after, doubleNn, cursorDelta)
    }

    #expect(results.0.contains("こんにちは"), "before cursor Delete: \(results.0)")
    #expect(results.3 == -4)
    #expect(results.2.convertTarget == "こんいちは")
    #expect(results.2.convertTargetCursorPosition == 2)
    #expect(results.2.input.count == 10)
    #expect(
        results.2.input == [
            ComposingText.InputElement(character: "k", inputStyle: .roman2kana),
            ComposingText.InputElement(character: "o", inputStyle: .roman2kana),
            ComposingText.InputElement(character: "n", inputStyle: .roman2kana),
            ComposingText.InputElement(character: "n", inputStyle: .roman2kana),
            ComposingText.InputElement(
                piece: .compositionSeparator,
                inputStyle: .mapped(id: .empty)
            ),
            ComposingText.InputElement(character: "i", inputStyle: .roman2kana),
            ComposingText.InputElement(character: "t", inputStyle: .roman2kana),
            ComposingText.InputElement(character: "i", inputStyle: .roman2kana),
            ComposingText.InputElement(character: "h", inputStyle: .roman2kana),
            ComposingText.InputElement(character: "a", inputStyle: .roman2kana),
        ]
    )
    #expect(!results.1.contains("こんにちは"), "after cursor Delete: \(results.1)")
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
    let result = try await MainActor.run {
        let inputStyle = try makeTemporaryCustomInputStyle(rows)
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
    let convertTarget = try await MainActor.run {
        let inputStyle = try makeTemporaryCustomInputStyle(rows)
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
    let preview = try await MainActor.run {
        let inputStyle = try makeTemporaryCustomInputStyle(rows)
        var source = ComposingText()
        source.insertAtCursorPosition("q", inputStyle: inputStyle)
        return makeCandidatePreviewComposingText(from: source)
    }

    #expect(preview.syntheticEndOfText == false)
    #expect(preview.composingText.convertTarget == "q")
}

@Test func backgroundWarmupAvoidsSharedInputStyleRegistry() async {
    let metrics = await MainActor.run {
        let disabled = makeBackgroundWarmupComposingText(zenzaiRuntimeEnabled: false)
        let enabled = makeBackgroundWarmupComposingText(zenzaiRuntimeEnabled: true)
        return (disabled: disabled, enabled: enabled)
    }

    #expect(metrics.disabled.convertTarget == "あ")
    #expect(metrics.enabled.convertTarget == "にほんご")
    #expect(metrics.disabled.input.count == 1)
    #expect(metrics.enabled.input.count == 4)
    #expect(metrics.disabled.input.allSatisfy { $0.inputStyle == .direct })
    #expect(metrics.enabled.input.allSatisfy { $0.inputStyle == .direct })
    #expect(
        effectiveZenzaiEnabledForCandidates(
            isConfigured: true,
            inputCount: metrics.disabled.input.count,
            hiraganaCount: metrics.disabled.convertTarget.count
        ) == false
    )
    #expect(
        effectiveZenzaiEnabledForCandidates(
            isConfigured: true,
            inputCount: metrics.enabled.input.count,
            hiraganaCount: metrics.enabled.convertTarget.count
        )
    )
    #expect(backgroundWarmupPreloadsDictionary == false)
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

@Test func cursorPrefixCandidatesRejectBoundaryThatConsumesPastDisplayedRuby() async throws {
    let boundary = await MainActor.run {
        var source = ComposingText()
        source.insertAtCursorPosition(
            "aruteidonagaibunsyoudemofukusuubunsetunibunkatusareru",
            inputStyle: .roman2kana
        )
        let preview = makeCandidatePreviewComposingText(from: source).composingText
        let overconsuming = testCandidate(
            word: "ある程度",
            ruby: "あるていど",
            composingCount: .inputCount(10)
        )
        let displayAligned = testCandidate(
            word: "ある程度",
            ruby: "あるていど",
            composingCount: .inputCount(8)
        )

        return cursorPrefixFirstClauseCorrespondingCount(
            firstClauseResults: [overconsuming, displayAligned],
            originalComposingText: source,
            previewComposingText: preview
        )
    }

    #expect(boundary == 8)
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
