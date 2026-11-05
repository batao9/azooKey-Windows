import Foundation

/// Immutable, validated search data. Canonical inputs live under
/// data/english-dictionaries; no dictionary payload is embedded in Swift code.
struct EnglishVocabulary: Sendable {
    let bytes: [UInt8]
    let wordCount: Int

    enum ResourceError: Error {
        case missingBundle
        case missingResource
        case invalidFormat
    }

    init(data: Data) throws {
        let data = Array(data)
        func integer(_ offset: Int) -> Int {
            Int(data[offset]) | Int(data[offset + 1]) << 8
                | Int(data[offset + 2]) << 16 | Int(data[offset + 3]) << 24
        }
        guard data.count >= 24, Array(data.prefix(4)) == Array("AZEV".utf8), integer(4) == 1 else {
            throw ResourceError.invalidFormat
        }
        let nodes = integer(8)
        let words = integer(12)
        guard (1...2_000_000).contains(nodes), (1...1_000_000).contains(words),
              data.count == 16 + nodes * 8 else { throw ResourceError.invalidFormat }
        var nextChild = 1
        var terminals = 0
        for node in 0..<nodes {
            let offset = 16 + node * 8
            let first = integer(offset)
            let count = Int(data[offset + 4])
            let label = data[offset + 5]
            guard count <= 26, data[offset + 6] <= 1, data[offset + 7] == 0,
                  (node == 0 ? label == 0 && data[offset + 6] == 0 : (97...122).contains(label)) else {
                throw ResourceError.invalidFormat
            }
            terminals += Int(data[offset + 6])
            if count == 0 {
                guard first == 0 else { throw ResourceError.invalidFormat }
                continue
            }
            guard first == nextChild, first > node, first + count <= nodes else {
                throw ResourceError.invalidFormat
            }
            for child in first..<(first + count - 1) {
                guard data[16 + child * 8 + 5] < data[16 + (child + 1) * 8 + 5] else {
                    throw ResourceError.invalidFormat
                }
            }
            nextChild += count
        }
        guard nextChild == nodes, terminals == words else { throw ResourceError.invalidFormat }
        self.bytes = Array(data.dropFirst(16))
        self.wordCount = words
    }

    static func loadBundled() throws -> Self {
        // SwiftPM's accessor can fatalError when the entire bundle is absent.
        // Validate the actual distribution layout before accessing Bundle.module,
        // rather than silently using its source-tree fallback in a packaged app.
        let directory = Bundle.main.bundleURL.appendingPathComponent("azookey-server_azookey-server.resources")
        guard FileManager.default.fileExists(atPath: directory.path) else { throw ResourceError.missingBundle }
        guard let url = Bundle.module.url(forResource: "english-vocabulary", withExtension: "bin"),
              url.deletingLastPathComponent().standardizedFileURL == directory.standardizedFileURL else {
            throw ResourceError.missingResource
        }
        return try Self(data: Data(contentsOf: url))
    }
}

enum EnglishVocabularyResource {
    // Success and failure are both cached. Initialize/LoadConfig primes this
    // before input; a missing asset never causes repeated typing-path I/O.
    static let result = Result { try EnglishVocabulary.loadBundled() }
}
