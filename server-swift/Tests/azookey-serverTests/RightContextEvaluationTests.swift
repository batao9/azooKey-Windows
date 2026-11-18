import Foundation
import KanaKanjiConverterModule
import Testing
@testable import azookey_server

// Public synthetic insertion cases. Expected output is only the converted span,
// never its committed right-hand continuation. Keep failures in the report.
private let rightContextCases: [(keys: String, right: String, expected: [String])] = [
    ("hashi", "を渡る。", ["橋"]), ("hashi", "でご飯を食べる。", ["箸"]),
    ("hashi", "に寄せて置く。", ["端"]),
    ("kisha", "に乗って旅に出た。", ["汽車"]), ("kisha", "が現場で取材した。", ["記者"]),
    ("kisha", "の製品について伺います。", ["貴社"]),
    ("kami", "に字を書く。", ["紙"]), ("kami", "を切ってもらう。", ["髪"]),
    ("kami", "に祈りを捧げる。", ["神"]),
    ("ame", "が降っている。", ["雨"]), ("ame", "をなめる。", ["飴"]),
    ("hana", "が咲いている。", ["花"]), ("hana", "をかんだ。", ["鼻"]),
    ("ishi", "を拾った。", ["石"]), ("ishi", "が患者を診察した。", ["医師"]),
    ("ishi", "を強く持つ。", ["意志", "意思"]),
    ("kikan", "誌を読む。", ["機関"]), ("kikan", "を延長した。", ["期間"]),
    ("kikan", "支が炎症を起こした。", ["気管"]),
    ("kouen", "で子どもが遊ぶ。", ["公園"]), ("kouen", "を聴講した。", ["講演"]),
    ("kouen", "のチケットを予約した。", ["公演"]),
    ("kousei", "員の名簿を作る。", ["構成"]), ("kousei", "刷りで誤字を直す。", ["校正"])
]

@Test(.enabled(if: ProcessInfo.processInfo.environment["AZOOKEY_ZENZ_RIGHT_EVALUATION_OUTPUT"] != nil))
func rightContextEvaluation() async throws {
    try await MainActor.run {
        let environment = ProcessInfo.processInfo.environment
        let model = URL(fileURLWithPath: try #require(environment["AZOOKEY_ZENZAI_TEST_MODEL"]))
        let output = try #require(environment["AZOOKEY_ZENZ_RIGHT_EVALUATION_OUTPUT"])
        try #require(model.lastPathComponent == "zenz-v3.2-small.gguf" && FileManager.default.fileExists(atPath: model.path))
        let package = URL(filePath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        let dictionary = package.deletingLastPathComponent().appending(path: "target/azookey-english-dictionary/Dictionary")
        try #require(FileManager.default.fileExists(atPath: dictionary.appending(path: "english-manifest.json").path))
        let oldConverter = converter, oldSupplement = normalNBestSupplementConverter
        let oldText = composingText, oldSnapshots = composingTextSnapshots
        let oldConfig = config, oldURL = execURL, oldLearning = currentLearningType
        let oldStyle = currentInputStyle, oldTable = currentRomajiInputTable
        let oldCache = learningCandidateCache
        defer {
            converter = oldConverter; normalNBestSupplementConverter = oldSupplement
            composingText = oldText; composingTextSnapshots = oldSnapshots
            config = oldConfig; execURL = oldURL; currentLearningType = oldLearning
            currentInputStyle = oldStyle; currentRomajiInputTable = oldTable; learningCandidateCache = oldCache
        }
        KanaKanjiConverterEngineRuntime.configure(gpuLayerCount: 0)
        converter = KanaKanjiConverter(dictionaryURL: dictionary, preloadDictionary: true)
        normalNBestSupplementConverter = KanaKanjiConverter(dictionaryURL: dictionary, preloadDictionary: true)
        config = ["enable": true, "backend": "cpu", "rightContextSupported": true, "modelFilename": model.lastPathComponent]
        execURL = model.deletingLastPathComponent()
        currentLearningType = .nothing; currentInputStyle = .roman2kana; currentRomajiInputTable = .defaultRomanToKana
        learningCandidateCache.removeAll()
        let expectedStatus = "load \(model.absoluteString)"
        @MainActor func type(_ keys: String) {
            var cursor: CInt = 0
            for key in keys { String(key).withCString { free_c_string(append_text(input: $0, cursorPtr: &cursor)) } }
        }
        @MainActor func candidates() -> (texts: [String], elapsed: Double) {
            var length: CInt = 0
            let start = zenzNow()
            let list = get_composed_text(lengthPtr: &length)
            let elapsed = zenzNow() - start
            defer { free_candidate_list(list, length) }
            let texts: [String] = (0..<Int(length)).compactMap { index in
                guard let value = list[index]?.pointee, value.correspondingCount == Int32(composingText.input.count),
                      value.subtext.map({ String(cString: $0).isEmpty }) == true,
                      let text = value.text else { return nil }
                return String(cString: text)
            }
            return (texts, elapsed)
        }
        var quality: [[String: Any]] = []
        for mode in ["kkc", "off", "left", "right", "both"] {
            for (index, item) in rightContextCases.enumerated() {
                clear_text()
                config["enable"] = mode != "kkc"
                setSurroundingContext(left: mode == "left" || mode == "both" ? "その" : "",
                                      right: mode == "right" || mode == "both" ? item.right : "")
                type(item.keys)
                let result = candidates()
                try #require(!result.texts.isEmpty && (mode == "kkc" || converter.zenzStatus == expectedStatus))
                quality.append(["id": index, "keys": item.keys, "right": item.right, "mode": mode,
                    "acceptable": item.expected, "top": result.texts[0], "candidates": result.texts,
                    "top1_accepted": item.expected.contains(result.texts[0]),
                    "any_accepted": result.texts.contains(where: item.expected.contains), "latency_ms": result.elapsed,
                    "neural_gate": mode != "kkc" && effectiveZenzaiEnabledForCandidates(isConfigured: true,
                        inputCount: composingText.input.count, hiraganaCount: composingText.convertTarget.count)])
                if index % 6 == 0 { print("RIGHT_CONTEXT_PROGRESS quality=\(mode) case=\(index)") }
            }
        }
        // Right-only changes on identical input must agree with a fresh converter
        // session, not retain an accepted prefix from the previous prompt.
        var transitions: [[String: Any]] = []
        clear_text(); type("hashi")
        config["enable"] = true
        for right in ["を渡る。", "でご飯を食べる。", "を渡る。", "", "\n😀を使う。"] {
            setSurroundingContext(left: "", right: right)
            let warm = candidates().texts
            converter.stopComposition(); normalNBestSupplementConverter.stopComposition()
            let fresh = candidates().texts
            try #require(!warm.isEmpty && warm.first == fresh.first, "Stale right-context constraint: \(right)")
            transitions.append(["right": right, "warm_top": warm[0], "fresh_top": fresh[0]])
        }
        // Exact edit history through the production Swift FFI. This does not
        // substitute for TSF focus/range or visible UI evidence.
        clear_text(); setSurroundingContext(left: "川の", right: "を渡る。")
        type("hashi"); _ = candidates()
        var cursor: CInt = 0
        free_c_string(remove_text(cursorPtr: &cursor))
        try #require(composingText.convertTarget == "は")
        type("shi")
        try #require(composingText.convertTarget == "はし")
        _ = candidates()
        reset_text_for_recovery()
        try #require(config["context"] as? String == "" && config["rightContext"] as? String == "")
        type("hashi"); let afterClear = candidates().texts
        clear_text()
        var live: [[String: Any]] = []
        let modes = environment["AZOOKEY_ZENZ_RIGHT_EVALUATION_REVERSE"] == "1" ? ["long", "short", "off"] : ["off", "short", "long"]
        for mode in modes {
            let right = mode == "off" ? "" : mode == "short" ? "を確認してから送信します。" : String(repeating: "を確認してから送信します。", count: 4)
            var normal: [Double] = [], medium: [Double] = []
            for pass in 0...3 {
                for keys in ["kyouhaiitenkidesu", "nihongonobunshouwonyuuryokusuru", "shamisenwoshimasu",
                             "nihongonobunshouwonyuuryokushitekakuninshimasukyoumoyoroshiku"] {
                    clear_text(); setSurroundingContext(left: "", right: right)
                    for (index, key) in keys.enumerated() {
                        type(String(key))
                        let result = candidates()
                        if pass > 0 && effectiveZenzaiEnabledForCandidates(isConfigured: true,
                            inputCount: composingText.input.count, hiraganaCount: composingText.convertTarget.count) {
                            if keys.count > 40 { medium.append(result.elapsed) } else { normal.append(result.elapsed) }
                        }
                        if index % 12 == 0 { print("RIGHT_CONTEXT_PROGRESS live=\(mode) pass=\(pass) key=\(index)/\(keys.count)") }
                    }
                    try #require(converter.zenzStatus == expectedStatus)
                }
            }
            try #require(!normal.isEmpty && !medium.isEmpty && (normal + medium).allSatisfy { $0.isFinite && $0 >= 0 })
            live.append(["mode": mode, "right_characters": min(40, right.count), "normal": zenzSummary(normal),
                         "medium": zenzSummary(medium), "memory": zenzMemory()])
        }
        let report: [String: Any] = ["schema_version": 1, "model_path": model.path, "model_status": converter.zenzStatus,
            "os": ProcessInfo.processInfo.operatingSystemVersionString, "processors": ProcessInfo.processInfo.activeProcessorCount,
            "scope": "production Swift GetComposedText FFI including normal N-best supplement and candidate allocation; excludes Rust IPC/TSF/UI; memory is whole test process",
            "quality": quality, "right_only_transitions": transitions, "after_clear_top": afterClear.first ?? "",
            "live": live, "measured_passes": 3, "warmup_passes": 1]
        try JSONSerialization.data(withJSONObject: report, options: [.sortedKeys]).write(to: URL(fileURLWithPath: output), options: .atomic)
        print("RIGHT_CONTEXT_REPORT \(output) cases=\(quality.count)")
    }
}
