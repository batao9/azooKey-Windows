import KanaKanjiConverterModule

// Distinct from explicit .direct and the engine's frozen .mapped(.empty).
// Empty tables keep each literal character an independent editing boundary.
let mixedInputAutomaticStyle: InputStyle = .mapped(id: .tableName("azookey-windows-mixed-automatic"))
let mixedInputProtectedStyle: InputStyle = .mapped(id: .tableName("azookey-windows-mixed-protected"))
private let mixedInputFinalNStyle: InputStyle = .mapped(id: .tableName("azookey-windows-mixed-final-n"))
private let mixedInputBarrierA: InputStyle = .mapped(id: .tableName("azookey-windows-mixed-barrier-a"))
private let mixedInputBarrierB: InputStyle = .mapped(id: .tableName("azookey-windows-mixed-barrier-b"))

func isMixedInputLiteralStyle(_ style: InputStyle) -> Bool {
    style == mixedInputAutomaticStyle || style == mixedInputProtectedStyle
}

/// An independently authored seed vocabulary, not a general English dictionary.
/// Readable/ambiguous entries are excluded using the user's actual romaji table.
private let mixedInputWords = """
google github gitlab microsoft windows chrome firefox youtube discord slack
python javascript typescript swift rust golang kotlin docker linux ubuntu
npm pnpm yarn pip cargo cmake gradle flutter react vue nextjs nodejs
git commit push pull merge branch build test debug issue bug input output
api http https html css json yaml xml sql ssh tcp udp ssl uuid unicode ascii
server client backend frontend request response token prompt function class
string array object file folder path log update install download upload
keyboard mouse excel word powerpoint vscode chatgpt openai medals ppt
kubernetes cluster postgresql mysql sqlite redis mongodb graphql compiler
interpreter namespace package import export struct enum return async await
vector regex buffer thread mutex socket packet timeout retry checkout fetch parse parser
clone reset diff grep curl clang llvm sdk cli cpu gpu aws gcp
meeting weekend ticket shopping email music lunch news
""".split(whereSeparator: \.isWhitespace).map(String.init)

@MainActor final class MixedInputConversion {
    static let maxInputCount = 256
    static let maxRomajiSpan = 16
    let inputStyle: InputStyle

    private struct TrieNode {
        var children: [UInt8: Int] = [:]
        var terminal = false
        var requiresLiteralReading = false
    }
    private var trie = [TrieNode()]
    private var maxWordLength = 0
    private let includeBundledEnglish: Bool
    private let vocabulary: EnglishVocabulary?
    // Only words actually encountered under this table are classified. Do not
    // construct 61k ComposingText values during startup/table reload.
    private var bundledEligibility: [Int: Bool] = [:]
    private struct JapaneseEdge {
        let count: Int
        let reading: String
    }
    private var japaneseEdgeCache: [[Character]: [JapaneseEdge]] = [:]

    init(inputStyle: InputStyle, table: InputTable = .defaultRomanToKana, includeBundledEnglish: Bool = true, englishDictionaryEnabled: Bool = true) {
        self.inputStyle = inputStyle
        self.includeBundledEnglish = includeBundledEnglish && englishDictionaryEnabled
        self.vocabulary = self.includeBundledEnglish ? (try? EnglishVocabularyResource.result.get()) : nil
        InputStyleManager.registerInputStyle(table: .empty, for: "azookey-windows-mixed-automatic")
        InputStyleManager.registerInputStyle(table: .empty, for: "azookey-windows-mixed-protected")
        InputStyleManager.registerInputStyle(table: InputTable(baseMapping: [
            [.piece(.character("n"))]: [.character("ん")]
        ]), for: "azookey-windows-mixed-final-n")
        InputStyleManager.registerInputStyle(table: table, for: "azookey-windows-mixed-barrier-a")
        InputStyleManager.registerInputStyle(table: table, for: "azookey-windows-mixed-barrier-b")
        for word in englishDictionaryEnabled ? mixedInputWords : [] {
            var japanese = ComposingText()
            japanese.insertAtCursorPosition(word, inputStyle: inputStyle)
            // Do not steal fully readable Japanese, including explicit custom
            // mappings. This intentionally leaves api/sushi/make ambiguous.
            guard japanese.convertTarget.utf8.contains(where: Self.isLetter) else { continue }
            japanese.insertAtCursorPosition([.init(piece: .compositionSeparator, inputStyle: inputStyle)])
            let requiresLiteralReading = japanese.convertTarget.utf8.contains(where: Self.isLetter)
            var node = 0
            for byte in word.utf8 {
                if let child = trie[node].children[byte] {
                    node = child
                } else {
                    let child = trie.count
                    trie.append(TrieNode())
                    trie[node].children[byte] = child
                    node = child
                }
            }
            trie[node].terminal = true
            trie[node].requiresLiteralReading = requiresLiteralReading
            maxWordLength = max(maxWordLength, word.utf8.count)
        }
    }

    private static func isLetter(_ byte: UInt8) -> Bool {
        (65...90).contains(byte) || (97...122).contains(byte)
    }

    private func romanByte(_ element: ComposingText.InputElement) -> UInt8? {
        guard element.inputStyle == inputStyle || element.inputStyle == mixedInputFinalNStyle
                || isMixedInputLiteralStyle(element.inputStyle),
              case .character(let character) = element.piece,
              let byte = character.asciiValue, Self.isLetter(byte) else { return nil }
        return byte >= 65 && byte <= 90 ? byte + 32 : byte
    }

    func normalize(_ source: ComposingText) -> ComposingText {
        guard !source.input.isEmpty, source.input.count <= Self.maxInputCount,
              let cursor = mixedInputCursorIndex(source) else { return source }
        // Added words are eligible only when the actual table leaves a Latin
        // letter. A fully Japanese surface cannot need the expanded lexicon;
        // keep the seed path unchanged and avoid a new DP on ordinary text.
        let searchBundled = includeBundledEnglish && source.convertTarget.utf8.contains(where: Self.isLetter)
        var elements = source.input
        var start = 0
        while start < elements.count {
            guard romanByte(elements[start]) != nil else { start += 1; continue }
            var end = start + 1
            while end < elements.count, romanByte(elements[end]) != nil { end += 1 }
            let run = Array(elements[start..<end])
            let literal = literalMask(run, searchBundled: searchBundled)
            for index in run.indices {
                if run[index].inputStyle == mixedInputProtectedStyle { continue }
                let style = literal[index] ? mixedInputAutomaticStyle : inputStyle
                if elements[start + index].inputStyle != style { elements[start + index].inputStyle = style }
            }
            start = end
        }
        // Switching tables does not finalize the Japanese table's pending n.
        // Represent it with the same raw n and a one-element finalized style;
        // never insert a synthetic input element or consume the English g.
        for boundary in 1..<elements.count where isMixedInputLiteralStyle(elements[boundary].inputStyle) {
            let japaneseStyle = elements[boundary - 1].inputStyle
            guard japaneseStyle == inputStyle || japaneseStyle == mixedInputBarrierA || japaneseStyle == mixedInputBarrierB,
                  elements[boundary - 1].piece == .character("n") else { continue }
            var start = boundary - 1
            while start > 0, elements[start - 1].inputStyle == japaneseStyle { start -= 1 }
            var japanese = ComposingText()
            japanese.insertAtCursorPosition(Array(elements[start..<boundary]))
            guard japanese.convertTarget.last == "n" else { continue }
            var finalized = japanese
            finalized.insertAtCursorPosition([.init(piece: .compositionSeparator, inputStyle: japaneseStyle)])
            var withoutN = ComposingText()
            withoutN.insertAtCursorPosition(Array(elements[start..<(boundary - 1)]))
            guard finalized.convertTarget == withoutN.convertTarget + "ん" else { return source }
            elements[boundary - 1].inputStyle = mixedInputFinalNStyle
        }
        guard elements != source.input else { return source }
        return replacingMixedInputElements(source, with: elements, cursor: cursor) ?? source
    }

    private struct Step {
        let start: Int
        let previousLanguage: Int
        let english: Bool
    }

    private struct EnglishMatch {
        let end: Int
        let requiresLiteralReading: Bool
        var vocabularyPenalty: Double = 0
    }

    @inline(__always)
    private func bundledChild(node: Int, byte: UInt8, data: UnsafeBufferPointer<UInt8>) -> Int? {
        let offset = node * 8
        let childCount = Int(data[offset + 4])
        guard childCount != 0 else { return nil }
        let first = Int(data[offset]) | Int(data[offset + 1]) << 8
            | Int(data[offset + 2]) << 16 | Int(data[offset + 3]) << 24
        var low = first
        var high = first + childCount
        while low < high {
            let middle = (low + high) / 2
            if data[middle * 8 + 5] < byte { low = middle + 1 } else { high = middle }
        }
        return low < first + childCount && data[low * 8 + 5] == byte ? low : nil
    }

    private func bundledWordRequiresLiteral(node: Int, bytes: ArraySlice<UInt8>) -> Bool {
        if let cached = bundledEligibility[node] { return cached }
        let word = String(decoding: bytes, as: UTF8.self)
        var japanese = ComposingText()
        japanese.insertAtCursorPosition(word, inputStyle: inputStyle)
        japanese.insertAtCursorPosition([.init(piece: .compositionSeparator, inputStyle: inputStyle)])
        let result = japanese.convertTarget.utf8.contains(where: Self.isLetter)
        if bundledEligibility.count >= 512 { bundledEligibility.removeAll(keepingCapacity: true) }
        bundledEligibility[node] = result
        return result
    }

    private func appendBundledMatches(bytes: [UInt8], matches: inout [[EnglishMatch]]) -> Bool {
        guard let vocabulary else { return false }
        return vocabulary.bytes.withUnsafeBufferPointer { data in
            var found = false
            for start in bytes.indices {
                var node = 0
                for end in start..<min(bytes.count, start + 20) {
                    guard let child = bundledChild(node: node, byte: bytes[end], data: data) else { break }
                    node = child
                    if data[child * 8 + 6] != 0,
                       !matches[start].contains(where: { $0.end == end + 1 }),
                       bundledWordRequiresLiteral(node: child, bytes: bytes[start...end]) {
                        matches[start].append(.init(end: end + 1, requiresLiteralReading: true, vocabularyPenalty: 1.2))
                        found = true
                    }
                }
            }
            return found
        }
    }

    private static let particles: Set<String> = ["は", "が", "を", "に", "で", "の", "と", "も", "へ", "し"]

    private func startsWithJapaneseParticle(_ elements: [ComposingText.InputElement], at start: Int) -> Bool {
        var key: [Character] = []
        for index in start..<min(elements.count, start + Self.maxRomajiSpan) {
            if elements[index].inputStyle == mixedInputProtectedStyle { break }
            if case .character(let character) = elements[index].piece { key.append(character) }
        }
        return japaneseEdges(key).contains { Self.particles.contains($0.reading) }
    }

    /// Two-language Viterbi with a bounded phonotactic Japanese alternative.
    /// No converter requests, dictionary I/O or OS spellchecker calls occur here.
    private func literalMask(_ elements: [ComposingText.InputElement], searchBundled: Bool) -> [Bool] {
        let count = elements.count
        let bytes = elements.map { romanByte($0)! }
        var matches = Array(repeating: [EnglishMatch](), count: count)
        var hasMatch = false
        for start in 0..<count {
            var node = 0
            for end in start..<min(count, start + maxWordLength) {
                guard let child = trie[node].children[bytes[end]] else { break }
                node = child
                if trie[node].terminal {
                    matches[start].append(.init(end: end + 1, requiresLiteralReading: trie[node].requiresLiteralReading))
                    hasMatch = true
                }
            }
        }
        if searchBundled, appendBundledMatches(bytes: bytes, matches: &matches) { hasMatch = true }
        let protected = elements.map { $0.inputStyle == mixedInputProtectedStyle }
        guard hasMatch || protected.contains(true) else { return Array(repeating: false, count: count) }
        var costs = Array(repeating: [Double.infinity, Double.infinity], count: count + 1)
        var steps = Array(repeating: [Step?](repeating: nil, count: 2), count: count + 1)
        costs[0][0] = 0

        func offer(_ start: Int, _ end: Int, english: Bool, cost: Double, particle: Bool = false, nounBoundary: Bool = false) {
            let language = english ? 1 : 0
            for previous in 0...1 where costs[start][previous].isFinite {
                let switching = start > 0 && previous != language && !particle && !nounBoundary ? 0.7 : 0
                let total = costs[start][previous] + cost + switching
                if total < costs[end][language] {
                    costs[end][language] = total
                    steps[end][language] = Step(start: start, previousLanguage: previous, english: english)
                }
            }
        }

        for start in 0..<count {
            for match in matches[start] {
                let end = match.end
                let completesProtected = protected[start..<end].contains(true)
                // A known non-Japanese word followed by an actual table's
                // particle has its own boundary: git|wo is not gi|two. A word
                // readable as Japanese after finalizing n (token/shutoken) is
                // deliberately ineligible for this preference.
                let nounBoundary = match.requiresLiteralReading && end < count
                    // Leave ordinary costs for t|to sokuon spellings such as
                    // resetto; don't turn their small-tsu into an English tail.
                    && !(bytes[end - 1] == 0x74 && bytes[end] == 0x74)
                    && startsWithJapaneseParticle(elements, at: end)
                offer(start, end, english: true,
                      cost: (completesProtected || nounBoundary ? 0 : 0.7) + Double(end - start) * 0.015 + match.vocabularyPenalty,
                      nounBoundary: nounBoundary)
            }
            if protected[start] {
                var end = start + 1
                while end < count, protected[end] { end += 1 }
                offer(start, end, english: true, cost: 0)
                continue
            }
            // An unresolved letter is retained, but is not evidence of a word.
            offer(start, start + 1, english: false, cost: 3.5)
            var key: [Character] = []
            for end in start..<min(count, start + Self.maxRomajiSpan) {
                if protected[end] { break }
                if case .character(let character) = elements[end].piece { key.append(character) }
            }
            for edge in japaneseEdges(key) {
                let morae = edge.reading.filter { !"ぁぃぅぇぉゃゅょゎ".contains($0) }.count
                offer(start, start + edge.count, english: false, cost: Double(max(1, morae)) * 0.25,
                      particle: Self.particles.contains(edge.reading))
            }
        }
        var mask = Array(repeating: false, count: count)
        var end = count
        var language = costs[count][0] <= costs[count][1] ? 0 : 1
        while end > 0, let step = steps[end][language] {
            if step.english { for index in step.start..<end { mask[index] = true } }
            end = step.start
            language = step.previousLanguage
        }
        return mask
    }

    private func japaneseEdges(_ key: [Character]) -> [JapaneseEdge] {
        if let cached = japaneseEdgeCache[key] { return cached }
        var edges: [JapaneseEdge] = []
        var japanese = ComposingText()
        for (index, character) in key.enumerated() {
            japanese.insertAtCursorPosition(String(character), inputStyle: inputStyle)
            let reading = japanese.convertTarget
            if !reading.isEmpty && !reading.utf8.contains(where: Self.isLetter) {
                edges.append(.init(count: index + 1, reading: reading))
                break
            }
            if reading == "n" {
                var finalized = japanese
                finalized.insertAtCursorPosition([.init(piece: .compositionSeparator, inputStyle: inputStyle)])
                if finalized.convertTarget == "ん" { edges.append(.init(count: index + 1, reading: "ん")) }
            }
        }
        // Bound table-local phonotactic memoization to 512 short keys.
        // Reloading a table replaces this instance and invalidates the cache.
        if japaneseEdgeCache.count >= 512 { japaneseEdgeCache.removeAll(keepingCapacity: true) }
        japaneseEdgeCache[key] = edges
        return edges
    }
}

@MainActor var mixedInputConversionCache: MixedInputConversion?

@MainActor func prepareMixedInputConversion(reset: Bool = false) {
    if reset { mixedInputConversionCache = nil }
    guard config["experimentalMixedInput"] as? Bool == true else { return }
    if mixedInputConversionCache?.inputStyle != currentInputStyle {
        mixedInputConversionCache = MixedInputConversion(inputStyle: currentInputStyle, table: currentRomajiInputTable,
            englishDictionaryEnabled: activeBundledDictionaryMode.english)
    }
}

@MainActor func normalizeMixedInputAfterEdit() {
    guard config["experimentalMixedInput"] as? Bool == true else { return }
    prepareMixedInputConversion()
    composingText = mixedInputConversionCache!.normalize(composingText)
}

/// Preserve the converter's edit isolation without a zero-width input element
/// that cannot be represented by the existing raw-input identity protocol.
/// Alternating aliases use the actual user's table and keep all original keys.
@MainActor func removeMixedInputEditingSeparators() {
    guard config["experimentalMixedInput"] as? Bool == true,
          composingText.input.contains(where: { $0.piece == .compositionSeparator }) else { return }
    prepareMixedInputConversion()
    var elements = composingText.input
    for index in elements.indices.reversed() where elements[index].piece == .compositionSeparator {
        guard elements[index].inputStyle == .mapped(id: .empty) else { continue }
        if index > 0, index + 1 < elements.count,
           elements[index - 1].inputStyle == elements[index + 1].inputStyle {
            let oldStyle = elements[index + 1].inputStyle
            let newStyle: InputStyle
            if oldStyle == currentInputStyle || oldStyle == mixedInputBarrierB { newStyle = mixedInputBarrierA }
            else if oldStyle == mixedInputBarrierA { newStyle = mixedInputBarrierB }
            else if oldStyle == .direct || oldStyle == .mapped(id: .empty) { newStyle = mixedInputProtectedStyle }
            else { continue }
            var end = index + 1
            while end < elements.count, elements[end].inputStyle == oldStyle {
                elements[end].inputStyle = newStyle
                end += 1
            }
        }
        elements.remove(at: index)
    }
    var rebuilt = ComposingText()
    rebuilt.insertAtCursorPosition(elements)
    // A separator can also finalize a pending Japanese n. Preserve its actual
    // output without adding a raw element when the following style changed.
    if rebuilt.convertTarget != composingText.convertTarget {
        for index in elements.indices where elements[index].piece == .character("n") {
            if index + 1 == elements.count || elements[index].inputStyle == elements[index + 1].inputStyle { continue }
            guard elements[index].inputStyle == currentInputStyle || elements[index].inputStyle == mixedInputBarrierA
                    || elements[index].inputStyle == mixedInputBarrierB else { continue }
            var trial = elements
            trial[index].inputStyle = mixedInputFinalNStyle
            var candidate = ComposingText()
            candidate.insertAtCursorPosition(trial)
            if candidate.convertTarget == composingText.convertTarget { elements = trial; rebuilt = candidate; break }
        }
    }
    guard rebuilt.convertTarget == composingText.convertTarget else { return }
    _ = rebuilt.moveCursorFromCursorPosition(count: composingText.convertTargetCursorPosition - rebuilt.convertTargetCursorPosition)
    composingText = rebuilt
}

private func mixedInputCursorIndex(_ source: ComposingText) -> Int? {
    if source.isAtEndIndex { return source.input.count }
    return source.inputIndexToSurfaceIndexMap().filter { $0.value == source.convertTargetCursorPosition }.keys.max()
}

private func replacingMixedInputElements(
    _ source: ComposingText, with elements: [ComposingText.InputElement], cursor: Int
) -> ComposingText? {
    var result = ComposingText()
    result.insertAtCursorPosition(elements)
    guard let surface = result.inputIndexToSurfaceIndexMap()[cursor] else { return nil }
    _ = result.moveCursorFromCursorPosition(count: surface - result.convertTargetCursorPosition)
    return result
}

/// Edits inside literal text must act on the displayed characters first, not
/// on a romanized shadow (google's final le must not become one small kana).
@MainActor func insertInsideMixedInputLiteral(_ text: String) -> Bool {
    guard config["experimentalMixedInput"] as? Bool == true,
          let cursor = mixedInputCursorIndex(composingText), cursor > 0, cursor < composingText.input.count,
          isMixedInputLiteralStyle(composingText.input[cursor - 1].inputStyle),
          isMixedInputLiteralStyle(composingText.input[cursor].inputStyle) else { return false }
    var elements = composingText.input
    var start = cursor - 1
    var end = cursor
    while start > 0, isMixedInputLiteralStyle(elements[start - 1].inputStyle) { start -= 1 }
    while end < elements.count, isMixedInputLiteralStyle(elements[end].inputStyle) { end += 1 }
    // Both sides of an interior edit are now fragments, not necessarily words.
    for index in start..<end { elements[index].inputStyle = mixedInputProtectedStyle }
    let inserted = text.map { ComposingText.InputElement(character: $0, inputStyle: mixedInputProtectedStyle) }
    elements.insert(contentsOf: inserted, at: cursor)
    guard let result = replacingMixedInputElements(composingText, with: elements, cursor: cursor + inserted.count) else { return false }
    composingText = result
    return true
}

@MainActor func removeMixedInputLiteralBeforeCursor() -> Bool {
    guard config["experimentalMixedInput"] as? Bool == true,
          let cursor = mixedInputCursorIndex(composingText), cursor > 0,
          isMixedInputLiteralStyle(composingText.input[cursor - 1].inputStyle) else { return false }
    var elements = composingText.input
    var start = cursor - 1
    var end = cursor
    while start > 0, isMixedInputLiteralStyle(elements[start - 1].inputStyle) { start -= 1 }
    while end < elements.count, isMixedInputLiteralStyle(elements[end].inputStyle) { end += 1 }
    elements.remove(at: cursor - 1)
    for index in start..<(end - 1) { elements[index].inputStyle = mixedInputProtectedStyle }
    guard let result = replacingMixedInputElements(composingText, with: elements, cursor: cursor - 1) else { return false }
    composingText = result
    return true
}

/// Preserve a fragment clipped by clause navigation/partial commit even though
/// it is no longer a complete dictionary word. The pre-shrink snapshot stays intact.
func protectMixedInputPrefixAfterShrink(_ source: ComposingText) -> ComposingText {
    var elements = source.input
    for index in elements.indices {
        guard isMixedInputLiteralStyle(elements[index].inputStyle) else { break }
        elements[index].inputStyle = mixedInputProtectedStyle
    }
    guard elements != source.input, let cursor = mixedInputCursorIndex(source) else { return source }
    return replacingMixedInputElements(source, with: elements, cursor: cursor) ?? source
}
