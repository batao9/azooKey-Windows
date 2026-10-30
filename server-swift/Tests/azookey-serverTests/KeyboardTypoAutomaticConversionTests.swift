import Foundation
import KanaKanjiConverterModule
import Testing
@testable import azookey_server

private let typoTestPackageRoot = URL(fileURLWithPath: #filePath)
    .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()

@MainActor private func withTypoTestServer(
    inputStyle: InputStyle? = nil,
    zenzaiModel: URL? = nil,
    _ body: () throws -> Void
) throws {
    let savedConverter = converter
    let savedSupplement = normalNBestSupplementConverter
    let savedComposition = composingText
    let savedSnapshots = composingTextSnapshots
    let savedStyle = currentInputStyle
    let savedConfig = config
    let savedExecURL = execURL
    let savedLearning = currentLearningType
    let savedMemory = currentLearningMemoryDirectoryURL
    let savedCache = learningCandidateCache
    let memory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    try FileManager.default.createDirectory(at: memory, withIntermediateDirectories: true)
    defer {
        converter = savedConverter
        normalNBestSupplementConverter = savedSupplement
        composingText = savedComposition
        composingTextSnapshots = savedSnapshots
        currentInputStyle = savedStyle
        config = savedConfig
        execURL = savedExecURL
        currentLearningType = savedLearning
        currentLearningMemoryDirectoryURL = savedMemory
        learningCandidateCache = savedCache
        try? FileManager.default.removeItem(at: memory)
    }
    let tableURL = typoTestPackageRoot.deletingLastPathComponent()
        .appendingPathComponent("crates/shared/src/default_romaji_table.txt")
    let rows = try String(contentsOf: tableURL, encoding: .utf8)
        .split(whereSeparator: \.isNewline).compactMap { line -> RomajiTableRow? in
            guard !line.hasPrefix("#") else { return nil }
            let fields = line.split(separator: "\t", omittingEmptySubsequences: false)
            guard fields.count >= 2, !fields[0].isEmpty, !fields[1].isEmpty else { return nil }
            return RomajiTableRow(input: String(fields[0]), output: String(fields[1]),
                next_input: fields.count > 2 ? String(fields[2]) : nil)
        }
    let compiledURL = memory.appendingPathComponent("romaji.tsv")
    try #require(buildCustomRomajiTableContent(rows: rows)).write(to: compiledURL, atomically: true, encoding: .utf8)
    let name = "automatic-typo-regression-\(UUID().uuidString)"
    InputStyleManager.registerInputStyle(table: try InputStyleManager.loadTable(from: compiledURL), for: name)
    currentInputStyle = inputStyle ?? .mapped(id: .tableName(name))
    let dictionary = typoTestPackageRoot.appendingPathComponent("azooKey_dictionary_storage/Dictionary")
    converter = KanaKanjiConverter(dictionaryURL: dictionary, preloadDictionary: true)
    normalNBestSupplementConverter = KanaKanjiConverter(dictionaryURL: dictionary)
    let entries = makeKeyboardTypoDictionaryEntries()
    converter.importDynamicUserDictionary(entries)
    normalNBestSupplementConverter.importDynamicUserDictionary(entries)
    composingText = ComposingText()
    composingTextSnapshots = []
    config = ["enable": zenzaiModel != nil, "backend": "cpu", "experimentalTypoCorrection": true]
    execURL = zenzaiModel?.deletingLastPathComponent() ?? typoTestPackageRoot
    currentLearningType = .nothing
    currentLearningMemoryDirectoryURL = memory
    learningCandidateCache.removeAll()
    try body()
}

private struct TypoTestCandidates {
    let texts: [String]
    let visible: [String]
    let reading: String
    let counts: [Int32]
}

@MainActor private func typoTestCandidates(cursorPrefix: Bool = false, requiredInputCount: CInt? = nil) -> TypoTestCandidates {
    var length: CInt = 0
    let list = cursorPrefix
        ? get_composed_text_for_cursor_prefix(requiredInputCount: requiredInputCount ?? Int32(composingText.prefixToCursorPosition().input.count), lengthPtr: &length)
        : get_composed_text(lengthPtr: &length)
    defer { free_candidate_list(list, length) }
    var texts: [String] = []
    var visible: [String] = []
    var counts: [Int32] = []
    var reading = ""
    for index in 0 ..< Int(length) {
        guard let value = list[index]?.pointee else { continue }
        let text = value.text.map { String(cString: $0) } ?? ""
        texts.append(text)
        visible.append(text + (value.subtext.map { String(cString: $0) } ?? ""))
        counts.append(value.correspondingCount)
        if index == 0 { reading = value.hiragana.map { String(cString: $0) } ?? "" }
    }
    return TypoTestCandidates(texts: texts, visible: visible, reading: reading, counts: counts)
}

@MainActor private func typeTypoTestKeys(_ keys: String, each: (TypoTestCandidates) -> Void = { _ in }) {
    for key in keys {
        var cursor: CInt = 0
        String(key).withCString { free_c_string(append_text(input: $0, cursorPtr: &cursor)) }
        each(typoTestCandidates())
    }
}

@Suite struct KeyboardTypoAutomaticConversionTests {
    @Test(.enabled(if: ProcessInfo.processInfo.environment["AZOOKEY_ZENZAI_TEST_MODEL"] != nil))
    @MainActor func realZenzaiPreservesRequiredCorrectionsAndProtectsTypingHistory() throws {
        let path = try #require(ProcessInfo.processInfo.environment["AZOOKEY_ZENZAI_TEST_MODEL"])
        let model = URL(fileURLWithPath: path)
        try #require(model.lastPathComponent == "zenz.gguf" && FileManager.default.fileExists(atPath: path))
        try withTypoTestServer(zenzaiModel: model) {
            for (keys, expectedPrefix) in [
                ("simasuta", "しました"),
                ("gokakuniyorosiku", "ご確認"),
                ("syoruinogokakuniyorosiku", "書類のご確認"),
                ("iikagentouitusimasuta", "いい加減統一しました"),
                ("kyuriwotaberu", "きゅうりを"),
                ("minnadesimasuta", "みんなでしました"),
                ("minnadegokakuniyorosiku", "みんなでご確認"),
            ] {
                clear_text()
                typeTypoTestKeys(keys)
                let result = typoTestCandidates()
                #expect(result.visible.first?.hasPrefix(expectedPrefix) == true, "\(keys): \(result.visible.prefix(5))")
                #expect(result.counts.first == Int32(composingText.input.count))
                // Reject a silent fallback to ordinary KKC when model loading
                // failed or the runtime disabled CPU inference.
                #expect(converter.zenzStatus == "load \(model.absoluteString)", "model status: \(converter.zenzStatus)")
                print("REAL_ZENZAI keys=\(keys) reading=\(result.reading) top=\(result.visible.first ?? "") status=\(converter.zenzStatus)")
            }
            for (keys, forbidden) in [
                ("sekyurithi", "きゅうり"), ("kyuriumu", "きゅうり"),
                ("ha-kyuri-zu", "きゅうり"), ("atanasiusu", "あなた"),
                ("dekisutorin", "できず"), ("osoyou", "およそ"),
            ] {
                clear_text()
                typeTypoTestKeys(keys) { result in
                    #expect(result.visible.first?.contains(forbidden) == false,
                        "real model \(keys), \(result.reading): \(result.visible.prefix(5))")
                }
                let result = typoTestCandidates()
                #expect(converter.zenzStatus == "load \(model.absoluteString)")
                print("REAL_ZENZAI negative=\(keys) reading=\(result.reading) top=\(result.visible.first ?? "")")
            }
            clear_text()
            typeTypoTestKeys("sekyurithi")
            _ = composingText.moveCursorFromCursorPosition(count: -2)
            let prefix = typoTestCandidates(cursorPrefix: true, requiredInputCount: -1)
            #expect(prefix.visible.first?.contains("きゅうり") == false)
            #expect(composingText.input.count == 10)
            _ = composingText.moveCursorFromCursorPosition(count: 2)
            for _ in 0 ..< 2 {
                var cursor: CInt = 0
                free_c_string(remove_text(cursorPtr: &cursor))
                #expect(typoTestCandidates().visible.first?.contains("きゅうり") == false)
            }
            #expect(composingText.convertTarget == "せきゅり")
            #expect(composingText.input.count == 7)
            typeTypoTestKeys("thi")
            #expect(typoTestCandidates().visible.first == "セキュリティ")
            print("REAL_ZENZAI editing=cursor-backspace-reentry reading=\(composingText.convertTarget) status=\(converter.zenzStatus)")
        }
    }

    @Test @MainActor func preservesDictionaryCorrectionsAsSelectableCandidates() throws {
        let source = typoTestPackageRoot.deletingLastPathComponent().appendingPathComponent("data/keyboard-typo-dictionary.tsv")
        let lines = try String(contentsOf: source, encoding: .utf8).split(whereSeparator: \.isNewline)
        let columns = try #require(lines.first).split(separator: "\t").map(String.init)
        let evaluation = ProcessInfo.processInfo.environment["AZOOKEY_TYPO_EVALUATION"] == "1"
        try withTypoTestServer {
            for line in lines.dropFirst() {
                let row = Dictionary(uniqueKeysWithValues: zip(columns, line.split(separator: "\t", omittingEmptySubsequences: false).map(String.init)))
                let keys = try #require(row["typed_keys"])
                let expected = try #require(row["expected_candidate"])
                clear_text()
                let start = ProcessInfo.processInfo.systemUptime
                typeTypoTestKeys(keys)
                let result = typoTestCandidates()
                let elapsed = (ProcessInfo.processInfo.systemUptime - start) * 1_000
                #expect(result.texts.contains(expected), "id=\(row["id"] ?? ""), \(keys): \(result.texts)")
                #expect((result.texts.firstIndex(of: expected) ?? .max) < 4,
                    "correction must remain readily selectable: \(keys): \(result.texts)")
                if evaluation {
                    let record: [String: Any] = ["id": row["id"] ?? "", "keys": keys, "expected": expected,
                        "reading": result.reading, "top": result.visible.first ?? "", "rank": result.texts.firstIndex(of: expected).map { $0 + 1 } ?? -1,
                        "sequenceMs": elapsed]
                    let encoded = try JSONSerialization.data(withJSONObject: record, options: [.sortedKeys])
                    print("TYPO_EVALUATION " + String(decoding: encoded, as: UTF8.self))
                }
            }
        }
    }

    @Test @MainActor func preservesRequiredAndContextualCorrections() throws {
        try withTypoTestServer {
            for (keys, expected) in [
                ("simasuta", "しました"),
                ("gokakuniyorosiku", "ご確認よろしく"),
                ("syoruinogokakuniyorosiku", "書類のご確認よろしく"),
                ("iikagentouitusimasuta", "いい加減統一しました"),
                ("gokakunionegaisimasu", "ご確認お願いします"),
                ("kyuriwotaberu", "きゅうりを食べる"),
                ("kyukeiwotoru", "休憩を取る"),
                ("tyugokuniiku", "中国に行く"),
                ("donna", "どんな"),
                ("onna", "おんな"),
                ("minnadesimasuta", "みんなでしました"),
                ("minnadegokakuniyorosiku", "みんなでご確認よろしく"),
            ] {
                clear_text()
                typeTypoTestKeys(keys)
                let result = typoTestCandidates()
                #expect(result.visible.first == expected, "\(keys): \(result.visible.prefix(5))")
                #expect(result.reading == makeCandidatePreviewComposingText(from: composingText).composingText.convertTarget)
                #expect(result.counts.first == Int32(composingText.input.count))
            }
        }
    }

    @Test @MainActor func protectsEveryKeystrokeOfOrdinaryWords() throws {
        try withTypoTestServer {
            for (keys, forbidden, completed) in [
                ("sekyurithi", "きゅうり", "セキュリティ"),
                ("kyuriumu", "きゅうり", "キュリウム"),
                ("ha-kyuri-zu", "きゅうり", "ハーキュリーズ"),
                ("atanasiusu", "あなた", "アタナシウス"),
                ("dekisutorin", "できず", "デキストリン"),
                ("osoyou", "およそ", "おそよう"),
            ] {
                clear_text()
                typeTypoTestKeys(keys) { result in
                    #expect(result.visible.first?.contains(forbidden) == false,
                        "\(keys), reading=\(result.reading): \(result.visible.prefix(5))")
                }
                #expect(typoTestCandidates().visible.first == completed)
            }
        }
    }

    @Test @MainActor func preservesRawInputAndSuggestionsAcrossDeletionAndCursorPrefix() throws {
        try withTypoTestServer {
            typeTypoTestKeys("sekyuri")
            let partial = typoTestCandidates()
            #expect(partial.reading == "せきゅり")
            #expect(partial.texts.first?.contains("きゅうり") == false)
            #expect(partial.texts.contains("背きゅうり"))
            #expect((partial.texts.firstIndex(of: "背きゅうり") ?? .max) <= 3)
            #expect(composingText.input.count == 7)
            typeTypoTestKeys("thi")
            #expect(typoTestCandidates().visible.first == "セキュリティ")
            _ = composingText.moveCursorFromCursorPosition(count: -2)
            let prefix = typoTestCandidates(cursorPrefix: true)
            #expect(prefix.reading == "せきゅりてぃ")
            #expect(prefix.texts.first?.contains("きゅうり") == false)
            #expect(prefix.counts.allSatisfy { $0 == 7 })
            let automaticBoundary = typoTestCandidates(cursorPrefix: true, requiredInputCount: -1)
            #expect(!automaticBoundary.visible.isEmpty)
            #expect(automaticBoundary.visible.first?.contains("きゅうり") == false)
            #expect(automaticBoundary.counts.allSatisfy { $0 > 0 && $0 <= 7 })
            #expect(composingText.input.count == 10)
            _ = composingText.moveCursorFromCursorPosition(count: 2)
            for _ in 0 ..< 2 {
                var cursor: CInt = 0
                free_c_string(remove_text(cursorPtr: &cursor))
                _ = typoTestCandidates()
            }
            #expect(composingText.convertTarget == "せきゅり")
            #expect(composingText.input.count == 7)
            #expect(typoTestCandidates().visible.first?.contains("きゅうり") == false)
            typeTypoTestKeys("thi")
            #expect(typoTestCandidates().visible.first == "セキュリティ")
        }
    }

    @Test func zenzaiDoesNotPromoteAmbiguousDictionaryCorrection() throws {
        let entry = try #require(makeKeyboardTypoDictionaryEntries().first { $0.ruby == "キュリ" })
        let prefix = DicdataElement(word: "背", ruby: "セ", lcid: 1285, rcid: 1285, mid: 333, value: -1)
        let corrected = Candidate(text: "背きゅうり", value: -1, composingCount: .surfaceCount(4), lastMid: entry.mid,
            data: [prefix, entry])
        let literal = Candidate(text: "セキュリ", value: -2, composingCount: .surfaceCount(4), lastMid: 501,
            data: [DicdataElement(word: "セキュリ", ruby: "セキュリ", lcid: 1285, rcid: 1285, mid: 501, value: -2)])
        let merged = mergeZenzaiMainResultsWithNormalNBest(zenzaiResults: [literal],
            normalNBestResults: [corrected, literal], hiragana: "せきゅり")
        #expect(merged.first?.text == "セキュリ")
        #expect(merged.contains { $0.text == "背きゅうり" })
    }

    @Test func zenzaiDictionaryConfidenceDistinguishesSplitFragmentsFromRealWords() throws {
        let entry = try #require(makeKeyboardTypoDictionaryEntries().first { $0.ruby == "ゴカクニ" })
        func element(_ word: String, _ ruby: String) -> DicdataElement {
            DicdataElement(word: word, ruby: ruby, lcid: 1285, rcid: 1285, mid: 501, value: -1)
        }
        func candidate(_ data: [DicdataElement]) -> Candidate {
            Candidate(text: data.map(\.word).joined(), value: -1,
                composingCount: .surfaceCount(data.reduce(0) { $0 + $1.ruby.count }), lastMid: 501, data: data)
        }
        // Exact model failures observed with one-key-at-a-time Windows input.
        for prefix in [[], [element("書類", "ショルイ"), element("の", "ノ")]] {
            let suffix = element("よろしく", "ヨロシク")
            let corrected = candidate(prefix + [entry, suffix])
            let splitModel = candidate(prefix + [element("ご格", "ゴカク"), element("に", "ニ"), suffix])
            let reading = prefix.isEmpty ? "ごかくによろしく" : "しょるいのごかくによろしく"
            let merged = mergeZenzaiMainResultsWithNormalNBest(zenzaiResults: [splitModel],
                normalNBestResults: [corrected], hiragana: reading)
            #expect(merged.first?.text == corrected.text)
            // A whole lexical interpretation of the original span is not
            // interchangeable with the unrelated fragments above.
            let lexicalModel = candidate(prefix + [element("御角煮", "ゴカクニ"), suffix])
            let lexicalMerged = mergeZenzaiMainResultsWithNormalNBest(zenzaiResults: [lexicalModel],
                normalNBestResults: [corrected], hiragana: reading)
            #expect(lexicalMerged.first?.text == lexicalModel.text)
            let largerModel = candidate(prefix + [element("ユーザー語", "ゴカクニヨロシク")])
            #expect(mergeZenzaiMainResultsWithNormalNBest(zenzaiResults: [largerModel],
                normalNBestResults: [corrected], hiragana: reading).first?.text == largerModel.text)
        }
    }

    @Test func checksActualCorrectionOffsetAndLimitsAutomaticCorrectionCount() throws {
        let entry = try #require(makeKeyboardTypoDictionaryEntries().first { $0.ruby == "キュリ" })
        let normalization = try #require(makeKeyboardTypoDictionaryEntries().first { $0.ruby == "ミンア" })
        let required = try #require(makeKeyboardTypoDictionaryEntries().first { $0.ruby == "シマスタ" })
        func literal(_ text: String) -> DicdataElement {
            DicdataElement(word: text, ruby: text, lcid: 1285, rcid: 1285, mid: 501, value: -1)
        }
        func candidate(_ data: [DicdataElement]) -> Candidate {
            Candidate(text: data.map(\.word).joined(), value: -1,
                composingCount: .surfaceCount(data.reduce(0) { $0 + $1.ruby.count }), lastMid: 501, data: data)
        }
        // Only the second キュリ is corrected. A lexical match at the first
        // occurrence must not suppress the unambiguous second occurrence.
        let secondOnly = candidate([literal("セキュリト"), entry, literal("ヲタベル")])
        #expect(KeyboardTypoAutomaticConversion(hiragana: "せきゅりときゅりをたべる").allowsAutomaticSelection(secondOnly))
        #expect(!KeyboardTypoAutomaticConversion(hiragana: "きゅり-").allowsAutomaticSelection(candidate([entry])))
        let normalizedAndCorrected = candidate([normalization, literal("デ"), required])
        #expect(KeyboardTypoAutomaticConversion(hiragana: "みんあでしますた").allowsAutomaticSelection(normalizedAndCorrected))
        #expect(keyboardTypoDictionaryCorrectionForAutomaticSelection(in: normalizedAndCorrected)?.entry.word == "しました")
        #expect(keyboardTypoDictionaryCorrectionForAutomaticSelection(in: normalizedAndCorrected)?.originalSurfaceRange == 4 ..< 8)
        let both = candidate([entry, literal("ト"), entry])
        let original = candidate([literal("キュリトキュリ")])
        let ranked = prioritizeAutomaticKeyboardTypoCandidates([both, original], hiragana: "きゅりときゅり") { $0 }
        #expect(ranked.map(\.text) == [original.text, both.text])
        let merged = mergeZenzaiMainResultsWithNormalNBest(zenzaiResults: [original],
            normalNBestResults: [both, original], hiragana: "きゅりときゅり")
        #expect(merged.first?.text == original.text)
        var mixed = secondOnly
        mixed.keyboardTypoCorrections = [.init(kind: .smallTsu, originalSurfaceRange: 0 ..< 4)]
        #expect(!KeyboardTypoAutomaticConversion(hiragana: "せきゅりときゅりをたべる").allowsAutomaticSelection(mixed))
    }

    @Test @MainActor func protectsBuiltInRomanFallbackWithoutChangingRawInput() throws {
        try withTypoTestServer(inputStyle: .roman2kana) {
            for keys in ["kyuri-", "ha-kyuri-zu"] {
                clear_text()
                typeTypoTestKeys(keys) { result in
                    #expect(result.visible.first?.contains("きゅうり") == false, "\(keys): \(result.visible.prefix(5))")
                }
                #expect(composingText.input == keys.map { ComposingText.InputElement(character: $0, inputStyle: .roman2kana) })
                #expect(composingText.convertTarget.contains("-"))
            }
            for (keys, expected) in [("simasuta", "しました"), ("gokakuniyorosiku", "ご確認よろしく")] {
                clear_text()
                typeTypoTestKeys(keys)
                #expect(typoTestCandidates().visible.first == expected)
            }
        }
    }
}
