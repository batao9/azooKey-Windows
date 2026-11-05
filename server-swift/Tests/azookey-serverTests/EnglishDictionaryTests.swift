import Foundation
import KanaKanjiConverterModule
import Testing
#if os(Windows)
import WinSDK
#elseif os(Linux)
import Glibc
#endif
@testable import azookey_server

private let englishPackageRoot = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
    .deletingLastPathComponent().deletingLastPathComponent()

private func englishDictionaryRoot(_ mode: String = "both") -> URL {
    if mode == "base" { return englishPackageRoot.appending(path: "azooKey_dictionary_storage/Dictionary") }
    let root = englishPackageRoot.deletingLastPathComponent().appending(path: "target/azookey-english-dictionary/Dictionary")
    return mode == "both" ? root : root.appending(path: "Variants/\(mode)")
}

private func englishOptions() -> ConvertRequestOptions {
    ConvertRequestOptions(
        N_best: 10, requireJapanesePrediction: .disabled, requireEnglishPrediction: .disabled,
        keyboardLanguage: .ja_JP, learningType: .nothing,
        memoryDirectoryURL: englishPackageRoot, sharedContainerURL: englishPackageRoot,
        textReplacer: .init { englishPackageRoot.appending(path: "azooKey_emoji_dictionary_storage/EmojiDictionary/emoji_all_E15.1.txt") },
        specialCandidateProviders: nil, zenzaiMode: .off, metadata: .init(versionString: "English dictionary verification")
    )
}

@Test func katakanaAbbreviationsPreferKanaToExpandedEnglish() async throws {
    try await MainActor.run {
        let table = try mixedInputWindowsTable()
        let modelPath = ProcessInfo.processInfo.environment["AZOOKEY_ZENZAI_TEST_MODEL"]
        for useModel in modelPath == nil ? [false] : [false, true] {
            let engine = KanaKanjiConverter(dictionaryURL: englishDictionaryRoot(), preloadDictionary: true)
            var options = englishOptions()
            if useModel, let modelPath {
                options.zenzaiMode = .on(weight: URL(fileURLWithPath: modelPath), inferenceLimit: 1,
                    requestRichCandidates: true, personalizationMode: nil,
                    versionDependentMode: .v3(.init(profile: "", leftSideContext: "")))
            }
            var text = ComposingText()
            for key in "insuto" {
                text.insertAtCursorPosition(String(key), inputStyle: table.style)
                _ = engine.requestCandidates(text, options: options)
            }
            let candidates = engine.requestCandidates(text, options: options).mainResults
            let kana = try #require(candidates.firstIndex { $0.text == "インスト" })
            let english = try #require(candidates.firstIndex { $0.text == "instrumental" })
            #expect(kana < english, "model=\(useModel), candidates=\(candidates.map(\.text))")
            #expect(candidates.first?.text != "instrumental")
            #expect(candidates[english].data.contains { $0.word == "instrumental" && $0.value() == PValue(-16) })
            let consumed = resolveCandidateComposition(composingText: text, candidateComposingCount: candidates[english].composingCount)
            #expect(consumed.correspondingCount == 6 && consumed.remainingConvertTarget.isEmpty)
            engine.stopComposition()
            text = ComposingText()
            text.insertAtCursorPosition("インストルメンタル", inputStyle: .direct)
            let full = engine.requestCandidates(text, options: options).mainResults
            #expect(full.flatMap(\.data).contains { $0.word == "instrumental" && $0.value() == PValue(-12) })
            if useModel, let modelPath {
                #expect(engine.zenzStatus == "load \(URL(fileURLWithPath: modelPath).absoluteString)")
            }
            print("KATAKANA_ABBREVIATION_PRIORITY model=\(useModel) kana_rank=\(kana) english_rank=\(english) raw_count=\(consumed.correspondingCount)")
        }
    }
}

@MainActor private func withDictionarySettingsServer(_ body: () throws -> Void) rethrows {
    let saved = (converter, normalNBestSupplementConverter, reconversionDictionary, composingText,
                 composingTextSnapshots, config, converterBaseDictionaryURL, converterDictionaryURL,
                 converterPreloadDictionary, activeBundledDictionaryMode,
                 bundledDictionaryConfigurationInitialized, pendingBundledDictionaryConfiguration,
                 mixedInputConversionCache, currentInputStyle, currentRomajiInputTable,
                 currentLearningType, currentLearningMemoryDirectoryURL, learningCandidateCache, learningSelectionOverrides,
                 lastConfigApplySucceeded)
    defer {
        (converter, normalNBestSupplementConverter, reconversionDictionary, composingText,
         composingTextSnapshots, config, converterBaseDictionaryURL, converterDictionaryURL,
         converterPreloadDictionary, activeBundledDictionaryMode,
         bundledDictionaryConfigurationInitialized, pendingBundledDictionaryConfiguration,
         mixedInputConversionCache, currentInputStyle, currentRomajiInputTable,
         currentLearningType, currentLearningMemoryDirectoryURL, learningCandidateCache, learningSelectionOverrides,
         lastConfigApplySucceeded) = saved
    }
    converterBaseDictionaryURL = englishDictionaryRoot()
    converterPreloadDictionary = true
    bundledDictionaryConfigurationInitialized = false
    pendingBundledDictionaryConfiguration = nil
    mixedInputConversionCache = nil
    currentInputStyle = .roman2kana
    currentRomajiInputTable = .defaultRomanToKana
    composingText = ComposingText()
    composingTextSnapshots = []
    currentLearningType = .nothing
    config["enable"] = false
    config["experimentalMixedInput"] = true
    try body()
}

@Test func englishDictionaryResourceValidationAndDefaults() throws {
    let vocabulary = try EnglishVocabularyResource.result.get()
    #expect(vocabulary.wordCount == 61_050)
    #expect(!vocabulary.bytes.isEmpty)
    let missing = try JSONDecoder().decode(GeneralSettings.self, from: Data("{}".utf8))
    #expect(missing.bundledDictionaries == .both)
    let explicit = try JSONDecoder().decode(GeneralSettings.self,
        from: Data("{\"english_dictionary\":false,\"katakana_english_dictionary\":true}".utf8))
    #expect(explicit.bundledDictionaries == .init(english: false, katakana: true))
    // Corrupted headers and child ranges must fail before indexed searches.
    var binary = Data("AZEV".utf8)
    for value in [UInt32(1), UInt32(vocabulary.bytes.count / 8), UInt32(vocabulary.wordCount)] {
        var little = value.littleEndian
        withUnsafeBytes(of: &little) { binary.append(contentsOf: $0) }
    }
    binary.append(contentsOf: vocabulary.bytes)
    #expect(try EnglishVocabulary(data: binary).wordCount == vocabulary.wordCount)
    var invalid = binary
    invalid[16] = 0xff
    #expect(throws: EnglishVocabulary.ResourceError.self) { _ = try EnglishVocabulary(data: invalid) }
    #expect(throws: EnglishVocabulary.ResourceError.self) { _ = try EnglishVocabulary(data: binary.prefix(20)) }
}

@Test func englishDictionaryShortRequestsDoNotChangeTheConfiguredGpuBackend() async {
    await MainActor.run {
        let saved = config
        defer { config = saved; configureEngineRuntime() }
        config["enable"] = true
        config["backend"] = "cuda"
        _ = getOptions(zenzaiEnabled: true)
        #expect(engineRuntimeGpuLayerCount == Int32.max)
        // Short/normal-KKC input must not change the process-global setting a
        // concurrently preparing replacement model will read at construction.
        _ = getOptions(zenzaiEnabled: false)
        #expect(engineRuntimeGpuLayerCount == Int32.max)
        config["backend"] = "cpu"
        _ = getOptions(zenzaiEnabled: false)
        #expect(engineRuntimeGpuLayerCount == 0)
    }
}

@Test(.enabled(if: ProcessInfo.processInfo.environment["AZOOKEY_RESOURCE_MISSING_PROBE"] == "1"))
func englishDictionaryMissingResourceFailsWithoutCrash() {
    #expect(throws: EnglishVocabulary.ResourceError.self) { _ = try EnglishVocabulary.loadBundled() }
}

@Test func englishDictionarySettingsSelectIndependentRootsAndOverlays() async throws {
    try await MainActor.run {
        try withDictionarySettingsServer {
            let user = DicdataElement(word: "UserDictionaryMarker", ruby: "ジショ", cid: 1288, mid: 501, value: -5)
            for english in [true, false] {
                for katakana in [true, false] {
                    clear_text()
                    let mode = BundledDictionaryMode(english: english, katakana: katakana)
                    try #require(configureBundledDictionaries(mode: mode, force: false,
                        userEntries: [user], conversionEntries: [user]))
                    #expect(activeBundledDictionaryMode == mode)
                    #expect(converterDictionaryURL == mode.directory(in: converterBaseDictionaryURL))
                    for engine in [converter, normalNBestSupplementConverter] {
                        engine.stopComposition()
                        var text = ComposingText()
                        text.insertAtCursorPosition("algorithm", inputStyle: .direct)
                        let ascii = engine.requestCandidates(text, options: englishOptions()).mainResults
                        let hasAddedSpelling = ascii.flatMap(\.data).contains { row in
                            row.ruby == "algorithm" && row.word == "algorithm" && row.lcid == 1285 && row.value() == PValue(-10)
                        }
                        #expect(hasAddedSpelling == english)
                        engine.stopComposition()
                        text = ComposingText()
                        text.insertAtCursorPosition("リベース", inputStyle: .direct)
                        let kana = engine.requestCandidates(text, options: englishOptions()).mainResults
                        #expect(kana.contains { $0.text == "rebase" } == katakana)
                        engine.stopComposition()
                        text = ComposingText()
                        text.insertAtCursorPosition("ジショ", inputStyle: .direct)
                        #expect(engine.requestCandidates(text, options: englishOptions()).mainResults.contains { $0.text == user.word })
                    }
                    var typed = ComposingText()
                    typed.insertAtCursorPosition("google", inputStyle: currentInputStyle)
                    let normalized = mixedInputConversionCache!.normalize(typed)
                    #expect((normalized.convertTarget == "google") == english)
                    #expect(reconversionDictionary.inferReadings(for: "rebase").contains("リベース") == katakana)
                }
            }
        }
    }
}

@Test func englishDictionarySettingsDeferWithoutLosingInputSnapshotsOrCandidateIds() async throws {
    try await MainActor.run {
        try withDictionarySettingsServer {
            try #require(configureBundledDictionaries(mode: .both, force: false, userEntries: [], conversionEntries: []))
            var cursor: CInt = 0
            "repository".withCString { free_c_string(append_text(input: $0, cursorPtr: &cursor)) }
            push_composing_text_snapshot(selectedCandidateId: 0)
            let before = composingText
            currentLearningType = .inputAndOutput
            let candidate = try #require(converter.requestCandidates(composingText, options: englishOptions()).mainResults.first)
            let batch = try #require(cacheLearningCandidates([candidate]))
            let candidateID = learningCandidateId(at: 0, batchFirstId: batch)
            let disabled = BundledDictionaryMode(english: false, katakana: false)
            try #require(configureBundledDictionaries(mode: disabled, force: false, userEntries: [], conversionEntries: []))
            #expect(composingText == before)
            #expect(composingTextSnapshots.last == before)
            #expect(activeBundledDictionaryMode == .both)
            #expect(consumeLearningCandidate(candidateID)?.text == candidate.text)
            // The last desired setting wins, including cancelling a pending change.
            try #require(configureBundledDictionaries(mode: .both, force: false, userEntries: [], conversionEntries: []))
            #expect(pendingBundledDictionaryConfiguration == nil)
            try #require(configureBundledDictionaries(mode: disabled, force: false, userEntries: [], conversionEntries: []))
            let prepared = try #require(pendingBundledDictionaryConfiguration)
            free_c_string(shrink_text(offset: 4))
            pop_composing_text_snapshot(selectedCandidateId: 0)
            #expect(composingText == before)
            clear_text()
            #expect(converter === prepared.main)
            #expect(activeBundledDictionaryMode == disabled)
            #expect(composingText.input.isEmpty && composingTextSnapshots.isEmpty)
            #expect(pendingBundledDictionaryConfiguration == nil)
            "google".withCString { free_c_string(append_text_direct(input: $0, cursorPtr: &cursor)) }
            #expect(composingText.convertTarget == "google")
            clear_text()
            let installed = converter
            converterBaseDictionaryURL = converterBaseDictionaryURL.appending(path: "missing")
            #expect(!configureBundledDictionaries(mode: .both, force: false, userEntries: [], conversionEntries: []))
            #expect(converter === installed)
            #expect(activeBundledDictionaryMode == disabled)
        }
    }
}

@Test func englishDictionarySettingsPreservePersistedLearningAcrossRootChanges() async throws {
    try await MainActor.run {
        try withDictionarySettingsServer {
            let memory = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
            try FileManager.default.createDirectory(at: memory, withIntermediateDirectories: true)
            defer { try? FileManager.default.removeItem(at: memory) }
            currentLearningType = .inputAndOutput
            currentLearningMemoryDirectoryURL = memory
            let user = DicdataElement(word: "DictionaryLearningMarker", ruby: "ジショ", cid: 1288, mid: 501, value: -5)
            try #require(configureBundledDictionaries(mode: .both, force: false, userEntries: [user], conversionEntries: [user]))
            var options = englishOptions()
            options.learningType = .inputAndOutput
            options.memoryDirectoryURL = memory
            composingText.insertAtCursorPosition("ジショ", inputStyle: .direct)
            let candidate = try #require(converter.requestCandidates(composingText, options: options).mainResults.first { $0.text == user.word })
            let batch = try #require(cacheLearningCandidates([candidate]))
            let id = learningCandidateId(at: 0, batchFirstId: batch)
            try #require(configureBundledDictionaries(mode: .init(english: false, katakana: false),
                force: false, userEntries: [], conversionEntries: []))
            let prepared = try #require(pendingBundledDictionaryConfiguration)
            var warmOptions = options
            warmOptions.learningType = .nothing
            var warmText = ComposingText()
            warmText.insertAtCursorPosition("ミズ", inputStyle: .direct)
            _ = prepared.main.requestCandidates(warmText, options: warmOptions)
            // Commit after preparation/warmup, as when typing finishes while
            // a settings request is preparing its replacement in the background.
            try #require(commit_learning_candidate(candidateId: id, commitKind: 0))
            clear_text()
            #expect(currentLearningMemoryDirectoryURL == memory)
            var text = ComposingText()
            text.insertAtCursorPosition("ジショ", inputStyle: .direct)
            #expect(converter.requestCandidates(text, options: options).mainResults.contains { $0.text == user.word })
        }
    }
}

@Test func englishDictionarySettingsDoNotApplyDuringRecoveryResets() async throws {
    try await MainActor.run {
        try withDictionarySettingsServer {
            try #require(configureBundledDictionaries(mode: .both, force: false, userEntries: [], conversionEntries: []))
            var cursor: CInt = 0
            for key in "repository" {
                String(key).withCString { free_c_string(append_text(input: $0, cursorPtr: &cursor)) }
            }
            push_composing_text_snapshot(selectedCandidateId: 0)
            let original = composingText
            let snapshots = composingTextSnapshots
            let off = BundledDictionaryMode(english: false, katakana: false)
            try #require(configureBundledDictionaries(mode: off, force: false, userEntries: [], conversionEntries: []))
            // Disconnected-owner cleanup and ReplaceComposition's initial reset
            // both preserve the effective mode of the replayed composition.
            reset_text_for_recovery()
            reset_text_for_recovery()
            #expect(activeBundledDictionaryMode == .both)
            #expect(pendingBundledDictionaryConfiguration?.mode == off)
            for key in "repository" {
                String(key).withCString { free_c_string(append_text(input: $0, cursorPtr: &cursor)) }
            }
            push_composing_text_snapshot(selectedCandidateId: 0)
            #expect(composingText == original)
            #expect(composingTextSnapshots == snapshots)
            var length: CInt = 0
            let candidates = get_composed_text(lengthPtr: &length)
            #expect(length > 0)
            #expect(candidates.pointee?.pointee.correspondingCount == 10)
            free_candidate_list(candidates, length)
            clear_text()
            #expect(activeBundledDictionaryMode == off)
            "repository".withCString { free_c_string(append_text(input: $0, cursorPtr: &cursor)) }
            #expect(composingText.convertTarget != "repository")
        }
    }
}

@Test func englishDictionaryInitializationFailureDoesNotBecomeReady() async throws {
    try await MainActor.run {
        try withDictionarySettingsServer {
            let savedURL = execURL
            defer { execURL = savedURL }
            let root = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
            try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
            defer { try? FileManager.default.removeItem(at: root) }
            root.path.withCString { initialize(path: $0, use_zenzai: false) }
            #expect(get_config_apply_status() == 0)
            #expect(!bundledDictionaryConfigurationInitialized)
            #expect(composingText.input.isEmpty)
        }
    }
}

@Test(.enabled(if: ProcessInfo.processInfo.environment["AZOOKEY_ZENZAI_TEST_MODEL"] != nil))
func englishDictionarySettingsPrepareZenzaiBeforeTheNextInput() async throws {
    let path = try #require(ProcessInfo.processInfo.environment["AZOOKEY_ZENZAI_TEST_MODEL"])
    try await MainActor.run {
        try withDictionarySettingsServer {
            let savedURL = execURL
            defer { execURL = savedURL }
            execURL = URL(fileURLWithPath: path).deletingLastPathComponent()
            config["enable"] = true
            config["backend"] = "cpu"
            try #require(configureBundledDictionaries(mode: .both, force: false, userEntries: [], conversionEntries: []))
            let options = getOptions(zenzaiEnabled: true)
            composingText.insertAtCursorPosition("ニホンゴ", inputStyle: .direct)
            _ = converter.requestCandidates(composingText, options: options)
            let readyStatus = "load \(URL(fileURLWithPath: path).absoluteString)"
            try #require(converter.zenzStatus == readyStatus)
            let before = composingText
            let began = englishNow()
            let beforeMemory = englishMemoryBytes()
            try #require(configureBundledDictionaries(mode: .init(english: false, katakana: true),
                force: false, userEntries: [], conversionEntries: [], warmupOptions: options))
            let queuedElapsed = englishNow() - began
            #expect(pendingBundledDictionaryConfiguration == nil)
            #expect(get_config_apply_status() == 2)
            var generation: UInt64 = 0
            let job = try #require(take_dictionary_model_preparation(generation: &generation))
            let workBegan = englishNow()
            let warmed = run_dictionary_model_preparation(job: job)
            let modelElapsed = englishNow() - workBegan
            let finished = finish_dictionary_model_preparation(generation: generation, succeeded: warmed == 1)
            guard finished == 1 else {
                Issue.record("Prepared model could not be published: \(finished)")
                return
            }
            let preparedMemory = englishMemoryBytes()
            let prepared = try #require(pendingBundledDictionaryConfiguration)
            #expect(prepared.main.zenzStatus == readyStatus)
            #expect(composingText == before)
            clear_text()
            #expect(converter === prepared.main)
            #expect(converter.zenzStatus == readyStatus)
            let metrics: [String: Any] = ["prepared_before_input": true, "queue_ms": queuedElapsed,
                "worker_ms": modelElapsed, "total_ms": englishNow() - began,
                "before_rss": beforeMemory, "prepared_rss": preparedMemory, "after_clear_rss": englishMemoryBytes()]
            let data = try JSONSerialization.data(withJSONObject: metrics, options: [.sortedKeys])
            if let output = ProcessInfo.processInfo.environment["AZOOKEY_DICTIONARY_MODEL_OUTPUT"] {
                try data.write(to: URL(fileURLWithPath: output), options: .atomic)
            } else { print("ENGLISH_DICTIONARY_MODEL_SWITCH_JSON " + String(decoding: data, as: UTF8.self)) }
        }
    }
}

@Test func englishDictionarySettingsFailedModelJobPreservesTheActiveState() async throws {
    try await MainActor.run {
        try withDictionarySettingsServer {
            try #require(configureBundledDictionaries(mode: .both, force: false, userEntries: [], conversionEntries: []))
            composingText.insertAtCursorPosition("google", inputStyle: .direct)
            let before = composingText
            let main = converter
            let off = BundledDictionaryMode(english: false, katakana: false)
            // With Zenzai deliberately off this job cannot report a ready model.
            // Exercise the real retained-job failure/publish lifecycle without
            // substituting a fake converter or accepting a normal-KKC fallback.
            try #require(configureBundledDictionaries(mode: off, force: false,
                userEntries: [], conversionEntries: [], warmupOptions: englishOptions()))
            #expect(get_config_apply_status() == 2)
            var generation: UInt64 = 0
            let job = try #require(take_dictionary_model_preparation(generation: &generation))
            let result = run_dictionary_model_preparation(job: job)
            #expect(result == 0)
            #expect(finish_dictionary_model_preparation(generation: generation, succeeded: false) == 0)
            #expect(get_config_apply_status() == 0)
            #expect(converter === main)
            #expect(composingText == before)
            #expect(activeBundledDictionaryMode == .both)
            #expect(pendingBundledDictionaryConfiguration == nil)
        }
    }
}

@Test func englishDictionaryProvidesKanaAndSpellingCandidatesWithRawCounts() async throws {
    try await MainActor.run {
        let root = englishDictionaryRoot()
        try #require(FileManager.default.fileExists(atPath: root.appending(path: "english-manifest.json").path),
                     "Run node scripts/generate-english-dictionary.mjs before Swift tests")
        let engine = KanaKanjiConverter(dictionaryURL: root, preloadDictionary: true)
        let windowsTable = try mixedInputWindowsTable()
        for (keys, word) in [("apuro-chi", "approach"), ("arugorizumu", "algorithm"),
                             ("inta-fe-su", "interface"), ("kyasshu", "cache"),
                             ("ripojitori", "repository")] {
            engine.stopComposition()
            var text = ComposingText()
            for key in keys {
                text.insertAtCursorPosition(String(key), inputStyle: windowsTable.style)
                _ = engine.requestCandidates(text, options: englishOptions())
            }
            let result = engine.requestCandidates(text, options: englishOptions())
            let candidate = try #require(result.mainResults.first { $0.text == word },
                                         "keys=\(keys), candidates=\(result.mainResults.map(\.text))")
            let consumed = resolveCandidateComposition(composingText: text, candidateComposingCount: candidate.composingCount)
            #expect(consumed.correspondingCount == keys.count)
            #expect(consumed.remainingConvertTarget.isEmpty)
            #expect(candidate.data.contains { $0.word == word }, "dictionary row: \(keys), \(candidate.data)")
            let beforeDelete = text
            text.deleteBackwardFromCursorPosition(count: 1)
            _ = engine.requestCandidates(text, options: englishOptions())
            // Deleting a small kana may rewrite the preceding romaji (shu →
            // shi), so a raw-count difference is not the removed key suffix.
            // Re-enter the deleted visible kana directly, an actual kana edit.
            let removedSurfaceCount = beforeDelete.convertTarget.count - text.convertTarget.count
            text.insertAtCursorPosition(String(beforeDelete.convertTarget.suffix(removedSurfaceCount)), inputStyle: .direct)
            let reentered = engine.requestCandidates(text, options: englishOptions()).mainResults
            #expect(reentered.contains { $0.text == word },
                    "reentry: \(keys), reading=\(text.convertTarget), candidates=\(reentered.map(\.text))")
        }
        for word in ["approach", "Approach", "APPROACH", "algorithm", "repository"] {
            engine.stopComposition()
            var text = ComposingText()
            text.insertAtCursorPosition(word, inputStyle: .direct)
            let candidates = engine.requestCandidates(text, options: englishOptions()).mainResults
            #expect(candidates.contains { candidate in
                candidate.text == word
                    && resolveCandidateComposition(composingText: text, candidateComposingCount: candidate.composingCount).correspondingCount == word.count
                    && candidate.data.contains { $0.ruby == word && $0.word == word && $0.lcid == 1285 && $0.mid == 501 }
            }, "Must use the added dictionary row, not just the unknown-word fallback: \(word)")
        }
    }
}

@Test func englishDictionaryExpandedRecognitionKeepsJapaneseAndSpelling() async {
    await MainActor.run {
        let normalizer = MixedInputConversion(inputStyle: .roman2kana)
        for (keys, expected) in [("approach", "approach"), ("algorithm", "algorithm"),
                                  ("repository", "repository"), ("Approach", "Approach")] {
            var text = ComposingText()
            for key in keys {
                text.insertAtCursorPosition(String(key), inputStyle: .roman2kana)
                text = normalizer.normalize(text)
            }
            #expect(text.convertTarget == expected)
            #expect(text.input.count == keys.count)
        }
        for keys in MixedInputScenarioCorpus.japaneseControls {
            var plain = ComposingText()
            plain.insertAtCursorPosition(keys, inputStyle: .roman2kana)
            #expect(normalizer.normalize(plain) == plain, "Japanese control: \(keys)")
        }
    }
}

#if os(Windows)
private let englishClockFrequency: Double = {
    var frequency = LARGE_INTEGER()
    _ = QueryPerformanceFrequency(&frequency)
    precondition(frequency.QuadPart > 0, "High-resolution performance clock unavailable")
    return Double(frequency.QuadPart)
}()
#endif

private func englishNow() -> Double {
    #if os(Windows)
    var counter = LARGE_INTEGER()
    _ = QueryPerformanceCounter(&counter)
    return Double(counter.QuadPart) * 1000 / englishClockFrequency
    #else
    return ProcessInfo.processInfo.systemUptime * 1000
    #endif
}

private func englishMemoryBytes() -> UInt64 {
    #if os(Windows)
    var counters = PROCESS_MEMORY_COUNTERS()
    let size = DWORD(MemoryLayout<PROCESS_MEMORY_COUNTERS>.size)
    counters.cb = size
    if K32GetProcessMemoryInfo(GetCurrentProcess(), &counters, size) {
        return UInt64(counters.WorkingSetSize)
    }
    #elseif os(Linux)
    if let fields = try? String(contentsOfFile: "/proc/self/statm", encoding: .utf8).split(separator: " "),
       fields.count > 1, let pages = UInt64(fields[1]) {
        return pages * UInt64(getpagesize())
    }
    #endif
    return 0
}

private func englishPercentiles(_ values: [Double]) -> [String: Double] {
    let sorted = values.sorted()
    func percentile(_ p: Double) -> Double { sorted[min(sorted.count - 1, Int(Double(sorted.count - 1) * p))] }
    return ["samples": Double(sorted.count), "p50_ms": percentile(0.5), "p95_ms": percentile(0.95),
            "p99_ms": percentile(0.99), "max_ms": sorted.last!]
}

// The VM runner starts a fresh process per mode/iteration. No timing assertion
// is used in a correctness test; compare the emitted evidence on the same VM.
@Test(.enabled(if: ProcessInfo.processInfo.environment["AZOOKEY_DICTIONARY_BENCHMARK_MODE"] != nil))
func englishDictionaryPerformance() async throws {
    try await MainActor.run {
        let mode = try #require(ProcessInfo.processInfo.environment["AZOOKEY_DICTIONARY_BENCHMARK_MODE"])
        let includeEnglish = mode == "both" || mode == "spelling"
        let windowsTable = try mixedInputWindowsTable()
        let start = englishNow()
        let engine = KanaKanjiConverter(dictionaryURL: englishDictionaryRoot(mode), preloadDictionary: true)
        let supplement = KanaKanjiConverter(dictionaryURL: englishDictionaryRoot(mode), preloadDictionary: true)
        let normalizer = MixedInputConversion(inputStyle: windowsTable.style, table: windowsTable.table,
            includeBundledEnglish: includeEnglish, englishDictionaryEnabled: includeEnglish)
        var modeProbe = ComposingText()
        modeProbe.insertAtCursorPosition("google", inputStyle: windowsTable.style)
        #expect((normalizer.normalize(modeProbe).convertTarget == "google") == includeEnglish)
        let initialization = englishNow() - start
        let memoryAfterLoad = englishMemoryBytes()
        let options = englishOptions()
        let corpora: [(String, [String])] = [
            ("japanese", ["kyouhaiitenkidesu", "nihongonobunshouwonyuuryokusuru", "shamisenwoshimasu", "resettonohouhou"]),
            ("existing_mixed", ["kyouhagoogledekensaku", "pythonnotestwokaku", "githubnipushshita", "gitwotsukau"]),
            ("new_english", ["approachwotsukau", "algorithmwokaku", "repositorywotsukuru", "Approachdesu"]),
            ("katakana", ["apuro-chiwokaeru", "arugorizumuwokaku", "inta-fe-suwotsukuru", "kyasshuwokesu"]),
            ("long", [String(repeating: "nihongodesu", count: 20)])
        ]
        var samples: [String: [Double]] = [:]
        var normalizeSamples: [Double] = []
        var firstKeyMs: Double = 0
        var checksum = 0
        for iteration in 0..<7 {
            print("ENGLISH_DICTIONARY_PROGRESS mode=\(mode) iteration=\(iteration)")
            for (category, sentences) in corpora {
                for keys in sentences {
                    engine.stopComposition()
                    var text = ComposingText()
                    for key in keys {
                        let began = englishNow()
                        text.insertAtCursorPosition(String(key), inputStyle: windowsTable.style)
                        let beforeNormalize = englishNow()
                        text = normalizer.normalize(text)
                        let afterNormalize = englishNow()
                        let result = engine.requestCandidates(text, options: options)
                        let elapsed = englishNow() - began
                        checksum += result.mainResults.count
                        if iteration == 0 && firstKeyMs == 0 { firstKeyMs = elapsed }
                        if iteration > 0 {
                            samples[category, default: []].append(elapsed)
                            normalizeSamples.append(afterNormalize - beforeNormalize)
                        }
                    }
                }
            }
        }
        let all = samples.values.flatMap { $0 }
        let evidence: [String: Any] = ["mode": mode, "initialization_ms": initialization,
            "english_dictionary": includeEnglish, "katakana_english_dictionary": mode == "both" || mode == "kana",
            "mode_contract": "current_settings_including_seed",
            "os": ProcessInfo.processInfo.operatingSystemVersionString,
            "romaji_table": "bundled Windows default",
            "first_key_ms": firstKeyMs, "working_set_after_load_bytes": memoryAfterLoad,
            "working_set_after_typing_bytes": englishMemoryBytes(), "normalization": englishPercentiles(normalizeSamples),
            "append_normalize_candidates": englishPercentiles(all),
            "categories": samples.mapValues(englishPercentiles), "checksum": checksum,
            "preloaded_converter_count": 2, "zenzai": false, "ipc_included": false]
        let bytes = try JSONSerialization.data(withJSONObject: evidence, options: [.sortedKeys])
        if let output = ProcessInfo.processInfo.environment["AZOOKEY_DICTIONARY_BENCHMARK_OUTPUT"] {
            try bytes.write(to: URL(fileURLWithPath: output), options: .atomic)
        } else {
            print("ENGLISH_DICTIONARY_PERFORMANCE " + String(decoding: bytes, as: UTF8.self))
        }
        withExtendedLifetime(supplement) {}
        #expect(checksum > 0)
    }
}
