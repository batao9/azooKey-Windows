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
]
let maxUserDictionaryEntryCount = 50
let minInputCountForZenzaiCandidates = 4
let minHiraganaCountForZenzaiCandidates = 2

private struct AppSettings: Decodable {
    let zenzai: ZenzaiSettings?
    let user_dictionary: UserDictionarySettings?
    let romaji_table: RomajiTableSettings?
}

private struct ZenzaiSettings: Decodable {
    let enable: Bool?
    let profile: String?
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
        print("Failed to apply custom romaji table: \(error)")
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

@MainActor func getOptions(context: String = "") -> ConvertRequestOptions {
    getOptions(context: context, zenzaiEnabled: (config["enable"] as? Bool) ?? false)
}

@MainActor func getOptions(
    context: String = "",
    zenzaiEnabled: Bool
) -> ConvertRequestOptions {
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
                    profile: config["profile"] as! String,
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
    let previousZenzaiEnabled = (config["enable"] as? Bool) ?? false
    let previousProfile = (config["profile"] as? String) ?? ""
    let previousUsedCustomRomajiTable = customRomajiTableURL != nil
    var dynamicUserDictionary: [DicdataElement] = []
    defer {
        converter.sendToDicdataStore(.importDynamicUserDict(dynamicUserDictionary))
    }

    config["enable"] = false
    config["profile"] = ""
    setRoman2KanaInputStyle()

    if let appDataPath = ProcessInfo.processInfo.environment["APPDATA"] {
        let settingsPath = URL(filePath: appDataPath).appendingPathComponent("Azookey/settings.json")
        
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
            }

            let isZenzaiEnabled = (config["enable"] as? Bool) ?? false
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
                print("User dictionary entries are truncated to \(maxUserDictionaryEntryCount).")
            }
        } catch {
            print("Failed to read settings: \(error)")
        }
    }

    let currentZenzaiEnabled = (config["enable"] as? Bool) ?? false
    let currentProfile = (config["profile"] as? String) ?? ""
    let currentUsedCustomRomajiTable = customRomajiTableURL != nil
    if previousZenzaiEnabled != currentZenzaiEnabled
        || previousProfile != currentProfile
        || previousUsedCustomRomajiTable != currentUsedCustomRomajiTable
    {
        converter.stopComposition()
        composingText = ComposingText()
        composingTextSnapshots.removeAll()
    }
}

@_silgen_name("Initialize")
@MainActor public func initialize(
    path: UnsafePointer<CChar>,
    use_zenzai: Bool
) {
    let path = String(cString: path)
    execURL = URL(filePath: path)

    load_config()

    composingText.insertAtCursorPosition("a", inputStyle: currentInputStyle)
    let useZenzaiForWarmup = effectiveZenzaiEnabledForCandidates(
        isConfigured: (config["enable"] as? Bool) ?? false,
        inputCount: composingText.input.count,
        hiraganaCount: composingText.convertTarget.count
    )
    converter.requestCandidates(
        composingText,
        options: getOptions(zenzaiEnabled: useZenzaiForWarmup)
    )
    composingText = ComposingText()
    composingTextSnapshots.removeAll()
}

@_silgen_name("AppendText")
@MainActor public func append_text(
    input: UnsafePointer<CChar>,
    cursorPtr: UnsafeMutablePointer<Int>
) -> UnsafeMutablePointer<CChar> {
    let inputString = String(cString: input)
    composingText.insertAtCursorPosition(inputString, inputStyle: currentInputStyle)

    cursorPtr.pointee = composingText.convertTargetCursorPosition    
    return _strdup(composingText.convertTarget)!
}

@_silgen_name("AppendTextDirect")
@MainActor public func append_text_direct(
    input: UnsafePointer<CChar>,
    cursorPtr: UnsafeMutablePointer<Int>
) -> UnsafeMutablePointer<CChar> {
    let inputString = String(cString: input)
    composingText.insertAtCursorPosition(inputString, inputStyle: .direct)

    cursorPtr.pointee = composingText.convertTargetCursorPosition
    return _strdup(composingText.convertTarget)!
}

@_silgen_name("RemoveText")
@MainActor public func remove_text(
    cursorPtr: UnsafeMutablePointer<Int>
) -> UnsafeMutablePointer<CChar> {
    composingText.deleteBackwardFromCursorPosition(count: 1)

    cursorPtr.pointee = composingText.convertTargetCursorPosition
    return _strdup(composingText.convertTarget)!
}

@_silgen_name("MoveCursor")
@MainActor public func move_cursor(
    offset: Int32,
    cursorPtr: UnsafeMutablePointer<Int>
) -> UnsafeMutablePointer<CChar> {
    if offset == 125 {
        composingTextSnapshots.removeAll()
        cursorPtr.pointee = composingText.convertTargetCursorPosition
        return _strdup(composingText.convertTarget)!
    }

    if offset == 126 {
        composingTextSnapshots.append(composingText)
        cursorPtr.pointee = composingText.convertTargetCursorPosition
        return _strdup(composingText.convertTarget)!
    }

    if offset == 127 {
        if let restored = composingTextSnapshots.popLast() {
            composingText = restored
        }
        cursorPtr.pointee = composingText.convertTargetCursorPosition
        return _strdup(composingText.convertTarget)!
    }

    let cursor = composingText.moveCursorFromCursorPosition(count: Int(offset))
    print("offset: \(offset), cursor: \(cursor)")

    cursorPtr.pointee = cursor
    return _strdup(composingText.convertTarget)!
}

@_silgen_name("ClearText")
@MainActor public func clear_text() {
    composingText = ComposingText()
    composingTextSnapshots.removeAll()
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
    let useZenzai = effectiveZenzaiEnabledForCandidates(
        isConfigured: (config["enable"] as? Bool) ?? false,
        inputCount: composingText.input.count,
        hiraganaCount: hiragana.count
    )
    let options = getOptions(context: contextString, zenzaiEnabled: useZenzai)
    let converted = converter.requestCandidates(composingText, options: options)
    var result: [FFICandidate] = []

    for i in 0..<converted.mainResults.count {
        let candidate = converted.mainResults[i]

        let text = strdup(constructCandidateString(candidate: candidate, hiragana: hiragana))
        let hiragana = strdup(hiragana)
        let correspondingCount = resolveCorrespondingCount(
            composingText: composingText,
            candidateComposingCount: candidate.composingCount,
            isZenzaiEnabled: useZenzai
        )
        let subtext = strdup(
            resolveSubtext(
                composingText: composingText,
                correspondingCount: correspondingCount,
                isZenzaiEnabled: useZenzai
            )
        )

        result.append(FFICandidate(text: text, subtext: subtext, hiragana: hiragana, correspondingCount: Int32(correspondingCount)))
    }

    lengthPtr.pointee = result.count

    return to_list_pointer(result)
}

@_silgen_name("GetComposedTextForCursorPrefix")
@MainActor public func get_composed_text_for_cursor_prefix(lengthPtr: UnsafeMutablePointer<Int>) -> UnsafeMutablePointer<UnsafeMutablePointer<FFICandidate>?> {
    let hiragana = composingText.convertTarget
    let suffixAfterCursor = String(hiragana.dropFirst(composingText.convertTargetCursorPosition))
    let prefixComposingText = composingText.prefixToCursorPosition()
    let prefixHiragana = prefixComposingText.convertTarget
    let contextString = (config["context"] as? String) ?? ""
    let useZenzai = effectiveZenzaiEnabledForCandidates(
        isConfigured: (config["enable"] as? Bool) ?? false,
        inputCount: prefixComposingText.input.count,
        hiraganaCount: prefixHiragana.count
    )
    let options = getOptions(context: contextString, zenzaiEnabled: useZenzai)
    let converted = converter.requestCandidates(prefixComposingText, options: options)
    var result: [FFICandidate] = []

    for i in 0..<converted.mainResults.count {
        let candidate = converted.mainResults[i]

        let text = strdup(constructCandidateString(candidate: candidate, hiragana: prefixHiragana))
        let hiragana = strdup(hiragana)
        let correspondingCount = resolveCorrespondingCount(
            composingText: prefixComposingText,
            candidateComposingCount: candidate.composingCount,
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

        result.append(FFICandidate(text: text, subtext: subtext, hiragana: hiragana, correspondingCount: Int32(correspondingCount)))
    }

    lengthPtr.pointee = result.count

    return to_list_pointer(result)
}

@_silgen_name("ShrinkText")
@MainActor public func shrink_text(
    offset: Int32
) -> UnsafeMutablePointer<CChar>  {
    var afterComposingText = composingText
    afterComposingText.prefixComplete(composingCount: .inputCount(Int(offset)))
    composingText = afterComposingText

    return _strdup(composingText.convertTarget)!
}

@_silgen_name("SetContext")
@MainActor public func set_context(
    context: UnsafePointer<CChar>
) {
    let contextString = String(cString: context)
    config["context"] = contextString
}
