import Foundation
import KanaKanjiConverterModule
import Testing
#if os(Windows)
import WinSDK
#endif
@testable import azookey_server

private struct ZenzEvaluationFixture: Decodable {
    let schemaVersion: Int
    let cases: [Case]

    struct Case: Decodable {
        let id: String
        let category: String
        let reading: String
        let externalContext: String
        let acceptableOutputs: [String]
    }
}

#if os(Windows)
private let zenzClockFrequency: Double = {
    var frequency = LARGE_INTEGER()
    precondition(QueryPerformanceFrequency(&frequency) && frequency.QuadPart > 0)
    return Double(frequency.QuadPart)
}()
#endif

func zenzNow() -> Double {
    #if os(Windows)
    var counter = LARGE_INTEGER()
    precondition(QueryPerformanceCounter(&counter))
    return Double(counter.QuadPart) * 1_000 / zenzClockFrequency
    #else
    return ProcessInfo.processInfo.systemUptime * 1_000
    #endif
}

func zenzMemory() -> [String: UInt64] {
    #if os(Windows)
    var counters = PROCESS_MEMORY_COUNTERS_EX()
    let size = DWORD(MemoryLayout<PROCESS_MEMORY_COUNTERS_EX>.size)
    counters.cb = size
    let success = withUnsafeMutablePointer(to: &counters) { pointer in
        pointer.withMemoryRebound(to: PROCESS_MEMORY_COUNTERS.self, capacity: 1) {
            K32GetProcessMemoryInfo(GetCurrentProcess(), $0, size)
        }
    }
    if success {
        return ["working_set_bytes": UInt64(counters.WorkingSetSize),
                "peak_working_set_bytes": UInt64(counters.PeakWorkingSetSize),
                "private_usage_bytes": UInt64(counters.PrivateUsage)]
    }
    var basic = PROCESS_MEMORY_COUNTERS()
    let basicSize = DWORD(MemoryLayout<PROCESS_MEMORY_COUNTERS>.size)
    basic.cb = basicSize
    if K32GetProcessMemoryInfo(GetCurrentProcess(), &basic, basicSize) {
        return ["working_set_bytes": UInt64(basic.WorkingSetSize),
                "peak_working_set_bytes": UInt64(basic.PeakWorkingSetSize)]
    }
    #endif
    return [:]
}

func zenzSummary(_ values: [Double]) -> [String: Any] {
    let ordered = values.sorted()
    func percentile(_ p: Double) -> Double { ordered[Int(Double(ordered.count - 1) * p)] }
    return ["count": ordered.count, "p50_ms": percentile(0.50), "p95_ms": percentile(0.95),
            "p99_ms": percentile(0.99), "max_ms": ordered.last!, "samples_ms": values]
}

// Opt-in: one fresh process and one converter per model; the runner supplies a hash-pinned GGUF.
@Test(.enabled(if: ProcessInfo.processInfo.environment["AZOOKEY_ZENZ_EVALUATION_OUTPUT"] != nil))
func zenzModelEvaluation() async throws {
    try await MainActor.run {
        let environment = ProcessInfo.processInfo.environment
        let path = try #require(environment["AZOOKEY_ZENZAI_TEST_MODEL"])
        let output = try #require(environment["AZOOKEY_ZENZ_EVALUATION_OUTPUT"])
        var isDirectory: ObjCBool = false
        try #require(NSString(string: path).isAbsolutePath && path.lowercased().hasSuffix(".gguf")
            && FileManager.default.fileExists(atPath: path, isDirectory: &isDirectory) && !isDirectory.boolValue,
            "AZOOKEY_ZENZAI_TEST_MODEL must be an absolute existing GGUF file")
        try #require(!output.isEmpty, "AZOOKEY_ZENZ_EVALUATION_OUTPUT must name a file")
        let package = URL(filePath: #filePath).deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
        let dictionary = package.deletingLastPathComponent().appending(path: "target/azookey-english-dictionary/Dictionary")
        try #require(FileManager.default.fileExists(atPath: dictionary.appending(path: "english-manifest.json").path),
                     "Generate the English dictionary before Swift tests")
        let fixtureURL = URL(filePath: #filePath).deletingLastPathComponent().appending(path: "Fixtures/jev-evaluation.json")
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        let fixture = try decoder.decode(ZenzEvaluationFixture.self, from: Data(contentsOf: fixtureURL))
        try #require(fixture.schemaVersion == 1 && fixture.cases.count == 40)

        // Engine runtime is process-wide; the VM runner starts a fresh process per model.
        KanaKanjiConverterEngineRuntime.configure(gpuLayerCount: 0)
        let model = URL(fileURLWithPath: path)
        let expectedStatus = "load \(model.absoluteString)"
        let started = zenzNow()
        let engine = KanaKanjiConverter(dictionaryURL: dictionary, preloadDictionary: true)
        defer { engine.stopComposition() }
        let initializationMs = zenzNow() - started
        let memoryAfterInitialization = zenzMemory()
        let emoji = package.appending(path: "azooKey_emoji_dictionary_storage/EmojiDictionary/emoji_all_E15.1.txt")
        var options = ConvertRequestOptions(
            N_best: 10, requireJapanesePrediction: .disabled, requireEnglishPrediction: .disabled,
            keyboardLanguage: .ja_JP, learningType: .nothing,
            memoryDirectoryURL: package, sharedContainerURL: package,
            textReplacer: .init { emoji }, specialCandidateProviders: nil,
            zenzaiMode: .off, metadata: .init(versionString: "Zenz model evaluation")
        )
        func setContext(_ context: String) {
            options.zenzaiMode = .on(weight: model, inferenceLimit: 1, requestRichCandidates: true,
                personalizationMode: nil, versionDependentMode: .v3(.init(profile: "", leftSideContext: context)))
        }
        @MainActor func fullCandidates(_ original: ComposingText, _ preview: ComposingText, _ result: ConversionResult) -> [String] {
            result.mainResults.compactMap { candidate in
                let consumed = resolveCandidateCompositionForDisplay(originalComposingText: original,
                    previewComposingText: preview, candidateComposingCount: candidate.composingCount)
                guard consumed.correspondingCount == original.input.count,
                      consumed.remainingConvertTarget.isEmpty else { return nil }
                let surface = constructCandidateString(candidate: candidate, hiragana: preview.convertTarget)
                return surface.isEmpty ? nil : surface
            }
        }
        setContext("")
        engine.stopComposition()
        var first = ComposingText()
        first.insertAtCursorPosition("にほんご", inputStyle: .direct)
        print("ZENZ_EVALUATION_PROGRESS cold model=\(model.lastPathComponent)")
        let coldStart = zenzNow()
        let coldResult = engine.requestCandidates(first, options: options)
        let coldMs = zenzNow() - coldStart
        try #require(!fullCandidates(first, first, coldResult).isEmpty && engine.zenzStatus == expectedStatus)
        let memoryAfterCold = zenzMemory()

        // The server's live gate requires >=4 raw keys AND >=2 hiragana characters.
        // Timing covers requestCandidates only, not insertion, preview preparation, IPC, UI or normal-N-best supplement.
        let sentences: [(category: String, keys: String)] = [
            ("normal", "kyouhaiitenkidesu"), ("normal", "nihongonobunshouwonyuuryokusuru"),
            ("normal", "shamisenwoshimasu"), ("normal", "resettonohouhou"),
            ("medium", "nihongonobunshouwonyuuryokushitekakuninshimasukyoumoyoroshiku")
        ]
        var samples: [String: [Double]] = [:]
        var finalTops: [[String: Any]] = []
        @MainActor func livePass(_ iteration: Int) throws {
            for sentence in sentences {
                engine.stopComposition()
                var text = ComposingText()
                var final = [String]()
                for (index, key) in sentence.keys.enumerated() {
                    text.insertAtCursorPosition(String(key), inputStyle: .roman2kana)
                    let preview = makeCandidatePreviewComposingText(from: text).composingText
                    let enabled = effectiveZenzaiEnabledForCandidates(isConfigured: true,
                        inputCount: text.input.count, hiraganaCount: text.convertTarget.count)
                    if enabled { setContext("") } else { options.zenzaiMode = .off }
                    let begin = zenzNow()
                    let result = engine.requestCandidates(preview, options: options)
                    let elapsed = zenzNow() - begin
                    if iteration > 0 { samples["\(sentence.category):\(index + 1):\(enabled)", default: []].append(elapsed) }
                    if index == sentence.keys.count - 1 { final = fullCandidates(text, preview, result) }
                    if index % 8 == 0 { print("ZENZ_EVALUATION_PROGRESS live_pass=\(iteration) category=\(sentence.category) key=\(index + 1)/\(sentence.keys.count)") }
                }
                try #require(!final.isEmpty && engine.zenzStatus == expectedStatus,
                             "Live conversion/model status: \(sentence.keys)")
                if iteration == 3 {
                    finalTops.append(["category": sentence.category, "keys": sentence.keys,
                                      "key_count": sentence.keys.count, "top": final[0]])
                }
            }
        }
        try livePass(0) // Untimed warmup, same sequence as the three measured passes.
        let memoryAfterWarmup = zenzMemory()

        var quality: [[String: Any]] = []
        for withContext in [false, true] {
            for (index, item) in fixture.cases.enumerated() {
                engine.stopComposition()
                let context = withContext ? item.externalContext : ""
                setContext(context)
                var text = ComposingText()
                text.insertAtCursorPosition(item.reading, inputStyle: .direct)
                let preview = makeCandidatePreviewComposingText(from: text).composingText
                let begin = zenzNow()
                let result = engine.requestCandidates(preview, options: options)
                let latency = zenzNow() - begin
                let candidates = fullCandidates(text, preview, result)
                try #require(!candidates.isEmpty && engine.zenzStatus == expectedStatus,
                             "Model/full-sentence candidate missing: \(item.id)")
                quality.append(["id": item.id, "category": item.category, "reading": item.reading,
                    "context_mode": withContext ? "external" : "off", "context": context,
                    "acceptable": item.acceptableOutputs, "top": candidates[0], "candidates": candidates,
                    "top1_accepted": item.acceptableOutputs.contains(candidates[0]),
                    "any_accepted": candidates.contains(where: item.acceptableOutputs.contains), "latency_ms": latency])
                if index % 5 == 0 { print("ZENZ_EVALUATION_PROGRESS quality_context=\(withContext) case=\(index + 1)/\(fixture.cases.count)") }
            }
        }
        for pass in 1...3 { try livePass(pass) }
        let memoryFinal = zenzMemory()
        #if os(Windows)
        try #require([memoryAfterInitialization, memoryAfterCold, memoryAfterWarmup, memoryFinal]
            .allSatisfy { $0["working_set_bytes"] != nil && $0["peak_working_set_bytes"] != nil },
            "Windows process memory counters unavailable")
        #endif
        try #require(quality.count == 80 && samples.count > 0 && quality.allSatisfy {
            ($0["latency_ms"] as? Double).map { $0.isFinite && $0 >= 0 } ?? false
        } && [initializationMs, coldMs].allSatisfy { $0.isFinite && $0 >= 0 }
            && samples.values.flatMap { $0 }.allSatisfy { $0.isFinite && $0 >= 0 }
            && engine.zenzStatus == expectedStatus)
        let perKey: [[String: Any]] = samples.keys.sorted().map { key in
            let pieces = key.split(separator: ":")
            return ["category": String(pieces[0]), "key_length": Int(pieces[1])!,
                    "zenzai_enabled": pieces[2] == "true", "latency": zenzSummary(samples[key]!)]
        }
        let report: [String: Any] = [
            "schema_version": 1, "model_path": model.path, "model_status": engine.zenzStatus,
            "os": ProcessInfo.processInfo.operatingSystemVersionString,
            "processors": ProcessInfo.processInfo.activeProcessorCount,
            "options": ["dictionary": dictionary.path, "preload": true, "gpu_layers": 0,
                "learning": "nothing", "predictions": "disabled", "n_best": 10,
                "inference_limit": 1, "rich_candidates": true, "profile": "",
                "context_modes": ["off", "external"], "romaji_input_style": "roman2kana",
                "live_gate": "raw_keys>=4 && hiragana>=2"],
            "scope": "engine_only_requestCandidates; excludes IPC/UI/normalNBest supplement; model load status is not an independent decode proof",
            "initialization_ms": initializationMs, "cold_first_request_ms": coldMs,
            "memory_after_initialization": memoryAfterInitialization, "memory_after_cold": memoryAfterCold,
            "memory_after_warmup": memoryAfterWarmup, "memory_final": memoryFinal,
            "quality": quality, "live": ["warmup_passes": 1, "measured_passes": 3,
                "per_key": perKey, "final_tops": finalTops]
        ]
        let data = try JSONSerialization.data(withJSONObject: report, options: [.sortedKeys])
        let url = URL(fileURLWithPath: output)
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try data.write(to: url, options: .atomic)
        print("ZENZ_EVALUATION_REPORT \(url.path) quality=\(quality.count) key_samples=\(samples.values.reduce(0) { $0 + $1.count })")
    }
}
