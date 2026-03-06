import KanaKanjiConverterModule
import Foundation
import ffi

@MainActor let converter = KanaKanjiConverter()
@MainActor var composingText = ComposingText()
@MainActor var composingTextSnapshots: [ComposingText] = []
@MainActor var currentInputStyle: InputStyle = .roman2kana
@MainActor var customRomajiTableURL: URL?

@MainActor var execURL = URL(filePath: "")
@MainActor var config: [String : Any] = [
    "enable": false,
    "profile": "",
    "backend": "cpu",
]
let maxUserDictionaryEntryCount = 50
let minInputCountForZenzaiCandidates = 4
let minHiraganaCountForZenzaiCandidates = 2
let serverLogFileName = "server.log"

private enum ServerLogLevel: Int {
    case off = 0
    case error = 1
    case warn = 2
    case info = 3
    case debug = 4

    init(label: String) {
        switch label.lowercased() {
        case "off":
            self = .off
        case "error", "panic":
            self = .error
        case "warn", "warning":
            self = .warn
        case "debug":
            self = .debug
        default:
            self = .info
        }
    }

    static func fromEnvironment() -> Self {
        guard let rawValue = ProcessInfo.processInfo.environment["AZOOKEY_SERVER_LOG_LEVEL"] else {
            return .warn
        }
        return .init(label: rawValue)
    }
}

private let serverLogThreshold = ServerLogLevel.fromEnvironment()

private func serverLogPath() -> URL {
    if let appDataPath = ProcessInfo.processInfo.environment["APPDATA"] {
        return URL(filePath: appDataPath)
            .appendingPathComponent("Azookey")
            .appendingPathComponent("logs")
            .appendingPathComponent(serverLogFileName)
    }

    return FileManager.default.temporaryDirectory
        .appendingPathComponent("Azookey")
        .appendingPathComponent("logs")
        .appendingPathComponent(serverLogFileName)
}

private func serverLogTimestampMillis() -> UInt64 {
    UInt64(Date().timeIntervalSince1970 * 1000)
}

private func serverLog(_ level: String = "INFO", _ message: @autoclosure () -> String) {
    let resolvedLevel = ServerLogLevel(label: level)
    guard resolvedLevel.rawValue <= serverLogThreshold.rawValue else {
        return
    }

    let resolvedMessage = message()
    let line = "[\(serverLogTimestampMillis())] [SWIFT/\(level)] \(resolvedMessage)"
    fputs("\(line)\n", stderr)

    let logPath = serverLogPath()
    let logDirectory = logPath.deletingLastPathComponent()

    do {
        try FileManager.default.createDirectory(
            at: logDirectory,
            withIntermediateDirectories: true
        )
    } catch {
        fputs("Failed to create log directory: \(error)\n", stderr)
        return
    }

    if !FileManager.default.fileExists(atPath: logPath.path) {
        FileManager.default.createFile(atPath: logPath.path, contents: nil)
    }

    guard let handle = FileHandle(forWritingAtPath: logPath.path) else {
        fputs("Failed to open log file at \(logPath.path)\n", stderr)
        return
    }

    defer {
        handle.closeFile()
    }

    guard let data = "\(line)\n".data(using: .utf8) else {
        return
    }

    handle.seekToEndOfFile()
    handle.write(data)
}

private struct AppSettings: Decodable {
    let zenzai: ZenzaiSettings?
    let user_dictionary: UserDictionarySettings?
    let romaji_table: RomajiTableSettings?
}

private struct ZenzaiSettings: Decodable {
    let enable: Bool?
    let profile: String?
    let backend: String?
}

private struct UserDictionarySettings: Decodable {
    let entries: [UserDictionaryEntry]?
}

private struct UserDictionaryEntry: Decodable {
    let reading: String
    let word: String
}

private struct RomajiTableSettings: Decodable {
    let rows: [RomajiTableRow]?
}

enum RomajiInputStyleSelection: Equatable {
    case roman2kana
    case custom
}

private func normalizeReading(_ reading: String) -> String {
    reading.applyingTransform(.hiraganaToKatakana, reverse: false) ?? reading
}

func resolveRomajiInputStyleSelection(
    rows: [RomajiTableRow]?,
    isZenzaiEnabled: Bool
) -> RomajiInputStyleSelection {
    if isZenzaiEnabled {
        return .roman2kana
    }

    guard let rows, buildCustomRomajiTableContent(rows: rows) != nil else {
        return .roman2kana
    }

    return .custom
}

func effectiveZenzaiEnabledForCandidates(
    isConfigured: Bool,
    inputCount: Int,
    hiraganaCount: Int
) -> Bool {
    isConfigured
        && inputCount >= minInputCountForZenzaiCandidates
        && hiraganaCount >= minHiraganaCountForZenzaiCandidates
}

func effectiveZenzaiRuntimeEnabled(
    isConfigured: Bool,
    backend: String?,
    cpuBackendSupported: Bool
) -> Bool {
    guard isConfigured else {
        return false
    }

    let normalizedBackend = (backend ?? "cpu")
        .trimmingCharacters(in: .whitespacesAndNewlines)
        .lowercased()

    if normalizedBackend.isEmpty || normalizedBackend == "cpu" {
        return cpuBackendSupported
    }

    return true
}

private func cpuZenzaiBackendSupportedFromEnvironment() -> Bool {
    ProcessInfo.processInfo.environment["AZOOKEY_ZENZAI_CPU_SUPPORTED"] != "0"
}

@MainActor private func setRoman2KanaInputStyle() {
    currentInputStyle = .roman2kana

    if let existing = customRomajiTableURL {
        try? FileManager.default.removeItem(at: existing)
    }

    customRomajiTableURL = nil
}

@MainActor private func setCustomRomajiInputStyle(rows: [RomajiTableRow]?) {
    guard let rows, let content = buildCustomRomajiTableContent(rows: rows) else {
        setRoman2KanaInputStyle()
        return
    }

    let fileURL = FileManager.default.temporaryDirectory
        .appendingPathComponent("azookey-romaji-\(UUID().uuidString).tsv")

    do {
        try content.write(to: fileURL, atomically: true, encoding: .utf8)
        let previousURL = customRomajiTableURL
        currentInputStyle = .mapped(id: .custom(fileURL))
        customRomajiTableURL = fileURL

        if let previousURL, previousURL != fileURL {
            try? FileManager.default.removeItem(at: previousURL)
        }
    } catch {
        serverLog("ERROR", "Failed to apply custom romaji table: \(error)")
        setRoman2KanaInputStyle()
    }
}

@MainActor private func applyRomajiInputStyle(
    rows: [RomajiTableRow]?,
    isZenzaiEnabled: Bool
) {
    switch resolveRomajiInputStyleSelection(
        rows: rows,
        isZenzaiEnabled: isZenzaiEnabled
    ) {
    case .roman2kana:
        setRoman2KanaInputStyle()
    case .custom:
        setCustomRomajiInputStyle(rows: rows)
    }
}

private func clampedCorrespondingCount(
    composingText: ComposingText,
    rawCount: Int
) -> Int {
    min(composingText.input.count, max(0, rawCount))
}

func legacyCorrespondingCount(from composingCount: ComposingCount) -> Int {
    var stack: [ComposingCount] = [composingCount]
    var total = 0

    while let current = stack.popLast() {
        switch current {
        case .inputCount(let count):
            total += max(0, count)
        case .surfaceCount(let count):
            total += max(0, count)
        case .composite(let left, let right):
            stack.append(left)
            stack.append(right)
        }
    }

    return max(0, total)
}

@MainActor private func resolveCorrespondingCount(
    composingText: ComposingText,
    candidateComposingCount: ComposingCount,
    isZenzaiEnabled: Bool
) -> Int {
    if isZenzaiEnabled {
        return clampedCorrespondingCount(
            composingText: composingText,
            rawCount: legacyCorrespondingCount(from: candidateComposingCount)
        )
    }

    var afterComposingText = composingText
    afterComposingText.prefixComplete(composingCount: candidateComposingCount)
    return clampedCorrespondingCount(
        composingText: composingText,
        rawCount: composingText.input.count - afterComposingText.input.count
    )
}

@MainActor private func resolveSubtext(
    composingText: ComposingText,
    correspondingCount: Int,
    isZenzaiEnabled: Bool
) -> String {
    if isZenzaiEnabled {
        return ""
    }

    var afterComposingText = composingText
    afterComposingText.prefixComplete(composingCount: .inputCount(correspondingCount))
    return afterComposingText.convertTarget
}

@MainActor private func resolveCursorPrefixSubtext(
    prefixComposingText: ComposingText,
    suffixAfterCursor: String,
    correspondingCount: Int,
    isZenzaiEnabled: Bool
) -> String {
    if isZenzaiEnabled {
        return suffixAfterCursor
    }

    var remainingPrefixComposingText = prefixComposingText
    remainingPrefixComposingText.prefixComplete(
        composingCount: .inputCount(correspondingCount)
    )
    return remainingPrefixComposingText.convertTarget + suffixAfterCursor
}

@MainActor private func debugLogResolvedCorrespondingCount(
    scope: String,
    candidateIndex: Int,
    candidateTotal: Int,
    candidateComposingCount: ComposingCount,
    resolvedCorrespondingCount: Int,
    inputCount: Int,
    isZenzaiEnabled: Bool
) {
    let mode = isZenzaiEnabled ? "zenzai" : "standard"
    serverLog(
        "DEBUG",
        "[\(scope)] mode=\(mode) candidate[\(candidateIndex + 1)/\(candidateTotal)] composingCount=\(candidateComposingCount) resolvedCorrespondingCount=\(resolvedCorrespondingCount) inputCount=\(inputCount)"
    )
}

@MainActor func getOptions(context: String = "") -> ConvertRequestOptions {
    getOptions(
        context: context,
        zenzaiEnabled: effectiveZenzaiRuntimeEnabled(
            isConfigured: (config["enable"] as? Bool) ?? false,
            backend: config["backend"] as? String,
            cpuBackendSupported: cpuZenzaiBackendSupportedFromEnvironment()
        )
    )
}

@MainActor func getOptions(
    context: String = "",
    zenzaiEnabled: Bool
) -> ConvertRequestOptions {
    let profile = (config["profile"] as? String) ?? ""
    return ConvertRequestOptions(
        requireJapanesePrediction: true,
        requireEnglishPrediction: false,
        keyboardLanguage: .ja_JP,
        learningType: .nothing,
        dictionaryResourceURL: execURL.appendingPathComponent("Dictionary"),
        memoryDirectoryURL: URL(filePath: "./test"),
        sharedContainerURL: URL(filePath: "./test"),
        textReplacer: .init {
            return execURL.appendingPathComponent("EmojiDictionary").appendingPathComponent("emoji_all_E15.1.txt")
        },
        // zenzai
        zenzaiMode: zenzaiEnabled ? .on(
            weight: execURL.appendingPathComponent("zenz.gguf"),
            inferenceLimit: 1,
            requestRichCandidates: false,
            personalizationMode: nil,
            versionDependentMode: .v3(
                .init(
                    profile: profile,
                    leftSideContext: context
                )
            )
        ) : .off,
        preloadDictionary: true,
        metadata: .init(versionString: "Azookey for Windows")
    )
}

class SimpleComposingText {
    init(text: String, cursor: Int) {
        self.text = UnsafeMutablePointer<CChar>(mutating: text.utf8String)!
        self.cursor = cursor
    }

    var text: UnsafeMutablePointer<CChar>
    var cursor: Int
}

struct SComposingText {
    var text: UnsafeMutablePointer<CChar>
    var cursor: Int
}

func constructCandidateString(candidate: Candidate, hiragana: String) -> String {
    var remainingHiragana = hiragana
    var result = ""
    
    for data in candidate.data {
        if remainingHiragana.count < data.ruby.count {
            result += remainingHiragana
            break
        }
        remainingHiragana.removeFirst(data.ruby.count)
        result += data.word
    }
    
    return result
}

@_silgen_name("LoadConfig")
@MainActor public func load_config() {
    serverLog("INFO", "LoadConfig: start")
    let previousZenzaiEnabled = (config["enable"] as? Bool) ?? false
    let previousProfile = (config["profile"] as? String) ?? ""
    let previousBackend = (config["backend"] as? String) ?? "cpu"
    let previousEffectiveZenzaiEnabled = effectiveZenzaiRuntimeEnabled(
        isConfigured: previousZenzaiEnabled,
        backend: previousBackend,
        cpuBackendSupported: cpuZenzaiBackendSupportedFromEnvironment()
    )
    let previousUsedCustomRomajiTable = customRomajiTableURL != nil
    var dynamicUserDictionary: [DicdataElement] = []
    defer {
        converter.sendToDicdataStore(.importDynamicUserDict(dynamicUserDictionary))
    }

    config["enable"] = false
    config["profile"] = ""
    config["backend"] = "cpu"
    setRoman2KanaInputStyle()

    if let appDataPath = ProcessInfo.processInfo.environment["APPDATA"] {
        let settingsPath = URL(filePath: appDataPath).appendingPathComponent("Azookey/settings.json")
        serverLog("INFO", "LoadConfig: reading settingsPath=\(settingsPath.path)")
        
        do {
            let data = try Data(contentsOf: settingsPath)
            let settings = try JSONDecoder().decode(AppSettings.self, from: data)

            if let zenzai = settings.zenzai {
                if let enableValue = zenzai.enable {
                    config["enable"] = enableValue
                }

                if let profileValue = zenzai.profile {
                    config["profile"] = profileValue
                }

                if let backendValue = zenzai.backend {
                    config["backend"] = backendValue
                }
            }

            let isZenzaiEnabled = effectiveZenzaiRuntimeEnabled(
                isConfigured: (config["enable"] as? Bool) ?? false,
                backend: config["backend"] as? String,
                cpuBackendSupported: cpuZenzaiBackendSupportedFromEnvironment()
            )
            applyRomajiInputStyle(
                rows: settings.romaji_table?.rows,
                isZenzaiEnabled: isZenzaiEnabled
            )

            let sourceEntries = settings.user_dictionary?.entries ?? []
            var seen: Set<String> = []
            var priorityRank = 0
            for entry in sourceEntries {
                if dynamicUserDictionary.count >= maxUserDictionaryEntryCount {
                    break
                }

                let reading = entry.reading.trimmingCharacters(in: .whitespacesAndNewlines)
                let word = entry.word.trimmingCharacters(in: .whitespacesAndNewlines)
                if reading.isEmpty || word.isEmpty {
                    continue
                }

                let normalizedReading = normalizeReading(reading)
                let key = normalizedReading + "\u{0}" + word
                if seen.contains(key) {
                    continue
                }
                seen.insert(key)

                let priorityAdjustedValue = PValue(-5 - Float(priorityRank) * 0.01)
                dynamicUserDictionary.append(
                    DicdataElement(
                        word: word,
                        ruby: normalizedReading,
                        cid: CIDData.固有名詞.cid,
                        mid: MIDData.一般.mid,
                        value: priorityAdjustedValue
                    )
                )
                priorityRank += 1
            }

            if sourceEntries.count > maxUserDictionaryEntryCount {
                serverLog("WARN", "User dictionary entries are truncated to \(maxUserDictionaryEntryCount).")
            }
        } catch {
            serverLog("ERROR", "Failed to read settings: \(error)")
        }
    } else {
        serverLog("WARN", "LoadConfig: APPDATA is not set. Using defaults.")
    }

    let currentZenzaiEnabled = (config["enable"] as? Bool) ?? false
    let currentProfile = (config["profile"] as? String) ?? ""
    let currentBackend = (config["backend"] as? String) ?? "cpu"
    let currentEffectiveZenzaiEnabled = effectiveZenzaiRuntimeEnabled(
        isConfigured: currentZenzaiEnabled,
        backend: currentBackend,
        cpuBackendSupported: cpuZenzaiBackendSupportedFromEnvironment()
    )
    let currentUsedCustomRomajiTable = customRomajiTableURL != nil
    if previousEffectiveZenzaiEnabled != currentEffectiveZenzaiEnabled
        || previousProfile != currentProfile
        || previousUsedCustomRomajiTable != currentUsedCustomRomajiTable
    {
        converter.stopComposition()
        composingText = ComposingText()
        composingTextSnapshots.removeAll()
    }

    serverLog(
        "INFO",
        "LoadConfig: completed enable=\(currentZenzaiEnabled) backend=\(currentBackend) effectiveEnable=\(currentEffectiveZenzaiEnabled) customRomaji=\(currentUsedCustomRomajiTable)"
    )
}

@_silgen_name("Initialize")
@MainActor public func initialize(
    path: UnsafePointer<CChar>,
    use_zenzai: Bool
) {
    let path = String(cString: path)
    serverLog("INFO", "Initialize: start path=\(path) use_zenzai=\(use_zenzai)")
    execURL = URL(filePath: path)

    load_config()

    composingText.insertAtCursorPosition("a", inputStyle: currentInputStyle)
    let useZenzaiForWarmup = effectiveZenzaiEnabledForCandidates(
        isConfigured: effectiveZenzaiRuntimeEnabled(
            isConfigured: (config["enable"] as? Bool) ?? false,
            backend: config["backend"] as? String,
            cpuBackendSupported: cpuZenzaiBackendSupportedFromEnvironment()
        ),
        inputCount: composingText.input.count,
        hiraganaCount: composingText.convertTarget.count
    )
    converter.requestCandidates(
        composingText,
        options: getOptions(zenzaiEnabled: useZenzaiForWarmup)
    )
    composingText = ComposingText()
    composingTextSnapshots.removeAll()
    serverLog(
        "INFO",
        "Initialize: completed inputStyle=\(String(describing: currentInputStyle)) warmupUseZenzai=\(useZenzaiForWarmup)"
    )
}

@_silgen_name("AppendText")
@MainActor public func append_text(
    input: UnsafePointer<CChar>,
    cursorPtr: UnsafeMutablePointer<Int>
) -> UnsafeMutablePointer<CChar> {
    let inputString = String(cString: input)
    serverLog("INFO", "AppendText: start inputLength=\(inputString.count) inputStyle=\(String(describing: currentInputStyle))")
    composingText.insertAtCursorPosition(inputString, inputStyle: currentInputStyle)

    cursorPtr.pointee = composingText.convertTargetCursorPosition
    serverLog(
        "INFO",
        "AppendText: completed cursor=\(cursorPtr.pointee) hiraganaLength=\(composingText.convertTarget.count) inputCount=\(composingText.input.count)"
    )
    return _strdup(composingText.convertTarget)!
}

@_silgen_name("AppendTextDirect")
@MainActor public func append_text_direct(
    input: UnsafePointer<CChar>,
    cursorPtr: UnsafeMutablePointer<Int>
) -> UnsafeMutablePointer<CChar> {
    let inputString = String(cString: input)
    serverLog("INFO", "AppendTextDirect: start inputLength=\(inputString.count)")
    composingText.insertAtCursorPosition(inputString, inputStyle: .direct)

    cursorPtr.pointee = composingText.convertTargetCursorPosition
    serverLog(
        "INFO",
        "AppendTextDirect: completed cursor=\(cursorPtr.pointee) hiraganaLength=\(composingText.convertTarget.count)"
    )
    return _strdup(composingText.convertTarget)!
}

@_silgen_name("RemoveText")
@MainActor public func remove_text(
    cursorPtr: UnsafeMutablePointer<Int>
) -> UnsafeMutablePointer<CChar> {
    serverLog("INFO", "RemoveText: start")
    composingText.deleteBackwardFromCursorPosition(count: 1)

    cursorPtr.pointee = composingText.convertTargetCursorPosition
    serverLog(
        "INFO",
        "RemoveText: completed cursor=\(cursorPtr.pointee) hiraganaLength=\(composingText.convertTarget.count) inputCount=\(composingText.input.count)"
    )
    return _strdup(composingText.convertTarget)!
}

@_silgen_name("MoveCursor")
@MainActor public func move_cursor(
    offset: Int32,
    cursorPtr: UnsafeMutablePointer<Int>
) -> UnsafeMutablePointer<CChar> {
    serverLog("INFO", "MoveCursor: start offset=\(offset)")
    if offset == 125 {
        composingTextSnapshots.removeAll()
        cursorPtr.pointee = composingText.convertTargetCursorPosition
        serverLog("INFO", "MoveCursor: clear snapshots")
        return _strdup(composingText.convertTarget)!
    }

    if offset == 126 {
        composingTextSnapshots.append(composingText)
        cursorPtr.pointee = composingText.convertTargetCursorPosition
        serverLog("INFO", "MoveCursor: push snapshot count=\(composingTextSnapshots.count)")
        return _strdup(composingText.convertTarget)!
    }

    if offset == 127 {
        if let restored = composingTextSnapshots.popLast() {
            composingText = restored
        }
        cursorPtr.pointee = composingText.convertTargetCursorPosition
        serverLog("INFO", "MoveCursor: pop snapshot remaining=\(composingTextSnapshots.count)")
        return _strdup(composingText.convertTarget)!
    }

    let cursor = composingText.moveCursorFromCursorPosition(count: Int(offset))
    serverLog("DEBUG", "MoveCursor: offset=\(offset) cursor=\(cursor)")

    cursorPtr.pointee = cursor
    serverLog("INFO", "MoveCursor: completed cursor=\(cursor)")
    return _strdup(composingText.convertTarget)!
}

@_silgen_name("ClearText")
@MainActor public func clear_text() {
    serverLog("INFO", "ClearText: start")
    composingText = ComposingText()
    composingTextSnapshots.removeAll()
    serverLog("INFO", "ClearText: completed")
}

func to_list_pointer(_ list: [FFICandidate]) -> UnsafeMutablePointer<UnsafeMutablePointer<FFICandidate>?> {
    let pointer = UnsafeMutablePointer<UnsafeMutablePointer<FFICandidate>?>.allocate(capacity: list.count)
    for (i, item) in list.enumerated() {
        pointer[i] = UnsafeMutablePointer<FFICandidate>.allocate(capacity: 1)
        pointer[i]?.pointee = item
    }
    return pointer
}

@_silgen_name("GetComposedText")
@MainActor public func get_composed_text(lengthPtr: UnsafeMutablePointer<Int>) -> UnsafeMutablePointer<UnsafeMutablePointer<FFICandidate>?> {
    let hiragana = composingText.convertTarget
    let contextString = (config["context"] as? String) ?? ""
    let runtimeZenzaiEnabled = effectiveZenzaiRuntimeEnabled(
        isConfigured: (config["enable"] as? Bool) ?? false,
        backend: config["backend"] as? String,
        cpuBackendSupported: cpuZenzaiBackendSupportedFromEnvironment()
    )
    serverLog(
        "INFO",
        "GetComposedText: start hiraganaLength=\(hiragana.count) inputCount=\(composingText.input.count) contextLength=\(contextString.count) runtimeZenzaiEnabled=\(runtimeZenzaiEnabled)"
    )
    let useZenzai = effectiveZenzaiEnabledForCandidates(
        isConfigured: runtimeZenzaiEnabled,
        inputCount: composingText.input.count,
        hiraganaCount: hiragana.count
    )
    let options = getOptions(context: contextString, zenzaiEnabled: useZenzai)
    serverLog("INFO", "GetComposedText: requestCandidates begin useZenzai=\(useZenzai)")
    let converted = converter.requestCandidates(composingText, options: options)
    serverLog("INFO", "GetComposedText: requestCandidates returned candidateCount=\(converted.mainResults.count)")
    var result: [FFICandidate] = []

    for i in 0..<converted.mainResults.count {
        let candidate = converted.mainResults[i]
        serverLog("DEBUG", "GetComposedText: candidate[\(i + 1)/\(converted.mainResults.count)] start")

        let text = strdup(constructCandidateString(candidate: candidate, hiragana: hiragana))
        serverLog("DEBUG", "GetComposedText: candidate[\(i + 1)] textReady")
        let hiragana = strdup(hiragana)
        let correspondingCount = resolveCorrespondingCount(
            composingText: composingText,
            candidateComposingCount: candidate.composingCount,
            isZenzaiEnabled: useZenzai
        )
        debugLogResolvedCorrespondingCount(
            scope: "GetComposedText",
            candidateIndex: i,
            candidateTotal: converted.mainResults.count,
            candidateComposingCount: candidate.composingCount,
            resolvedCorrespondingCount: correspondingCount,
            inputCount: composingText.input.count,
            isZenzaiEnabled: useZenzai
        )
        let subtext = strdup(
            resolveSubtext(
                composingText: composingText,
                correspondingCount: correspondingCount,
                isZenzaiEnabled: useZenzai
            )
        )
        serverLog("DEBUG", "GetComposedText: candidate[\(i + 1)] subtextReady")

        result.append(FFICandidate(text: text, subtext: subtext, hiragana: hiragana, correspondingCount: Int32(correspondingCount)))
    }

    lengthPtr.pointee = result.count
    serverLog("INFO", "GetComposedText: completed candidateCount=\(result.count) useZenzai=\(useZenzai)")

    return to_list_pointer(result)
}

@_silgen_name("GetComposedTextForCursorPrefix")
@MainActor public func get_composed_text_for_cursor_prefix(lengthPtr: UnsafeMutablePointer<Int>) -> UnsafeMutablePointer<UnsafeMutablePointer<FFICandidate>?> {
    let hiragana = composingText.convertTarget
    let suffixAfterCursor = String(hiragana.dropFirst(composingText.convertTargetCursorPosition))
    let prefixComposingText = composingText.prefixToCursorPosition()
    let prefixHiragana = prefixComposingText.convertTarget
    let contextString = (config["context"] as? String) ?? ""
    let runtimeZenzaiEnabled = effectiveZenzaiRuntimeEnabled(
        isConfigured: (config["enable"] as? Bool) ?? false,
        backend: config["backend"] as? String,
        cpuBackendSupported: cpuZenzaiBackendSupportedFromEnvironment()
    )
    serverLog(
        "INFO",
        "GetComposedTextForCursorPrefix: start prefixLength=\(prefixHiragana.count) suffixLength=\(suffixAfterCursor.count) inputCount=\(prefixComposingText.input.count) contextLength=\(contextString.count) runtimeZenzaiEnabled=\(runtimeZenzaiEnabled)"
    )
    let useZenzai = effectiveZenzaiEnabledForCandidates(
        isConfigured: runtimeZenzaiEnabled,
        inputCount: prefixComposingText.input.count,
        hiraganaCount: prefixHiragana.count
    )
    let options = getOptions(context: contextString, zenzaiEnabled: useZenzai)
    serverLog("INFO", "GetComposedTextForCursorPrefix: requestCandidates begin useZenzai=\(useZenzai)")
    let converted = converter.requestCandidates(prefixComposingText, options: options)
    serverLog("INFO", "GetComposedTextForCursorPrefix: requestCandidates returned candidateCount=\(converted.mainResults.count)")
    var result: [FFICandidate] = []

    for i in 0..<converted.mainResults.count {
        let candidate = converted.mainResults[i]
        serverLog("DEBUG", "GetComposedTextForCursorPrefix: candidate[\(i + 1)/\(converted.mainResults.count)] start")

        let text = strdup(constructCandidateString(candidate: candidate, hiragana: prefixHiragana))
        serverLog("DEBUG", "GetComposedTextForCursorPrefix: candidate[\(i + 1)] textReady")
        let hiragana = strdup(hiragana)
        let correspondingCount = resolveCorrespondingCount(
            composingText: prefixComposingText,
            candidateComposingCount: candidate.composingCount,
            isZenzaiEnabled: useZenzai
        )
        debugLogResolvedCorrespondingCount(
            scope: "GetComposedTextForCursorPrefix",
            candidateIndex: i,
            candidateTotal: converted.mainResults.count,
            candidateComposingCount: candidate.composingCount,
            resolvedCorrespondingCount: correspondingCount,
            inputCount: prefixComposingText.input.count,
            isZenzaiEnabled: useZenzai
        )
        let subtext = strdup(
            resolveCursorPrefixSubtext(
                prefixComposingText: prefixComposingText,
                suffixAfterCursor: suffixAfterCursor,
                correspondingCount: correspondingCount,
                isZenzaiEnabled: useZenzai
            )
        )
        serverLog("DEBUG", "GetComposedTextForCursorPrefix: candidate[\(i + 1)] subtextReady")

        result.append(FFICandidate(text: text, subtext: subtext, hiragana: hiragana, correspondingCount: Int32(correspondingCount)))
    }

    lengthPtr.pointee = result.count
    serverLog("INFO", "GetComposedTextForCursorPrefix: completed candidateCount=\(result.count) useZenzai=\(useZenzai)")

    return to_list_pointer(result)
}

@_silgen_name("ShrinkText")
@MainActor public func shrink_text(
    offset: Int32
) -> UnsafeMutablePointer<CChar>  {
    serverLog("INFO", "ShrinkText: start offset=\(offset)")
    var afterComposingText = composingText
    afterComposingText.prefixComplete(composingCount: .inputCount(Int(offset)))
    composingText = afterComposingText

    serverLog("INFO", "ShrinkText: completed hiraganaLength=\(composingText.convertTarget.count) inputCount=\(composingText.input.count)")
    return _strdup(composingText.convertTarget)!
}

@_silgen_name("SetContext")
@MainActor public func set_context(
    context: UnsafePointer<CChar>
) {
    let contextString = String(cString: context)
    config["context"] = contextString
    serverLog("INFO", "SetContext: contextLength=\(contextString.count)")
}
