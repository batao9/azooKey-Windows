import Foundation
import KanaKanjiConverterModule

/// Actor-owned, single-resource cache. Returned values are immutable Sendable
/// snapshots, so background warmup can reuse them without a lock or another read.
struct EmojiTextReplacerCache {
    private var dictionaryURL: URL?
    private var replacer = TextReplacer.empty

    mutating func value(for dictionaryURL: URL) -> TextReplacer {
        if self.dictionaryURL != dictionaryURL {
            replacer = TextReplacer(emojiDataProvider: { dictionaryURL })
            self.dictionaryURL = dictionaryURL
        }
        return replacer
    }

    /// TextReplacer reports read failures as an empty value. Retry such loads,
    /// and refresh changed resource contents, on explicit configuration reload.
    mutating func invalidate() {
        dictionaryURL = nil
        replacer = .empty
    }
}
