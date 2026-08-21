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
@MainActor var normalNBestSupplementConverter = KanaKanjiConverter(
    dictionaryURL: fallbackDictionaryURL,
    preloadDictionary: false
)
@MainActor var composingText = ComposingText()
@MainActor var composingTextSnapshots: [ComposingText] = []
@MainActor var currentInputStyle: InputStyle = .roman2kana
@MainActor var customRomajiTableEnabled = false
@MainActor var currentLearningType: LearningType = .inputAndOutput
@MainActor var currentLearningMemoryDirectoryURL: URL?
@MainActor var learningCandidateCache = LearningCandidateCache()
@MainActor var learningSelectionOverrides: [String: String] = [:]
@MainActor var reconversionDictionary = ReconversionDictionary()

@MainActor var execURL = URL(filePath: "")
@MainActor var config: [String : Any] = [
    "enable": false,
    "profile": "",
    "backend": "cpu",
    "experimentalTypoCorrection": false,
]
let maxUserDictionaryEntryCount = 50
let minInputCountForZenzaiCandidates = 4
let minHiraganaCountForZenzaiCandidates = 2
let zenzaiWarmupRomanInput = "nihongo"
let warmupRequestCandidatesWarningMs = 5_000
// Request exact-clause supplements only when boundary-matched candidates are sparse.
let cursorPrefixExactClauseSupplementCandidateThreshold = 5
// Bound bulk candidate generations while retaining the current clause plus at
// most 16 prepared future clauses with more than 2x slot headroom at the
// observed high-water mark of 201 candidates per generation. Past clauses can
// outlive this ring, so their one selected candidate is pinned separately until
// the composition is cleared.
let maxLearningCandidateCacheBatchCount = 64
let maxLearningCandidateCacheSlotCount = 8_192
let maxLearningSelectionOverrideCount = 4_096
let learningSelectionOverridesFilename = "selection-overrides.json"
let backgroundWarmupPreloadsDictionary = false
let maxReconversionSurfaceCount = 128
let maxReconversionDictionarySpan = 40
let maxReconversionReadingCount = 4

@MainActor var currentRequestId: UInt64 = 0

struct ReconversionDictionaryEntry: Equatable {
    let reading: String
    let score: Float
}

struct ReconversionDictionary {
    private static let magic = Array("AZR2".utf8)
    private var data = Data()
    private(set) var recordCount = 0
    private var userEntries: [String: [ReconversionDictionaryEntry]] = [:]

    mutating func load(from dictionaryURL: URL) throws {
        let url = dictionaryURL
            .appendingPathComponent("Reverse", isDirectory: true)
            .appendingPathComponent("reverse-v2.bin", isDirectory: false)
        let loaded = try Data(contentsOf: url, options: [.mappedIfSafe])
        guard loaded.count >= 12,
              Array(loaded.prefix(4)) == Self.magic else {
            throw CocoaError(.fileReadCorruptFile)
        }
        let count = Int(Self.readUInt32LE(loaded, at: 4))
        let headerEnd = 8 + (count + 1) * 4
        guard count > 0, headerEnd <= loaded.count,
              Int(Self.readUInt32LE(loaded, at: 8)) == headerEnd,
              Int(Self.readUInt32LE(loaded, at: 8 + count * 4)) == loaded.count else {
            throw CocoaError(.fileReadCorruptFile)
        }
        data = loaded
        recordCount = count
    }

    mutating func replaceUserEntries(_ dictionaryEntries: [DicdataElement]) {
        userEntries = Dictionary(grouping: dictionaryEntries, by: \.word).mapValues { entries in
            Dictionary(
                entries.map {
                    ($0.ruby, ReconversionDictionaryEntry(reading: $0.ruby, score: -4))
                },
                uniquingKeysWith: { first, _ in first }
            ).values.sorted { $0.reading < $1.reading }
        }
    }

    func inferReadings(
        for surface: String,
        limit: Int = maxReconversionReadingCount
    ) -> [String] {
        let characters = Array(surface)
        guard !characters.isEmpty,
              characters.count <= maxReconversionSurfaceCount,
              limit > 0 else {
            return []
        }

        let exactEntries = entries(for: surface)
        if !exactEntries.isEmpty {
            return exactEntries
                .sorted {
                    if $0.score != $1.score {
                        return $0.score > $1.score
                    }
                    return $0.reading < $1.reading
                }
                .prefix(limit)
                .map(\.reading)
        }

        struct Path {
            var score: Float
            var reading: String
        }

        var paths: [[Path]] = Array(repeating: [], count: characters.count + 1)
        paths[0] = [Path(score: 0, reading: "")]
        func insert(_ candidate: Path, at index: Int) {
            if let existing = paths[index].firstIndex(where: { $0.reading == candidate.reading }) {
                if candidate.score > paths[index][existing].score {
                    paths[index][existing] = candidate
                }
            } else {
                paths[index].append(candidate)
            }
            paths[index].sort {
                if $0.score != $1.score {
                    return $0.score > $1.score
                }
                return $0.reading < $1.reading
            }
            if paths[index].count > limit {
                paths[index].removeLast(paths[index].count - limit)
            }
        }

        for start in characters.indices {
            guard !paths[start].isEmpty else {
                continue
            }

            let sourcePaths = paths[start]
            for path in sourcePaths {
                if Self.canPassThrough(characters[start]) {
                    insert(
                        Path(
                            score: path.score - 0.05,
                            reading: path.reading + Self.normalizedLiteral(characters[start])
                        ),
                        at: start + 1
                    )
                }

                let upperBound = min(characters.count, start + maxReconversionDictionarySpan)
                guard start < upperBound else {
                    continue
                }
                var word = ""
                for end in (start + 1)...upperBound {
                    word.append(characters[end - 1])
                    for entry in entries(for: word) {
                        insert(
                            Path(
                                score: path.score + entry.score - 0.2,
                                reading: path.reading + entry.reading
                            ),
                            at: end
                        )
                    }
                }
            }
        }
        return paths[characters.count].map(\.reading)
    }

    private static func canPassThrough(_ character: Character) -> Bool {
        !character.unicodeScalars.contains { scalar in
            switch scalar.value {
            case 0x3005...0x3007, 0x303b,
                 0x3400...0x4dbf, 0x4e00...0x9fff, 0xf900...0xfaff,
                 0x20000...0x2fa1f:
                return true
            default:
                return false
            }
        }
    }

    private static func normalizedLiteral(_ character: Character) -> String {
        hiraganaToKatakana(String(character))
    }

    private func entries(for surface: String) -> [ReconversionDictionaryEntry] {
        var result = userEntries[surface] ?? []
        guard recordCount > 0 else {
            return result
        }
        let target = Array(surface.utf8)
        var lowerBound = 0
        var upperBound = recordCount
        while lowerBound < upperBound {
            let middle = lowerBound + (upperBound - lowerBound) / 2
            guard let record = record(at: middle) else {
                return result
            }
            let comparison = compareBytes(record.surface, target)
            if comparison < 0 {
                lowerBound = middle + 1
            } else {
                upperBound = middle
            }
        }
        var index = lowerBound
        while index < recordCount, let record = record(at: index), compareBytes(record.surface, target) == 0 {
            let reading = String(decoding: record.reading, as: UTF8.self)
            if !result.contains(where: { $0.reading == reading }) {
                result.append(ReconversionDictionaryEntry(reading: reading, score: record.score))
            }
            index += 1
        }
        return result
    }

    private func record(at index: Int) -> (surface: Data.SubSequence, reading: Data.SubSequence, score: Float)? {
        guard (0..<recordCount).contains(index) else {
            return nil
        }
        let start = Int(Self.readUInt32LE(data, at: 8 + index * 4))
        let end = Int(Self.readUInt32LE(data, at: 8 + (index + 1) * 4))
        guard start >= 8 + (recordCount + 1) * 4,
              end >= start + 8,
              end <= data.count else {
            return nil
        }
        let surfaceLength = Int(Self.readUInt16LE(data, at: start))
        let readingLength = Int(Self.readUInt16LE(data, at: start + 2))
        let surfaceStart = start + 8
        let readingStart = surfaceStart + surfaceLength
        guard readingStart + readingLength == end else {
            return nil
        }
        return (
            data[surfaceStart..<readingStart],
            data[readingStart..<end],
            Self.readFloat32LE(data, at: start + 4)
        )
    }

    private func compareBytes(_ left: Data.SubSequence, _ right: [UInt8]) -> Int {
        let commonCount = min(left.count, right.count)
        for index in 0..<commonCount {
            let leftByte = left[left.startIndex + index]
            let rightByte = right[index]
            if leftByte != rightByte {
                return leftByte < rightByte ? -1 : 1
            }
        }
        if left.count == right.count {
            return 0
        }
        return left.count < right.count ? -1 : 1
    }

    private static func readUInt16LE(_ data: Data, at offset: Int) -> UInt16 {
        UInt16(data[data.startIndex + offset])
            | (UInt16(data[data.startIndex + offset + 1]) << 8)
    }

    private static func readUInt32LE(_ data: Data, at offset: Int) -> UInt32 {
        UInt32(data[data.startIndex + offset])
            | (UInt32(data[data.startIndex + offset + 1]) << 8)
            | (UInt32(data[data.startIndex + offset + 2]) << 16)
            | (UInt32(data[data.startIndex + offset + 3]) << 24)
    }

    private static func readFloat32LE(_ data: Data, at offset: Int) -> Float {
        Float(bitPattern: readUInt32LE(data, at: offset))
    }
}

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

private func serverLog(
    requestId: UInt64,
    _ level: String = "INFO",
    _ message: @autoclosure () -> String,
    flush: Bool = false
) {
    guard serverLogCallbacks.isLogEnabled(level: level) else {
        return
    }

    serverLogCallbacks.log(level: level, message: "request_id=\(requestId) \(message())")
    if flush {
        serverLogCallbacks.flush()
    }
}

@MainActor private func serverLog(
    _ level: String = "INFO",
    _ message: @autoclosure () -> String,
    flush: Bool = false
) {
    serverLog(requestId: currentRequestId, level, message(), flush: flush)
}

private func crashTrace(
    requestId: UInt64,
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
        details: "request_id=\(requestId);\(details())"
    )
}

@MainActor private func crashTrace(
    operation: String,
    stage: String,
    state: String,
    details: @autoclosure () -> String = ""
) {
    crashTrace(
        requestId: currentRequestId,
        operation: operation,
        stage: stage,
        state: state,
        details: details()
    )
}

@MainActor private func candidateCrashTrace(
    useZenzai: Bool,
    operation: String,
    stage: String,
    state: String,
    details: @autoclosure () -> String = ""
) {
    guard useZenzai else {
        return
    }

    crashTrace(operation: operation, stage: stage, state: state, details: details())
}

private func performanceLog(
    requestId: UInt64,
    operation: String,
    stage: String,
    elapsedMs: Int,
    details: @autoclosure () -> String = ""
) {
    guard serverLogCallbacks.isPerformanceLogEnabled() else {
        return
    }

    serverLogCallbacks.performanceLog(
        requestId: requestId,
        operation: operation,
        stage: stage,
        elapsedMs: UInt64(max(0, elapsedMs)),
        details: details()
    )
}

@MainActor private func performanceLog(
    operation: String,
    stage: String,
    elapsedMs: Int,
    details: @autoclosure () -> String = ""
) {
    performanceLog(
        requestId: currentRequestId,
        operation: operation,
        stage: stage,
        elapsedMs: elapsedMs,
        details: details()
    )
}

private func performanceNow() -> TimeInterval {
    ProcessInfo.processInfo.systemUptime
}

private func elapsedPerformanceMilliseconds(since start: TimeInterval) -> Int {
    Int((performanceNow() - start) * 1000)
}

func applicationDataDirectoryURL(
    appDataPath: String?,
    temporaryDirectoryURL: URL
) -> URL {
    if let appDataPath,
       !appDataPath.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
        return URL(filePath: appDataPath)
            .appendingPathComponent("Azookey", isDirectory: true)
    }

    return temporaryDirectoryURL.appendingPathComponent("Azookey", isDirectory: true)
}

private func applicationDataDirectoryURL() -> URL {
    applicationDataDirectoryURL(
        appDataPath: ProcessInfo.processInfo.environment["APPDATA"],
        temporaryDirectoryURL: FileManager.default.temporaryDirectory
    )
}

func engineRuntimeDirectoryURL(
    appDataPath: String?,
    temporaryDirectoryURL: URL
) -> URL {
    applicationDataDirectoryURL(
        appDataPath: appDataPath,
        temporaryDirectoryURL: temporaryDirectoryURL
    ).appendingPathComponent("EngineRuntime", isDirectory: true)
}

private func settingsPath() -> URL {
    applicationDataDirectoryURL().appendingPathComponent("settings.json")
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
    normalNBestSupplementConverter = KanaKanjiConverter(
        dictionaryURL: converterDictionaryURL,
        preloadDictionary: converterPreloadDictionary
    )
}

@MainActor private func converterRuntimeDirectoryURL() -> URL {
    engineRuntimeDirectoryURL(
        appDataPath: ProcessInfo.processInfo.environment["APPDATA"],
        temporaryDirectoryURL: FileManager.default.temporaryDirectory
    )
}

@MainActor private func ensureConverterRuntimeDirectory() {
    let runtimeDirectoryURL = converterRuntimeDirectoryURL()
    do {
        try FileManager.default.createDirectory(
            at: runtimeDirectoryURL,
            withIntermediateDirectories: true
        )
    } catch {
        serverLog(
            "WARN",
            "Failed to create engine runtime directory \(runtimeDirectoryURL.path): \(error)"
        )
    }
}

func normalizedZenzaiBackend(_ backend: String?) -> String {
    (backend ?? "cpu")
        .trimmingCharacters(in: .whitespacesAndNewlines)
        .lowercased()
}

private func shouldOffloadZenzaiToGpu(zenzaiEnabled: Bool, backend: String?) -> Bool {
    let normalizedBackend = normalizedZenzaiBackend(backend)
    return zenzaiEnabled && !normalizedBackend.isEmpty && normalizedBackend != "cpu"
}

@MainActor private func configureEngineRuntime(zenzaiEnabled: Bool) {
    let shouldOffloadToGpu = shouldOffloadZenzaiToGpu(
        zenzaiEnabled: zenzaiEnabled,
        backend: config["backend"] as? String
    )
    KanaKanjiConverterEngineRuntime.configure(
        gpuLayerCount: shouldOffloadToGpu ? Int32.max : 0
    )
}

@MainActor private func learningMemoryDirectoryURL(settingsPath: URL?) -> URL {
    if let settingsPath {
        return settingsPath
            .deletingLastPathComponent()
            .appendingPathComponent("LearningMemory", isDirectory: true)
    }

    return applicationDataDirectoryURL()
        .appendingPathComponent("LearningMemory", isDirectory: true)
}

private func normalizedLearningMode(_ mode: String?) -> String {
    (mode ?? "enabled")
        .trimmingCharacters(in: .whitespacesAndNewlines)
        .lowercased()
        .replacingOccurrences(of: "-", with: "_")
}

private func learningType(for mode: String?) -> LearningType {
    switch normalizedLearningMode(mode) {
    case "read_only", "readonly", "no_new_learning":
        return .onlyOutput
    case "disabled":
        return .nothing
    default:
        return .inputAndOutput
    }
}

@MainActor private func ensureLearningMemoryDirectoryIfNeeded() {
    guard currentLearningType != .nothing,
          let currentLearningMemoryDirectoryURL else {
        return
    }

    do {
        try FileManager.default.createDirectory(
            at: currentLearningMemoryDirectoryURL,
            withIntermediateDirectories: true
        )
    } catch {
        serverLog(
            "WARN",
            "Failed to create learning memory directory \(currentLearningMemoryDirectoryURL.path): \(error)"
        )
    }
}

@MainActor private func resetLearningMemoryDirectory() -> Bool {
    guard let currentLearningMemoryDirectoryURL else {
        serverLog("WARN", "ResetLearningMemory: learning memory directory is not configured")
        return false
    }

    do {
        if FileManager.default.fileExists(atPath: currentLearningMemoryDirectoryURL.path) {
            try FileManager.default.removeItem(at: currentLearningMemoryDirectoryURL)
        }
        try FileManager.default.createDirectory(
            at: currentLearningMemoryDirectoryURL,
            withIntermediateDirectories: true
        )
        return true
    } catch {
        serverLog(
            "WARN",
            "ResetLearningMemory: failed to reset directory \(currentLearningMemoryDirectoryURL.path): \(error)"
        )
        return false
    }
}

struct LearningCandidateCache {
    private struct Batch {
        let firstId: UInt64
        let candidates: [Candidate]
        var consumedOffsets: Set<Int> = []
        var isProtected = false
    }

    private let maxBatchCount: Int
    private let maxSlotCount: Int
    private var batches: [Batch?]
    private var firstBatchIndex = 0
    private var nextCandidateId: UInt64 = 1
    private var idSpaceExhausted = false
    private(set) var slotCount = 0
    private(set) var batchCount = 0
    private(set) var protectedSlotCount = 0

    init(
        maxBatchCount: Int = maxLearningCandidateCacheBatchCount,
        maxSlotCount: Int = maxLearningCandidateCacheSlotCount
    ) {
        precondition(maxBatchCount > 0)
        precondition(maxSlotCount > 0)
        self.maxBatchCount = maxBatchCount
        self.maxSlotCount = maxSlotCount
        self.batches = Array(repeating: nil, count: maxBatchCount)
    }

    mutating func appendBatch(_ candidates: [Candidate]) -> UInt64? {
        guard !candidates.isEmpty, !idSpaceExhausted else {
            return nil
        }

        let availableBatchSlotCount = maxSlotCount - protectedSlotCount
        guard availableBatchSlotCount > 0 else {
            return nil
        }
        let retainedCandidates = if candidates.count > availableBatchSlotCount {
            Array(candidates.prefix(availableBatchSlotCount))
        } else {
            candidates
        }
        let candidateCount = UInt64(retainedCandidates.count)
        let (nextId, overflow) = nextCandidateId.addingReportingOverflow(candidateCount)
        if overflow {
            idSpaceExhausted = true
            return nil
        }

        while batchCount == maxBatchCount
            || slotCount > maxSlotCount - retainedCandidates.count
        {
            guard evictOldestUnprotectedBatch() else {
                return nil
            }
        }

        let firstId = nextCandidateId
        let insertionIndex = physicalIndex(forLogicalIndex: batchCount)
        batches[insertionIndex] = Batch(
            firstId: firstId,
            candidates: retainedCandidates
        )
        batchCount += 1
        slotCount += retainedCandidates.count
        nextCandidateId = nextId
        return firstId
    }

    func candidateId(at index: Int, batchFirstId: UInt64) -> UInt64 {
        guard index >= 0,
              batchCount > 0,
              let batch = batches[physicalIndex(forLogicalIndex: batchCount - 1)],
              batch.firstId == batchFirstId,
              index < batch.candidates.count else {
            return 0
        }

        return batchFirstId + UInt64(index)
    }

    mutating func pin(_ candidateId: UInt64) -> Bool {
        guard candidateId > 0 else {
            return false
        }
        guard let location = candidateLocation(for: candidateId),
              !batches[location.batchIndex]!.consumedOffsets.contains(location.candidateIndex)
        else {
            return false
        }
        if batches[location.batchIndex]!.isProtected {
            return true
        }

        // A client snapshot can later select any candidate from this batch.
        // Protect the complete batch so every issued nonzero ID stays valid.
        let candidateCount = batches[location.batchIndex]!.candidates.count
        batches[location.batchIndex]!.isProtected = true
        protectedSlotCount += candidateCount
        return true
    }

    mutating func consume(_ candidateId: UInt64) -> Candidate? {
        guard candidateId > 0 else {
            return nil
        }
        guard let location = candidateLocation(for: candidateId) else {
            return nil
        }
        guard batches[location.batchIndex]!.consumedOffsets
            .insert(location.candidateIndex).inserted else {
            return nil
        }

        return batches[location.batchIndex]!.candidates[location.candidateIndex]
    }

    mutating func removeAll() {
        for index in batches.indices {
            batches[index] = nil
        }
        firstBatchIndex = 0
        batchCount = 0
        slotCount = 0
        protectedSlotCount = 0
    }

    mutating func removeAllPins() {
        guard protectedSlotCount > 0 else {
            return
        }
        for logicalIndex in 0..<batchCount {
            batches[physicalIndex(forLogicalIndex: logicalIndex)]!.isProtected = false
        }
        protectedSlotCount = 0
    }

    private func physicalIndex(forLogicalIndex logicalIndex: Int) -> Int {
        (firstBatchIndex + logicalIndex) % maxBatchCount
    }

    private func candidateLocation(
        for candidateId: UInt64
    ) -> (batchIndex: Int, candidateIndex: Int)? {
        var lowerBound = 0
        var upperBound = batchCount
        while lowerBound < upperBound {
            let middle = lowerBound + (upperBound - lowerBound) / 2
            let batch = batches[physicalIndex(forLogicalIndex: middle)]!
            if batch.firstId <= candidateId {
                lowerBound = middle + 1
            } else {
                upperBound = middle
            }
        }

        guard lowerBound > 0 else {
            return nil
        }
        let batchIndex = physicalIndex(forLogicalIndex: lowerBound - 1)
        let batch = batches[batchIndex]!
        let offset = candidateId - batch.firstId
        guard offset < UInt64(batch.candidates.count) else {
            return nil
        }
        return (batchIndex, Int(offset))
    }

    private mutating func evictOldestUnprotectedBatch() -> Bool {
        guard batchCount > 0 else {
            return false
        }

        var victimLogicalIndex: Int?
        for logicalIndex in 0..<batchCount {
            let batchIndex = physicalIndex(forLogicalIndex: logicalIndex)
            if !batches[batchIndex]!.isProtected {
                victimLogicalIndex = logicalIndex
                break
            }
        }
        guard let victimLogicalIndex else {
            return false
        }

        let victimBatchIndex = physicalIndex(forLogicalIndex: victimLogicalIndex)
        slotCount -= batches[victimBatchIndex]!.candidates.count
        if victimLogicalIndex == 0 {
            batches[victimBatchIndex] = nil
            firstBatchIndex = (firstBatchIndex + 1) % maxBatchCount
        } else {
            for logicalIndex in victimLogicalIndex..<(batchCount - 1) {
                let destination = physicalIndex(forLogicalIndex: logicalIndex)
                let source = physicalIndex(forLogicalIndex: logicalIndex + 1)
                batches[destination] = batches[source]
            }
            batches[physicalIndex(forLogicalIndex: batchCount - 1)] = nil
        }
        batchCount -= 1
        return true
    }
}

@MainActor private func clearLearningCandidateCache() {
    learningCandidateCache.removeAll()
}

@MainActor func cacheLearningCandidates(_ candidates: [Candidate]) -> UInt64? {
    guard currentLearningType == .inputAndOutput else {
        return nil
    }

    return learningCandidateCache.appendBatch(
        disableLearningForKeyboardTypoCorrectionCandidates(
            candidates,
            experimentalTypoCorrectionEnabled:
                (config["experimentalTypoCorrection"] as? Bool) ?? false
        )
    )
}

@MainActor func learningCandidateId(at index: Int, batchFirstId: UInt64?) -> UInt64 {
    guard let batchFirstId else {
        return 0
    }

    return learningCandidateCache.candidateId(at: index, batchFirstId: batchFirstId)
}

@MainActor func consumeLearningCandidate(_ candidateId: UInt64) -> Candidate? {
    learningCandidateCache.consume(candidateId)
}

@_silgen_name("PinLearningCandidate")
@MainActor public func pin_learning_candidate(candidateId: UInt64) -> Bool {
    learningCandidateCache.pin(candidateId)
}

// Client-side future snapshots do not have matching server composition
// snapshots. The first subsequent composition edit invalidates those futures,
// so release their selected candidates without adding another IPC round trip.
@MainActor func discardPinnedLearningCandidatesBeforeCompositionEdit() {
    guard composingTextSnapshots.isEmpty,
          learningCandidateCache.protectedSlotCount > 0 else {
        return
    }
    learningCandidateCache.removeAllPins()
}

private func normalizedLearningRuby(_ ruby: String) -> String {
    ruby.unicodeScalars.reduce(into: "") { normalized, scalar in
        let value = scalar.value
        let normalizedValue = switch value {
        case 0x3041...0x3096, 0x309D...0x309F:
            value + 0x60
        default:
            value
        }
        normalized.unicodeScalars.append(UnicodeScalar(normalizedValue)!)
    }
}

private func learningCandidateRuby(_ candidate: Candidate) -> String {
    normalizedLearningRuby(candidate.data.reduce(into: "") { $0 += $1.ruby })
}

private func learningCandidateOutput(_ candidate: Candidate) -> String {
    candidate.data.reduce(into: "") { $0 += $1.word }
}

@MainActor private func learningSelectionOverridesURL() -> URL? {
    currentLearningMemoryDirectoryURL?.appendingPathComponent(
        learningSelectionOverridesFilename,
        isDirectory: false
    )
}

@MainActor func loadLearningSelectionOverrides() {
    guard currentLearningType != .nothing,
          let overridesURL = learningSelectionOverridesURL() else {
        learningSelectionOverrides.removeAll(keepingCapacity: false)
        return
    }

    guard FileManager.default.fileExists(atPath: overridesURL.path) else {
        learningSelectionOverrides.removeAll(keepingCapacity: false)
        return
    }

    do {
        let data = try Data(contentsOf: overridesURL)
        let decoded = try JSONDecoder().decode([String: String].self, from: data)
        learningSelectionOverrides = Dictionary(
            uniqueKeysWithValues: decoded.prefix(maxLearningSelectionOverrideCount).map {
                ($0.key, $0.value)
            }
        )
    } catch {
        learningSelectionOverrides.removeAll(keepingCapacity: false)
        serverLog(
            "WARN",
            "Failed to load learning selection overrides at \(overridesURL.path): \(error)"
        )
    }
}

@MainActor private func saveLearningSelectionOverrides() {
    guard let overridesURL = learningSelectionOverridesURL() else {
        return
    }

    do {
        ensureLearningMemoryDirectoryIfNeeded()
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        let data = try encoder.encode(learningSelectionOverrides)
        try data.write(to: overridesURL, options: .atomic)
    } catch {
        serverLog(
            "WARN",
            "Failed to save learning selection overrides at \(overridesURL.path): \(error)"
        )
    }
}

@MainActor private func updateLearningSelectionOverride(_ candidate: Candidate) -> Bool {
    guard candidate.isLearningTarget else {
        return false
    }

    let ruby = learningCandidateRuby(candidate)
    let output = learningCandidateOutput(candidate)
    guard !ruby.isEmpty, !output.isEmpty else {
        return false
    }

    if learningSelectionOverrides[ruby] == nil,
       learningSelectionOverrides.count >= maxLearningSelectionOverrideCount,
       let evictedRuby = learningSelectionOverrides.keys.first {
        learningSelectionOverrides.removeValue(forKey: evictedRuby)
    }
    learningSelectionOverrides[ruby] = output
    return true
}

@MainActor private func recordLearningSelectionOverride(_ candidate: Candidate) {
    guard updateLearningSelectionOverride(candidate) else {
        return
    }
    saveLearningSelectionOverrides()
}

@MainActor func prioritizeLearningSelectionOverrides<Element>(
    _ elements: [Element],
    ruby: String,
    candidate: (Element) -> Candidate
) -> [Element] {
    let normalizedRuby = normalizedLearningRuby(ruby)
    guard currentLearningType != .nothing,
          let preferredOutput = learningSelectionOverrides[normalizedRuby],
          elements.count > 1 else {
        return elements
    }

    var firstIndex: Int?
    var preferredIndex: Int?
    for (index, element) in elements.enumerated() {
        let value = candidate(element)
        guard learningCandidateRuby(value) == normalizedRuby else {
            continue
        }
        if firstIndex == nil {
            firstIndex = index
        }
        if preferredOutput == learningCandidateOutput(value) {
            preferredIndex = index
            break
        }
    }

    guard let firstIndex, let preferredIndex, firstIndex != preferredIndex else {
        return elements
    }
    var prioritized = elements
    prioritized.swapAt(firstIndex, preferredIndex)
    return prioritized
}

private func makeConvertRequestOptions(
    context: String,
    zenzaiEnabled: Bool,
    runtimeDirectoryURL: URL,
    emojiDictionaryURL: URL,
    zenzaiWeightURL: URL,
    profile: String,
    learningType: LearningType = .nothing,
    learningMemoryDirectoryURL: URL? = nil,
    experimentalKeyboardTypoCorrection: Bool = false
) -> ConvertRequestOptions {
    return ConvertRequestOptions(
        requireJapanesePrediction: .disabled,
        requireEnglishPrediction: .disabled,
        keyboardLanguage: .ja_JP,
        learningType: learningType,
        memoryDirectoryURL: learningMemoryDirectoryURL ?? runtimeDirectoryURL,
        sharedContainerURL: runtimeDirectoryURL,
        textReplacer: .init {
            return emojiDictionaryURL
        },
        specialCandidateProviders: nil,
        zenzaiMode: zenzaiEnabled ? .on(
            weight: zenzaiWeightURL,
            inferenceLimit: 1,
            requestRichCandidates: true,
            personalizationMode: nil,
            versionDependentMode: .v3(
                .init(
                    profile: profile,
                    leftSideContext: context
                )
            )
        ) : .off,
        experimentalKeyboardTypoCorrection: experimentalKeyboardTypoCorrection,
        metadata: .init(versionString: "Azookey for Windows")
    )
}

private struct AppSettings: Decodable {
    let zenzai: ZenzaiSettings?
    let learning: LearningSettings?
    let user_dictionary: UserDictionarySettings?
    let romaji_table: RomajiTableSettings?
    let general: GeneralSettings?
}

private struct GeneralSettings: Decodable {
    let experimental_typo_correction: Bool?
}

private struct LearningSettings: Decodable {
    let mode: String?
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
    // A first-clause result can occasionally consume the first character of
    // the following clause while its displayed ruby stops before it. Such a
    // boundary makes the first Shift+Left change invisible (for example,
    // "ある程度な" -> "ある程度"). Prefer a display-aligned boundary.
    let overconsumedSurfacePenalty = prefixSurfaceCount > candidate.rubyCount ? 160 : 0

    return resolution.correspondingCount * 4
        + terminalBonus
        - tokenBoundaryPenalty
        - overconsumedSurfacePenalty
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
    return makeConvertRequestOptions(
        context: context,
        zenzaiEnabled: zenzaiEnabled,
        runtimeDirectoryURL: converterRuntimeDirectoryURL(),
        emojiDictionaryURL: execURL
            .appendingPathComponent("EmojiDictionary")
            .appendingPathComponent("emoji_all_E15.1.txt"),
        zenzaiWeightURL: execURL.appendingPathComponent("zenz.gguf"),
        profile: (config["profile"] as? String) ?? "",
        learningType: currentLearningType,
        learningMemoryDirectoryURL: currentLearningMemoryDirectoryURL,
        experimentalKeyboardTypoCorrection:
            (config["experimentalTypoCorrection"] as? Bool) ?? false
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

@MainActor private func makeWarmupComposingText(
    input: String,
    inputStyle: InputStyle
) -> ComposingText {
    var warmupComposingText = ComposingText()
    warmupComposingText.insertAtCursorPosition(input, inputStyle: inputStyle)
    return warmupComposingText
}

@MainActor func makeWarmupComposingText(
    zenzaiRuntimeEnabled: Bool? = nil,
    inputStyle: InputStyle? = nil
) -> ComposingText {
    let selectedInputStyle = inputStyle ?? currentInputStyle
    let useZenzaiWarmup = zenzaiRuntimeEnabled ?? currentRuntimeZenzaiEnabled()
    guard useZenzaiWarmup else {
        return makeWarmupComposingText(input: "a", inputStyle: selectedInputStyle)
    }

    return makeWarmupComposingText(input: zenzaiWarmupRomanInput, inputStyle: .roman2kana)
}

private struct WarmupExecutionSnapshot: Sendable {
    let requestId: UInt64
    let dictionaryURL: URL
    let preloadDictionary: Bool
    let runtimeDirectoryURL: URL
    let emojiDictionaryURL: URL
    let zenzaiWeightURL: URL
    let profile: String
    let context: String
    let input: String
    let useZenzai: Bool
    let learningCacheBatchCount: Int
    let learningCacheSlotCount: Int
    let learningCacheProtectedSlotCount: Int
    let diagnosticDetails: String
}

private struct WarmupConverterKey: Equatable {
    let dictionaryURL: URL
    let preloadDictionary: Bool
}

private final class BackgroundWarmupRunner: @unchecked Sendable {
    private let lock = NSLock()
    private let queue = DispatchQueue(label: "azookey.server.warmup", qos: .utility)
    private var isRunning = false
    private var converterKey: WarmupConverterKey?
    private var converter: KanaKanjiConverter?

    func schedule(_ snapshot: WarmupExecutionSnapshot) -> Bool {
        lock.lock()
        guard !isRunning else {
            lock.unlock()
            return false
        }
        isRunning = true
        lock.unlock()

        queue.async { [self, snapshot] in
            defer {
                self.lock.lock()
                self.isRunning = false
                self.lock.unlock()
            }
            self.run(snapshot)
        }
        return true
    }

    private func run(_ snapshot: WarmupExecutionSnapshot) {
        var warmupComposingText = ComposingText()
        warmupComposingText.insertAtCursorPosition(
            snapshot.input,
            inputStyle: .direct
        )
        let options = makeConvertRequestOptions(
            context: snapshot.context,
            zenzaiEnabled: snapshot.useZenzai,
            runtimeDirectoryURL: snapshot.runtimeDirectoryURL,
            emojiDictionaryURL: snapshot.emojiDictionaryURL,
            zenzaiWeightURL: snapshot.zenzaiWeightURL,
            profile: snapshot.profile
        )

        crashTrace(
            requestId: snapshot.requestId,
            operation: "Warmup",
            stage: "requestCandidates",
            state: "begin",
            details: snapshot.diagnosticDetails
        )
        serverLog(
            requestId: snapshot.requestId,
            "DEBUG",
            "Warmup: requestCandidates begin \(snapshot.diagnosticDetails)",
            flush: true
        )

        let key = WarmupConverterKey(
            dictionaryURL: snapshot.dictionaryURL,
            preloadDictionary: snapshot.preloadDictionary
        )
        let reusedConverter = converterKey == key && converter != nil
        performanceLog(
            requestId: snapshot.requestId,
            operation: "warmup_resources",
            stage: "before_request_candidates",
            elapsedMs: 0,
            details: "foreground_converter_count=2;background_converter_count=\(converter == nil ? 0 : 1);background_converter_reused=\(reusedConverter);background_preload_dictionary=\(snapshot.preloadDictionary);background_use_zenzai=\(snapshot.useZenzai);learning_cache_batches=\(snapshot.learningCacheBatchCount);learning_cache_slots=\(snapshot.learningCacheSlotCount);learning_cache_protected_slots=\(snapshot.learningCacheProtectedSlotCount)"
        )
        if converterKey != key || converter == nil {
            converter = KanaKanjiConverter(
                dictionaryURL: snapshot.dictionaryURL,
                preloadDictionary: snapshot.preloadDictionary
            )
            converterKey = key
        }

        let requestStart = ProcessInfo.processInfo.systemUptime
        let converted = converter!.requestCandidates(
            warmupComposingText,
            options: options
        )
        let requestMs = Int((ProcessInfo.processInfo.systemUptime - requestStart) * 1000)
        if requestMs >= warmupRequestCandidatesWarningMs {
            serverLog(
                requestId: snapshot.requestId,
                "WARN",
                "Warmup: requestCandidates slow elapsed_ms=\(requestMs);threshold_ms=\(warmupRequestCandidatesWarningMs) \(snapshot.diagnosticDetails)",
                flush: true
            )
        }
        performanceLog(
            requestId: snapshot.requestId,
            operation: "warmup",
            stage: "request_candidates",
            elapsedMs: requestMs,
            details: "candidate_count=\(converted.mainResults.count);foreground_converter_count=2;background_converter_count=1;background_converter_reused=\(reusedConverter);learning_cache_batches=\(snapshot.learningCacheBatchCount);learning_cache_slots=\(snapshot.learningCacheSlotCount);learning_cache_protected_slots=\(snapshot.learningCacheProtectedSlotCount);\(snapshot.diagnosticDetails)"
        )
        crashTrace(
            requestId: snapshot.requestId,
            operation: "Warmup",
            stage: "requestCandidates",
            state: "completed",
            details: "candidate_count=\(converted.mainResults.count);\(snapshot.diagnosticDetails)"
        )
        serverLog(
            requestId: snapshot.requestId,
            "DEBUG",
            "Warmup: requestCandidates returned candidateCount=\(converted.mainResults.count) \(snapshot.diagnosticDetails)"
        )
        serverLog(
            requestId: snapshot.requestId,
            "DEBUG",
            "Warmup: completed \(snapshot.diagnosticDetails)"
        )
    }
}

private let backgroundWarmupRunner = BackgroundWarmupRunner()

// InputStyleManager stores custom tables in an unsynchronized process-wide
// dictionary. Background work must use direct input so it never reads that
// registry concurrently with a foreground config reload.
@MainActor func makeBackgroundWarmupComposingText(
    zenzaiRuntimeEnabled: Bool
) -> ComposingText {
    makeWarmupComposingText(
        input: zenzaiRuntimeEnabled ? "にほんご" : "あ",
        inputStyle: .direct
    )
}

@MainActor private func makeBackgroundWarmupSnapshot() -> WarmupExecutionSnapshot {
    let contextString = (config["context"] as? String) ?? ""
    let diagnosticSnapshot = zenzaiDiagnosticSnapshot()
    let warmupComposingText = makeBackgroundWarmupComposingText(
        zenzaiRuntimeEnabled: diagnosticSnapshot.runtimeEnabled
    )
    let input = warmupComposingText.convertTarget
    let useZenzai = effectiveZenzaiEnabledForCandidates(
        isConfigured: diagnosticSnapshot.runtimeEnabled,
        inputCount: warmupComposingText.input.count,
        hiraganaCount: warmupComposingText.convertTarget.count
    )
    configureEngineRuntime(zenzaiEnabled: useZenzai)
    let diagnosticDetails = zenzaiDiagnosticDetails(
        snapshot: diagnosticSnapshot,
        contextLength: contextString.count,
        inputCount: warmupComposingText.input.count,
        hiraganaLength: warmupComposingText.convertTarget.count,
        useZenzai: useZenzai
    ) + ";warmup_input_style=direct;background=true"

    return WarmupExecutionSnapshot(
        requestId: currentRequestId,
        dictionaryURL: converterDictionaryURL,
        preloadDictionary: backgroundWarmupPreloadsDictionary,
        runtimeDirectoryURL: converterRuntimeDirectoryURL(),
        emojiDictionaryURL: execURL
            .appendingPathComponent("EmojiDictionary")
            .appendingPathComponent("emoji_all_E15.1.txt"),
        zenzaiWeightURL: execURL.appendingPathComponent("zenz.gguf"),
        profile: (config["profile"] as? String) ?? "",
        context: contextString,
        input: input,
        useZenzai: useZenzai,
        learningCacheBatchCount: learningCandidateCache.batchCount,
        learningCacheSlotCount: learningCandidateCache.slotCount,
        learningCacheProtectedSlotCount: learningCandidateCache.protectedSlotCount,
        diagnosticDetails: diagnosticDetails
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
    var result = ""
    result.reserveCapacity(hiragana.count)

    var remainingStart = hiragana.startIndex
    var remainingCount = hiragana.count
    for data in candidate.data {
        let rubyCount = data.ruby.count
        if remainingCount < rubyCount {
            result += hiragana[remainingStart...]
            break
        }

        remainingStart = hiragana.index(remainingStart, offsetBy: rubyCount)
        remainingCount -= rubyCount
        result += data.word
    }

    return result
}

func hiraganaToKatakana(_ text: String) -> String {
    var scalars = String.UnicodeScalarView()
    scalars.reserveCapacity(text.unicodeScalars.count)

    for scalar in text.unicodeScalars {
        let value = scalar.value
        if (0x3041...0x3096).contains(value), let converted = UnicodeScalar(value + 0x60) {
            scalars.append(converted)
        } else {
            scalars.append(scalar)
        }
    }

    return String(scalars)
}

func shouldKeepZenzaiAlternativeCandidate(candidate: Candidate, hiragana: String) -> Bool {
    guard candidate.rubyCount >= hiragana.count else {
        return false
    }

    let text = constructCandidateString(candidate: candidate, hiragana: hiragana)
        .trimmingCharacters(in: .whitespacesAndNewlines)
    guard !text.isEmpty else {
        return false
    }

    return text != hiragana && text != hiraganaToKatakana(hiragana)
}

private func displayRubyForKeyboardTypoLookup(_ ruby: String) -> String {
    var displayRuby = ruby.replacingOccurrences(of: "-", with: "ー")
    if displayRuby.last == "n" {
        displayRuby.removeLast()
        displayRuby.append("ン")
    }
    return displayRuby
}

private func hasLexicalAlternativeAcrossWholeSpan(
    candidate: Candidate,
    rawTypoSpan: String,
    originalSurfaceRange: Range<Int>? = nil
) -> Bool {
    guard !rawTypoSpan.isEmpty else {
        return false
    }

    let elements = candidate.data.map { element in
        (
            ruby: displayRubyForKeyboardTypoLookup(element.ruby),
            word: element.word,
            isKeyboardTypoCorrection: element.metadata.contains(.isKeyboardTypoCorrection)
        )
    }
    var elementStartOffsets: [Int] = []
    elementStartOffsets.reserveCapacity(elements.count)
    var offset = 0
    for element in elements {
        elementStartOffsets.append(offset)
        offset += element.ruby.count
    }

    for startIndex in elements.indices {
        let startOffset = elementStartOffsets[startIndex]
        if let originalSurfaceRange, startOffset != originalSurfaceRange.lowerBound {
            continue
        }

        var ruby = ""
        var word = ""
        for endIndex in startIndex ..< elements.endIndex {
            let element = elements[endIndex]
            if element.isKeyboardTypoCorrection {
                break
            }
            ruby += element.ruby
            word += element.word

            guard rawTypoSpan.hasPrefix(ruby) else {
                break
            }
            guard ruby == rawTypoSpan else {
                continue
            }

            let endOffset = elementStartOffsets[endIndex]
                + element.ruby.count
            if let originalSurfaceRange, endOffset != originalSurfaceRange.upperBound {
                break
            }
            return hiraganaToKatakana(word) != rawTypoSpan
        }
    }
    return false
}

func confidentKeyboardTypoCorrectionForZenzaiMerge(
    zenzaiResults: [Candidate],
    normalNBestResults: [Candidate],
    hiragana: String
) -> Candidate? {
    guard let zenzaiTop = zenzaiResults.first else {
        return nil
    }

    // Zenzai already selected a path through the bounded rewrite. Preserve its
    // semantic/surface decision (for example, こんにちは vs 今日は).
    guard zenzaiTop.keyboardTypoCorrections.isEmpty else {
        return nil
    }

    let hiraganaCharacters = Array(hiragana)
    func canPromoteRewriteCandidate(_ candidate: Candidate) -> Bool {
        guard candidate.keyboardTypoCorrections.count == 1,
              let correction = candidate.keyboardTypoCorrections.first,
              correction.originalSurfaceRange.lowerBound >= 0,
              correction.originalSurfaceRange.upperBound <= hiraganaCharacters.count
        else {
            return false
        }

        let rawTypoSpan = hiraganaToKatakana(
            String(hiraganaCharacters[correction.originalSurfaceRange])
        )
        let hasWholeSpanLexicalAlternative = hasLexicalAlternativeAcrossWholeSpan(
            candidate: zenzaiTop,
            rawTypoSpan: rawTypoSpan,
            originalSurfaceRange: correction.originalSurfaceRange
        )
        return !rawTypoSpan.isEmpty && !hasWholeSpanLexicalAlternative
    }

    if let correctedZenzaiCandidate = zenzaiResults.dropFirst().first(where: canPromoteRewriteCandidate) {
        return correctedZenzaiCandidate
    }
    if let normalTop = normalNBestResults.first,
       canPromoteRewriteCandidate(normalTop)
    {
        return normalTop
    }

    guard let normalTop = normalNBestResults.first,
          let correctionEntry = keyboardTypoDictionaryEntry(in: normalTop)
    else {
        return nil
    }

    let rawTypoSpan = displayRubyForKeyboardTypoLookup(correctionEntry.ruby)
    let hasWholeSpanLexicalAlternative = hasLexicalAlternativeAcrossWholeSpan(
        candidate: zenzaiTop,
        rawTypoSpan: rawTypoSpan
    )
    guard !rawTypoSpan.isEmpty, !hasWholeSpanLexicalAlternative else {
        return nil
    }
    return normalTop
}

func mergeZenzaiMainResultsWithNormalNBest(
    zenzaiResults: [Candidate],
    normalNBestResults: [Candidate],
    hiragana: String,
    filterZenzaiAlternatives: Bool = true
) -> [Candidate] {
    var seenTexts = Set<String>()
    var results: [Candidate] = []

    func appendIfNeeded(_ candidate: Candidate) {
        let text = constructCandidateString(candidate: candidate, hiragana: hiragana)
        guard seenTexts.insert(text).inserted else {
            return
        }
        results.append(candidate)
    }

    if let correction = confidentKeyboardTypoCorrectionForZenzaiMerge(
        zenzaiResults: zenzaiResults,
        normalNBestResults: normalNBestResults,
        hiragana: hiragana
    ) {
        appendIfNeeded(correction)
    }
    if let topCandidate = zenzaiResults.first {
        appendIfNeeded(topCandidate)
    }
    for candidate in zenzaiResults.dropFirst() {
        if filterZenzaiAlternatives && !shouldKeepZenzaiAlternativeCandidate(candidate: candidate, hiragana: hiragana) {
            continue
        }
        appendIfNeeded(candidate)
    }
    for candidate in normalNBestResults {
        appendIfNeeded(candidate)
    }

    return results
}

func cursorPrefixBoundaryFirstClauseResults(
    zenzaiFirstClauseResults: [Candidate],
    mergedFirstClauseResults: [Candidate]
) -> [Candidate] {
    zenzaiFirstClauseResults.isEmpty ? mergedFirstClauseResults : zenzaiFirstClauseResults
}

@MainActor private func requestNormalNBestSupplementCandidates(
    inputData: ComposingText,
    options: ConvertRequestOptions,
    operation: String,
    diagnosticDetails: String
) -> ConversionResult {
    var normalOptions = options
    normalOptions.zenzaiMode = .off

    let requestStart = performanceNow()
    let converted = normalNBestSupplementConverter.requestCandidates(inputData, options: normalOptions)
    let requestMs = elapsedPerformanceMilliseconds(since: requestStart)
    performanceLog(
        operation: operation,
        stage: "request_normal_nbest_supplement",
        elapsedMs: requestMs,
        details: "candidate_count=\(converted.mainResults.count);\(diagnosticDetails)"
    )
    serverLog(
        "DEBUG",
        "\(operation): normal N-best supplement returned candidateCount=\(converted.mainResults.count) \(diagnosticDetails)"
    )

    return converted
}

@_silgen_name("LoadConfig")
@MainActor public func load_config() {
    let loadedSettingsPath = settingsPath()
    var loadedSettings: AppSettings?
    var settingsLoadError: Error?
    do {
        let settings = try readAppSettings(at: loadedSettingsPath)
        loadedSettings = settings
    } catch {
        settingsLoadError = error
    }

    serverLog("INFO", "LoadConfig: start")
    let previousZenzaiEnabled = (config["enable"] as? Bool) ?? false
    let previousProfile = (config["profile"] as? String) ?? ""
    let previousBackend = (config["backend"] as? String) ?? "cpu"
    let previousExperimentalTypoCorrection =
        (config["experimentalTypoCorrection"] as? Bool) ?? false
    let previousEffectiveZenzaiEnabled = effectiveZenzaiRuntimeEnabled(
        isConfigured: previousZenzaiEnabled,
        backend: previousBackend,
        cpuBackendSupported: cpuZenzaiBackendSupportedFromEnvironment()
    )
    let previousUsedCustomRomajiTable = customRomajiTableEnabled
    let previousLearningType = currentLearningType
    let previousLearningMemoryDirectoryURL = currentLearningMemoryDirectoryURL
    var dynamicUserDictionary: [DicdataElement] = []
    defer {
        let conversionDictionary = makeConversionDictionaryEntries(
            userEntries: dynamicUserDictionary,
            experimentalTypoCorrectionEnabled:
                (config["experimentalTypoCorrection"] as? Bool) ?? false
        )
        converter.importDynamicUserDictionary(conversionDictionary)
        normalNBestSupplementConverter.importDynamicUserDictionary(conversionDictionary)
        reconversionDictionary.replaceUserEntries(dynamicUserDictionary)
    }

    config["enable"] = false
    config["profile"] = ""
    config["backend"] = "cpu"
    config["experimentalTypoCorrection"] = false
    setRoman2KanaInputStyle()
    currentLearningType = .inputAndOutput
    currentLearningMemoryDirectoryURL = learningMemoryDirectoryURL(settingsPath: loadedSettingsPath)

    if let settings = loadedSettings {
        serverLog("INFO", "LoadConfig: reading settingsPath=\(loadedSettingsPath.path)")

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
        currentLearningType = learningType(for: settings.learning?.mode)
        config["experimentalTypoCorrection"] =
            settings.general?.experimental_typo_correction ?? false

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
    let currentExperimentalTypoCorrection =
        (config["experimentalTypoCorrection"] as? Bool) ?? false
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
        || previousExperimentalTypoCorrection != currentExperimentalTypoCorrection
    {
        if backendChanged {
            rebuildConverter()
        } else {
            converter.stopComposition()
            normalNBestSupplementConverter.stopComposition()
        }
        composingText = ComposingText()
        composingTextSnapshots.removeAll()
        clearLearningCandidateCache()
    }
    if previousLearningType != currentLearningType
        || previousLearningMemoryDirectoryURL != currentLearningMemoryDirectoryURL
    {
        clearLearningCandidateCache()
    }
    ensureLearningMemoryDirectoryIfNeeded()
    loadLearningSelectionOverrides()

    serverLog(
        "INFO",
        "LoadConfig: completed enable=\(currentZenzaiEnabled) backend=\(currentBackend) effectiveEnable=\(currentEffectiveZenzaiEnabled) customRomaji=\(currentUsedCustomRomajiTable) learningType=\(currentLearningType) experimentalTypoCorrection=\(currentExperimentalTypoCorrection)"
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
    ensureConverterRuntimeDirectory()
    let reverseIndexStart = performanceNow()
    do {
        try reconversionDictionary.load(from: converterDictionaryURL)
        serverLog(
            "INFO",
            "Initialize: reconversion dictionary loaded entries=\(reconversionDictionary.recordCount) elapsed_ms=\(elapsedPerformanceMilliseconds(since: reverseIndexStart))"
        )
    } catch {
        reconversionDictionary = ReconversionDictionary()
        serverLog("ERROR", "Initialize: failed to load reconversion dictionary: \(error)")
    }
    rebuildConverter()
    clearLearningCandidateCache()

    load_config()

    let diagnosticSnapshot = zenzaiDiagnosticSnapshot()
    composingText = makeWarmupComposingText(
        zenzaiRuntimeEnabled: diagnosticSnapshot.runtimeEnabled
    )
    let useZenzaiForWarmup = effectiveZenzaiEnabledForCandidates(
        isConfigured: diagnosticSnapshot.runtimeEnabled,
        inputCount: composingText.input.count,
        hiraganaCount: composingText.convertTarget.count
    )
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
@MainActor public func warmup() -> Bool {
    let snapshot = makeBackgroundWarmupSnapshot()
    let scheduled = backgroundWarmupRunner.schedule(snapshot)
    if scheduled {
        serverLog("DEBUG", "Warmup: scheduled \(snapshot.diagnosticDetails)")
    } else {
        serverLog(
            "DEBUG",
            "Warmup: skipped reason=background_warmup_in_progress \(snapshot.diagnosticDetails)"
        )
    }
    return scheduled
}

@_silgen_name("HasActiveComposition")
@MainActor public func has_active_composition() -> Bool {
    !composingText.input.isEmpty
}

@_silgen_name("InferReconversionReadings")
@MainActor public func infer_reconversion_readings(
    surface: UnsafePointer<CChar>
) -> UnsafeMutablePointer<CChar>? {
    let surfaceString = String(cString: surface)
    let readings = reconversionDictionary.inferReadings(for: surfaceString)
    guard !readings.isEmpty else {
        serverLog("DEBUG", "InferReconversionReading: unsupported surfaceLength=\(surfaceString.count)")
        return nil
    }
    guard JSONSerialization.isValidJSONObject(readings),
          let data = try? JSONSerialization.data(withJSONObject: readings),
          let encodedReadings = String(data: data, encoding: .utf8) else {
        serverLog("ERROR", "InferReconversionReading: failed to encode readings")
        return nil
    }
    serverLog(
        "DEBUG",
        "InferReconversionReading: completed surfaceLength=\(surfaceString.count) readingCount=\(readings.count)"
    )
    return _strdup(encodedReadings)
}

@_silgen_name("AppendText")
@MainActor public func append_text(
    input: UnsafePointer<CChar>,
    cursorPtr: UnsafeMutablePointer<CInt>
) -> UnsafeMutablePointer<CChar> {
    discardPinnedLearningCandidatesBeforeCompositionEdit()
    let inputString = String(cString: input)
    serverLog("DEBUG", "AppendText: start inputLength=\(inputString.count) inputStyle=\(String(describing: currentInputStyle))")
    composingText.insertAtCursorPosition(inputString, inputStyle: currentInputStyle)

    cursorPtr.pointee = CInt(composingText.convertTargetCursorPosition)
    serverLog(
        "DEBUG",
        "AppendText: completed cursor=\(cursorPtr.pointee) hiraganaLength=\(composingText.convertTarget.count) inputCount=\(composingText.input.count)"
    )
    return _strdup(composingText.convertTarget)!
}

@_silgen_name("AppendTextDirect")
@MainActor public func append_text_direct(
    input: UnsafePointer<CChar>,
    cursorPtr: UnsafeMutablePointer<CInt>
) -> UnsafeMutablePointer<CChar> {
    discardPinnedLearningCandidatesBeforeCompositionEdit()
    let inputString = String(cString: input)
    serverLog("DEBUG", "AppendTextDirect: start inputLength=\(inputString.count)")
    composingText.insertAtCursorPosition(inputString, inputStyle: .direct)

    cursorPtr.pointee = CInt(composingText.convertTargetCursorPosition)
    serverLog(
        "DEBUG",
        "AppendTextDirect: completed cursor=\(cursorPtr.pointee) hiraganaLength=\(composingText.convertTarget.count)"
    )
    return _strdup(composingText.convertTarget)!
}

@_silgen_name("RemoveText")
@MainActor public func remove_text(
    cursorPtr: UnsafeMutablePointer<CInt>
) -> UnsafeMutablePointer<CChar> {
    discardPinnedLearningCandidatesBeforeCompositionEdit()
    serverLog("DEBUG", "RemoveText: start")
    composingText.deleteBackwardFromCursorPosition(count: 1)

    cursorPtr.pointee = CInt(composingText.convertTargetCursorPosition)
    serverLog(
        "DEBUG",
        "RemoveText: completed cursor=\(cursorPtr.pointee) hiraganaLength=\(composingText.convertTarget.count) inputCount=\(composingText.input.count)"
    )
    return _strdup(composingText.convertTarget)!
}

@_silgen_name("MoveCursor")
@MainActor public func move_cursor(
    offset: Int32,
    cursorPtr: UnsafeMutablePointer<CInt>
) -> UnsafeMutablePointer<CChar> {
    serverLog("DEBUG", "MoveCursor: start offset=\(offset)")
    let cursor = composingText.moveCursorFromCursorPosition(count: Int(offset))
    serverLog("DEBUG", "MoveCursor: offset=\(offset) cursor=\(cursor)")

    cursorPtr.pointee = CInt(cursor)
    serverLog("DEBUG", "MoveCursor: completed cursor=\(cursor)")
    return _strdup(composingText.convertTarget)!
}

@_silgen_name("GetCursorPosition")
@MainActor public func get_cursor_position() -> CInt {
    CInt(clamping: composingText.convertTargetCursorPosition)
}

@_silgen_name("GetRawInput")
@MainActor public func get_raw_input() -> UnsafeMutablePointer<CChar>? {
    let characters = composingText.input.compactMap(inputCharacter)
    guard characters.count == composingText.input.count else {
        serverLog(
            "ERROR",
            "GetRawInput: failed inputCount=\(composingText.input.count) characterCount=\(characters.count)"
        )
        return nil
    }
    return _strdup(String(characters))
}

@MainActor private func clauseAdjustmentBoundaryMap(
    composingText: ComposingText
) -> [Int: Int] {
    var boundaries = composingText.inputIndexToSurfaceIndexMap()
    let independentBoundaries = boundaries.sorted { $0.key < $1.key }
    let surfaceCharacters = Array(composingText.convertTarget)

    for (start, end) in zip(independentBoundaries, independentBoundaries.dropFirst()) {
        let inputStart = start.key
        let surfaceStart = start.value
        guard end.key > inputStart + 1,
              end.value > surfaceStart + 1,
              composingText.input.indices.contains(inputStart),
              surfaceCharacters.indices.contains(surfaceStart),
              boundaries[inputStart + 1] == nil
        else {
            continue
        }

        let firstElement = composingText.input[inputStart]
        guard firstElement.inputStyle != .direct,
              let firstInput = inputCharacter(firstElement),
              asciiLowercase(firstInput) == "n",
              surfaceCharacters[surfaceStart] == "ん"
        else {
            continue
        }

        var suffixComposingText = ComposingText()
        suffixComposingText.insertAtCursorPosition(
            Array(composingText.input[(inputStart + 1) ..< end.key])
        )
        let expectedSurfaceSuffix = String(
            surfaceCharacters[(surfaceStart + 1) ..< end.value]
        )
        guard suffixComposingText.convertTarget == expectedSurfaceSuffix else {
            continue
        }

        // A single `n` is resolved only after the following romaji segment starts,
        // so the converter's independent-segment map omits the useful boundary
        // between `ん` and that following kana (for example, `nto` -> `んと`).
        boundaries[inputStart + 1] = surfaceStart + 1
    }

    return boundaries
}

@_silgen_name("AdjustClauseBoundary")
@MainActor public func adjust_clause_boundary(
    currentInputCount: Int32,
    direction: Int32,
    expectedRawInput: UnsafePointer<CChar>? = nil,
    appliedPtr: UnsafeMutablePointer<CInt>,
    adjustedInputCountPtr: UnsafeMutablePointer<CInt>,
    cursorOffsetPtr: UnsafeMutablePointer<CInt>
) -> UnsafeMutablePointer<CChar> {
    appliedPtr.pointee = 0
    adjustedInputCountPtr.pointee = currentInputCount
    cursorOffsetPtr.pointee = 0

    guard currentInputCount >= 0, direction != 0 else {
        serverLog(
            "DEBUG",
            "AdjustClauseBoundary: skipped currentInputCount=\(currentInputCount) direction=\(direction) reason=invalid_request"
        )
        return _strdup(composingText.convertTarget)!
    }

    if let expectedRawInput {
        let expected = String(cString: expectedRawInput)
        let actualCharacters = composingText.input.compactMap(inputCharacter)
        let actual = String(actualCharacters)
        guard actualCharacters.count == composingText.input.count, actual == expected else {
            serverLog(
                "ERROR",
                "AdjustClauseBoundary: skipped currentInputCount=\(currentInputCount) direction=\(direction) reason=raw_input_mismatch expectedInputCount=\(expected.count) actualInputCount=\(composingText.input.count)"
            )
            return _strdup(composingText.convertTarget)!
        }
    }

    let surfaceIndexByInputIndex = clauseAdjustmentBoundaryMap(composingText: composingText)
    let inputBoundaries = surfaceIndexByInputIndex.keys.sorted()
    guard let currentBoundaryIndex = inputBoundaries.firstIndex(of: Int(currentInputCount)),
          let currentSurfaceIndex = surfaceIndexByInputIndex[Int(currentInputCount)]
    else {
        serverLog(
            "DEBUG",
            "AdjustClauseBoundary: skipped currentInputCount=\(currentInputCount) direction=\(direction) reason=missing_input_boundary"
        )
        return _strdup(composingText.convertTarget)!
    }

    let targetBoundaryIndex = direction < 0
        ? currentBoundaryIndex - 1
        : currentBoundaryIndex + 1
    guard inputBoundaries.indices.contains(targetBoundaryIndex) else {
        serverLog(
            "DEBUG",
            "AdjustClauseBoundary: skipped currentInputCount=\(currentInputCount) direction=\(direction) reason=edge"
        )
        return _strdup(composingText.convertTarget)!
    }

    let adjustedInputCount = inputBoundaries[targetBoundaryIndex]
    guard adjustedInputCount > 0 else {
        serverLog(
            "DEBUG",
            "AdjustClauseBoundary: skipped currentInputCount=\(currentInputCount) direction=\(direction) reason=empty_current_clause"
        )
        return _strdup(composingText.convertTarget)!
    }
    guard let targetSurfaceIndex = surfaceIndexByInputIndex[adjustedInputCount] else {
        serverLog(
            "DEBUG",
            "AdjustClauseBoundary: skipped currentInputCount=\(currentInputCount) direction=\(direction) reason=missing_surface_boundary"
        )
        return _strdup(composingText.convertTarget)!
    }

    let requestedCursorOffset = targetSurfaceIndex - composingText.convertTargetCursorPosition
    let cursorOffset = composingText.moveCursorFromCursorPosition(count: requestedCursorOffset)
    guard composingText.convertTargetCursorPosition == targetSurfaceIndex else {
        let rollbackOffset = currentSurfaceIndex - composingText.convertTargetCursorPosition
        _ = composingText.moveCursorFromCursorPosition(count: rollbackOffset)
        serverLog(
            "ERROR",
            "AdjustClauseBoundary: skipped currentInputCount=\(currentInputCount) direction=\(direction) reason=cursor_clamped"
        )
        return _strdup(composingText.convertTarget)!
    }

    appliedPtr.pointee = 1
    adjustedInputCountPtr.pointee = CInt(adjustedInputCount)
    cursorOffsetPtr.pointee = CInt(cursorOffset)
    serverLog(
        "DEBUG",
        "AdjustClauseBoundary: applied currentInputCount=\(currentInputCount) adjustedInputCount=\(adjustedInputCount) direction=\(direction) currentSurfaceIndex=\(currentSurfaceIndex) targetSurfaceIndex=\(targetSurfaceIndex) cursorOffset=\(cursorOffset)"
    )
    return _strdup(composingText.convertTarget)!
}

@_silgen_name("ClearComposingTextSnapshots")
@MainActor public func clear_composing_text_snapshots() {
    composingTextSnapshots.removeAll()
    learningCandidateCache.removeAllPins()
    serverLog("DEBUG", "ClearComposingTextSnapshots: completed")
}

@_silgen_name("PushComposingTextSnapshot")
@MainActor public func push_composing_text_snapshot(selectedCandidateId: UInt64) {
    if selectedCandidateId != 0 && !pin_learning_candidate(candidateId: selectedCandidateId) {
        serverLog(
            "DEBUG",
            "PushComposingTextSnapshot: learning candidate unavailable id=\(selectedCandidateId)"
        )
    }
    composingTextSnapshots.append(composingText)
    serverLog(
        "DEBUG",
        "PushComposingTextSnapshot: completed count=\(composingTextSnapshots.count) protectedLearningCandidateSlots=\(learningCandidateCache.protectedSlotCount)"
    )
}

@_silgen_name("PopComposingTextSnapshot")
@MainActor public func pop_composing_text_snapshot(selectedCandidateId: UInt64) {
    if selectedCandidateId != 0 && !pin_learning_candidate(candidateId: selectedCandidateId) {
        serverLog(
            "DEBUG",
            "PopComposingTextSnapshot: learning candidate unavailable id=\(selectedCandidateId)"
        )
    }
    if let restored = composingTextSnapshots.popLast() {
        composingText = restored
    }
    serverLog(
        "DEBUG",
        "PopComposingTextSnapshot: completed remaining=\(composingTextSnapshots.count)"
    )
}

@_silgen_name("ClearText")
@MainActor public func clear_text() {
    serverLog("DEBUG", "ClearText: start")
    converter.stopComposition()
    normalNBestSupplementConverter.stopComposition()
    composingText = ComposingText()
    composingTextSnapshots.removeAll()
    clearLearningCandidateCache()
    serverLog("DEBUG", "ClearText: completed")
}

@_silgen_name("CommitLearningCandidate")
@MainActor public func commit_learning_candidate(
    candidateId: UInt64,
    commitKind: Int32
) -> Bool {
    guard currentLearningType == .inputAndOutput else {
        serverLog(
            "DEBUG",
            "CommitLearningCandidate: skipped learningType=\(currentLearningType) candidateId=\(candidateId) commitKind=\(commitKind)"
        )
        return true
    }

    guard let candidate = consumeLearningCandidate(candidateId) else {
        serverLog(
            "WARN",
            "CommitLearningCandidate: candidate not found candidateId=\(candidateId) commitKind=\(commitKind)"
        )
        return false
    }

    ensureLearningMemoryDirectoryIfNeeded()
    converter.setCompletedData(candidate)
    converter.updateLearningData(candidate)
    converter.commitUpdateLearningData()
    recordLearningSelectionOverride(candidate)
    serverLog(
        "DEBUG",
        "CommitLearningCandidate: completed candidateId=\(candidateId) commitKind=\(commitKind)"
    )
    return true
}

@_silgen_name("CommitLearningCandidates")
@MainActor public func commit_learning_candidates(
    candidateIds: UnsafePointer<UInt64>?,
    commitKinds: UnsafePointer<Int32>?,
    count: Int32
) -> Int32 {
    guard count >= 0, let candidateIds, let commitKinds else {
        serverLog("WARN", "CommitLearningCandidates: invalid batch count=\(count)")
        return 0
    }
    guard count > 0 else {
        return 0
    }
    guard currentLearningType == .inputAndOutput else {
        serverLog(
            "DEBUG",
            "CommitLearningCandidates: skipped learningType=\(currentLearningType) count=\(count)"
        )
        return count
    }

    var candidates: [(candidate: Candidate, candidateId: UInt64, commitKind: Int32)] = []
    candidates.reserveCapacity(Int(count))
    for index in 0..<Int(count) {
        let candidateId = candidateIds[index]
        let commitKind = commitKinds[index]
        guard let candidate = consumeLearningCandidate(candidateId) else {
            serverLog(
                "WARN",
                "CommitLearningCandidates: candidate not found candidateId=\(candidateId) commitKind=\(commitKind)"
            )
            continue
        }
        candidates.append((candidate, candidateId, commitKind))
    }
    guard !candidates.isEmpty else {
        return 0
    }

    ensureLearningMemoryDirectoryIfNeeded()
    var selectionOverrideChanged = false
    for entry in candidates {
        converter.setCompletedData(entry.candidate)
        converter.updateLearningData(entry.candidate)
        selectionOverrideChanged =
            updateLearningSelectionOverride(entry.candidate) || selectionOverrideChanged
    }
    converter.commitUpdateLearningData()
    if selectionOverrideChanged {
        saveLearningSelectionOverrides()
    }
    serverLog(
        "DEBUG",
        "CommitLearningCandidates: completed requestedCount=\(count) committedCount=\(candidates.count)"
    )
    return Int32(candidates.count)
}

@_silgen_name("ResetLearningMemory")
@MainActor public func reset_learning_memory() -> Bool {
    let resetDirectory = resetLearningMemoryDirectory()
    converter.resetMemory()
    normalNBestSupplementConverter.resetMemory()
    clearLearningCandidateCache()
    learningSelectionOverrides.removeAll(keepingCapacity: false)
    serverLog("INFO", "ResetLearningMemory: completed resetDirectory=\(resetDirectory)")
    return resetDirectory
}

func to_list_pointer(_ list: [FFICandidate]) -> UnsafeMutablePointer<UnsafeMutablePointer<FFICandidate>?> {
    let pointer = UnsafeMutablePointer<UnsafeMutablePointer<FFICandidate>?>.allocate(capacity: list.count)
    guard !list.isEmpty else {
        return pointer
    }

    let candidateStorage = UnsafeMutablePointer<FFICandidate>.allocate(capacity: list.count)
    candidateStorage.initialize(from: list, count: list.count)
    for i in 0..<list.count {
        pointer.advanced(by: i).initialize(to: candidateStorage.advanced(by: i))
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

    let count = Int(length)
    guard count > 0 else {
        ptr.deallocate()
        return
    }

    let candidateStorage = ptr[0]
    let isContiguousCandidateStorage = candidateStorage.map { storage in
        (0..<count).allSatisfy { index in
            ptr[index] == storage.advanced(by: index)
        }
    } ?? false
    for index in 0..<count {
        guard let candidatePtr = ptr[index] else {
            continue
        }

        let candidate = candidatePtr.pointee
        free(candidate.text)
        free(candidate.subtext)
        free(candidate.hiragana)
    }

    if isContiguousCandidateStorage, let candidateStorage {
        candidateStorage.deinitialize(count: count)
        candidateStorage.deallocate()
    } else {
        for index in 0..<count {
            guard let candidatePtr = ptr[index] else {
                continue
            }
            candidatePtr.deinitialize(count: 1)
            candidatePtr.deallocate()
        }
    }

    ptr.deinitialize(count: count)
    ptr.deallocate()
}

@_silgen_name("GetComposedText")
@MainActor public func get_composed_text(lengthPtr: UnsafeMutablePointer<CInt>) -> UnsafeMutablePointer<UnsafeMutablePointer<FFICandidate>?> {
    get_composed_text_impl(lengthPtr: lengthPtr, allowZenzai: true)
}

@_silgen_name("GetComposedTextForReconversion")
@MainActor public func get_composed_text_for_reconversion(lengthPtr: UnsafeMutablePointer<CInt>) -> UnsafeMutablePointer<UnsafeMutablePointer<FFICandidate>?> {
    get_composed_text_impl(lengthPtr: lengthPtr, allowZenzai: false)
}

@MainActor private func get_composed_text_impl(
    lengthPtr: UnsafeMutablePointer<CInt>,
    allowZenzai: Bool
) -> UnsafeMutablePointer<UnsafeMutablePointer<FFICandidate>?> {
    let functionStart = performanceNow()
    let performanceEnabled = serverLogCallbacks.isPerformanceLogEnabled()
    let originalHiragana = composingText.convertTarget
    let contextString = (config["context"] as? String) ?? ""
    let diagnosticSnapshot = zenzaiDiagnosticSnapshot()
    let runtimeZenzaiEnabled = diagnosticSnapshot.runtimeEnabled && allowZenzai
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
    candidateCrashTrace(
        useZenzai: useZenzai,
        operation: "GetComposedText",
        stage: "requestCandidates",
        state: "begin",
        details: diagnosticDetails
    )
    serverLog("DEBUG", "GetComposedText: requestCandidates begin \(diagnosticDetails)")
    if performanceEnabled {
        performanceLog(
            operation: "get_composed_text",
            stage: "prepare_request",
            elapsedMs: elapsedPerformanceMilliseconds(since: functionStart),
            details: diagnosticDetails
        )
    }
    let normalNBestConverted: ConversionResult?
    if useZenzai {
        normalNBestConverted = requestNormalNBestSupplementCandidates(
            inputData: previewComposingText,
            options: options,
            operation: "get_composed_text",
            diagnosticDetails: diagnosticDetails
        )
    } else {
        normalNBestConverted = nil
    }
    let requestStart = performanceNow()
    let converted = converter.requestCandidates(previewComposingText, options: options)
    let requestMs = elapsedPerformanceMilliseconds(since: requestStart)
    performanceLog(
        operation: "get_composed_text",
        stage: "request_candidates",
        elapsedMs: requestMs,
        details: "candidate_count=\(converted.mainResults.count);\(diagnosticDetails)"
    )
    candidateCrashTrace(
        useZenzai: useZenzai,
        operation: "GetComposedText",
        stage: "requestCandidates",
        state: "completed",
        details: "candidate_count=\(converted.mainResults.count);\(diagnosticDetails)"
    )
    serverLog("DEBUG", "GetComposedText: requestCandidates returned candidateCount=\(converted.mainResults.count) \(diagnosticDetails)")
    let mergedMainResults = normalNBestConverted.map {
        mergeZenzaiMainResultsWithNormalNBest(
            zenzaiResults: converted.mainResults,
            normalNBestResults: $0.mainResults,
            hiragana: previewHiragana
        )
    } ?? converted.mainResults
    let mainResults = prioritizeLearningSelectionOverrides(
        mergedMainResults,
        ruby: previewHiragana
    ) { $0 }
    if let normalNBestConverted {
        serverLog(
            "DEBUG",
            "GetComposedText: merged Zenzai candidates candidateCount=\(mainResults.count) zenzaiCandidateCount=\(converted.mainResults.count) normalNBestCandidateCount=\(normalNBestConverted.mainResults.count) \(diagnosticDetails)"
        )
    }
    candidateCrashTrace(
        useZenzai: useZenzai,
        operation: "GetComposedText",
        stage: "postprocessCandidates",
        state: "begin",
        details: "candidate_count=\(mainResults.count);zenzai_candidate_count=\(converted.mainResults.count);\(diagnosticDetails)"
    )
    let buildStart = performanceEnabled ? performanceNow() : 0
    var constructCandidateStringMs = 0
    var resolveCandidateCompositionMs = 0
    var strdupCandidatesMs = 0
    var resolutionCache: [String: CandidateDisplayResolution] = [:]
    var result: [FFICandidate] = []
    result.reserveCapacity(mainResults.count)
    let learningCandidateBatchFirstId = cacheLearningCandidates(mainResults)

    for i in 0..<mainResults.count {
        let candidate = mainResults[i]

        let constructStart = performanceEnabled ? performanceNow() : 0
        let candidateText = constructCandidateString(candidate: candidate, hiragana: previewHiragana)
        if performanceEnabled {
            constructCandidateStringMs += elapsedPerformanceMilliseconds(since: constructStart)
        }

        let resolveStart = performanceEnabled ? performanceNow() : 0
        let resolvedCandidate = resolveCandidateCompositionForDisplay(
            originalComposingText: composingText,
            previewComposingText: previewComposingText,
            candidateComposingCount: candidate.composingCount,
            resolutionCache: &resolutionCache
        )
        if performanceEnabled {
            resolveCandidateCompositionMs += elapsedPerformanceMilliseconds(since: resolveStart)
        }
        let correspondingCount = resolvedCandidate.correspondingCount

        let strdupStart = performanceEnabled ? performanceNow() : 0
        let text = _strdup(candidateText)
        let subtext = _strdup(resolvedCandidate.remainingConvertTarget)
        let hiragana = i == 0 ? _strdup(previewHiragana) : nil
        if performanceEnabled {
            strdupCandidatesMs += elapsedPerformanceMilliseconds(since: strdupStart)
        }

        result.append(
            FFICandidate(
                text: text,
                subtext: subtext,
                hiragana: hiragana,
                correspondingCount: Int32(correspondingCount),
                candidateId: learningCandidateId(at: i, batchFirstId: learningCandidateBatchFirstId)
            )
        )
    }

    lengthPtr.pointee = CInt(result.count)
    let listPointer = to_list_pointer(result)
    if performanceEnabled {
        let stringAllocationCount = result.isEmpty ? 0 : result.count * 2 + 1
        performanceLog(
            operation: "get_composed_text",
            stage: "construct_candidate_string",
            elapsedMs: constructCandidateStringMs,
            details: "candidate_count=\(result.count);main_candidate_count=\(mainResults.count);zenzai_candidate_count=\(converted.mainResults.count);normal_nbest_candidate_count=\(normalNBestConverted?.mainResults.count ?? 0);\(diagnosticDetails)"
        )
        performanceLog(
            operation: "get_composed_text",
            stage: "resolve_candidate_composition",
            elapsedMs: resolveCandidateCompositionMs,
            details: "candidate_count=\(result.count);cache_entries=\(resolutionCache.count);\(diagnosticDetails)"
        )
        performanceLog(
            operation: "get_composed_text",
            stage: "strdup_candidates",
            elapsedMs: strdupCandidatesMs,
            details: "candidate_count=\(result.count);string_allocations=\(stringAllocationCount);\(diagnosticDetails)"
        )
        performanceLog(
            operation: "get_composed_text",
            stage: "build_ffi_candidates_total",
            elapsedMs: elapsedPerformanceMilliseconds(since: buildStart),
            details: "candidate_count=\(result.count);main_candidate_count=\(mainResults.count);zenzai_candidate_count=\(converted.mainResults.count);normal_nbest_candidate_count=\(normalNBestConverted?.mainResults.count ?? 0);cache_entries=\(resolutionCache.count);string_allocations=\(stringAllocationCount);learning_cache_batches=\(learningCandidateCache.batchCount);learning_cache_slots=\(learningCandidateCache.slotCount);learning_cache_protected_slots=\(learningCandidateCache.protectedSlotCount);\(diagnosticDetails)"
        )
    }
    candidateCrashTrace(
        useZenzai: useZenzai,
        operation: "GetComposedText",
        stage: "postprocessCandidates",
        state: "completed",
        details: "candidate_count=\(result.count);main_candidate_count=\(mainResults.count);zenzai_candidate_count=\(converted.mainResults.count);normal_nbest_candidate_count=\(normalNBestConverted?.mainResults.count ?? 0);\(diagnosticDetails)"
    )
    serverLog("DEBUG", "GetComposedText: postprocessCandidates completed candidateCount=\(result.count) mainCandidateCount=\(mainResults.count) zenzaiCandidateCount=\(converted.mainResults.count) normalNBestCandidateCount=\(normalNBestConverted?.mainResults.count ?? 0) \(diagnosticDetails)")
    serverLog("DEBUG", "GetComposedText: completed candidateCount=\(result.count) \(diagnosticDetails)")

    return listPointer
}

@_silgen_name("GetComposedTextForCursorPrefix")
@MainActor public func get_composed_text_for_cursor_prefix(
    requiredInputCount: Int32,
    lengthPtr: UnsafeMutablePointer<CInt>
) -> UnsafeMutablePointer<UnsafeMutablePointer<FFICandidate>?> {
    let functionStart = performanceNow()
    let performanceEnabled = serverLogCallbacks.isPerformanceLogEnabled()
    let hiragana = composingText.convertTarget
    let suffixAfterCursor = String(hiragana.dropFirst(composingText.convertTargetCursorPosition))
    let prefixComposingText = composingText.prefixToCursorPosition()
    let requiredBoundary = requiredInputCount >= 0 ? Int(requiredInputCount) : nil
    if let requiredBoundary, requiredBoundary != prefixComposingText.input.count {
        lengthPtr.pointee = 0
        serverLog(
            "ERROR",
            "GetComposedTextForCursorPrefix: skipped reason=required_boundary_mismatch requiredInputCount=\(requiredBoundary) actualInputCount=\(prefixComposingText.input.count)"
        )
        return to_list_pointer([])
    }
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
    candidateCrashTrace(
        useZenzai: useZenzai,
        operation: "GetComposedTextForCursorPrefix",
        stage: "requestCandidates",
        state: "begin",
        details: "suffix_len=\(suffixAfterCursor.count);\(diagnosticDetails)"
    )
    serverLog("DEBUG", "GetComposedTextForCursorPrefix: requestCandidates begin suffix_len=\(suffixAfterCursor.count);\(diagnosticDetails)")
    if performanceEnabled {
        performanceLog(
            operation: "get_composed_text_for_cursor_prefix",
            stage: "prepare_request",
            elapsedMs: elapsedPerformanceMilliseconds(since: functionStart),
            details: "suffix_len=\(suffixAfterCursor.count);\(diagnosticDetails)"
        )
    }
    let totalStart = performanceNow()
    let normalNBestConverted: ConversionResult?
    if useZenzai {
        normalNBestConverted = requestNormalNBestSupplementCandidates(
            inputData: previewPrefixComposingText,
            options: options,
            operation: "get_composed_text_for_cursor_prefix",
            diagnosticDetails: "suffix_len=\(suffixAfterCursor.count);\(diagnosticDetails)"
        )
    } else {
        normalNBestConverted = nil
    }
    let requestStart = performanceNow()
    let converted = converter.requestCandidates(previewPrefixComposingText, options: options)
    let requestMs = elapsedPerformanceMilliseconds(since: requestStart)
    performanceLog(
        operation: "get_composed_text_for_cursor_prefix",
        stage: "request_candidates",
        elapsedMs: requestMs,
        details: "first_clause_candidate_count=\(converted.firstClauseResults.count);main_candidate_count=\(converted.mainResults.count);suffix_len=\(suffixAfterCursor.count);\(diagnosticDetails)"
    )
    candidateCrashTrace(
        useZenzai: useZenzai,
        operation: "GetComposedTextForCursorPrefix",
        stage: "requestCandidates",
        state: "completed",
        details: "first_clause_candidate_count=\(converted.firstClauseResults.count);main_candidate_count=\(converted.mainResults.count);suffix_len=\(suffixAfterCursor.count);\(diagnosticDetails)"
    )
    serverLog("DEBUG", "GetComposedTextForCursorPrefix: requestCandidates returned firstClauseCandidateCount=\(converted.firstClauseResults.count) mainCandidateCount=\(converted.mainResults.count) suffix_len=\(suffixAfterCursor.count);\(diagnosticDetails)")
    let cursorPrefixMainResults = normalNBestConverted.map {
        mergeZenzaiMainResultsWithNormalNBest(
            zenzaiResults: converted.mainResults,
            normalNBestResults: $0.mainResults,
            hiragana: previewPrefixHiragana
        )
    } ?? converted.mainResults
    let cursorPrefixFirstClauseResults = normalNBestConverted.map {
        mergeZenzaiMainResultsWithNormalNBest(
            zenzaiResults: converted.firstClauseResults,
            normalNBestResults: $0.firstClauseResults,
            hiragana: previewPrefixHiragana,
            filterZenzaiAlternatives: false
        )
    } ?? converted.firstClauseResults
    if let normalNBestConverted {
        serverLog(
            "DEBUG",
            "GetComposedTextForCursorPrefix: merged Zenzai candidates firstClauseCandidateCount=\(cursorPrefixFirstClauseResults.count) mainCandidateCount=\(cursorPrefixMainResults.count) zenzaiFirstClauseCandidateCount=\(converted.firstClauseResults.count) zenzaiMainCandidateCount=\(converted.mainResults.count) normalNBestFirstClauseCandidateCount=\(normalNBestConverted.firstClauseResults.count) normalNBestMainCandidateCount=\(normalNBestConverted.mainResults.count) suffix_len=\(suffixAfterCursor.count);\(diagnosticDetails)"
        )
    }
    candidateCrashTrace(
        useZenzai: useZenzai,
        operation: "GetComposedTextForCursorPrefix",
        stage: "postprocessCandidates",
        state: "begin",
        details: "phase=first_clause;first_clause_candidate_count=\(cursorPrefixFirstClauseResults.count);main_candidate_count=\(cursorPrefixMainResults.count);zenzai_first_clause_candidate_count=\(converted.firstClauseResults.count);zenzai_main_candidate_count=\(converted.mainResults.count);normal_nbest_first_clause_candidate_count=\(normalNBestConverted?.firstClauseResults.count ?? 0);normal_nbest_main_candidate_count=\(normalNBestConverted?.mainResults.count ?? 0);suffix_len=\(suffixAfterCursor.count);\(diagnosticDetails)"
    )
    var cursorPrefixResolutionCache: [String: CandidateDisplayResolution] = [:]
    let boundaryFirstClauseResults = cursorPrefixBoundaryFirstClauseResults(
        zenzaiFirstClauseResults: converted.firstClauseResults,
        mergedFirstClauseResults: cursorPrefixFirstClauseResults
    )
    let firstClauseCorrespondingCount = requiredBoundary
        ?? cursorPrefixFirstClauseCorrespondingCount(
            firstClauseResults: boundaryFirstClauseResults,
            originalComposingText: prefixComposingText,
            previewComposingText: previewPrefixComposingText,
            resolutionCache: &cursorPrefixResolutionCache
        )
    let preliminaryCursorPrefixResults = cursorPrefixCandidateDisplayResults(
        mainResults: cursorPrefixMainResults,
        firstClauseResults: cursorPrefixFirstClauseResults,
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
        candidateCrashTrace(
            useZenzai: useZenzai,
            operation: "GetComposedTextForCursorPrefix",
            stage: "requestCandidatesExactClause",
            state: "begin",
            details: "corresponding_count=\(firstClauseCorrespondingCount);\(exactClauseDiagnosticDetails)"
        )
        serverLog(
            "DEBUG",
            "GetComposedTextForCursorPrefix: requestCandidates exactClause begin correspondingCount=\(firstClauseCorrespondingCount) \(exactClauseDiagnosticDetails)"
        )
        let exactClauseNormalNBestConverted: ConversionResult?
        if useZenzai {
            exactClauseNormalNBestConverted = requestNormalNBestSupplementCandidates(
                inputData: exactClausePreviewState.composingText,
                options: options,
                operation: "get_composed_text_for_cursor_prefix_exact_clause",
                diagnosticDetails: "corresponding_count=\(firstClauseCorrespondingCount);\(exactClauseDiagnosticDetails)"
            )
        } else {
            exactClauseNormalNBestConverted = nil
        }
        let exactClauseRequestStart = performanceNow()
        let exactClauseConverted = converter.requestCandidates(
            exactClausePreviewState.composingText,
            options: options
        )
        exactClauseResults = exactClauseNormalNBestConverted.map {
            mergeZenzaiMainResultsWithNormalNBest(
                zenzaiResults: exactClauseConverted.mainResults,
                normalNBestResults: $0.mainResults,
                hiragana: exactClausePreviewState.composingText.convertTarget
            )
        } ?? exactClauseConverted.mainResults
        if let exactClauseNormalNBestConverted {
            serverLog(
                "DEBUG",
                "GetComposedTextForCursorPrefix: merged exactClause Zenzai candidates candidateCount=\(exactClauseResults.count) zenzaiCandidateCount=\(exactClauseConverted.mainResults.count) normalNBestCandidateCount=\(exactClauseNormalNBestConverted.mainResults.count) correspondingCount=\(firstClauseCorrespondingCount) \(exactClauseDiagnosticDetails)"
            )
        }
        let exactClauseRequestMs = elapsedPerformanceMilliseconds(since: exactClauseRequestStart)
        performanceLog(
            operation: "get_composed_text_for_cursor_prefix",
            stage: "request_candidates_exact_clause",
            elapsedMs: exactClauseRequestMs,
            details: "candidate_count=\(exactClauseResults.count);zenzai_candidate_count=\(exactClauseConverted.mainResults.count);normal_nbest_candidate_count=\(exactClauseNormalNBestConverted?.mainResults.count ?? 0);corresponding_count=\(firstClauseCorrespondingCount);\(exactClauseDiagnosticDetails)"
        )
        candidateCrashTrace(
            useZenzai: useZenzai,
            operation: "GetComposedTextForCursorPrefix",
            stage: "requestCandidatesExactClause",
            state: "completed",
            details: "candidate_count=\(exactClauseResults.count);zenzai_candidate_count=\(exactClauseConverted.mainResults.count);normal_nbest_candidate_count=\(exactClauseNormalNBestConverted?.mainResults.count ?? 0);corresponding_count=\(firstClauseCorrespondingCount);\(exactClauseDiagnosticDetails)"
        )
        serverLog(
            "DEBUG",
            "GetComposedTextForCursorPrefix: requestCandidates exactClause returned candidateCount=\(exactClauseResults.count) zenzaiCandidateCount=\(exactClauseConverted.mainResults.count) normalNBestCandidateCount=\(exactClauseNormalNBestConverted?.mainResults.count ?? 0) correspondingCount=\(firstClauseCorrespondingCount) \(exactClauseDiagnosticDetails)"
        )
    }
    candidateCrashTrace(
        useZenzai: useZenzai,
        operation: "GetComposedTextForCursorPrefix",
        stage: "postprocessCandidates",
        state: "begin",
        details: "phase=merge;preliminary_candidate_count=\(preliminaryCursorPrefixResults.count);exact_clause_candidate_count=\(exactClauseResults.count);suffix_len=\(suffixAfterCursor.count);\(diagnosticDetails)"
    )
    let rawCursorPrefixResults = exactClauseResults.isEmpty
        ? preliminaryCursorPrefixResults
        : cursorPrefixCandidateDisplayResults(
            mainResults: cursorPrefixMainResults,
            firstClauseResults: cursorPrefixFirstClauseResults,
            exactClauseResults: exactClauseResults,
            firstClauseCorrespondingCount: firstClauseCorrespondingCount,
            originalComposingText: prefixComposingText,
            previewComposingText: previewPrefixComposingText,
            previewHiragana: previewPrefixHiragana,
            resolutionCache: &cursorPrefixResolutionCache
        )
    let cursorPrefixResults = prioritizeLearningSelectionOverrides(
        rawCursorPrefixResults,
        ruby: previewPrefixHiragana
    ) { $0.candidate }
    let totalMs = elapsedPerformanceMilliseconds(since: totalStart)
    performanceLog(
        operation: "get_composed_text_for_cursor_prefix",
        stage: "total_before_ffi_candidates",
        elapsedMs: totalMs,
        details: "candidate_count=\(cursorPrefixResults.count);first_clause_candidate_count=\(cursorPrefixFirstClauseResults.count);main_candidate_count=\(cursorPrefixMainResults.count);zenzai_first_clause_candidate_count=\(converted.firstClauseResults.count);zenzai_main_candidate_count=\(converted.mainResults.count);normal_nbest_first_clause_candidate_count=\(normalNBestConverted?.firstClauseResults.count ?? 0);normal_nbest_main_candidate_count=\(normalNBestConverted?.mainResults.count ?? 0);exact_clause_candidate_count=\(exactClauseResults.count);suffix_len=\(suffixAfterCursor.count);\(diagnosticDetails)"
    )
    let buildStart = performanceEnabled ? performanceNow() : 0
    var resolveCandidateCompositionMs = 0
    var strdupCandidatesMs = 0
    var result: [FFICandidate] = []
    result.reserveCapacity(cursorPrefixResults.count)
    let learningCandidateBatchFirstId = cacheLearningCandidates(
        cursorPrefixResults.map(\.candidate)
    )
    let ffiHiragana = previewPrefixHiragana + suffixAfterCursor

    for i in 0..<cursorPrefixResults.count {
        let cursorPrefixResult = cursorPrefixResults[i]
        let candidate = cursorPrefixResult.candidate

        let resolveStart = performanceEnabled ? performanceNow() : 0
        let resolvedCandidate = resolveCandidateCompositionForDisplay(
            originalComposingText: prefixComposingText,
            previewComposingText: previewPrefixComposingText,
            candidateComposingCount: candidate.composingCount,
            resolutionCache: &cursorPrefixResolutionCache
        )
        if performanceEnabled {
            resolveCandidateCompositionMs += elapsedPerformanceMilliseconds(since: resolveStart)
        }
        let correspondingCount = resolvedCandidate.correspondingCount

        let strdupStart = performanceEnabled ? performanceNow() : 0
        let text = _strdup(cursorPrefixResult.displayText)
        let subtext = _strdup(resolvedCandidate.remainingConvertTarget + suffixAfterCursor)
        let hiragana = i == 0 ? _strdup(ffiHiragana) : nil
        if performanceEnabled {
            strdupCandidatesMs += elapsedPerformanceMilliseconds(since: strdupStart)
        }

        result.append(
            FFICandidate(
                text: text,
                subtext: subtext,
                hiragana: hiragana,
                correspondingCount: Int32(correspondingCount),
                candidateId: learningCandidateId(at: i, batchFirstId: learningCandidateBatchFirstId)
            )
        )
    }

    lengthPtr.pointee = CInt(result.count)
    let listPointer = to_list_pointer(result)
    if performanceEnabled {
        let stringAllocationCount = result.isEmpty ? 0 : result.count * 2 + 1
        performanceLog(
            operation: "get_composed_text_for_cursor_prefix",
            stage: "resolve_candidate_composition",
            elapsedMs: resolveCandidateCompositionMs,
            details: "candidate_count=\(result.count);cache_entries=\(cursorPrefixResolutionCache.count);suffix_len=\(suffixAfterCursor.count);\(diagnosticDetails)"
        )
        performanceLog(
            operation: "get_composed_text_for_cursor_prefix",
            stage: "strdup_candidates",
            elapsedMs: strdupCandidatesMs,
            details: "candidate_count=\(result.count);string_allocations=\(stringAllocationCount);suffix_len=\(suffixAfterCursor.count);\(diagnosticDetails)"
        )
        performanceLog(
            operation: "get_composed_text_for_cursor_prefix",
            stage: "build_ffi_candidates_total",
            elapsedMs: elapsedPerformanceMilliseconds(since: buildStart),
            details: "candidate_count=\(result.count);first_clause_candidate_count=\(cursorPrefixFirstClauseResults.count);main_candidate_count=\(cursorPrefixMainResults.count);zenzai_first_clause_candidate_count=\(converted.firstClauseResults.count);zenzai_main_candidate_count=\(converted.mainResults.count);normal_nbest_first_clause_candidate_count=\(normalNBestConverted?.firstClauseResults.count ?? 0);normal_nbest_main_candidate_count=\(normalNBestConverted?.mainResults.count ?? 0);exact_clause_candidate_count=\(exactClauseResults.count);cache_entries=\(cursorPrefixResolutionCache.count);string_allocations=\(stringAllocationCount);learning_cache_batches=\(learningCandidateCache.batchCount);learning_cache_slots=\(learningCandidateCache.slotCount);learning_cache_protected_slots=\(learningCandidateCache.protectedSlotCount);suffix_len=\(suffixAfterCursor.count);\(diagnosticDetails)"
        )
    }
    candidateCrashTrace(
        useZenzai: useZenzai,
        operation: "GetComposedTextForCursorPrefix",
        stage: "postprocessCandidates",
        state: "completed",
        details: "candidate_count=\(result.count);first_clause_candidate_count=\(cursorPrefixFirstClauseResults.count);main_candidate_count=\(cursorPrefixMainResults.count);zenzai_first_clause_candidate_count=\(converted.firstClauseResults.count);zenzai_main_candidate_count=\(converted.mainResults.count);normal_nbest_first_clause_candidate_count=\(normalNBestConverted?.firstClauseResults.count ?? 0);normal_nbest_main_candidate_count=\(normalNBestConverted?.mainResults.count ?? 0);exact_clause_candidate_count=\(exactClauseResults.count);suffix_len=\(suffixAfterCursor.count);\(diagnosticDetails)"
    )
    serverLog("DEBUG", "GetComposedTextForCursorPrefix: postprocessCandidates completed candidateCount=\(result.count) firstClauseCandidateCount=\(cursorPrefixFirstClauseResults.count) mainCandidateCount=\(cursorPrefixMainResults.count) zenzaiFirstClauseCandidateCount=\(converted.firstClauseResults.count) zenzaiMainCandidateCount=\(converted.mainResults.count) normalNBestFirstClauseCandidateCount=\(normalNBestConverted?.firstClauseResults.count ?? 0) normalNBestMainCandidateCount=\(normalNBestConverted?.mainResults.count ?? 0) exactClauseCandidateCount=\(exactClauseResults.count) suffix_len=\(suffixAfterCursor.count);\(diagnosticDetails)")
    serverLog("DEBUG", "GetComposedTextForCursorPrefix: completed candidateCount=\(result.count) suffix_len=\(suffixAfterCursor.count);\(diagnosticDetails)")

    return listPointer
}

@_silgen_name("ShrinkText")
@MainActor public func shrink_text(
    offset: Int32
) -> UnsafeMutablePointer<CChar>  {
    discardPinnedLearningCandidatesBeforeCompositionEdit()
    serverLog("DEBUG", "ShrinkText: start offset=\(offset)")
    var afterComposingText = composingText
    let boundedOffset = min(max(Int(offset), 0), afterComposingText.input.count)
    afterComposingText.prefixComplete(composingCount: .inputCount(boundedOffset))
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
