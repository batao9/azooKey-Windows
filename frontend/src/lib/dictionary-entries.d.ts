export type DictionaryEntry = { reading: string; word: string };
export type DictionaryRowErrors = Partial<Record<keyof DictionaryEntry, string>>;
export declare const MAX_ENTRIES: number;
export declare const normalizeDictionaryEntries: (value: unknown) => DictionaryEntry[];
export declare const validateDictionaryEntries: (draft: DictionaryEntry[]) => {
    entries: DictionaryEntry[];
    errors: DictionaryRowErrors[];
};
export declare const isDictionaryDirty: (
    draft: DictionaryEntry[],
    persisted: DictionaryEntry[] | null,
) => boolean;
