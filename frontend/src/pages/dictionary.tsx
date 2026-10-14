import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { toast } from "sonner";
import { Plus, Save, Trash2 } from "lucide-react";

import { UnsavedChangesGuard } from "@/components/unsaved-changes-guard";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { getConfigAfterPendingUpdates, saveConfigWithToast } from "@/lib/config";
import {
    MAX_ENTRIES,
    normalizeDictionaryEntries,
    validateDictionaryEntries,
    isDictionaryDirty,
    type DictionaryEntry,
} from "@/lib/dictionary-entries.js";

export const Dictionary = () => {
    const [entries, setEntries] = useState<DictionaryEntry[]>([]);
    const [persisted, setPersisted] = useState<DictionaryEntry[] | null>(null);
    const [isLoading, setIsLoading] = useState(true);
    const [loadFailed, setLoadFailed] = useState(false);
    const [loadAttempt, setLoadAttempt] = useState(0);
    const [isSaving, setIsSaving] = useState(false);
    const savingRef = useRef(false);
    const [validationAttempted, setValidationAttempted] = useState(false);
    const [pendingFocus, setPendingFocus] = useState<{
        index: number;
        field: keyof DictionaryEntry;
        end?: boolean;
    } | null>(null);
    const listRef = useRef<HTMLDivElement>(null);
    const inputRefs = useRef<Array<Partial<Record<keyof DictionaryEntry, HTMLInputElement | null>>>>([]);
    const savedView = useRef<{
        element: HTMLElement | null;
        selectionStart: number | null;
        selectionEnd: number | null;
        top: number;
        left: number;
    } | null>(null);
    const validation = validateDictionaryEntries(entries);
    const dirty = isDictionaryDirty(entries, persisted);
    const disabled = isLoading || loadFailed || persisted === null || isSaving;

    useEffect(() => {
        let active = true;
        setIsLoading(true);
        setLoadFailed(false);
        getConfigAfterPendingUpdates()
            .then((data) => {
                if (!active) return;
                const loaded = normalizeDictionaryEntries(data.user_dictionary?.entries);
                setEntries(loaded);
                setPersisted(loaded);
            })
            .catch(() => {
                if (active) setLoadFailed(true);
            })
            .finally(() => {
                if (active) setIsLoading(false);
            });
        return () => { active = false; };
    }, [loadAttempt]);

    useLayoutEffect(() => {
        if (!pendingFocus) return;
        const input = inputRefs.current[pendingFocus.index]?.[pendingFocus.field];
        const list = listRef.current;
        if (input && list) {
            input.focus({ preventScroll: true });
            if (pendingFocus.end) {
                list.scrollTop = list.scrollHeight;
            } else {
                const bounds = input.getBoundingClientRect();
                const viewport = list.getBoundingClientRect();
                const headerHeight = list.querySelector("thead")?.getBoundingClientRect().height ?? 0;
                if (bounds.top < viewport.top + headerHeight) {
                    list.scrollTop += bounds.top - viewport.top - headerHeight;
                } else if (bounds.bottom > viewport.bottom) {
                    list.scrollTop += bounds.bottom - viewport.bottom;
                }
            }
        }
        setPendingFocus(null);
    }, [pendingFocus]);

    useLayoutEffect(() => {
        if (isSaving || !savedView.current) return;
        const view = savedView.current;
        savedView.current = null;
        if (view.element?.isConnected &&
            (document.activeElement === document.body || document.activeElement === view.element)) {
            view.element.focus({ preventScroll: true });
            if (view.element instanceof HTMLInputElement) {
                view.element.setSelectionRange(view.selectionStart, view.selectionEnd);
            }
        }
        if (listRef.current) {
            listRef.current.scrollTop = view.top;
            listRef.current.scrollLeft = view.left;
        }
    }, [isSaving]);

    const setEntryValue = (index: number, key: keyof DictionaryEntry, value: string) => {
        if (disabled || savingRef.current) return;
        setEntries((prev) => prev.map((entry, row) => row === index ? { ...entry, [key]: value } : entry));
    };

    const addEntry = () => {
        if (disabled || savingRef.current || entries.length >= MAX_ENTRIES) return;
        setEntries((prev) => [...prev, { reading: "", word: "" }]);
        setPendingFocus({ index: entries.length, field: "reading", end: true });
    };

    const removeEntry = (index: number) => {
        if (disabled || savingRef.current) return;
        setEntries((prev) => prev.filter((_, rowIndex) => rowIndex !== index));
    };

    const saveEntries = async () => {
        if (disabled || savingRef.current) return;
        setValidationAttempted(true);
        const invalidIndex = validation.errors.findIndex((error) => error.reading || error.word);
        if (invalidIndex !== -1) {
            setPendingFocus({
                index: invalidIndex,
                field: validation.errors[invalidIndex].reading ? "reading" : "word",
            });
            return;
        }

        savingRef.current = true;
        const element = document.activeElement instanceof HTMLElement ? document.activeElement : null;
        savedView.current = {
            element,
            selectionStart: element instanceof HTMLInputElement ? element.selectionStart : null,
            selectionEnd: element instanceof HTMLInputElement ? element.selectionEnd : null,
            top: listRef.current?.scrollTop ?? 0,
            left: listRef.current?.scrollLeft ?? 0,
        };
        setIsSaving(true);
        try {
            const config = await saveConfigWithToast((config) => {
                config.user_dictionary = config.user_dictionary ?? {};
                config.user_dictionary.entries = validation.entries;
            }, "ユーザ辞書の保存に失敗しました");
            if (!config) return;
            const saved = normalizeDictionaryEntries(config.user_dictionary?.entries);
            setEntries(saved);
            setPersisted(saved);
            setValidationAttempted(false);
            toast("ユーザ辞書を保存しました");
        } catch (_error) {
            toast("ユーザ辞書の保存に失敗しました");
        } finally {
            savingRef.current = false;
            setIsSaving(false);
        }
    };

    return (
        <div className="h-full min-h-0 flex flex-col">
            <UnsavedChangesGuard dirty={dirty} />
            <div ref={listRef} className="min-h-0 flex-1 overflow-auto rounded-md border">
                {isLoading ? (
                    <p className="settings-description p-4" role="status">読み込み中…</p>
                ) : loadFailed ? (
                    <div className="space-y-3 p-4">
                        <p className="text-sm text-destructive" role="alert">辞書設定の読み込みに失敗しました。</p>
                        <Button variant="secondary" onClick={() => setLoadAttempt((attempt) => attempt + 1)}>再試行</Button>
                    </div>
                ) : entries.length === 0 ? (
                    <p className="settings-description p-4">登録がありません。「行を追加」から登録できます。</p>
                ) : (
                    <table className="w-full table-fixed text-sm">
                        <colgroup>
                            <col />
                            <col />
                            <col className="w-14" />
                        </colgroup>
                        <thead className="sticky top-0 z-10 bg-background text-left text-xs text-muted-foreground">
                            <tr>
                                <th scope="col" className="px-3 py-2 font-medium">読み</th>
                                <th scope="col" className="px-3 py-2 font-medium">単語</th>
                                <th scope="col" className="px-2 py-2 text-center font-medium">削除</th>
                            </tr>
                        </thead>
                        <tbody>
                            {entries.map((entry, index) => (
                                <tr key={`row-${index}`} className="border-t">
                                    {(["reading", "word"] as const).map((field) => {
                                        const error = validationAttempted ? validation.errors[index][field] : undefined;
                                        const errorId = `dictionary-${index}-${field}-error`;
                                        const label = field === "reading" ? "読み" : "単語";
                                        return (
                                            <td key={field} className="px-3 py-2 align-top">
                                                <Input
                                                    ref={(element) => {
                                                        inputRefs.current[index] ??= {};
                                                        inputRefs.current[index][field] = element;
                                                    }}
                                                    value={entry[field]}
                                                    placeholder={field === "reading" ? "よみ" : "単語"}
                                                    aria-label={`${index + 1} 行目の${label}`}
                                                    aria-invalid={Boolean(error)}
                                                    aria-describedby={error ? errorId : undefined}
                                                    disabled={disabled}
                                                    onChange={(event) => setEntryValue(index, field, event.target.value)}
                                                />
                                                {error && <p id={errorId} className="mt-1 text-xs text-destructive">{error}</p>}
                                            </td>
                                        );
                                    })}
                                    <td className="px-2 py-2 text-center align-top">
                                        <Button
                                            variant="ghost"
                                            size="icon"
                                            onClick={() => removeEntry(index)}
                                            disabled={disabled}
                                            aria-label={`${index + 1} 行目を削除`}
                                        >
                                            <Trash2 className="h-4 w-4" />
                                        </Button>
                                    </td>
                                </tr>
                            ))}
                        </tbody>
                    </table>
                )}
            </div>
            <div className="settings-actions flex shrink-0 items-center gap-3 pt-3">
                <span className="text-sm tabular-nums text-muted-foreground">{entries.length}/{MAX_ENTRIES}</span>
                <span className="settings-description" role="status">{isSaving ? "保存中" : dirty ? "未保存の変更" : ""}</span>
                <div className="ml-auto flex gap-2">
                    <Button variant="secondary" onClick={addEntry} disabled={disabled || entries.length >= MAX_ENTRIES}>
                        <Plus className="h-4 w-4" />
                        行を追加
                    </Button>
                    <Button onClick={saveEntries} disabled={disabled}>
                        <Save className="h-4 w-4" />
                        保存
                    </Button>
                </div>
            </div>
        </div>
    );
};
