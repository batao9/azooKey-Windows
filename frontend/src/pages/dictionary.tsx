import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { toast } from "sonner";
import { Plus, Save, Trash2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

type DictionaryEntry = {
    reading: string;
    word: string;
};

const MAX_ENTRIES = 50;

const normalizeDictionaryEntries = (value?: unknown): DictionaryEntry[] => {
    if (!Array.isArray(value)) {
        return [];
    }

    return value
        .map((entry) => {
            if (!entry || typeof entry !== "object") {
                return null;
            }
            const record = entry as Record<string, unknown>;
            if (typeof record.reading !== "string" || typeof record.word !== "string") {
                return null;
            }
            return {
                reading: record.reading,
                word: record.word,
            };
        })
        .filter((entry): entry is DictionaryEntry => entry !== null);
};

const trimDictionaryEntries = (entries: DictionaryEntry[]): DictionaryEntry[] =>
    entries.map((entry) => ({
        reading: entry.reading.trim(),
        word: entry.word.trim(),
    }));

export const Dictionary = () => {
    const [entries, setEntries] = useState<DictionaryEntry[]>([]);
    const [isLoading, setIsLoading] = useState(true);
    const [isSaving, setIsSaving] = useState(false);

    useEffect(() => {
        invoke<any>("get_config")
            .then((data) => {
                setEntries(
                    normalizeDictionaryEntries(data.user_dictionary?.entries),
                );
            })
            .catch(() => {
                toast("辞書設定の読み込みに失敗しました");
            })
            .finally(() => {
                setIsLoading(false);
            });
    }, []);

    const setEntryValue = (
        index: number,
        key: keyof DictionaryEntry,
        value: string,
    ) => {
        setEntries((prev) => {
            const next = [...prev];
            next[index] = { ...next[index], [key]: value };
            return next;
        });
    };

    const addEntry = () => {
        if (entries.length >= MAX_ENTRIES) {
            toast(`ユーザ辞書は最大 ${MAX_ENTRIES} 件までです`);
            return;
        }
        setEntries((prev) => [...prev, { reading: "", word: "" }]);
    };

    const removeEntry = (index: number) => {
        setEntries((prev) => prev.filter((_, rowIndex) => rowIndex !== index));
    };

    const saveEntries = async () => {
        if (isSaving) {
            return;
        }

        const normalized = trimDictionaryEntries(entries);
        if (normalized.some((entry) => !entry.reading || !entry.word)) {
            toast("読みと単語の両方を入力してください");
            return;
        }

        if (normalized.length > MAX_ENTRIES) {
            toast(`ユーザ辞書は最大 ${MAX_ENTRIES} 件までです`);
            return;
        }

        const unique = new Set(
            normalized.map((entry) => `${entry.reading}\u0000${entry.word}`),
        );
        if (unique.size !== normalized.length) {
            toast("同じ読み・単語の組み合わせが重複しています");
            return;
        }

        setIsSaving(true);
        try {
            const config = await invoke<any>("get_config");
            config.user_dictionary = config.user_dictionary ?? {};
            config.user_dictionary.entries = normalized;
            await invoke("update_config", { newConfig: config });
            setEntries(normalized);
            toast("ユーザ辞書を保存しました");
        } catch (_error) {
            toast("ユーザ辞書の保存に失敗しました");
        } finally {
            setIsSaving(false);
        }
    };

    return (
        <div className="space-y-6">
            <section className="space-y-2">
                <h1 className="text-sm font-bold text-foreground">ユーザ辞書</h1>
                <p className="text-sm text-muted-foreground">
                    読みと単語を登録できます（最大 {MAX_ENTRIES} 件）。
                </p>
            </section>

            <section className="space-y-3 rounded-md border p-4">
                <div className="flex flex-wrap items-center gap-2">
                    <p className="text-sm font-medium">登録件数: {entries.length} 件</p>
                    <div className="ml-auto flex w-full justify-end gap-2 sm:w-auto">
                        <Button variant="secondary" onClick={addEntry} disabled={entries.length >= MAX_ENTRIES || isLoading}>
                            <Plus className="h-4 w-4" />
                            追加
                        </Button>
                        <Button onClick={saveEntries} disabled={isLoading || isSaving}>
                            <Save className="h-4 w-4" />
                            保存
                        </Button>
                    </div>
                </div>

                {entries.length === 0 ? (
                    <p className="text-sm text-muted-foreground">
                        まだ登録がありません。右上の「追加」から作成してください。
                    </p>
                ) : (
                    <div className="overflow-x-auto rounded-md border">
                        <table className="w-full min-w-[560px] text-sm">
                            <thead className="bg-muted/30 text-left text-xs text-muted-foreground">
                                <tr>
                                    <th className="px-3 py-2 font-medium">読み</th>
                                    <th className="px-3 py-2 font-medium">単語</th>
                                    <th className="w-20 px-3 py-2 text-right font-medium">操作</th>
                                </tr>
                            </thead>
                            <tbody>
                                {entries.map((entry, index) => (
                                    <tr key={`row-${index}`} className="border-t">
                                        <td className="px-3 py-2">
                                            <Input
                                                value={entry.reading}
                                                placeholder="よみ"
                                                onChange={(event) =>
                                                    setEntryValue(index, "reading", event.target.value)
                                                }
                                            />
                                        </td>
                                        <td className="px-3 py-2">
                                            <Input
                                                value={entry.word}
                                                placeholder="単語"
                                                onChange={(event) =>
                                                    setEntryValue(index, "word", event.target.value)
                                                }
                                            />
                                        </td>
                                        <td className="px-3 py-2 text-right">
                                            <Button
                                                variant="ghost"
                                                size="icon"
                                                onClick={() => removeEntry(index)}
                                                aria-label="行を削除"
                                            >
                                                <Trash2 className="h-4 w-4" />
                                            </Button>
                                        </td>
                                    </tr>
                                ))}
                            </tbody>
                        </table>
                    </div>
                )}
            </section>
        </div>
    );
};
