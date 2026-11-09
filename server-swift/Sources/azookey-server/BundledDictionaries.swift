import Foundation
import KanaKanjiConverterModule

struct BundledDictionaryMode: Equatable, Sendable {
    let english: Bool
    let katakana: Bool
    static let both = Self(english: true, katakana: true)

    var variant: String? {
        switch (english, katakana) {
        case (true, true): nil
        case (false, false): "base"
        case (false, true): "kana"
        case (true, false): "spelling"
        }
    }

    func directory(in base: URL) -> URL {
        variant.map { base.appending(path: "Variants/\($0)") } ?? base
    }
}

/// Created on an exclusive preparation worker, then transferred to the input
/// owner only after that worker has finished. Never accessed concurrently.
private final class DictionaryResources: @unchecked Sendable {
    let mode: BundledDictionaryMode
    let directory: URL
    let main: KanaKanjiConverter
    let supplement: KanaKanjiConverter
    let reverse: ReconversionDictionary

    init(mode: BundledDictionaryMode, base: URL, preload: Bool,
         userEntries: [DicdataElement], conversionEntries: [DicdataElement]) throws {
        let directory = mode.directory(in: base)
        struct Manifest: Decodable { let format: Int; let kana: Bool; let spelling: Bool; let losslessRoundtripVerified: Bool }
        let manifest = try JSONDecoder().decode(Manifest.self,
            from: Data(contentsOf: directory.appending(path: "english-manifest.json")))
        guard manifest.format == 2, manifest.kana == mode.katakana, manifest.spelling == mode.english,
              manifest.losslessRoundtripVerified else { throw EnglishVocabulary.ResourceError.invalidFormat }
        for path in ["mm.binary", "cb", "louds/charID.chid", "Reverse/reverse-v2.bin"] {
            guard FileManager.default.fileExists(atPath: directory.appending(path: path).path) else {
                throw CocoaError(.fileNoSuchFile)
            }
        }
        if mode.english { _ = try EnglishVocabularyResource.result.get() }
        var reverse = ReconversionDictionary()
        try reverse.load(from: directory)
        reverse.replaceUserEntries(userEntries)
        let main = KanaKanjiConverter(dictionaryURL: directory, preloadDictionary: preload)
        let supplement = KanaKanjiConverter(dictionaryURL: directory, preloadDictionary: preload)
        main.importDynamicUserDictionary(conversionEntries)
        supplement.importDynamicUserDictionary(conversionEntries)
        self.mode = mode
        self.directory = directory
        self.reverse = reverse
        self.main = main
        self.supplement = supplement
    }
}

@MainActor final class PreparedBundledDictionaries {
    let mode: BundledDictionaryMode
    let directory: URL
    let main: KanaKanjiConverter
    let supplement: KanaKanjiConverter
    var reverse: ReconversionDictionary
    var normalizer: MixedInputConversion?

    fileprivate init(resources: DictionaryResources) {
        mode = resources.mode
        directory = resources.directory
        main = resources.main
        supplement = resources.supplement
        reverse = resources.reverse
        prepareNormalizer()
    }

    func configureOverlays(userEntries: [DicdataElement], conversionEntries: [DicdataElement]) {
        main.importDynamicUserDictionary(conversionEntries)
        supplement.importDynamicUserDictionary(conversionEntries)
        reverse.replaceUserEntries(userEntries)
        prepareNormalizer()
    }

    private func prepareNormalizer() {
        // Registry access is confined to the input owner. File I/O and resource
        // validation have already finished; this is bounded seed-table work.
        normalizer = config["experimentalMixedInput"] as? Bool == true
            ? MixedInputConversion(inputStyle: currentInputStyle, table: currentRomajiInputTable,
                                   englishDictionaryEnabled: mode.english) : nil
    }
}

@MainActor var activeBundledDictionaryMode = BundledDictionaryMode.both
@MainActor var bundledDictionaryConfigurationInitialized = false
@MainActor var pendingBundledDictionaryConfiguration: PreparedBundledDictionaries?
@MainActor private var preparation: InFlightDictionaryPreparation?
@MainActor private var preparationGeneration: UInt64 = 0
@MainActor var dictionaryModelPreparationInFlight: Bool { preparation != nil }

/// A job owns its new converters exclusively. The Rust task consumes the retained
/// job exactly once, without holding the input mutex, then hands it back under
/// that mutex. It never touches live converters or mapped input-table registries.
private final class DictionaryPreparationJob: @unchecked Sendable {
    let mode: BundledDictionaryMode
    let directory: URL
    private let base: URL
    private let preload: Bool
    private let userEntries: [DicdataElement]
    private let conversionEntries: [DicdataElement]
    private let options: ConvertRequestOptions?
    private let expectedModelStatus: String
    private(set) var resources: DictionaryResources?
    private(set) var failure: String?

    init(mode: BundledDictionaryMode, base: URL, preload: Bool, userEntries: [DicdataElement],
         conversionEntries: [DicdataElement], options: ConvertRequestOptions?, expectedModelStatus: String) {
        self.mode = mode
        self.directory = mode.directory(in: base)
        self.base = base
        self.preload = preload
        self.userEntries = userEntries
        self.conversionEntries = conversionEntries
        self.options = options
        self.expectedModelStatus = expectedModelStatus
    }

    func run() -> Bool {
        do {
            let resources = try DictionaryResources(mode: mode, base: base, preload: preload,
                userEntries: userEntries, conversionEntries: conversionEntries)
            if var options {
                options.learningType = .nothing
                options.experimentalKeyboardTypoCorrection = false
                var text = ComposingText()
                text.insertAtCursorPosition("ニホンゴ", inputStyle: .direct)
                _ = resources.main.requestCandidates(text, options: options)
                resources.main.stopComposition()
                guard resources.main.zenzStatus == expectedModelStatus else { throw CocoaError(.fileReadUnknown) }
            }
            self.resources = resources
            return true
        } catch {
            failure = String(describing: error)
            return false
        }
    }
}

@MainActor private final class InFlightDictionaryPreparation {
    let generation: UInt64
    let job: DictionaryPreparationJob
    var taken = false

    init(job: DictionaryPreparationJob) {
        preparationGeneration &+= 1
        generation = preparationGeneration
        self.job = job
    }
}

@_silgen_name("TakeDictionaryModelPreparation")
@MainActor public func take_dictionary_model_preparation(generation: UnsafeMutablePointer<UInt64>) -> UnsafeMutableRawPointer? {
    guard let preparation, !preparation.taken else { return nil }
    preparation.taken = true
    generation.pointee = preparation.generation
    return Unmanaged.passRetained(preparation.job).toOpaque()
}

@_silgen_name("RunDictionaryModelPreparation")
public func run_dictionary_model_preparation(job: UnsafeMutableRawPointer) -> Int32 {
    Unmanaged<DictionaryPreparationJob>.fromOpaque(job).takeRetainedValue().run() ? 1 : 0
}

@_silgen_name("FinishDictionaryModelPreparation")
@MainActor public func finish_dictionary_model_preparation(generation: UInt64, succeeded: Bool) -> Int32 {
    guard let current = preparation, current.generation == generation else { return 0 }
    preparation = nil
    guard succeeded, let resources = current.job.resources else {
        lastConfigApplySucceeded = false
        serverLog("ERROR", "LoadConfig: bundled preparation failed: \(current.job.failure ?? "worker failed")")
        return 0
    }
    lastConfigApplySucceeded = true
    pendingBundledDictionaryConfiguration = PreparedBundledDictionaries(resources: resources)
    if composingText.input.isEmpty && composingTextSnapshots.isEmpty { applyPendingBundledDictionaries() }
    return 1
}

/// Startup/direct tests can prepare synchronously before serving input. Running
/// settings updates queue *all* validation, dictionary/reverse I/O and model work
/// off the input mutex. Only the final publication touches the input owner.
@MainActor func configureBundledDictionaries(
    mode: BundledDictionaryMode, force: Bool, userEntries: [DicdataElement], conversionEntries: [DicdataElement],
    warmupOptions: ConvertRequestOptions? = nil, prepareAsynchronously: Bool = false
) -> Bool {
    preparation = nil
    let directory = mode.directory(in: converterBaseDictionaryURL)
    if bundledDictionaryConfigurationInitialized && mode == activeBundledDictionaryMode
        && directory == converterDictionaryURL && !force {
        pendingBundledDictionaryConfiguration = nil
        return true
    }
    if let pending = pendingBundledDictionaryConfiguration,
       pending.mode == mode, pending.directory == directory, !force {
        pending.configureOverlays(userEntries: userEntries, conversionEntries: conversionEntries)
        if composingText.input.isEmpty && composingTextSnapshots.isEmpty { applyPendingBundledDictionaries() }
        return true
    }
    pendingBundledDictionaryConfiguration = nil
    let job = DictionaryPreparationJob(mode: mode, base: converterBaseDictionaryURL,
        preload: converterPreloadDictionary, userEntries: userEntries, conversionEntries: conversionEntries,
        options: warmupOptions, expectedModelStatus: "load \(currentZenzaiModelURL().absoluteString)")
    let current = InFlightDictionaryPreparation(job: job)
    preparation = current
    if prepareAsynchronously || warmupOptions != nil {
        serverLog("INFO", "DictionaryModelPreparation: queued generation=\(current.generation)", flush: true)
        return true
    }
    return finish_dictionary_model_preparation(generation: current.generation, succeeded: job.run()) == 1
}

@MainActor func applyPendingBundledDictionaries() {
    guard let prepared = pendingBundledDictionaryConfiguration else { return }
    converter = prepared.main
    normalNBestSupplementConverter = prepared.supplement
    reconversionDictionary = prepared.reverse
    mixedInputConversionCache = prepared.normalizer
    converterDictionaryURL = prepared.directory
    activeBundledDictionaryMode = prepared.mode
    bundledDictionaryConfigurationInitialized = true
    pendingBundledDictionaryConfiguration = nil
}
