import Foundation
import KanaKanjiConverterModule
import Testing
@testable import azookey_server

private func emojiFixtureDirectory() throws -> URL {
    let directory = FileManager.default.temporaryDirectory
        .appendingPathComponent("azookey-emoji-cache-\(UUID().uuidString)", isDirectory: true)
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    return directory
}

private func writeEmojiFixture(_ emoji: String, to url: URL) throws {
    try "\(emoji)\tえがお\t\n".write(to: url, atomically: true, encoding: .utf8)
}

private func emojiResults(_ replacer: TextReplacer) -> [String] {
    replacer.getSearchResult(query: "えがお", target: [.emoji]).map(\.text)
}

@Test func emojiReplacerReusesLoadedResourceWithoutReadingEachRequest() async throws {
    let directory = try emojiFixtureDirectory()
    defer { try? FileManager.default.removeItem(at: directory) }
    let url = directory.appendingPathComponent("emoji.txt")
    try writeEmojiFixture("😀", to: url)
    var cache = EmojiTextReplacerCache()
    let snapshot = cache.value(for: url)
    #expect(emojiResults(snapshot) == ["😀"])
    try FileManager.default.removeItem(at: url)
    for _ in 0..<64 {
        #expect(emojiResults(cache.value(for: url)) == ["😀"])
    }
    cache.invalidate()
    let backgroundResults = await Task.detached {
        emojiResults(snapshot)
    }.value
    #expect(backgroundResults == ["😀"])
}

@Test func emojiReplacerChangesResourceAndRefreshesOnExplicitReload() throws {
    let directory = try emojiFixtureDirectory()
    defer { try? FileManager.default.removeItem(at: directory) }
    let first = directory.appendingPathComponent("first.txt")
    let second = directory.appendingPathComponent("second.txt")
    try writeEmojiFixture("😀", to: first)
    try writeEmojiFixture("😄", to: second)
    var cache = EmojiTextReplacerCache()
    #expect(emojiResults(cache.value(for: first)) == ["😀"])
    #expect(emojiResults(cache.value(for: second)) == ["😄"])
    try writeEmojiFixture("😁", to: second)
    #expect(emojiResults(cache.value(for: second)) == ["😄"])
    cache.invalidate()
    #expect(emojiResults(cache.value(for: second)) == ["😁"])
}

@Test func emojiReplacerRetriesAnUnavailableResourceOnExplicitReload() throws {
    let directory = try emojiFixtureDirectory()
    defer { try? FileManager.default.removeItem(at: directory) }
    let url = directory.appendingPathComponent("missing.txt")
    var cache = EmojiTextReplacerCache()
    #expect(emojiResults(cache.value(for: url)).isEmpty)
    try writeEmojiFixture("😀", to: url)
    #expect(emojiResults(cache.value(for: url)).isEmpty)
    cache.invalidate()
    #expect(emojiResults(cache.value(for: url)) == ["😀"])
}

@Test func emojiReplacerPreservesEveryPackagedQueryAndVariation() throws {
    let packageRoot = URL(filePath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
    let url = packageRoot.appendingPathComponent("azooKey_emoji_dictionary_storage")
        .appendingPathComponent("EmojiDictionary/emoji_all_E15.1.txt")
    let lines = try String(contentsOf: url, encoding: .utf8)
        .split(whereSeparator: \.isNewline)
    let fresh = TextReplacer(emojiDataProvider: { url })
    var cache = EmojiTextReplacerCache()
    let cached = cache.value(for: url)
    var queries = Set<String>()
    for line in lines {
        let columns = line.split(separator: "\t", omittingEmptySubsequences: false)
        #expect(columns.count == 3)
        guard columns.count == 3 else { continue }
        queries.formUnion(columns[1].split(separator: ",").map(String.init))
        let base = String(columns[0])
        for (left, center) in [(base, ""), ("", base)] {
            let expected = fresh.getReplacementCandidate(left: left, center: center, right: "", target: [.emoji])
                .map { $0.target + "\t" + $0.text }
            let actual = cached.getReplacementCandidate(left: left, center: center, right: "", target: [.emoji])
                .map { $0.target + "\t" + $0.text }
            #expect(actual == expected)
        }
    }
    #expect(!queries.isEmpty)
    for query in queries {
        for ignoreNonBase in [false, true] {
            #expect(cached.getSearchResult(query: query, target: [.emoji], ignoreNonBaseEmoji: ignoreNonBase)
                == fresh.getSearchResult(query: query, target: [.emoji], ignoreNonBaseEmoji: ignoreNonBase))
        }
    }
}

@Test func emojiReplacerOptionsReuseResourceWithoutFreezingDynamicSettings() async throws {
    let directory = try emojiFixtureDirectory()
    defer { try? FileManager.default.removeItem(at: directory) }
    let emojiDirectory = directory.appendingPathComponent("EmojiDictionary", isDirectory: true)
    try FileManager.default.createDirectory(at: emojiDirectory, withIntermediateDirectories: true)
    let url = emojiDirectory.appendingPathComponent("emoji_all_E15.1.txt")
    try writeEmojiFixture("😀", to: url)

    try await MainActor.run {
        let previousURL = execURL
        let previousConfig = config
        let previousLearning = currentLearningType
        let previousMemory = currentLearningMemoryDirectoryURL
        let previousCache = emojiTextReplacerCache
        defer {
            execURL = previousURL
            config = previousConfig
            currentLearningType = previousLearning
            currentLearningMemoryDirectoryURL = previousMemory
            emojiTextReplacerCache = previousCache
        }
        execURL = directory
        emojiTextReplacerCache.invalidate()
        config["backend"] = "cpu"
        config["profile"] = "first"
        config["experimentalTypoCorrection"] = false
        currentLearningType = .nothing
        let first = getOptions(context: "before", zenzaiEnabled: true)
        #expect(emojiResults(first.textReplacer) == ["😀"])
        #expect(first.zenzaiMode == .on(
            weight: directory.appendingPathComponent("zenz.gguf"), inferenceLimit: 1,
            requestRichCandidates: true, personalizationMode: nil,
            versionDependentMode: .v3(.init(profile: "first", leftSideContext: "before"))
        ))
        try FileManager.default.removeItem(at: url)
        config["profile"] = "second"
        config["experimentalTypoCorrection"] = true
        currentLearningType = .onlyOutput
        currentLearningMemoryDirectoryURL = directory.appendingPathComponent("new-memory")
        let second = getOptions(context: "after", zenzaiEnabled: true)
        #expect(emojiResults(second.textReplacer) == ["😀"])
        #expect(second.experimentalKeyboardTypoCorrection)
        #expect(second.learningType == .onlyOutput)
        #expect(second.memoryDirectoryURL == currentLearningMemoryDirectoryURL)
        #expect(second.zenzaiMode == .on(
            weight: directory.appendingPathComponent("zenz.gguf"), inferenceLimit: 1,
            requestRichCandidates: true, personalizationMode: nil,
            versionDependentMode: .v3(.init(profile: "second", leftSideContext: "after"))
        ))
    }
}
