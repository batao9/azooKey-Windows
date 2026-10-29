import KanaKanjiConverterModule

struct KeyboardTypoProtectedReading: Sendable {
    let characters: [Character]
    let typoOffset: Int

    init(reading: String, typoOffset: Int) {
        self.characters = Array(reading)
        self.typoOffset = typoOffset
    }
}

func isKeyboardComposerNormalization(_ element: DicdataElement) -> Bool {
    element.metadata.contains(.isKeyboardTypoCorrection)
        && keyboardComposerNormalizationReadings.contains(element.ruby)
}

/// Dictionary rubies describe original spans, including composer-only entries.
/// Prefer the one actual typo over a preceding normalization such as minna.
/// A length-changing rewrite cannot be located by summing dictionary rubies.
func keyboardTypoDictionaryCorrectionForAutomaticSelection(
    in candidate: Candidate
) -> (entry: DicdataElement, originalSurfaceRange: Range<Int>)? {
    guard candidate.keyboardTypoCorrections.isEmpty else { return nil }
    var result: (entry: DicdataElement, originalSurfaceRange: Range<Int>)?
    var normalization: (entry: DicdataElement, originalSurfaceRange: Range<Int>)?
    var offset = 0
    for element in candidate.data {
        let end = offset + element.ruby.count
        if element.metadata.contains(.isKeyboardTypoCorrection) {
            if isKeyboardComposerNormalization(element) {
                if normalization == nil { normalization = (element, offset ..< end) }
            } else {
                guard result == nil else { return nil }
                result = (element, offset ..< end)
            }
        }
        offset = end
    }
    return result ?? normalization
}

func hasSingleLexicalAlternativeCoveringTypoSpan(
    candidate: Candidate,
    originalSurfaceRange: Range<Int>
) -> Bool {
    var offset = 0
    for element in candidate.data {
        let ruby = displayRubyForKeyboardTypoLookup(element.ruby)
        let end = offset + ruby.count
        if offset <= originalSurfaceRange.lowerBound, end >= originalSurfaceRange.upperBound,
           !element.metadata.contains(.isKeyboardTypoCorrection) {
            return hiraganaToKatakana(element.word) != ruby
        }
        offset = end
    }
    return false
}

/// Eligibility for automatic selection, separate from candidate generation.
/// Ambiguous corrections remain available for explicit candidate selection.
struct KeyboardTypoAutomaticConversion {
    private let input: [Character]

    init(hiragana: String) {
        // The built-in roman2kana fallback retains a literal '-' where the
        // Windows mapped table emits 'ー'. Normalize only the comparison copy.
        var input = hiraganaToKatakana(hiragana).map { $0 == "-" ? "ー" : $0 }
        // An unresolved roman suffix (e.g. せきゅりth) is not evidence that the
        // user has stopped typing セキュリティ. Compare its stable kana prefix.
        while let last = input.last, ("a" ... "z").contains(last) {
            input.removeLast()
        }
        self.input = input
    }

    func allowsAutomaticSelection(_ candidate: Candidate) -> Bool {
        var correctionCount = candidate.keyboardTypoCorrections.count
        var correctionRuby: String?
        var correctionStart = 0
        var offset = 0
        for element in candidate.data {
            if element.metadata.contains(.isKeyboardTypoCorrection), !isKeyboardComposerNormalization(element) {
                correctionCount += 1
                correctionRuby = displayRubyForKeyboardTypoLookup(element.ruby)
                correctionStart = offset
            }
            offset += element.ruby.count
        }
        // A single actual typing error or rewrite may be automatic. Correctly
        // spelled composer normalizations keep their original ruby coordinates
        // and must not disable the main correction later in the sentence.
        guard correctionCount <= 1 else { return false }
        guard let correctionRuby,
              let protections = keyboardTypoProtectedReadings[correctionRuby]
        else { return true }

        for protection in protections {
            let start = correctionStart - protection.typoOffset
            guard start >= 0, start < input.count else { continue }
            let count = min(protection.characters.count, input.count - start)
            // Require the entire typo span to occur in the stable input.
            guard count >= protection.typoOffset + correctionRuby.count else { continue }
            if input[start ..< start + count].elementsEqual(protection.characters.prefix(count)) {
                return false
            }
        }
        return true
    }
}

func prioritizeAutomaticKeyboardTypoCandidates<Element>(
    _ elements: [Element],
    hiragana: String,
    candidate: (Element) -> Candidate
) -> [Element] {
    var policy: KeyboardTypoAutomaticConversion?
    var automatic: [Element] = []
    var suggestions: [Element] = []
    for (index, element) in elements.enumerated() {
        let value = candidate(element)
        let hasCorrection = !value.keyboardTypoCorrections.isEmpty
            || value.data.contains { $0.metadata.contains(.isKeyboardTypoCorrection) }
        if hasCorrection && policy == nil {
            policy = KeyboardTypoAutomaticConversion(hiragana: hiragana)
        }
        if hasCorrection, policy?.allowsAutomaticSelection(value) == false {
            if suggestions.isEmpty {
                automatic = Array(elements[..<index])
            }
            suggestions.append(element)
        } else if !suggestions.isEmpty {
            automatic.append(element)
        }
    }
    guard !suggestions.isEmpty else { return elements }
    // Keep three uncorrected/eligible choices first, but do not bury useful
    // explicit corrections behind hundreds of ordinary dictionary candidates.
    let insertionIndex = min(3, automatic.count)
    automatic.insert(contentsOf: suggestions, at: insertionIndex)
    return automatic
}
