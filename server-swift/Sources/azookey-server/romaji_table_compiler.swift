import Foundation

struct RomajiTableRow: Decodable {
    let input: String
    let output: String
    let next_input: String?
}

private struct NormalizedRomajiRow {
    let input: String
    let output: String
    let nextInput: String
}

private enum RomajiTableEntrySource {
    case explicit
    case generated
}

func normalizeRomajiCell(_ value: String) -> String {
    value
        .replacingOccurrences(of: "\t", with: "")
        .replacingOccurrences(of: "\n", with: "")
        .replacingOccurrences(of: "\r", with: "")
        .trimmingCharacters(in: .whitespacesAndNewlines)
}

private func escapeInputTableToken(_ value: String) -> String {
    var escaped = ""
    escaped.reserveCapacity(value.count)

    for character in value {
        switch character {
        case "{":
            escaped.append("{lbracket}")
        case "}":
            escaped.append("{rbracket}")
        default:
            escaped.append(character)
        }
    }

    return escaped
}

func buildCustomRomajiTableEntries(rows: [RomajiTableRow]) -> [(key: String, value: String)] {
    let normalizedRows: [NormalizedRomajiRow] = rows.compactMap { row in
        let input = normalizeRomajiCell(row.input)
        let output = normalizeRomajiCell(row.output)
        let nextInput = normalizeRomajiCell(row.next_input ?? "")

        guard !input.isEmpty, !output.isEmpty else {
            return nil
        }

        return NormalizedRomajiRow(input: input, output: output, nextInput: nextInput)
    }

    if normalizedRows.isEmpty {
        return []
    }

    let explicitInputs = Set(normalizedRows.map(\.input))
    let delayedInputs: Set<String> = Set(normalizedRows.compactMap { row in
        guard row.nextInput.isEmpty else {
            return nil
        }

        return normalizedRows.contains(where: { other in
            other.input.count > row.input.count && other.input.hasPrefix(row.input)
        }) ? row.input : nil
    })

    var entries: [(key: String, value: String)] = []
    var indexByKey: [String: Int] = [:]
    var sourceByKey: [String: RomajiTableEntrySource] = [:]

    func upsertEntry(
        rawKey: String,
        rawValue: String,
        source: RomajiTableEntrySource,
        literalEscaping: Bool = true
    ) {
        guard !rawKey.isEmpty, !rawValue.isEmpty else {
            return
        }

        let key = literalEscaping ? escapeInputTableToken(rawKey) : rawKey
        let value = literalEscaping ? escapeInputTableToken(rawValue) : rawValue
        guard !key.isEmpty, !value.isEmpty else {
            return
        }

        if let index = indexByKey[key] {
            let currentSource = sourceByKey[key] ?? .generated
            if currentSource == .explicit && source == .generated {
                return
            }
            entries[index] = (key: key, value: value)
            sourceByKey[key] = source
            return
        }

        indexByKey[key] = entries.count
        sourceByKey[key] = source
        entries.append((key: key, value: value))
    }

    for base in normalizedRows where !base.nextInput.isEmpty {
        for longer in normalizedRows where longer.input.count > base.input.count && longer.input.hasPrefix(base.input) {
            let suffix = String(longer.input.dropFirst(base.input.count))
            let key = base.output + base.nextInput + suffix
            let value = longer.output + longer.nextInput
            upsertEntry(rawKey: key, rawValue: value, source: .generated)
        }

        for follow in normalizedRows where follow.input.hasPrefix(base.nextInput) {
            let suffix = String(follow.input.dropFirst(base.nextInput.count))
            let key = base.input + suffix
            let value = base.output + follow.output + follow.nextInput
            upsertEntry(rawKey: key, rawValue: value, source: .generated)
        }
    }

    for base in normalizedRows where delayedInputs.contains(base.input) {
        let escapedInput = escapeInputTableToken(base.input)
        let escapedOutput = escapeInputTableToken(base.output)

        // If a delayed-commit prefix is followed by a single-character rule
        // (e.g. "-" -> "ー"), prefer concrete mapping such as "n-" -> "んー".
        for follow in normalizedRows where follow.input.count == 1 {
            upsertEntry(
                rawKey: base.input + follow.input,
                rawValue: base.output + follow.output + follow.nextInput,
                source: .generated
            )
        }

        upsertEntry(
            rawKey: "\(escapedInput){composition-separator}",
            rawValue: escapedOutput,
            source: .generated,
            literalEscaping: false
        )
        upsertEntry(
            rawKey: "\(escapedInput){any-0x00}",
            rawValue: "\(escapedOutput){any-0x00}",
            source: .generated,
            literalEscaping: false
        )

        var intermediatePrefixes: Set<String> = []
        for longer in normalizedRows where longer.input.count > base.input.count && longer.input.hasPrefix(base.input) {
            let longerChars = Array(longer.input)
            if longerChars.count <= base.input.count + 1 {
                continue
            }

            for prefixLength in (base.input.count + 1)..<longerChars.count {
                intermediatePrefixes.insert(String(longerChars.prefix(prefixLength)))
            }
        }

        for prefix in intermediatePrefixes.sorted(by: {
            if $0.count == $1.count {
                return $0 < $1
            }
            return $0.count < $1.count
        }) where !explicitInputs.contains(prefix) {
            upsertEntry(rawKey: prefix, rawValue: prefix, source: .generated)
        }
    }

    for row in normalizedRows where !delayedInputs.contains(row.input) {
        upsertEntry(
            rawKey: row.input,
            rawValue: row.output + row.nextInput,
            source: .explicit
        )
    }

    return entries
}

func buildCustomRomajiTableContent(rows: [RomajiTableRow]) -> String? {
    let entries = buildCustomRomajiTableEntries(rows: rows)
    guard !entries.isEmpty else {
        return nil
    }

    return entries
        .map { "\($0.key)\t\($0.value)" }
        .joined(separator: "\n")
}
