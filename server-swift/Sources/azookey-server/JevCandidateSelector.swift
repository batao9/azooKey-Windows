import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

enum JevCandidateSelectorError: Error, Equatable {
    case missingAPIKey
    case invalidInput
    case invalidTimeout
    case requestEncoding
    case timeout
    case transport
    case unexpectedHTTPStatus(Int)
    case invalidResponse
}

/// A stateless TypeSafe Choice client for ranking complete KKC candidates.
///
/// This type deliberately has no composition or converter state. Call it from a
/// background worker after KKC has produced its ordinary N-best list.
final class JevCandidateSelector {
    typealias Transport = (Data, TimeInterval) throws -> Data

    static let maximumCandidates = 32
    static let maximumReadingLength = 256
    static let maximumContextLength = 2_048
    static let maximumCandidateLength = 512
    static let defaultTimeout: TimeInterval = 1.5

    private let apiKey: String
    private let model: String
    private let timeout: TimeInterval
    private let transport: Transport?

    init(
        apiKey: String,
        model: String = "jev-latest",
        timeout: TimeInterval = JevCandidateSelector.defaultTimeout,
        transport: Transport? = nil
    ) {
        self.apiKey = apiKey
        self.model = model
        self.timeout = timeout
        self.transport = transport
    }

    func select(reading: String, context: String, candidates: [String]) throws -> Int {
        guard !apiKey.isEmpty else { throw JevCandidateSelectorError.missingAPIKey }
        guard timeout.isFinite, timeout > 0, timeout <= Self.defaultTimeout else {
            throw JevCandidateSelectorError.invalidTimeout
        }
        guard isValidInput(reading: reading, context: context, candidates: candidates) else {
            throw JevCandidateSelectorError.invalidInput
        }

        let body: Data
        do {
            body = try JSONEncoder().encode(
                JevCandidateRequest(reading: reading, context: context, candidates: candidates, model: model)
            )
        } catch {
            throw JevCandidateSelectorError.requestEncoding
        }

        let response: Data
        do {
            response = try transport?(body, timeout) ?? send(body: body)
        } catch let error as JevCandidateSelectorError {
            throw error
        } catch {
            throw JevCandidateSelectorError.transport
        }
        return try selectedIndex(response: response, candidateCount: candidates.count)
    }

    private func isValidInput(reading: String, context: String, candidates: [String]) -> Bool {
        !reading.isEmpty
            && reading.count <= Self.maximumReadingLength
            && context.count <= Self.maximumContextLength
            && !candidates.isEmpty
            && candidates.count <= Self.maximumCandidates
            && candidates.allSatisfy { !$0.isEmpty && $0.count <= Self.maximumCandidateLength }
    }

    private func selectedIndex(response: Data, candidateCount: Int) throws -> Int {
        let decoded: JevCandidateResponse
        do {
            decoded = try JSONDecoder().decode(JevCandidateResponse.self, from: response)
        } catch {
            throw JevCandidateSelectorError.invalidResponse
        }
        let ids = (0..<candidateCount).map { "c\($0)" }
        guard decoded.answers.count == 1,
              let answer = decoded.answers["g0"],
              answer.type == "choice",
              ids.contains(answer.choice),
              answer.probabilities.count == ids.count,
              Set(answer.probabilities.keys) == Set(ids) else {
            throw JevCandidateSelectorError.invalidResponse
        }

        var bestIndex = 0
        var bestProbability = -Double.infinity
        for (index, id) in ids.enumerated() {
            guard let probability = answer.probabilities[id],
                  probability.isFinite, probability >= 0 else {
                throw JevCandidateSelectorError.invalidResponse
            }
            // Keeping the first index on ties makes the result deterministic.
            if probability > bestProbability {
                bestProbability = probability
                bestIndex = index
            }
        }
        guard bestProbability > 0 else {
            throw JevCandidateSelectorError.invalidResponse
        }
        return bestIndex
    }

    private func send(body: Data) throws -> Data {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.requestCachePolicy = .reloadIgnoringLocalCacheData
        configuration.urlCredentialStorage = nil
        configuration.httpShouldSetCookies = false
        let delegateQueue = OperationQueue()
        delegateQueue.maxConcurrentOperationCount = 1
        let session = URLSession(
            configuration: configuration,
            delegate: JevCandidateNoRedirectDelegate(),
            delegateQueue: delegateQueue
        )
        defer { session.invalidateAndCancel() }

        var request = URLRequest(url: URL(string: "https://api.typesafe.ai/v1/systemone")!)
        request.httpMethod = "POST"
        request.httpBody = body
        request.timeoutInterval = timeout
        request.setValue("Bearer \(apiKey)", forHTTPHeaderField: "Authorization")
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")

        let box = JevCandidateResponseBox()
        let task = session.dataTask(with: request) { data, urlResponse, error in
            box.set(data: data, response: urlResponse, error: error)
        }
        task.resume()
        guard box.wait(timeout: timeout) == .success else {
            task.cancel()
            throw JevCandidateSelectorError.timeout
        }

        let result = box.result()
        if result.error != nil { throw JevCandidateSelectorError.transport }
        guard let httpResponse = result.response as? HTTPURLResponse else {
            throw JevCandidateSelectorError.transport
        }
        guard (200...299).contains(httpResponse.statusCode) else {
            throw JevCandidateSelectorError.unexpectedHTTPStatus(httpResponse.statusCode)
        }
        guard let data = result.data else { throw JevCandidateSelectorError.invalidResponse }
        return data
    }
}

@_cdecl("SelectJevCandidate")
public func selectJevCandidate(_ json: UnsafePointer<CChar>?) -> Int32 {
    guard let json,
          let input = String(validatingCString: json),
          let data = input.data(using: .utf8),
          let request = try? JSONDecoder().decode(JevCandidateFFIRequest.self, from: data),
          !request.apiKey.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
          !request.apiKey.contains(where: { $0.isNewline }) else {
        return -1
    }
    do {
        let selected = try JevCandidateSelector(apiKey: request.apiKey.trimmingCharacters(in: .whitespacesAndNewlines)).select(
            reading: request.reading,
            context: request.context,
            candidates: request.candidates
        )
        return Int32(selected)
    } catch {
        return -1
    }
}

struct JevCandidateFFIRequest: Decodable {
    let apiKey: String
    let reading: String
    let context: String
    let candidates: [String]
}

private struct JevCandidateRequest: Encodable {
    struct State: Encodable { let task: String }
    struct Question: Encodable {
        struct Instructions: Encodable {
            let task: String
            let reading: String
            let context: String
        }
        struct Criterion: Encodable { let text: String }

        let type = "choice"
        let instructions: Instructions
        let criteria: [String: Criterion]
    }

    let state: State
    let model: String
    let questions: [String: Question]

    init(reading: String, context: String, candidates: [String], model: String) {
        state = .init(task: "Select the best Japanese IME conversion.")
        self.model = model
        questions = [
            "g0": .init(
                instructions: .init(
                    task: "Choose the candidate that best converts the complete Japanese IME reading.",
                    reading: reading,
                    context: context
                ),
                criteria: Dictionary(uniqueKeysWithValues: candidates.enumerated().map { index, candidate in
                    ("c\(index)", .init(text: candidate))
                })
            )
        ]
    }
}

private struct JevCandidateResponse: Decodable {
    struct Answer: Decodable {
        let type: String
        let choice: String
        let probabilities: [String: Double]
    }

    let answers: [String: Answer]
}

private final class JevCandidateResponseBox: @unchecked Sendable {
    private let lock = NSLock()
    private let semaphore = DispatchSemaphore(value: 0)
    private var data: Data?
    private var response: URLResponse?
    private var error: Error?

    func set(data: Data?, response: URLResponse?, error: Error?) {
        lock.lock()
        self.data = data
        self.response = response
        self.error = error
        lock.unlock()
        semaphore.signal()
    }

    func wait(timeout: TimeInterval) -> DispatchTimeoutResult {
        semaphore.wait(timeout: .now() + timeout)
    }

    func result() -> (data: Data?, response: URLResponse?, error: Error?) {
        lock.lock()
        defer { lock.unlock() }
        return (data, response, error)
    }
}

private final class JevCandidateNoRedirectDelegate: NSObject, URLSessionTaskDelegate {
    func urlSession(
        _ session: URLSession,
        task: URLSessionTask,
        willPerformHTTPRedirection response: HTTPURLResponse,
        newRequest request: URLRequest,
        completionHandler: @escaping (URLRequest?) -> Void
    ) {
        completionHandler(nil)
    }
}
