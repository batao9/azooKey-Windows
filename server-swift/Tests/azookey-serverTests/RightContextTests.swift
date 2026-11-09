import Foundation
@testable import KanaKanjiConverterModule
import Testing
@testable import azookey_server

@Test func rightContextPromptMatchesUpstreamOrderAndUnicodeLimits() {
    for (left, right, prefix) in [
        ("", "", ""),
        ("", "を渡る", "\u{EE07}を渡る"),
        ("川の", "を渡る", "\u{EE02}川の\u{EE07}を渡る"),
        ("😀か\u{3099}", "👨‍👩‍👧‍👦か\u{3099}後", "\u{EE02}😀か\u{3099}\u{EE07}👨‍👩‍👧‍👦か\u{3099}")
    ] {
        let mode = ConvertRequestOptions.ZenzaiV3DependentMode(leftSideContext: left,
            rightSideContext: right, maxRightSideContextLength: right.hasPrefix("👨‍👩‍👧‍👦") ? 2 : 40)
        #expect(ZenzPromptBuilder.candidateEvaluationPrompt(input: "ハシ", userDictionaryPrompt: "",
            versionDependentConfig: .v3(mode)) == prefix + "\u{EE00}ハシ\u{EE01}")
        #expect(ZenzPromptBuilder.inputPredictionPrompt(leftSideContext: left, composingText: "はし",
            versionDependentConfig: .v3(mode)) == prefix + "\u{EE00}ハシ")
    }
}

@Test func rightContextOptionsRequireV32AndClearAtCompositionBoundaries() async throws {
    try await MainActor.run {
        let oldConfig = config
        let oldURL = execURL
        let oldText = composingText
        let oldSnapshots = composingTextSnapshots
        defer { config = oldConfig; execURL = oldURL; composingText = oldText; composingTextSnapshots = oldSnapshots }
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        execURL = directory
        config = ["backend": "cpu"]
        selectZenzaiModel()
        setSurroundingContext(left: "川の", right: "を渡る")
        let legacy = getOptions(context: "川の", zenzaiEnabled: true)
        #expect(legacy.zenzaiMode == .on(weight: directory.appendingPathComponent("zenz.gguf"),
            inferenceLimit: 1, requestRichCandidates: true, personalizationMode: nil,
            versionDependentMode: .v3(.init(profile: "", leftSideContext: "川の"))))
        try Data().write(to: directory.appendingPathComponent("zenz-v3.2-small.gguf"))
        selectZenzaiModel()
        let v32 = getOptions(context: "川の", zenzaiEnabled: true)
        #expect(v32.zenzaiMode == .on(weight: directory.appendingPathComponent("zenz-v3.2-small.gguf"),
            inferenceLimit: 1, requestRichCandidates: true, personalizationMode: nil,
            versionDependentMode: .v3(.init(profile: "", leftSideContext: "川の", rightSideContext: "を渡る"))))
        setSurroundingContext(left: String(repeating: "左", count: 50), right: String(repeating: "右", count: 50))
        #expect(config["context"] as? String == String(repeating: "左", count: 40))
        #expect(config["rightContext"] as? String == String(repeating: "右", count: 40))
        reset_text_for_recovery()
        #expect(config["context"] as? String == "")
        #expect(config["rightContext"] as? String == "")
        setSurroundingContext(left: "古い", right: "文書")
        "互換".withCString { set_context(context: $0) }
        #expect(config["context"] as? String == "互換")
        #expect(config["rightContext"] as? String == "")
    }
}

@Test func rightContextRefreshUsesExactRawBoundaryAndRestoresTheLiveCursor() async throws {
    try await MainActor.run {
        let oldConverter = converter, oldSupplement = normalNBestSupplementConverter
        let oldText = composingText, oldConfig = config, oldStyle = currentInputStyle
        let oldLearning = currentLearningType, oldCache = learningCandidateCache
        defer { converter = oldConverter; normalNBestSupplementConverter = oldSupplement
            composingText = oldText; config = oldConfig; currentInputStyle = oldStyle
            currentLearningType = oldLearning; learningCandidateCache = oldCache }
        let package = URL(filePath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        let dictionary = package.deletingLastPathComponent().appending(path: "target/azookey-english-dictionary/Dictionary")
        converter = KanaKanjiConverter(dictionaryURL: dictionary, preloadDictionary: true)
        normalNBestSupplementConverter = KanaKanjiConverter(dictionaryURL: dictionary, preloadDictionary: true)
        composingText = ComposingText(); currentInputStyle = .roman2kana; currentLearningType = .nothing
        config = ["enable": false, "backend": "cpu"]
        composingText.insertAtCursorPosition("kyouhaiitenkidesu", inputStyle: .roman2kana)
        let saved = composingText
        for required in [Int32(saved.input.count), 6] {
            setSurroundingContext(left: "文中", right: "。次の文です。")
            var length: CInt = 0
            let list = refresh_composed_text_for_input_count(requiredInputCount: required, lengthPtr: &length)
            defer { free_candidate_list(list, length) }
            try #require(length > 0)
            for index in 0..<Int(length) { #expect(list[index]?.pointee.correspondingCount == required) }
            #expect(composingText == saved)
        }
        var length: CInt = -1
        let list = refresh_composed_text_for_input_count(requiredInputCount: Int32(saved.input.count + 1), lengthPtr: &length)
        free_candidate_list(list, length)
        #expect(length == 0 && composingText == saved)
    }
}
