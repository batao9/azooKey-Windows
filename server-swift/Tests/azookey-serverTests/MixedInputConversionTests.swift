import Foundation
import ffi
import KanaKanjiConverterModule
import Testing
@testable import azookey_server

@MainActor private func withMixedInputServer(
    enabled: Bool = true,
    realDictionary: Bool = false,
    inputStyle: InputStyle = .roman2kana,
    inputTable: InputTable = .defaultRomanToKana,
    _ body: () throws -> Void
) rethrows {
    let savedText = composingText
    let savedSnapshots = composingTextSnapshots
    let savedStyle = currentInputStyle
    let savedTable = currentRomajiInputTable
    let savedConfig = config
    let savedConverter = converter
    let savedSupplement = normalNBestSupplementConverter
    let savedLearning = currentLearningType
    let savedExecURL = execURL
    let savedLearningCache = learningCandidateCache
    defer {
        composingText = savedText
        composingTextSnapshots = savedSnapshots
        currentInputStyle = savedStyle
        currentRomajiInputTable = savedTable
        config = savedConfig
        converter = savedConverter
        normalNBestSupplementConverter = savedSupplement
        currentLearningType = savedLearning
        execURL = savedExecURL
        learningCandidateCache = savedLearningCache
    }
    composingText = ComposingText()
    composingTextSnapshots = []
    currentInputStyle = inputStyle
    currentRomajiInputTable = inputTable
    currentLearningType = .nothing
    learningCandidateCache.removeAll()
    config["enable"] = false
    config["context"] = ""
    config["experimentalTypoCorrection"] = false
    config["experimentalMixedInput"] = enabled
    if realDictionary {
        let root = URL(filePath: #filePath).deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
        let dictionary = root.deletingLastPathComponent().appending(path: "target/azookey-english-dictionary/Dictionary")
        converter = KanaKanjiConverter(dictionaryURL: dictionary, preloadDictionary: true)
        normalNBestSupplementConverter = KanaKanjiConverter(dictionaryURL: dictionary, preloadDictionary: false)
        execURL = root
    }
    try body()
}

@MainActor private func typeMixedInput(_ keys: String) {
    var cursor: CInt = 0
    for key in keys {
        String(key).withCString { free_c_string(append_text(input: $0, cursorPtr: &cursor)) }
        #expect(cursor == composingText.convertTargetCursorPosition)
    }
}

@MainActor private func mixedInputRaw() -> String? {
    let raw = get_raw_input()
    defer { free_c_string(raw) }
    return raw.map { String(cString: $0) }
}

private struct MixedInputCandidate {
    let text: String
    let suffix: String
    let count: Int
}

@MainActor private func mixedInputCandidates(boundary: Int? = nil) -> [MixedInputCandidate] {
    var length: CInt = 0
    let list = if let boundary {
        get_composed_text_for_cursor_prefix(requiredInputCount: CInt(boundary), lengthPtr: &length)
    } else {
        get_composed_text(lengthPtr: &length)
    }
    defer { free_candidate_list(list, length) }
    return (0..<Int(length)).compactMap { index in
        guard let item = list.advanced(by: index).pointee?.pointee else { return nil }
        return MixedInputCandidate(
            text: String(cString: item.text),
            suffix: String(cString: item.subtext),
            count: Int(item.correspondingCount)
        )
    }
}

@Test func mixedInputPreservesWordsAndOriginalInputOneKeyAtATime() async {
    await MainActor.run {
        withMixedInputServer {
            for (keys, expected) in [
                ("kyouhagoogledekensaku", "きょうはgoogleでけんさく"),
                ("kyouhagithubnipushshita", "きょうはgithubにpushした"),
                ("pythonnotestwokaku", "pythonのtestをかく"),
                ("medalsha", "medalsは"),
                ("exceltoppt", "excelとppt"),
                ("Rustdesu", "Rustです"),
                ("gOoGlede", "gOoGleで"),
                ("nihongoogle", "にほんgoogle"),
            ] {
                composingText = ComposingText()
                typeMixedInput(keys)
                #expect(composingText.convertTarget == expected, "input: \(keys)")
                #expect(mixedInputRaw() == keys)
                #expect(composingText.input.count == keys.count)
            }
        }
    }
}

@Test func mixedInputProgrammingParticlesKeepRecognizedEnglish() async {
    await MainActor.run {
        withMixedInputServer {
            for (keys, reading) in [
                ("gitwotsukau", "gitをつかう"),
                ("kyouhagitwotsukau", "きょうはgitをつかう"),
                ("githa", "gitは"),
                ("bugwoshusei", "bugをしゅせい"),
                ("debugwoshuseishita", "debugをしゅせいした"),
                ("logwomiru", "logをみる"),
                ("inputwoshori", "inputをしょり"),
                ("commitwomodoshita", "commitをもどした"),
                ("UUIDwotsukau", "UUIDをつかう"),
            ] {
                clear_text()
                typeMixedInput(keys)
                #expect(composingText.convertTarget == reading, "keys: \(keys)")
                #expect(mixedInputRaw() == keys)
                #expect(composingText.input.count == keys.count)
            }
        }
    }
}

@Test func mixedInputDoesNotSplitOrdinaryJapaneseAndDisabledInput() async {
    await MainActor.run {
        for enabled in [false, true] {
            withMixedInputServer(enabled: enabled) {
                let words = enabled
                    ? ["nihongo", "tomato", "sushi", "make", "gitai", "kore", "kaisha", "mottekita"]
                    : ["kyouhagoogledekensaku", "pythonno", "google"]
                for keys in words {
                    composingText = ComposingText()
                    var ordinary = ComposingText()
                    for key in keys { ordinary.insertAtCursorPosition(String(key), inputStyle: .roman2kana) }
                    typeMixedInput(keys)
                    #expect(composingText == ordinary, "enabled: \(enabled), input: \(keys)")
                }
            }
        }
    }
}

@MainActor func mixedInputWindowsTable() throws -> (style: InputStyle, table: InputTable) {
    let root = URL(filePath: #filePath).deletingLastPathComponent()
        .deletingLastPathComponent().deletingLastPathComponent()
    let tableURL = root.deletingLastPathComponent().appending(path: "crates/shared/src/default_romaji_table.txt")
    let rows = try String(contentsOf: tableURL, encoding: .utf8)
        .split(whereSeparator: \.isNewline).compactMap { line -> RomajiTableRow? in
            guard !line.hasPrefix("#") else { return nil }
            let fields = line.split(separator: "\t", omittingEmptySubsequences: false)
            guard fields.count >= 2, !fields[0].isEmpty, !fields[1].isEmpty else { return nil }
            return RomajiTableRow(input: String(fields[0]), output: String(fields[1]),
                next_input: fields.count > 2 ? String(fields[2]) : nil)
        }
    let compiled = FileManager.default.temporaryDirectory.appendingPathComponent("mixed-input-\(UUID().uuidString).tsv")
    defer { try? FileManager.default.removeItem(at: compiled) }
    try #require(buildCustomRomajiTableContent(rows: rows)).write(to: compiled, atomically: true, encoding: .utf8)
    let name = "mixed-input-windows-table-\(UUID().uuidString)"
    let table = try InputStyleManager.loadTable(from: compiled)
    InputStyleManager.registerInputStyle(table: table, for: name)
    return (.mapped(id: .tableName(name)), table)
}

@Test func mixedInputWindowsRomajiTablePreservesNnDigraphsAndEnglishEdits() async throws {
    try await MainActor.run {
        let windows = try mixedInputWindowsTable()
        withMixedInputServer(realDictionary: true, inputStyle: windows.style, inputTable: windows.table) {
            for (keys, reading) in [
                ("kyouhagoogledekensaku", "きょうはgoogleでけんさく"),
                // The optional typo dictionary separately normalizes minna.
                ("minnanigithubwomiseta", "みんあにgithubをみせた"),
                ("denshadegithubwomita", "でんしゃでgithubをみた"),
                ("tyagoogle", "ちゃgoogle"), ("chagoogle", "ちゃgoogle"),
                ("nihongoogle", "にほんgoogle"),
            ] {
                clear_text()
                typeMixedInput(keys)
                #expect(composingText.convertTarget == reading, "keys: \(keys)")
                #expect(mixedInputRaw() == keys)
            }
            clear_text()
            typeMixedInput("googlede")
            var cursor: CInt = 0
            free_c_string(remove_text(cursorPtr: &cursor))
            #expect(mixedInputRaw() == "google") // one kana consumes de
            free_c_string(remove_text(cursorPtr: &cursor))
            #expect(mixedInputRaw() == "googl") // one literal consumes only e
            typeMixedInput("edekensaku")
            #expect(composingText.convertTarget == "googleでけんさく")
            #expect(mixedInputCandidates().first?.text == "googleで検索")
        }
    }
}

@Test func mixedInputDeletionAndReentryEditTheDisplayedEnglishCharacter() async {
    await MainActor.run {
        withMixedInputServer {
            typeMixedInput("google")
            var cursor: CInt = 0
            free_c_string(remove_text(cursorPtr: &cursor))
            #expect(composingText.convertTarget == "googl")
            #expect(mixedInputRaw() == "googl")
            #expect(cursor == 5)
            typeMixedInput("ede")
            #expect(composingText.convertTarget == "googleで")
            #expect(mixedInputRaw() == "googlede")
        }
    }
}

@Test func mixedInputInsertionInsideEnglishPreservesLiteralIntent() async {
    await MainActor.run {
        withMixedInputServer {
            typeMixedInput("google")
            var cursor: CInt = 0
            free_c_string(move_cursor(offset: -3, cursorPtr: &cursor))
            typeMixedInput("x")
            #expect(composingText.convertTarget == "gooxgle")
            #expect(mixedInputRaw() == "gooxgle")
            free_c_string(remove_text(cursorPtr: &cursor))
            #expect(composingText.convertTarget == "google")
            #expect(mixedInputRaw() == "google")
            #expect(composingText.convertTargetCursorPosition == 3)
        }
    }
}

@Test func mixedInputInsertionsAtJapaneseEnglishBoundariesKeepRawIdentity() async {
    await MainActor.run {
        withMixedInputServer(realDictionary: true) {
            let raw = "kyouhagoogledekensaku"
            for boundary in [0, 6, 7, 11, 12, 14] {
                clear_text()
                typeMixedInput(raw)
                let original = composingText.convertTarget
                let target = composingText.inputIndexToSurfaceIndexMap()[boundary]!
                var cursor: CInt = 0
                free_c_string(move_cursor(offset: CInt(target - composingText.convertTargetCursorPosition), cursorPtr: &cursor))
                typeMixedInput("x")
                let expected = String(raw.prefix(boundary)) + "x" + String(raw.dropFirst(boundary))
                #expect(mixedInputRaw() == expected, "boundary: \(boundary)")
                #expect(composingText.input.count == 22)
                #expect(composingText.convertTargetCursorPosition == target + 1)
                free_c_string(remove_text(cursorPtr: &cursor))
                #expect(mixedInputRaw() == raw, "boundary: \(boundary)")
                #expect(composingText.convertTarget == original)
                if boundary > 0 {
                    var applied: CInt = 0
                    var adjusted: CInt = 0
                    raw.withCString {
                        free_c_string(adjust_clause_boundary(currentInputCount: CInt(boundary), direction: -1,
                            expectedRawInput: $0, appliedPtr: &applied, adjustedInputCountPtr: &adjusted, cursorOffsetPtr: &cursor))
                    }
                    #expect(applied == 1)
                    let candidates = mixedInputCandidates(boundary: Int(adjusted))
                    #expect(!candidates.isEmpty)
                    #expect(candidates.allSatisfy { $0.count == adjusted })
                }
            }
        }
    }
}

@Test func mixedInputProtectedEnglishFollowedByKanaKeepsExactBoundary() async {
    await MainActor.run {
        withMixedInputServer(realDictionary: true) {
            typeMixedInput("input")
            var cursor: CInt = 0
            free_c_string(remove_text(cursorPtr: &cursor))
            free_c_string(remove_text(cursorPtr: &cursor))
            typeMixedInput("u")
            #expect(composingText.convertTarget == "inpう")
            #expect(mixedInputRaw() == "inpu")
            var applied: CInt = 0
            var boundary: CInt = 0
            "inpu".withCString {
                free_c_string(adjust_clause_boundary(currentInputCount: 4, direction: -1, expectedRawInput: $0,
                    appliedPtr: &applied, adjustedInputCountPtr: &boundary, cursorOffsetPtr: &cursor))
            }
            #expect(applied == 1 && boundary == 3)
            #expect(mixedInputCandidates(boundary: 3).contains { $0.text == "inp" && $0.suffix == "う" && $0.count == 3 })
            free_c_string(shrink_text(offset: 3))
            #expect(mixedInputRaw() == "u")
            #expect(composingText.convertTarget == "う")
        }
    }
}

@Test func mixedInputPartialCommitProtectsTheClippedEnglishSuffixAndSnapshots() async {
    await MainActor.run {
        withMixedInputServer {
            typeMixedInput("google")
            let original = composingText
            push_composing_text_snapshot(selectedCandidateId: 0)
            free_c_string(shrink_text(offset: 3))
            #expect(composingText.convertTarget == "gle")
            #expect(mixedInputRaw() == "gle")
            typeMixedInput("de")
            #expect(composingText.convertTarget == "gleで")
            #expect(mixedInputRaw() == "glede")
            pop_composing_text_snapshot(selectedCandidateId: 0)
            #expect(composingText == original)
        }
    }
}

@Test func mixedInputLiteralNDoesNotRewindTheConsumedInputCount() async {
    await MainActor.run {
        withMixedInputServer {
            typeMixedInput("input")
            #expect(composingText.convertTarget == "input")
            let resolution = resolveCandidateComposition(
                composingText: composingText, candidateComposingCount: .inputCount(3)
            )
            #expect(resolution.correspondingCount == 3)
            #expect(resolution.remainingConvertTarget == "ut")
        }
    }
}

@Test func mixedInputActualConverterProducesTheRequestedMixedSentence() async {
    await MainActor.run {
        withMixedInputServer(realDictionary: true) {
            typeMixedInput("kyouhagoogledekensaku")
            let candidates = mixedInputCandidates()
            #expect(candidates.first?.text == "今日はgoogleで検索", "candidates: \(candidates.prefix(5).map(\.text))")
            #expect(candidates.first?.suffix.isEmpty == true)
            #expect(candidates.first?.count == 21)
        }
    }
}

@Test func mixedInputRespectsDirectInputCustomMappingsAndTheWorkLimit() async {
    await MainActor.run {
        withMixedInputServer {
            var cursor: CInt = 0
            "google".withCString { free_c_string(append_text_direct(input: $0, cursorPtr: &cursor)) }
            let manual = composingText.input
            typeMixedInput("de")
            #expect(composingText.convertTarget == "googleで")
            #expect(Array(composingText.input.prefix(6)) == manual)

            InputStyleManager.registerInputStyle(table: InputTable(baseMapping: [
                [.piece(.character("g")), .piece(.character("o")), .piece(.character("o")),
                 .piece(.character("g")), .piece(.character("l")), .piece(.character("e"))]: [.character("ぐ")]
            ]), for: "mixed-input-custom-test")
            currentInputStyle = .mapped(id: .tableName("mixed-input-custom-test"))
            composingText = ComposingText()
            typeMixedInput("google")
            #expect(composingText.convertTarget == "ぐ")

            let segmenter = MixedInputConversion(inputStyle: .roman2kana)
            var longInput = ComposingText()
            longInput.insertAtCursorPosition(String(repeating: "google", count: 50), inputStyle: .roman2kana)
            #expect(segmenter.normalize(longInput) == longInput)
        }
    }
}

@Test func mixedInputReloadsCustomTableContentsUnderTheSameStyle() async {
    await MainActor.run {
        let savedCustom = customRomajiTableEnabled
        let savedCache = mixedInputConversionCache
        let savedTable = currentRomajiInputTable
        defer {
            customRomajiTableEnabled = savedCustom
            mixedInputConversionCache = savedCache
            for name in ["azookey-windows-custom-romaji", "azookey-windows-mixed-barrier-a", "azookey-windows-mixed-barrier-b"] {
                InputStyleManager.registerInputStyle(table: savedTable, for: name)
            }
        }
        withMixedInputServer {
            let common = [RomajiTableRow(input: "a", output: "あ", next_input: nil), RomajiTableRow(input: "i", output: "い", next_input: nil)]
            let first = common + [RomajiTableRow(input: "api", output: "あ", next_input: nil), RomajiTableRow(input: "ka", output: "か", next_input: nil)]
            let second = common + [RomajiTableRow(input: "ka", output: "き", next_input: nil)]
            setCustomRomajiInputStyle(rows: first)
            #expect(customRomajiTableEnabled)
            let style = currentInputStyle
            prepareMixedInputConversion(reset: true)
            typeMixedInput("api")
            #expect(composingText.convertTarget == "あ")
            #expect(mixedInputRaw() == "api" && composingText.input.count == 3)
            clear_text()
            typeMixedInput("kagoogle")
            #expect(composingText.convertTarget == "かgoogle")
            clear_text()

            // Both saves remain custom and reuse the production registry ID.
            // api is shorter than the bulk lexicon's minimum, so that lexicon
            // cannot mask a stale seed-eligibility decision from the first table.
            setCustomRomajiInputStyle(rows: second)
            #expect(customRomajiTableEnabled && currentInputStyle == style)
            prepareMixedInputConversion()
            typeMixedInput("api")
            #expect(composingText.convertTarget == "api")
            #expect(mixedInputRaw() == "api" && composingText.input.count == 3)
            clear_text()
            typeMixedInput("kagoogle")
            #expect(composingText.convertTarget == "きgoogle")
            clear_text()

            // Editing separators must be replaced with aliases of the new table,
            // without changing the reading or leaving an extra canonical element.
            composingText.insertAtCursorPosition("ka", inputStyle: style)
            composingText.insertAtCursorPosition([.init(piece: .compositionSeparator, inputStyle: .mapped(id: .empty))])
            composingText.insertAtCursorPosition("ka", inputStyle: style)
            #expect(composingText.convertTarget == "きき")
            removeMixedInputEditingSeparators()
            #expect(composingText.convertTarget == "きき")
            #expect(mixedInputRaw() == "kaka" && composingText.input.count == 4)
            clear_text()

            setCustomRomajiInputStyle(rows: first)
            #expect(currentInputStyle == style)
            prepareMixedInputConversion()
            typeMixedInput("api")
            #expect(composingText.convertTarget == "あ")
            #expect(mixedInputRaw() == "api" && composingText.input.count == 3)
        }
    }
}

@Test func mixedInputRecoveryReplaysEditingAndClauseHistoryWithoutLosingStyles() async {
    await MainActor.run {
        withMixedInputServer {
            @MainActor func history() -> ComposingText {
                typeMixedInput("kyouhagoogledekensaku")
                push_composing_text_snapshot(selectedCandidateId: 0)
                free_c_string(shrink_text(offset: 9)) // leave gle... inside google
                typeMixedInput("de")
                var cursor: CInt = 0
                free_c_string(remove_text(cursorPtr: &cursor))
                return composingText
            }
            let original = history()
            let snapshots = composingTextSnapshots
            clear_text()
            let replayed = history()
            #expect(replayed == original)
            #expect(composingTextSnapshots == snapshots)
            pop_composing_text_snapshot(selectedCandidateId: 0)
            #expect(mixedInputRaw() == "kyouhagoogledekensaku")
            #expect(composingText.convertTarget == "きょうはgoogleでけんさく")
        }
    }
}

@Test func mixedInputReportsBoundedSegmentationLatency() async {
    await MainActor.run {
        let segmenter = MixedInputConversion(inputStyle: .roman2kana)
        for (name, keys) in [
            ("no-match", String(repeating: "nihongo", count: 36)),
            ("mixed-max", String(repeating: "googlede", count: 32)),
            ("programming-max", String(repeating: "gitwo", count: 51)),
        ] {
            var source = ComposingText()
            source.insertAtCursorPosition(keys, inputStyle: .roman2kana)
            _ = segmenter.normalize(source)
            var samples: [Double] = []
            for _ in 0..<40 {
                let clock = ContinuousClock()
                let start = clock.now
                let result = segmenter.normalize(source)
                let elapsed = start.duration(to: clock.now).components
                samples.append(Double(elapsed.seconds) * 1_000 + Double(elapsed.attoseconds) / 1e15)
                #expect(result.input.count == source.input.count)
            }
            samples.sort()
            print("MIXED_INPUT_LATENCY case=\(name) count=\(keys.count) samples=\(samples.count) p95_ms=\(samples[37]) p99_ms=\(samples[39])")
        }
    }
}

@Test(.enabled(if: ProcessInfo.processInfo.environment["AZOOKEY_ZENZAI_TEST_MODEL"] != nil))
func mixedInputRealZenzaiPreservesEnglishAndAdjustedRanges() async throws {
    let path = try #require(ProcessInfo.processInfo.environment["AZOOKEY_ZENZAI_TEST_MODEL"])
    let model = URL(fileURLWithPath: path)
    try #require(FileManager.default.fileExists(atPath: path) && model.lastPathComponent == "zenz.gguf")
    await MainActor.run {
        withMixedInputServer(realDictionary: true) {
            config["enable"] = true
            config["backend"] = "cpu"
            execURL = model.deletingLastPathComponent()
            typeMixedInput("kyouhagoogledekensaku")
            let candidates = mixedInputCandidates()
            #expect(candidates.first?.text == "今日はgoogleで検索", "candidates: \(candidates.prefix(5).map(\.text))")
            #expect(candidates.first?.count == 21)
            #expect(converter.zenzStatus == "load \(model.absoluteString)", "status: \(converter.zenzStatus)")
            var applied: CInt = 0
            var boundary: CInt = 0
            var cursor: CInt = 0
            "kyouhagoogledekensaku".withCString {
                free_c_string(adjust_clause_boundary(currentInputCount: 12, direction: -1, expectedRawInput: $0,
                    appliedPtr: &applied, adjustedInputCountPtr: &boundary, cursorOffsetPtr: &cursor))
            }
            #expect(applied == 1 && boundary == 11)
            let adjusted = mixedInputCandidates(boundary: 11)
            #expect(!adjusted.isEmpty)
            #expect(adjusted.allSatisfy { $0.count == 11 && $0.suffix == "eでけんさく" })
            #expect(adjusted.contains { $0.text.hasSuffix("googl") })
            print("MIXED_INPUT_REAL_ZENZAI top=\(candidates.first?.text ?? "") adjusted_count=\(boundary) status=\(converter.zenzStatus)")
        }
    }
}

@Test func mixedInputShiftArrowRoundTripsMatchVisibleCandidatesAndConsumedRanges() async {
    await MainActor.run {
        withMixedInputServer(realDictionary: true) {
            let raw = "kyouhagoogledekensaku"
            typeMixedInput(raw)
            var boundary: CInt = 12
            // Repeated Shift+Left then Shift+Right across every English letter.
            for (direction, expected) in [(-1, 11), (-1, 10), (-1, 9), (-1, 8), (-1, 7), (-1, 6),
                                          (1, 7), (1, 8), (1, 9), (1, 10), (1, 11), (1, 12), (1, 14), (-1, 12)] {
                var applied: CInt = 0
                var adjusted: CInt = -1
                var cursor: CInt = 0
                raw.withCString {
                    free_c_string(adjust_clause_boundary(
                        currentInputCount: boundary, direction: CInt(direction), expectedRawInput: $0,
                        appliedPtr: &applied, adjustedInputCountPtr: &adjusted, cursorOffsetPtr: &cursor
                    ))
                }
                #expect(applied == 1)
                #expect(adjusted == expected)
                boundary = adjusted
                #expect(mixedInputRaw() == raw)
                let surface = Array("きょうはgoogleでけんさく")
                let surfaceBoundary = expected <= 12 ? expected - 2 : 11
                #expect(composingText.convertTargetCursorPosition == surfaceBoundary)
                let candidates = mixedInputCandidates(boundary: expected)
                #expect(!candidates.isEmpty)
                #expect(candidates.allSatisfy { $0.count == expected })
                #expect(candidates.allSatisfy { $0.suffix == String(surface.dropFirst(surfaceBoundary)) })
                if (7...12).contains(expected) {
                    let literal = String("google".prefix(expected - 6))
                    #expect(candidates.contains { $0.text.hasSuffix(literal) })
                }
                let saved = composingText
                free_c_string(shrink_text(offset: adjusted))
                #expect(mixedInputRaw() == String(raw.dropFirst(expected)))
                #expect(composingText.convertTarget == String(surface.dropFirst(surfaceBoundary)))
                composingText = saved
            }
        }
    }
}

@MainActor private func mixedInputScenarioTables() throws -> [(String, InputStyle, InputTable)] {
    let windows = try mixedInputWindowsTable()
    return [("builtin", .roman2kana, .defaultRomanToKana), ("windows", windows.style, windows.table)]
}

@MainActor private func expectMixedScenario(_ scenario: MixedInputScenario) {
    #expect(composingText.convertTarget == scenario.reading, "keys: \(scenario.keys)")
    #expect(mixedInputRaw() == scenario.keys, "keys: \(scenario.keys)")
    #expect(composingText.input.count == scenario.keys.count, "keys: \(scenario.keys)")
    guard composingText.input.count == scenario.keys.count else { return }
    let boundaries = composingText.inputIndexToSurfaceIndexMap()
    var searchStart = scenario.keys.startIndex
    for literal in scenario.literals {
        guard let range = scenario.keys.range(of: literal, range: searchStart..<scenario.keys.endIndex) else {
            Issue.record("Missing fixture literal \(literal) in \(scenario.keys)")
            continue
        }
        let start = scenario.keys.distance(from: scenario.keys.startIndex, to: range.lowerBound)
        let end = start + literal.count
        #expect(composingText.input[start..<end].allSatisfy { isMixedInputLiteralStyle($0.inputStyle) },
                "English span: \(literal), keys: \(scenario.keys)")
        #expect((start...end).allSatisfy { boundaries[$0] != nil },
                "Independent English boundaries: \(literal), keys: \(scenario.keys)")
        searchStart = range.upperBound
    }
}

@Test func mixedInputExpandedProgrammingAndGeneralCorpus() async throws {
    try await MainActor.run {
        for (name, style, table) in try mixedInputScenarioTables() {
            withMixedInputServer(inputStyle: style, inputTable: table) {
                let cases = MixedInputScenarioCorpus.programmingMatrix + MixedInputScenarioCorpus.sentences
                for scenario in cases {
                    clear_text()
                    typeMixedInput(scenario.keys)
                    expectMixedScenario(scenario)
                }
                print("MIXED_SCENARIO_CORPUS table=\(name) programming=\(MixedInputScenarioCorpus.programmingMatrix.count) sentences=\(MixedInputScenarioCorpus.sentences.count)")
            }
        }
    }
}

@Test func mixedInputExpandedJapaneseControlsAndExplicitLiterals() async throws {
    try await MainActor.run {
        for (name, style, table) in try mixedInputScenarioTables() {
            for enabled in [false, true] {
                withMixedInputServer(enabled: enabled, inputStyle: style, inputTable: table) {
                    for keys in MixedInputScenarioCorpus.japaneseControls {
                        clear_text()
                        var control = ComposingText()
                        for key in keys { control.insertAtCursorPosition(String(key), inputStyle: style) }
                        typeMixedInput(keys)
                        #expect(composingText == control, "Japanese control: \(keys), table: \(name), enabled: \(enabled)")
                        clear_text()
                        var cursor: CInt = 0
                        keys.withCString { free_c_string(append_text(input: $0, cursorPtr: &cursor)) }
                        #expect(composingText == control, "Japanese bulk control: \(keys), table: \(name), enabled: \(enabled)")
                    }
                    for literal in MixedInputScenarioCorpus.explicitLiterals {
                        clear_text()
                        typeMixedInput("kyouha")
                        var cursor: CInt = 0
                        literal.withCString { free_c_string(append_text_direct(input: $0, cursorPtr: &cursor)) }
                        typeMixedInput("wotsukau")
                        #expect(composingText.convertTarget == "きょうは" + literal + "をつかう", "manual: \(literal)")
                        #expect(mixedInputRaw() == "kyouha" + literal + "wotsukau", "manual: \(literal)")
                        #expect(composingText.input[6..<(6 + literal.count)].allSatisfy { $0.inputStyle == .direct })
                    }
                    print("MIXED_SCENARIO_CONTROLS table=\(name) enabled=\(enabled) japanese=\(MixedInputScenarioCorpus.japaneseControls.count) explicit=\(MixedInputScenarioCorpus.explicitLiterals.count)")
                }
            }
        }
    }
}

@Test func mixedInputProgrammingBatchAndPerKeyInputsAgree() async throws {
    try await MainActor.run {
        for (_, style, table) in try mixedInputScenarioTables() {
            withMixedInputServer(inputStyle: style, inputTable: table) {
                for scenario in MixedInputScenarioCorpus.sentences {
                    clear_text()
                    typeMixedInput(scenario.keys)
                    let incremental = composingText
                    clear_text()
                    var cursor: CInt = 0
                    scenario.keys.withCString { free_c_string(append_text(input: $0, cursorPtr: &cursor)) }
                    expectMixedScenario(scenario)
                    #expect(composingText == incremental, "Bulk/per-key: \(scenario.keys)")
                }
            }
        }
    }
}

@Test func mixedInputProgrammingConversionsKeepOriginalSpelling() async throws {
    try await MainActor.run {
        for (name, style, table) in try mixedInputScenarioTables() {
            withMixedInputServer(realDictionary: true, inputStyle: style, inputTable: table) {
                for scenario in MixedInputScenarioCorpus.sentences {
                    clear_text()
                    typeMixedInput(scenario.keys)
                    expectMixedScenario(scenario)
                    let candidates = mixedInputCandidates()
                    #expect(candidates.first?.count == scenario.keys.count, "full candidate: \(scenario.keys)")
                    #expect(candidates.first?.suffix.isEmpty == true)
                    for literal in scenario.literals {
                        #expect(candidates.first?.text.contains(literal) == true,
                                "spelling \(literal), keys: \(scenario.keys), top: \(candidates.first?.text ?? "")")
                    }
                }
                print("MIXED_SCENARIO_CONVERSION table=\(name) cases=\(MixedInputScenarioCorpus.sentences.count)")
            }
        }
    }
}

@Test func mixedInputProgrammingEditAndClauseHistories() async throws {
    try await MainActor.run {
        let words = ["git", "bug", "input", "commit", "function", "json", "typescript", "postgresql", "kubernetes", "mutex", "timeout", "UUID", "approach", "algorithm", "repository"]
        for (name, style, table) in try mixedInputScenarioTables() {
            withMixedInputServer(realDictionary: true, inputStyle: style, inputTable: table) {
                for word in words {
                    clear_text()
                    typeMixedInput(word)
                    var cursor: CInt = 0
                    let deleted = min(2, word.count - 1)
                    for _ in 0..<deleted { free_c_string(remove_text(cursorPtr: &cursor)) }
                    #expect(composingText.convertTarget == String(word.dropLast(deleted)))
                    #expect(mixedInputRaw() == String(word.dropLast(deleted)))
                    typeMixedInput(String(word.suffix(deleted)) + "desu")
                    #expect(composingText.convertTarget == word + "です", "reentry: \(word)")

                    clear_text()
                    let raw = "nihon" + word + "wotsukau"
                    typeMixedInput(raw)
                    #expect(composingText.convertTarget == "にほん" + word + "をつかう")
                    let original = composingText
                    push_composing_text_snapshot(selectedCandidateId: 0)
                    var boundary = 5 + word.count
                    for direction in Array(repeating: -1, count: word.count) + Array(repeating: 1, count: word.count) {
                        var applied: CInt = 0
                        var adjusted: CInt = 0
                        raw.withCString { free_c_string(adjust_clause_boundary(currentInputCount: CInt(boundary),
                            direction: CInt(direction), expectedRawInput: $0, appliedPtr: &applied,
                            adjustedInputCountPtr: &adjusted, cursorOffsetPtr: &cursor)) }
                        #expect(applied == 1)
                        #expect(adjusted == boundary + direction)
                        boundary = Int(adjusted)
                        let suffix = String(word.dropFirst(boundary - 5)) + "をつかう"
                        #expect(composingText.convertTargetCursorPosition == 3 + boundary - 5)
                        let candidates = mixedInputCandidates(boundary: boundary)
                        #expect(!candidates.isEmpty)
                        #expect(candidates.allSatisfy { $0.count == boundary && $0.suffix == suffix }, "boundary: \(word), \(boundary)")
                        let prefix = String(word.prefix(boundary - 5))
                        #expect(candidates.contains { $0.text.hasSuffix(prefix) }, "visible prefix: \(word), \(boundary)")
                        #expect(candidates.first?.text.hasSuffix(prefix) == true, "selected prefix: \(word), \(boundary)")
                        let saved = composingText
                        free_c_string(shrink_text(offset: CInt(boundary)))
                        #expect(mixedInputRaw() == String(raw.dropFirst(boundary)))
                        #expect(composingText.convertTarget == suffix)
                        composingText = saved
                    }
                    pop_composing_text_snapshot(selectedCandidateId: 0)
                    #expect(composingText == original)
                }
                print("MIXED_SCENARIO_EDIT table=\(name) words=\(words.count)")
            }
        }
    }
}

@Test func englishDictionaryKanaClauseHistoryKeepsRawConsumption() async throws {
    try await MainActor.run {
        for (name, style, table) in try mixedInputScenarioTables() {
            withMixedInputServer(realDictionary: true, inputStyle: style, inputTable: table) {
                clear_text()
                let prefix = name == "builtin" ? "apuroーchi" : "apuro-chi"
                typeMixedInput(prefix)
                var cursor: CInt = 0
                free_c_string(remove_text(cursorPtr: &cursor))
                typeMixedInput("chiwotsukau")
                #expect(mixedInputRaw() == prefix + "wotsukau")
                let saved = composingText
                push_composing_text_snapshot(selectedCandidateId: 0)
                free_c_string(move_cursor(offset: -4, cursorPtr: &cursor))
                let clause = mixedInputCandidates(boundary: 9)
                #expect(clause.contains { $0.text == "approach" && $0.count == 9 && $0.suffix == "をつかう" },
                        "table=\(name), surface=\(composingText.convertTarget), clause=\(clause.map { ($0.text, $0.count, $0.suffix) })")
                free_c_string(shrink_text(offset: 9))
                #expect(mixedInputRaw() == "wotsukau")
                #expect(composingText.convertTarget == "をつかう")
                pop_composing_text_snapshot(selectedCandidateId: 0)
                #expect(composingText == saved)
            }
        }
    }
}

@Test(.enabled(if: ProcessInfo.processInfo.environment["AZOOKEY_ZENZAI_TEST_MODEL"] != nil))
func mixedInputExpandedRealZenzaiScenarios() async throws {
    let path = try #require(ProcessInfo.processInfo.environment["AZOOKEY_ZENZAI_TEST_MODEL"])
    let model = URL(fileURLWithPath: path)
    try #require(FileManager.default.fileExists(atPath: path))
    try await MainActor.run {
        let windows = try mixedInputWindowsTable()
        withMixedInputServer(realDictionary: true, inputStyle: windows.style, inputTable: windows.table) {
            config["enable"] = true
            config["backend"] = "cpu"
            execURL = model.deletingLastPathComponent()
            let cases = Array(MixedInputScenarioCorpus.sentences.prefix(18))
                + Array(MixedInputScenarioCorpus.sentences.suffix(8))
            for scenario in cases {
                clear_text()
                typeMixedInput(scenario.keys)
                expectMixedScenario(scenario)
                let candidates = mixedInputCandidates()
                #expect(candidates.first?.count == scenario.keys.count)
                #expect(candidates.first?.suffix.isEmpty == true)
                for literal in scenario.literals {
                    #expect(candidates.first?.text.contains(literal) == true,
                            "model spelling \(literal), keys: \(scenario.keys), top: \(candidates.first?.text ?? "")")
                }
                #expect(converter.zenzStatus == "load \(model.absoluteString)", "status: \(converter.zenzStatus)")
                print("MIXED_SCENARIO_REAL_ZENZAI keys=\(scenario.keys) top=\(candidates.first?.text ?? "")")
            }
        }
    }
}
