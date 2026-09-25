import Foundation
import KanaKanjiConverterModule
import Testing
@testable import azookey_server

private struct JevEvaluationFixture: Decodable {
    let schemaVersion: Int
    let title: String
    let cases: [Case]

    struct Case: Decodable {
        let id: String
        let category: String
        let reading: String
        let externalContext: String
        let acceptableOutputs: [String]
    }
}

private struct JevEvaluationRecord: Encodable {
    let id: String
    let category: String
    let reading: String
    let context: String
    let acceptable: [String]
    let candidates: [String]
    let baseline: String
    let selected: String
    let accepted: Bool
    let oracle: Bool
    let kkcMs: Int
    let jevMs: Int
    let n: Int
    let actualCount: Int
    let reused: Bool
    let fallback: Bool
    let errorCategory: String?

    enum CodingKeys: String, CodingKey {
        case id, category, reading, context, acceptable, candidates, baseline, selected, accepted, oracle
        case kkcMs = "kkc_ms"
        case jevMs = "jev_ms"
        case n
        case actualCount = "actual_count"
        case reused, fallback
        case errorCategory = "error_category"
    }
}

private struct JevEvaluationReport: Encodable {
    let schemaVersion = 1
    let fixtureSchemaVersion: Int
    let fixtureTitle: String
    let requestedN: [Int]
    let records: [JevEvaluationRecord]

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case fixtureSchemaVersion = "fixture_schema_version"
        case fixtureTitle = "fixture_title"
        case requestedN = "requested_n"
        case records
    }
}

private struct JevEvaluationSelection {
    let selectedIndex: Int
    let jevMs: Int
    let fallback: Bool
    let errorCategory: String?
}

private func jevEvaluationFixtureURL() -> URL {
    URL(filePath: #filePath)
        .deletingLastPathComponent()
        .appending(path: "Fixtures")
        .appending(path: "jev-evaluation.json")
}

private func loadJevEvaluationFixture() throws -> JevEvaluationFixture {
    let decoder = JSONDecoder()
    decoder.keyDecodingStrategy = .convertFromSnakeCase
    return try decoder.decode(JevEvaluationFixture.self, from: Data(contentsOf: jevEvaluationFixtureURL()))
}

private func jevEvaluationPackageRootURL() -> URL {
    URL(filePath: #filePath)
        .deletingLastPathComponent()
        .deletingLastPathComponent()
        .deletingLastPathComponent()
}

private func jevEvaluationOptions(memoryURL: URL) -> ConvertRequestOptions {
    let packageRoot = jevEvaluationPackageRootURL()
    return ConvertRequestOptions(
        requireJapanesePrediction: .disabled,
        requireEnglishPrediction: .disabled,
        keyboardLanguage: .ja_JP,
        learningType: .nothing,
        memoryDirectoryURL: memoryURL,
        sharedContainerURL: memoryURL,
        textReplacer: .init {
            packageRoot
                .appending(path: "azooKey_emoji_dictionary_storage")
                .appending(path: "EmojiDictionary")
                .appending(path: "emoji_all_E15.1.txt")
        },
        specialCandidateProviders: nil,
        zenzaiMode: .off,
        metadata: .init(versionString: "Jev explicit-conversion evaluation")
    )
}

@MainActor
private func fullJevEvaluationCandidates(
    reading: String,
    converter: KanaKanjiConverter,
    options: ConvertRequestOptions
) -> (candidates: [String], kkcMs: Int) {
    converter.stopComposition()
    var composingText = ComposingText()
    composingText.insertAtCursorPosition(reading, inputStyle: .direct)
    let preview = makeCandidatePreviewComposingText(from: composingText)
    let start = ProcessInfo.processInfo.systemUptime
    let conversion = converter.requestCandidates(preview.composingText, options: options)
    let kkcMs = Int((ProcessInfo.processInfo.systemUptime - start) * 1_000)

    var seen = Set<String>()
    let candidates = conversion.mainResults.compactMap { candidate -> String? in
        let resolution = resolveCandidateCompositionForDisplay(
            originalComposingText: composingText,
            previewComposingText: preview.composingText,
            candidateComposingCount: candidate.composingCount
        )
        guard resolution.correspondingCount == composingText.input.count,
              resolution.remainingConvertTarget.isEmpty else {
            return nil
        }
        let surface = constructCandidateString(candidate: candidate, hiragana: preview.composingText.convertTarget)
        guard !surface.isEmpty, seen.insert(surface).inserted else {
            return nil
        }
        return surface
    }

    return (Array(candidates.prefix(JevCandidateSelector.maximumCandidates)), kkcMs)
}

private func jevEvaluationErrorCategory(_ error: Error) -> String {
    guard let selectorError = error as? JevCandidateSelectorError else {
        return "unknown"
    }
    switch selectorError {
    case .missingAPIKey: return "missing_api_key"
    case .invalidInput: return "invalid_input"
    case .invalidTimeout: return "invalid_timeout"
    case .requestEncoding: return "request_encoding"
    case .timeout: return "timeout"
    case .transport: return "transport"
    case .unexpectedHTTPStatus: return "http_status"
    case .invalidResponse: return "invalid_response"
    }
}

private func jevEvaluationOutputURL(environment: [String: String]) -> URL {
    if let path = environment["AZOOKEY_JEV_EVALUATION_OUTPUT"], !path.isEmpty {
        return URL(filePath: path)
    }
    return FileManager.default.temporaryDirectory
        .appending(path: "azookey-jev-evaluation-\(UUID().uuidString).json")
}

@Test func jevEvaluationFixtureHasStableShape() throws {
    let fixture = try loadJevEvaluationFixture()

    #expect(fixture.schemaVersion == 1)
    #expect(fixture.cases.count == 40)
    #expect(Set(fixture.cases.map(\.id)).count == fixture.cases.count)
    #expect(fixture.cases.allSatisfy {
        !$0.id.isEmpty
            && !$0.category.isEmpty
            && !$0.reading.isEmpty
            && !$0.acceptableOutputs.isEmpty
            && $0.acceptableOutputs.allSatisfy { !$0.isEmpty }
    })
}

// Explicit opt-in only. The fixture contains public text, but this test sends
// its readings, contexts, and candidate surfaces to TypeSafe AI.
@Test(.enabled(if: ProcessInfo.processInfo.environment["AZOOKEY_JEV_EVALUATE"] == "1"))
func jevExplicitConversionEvaluation() async throws {
    let environment = ProcessInfo.processInfo.environment
    let apiKey = try #require(environment["TYPESAFE_API_KEY"]?.trimmingCharacters(in: .whitespacesAndNewlines))
    #expect(!apiKey.isEmpty)
    let fixture = try loadJevEvaluationFixture()
    let temporary = FileManager.default.temporaryDirectory.appending(path: "azookey-jev-evaluation-\(UUID().uuidString)")
    defer { try? FileManager.default.removeItem(at: temporary) }

    let generated = try await MainActor.run { () throws -> [(fixture: JevEvaluationFixture.Case, candidates: [String], kkcMs: Int)] in
        let packageRoot = jevEvaluationPackageRootURL()
        let converter = KanaKanjiConverter(
            dictionaryURL: packageRoot.appending(path: "azooKey_dictionary_storage/Dictionary"),
            preloadDictionary: true
        )
        defer { converter.stopComposition() }
        let options = jevEvaluationOptions(memoryURL: temporary)
        return fixture.cases.map {
            let generated = fullJevEvaluationCandidates(reading: $0.reading, converter: converter, options: options)
            return (fixture: $0, candidates: generated.candidates, kkcMs: generated.kkcMs)
        }
    }

    let selector = JevCandidateSelector(apiKey: apiKey)
    let requestedN = [8, 16, 32]
    var records: [JevEvaluationRecord] = []
    records.reserveCapacity(generated.count * requestedN.count)

    for generatedCase in generated {
        var selections: [[String]: JevEvaluationSelection] = [:]
        for n in requestedN {
            let options = Array(generatedCase.candidates.prefix(n))
            let selection: JevEvaluationSelection
            let reused: Bool
            if let cached = selections[options] {
                selection = cached
                reused = true
            } else if options.count == 1 {
                selection = .init(selectedIndex: 0, jevMs: 0, fallback: false, errorCategory: nil)
                selections[options] = selection
                reused = false
            } else if options.isEmpty {
                selection = .init(selectedIndex: 0, jevMs: 0, fallback: true, errorCategory: "no_full_candidate")
                selections[options] = selection
                reused = false
            } else {
                let start = ProcessInfo.processInfo.systemUptime
                do {
                    selection = .init(
                        selectedIndex: try selector.select(
                            reading: generatedCase.fixture.reading,
                            context: generatedCase.fixture.externalContext,
                            candidates: options
                        ),
                        jevMs: Int((ProcessInfo.processInfo.systemUptime - start) * 1_000),
                        fallback: false,
                        errorCategory: nil
                    )
                } catch {
                    selection = .init(
                        selectedIndex: 0,
                        jevMs: Int((ProcessInfo.processInfo.systemUptime - start) * 1_000),
                        fallback: true,
                        errorCategory: jevEvaluationErrorCategory(error)
                    )
                }
                selections[options] = selection
                reused = false
            }

            let baseline = options.first ?? ""
            let selected = options.indices.contains(selection.selectedIndex) ? options[selection.selectedIndex] : baseline
            records.append(
                .init(
                    id: generatedCase.fixture.id,
                    category: generatedCase.fixture.category,
                    reading: generatedCase.fixture.reading,
                    context: generatedCase.fixture.externalContext,
                    acceptable: generatedCase.fixture.acceptableOutputs,
                    candidates: options,
                    baseline: baseline,
                    selected: selected,
                    accepted: generatedCase.fixture.acceptableOutputs.contains(selected),
                    oracle: options.contains(where: generatedCase.fixture.acceptableOutputs.contains),
                    kkcMs: generatedCase.kkcMs,
                    jevMs: selection.jevMs,
                    n: n,
                    actualCount: options.count,
                    reused: reused,
                    fallback: selection.fallback,
                    errorCategory: selection.errorCategory
                )
            )
        }
    }

    let report = JevEvaluationReport(
        fixtureSchemaVersion: fixture.schemaVersion,
        fixtureTitle: fixture.title,
        requestedN: requestedN,
        records: records
    )
    let outputURL = jevEvaluationOutputURL(environment: environment)
    try FileManager.default.createDirectory(at: outputURL.deletingLastPathComponent(), withIntermediateDirectories: true)
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
    try encoder.encode(report).write(to: outputURL, options: .atomic)

    #expect(records.count == fixture.cases.count * requestedN.count)
    #expect(Set(records.map(\.n)) == Set(requestedN))
    #expect(records.allSatisfy { $0.actualCount == $0.candidates.count })
    print("Jev explicit evaluation report: \(outputURL.path) records=\(records.count)")
}
