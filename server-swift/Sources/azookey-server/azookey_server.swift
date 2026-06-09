import KanaKanjiConverterModule
import Foundation
import ffi

private func executableDirectoryURL() -> URL? {
    guard let executablePath = CommandLine.arguments.first, !executablePath.isEmpty else {
        return nil
    }
    return URL(filePath: executablePath).deletingLastPathComponent()
}

private let fallbackDictionaryURL =
    executableDirectoryURL()?.appendingPathComponent("Dictionary", isDirectory: true)
    ?? URL(filePath: FileManager.default.currentDirectoryPath)

@MainActor var converterDictionaryURL = fallbackDictionaryURL
@MainActor var converterPreloadDictionary = false
@MainActor var converter = KanaKanjiConverter(
    dictionaryURL: fallbackDictionaryURL,
    preloadDictionary: false
)
@MainActor var composingText = ComposingText()
@MainActor var composingTextSnapshots: [ComposingText] = []
@MainActor var currentInputStyle: InputStyle = .roman2kana
@MainActor var customRomajiTableEnabled = false

@MainActor var execURL = URL(filePath: "")
@MainActor var config: [String : Any] = [
    "enable": false,
    "profile": "",
    "backend": "cpu",
]
let maxUserDictionaryEntryCount = 50
let minInputCountForZenzaiCandidates = 4
let minHiraganaCountForZenzaiCandidates = 2
// Request exact-clause supplements only when boundary-matched candidates are sparse.
let cursorPrefixExactClauseSupplementCandidateThreshold = 5

@MainActor var currentRequestId: UInt64 = 0

public typealias ServerLogEnabledCallback = @convention(c) () -> Bool
public typealias ServerLogLevelEnabledCallback = @convention(c) (
    UnsafePointer<CChar>?
) -> Bool
public typealias ServerLogWriteCallback = @convention(c) (
    UnsafePointer<CChar>?,
    UnsafePointer<CChar>?
) -> Void
public typealias ServerPerformanceLogWriteCallback = @convention(c) (
    UInt64,
    UnsafePointer<CChar>?,
    UnsafePointer<CChar>?,
    UInt64,
    UnsafePointer<CChar>?
) -> Void
public typealias ServerLogFlushCallback = @convention(c) () -> Void
public typealias ServerCrashTraceWriteCallback = @convention(c) (
    UnsafePointer<CChar>?,
    UnsafePointer<CChar>?,
    UnsafePointer<CChar>?,
    UnsafePointer<CChar>?
) -> Void

private final class ServerLogCallbacks: @unchecked Sendable {
    private let lock = NSLock()
    private var logEnabled: ServerLogEnabledCallback?
    private var logLevelEnabled: ServerLogLevelEnabledCallback?
    private var performanceLogEnabled: ServerLogEnabledCallback?
    private var writeLog: ServerLogWriteCallback?
    private var writePerformanceLog: ServerPerformanceLogWriteCallback?
    private var flushLog: ServerLogFlushCallback?
    private var crashTraceEnabled: ServerLogEnabledCallback?
    private var writeCrashTrace: ServerCrashTraceWriteCallback?

    func configure(
        logEnabled: ServerLogEnabledCallback?,
        logLevelEnabled: ServerLogLevelEnabledCallback?,
        performanceLogEnabled: ServerLogEnabledCallback?,
        writeLog: ServerLogWriteCallback?,
        writePerformanceLog: ServerPerformanceLogWriteCallback?,
        flushLog: ServerLogFlushCallback?,
        crashTraceEnabled: ServerLogEnabledCallback?,
        writeCrashTrace: ServerCrashTraceWriteCallback?
    ) {
        lock.lock()
        self.logEnabled = logEnabled
        self.logLevelEnabled = logLevelEnabled
        self.performanceLogEnabled = performanceLogEnabled
        self.writeLog = writeLog
        self.writePerformanceLog = writePerformanceLog
        self.flushLog = flushLog
        self.crashTraceEnabled = crashTraceEnabled
        self.writeCrashTrace = writeCrashTrace
        lock.unlock()
    }

    func isLogEnabled(level: String) -> Bool {
        lock.lock()
        let fallbackCallback = logEnabled
        let levelCallback = logLevelEnabled
        lock.unlock()
        if let levelCallback {
            return level.withCString { levelPointer in
                levelCallback(levelPointer)
            }
        }
        return fallbackCallback?() ?? false
    }

    func isPerformanceLogEnabled() -> Bool {
        lock.lock()
        let callback = performanceLogEnabled
        lock.unlock()
        return callback?() ?? false
    }

    func log(level: String, message: String) {
        lock.lock()
        let callback = writeLog
        lock.unlock()

        guard let callback else {
            return
        }

        level.withCString { levelPointer in
            message.withCString { messagePointer in
                callback(levelPointer, messagePointer)
            }
        }
    }

    func performanceLog(
        requestId: UInt64,
        operation: String,
        stage: String,
        elapsedMs: UInt64,
        details: String
    ) {
        lock.lock()
        let callback = writePerformanceLog
        lock.unlock()

        guard let callback else {
            return
        }

        operation.withCString { operationPointer in
            stage.withCString { stagePointer in
                details.withCString { detailsPointer in
                    callback(requestId, operationPointer, stagePointer, elapsedMs, detailsPointer)
                }
            }
        }
    }

    func flush() {
        lock.lock()
        let callback = flushLog
        lock.unlock()

        callback?()
    }

    func isCrashTraceEnabled() -> Bool {
        lock.lock()
        let callback = crashTraceEnabled
        lock.unlock()
        return callback?() ?? false
    }

    func crashTrace(operation: String, stage: String, state: String, details: String) {
        lock.lock()
        let callback = writeCrashTrace
        lock.unlock()

        guard let callback else {
            return
        }

        operation.withCString { operationPointer in
            stage.withCString { stagePointer in
                state.withCString { statePointer in
                    details.withCString { detailsPointer in
                        callback(operationPointer, stagePointer, statePointer, detailsPointer)
                    }
                }
            }
        }
    }
}

private let serverLogCallbacks = ServerLogCallbacks()

@_silgen_name("SetServerLogCallbacks")
public func set_server_log_callbacks(
    _ logEnabled: ServerLogEnabledCallback?,
    _ logLevelEnabled: ServerLogLevelEnabledCallback?,
    _ performanceLogEnabled: ServerLogEnabledCallback?,
    _ writeLog: ServerLogWriteCallback?,
    _ writePerformanceLog: ServerPerformanceLogWriteCallback?,
    _ flushLog: ServerLogFlushCallback?,
    _ crashTraceEnabled: ServerLogEnabledCallback?,
    _ writeCrashTrace: ServerCrashTraceWriteCallback?
) {
    serverLogCallbacks.configure(
        logEnabled: logEnabled,
        logLevelEnabled: logLevelEnabled,
        performanceLogEnabled: performanceLogEnabled,
        writeLog: writeLog,
        writePerformanceLog: writePerformanceLog,
        flushLog: flushLog,
        crashTraceEnabled: crashTraceEnabled,
        writeCrashTrace: writeCrashTrace
    )
}

@MainActor private func serverLog(
    _ level: String = "INFO",
    _ message: @autoclosure () -> String,
    flush: Bool = false
) {
    guard serverLogCallbacks.isLogEnabled(level: level) else {
        return
    }

    serverLogCallbacks.log(level: level, message: "request_id=\(currentRequestId) \(message())")
    if flush {
        serverLogCallbacks.flush()
    }
}

@MainActor private func crashTrace(
    operation: String,
    stage: String,
    state: String,
    details: @autoclosure () -> String = ""
) {
    guard serverLogCallbacks.isCrashTraceEnabled() else {
        return
    }

    serverLogCallbacks.crashTrace(
        operation: operation,
        stage: stage,
        state: state,
        details: "request_id=\(currentRequestId);\(details())"
    )
}

@MainActor private func performanceLog(
    operation: String,
    stage: String,
    elapsedMs: Int,
    details: @autoclosure () -> String = ""
) {
    guard serverLogCallbacks.isPerformanceLogEnabled() else {
        return
    }

    serverLogCallbacks.performanceLog(
        requestId: currentRequestId,
        operation: operation,
        stage: stage,
        elapsedMs: UInt64(max(0, elapsedMs)),
        details: details()
    )
}

private func settingsPath() -> URL? {
    guard let appDataPath = ProcessInfo.processInfo.environment["APPDATA"] else {
        return nil
    }
    return URL(filePath: appDataPath).appendingPathComponent("Azookey/settings.json")
}

private func readAppSettings(at path: URL) throws -> AppSettings {
    let data = try Data(contentsOf: path)
    return try JSONDecoder().decode(AppSettings.self, from: data)
}

@MainActor private func rebuildConverter() {
    converter = KanaKanjiConverter(
        dictionaryURL: converterDictionaryURL,
        preloadDictionary: converterPreloadDictionary
    )
}

@MainActor private func converterRuntimeDirectoryURL() -> URL {
    execURL.appendingPathComponent("EngineRuntime", isDirectory: true)
}

func normalizedZenzaiBackend(_ backend: String?) -> String {
    (backend ?? "cpu")
        .trimmingCharacters(in: .whitespacesAndNewlines)
        .lowercased()
}

@MainActor private func configureEngineRuntime(zenzaiEnabled: Bool) {
    let normalizedBackend = normalizedZenzaiBackend(config["backend"] as? String)
    let shouldOffloadToGpu = zenzaiEnabled && !normalizedBackend.isEmpty && normalizedBackend != "cpu"
    KanaKanjiConverterEngineRuntime.configure(
        gpuLayerCount: shouldOffloadToGpu ? Int32.max : 0
    )
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

    let normalizedBackend = normalizedZenzaiBackend(backend)

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
    customRomajiTableEnabled = false
}

@MainActor private func setCustomRomajiInputStyle(rows: [RomajiTableRow]?) {
    guard let rows, let content = buildCustomRomajiTableContent(rows: rows) else {
        setRoman2KanaInputStyle()
        return
    }

    let runtimeDirectoryURL = converterRuntimeDirectoryURL()
    let fileURL = runtimeDirectoryURL
        .appendingPathComponent("azookey-romaji-\(UUID().uuidString).tsv")

    do {
        try FileManager.default.createDirectory(
            at: runtimeDirectoryURL,
            withIntermediateDirectories: true
        )
        try content.write(to: fileURL, atomically: true, encoding: .utf8)
        defer {
            try? FileManager.default.removeItem(at: fileURL)
        }
        let tableName = "azookey-windows-custom-romaji"
        let table = try InputStyleManager.loadTable(from: fileURL)
        InputStyleManager.registerInputStyle(table: table, for: tableName)
        currentInputStyle = .mapped(id: .tableName(tableName))
        customRomajiTableEnabled = true
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

private func inputCharacter(_ element: ComposingText.InputElement) -> Character? {
    switch element.piece {
    case .character(let character):
        character
    case .key(_, let input, _):
        input
    case .compositionSeparator:
        nil
    }
}

private func asciiLowercase(_ character: Character) -> Character? {
    let scalars = String(character).unicodeScalars
    guard scalars.count == 1, let scalar = scalars.first else {
        return nil
    }

    let value = scalar.value
    if (65...90).contains(value), let lowered = UnicodeScalar(value + 32) {
        return Character(lowered)
    }
    if (97...122).contains(value) {
        return character
    }
    return nil
}

private func isAsciiRomajiVowel(_ character: Character) -> Bool {
    guard let lowered = asciiLowercase(character) else {
        return false
    }
    switch lowered {
    case "a", "i", "u", "e", "o":
        return true
    default:
        return false
    }
}

private func isAsciiRomajiConsonantExceptN(_ character: Character) -> Bool {
    guard let lowered = asciiLowercase(character) else {
        return false
    }
    return lowered != "n" && !isAsciiRomajiVowel(lowered)
}

private func adjustedCorrespondingCountForDelayedSingleN(
    composingText: ComposingText,
    rawCount: Int
) -> Int {
    let splitAt = clampedCorrespondingCount(composingText: composingText, rawCount: rawCount)
    guard splitAt >= 2, splitAt < composingText.input.count else {
        return splitAt
    }

    let previousElement = composingText.input[splitAt - 2]
    let consumedElement = composingText.input[splitAt - 1]
    let nextElement = composingText.input[splitAt]
    guard previousElement.inputStyle != .direct,
          consumedElement.inputStyle != .direct,
          nextElement.inputStyle != .direct,
          let previous = inputCharacter(previousElement),
          asciiLowercase(previous) == "n",
          let consumed = inputCharacter(consumedElement),
          isAsciiRomajiConsonantExceptN(consumed),
          let next = inputCharacter(nextElement),
          isAsciiRomajiVowel(next)
    else {
        return splitAt
    }

    return splitAt - 1
}

@MainActor func resolveCandidateComposition(
    composingText: ComposingText,
    candidateComposingCount: ComposingCount
) -> (correspondingCount: Int, remainingConvertTarget: String) {
    var remainingComposingText = composingText
    remainingComposingText.prefixComplete(composingCount: candidateComposingCount)

    let rawCount = composingText.input.count - remainingComposingText.input.count
    let correspondingCount = adjustedCorrespondingCountForDelayedSingleN(
        composingText: composingText,
        rawCount: rawCount
    )
    if correspondingCount != rawCount {
        var adjustedRemainingComposingText = composingText
        adjustedRemainingComposingText.prefixComplete(
            composingCount: .inputCount(correspondingCount)
        )
        return (
            correspondingCount: correspondingCount,
            remainingConvertTarget: adjustedRemainingComposingText.convertTarget
        )
    }

    return (
        correspondingCount: correspondingCount,
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
    case .character, .key:
        guard trailingElement.inputStyle != .direct else {
            return (composingText: composingText, syntheticEndOfText: false)
        }
    case .compositionSeparator:
        return (composingText: composingText, syntheticEndOfText: false)
    }

    var previewComposingText = composingText
    let originalConvertTarget = previewComposingText.convertTarget
    previewComposingText.insertAtCursorPosition([
        .init(piece: .compositionSeparator, inputStyle: trailingElement.inputStyle)
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
) -> CandidateDisplayResolution {
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
        remainingConvertTarget: previewResolution.remainingConvertTarget,
        remainingConvertTargetCount: previewResolution.remainingConvertTarget.count
    )
}

typealias CandidateDisplayResolution = (
    correspondingCount: Int,
    remainingConvertTarget: String,
    remainingConvertTargetCount: Int
)

struct CursorPrefixCandidateResult {
    let candidate: Candidate
    let displayText: String
}

private struct CursorPrefixBoundaryCandidate {
    let index: Int
    let correspondingCount: Int
    let score: Int
}

private struct CursorPrefixBoundaryScoringContext {
    let previewHiragana: String
    let previewHiraganaBoundaries: [String.Index]

    init(previewHiragana: String) {
        self.previewHiragana = previewHiragana

        var boundaries = [String.Index]()
        boundaries.append(previewHiragana.startIndex)

        var index = previewHiragana.startIndex
        while index < previewHiragana.endIndex {
            index = previewHiragana.index(after: index)
            boundaries.append(index)
        }
        self.previewHiraganaBoundaries = boundaries
    }

    var previewHiraganaCount: Int {
        max(0, previewHiraganaBoundaries.count - 1)
    }

    func boundaryIndex(afterCharacters count: Int) -> String.Index? {
        guard count >= 0, count < previewHiraganaBoundaries.count else {
            return nil
        }
        return previewHiraganaBoundaries[count]
    }
}

private let cursorPrefixClauseTerminalSuffixes = [
    "ではない",
    "じゃない",
    "である",
    "でした",
    "だった",
    "ました",
    "ません",
    "です",
    "ます",
    "ない",
]

private func cursorPrefixHasCandidateRubyBoundary(
    candidate: Candidate,
    prefixSurfaceCount: Int
) -> Bool {
    var cursor = 0
    for element in candidate.data {
        cursor += element.ruby.count
        if cursor == prefixSurfaceCount {
            return true
        }
        if cursor > prefixSurfaceCount {
            return false
        }
    }
    return false
}

private func cursorPrefixTerminalPhraseBonus(
    context: CursorPrefixBoundaryScoringContext,
    prefixSurfaceCount: Int
) -> Int {
    guard let prefixEndIndex = context.boundaryIndex(afterCharacters: prefixSurfaceCount) else {
        return 0
    }

    for suffix in cursorPrefixClauseTerminalSuffixes {
        let suffixCount = suffix.count
        guard prefixSurfaceCount >= suffixCount else {
            continue
        }

        let suffixStartIndex = context.previewHiragana.index(
            prefixEndIndex,
            offsetBy: -suffixCount
        )
        if context.previewHiragana[suffixStartIndex..<prefixEndIndex].elementsEqual(suffix) {
            return 120
        }
    }
    return 0
}

private func cursorPrefixTokenBoundaryPenalty(
    candidate: Candidate,
    prefixSurfaceCount: Int
) -> Int {
    guard prefixSurfaceCount > 0,
          prefixSurfaceCount < candidate.rubyCount
    else {
        return 0
    }

    return cursorPrefixHasCandidateRubyBoundary(
        candidate: candidate,
        prefixSurfaceCount: prefixSurfaceCount
    ) ? 0 : 160
}

private func cursorPrefixBoundaryScore(
    candidate: Candidate,
    candidateIndex: Int,
    resolution: CandidateDisplayResolution,
    context: CursorPrefixBoundaryScoringContext
) -> Int {
    let remainingCount = resolution.remainingConvertTargetCount
    let prefixSurfaceCount = max(0, context.previewHiraganaCount - remainingCount)
    let terminalBonus = cursorPrefixTerminalPhraseBonus(
        context: context,
        prefixSurfaceCount: prefixSurfaceCount
    )
    let tokenBoundaryPenalty = cursorPrefixTokenBoundaryPenalty(
        candidate: candidate,
        prefixSurfaceCount: prefixSurfaceCount
    )

    return resolution.correspondingCount * 4
        + terminalBonus
        - tokenBoundaryPenalty
        - candidateIndex
}

private func preferCursorPrefixBoundary(
    _ candidate: CursorPrefixBoundaryCandidate,
    over current: CursorPrefixBoundaryCandidate?
) -> Bool {
    guard let current else {
        return true
    }
    if candidate.score != current.score {
        return candidate.score > current.score
    }
    if candidate.correspondingCount != current.correspondingCount {
        return candidate.correspondingCount > current.correspondingCount
    }
    return candidate.index < current.index
}

@MainActor func resolveCandidateCompositionForDisplay(
    originalComposingText: ComposingText,
    previewComposingText: ComposingText,
    candidateComposingCount: ComposingCount,
    resolutionCache: inout [String: CandidateDisplayResolution]
) -> CandidateDisplayResolution {
    let cacheKey = String(describing: candidateComposingCount)
    if let cached = resolutionCache[cacheKey] {
        return cached
    }

    let resolved = resolveCandidateCompositionForDisplay(
        originalComposingText: originalComposingText,
        previewComposingText: previewComposingText,
        candidateComposingCount: candidateComposingCount
    )
    resolutionCache[cacheKey] = resolved
    return resolved
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

@MainActor func cursorPrefixCandidateResults(
    mainResults: [Candidate],
    firstClauseResults: [Candidate],
    exactClauseResults: [Candidate] = [],
    originalComposingText: ComposingText,
    previewComposingText: ComposingText,
    previewHiragana: String
) -> [Candidate] {
    cursorPrefixCandidateDisplayResults(
        mainResults: mainResults,
        firstClauseResults: firstClauseResults,
        exactClauseResults: exactClauseResults,
        originalComposingText: originalComposingText,
        previewComposingText: previewComposingText,
        previewHiragana: previewHiragana
    ).map(\.candidate)
}

@MainActor func cursorPrefixCandidateDisplayResults(
    mainResults: [Candidate],
    firstClauseResults: [Candidate],
    exactClauseResults: [Candidate] = [],
    originalComposingText: ComposingText,
    previewComposingText: ComposingText,
    previewHiragana: String
) -> [CursorPrefixCandidateResult] {
    var resolutionCache: [String: CandidateDisplayResolution] = [:]
    let firstClauseCorrespondingCount = cursorPrefixFirstClauseCorrespondingCount(
        firstClauseResults: firstClauseResults,
        originalComposingText: originalComposingText,
        previewComposingText: previewComposingText,
        resolutionCache: &resolutionCache
    )
    return cursorPrefixCandidateDisplayResults(
        mainResults: mainResults,
        firstClauseResults: firstClauseResults,
        exactClauseResults: exactClauseResults,
        firstClauseCorrespondingCount: firstClauseCorrespondingCount,
        originalComposingText: originalComposingText,
        previewComposingText: previewComposingText,
        previewHiragana: previewHiragana,
        resolutionCache: &resolutionCache
    )
}

@MainActor func cursorPrefixCandidateDisplayResults(
    mainResults: [Candidate],
    firstClauseResults: [Candidate],
    exactClauseResults: [Candidate] = [],
    firstClauseCorrespondingCount: Int?,
    originalComposingText: ComposingText,
    previewComposingText: ComposingText,
    previewHiragana: String,
    resolutionCache: inout [String: CandidateDisplayResolution]
) -> [CursorPrefixCandidateResult] {
    guard let firstClauseCorrespondingCount else {
        return mainResults.map {
            CursorPrefixCandidateResult(
                candidate: $0,
                displayText: constructCandidateString(candidate: $0, hiragana: previewHiragana)
            )
        }
    }

    var seenTexts = Set<String>()
    var results: [CursorPrefixCandidateResult] = []

    func appendIfNeeded(_ candidate: Candidate) {
        let text = constructCandidateString(candidate: candidate, hiragana: previewHiragana)
        guard seenTexts.insert(text).inserted else {
            return
        }
        results.append(CursorPrefixCandidateResult(candidate: candidate, displayText: text))
    }

    func matchesFirstClauseBoundary(_ candidate: Candidate) -> Bool {
        let correspondingCount = resolveCandidateCompositionForDisplay(
            originalComposingText: originalComposingText,
            previewComposingText: previewComposingText,
            candidateComposingCount: candidate.composingCount,
            resolutionCache: &resolutionCache
        ).correspondingCount
        return correspondingCount == firstClauseCorrespondingCount
    }

    for candidate in firstClauseResults {
        guard matchesFirstClauseBoundary(candidate) else {
            continue
        }
        appendIfNeeded(candidate)
    }

    for candidate in mainResults {
        guard matchesFirstClauseBoundary(candidate) else {
            continue
        }
        appendIfNeeded(candidate)
    }

    for candidate in exactClauseResults {
        guard matchesFirstClauseBoundary(candidate) else {
            continue
        }
        appendIfNeeded(candidate)
    }

    return results
}

@MainActor func cursorPrefixFirstClauseCorrespondingCount(
    firstClauseResults: [Candidate],
    originalComposingText: ComposingText,
    previewComposingText: ComposingText
) -> Int? {
    var resolutionCache: [String: CandidateDisplayResolution] = [:]
    return cursorPrefixFirstClauseCorrespondingCount(
        firstClauseResults: firstClauseResults,
        originalComposingText: originalComposingText,
        previewComposingText: previewComposingText,
        resolutionCache: &resolutionCache
    )
}

@MainActor func cursorPrefixFirstClauseCorrespondingCount(
    firstClauseResults: [Candidate],
    originalComposingText: ComposingText,
    previewComposingText: ComposingText,
    resolutionCache: inout [String: CandidateDisplayResolution]
) -> Int? {
    let inputCount = originalComposingText.input.count
    let scoringContext = CursorPrefixBoundaryScoringContext(
        previewHiragana: previewComposingText.convertTarget
    )
    var splitBoundary: CursorPrefixBoundaryCandidate?
    var fallbackBoundary: CursorPrefixBoundaryCandidate?

    for (index, candidate) in firstClauseResults.enumerated() {
        let resolution = resolveCandidateCompositionForDisplay(
            originalComposingText: originalComposingText,
            previewComposingText: previewComposingText,
            candidateComposingCount: candidate.composingCount,
            resolutionCache: &resolutionCache
        )
        guard resolution.correspondingCount > 0 else {
            continue
        }

        let boundary = CursorPrefixBoundaryCandidate(
            index: index,
            correspondingCount: resolution.correspondingCount,
            score: cursorPrefixBoundaryScore(
                candidate: candidate,
                candidateIndex: index,
                resolution: resolution,
                context: scoringContext
            )
        )

        if resolution.correspondingCount < inputCount,
           preferCursorPrefixBoundary(boundary, over: splitBoundary)
        {
            splitBoundary = boundary
        }
        if preferCursorPrefixBoundary(boundary, over: fallbackBoundary) {
            fallbackBoundary = boundary
        }
    }

    return splitBoundary?.correspondingCount ?? fallbackBoundary?.correspondingCount
}

@MainActor func makeCursorPrefixExactClauseComposingText(
    prefixComposingText: ComposingText,
    correspondingCount: Int
) -> ComposingText {
    var clauseComposingText = ComposingText()
    let count = clampedCorrespondingCount(
        composingText: prefixComposingText,
        rawCount: correspondingCount
    )
    clauseComposingText.insertAtCursorPosition(
        Array(prefixComposingText.input.prefix(count))
    )
    return clauseComposingText
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
    configureEngineRuntime(zenzaiEnabled: zenzaiEnabled)
    let profile = (config["profile"] as? String) ?? ""
    return ConvertRequestOptions(
        requireJapanesePrediction: .disabled,
        requireEnglishPrediction: .disabled,
        keyboardLanguage: .ja_JP,
        learningType: .nothing,
        memoryDirectoryURL: converterRuntimeDirectoryURL(),
        sharedContainerURL: converterRuntimeDirectoryURL(),
        textReplacer: .init {
            return execURL.appendingPathComponent("EmojiDictionary").appendingPathComponent("emoji_all_E15.1.txt")
        },
        specialCandidateProviders: nil,
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

private struct ZenzaiDiagnosticSnapshot {
    let configuredEnabled: Bool
    let backend: String
    let normalizedBackend: String
    let profileLength: Int
    let cpuBackendSupported: Bool
    let runtimeEnabled: Bool
}

@MainActor private func zenzaiDiagnosticSnapshot() -> ZenzaiDiagnosticSnapshot {
    let configuredEnabled = (config["enable"] as? Bool) ?? false
    let backend = (config["backend"] as? String) ?? "cpu"
    let profile = (config["profile"] as? String) ?? ""
    let cpuBackendSupported = cpuZenzaiBackendSupportedFromEnvironment()
    return ZenzaiDiagnosticSnapshot(
        configuredEnabled: configuredEnabled,
        backend: backend,
        normalizedBackend: normalizedZenzaiBackend(backend),
        profileLength: profile.count,
        cpuBackendSupported: cpuBackendSupported,
        runtimeEnabled: effectiveZenzaiRuntimeEnabled(
            isConfigured: configuredEnabled,
            backend: backend,
            cpuBackendSupported: cpuBackendSupported
        )
    )
}

private func sanitizeDiagnosticField(_ value: String, maxLength: Int = 80) -> String {
    let text = String(value.map { character -> Character in
        switch character {
        case "\t", "\r", "\n", ";":
            return " "
        default:
            return character
        }
    })
    if text.count <= maxLength {
        return text
    }
    return String(text.prefix(maxLength))
}

@MainActor private func zenzaiDiagnosticDetails(
    snapshot: ZenzaiDiagnosticSnapshot,
    contextLength: Int,
    inputCount: Int,
    hiraganaLength: Int,
    previewHiraganaLength: Int? = nil,
    useZenzai: Bool,
    syntheticEndOfText: Bool? = nil
) -> String {
    var fields = [
        "configured_zenzai=\(snapshot.configuredEnabled)",
        "runtime_zenzai=\(snapshot.runtimeEnabled)",
        "use_zenzai=\(useZenzai)",
        "backend=\(sanitizeDiagnosticField(snapshot.normalizedBackend))",
        "backend_raw=\(sanitizeDiagnosticField(snapshot.backend))",
        "cpu_backend_supported=\(snapshot.cpuBackendSupported)",
        "profile_len=\(snapshot.profileLength)",
        "context_len=\(contextLength)",
        "input_count=\(inputCount)",
        "hiragana_len=\(hiraganaLength)",
    ]
    if let previewHiraganaLength {
        fields.append("preview_hiragana_len=\(previewHiraganaLength)")
    }
    if let syntheticEndOfText {
        fields.append("synthetic_end_of_text=\(syntheticEndOfText)")
    }
    return fields.joined(separator: ";")
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
    let loadedSettingsPath = settingsPath()
    var loadedSettings: AppSettings?
    var settingsLoadError: Error?
    if let loadedSettingsPath {
        do {
            let settings = try readAppSettings(at: loadedSettingsPath)
            loadedSettings = settings
        } catch {
            settingsLoadError = error
        }
    }

    serverLog("INFO", "LoadConfig: start")
    let previousZenzaiEnabled = (config["enable"] as? Bool) ?? false
    let previousProfile = (config["profile"] as? String) ?? ""
    let previousBackend = (config["backend"] as? String) ?? "cpu"
    let previousEffectiveZenzaiEnabled = effectiveZenzaiRuntimeEnabled(
        isConfigured: previousZenzaiEnabled,
        backend: previousBackend,
        cpuBackendSupported: cpuZenzaiBackendSupportedFromEnvironment()
    )
    let previousUsedCustomRomajiTable = customRomajiTableEnabled
    var dynamicUserDictionary: [DicdataElement] = []
    defer {
        converter.importDynamicUserDictionary(dynamicUserDictionary)
    }

    config["enable"] = false
    config["profile"] = ""
    config["backend"] = "cpu"
    setRoman2KanaInputStyle()

    if let settings = loadedSettings {
        if let loadedSettingsPath {
            serverLog("INFO", "LoadConfig: reading settingsPath=\(loadedSettingsPath.path)")
        }

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
    } else if let settingsLoadError {
        serverLog("ERROR", "Failed to read settings: \(settingsLoadError)")
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
    let currentUsedCustomRomajiTable = customRomajiTableEnabled
    let backendChanged = normalizedZenzaiBackend(previousBackend) != normalizedZenzaiBackend(currentBackend)
    if previousEffectiveZenzaiEnabled != currentEffectiveZenzaiEnabled
        || previousProfile != currentProfile
        || backendChanged
        || previousUsedCustomRomajiTable != currentUsedCustomRomajiTable
    {
        if backendChanged {
            rebuildConverter()
        } else {
            converter.stopComposition()
        }
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
    converterDictionaryURL = execURL.appendingPathComponent("Dictionary")
    converterPreloadDictionary = true
    rebuildConverter()

    load_config()

    composingText = makeWarmupComposingText()
    let useZenzaiForWarmup = effectiveZenzaiEnabledForCandidates(
        isConfigured: currentRuntimeZenzaiEnabled(),
        inputCount: composingText.input.count,
        hiraganaCount: composingText.convertTarget.count
    )
    let diagnosticSnapshot = zenzaiDiagnosticSnapshot()
    let diagnosticDetails = zenzaiDiagnosticDetails(
        snapshot: diagnosticSnapshot,
        contextLength: 0,
        inputCount: composingText.input.count,
        hiraganaLength: composingText.convertTarget.count,
        useZenzai: useZenzaiForWarmup
    )
    let options = getOptions(zenzaiEnabled: useZenzaiForWarmup)
    crashTrace(operation: "Initialize", stage: "requestCandidates", state: "begin", details: diagnosticDetails)
    serverLog("DEBUG", "Initialize: requestCandidates begin \(diagnosticDetails)", flush: true)
    let converted = converter.requestCandidates(
        composingText,
        options: options
    )
    crashTrace(
        operation: "Initialize",
        stage: "requestCandidates",
        state: "completed",
        details: "candidate_count=\(converted.mainResults.count);\(diagnosticDetails)"
    )
    serverLog("DEBUG", "Initialize: requestCandidates returned candidateCount=\(converted.mainResults.count) \(diagnosticDetails)")
    composingText = ComposingText()
    composingTextSnapshots.removeAll()
    serverLog(
        "INFO",
        "Initialize: completed inputStyle=\(String(describing: currentInputStyle)) warmupUseZenzai=\(useZenzaiForWarmup) \(diagnosticDetails)"
    )
}

@_silgen_name("SetRequestId")
@MainActor public func set_request_id(_ requestID: UInt64) {
    currentRequestId = requestID
}

@_silgen_name("Warmup")
@MainActor public func warmup() {
    let contextString = (config["context"] as? String) ?? ""
    let warmupComposingText = makeWarmupComposingText()
    let diagnosticSnapshot = zenzaiDiagnosticSnapshot()
    let useZenzai = effectiveZenzaiEnabledForCandidates(
        isConfigured: diagnosticSnapshot.runtimeEnabled,
        inputCount: warmupComposingText.input.count,
        hiraganaCount: warmupComposingText.convertTarget.count
    )
    let diagnosticDetails = zenzaiDiagnosticDetails(
        snapshot: diagnosticSnapshot,
        contextLength: contextString.count,
        inputCount: warmupComposingText.input.count,
        hiraganaLength: warmupComposingText.convertTarget.count,
        useZenzai: useZenzai
    )
    serverLog(
        "DEBUG",
        "Warmup: start \(diagnosticDetails)"
    )
    let options = getOptions(context: contextString, zenzaiEnabled: useZenzai)
    crashTrace(operation: "Warmup", stage: "requestCandidates", state: "begin", details: diagnosticDetails)
    serverLog("DEBUG", "Warmup: requestCandidates begin \(diagnosticDetails)", flush: true)
    let requestStart = ProcessInfo.processInfo.systemUptime
    let converted = converter.requestCandidates(
        warmupComposingText,
        options: options
    )
    let requestMs = Int((ProcessInfo.processInfo.systemUptime - requestStart) * 1000)
    performanceLog(
        operation: "warmup",
        stage: "request_candidates",
        elapsedMs: requestMs,
        details: "candidate_count=\(converted.mainResults.count);\(diagnosticDetails)"
    )
    crashTrace(
        operation: "Warmup",
        stage: "requestCandidates",
        state: "completed",
        details: "candidate_count=\(converted.mainResults.count);\(diagnosticDetails)"
    )
    serverLog("DEBUG", "Warmup: requestCandidates returned candidateCount=\(converted.mainResults.count) \(diagnosticDetails)")
    serverLog("DEBUG", "Warmup: completed \(diagnosticDetails)")
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
    serverLog("DEBUG", "AppendText: start inputLength=\(inputString.count) inputStyle=\(String(describing: currentInputStyle))")
    composingText.insertAtCursorPosition(inputString, inputStyle: currentInputStyle)

    cursorPtr.pointee = composingText.convertTargetCursorPosition
    serverLog(
        "DEBUG",
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
    serverLog("DEBUG", "AppendTextDirect: start inputLength=\(inputString.count)")
    composingText.insertAtCursorPosition(inputString, inputStyle: .direct)

    cursorPtr.pointee = composingText.convertTargetCursorPosition
    serverLog(
        "DEBUG",
        "AppendTextDirect: completed cursor=\(cursorPtr.pointee) hiraganaLength=\(composingText.convertTarget.count)"
    )
    return _strdup(composingText.convertTarget)!
}

@_silgen_name("RemoveText")
@MainActor public func remove_text(
    cursorPtr: UnsafeMutablePointer<Int>
) -> UnsafeMutablePointer<CChar> {
    serverLog("DEBUG", "RemoveText: start")
    composingText.deleteBackwardFromCursorPosition(count: 1)

    cursorPtr.pointee = composingText.convertTargetCursorPosition
    serverLog(
        "DEBUG",
        "RemoveText: completed cursor=\(cursorPtr.pointee) hiraganaLength=\(composingText.convertTarget.count) inputCount=\(composingText.input.count)"
    )
    return _strdup(composingText.convertTarget)!
}

@_silgen_name("MoveCursor")
@MainActor public func move_cursor(
    offset: Int32,
    cursorPtr: UnsafeMutablePointer<Int>
) -> UnsafeMutablePointer<CChar> {
    serverLog("DEBUG", "MoveCursor: start offset=\(offset)")
    if offset == 125 {
        composingTextSnapshots.removeAll()
        cursorPtr.pointee = composingText.convertTargetCursorPosition
        serverLog("DEBUG", "MoveCursor: clear snapshots")
        return _strdup(composingText.convertTarget)!
    }

    if offset == 126 {
        composingTextSnapshots.append(composingText)
        cursorPtr.pointee = composingText.convertTargetCursorPosition
        serverLog("DEBUG", "MoveCursor: push snapshot count=\(composingTextSnapshots.count)")
        return _strdup(composingText.convertTarget)!
    }

    if offset == 127 {
        if let restored = composingTextSnapshots.popLast() {
            composingText = restored
        }
        cursorPtr.pointee = composingText.convertTargetCursorPosition
        serverLog("DEBUG", "MoveCursor: pop snapshot remaining=\(composingTextSnapshots.count)")
        return _strdup(composingText.convertTarget)!
    }

    let cursor = composingText.moveCursorFromCursorPosition(count: Int(offset))
    serverLog("DEBUG", "MoveCursor: offset=\(offset) cursor=\(cursor)")

    cursorPtr.pointee = cursor
    serverLog("DEBUG", "MoveCursor: completed cursor=\(cursor)")
    return _strdup(composingText.convertTarget)!
}

@_silgen_name("ClearText")
@MainActor public func clear_text() {
    serverLog("DEBUG", "ClearText: start")
    composingText = ComposingText()
    composingTextSnapshots.removeAll()
    serverLog("DEBUG", "ClearText: completed")
}

func to_list_pointer(_ list: [FFICandidate]) -> UnsafeMutablePointer<UnsafeMutablePointer<FFICandidate>?> {
    let pointer = UnsafeMutablePointer<UnsafeMutablePointer<FFICandidate>?>.allocate(capacity: list.count)
    for (i, item) in list.enumerated() {
        let candidatePtr = UnsafeMutablePointer<FFICandidate>.allocate(capacity: 1)
        candidatePtr.initialize(to: item)
        pointer.advanced(by: i).initialize(to: candidatePtr)
    }
    return pointer
}

@_silgen_name("FreeCString")
public func free_c_string(_ ptr: UnsafeMutablePointer<CChar>?) {
    guard let ptr else {
        return
    }
    free(ptr)
}

@_silgen_name("FreeCandidateList")
public func free_candidate_list(
    _ ptr: UnsafeMutablePointer<UnsafeMutablePointer<FFICandidate>?>?,
    _ length: Int32
) {
    guard let ptr else {
        return
    }

    guard length > 0 else {
        ptr.deinitialize(count: 0)
        ptr.deallocate()
        return
    }

    for index in 0..<Int(length) {
        guard let candidatePtr = ptr[index] else {
            continue
        }

        let candidate = candidatePtr.pointee
        free(candidate.text)
        free(candidate.subtext)
        free(candidate.hiragana)
        candidatePtr.deinitialize(count: 1)
        candidatePtr.deallocate()
    }

    ptr.deinitialize(count: Int(length))
    ptr.deallocate()
}

@_silgen_name("GetComposedText")
@MainActor public func get_composed_text(lengthPtr: UnsafeMutablePointer<Int>) -> UnsafeMutablePointer<UnsafeMutablePointer<FFICandidate>?> {
    let originalHiragana = composingText.convertTarget
    let contextString = (config["context"] as? String) ?? ""
    let diagnosticSnapshot = zenzaiDiagnosticSnapshot()
    let runtimeZenzaiEnabled = diagnosticSnapshot.runtimeEnabled
    let previewState = makeCandidatePreviewComposingText(from: composingText)
    let previewComposingText = previewState.composingText
    let previewHiragana = previewComposingText.convertTarget
    let useZenzai = effectiveZenzaiEnabledForCandidates(
        isConfigured: runtimeZenzaiEnabled,
        inputCount: composingText.input.count,
        hiraganaCount: originalHiragana.count
    )
    let diagnosticDetails = zenzaiDiagnosticDetails(
        snapshot: diagnosticSnapshot,
        contextLength: contextString.count,
        inputCount: composingText.input.count,
        hiraganaLength: originalHiragana.count,
        previewHiraganaLength: previewHiragana.count,
        useZenzai: useZenzai,
        syntheticEndOfText: previewState.syntheticEndOfText
    )
    serverLog(
        "DEBUG",
        "GetComposedText: start \(diagnosticDetails)"
    )
    let options = getOptions(context: contextString, zenzaiEnabled: useZenzai)
    crashTrace(operation: "GetComposedText", stage: "requestCandidates", state: "begin", details: diagnosticDetails)
    serverLog("DEBUG", "GetComposedText: requestCandidates begin \(diagnosticDetails)", flush: true)
    let requestStart = ProcessInfo.processInfo.systemUptime
    let converted = converter.requestCandidates(previewComposingText, options: options)
    let requestMs = Int((ProcessInfo.processInfo.systemUptime - requestStart) * 1000)
    performanceLog(
        operation: "get_composed_text",
        stage: "request_candidates",
        elapsedMs: requestMs,
        details: "candidate_count=\(converted.mainResults.count);\(diagnosticDetails)"
    )
    crashTrace(
        operation: "GetComposedText",
        stage: "requestCandidates",
        state: "completed",
        details: "candidate_count=\(converted.mainResults.count);\(diagnosticDetails)"
    )
    serverLog("DEBUG", "GetComposedText: requestCandidates returned candidateCount=\(converted.mainResults.count) \(diagnosticDetails)")
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
    serverLog("DEBUG", "GetComposedText: completed candidateCount=\(result.count) \(diagnosticDetails)")

    return to_list_pointer(result)
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
    let diagnosticSnapshot = zenzaiDiagnosticSnapshot()
    let runtimeZenzaiEnabled = diagnosticSnapshot.runtimeEnabled
    let useZenzai = effectiveZenzaiEnabledForCandidates(
        isConfigured: runtimeZenzaiEnabled,
        inputCount: prefixComposingText.input.count,
        hiraganaCount: prefixHiragana.count
    )
    let diagnosticDetails = zenzaiDiagnosticDetails(
        snapshot: diagnosticSnapshot,
        contextLength: contextString.count,
        inputCount: prefixComposingText.input.count,
        hiraganaLength: prefixHiragana.count,
        previewHiraganaLength: previewPrefixHiragana.count,
        useZenzai: useZenzai,
        syntheticEndOfText: previewState.syntheticEndOfText
    )
    serverLog(
        "DEBUG",
        "GetComposedTextForCursorPrefix: start suffix_len=\(suffixAfterCursor.count);\(diagnosticDetails)"
    )
    let options = getOptions(context: contextString, zenzaiEnabled: useZenzai)
    crashTrace(
        operation: "GetComposedTextForCursorPrefix",
        stage: "requestCandidates",
        state: "begin",
        details: "suffix_len=\(suffixAfterCursor.count);\(diagnosticDetails)"
    )
    serverLog("DEBUG", "GetComposedTextForCursorPrefix: requestCandidates begin suffix_len=\(suffixAfterCursor.count);\(diagnosticDetails)", flush: true)
    let totalStart = ProcessInfo.processInfo.systemUptime
    let requestStart = ProcessInfo.processInfo.systemUptime
    let converted = converter.requestCandidates(previewPrefixComposingText, options: options)
    let requestMs = Int((ProcessInfo.processInfo.systemUptime - requestStart) * 1000)
    performanceLog(
        operation: "get_composed_text_for_cursor_prefix",
        stage: "request_candidates",
        elapsedMs: requestMs,
        details: "first_clause_candidate_count=\(converted.firstClauseResults.count);main_candidate_count=\(converted.mainResults.count);suffix_len=\(suffixAfterCursor.count);\(diagnosticDetails)"
    )
    crashTrace(
        operation: "GetComposedTextForCursorPrefix",
        stage: "requestCandidates",
        state: "completed",
        details: "first_clause_candidate_count=\(converted.firstClauseResults.count);main_candidate_count=\(converted.mainResults.count);suffix_len=\(suffixAfterCursor.count);\(diagnosticDetails)"
    )
    serverLog("DEBUG", "GetComposedTextForCursorPrefix: requestCandidates returned firstClauseCandidateCount=\(converted.firstClauseResults.count) mainCandidateCount=\(converted.mainResults.count) suffix_len=\(suffixAfterCursor.count);\(diagnosticDetails)")
    crashTrace(
        operation: "GetComposedTextForCursorPrefix",
        stage: "postprocessCandidates",
        state: "begin",
        details: "phase=first_clause;first_clause_candidate_count=\(converted.firstClauseResults.count);main_candidate_count=\(converted.mainResults.count);suffix_len=\(suffixAfterCursor.count);\(diagnosticDetails)"
    )
    var cursorPrefixResolutionCache: [String: CandidateDisplayResolution] = [:]
    let firstClauseCorrespondingCount = cursorPrefixFirstClauseCorrespondingCount(
        firstClauseResults: converted.firstClauseResults,
        originalComposingText: prefixComposingText,
        previewComposingText: previewPrefixComposingText,
        resolutionCache: &cursorPrefixResolutionCache
    )
    let preliminaryCursorPrefixResults = cursorPrefixCandidateDisplayResults(
        mainResults: converted.mainResults,
        firstClauseResults: converted.firstClauseResults,
        firstClauseCorrespondingCount: firstClauseCorrespondingCount,
        originalComposingText: prefixComposingText,
        previewComposingText: previewPrefixComposingText,
        previewHiragana: previewPrefixHiragana,
        resolutionCache: &cursorPrefixResolutionCache
    )
    let shouldRequestExactClauseResults = preliminaryCursorPrefixResults.count < cursorPrefixExactClauseSupplementCandidateThreshold
    var exactClauseResults: [Candidate] = []
    if let firstClauseCorrespondingCount, shouldRequestExactClauseResults {
        let exactClauseComposingText = makeCursorPrefixExactClauseComposingText(
            prefixComposingText: prefixComposingText,
            correspondingCount: firstClauseCorrespondingCount
        )
        let exactClausePreviewState = makeCandidatePreviewComposingText(
            from: exactClauseComposingText
        )
        let exactClauseDiagnosticDetails = zenzaiDiagnosticDetails(
            snapshot: diagnosticSnapshot,
            contextLength: contextString.count,
            inputCount: exactClauseComposingText.input.count,
            hiraganaLength: exactClauseComposingText.convertTarget.count,
            previewHiraganaLength: exactClausePreviewState.composingText.convertTarget.count,
            useZenzai: useZenzai,
            syntheticEndOfText: exactClausePreviewState.syntheticEndOfText
        )
        crashTrace(
            operation: "GetComposedTextForCursorPrefix",
            stage: "requestCandidatesExactClause",
            state: "begin",
            details: "corresponding_count=\(firstClauseCorrespondingCount);\(exactClauseDiagnosticDetails)"
        )
        serverLog(
            "DEBUG",
            "GetComposedTextForCursorPrefix: requestCandidates exactClause begin correspondingCount=\(firstClauseCorrespondingCount) \(exactClauseDiagnosticDetails)",
            flush: true
        )
        let exactClauseRequestStart = ProcessInfo.processInfo.systemUptime
        let exactClauseConverted = converter.requestCandidates(
            exactClausePreviewState.composingText,
            options: options
        )
        exactClauseResults = exactClauseConverted.mainResults
        let exactClauseRequestMs = Int((ProcessInfo.processInfo.systemUptime - exactClauseRequestStart) * 1000)
        performanceLog(
            operation: "get_composed_text_for_cursor_prefix",
            stage: "request_candidates_exact_clause",
            elapsedMs: exactClauseRequestMs,
            details: "candidate_count=\(exactClauseResults.count);corresponding_count=\(firstClauseCorrespondingCount);\(exactClauseDiagnosticDetails)"
        )
        crashTrace(
            operation: "GetComposedTextForCursorPrefix",
            stage: "requestCandidatesExactClause",
            state: "completed",
            details: "candidate_count=\(exactClauseResults.count);corresponding_count=\(firstClauseCorrespondingCount);\(exactClauseDiagnosticDetails)"
        )
        serverLog(
            "DEBUG",
            "GetComposedTextForCursorPrefix: requestCandidates exactClause returned candidateCount=\(exactClauseResults.count) correspondingCount=\(firstClauseCorrespondingCount) \(exactClauseDiagnosticDetails)"
        )
    }
    crashTrace(
        operation: "GetComposedTextForCursorPrefix",
        stage: "postprocessCandidates",
        state: "begin",
        details: "phase=merge;preliminary_candidate_count=\(preliminaryCursorPrefixResults.count);exact_clause_candidate_count=\(exactClauseResults.count);suffix_len=\(suffixAfterCursor.count);\(diagnosticDetails)"
    )
    let cursorPrefixResults = exactClauseResults.isEmpty
        ? preliminaryCursorPrefixResults
        : cursorPrefixCandidateDisplayResults(
            mainResults: converted.mainResults,
            firstClauseResults: converted.firstClauseResults,
            exactClauseResults: exactClauseResults,
            firstClauseCorrespondingCount: firstClauseCorrespondingCount,
            originalComposingText: prefixComposingText,
            previewComposingText: previewPrefixComposingText,
            previewHiragana: previewPrefixHiragana,
            resolutionCache: &cursorPrefixResolutionCache
        )
    let totalMs = Int((ProcessInfo.processInfo.systemUptime - totalStart) * 1000)
    performanceLog(
        operation: "get_composed_text_for_cursor_prefix",
        stage: "total_before_ffi_candidates",
        elapsedMs: totalMs,
        details: "candidate_count=\(cursorPrefixResults.count);first_clause_candidate_count=\(converted.firstClauseResults.count);main_candidate_count=\(converted.mainResults.count);exact_clause_candidate_count=\(exactClauseResults.count);suffix_len=\(suffixAfterCursor.count);\(diagnosticDetails)"
    )
    var result: [FFICandidate] = []

    for i in 0..<cursorPrefixResults.count {
        let cursorPrefixResult = cursorPrefixResults[i]
        let candidate = cursorPrefixResult.candidate
        serverLog("DEBUG", "GetComposedTextForCursorPrefix: candidate[\(i + 1)/\(cursorPrefixResults.count)] start")

        let text = strdup(cursorPrefixResult.displayText)
        serverLog("DEBUG", "GetComposedTextForCursorPrefix: candidate[\(i + 1)] textReady")
        let hiragana = strdup(previewPrefixHiragana + suffixAfterCursor)
        let resolvedCandidate = resolveCandidateCompositionForDisplay(
            originalComposingText: prefixComposingText,
            previewComposingText: previewPrefixComposingText,
            candidateComposingCount: candidate.composingCount,
            resolutionCache: &cursorPrefixResolutionCache
        )
        let correspondingCount = resolvedCandidate.correspondingCount
        debugLogResolvedCorrespondingCount(
            scope: "GetComposedTextForCursorPrefix",
            candidateIndex: i,
            candidateTotal: cursorPrefixResults.count,
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
    crashTrace(
        operation: "GetComposedTextForCursorPrefix",
        stage: "postprocessCandidates",
        state: "completed",
        details: "candidate_count=\(result.count);first_clause_candidate_count=\(converted.firstClauseResults.count);main_candidate_count=\(converted.mainResults.count);exact_clause_candidate_count=\(exactClauseResults.count);suffix_len=\(suffixAfterCursor.count);\(diagnosticDetails)"
    )
    serverLog("DEBUG", "GetComposedTextForCursorPrefix: postprocessCandidates completed candidateCount=\(result.count) firstClauseCandidateCount=\(converted.firstClauseResults.count) mainCandidateCount=\(converted.mainResults.count) exactClauseCandidateCount=\(exactClauseResults.count) suffix_len=\(suffixAfterCursor.count);\(diagnosticDetails)")
    serverLog("DEBUG", "GetComposedTextForCursorPrefix: completed candidateCount=\(result.count) suffix_len=\(suffixAfterCursor.count);\(diagnosticDetails)")

    return to_list_pointer(result)
}

@_silgen_name("ShrinkText")
@MainActor public func shrink_text(
    offset: Int32
) -> UnsafeMutablePointer<CChar>  {
    serverLog("DEBUG", "ShrinkText: start offset=\(offset)")
    var afterComposingText = composingText
    afterComposingText.prefixComplete(composingCount: .inputCount(Int(offset)))
    composingText = afterComposingText

    serverLog("DEBUG", "ShrinkText: completed hiraganaLength=\(composingText.convertTarget.count) inputCount=\(composingText.input.count)")
    return _strdup(composingText.convertTarget)!
}

@_silgen_name("SetContext")
@MainActor public func set_context(
    context: UnsafePointer<CChar>
) {
    let contextString = String(cString: context)
    config["context"] = contextString
    serverLog("DEBUG", "SetContext: contextLength=\(contextString.count)")
}
