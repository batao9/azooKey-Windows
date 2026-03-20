import KanaKanjiConverterModule
import Foundation
import ffi

@MainActor let converter = KanaKanjiConverter()
@MainActor var composingText = ComposingText()
@MainActor var composingTextSnapshots: [ComposingText] = []
@MainActor var currentInputStyle: InputStyle = .roman2kana
@MainActor var customRomajiTableURL: URL?
@MainActor private var currentComposedClauses: [ClausePayload] = []

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

private struct ClausePayload {
    let text: String
    let rawHiragana: String
    let correspondingCount: Int
}

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
    rows: [RomajiTableRow]?
) -> RomajiInputStyleSelection {
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
    rows: [RomajiTableRow]?
) {
    switch resolveRomajiInputStyleSelection(
        rows: rows
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

@MainActor func resolveCandidateComposition(
    composingText: ComposingText,
    candidateComposingCount: ComposingCount
) -> (correspondingCount: Int, remainingConvertTarget: String) {
    var remainingComposingText = composingText
    remainingComposingText.prefixComplete(composingCount: candidateComposingCount)

    return (
        correspondingCount: clampedCorrespondingCount(
            composingText: composingText,
            rawCount: composingText.input.count - remainingComposingText.input.count
        ),
        remainingConvertTarget: remainingComposingText.convertTarget
    )
}

@MainActor func makeCandidatePreviewComposingText(
    from composingText: ComposingText
) -> (composingText: ComposingText, syntheticEndOfText: Bool) {
    guard composingText.convertTarget.last == "n" else {
        return (composingText: composingText, syntheticEndOfText: false)
    }

    guard let trailingElement = composingText.input.last else {
        return (composingText: composingText, syntheticEndOfText: false)
    }

    switch trailingElement.piece {
    case .character:
        guard trailingElement.inputStyle != .direct else {
            return (composingText: composingText, syntheticEndOfText: false)
        }
    case .endOfText:
        return (composingText: composingText, syntheticEndOfText: false)
    }

    var previewComposingText = composingText
    let originalConvertTarget = previewComposingText.convertTarget
    previewComposingText.insertAtCursorPosition([
        .init(piece: .endOfText, inputStyle: trailingElement.inputStyle)
    ])

    guard previewComposingText.convertTarget != originalConvertTarget else {
        return (composingText: composingText, syntheticEndOfText: false)
    }

    return (composingText: previewComposingText, syntheticEndOfText: true)
}

@MainActor func makeCandidatePreviewComposingTextForCursorPrefix(
    prefixComposingText: ComposingText,
    suffixAfterCursor: String
) -> (composingText: ComposingText, syntheticEndOfText: Bool) {
    guard suffixAfterCursor.isEmpty else {
        return (composingText: prefixComposingText, syntheticEndOfText: false)
    }

    return makeCandidatePreviewComposingText(from: prefixComposingText)
}

@MainActor func resolveCandidateCompositionForDisplay(
    originalComposingText: ComposingText,
    previewComposingText: ComposingText,
    candidateComposingCount: ComposingCount
) -> (correspondingCount: Int, remainingConvertTarget: String) {
    let originalResolution = resolveCandidateComposition(
        composingText: originalComposingText,
        candidateComposingCount: candidateComposingCount
    )
    let previewResolution = resolveCandidateComposition(
        composingText: previewComposingText,
        candidateComposingCount: candidateComposingCount
    )

    return (
        correspondingCount: originalResolution.correspondingCount,
        remainingConvertTarget: previewResolution.remainingConvertTarget
    )
}

private func isClauseBoundary(_ former: Int, _ latter: Int) -> Bool {
    let latterWordType = DicdataStore.wordTypes[latter]
    if latterWordType == 3 {
        return false
    }

    let formerWordType = DicdataStore.wordTypes[former]
    if formerWordType == 3 {
        return false
    }

    if latterWordType == 0 || latterWordType == 1 {
        return formerWordType != 0
    }

    return false
}

@MainActor private func buildClausePayloads(
    candidate: Candidate,
    originalComposingText: ComposingText
) -> [ClausePayload] {
    struct ClauseComponent {
        var text: String
        var ruby: String
    }

    var components: [ClauseComponent] = []
    var currentComponent = ClauseComponent(text: "", ruby: "")
    var previousRcid: Int?

    for data in candidate.data where !data.word.isEmpty {
        if let previousRcid,
           isClauseBoundary(previousRcid, data.lcid),
           !currentComponent.text.isEmpty || !currentComponent.ruby.isEmpty
        {
            components.append(currentComponent)
            currentComponent = ClauseComponent(text: "", ruby: "")
        }

        currentComponent.text.append(data.word)
        currentComponent.ruby.append(data.ruby)
        previousRcid = data.rcid
    }

    if !currentComponent.text.isEmpty || !currentComponent.ruby.isEmpty {
        components.append(currentComponent)
    }

    guard !components.isEmpty else {
        guard !candidate.text.isEmpty else {
            return []
        }

        return [
            ClausePayload(
                text: candidate.text,
                rawHiragana: originalComposingText.convertTarget,
                correspondingCount: originalComposingText.input.count
            )
        ]
    }

    var result: [ClausePayload] = []
    var previousCorrespondingCount = 0
    var previousRemainingConvertTarget = originalComposingText.convertTarget
    var cumulativeSurfaceCount = 0

    for component in components {
        cumulativeSurfaceCount += component.ruby.count
        let resolution = resolveCandidateComposition(
            composingText: originalComposingText,
            candidateComposingCount: .surfaceCount(cumulativeSurfaceCount)
        )
        let rawHiragana: String
        if previousRemainingConvertTarget.hasSuffix(resolution.remainingConvertTarget) {
            rawHiragana = String(
                previousRemainingConvertTarget.dropLast(resolution.remainingConvertTarget.count)
            )
        } else {
            rawHiragana = component.ruby
        }
        let correspondingCount = max(
            0,
            resolution.correspondingCount - previousCorrespondingCount
        )

        if !component.text.isEmpty && !rawHiragana.isEmpty && correspondingCount > 0 {
            result.append(
                ClausePayload(
                    text: component.text,
                    rawHiragana: rawHiragana,
                    correspondingCount: correspondingCount
                )
            )
        }

        previousCorrespondingCount = resolution.correspondingCount
        previousRemainingConvertTarget = resolution.remainingConvertTarget
    }

    return result
}

@MainActor func debugClausePayloads(
    candidate: Candidate,
    originalComposingText: ComposingText
) -> [(text: String, rawHiragana: String, correspondingCount: Int)] {
    buildClausePayloads(
        candidate: candidate,
        originalComposingText: originalComposingText
    )
    .map {
        (
            text: $0.text,
            rawHiragana: $0.rawHiragana,
            correspondingCount: $0.correspondingCount
        )
    }
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

@MainActor private func currentRuntimeZenzaiEnabled() -> Bool {
    effectiveZenzaiRuntimeEnabled(
        isConfigured: (config["enable"] as? Bool) ?? false,
        backend: config["backend"] as? String,
        cpuBackendSupported: cpuZenzaiBackendSupportedFromEnvironment()
    )
}

@MainActor private func makeWarmupComposingText() -> ComposingText {
    var warmupComposingText = ComposingText()
    warmupComposingText.insertAtCursorPosition("a", inputStyle: currentInputStyle)
    return warmupComposingText
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

            applyRomajiInputStyle(rows: settings.romaji_table?.rows)

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

    composingText = makeWarmupComposingText()
    let useZenzaiForWarmup = effectiveZenzaiEnabledForCandidates(
        isConfigured: currentRuntimeZenzaiEnabled(),
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

@_silgen_name("Warmup")
@MainActor public func warmup() {
    let contextString = (config["context"] as? String) ?? ""
    let warmupComposingText = makeWarmupComposingText()
    let useZenzai = effectiveZenzaiEnabledForCandidates(
        isConfigured: currentRuntimeZenzaiEnabled(),
        inputCount: warmupComposingText.input.count,
        hiraganaCount: warmupComposingText.convertTarget.count
    )
    serverLog(
        "DEBUG",
        "Warmup: start hiraganaLength=\(warmupComposingText.convertTarget.count) inputCount=\(warmupComposingText.input.count) contextLength=\(contextString.count) useZenzai=\(useZenzai)"
    )
    _ = converter.requestCandidates(
        warmupComposingText,
        options: getOptions(context: contextString, zenzaiEnabled: useZenzai)
    )
    serverLog("DEBUG", "Warmup: completed")
}

@_silgen_name("HasActiveComposition")
@MainActor public func has_active_composition() -> Bool {
    !composingText.input.isEmpty
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
    currentComposedClauses.removeAll()
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

private func to_clause_list_pointer(
    _ list: [ClausePayload]
) -> UnsafeMutablePointer<UnsafeMutablePointer<FFIClause>?>? {
    guard !list.isEmpty else {
        return nil
    }

    let pointer = UnsafeMutablePointer<UnsafeMutablePointer<FFIClause>?>.allocate(
        capacity: list.count
    )
    for (i, item) in list.enumerated() {
        pointer[i] = UnsafeMutablePointer<FFIClause>.allocate(capacity: 1)
        pointer[i]?.pointee = FFIClause(
            text: strdup(item.text),
            rawHiragana: strdup(item.rawHiragana),
            correspondingCount: Int32(item.correspondingCount)
        )
    }
    return pointer
}

@_silgen_name("GetComposedText")
@MainActor public func get_composed_text(lengthPtr: UnsafeMutablePointer<Int>) -> UnsafeMutablePointer<UnsafeMutablePointer<FFICandidate>?> {
    let originalHiragana = composingText.convertTarget
    let contextString = (config["context"] as? String) ?? ""
    let runtimeZenzaiEnabled = currentRuntimeZenzaiEnabled()
    let previewState = makeCandidatePreviewComposingText(from: composingText)
    let previewComposingText = previewState.composingText
    let previewHiragana = previewComposingText.convertTarget
    serverLog(
        "INFO",
        "GetComposedText: start hiraganaLength=\(originalHiragana.count) previewHiraganaLength=\(previewHiragana.count) inputCount=\(composingText.input.count) contextLength=\(contextString.count) runtimeZenzaiEnabled=\(runtimeZenzaiEnabled) syntheticEndOfText=\(previewState.syntheticEndOfText)"
    )
    let useZenzai = effectiveZenzaiEnabledForCandidates(
        isConfigured: runtimeZenzaiEnabled,
        inputCount: composingText.input.count,
        hiraganaCount: originalHiragana.count
    )
    let options = getOptions(context: contextString, zenzaiEnabled: useZenzai)
    serverLog("INFO", "GetComposedText: requestCandidates begin useZenzai=\(useZenzai) syntheticEndOfText=\(previewState.syntheticEndOfText)")
    let requestStart = ProcessInfo.processInfo.systemUptime
    let converted = converter.requestCandidates(previewComposingText, options: options)
    let requestMs = Int((ProcessInfo.processInfo.systemUptime - requestStart) * 1000)
    serverLog("INFO", "GetComposedText: requestCandidates returned candidateCount=\(converted.mainResults.count) elapsed_ms=\(requestMs)")
    currentComposedClauses = converted.mainResults.first.map {
        buildClausePayloads(candidate: $0, originalComposingText: composingText)
    } ?? []
    var result: [FFICandidate] = []

    for i in 0..<converted.mainResults.count {
        let candidate = converted.mainResults[i]
        serverLog("DEBUG", "GetComposedText: candidate[\(i + 1)/\(converted.mainResults.count)] start")

        let text = strdup(constructCandidateString(candidate: candidate, hiragana: previewHiragana))
        serverLog("DEBUG", "GetComposedText: candidate[\(i + 1)] textReady")
        let hiragana = strdup(previewHiragana)
        let resolvedCandidate = resolveCandidateCompositionForDisplay(
            originalComposingText: composingText,
            previewComposingText: previewComposingText,
            candidateComposingCount: candidate.composingCount
        )
        let correspondingCount = resolvedCandidate.correspondingCount
        debugLogResolvedCorrespondingCount(
            scope: "GetComposedText",
            candidateIndex: i,
            candidateTotal: converted.mainResults.count,
            candidateComposingCount: candidate.composingCount,
            resolvedCorrespondingCount: correspondingCount,
            inputCount: composingText.input.count,
            isZenzaiEnabled: useZenzai
        )
        let subtext = strdup(resolvedCandidate.remainingConvertTarget)
        serverLog("DEBUG", "GetComposedText: candidate[\(i + 1)] subtextReady")

        result.append(FFICandidate(text: text, subtext: subtext, hiragana: hiragana, correspondingCount: Int32(correspondingCount)))
    }

    lengthPtr.pointee = result.count
    serverLog("INFO", "GetComposedText: completed candidateCount=\(result.count) useZenzai=\(useZenzai)")

    return to_list_pointer(result)
}

@_silgen_name("GetCurrentClauses")
@MainActor public func get_current_clauses(lengthPtr: UnsafeMutablePointer<Int>) -> UnsafeMutablePointer<UnsafeMutablePointer<FFIClause>?>? {
    lengthPtr.pointee = currentComposedClauses.count
    return to_clause_list_pointer(currentComposedClauses)
}

@_silgen_name("GetComposedTextForCursorPrefix")
@MainActor public func get_composed_text_for_cursor_prefix(lengthPtr: UnsafeMutablePointer<Int>) -> UnsafeMutablePointer<UnsafeMutablePointer<FFICandidate>?> {
    let hiragana = composingText.convertTarget
    let suffixAfterCursor = String(hiragana.dropFirst(composingText.convertTargetCursorPosition))
    let prefixComposingText = composingText.prefixToCursorPosition()
    let previewState = makeCandidatePreviewComposingTextForCursorPrefix(
        prefixComposingText: prefixComposingText,
        suffixAfterCursor: suffixAfterCursor
    )
    let previewPrefixComposingText = previewState.composingText
    let prefixHiragana = prefixComposingText.convertTarget
    let previewPrefixHiragana = previewPrefixComposingText.convertTarget
    let contextString = (config["context"] as? String) ?? ""
    let runtimeZenzaiEnabled = currentRuntimeZenzaiEnabled()
    serverLog(
        "INFO",
        "GetComposedTextForCursorPrefix: start prefixLength=\(prefixHiragana.count) previewPrefixLength=\(previewPrefixHiragana.count) suffixLength=\(suffixAfterCursor.count) inputCount=\(prefixComposingText.input.count) contextLength=\(contextString.count) runtimeZenzaiEnabled=\(runtimeZenzaiEnabled) syntheticEndOfText=\(previewState.syntheticEndOfText)"
    )
    let useZenzai = effectiveZenzaiEnabledForCandidates(
        isConfigured: runtimeZenzaiEnabled,
        inputCount: prefixComposingText.input.count,
        hiraganaCount: prefixHiragana.count
    )
    let options = getOptions(context: contextString, zenzaiEnabled: useZenzai)
    serverLog("INFO", "GetComposedTextForCursorPrefix: requestCandidates begin useZenzai=\(useZenzai) syntheticEndOfText=\(previewState.syntheticEndOfText)")
    let requestStart = ProcessInfo.processInfo.systemUptime
    let converted = converter.requestCandidates(previewPrefixComposingText, options: options)
    let requestMs = Int((ProcessInfo.processInfo.systemUptime - requestStart) * 1000)
    serverLog("INFO", "GetComposedTextForCursorPrefix: requestCandidates returned candidateCount=\(converted.mainResults.count) elapsed_ms=\(requestMs)")
    currentComposedClauses.removeAll()
    var result: [FFICandidate] = []

    for i in 0..<converted.mainResults.count {
        let candidate = converted.mainResults[i]
        serverLog("DEBUG", "GetComposedTextForCursorPrefix: candidate[\(i + 1)/\(converted.mainResults.count)] start")

        let text = strdup(constructCandidateString(candidate: candidate, hiragana: previewPrefixHiragana))
        serverLog("DEBUG", "GetComposedTextForCursorPrefix: candidate[\(i + 1)] textReady")
        let hiragana = strdup(previewPrefixHiragana + suffixAfterCursor)
        let resolvedCandidate = resolveCandidateCompositionForDisplay(
            originalComposingText: prefixComposingText,
            previewComposingText: previewPrefixComposingText,
            candidateComposingCount: candidate.composingCount
        )
        let correspondingCount = resolvedCandidate.correspondingCount
        debugLogResolvedCorrespondingCount(
            scope: "GetComposedTextForCursorPrefix",
            candidateIndex: i,
            candidateTotal: converted.mainResults.count,
            candidateComposingCount: candidate.composingCount,
            resolvedCorrespondingCount: correspondingCount,
            inputCount: prefixComposingText.input.count,
            isZenzaiEnabled: useZenzai
        )
        let subtext = strdup(resolvedCandidate.remainingConvertTarget + suffixAfterCursor)
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
