import { useEffect, useRef, useState } from "react";
import * as Dialog from "@radix-ui/react-dialog";
import { invoke } from "@tauri-apps/api/core";
import { Trash2 } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
    AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent,
    AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { UnsavedChangesGuard } from "@/components/unsaved-changes-guard";
import { saveConfigWithToast } from "@/lib/config";

export type RomajiRow = { input: string; output: string; next_input: string };
const emptyRow = (): RomajiRow => ({ input: "", output: "", next_input: "" });
export const normalizeRomajiRows = (rows: unknown): RomajiRow[] => {
    if (rows === undefined) return [];
    if (!Array.isArray(rows)) throw new Error("ローマ字テーブルの形式が不正です");
    return rows.map((row) => {
        if (!row || typeof row.input !== "string" || typeof row.output !== "string" ||
            (row.next_input !== undefined && typeof row.next_input !== "string")) {
            throw new Error("ローマ字テーブルの形式が不正です");
        }
        return { input: row.input, output: row.output, next_input: row.next_input ?? "" };
    });
};
const trimmedRow = (row: RomajiRow): RomajiRow => ({
    input: row.input.trim(), output: row.output.trim(), next_input: row.next_input.trim(),
});
const hasContent = (row: RomajiRow) => !!(row.input || row.output || row.next_input);

export function RomajiEditor({ rows, onSaved }: { rows: RomajiRow[]; onSaved: (rows: RomajiRow[]) => void }) {
    const [open, setOpen] = useState(false);
    const [draft, setDraft] = useState<RomajiRow[]>([]);
    const [baseline, setBaseline] = useState("");
    const [confirmation, setConfirmation] = useState<"discard" | "reset" | null>(null);
    const [busy, setBusy] = useState(false);
    const busyRef = useRef(false);
    const [validate, setValidate] = useState(false);
    const [focusNewRow, setFocusNewRow] = useState(false);
    const inputRefs = useRef<(HTMLInputElement | null)[]>([]);
    const outputRefs = useRef<(HTMLInputElement | null)[]>([]);
    const dirty = open && JSON.stringify(draft) !== baseline;
    const errors = draft.map((row) => {
        const trimmed = trimmedRow(row);
        return hasContent(trimmed) ? {
            input: trimmed.input ? "" : "入力を記入してください",
            output: trimmed.output ? "" : "出力を記入してください",
        } : { input: "", output: "" };
    });

    useEffect(() => {
        if (!open || !focusNewRow) return;
        const frame = requestAnimationFrame(() => {
            const input = inputRefs.current[draft.length - 1];
            input?.scrollIntoView({ block: "nearest" });
            input?.focus();
            setFocusNewRow(false);
        });
        return () => cancelAnimationFrame(frame);
    }, [open, focusNewRow, draft.length]);

    const requestClose = () => {
        if (busyRef.current) return;
        if (dirty) setConfirmation("discard");
        else setOpen(false);
    };
    const updateRow = (index: number, key: keyof RomajiRow, value: string) => {
        if (busyRef.current) return;
        setDraft((previous) => previous.map((row, rowIndex) => rowIndex === index ? { ...row, [key]: value } : row));
    };
    const reset = async () => {
        if (busyRef.current) return;
        busyRef.current = true;
        setBusy(true);
        try {
            const defaults = normalizeRomajiRows(await invoke("get_default_romaji_rows"));
            if (!defaults.length) throw new Error("初期テーブルが空です");
            setDraft(defaults);
            setValidate(false);
        } catch { toast("初期テーブルの取得に失敗しました"); }
        finally { busyRef.current = false; setBusy(false); }
    };
    const save = async () => {
        if (busyRef.current) return;
        setValidate(true);
        if (errors.some((row) => row.input || row.output)) {
            const index = errors.findIndex((row) => row.input || row.output);
            (errors[index].input ? inputRefs : outputRefs).current[index]?.focus();
            return;
        }
        busyRef.current = true;
        setBusy(true);
        try {
            const normalized = draft.map(trimmedRow).filter(hasContent);
            const data = await saveConfigWithToast((config) => {
                config.romaji_table = config.romaji_table ?? {};
                config.romaji_table.rows = normalized;
            });
            if (data) {
                onSaved(normalizeRomajiRows(data.romaji_table?.rows));
                setOpen(false);
                toast("ローマ字テーブルを更新しました");
            }
        } catch { toast("ローマ字テーブルを更新できませんでした"); }
        finally { busyRef.current = false; setBusy(false); }
    };

    return (
        <>
            <UnsavedChangesGuard dirty={dirty} />
            <Dialog.Root open={open} onOpenChange={(nextOpen) => {
                if (!nextOpen) { requestClose(); return; }
                const initial = rows.length ? rows.map((row) => ({ ...row })) : [emptyRow()];
                setDraft(initial);
                setBaseline(JSON.stringify(initial));
                setValidate(false);
                setFocusNewRow(false);
                setOpen(true);
            }}>
                <Dialog.Trigger asChild><Button variant="outline">編集</Button></Dialog.Trigger>
                <Dialog.Portal>
                    <Dialog.Overlay className="fixed inset-0 z-50 bg-black/50" />
                    <Dialog.Content className="fixed left-1/2 top-1/2 z-50 flex h-[min(85dvh,800px)] w-[calc(100%-1rem)] max-w-5xl -translate-x-1/2 -translate-y-1/2 flex-col overflow-hidden rounded-lg border bg-background p-3 shadow-lg sm:p-5"
                        onEscapeKeyDown={(event) => { event.preventDefault(); if (!confirmation) requestClose(); }}
                        onPointerDownOutside={(event) => event.preventDefault()}>
                        <div className="shrink-0 pb-3">
                            <Dialog.Title className="text-base font-semibold">ローマ字テーブル</Dialog.Title>
                            <Dialog.Description className="sr-only">ローマ字の入力と出力、次の入力を編集します。</Dialog.Description>
                        </div>
                        <fieldset disabled={busy} className="m-0 min-h-0 min-w-0 flex-1 overflow-auto rounded-md border p-0" aria-busy={busy}>
                            <table className="w-full min-w-[540px] table-fixed text-sm">
                                <colgroup><col className="w-10" /><col /><col /><col /><col className="w-12" /></colgroup>
                                <thead className="sticky top-0 z-10 bg-background text-left text-xs text-muted-foreground">
                                    <tr>
                                        <th scope="col" className="px-2 py-2">#</th>
                                        <th scope="col" className="px-2 py-2">入力</th>
                                        <th scope="col" className="px-2 py-2">出力</th>
                                        <th scope="col" className="px-2 py-2">
                                            次の入力
                                            <p id="next-input-help" className="mt-1 font-normal">変換後に残す入力（例: kk → っ、k）</p>
                                        </th>
                                        <th scope="col" className="px-2 py-2"><span className="sr-only">削除</span></th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {draft.map((row, index) => (
                                        <tr key={index} className="border-t">
                                            <td className="px-2 py-2 text-xs text-muted-foreground">{index + 1}</td>
                                            {(["input", "output", "next_input"] as const).map((key) => {
                                                const error = validate && key !== "next_input" ? errors[index][key] : "";
                                                const errorId = `romaji-${index}-${key}-error`;
                                                return <td key={key} className="px-2 py-2 align-top">
                                                    <Input value={row[key]} aria-label={`${index + 1}行目 ${key === "input" ? "入力" : key === "output" ? "出力" : "次の入力"}`}
                                                        aria-invalid={!!error} aria-describedby={error ? errorId : key === "next_input" ? "next-input-help" : undefined}
                                                        spellCheck={false} autoComplete="off"
                                                        ref={key === "input" ? (element) => { inputRefs.current[index] = element; }
                                                            : key === "output" ? (element) => { outputRefs.current[index] = element; } : undefined}
                                                        onChange={(event) => updateRow(index, key, event.target.value)} />
                                                    {error && <p id={errorId} className="mt-1 text-xs text-destructive" role="alert">{error}</p>}
                                                </td>;
                                            })}
                                            <td className="px-1 py-2 align-top">
                                                <Button variant="ghost" size="icon" aria-label={`${index + 1}行目を削除`} onClick={() => {
                                                    if (busyRef.current) return;
                                                    setDraft((previous) => previous.length === 1 ? [emptyRow()] : previous.filter((_, rowIndex) => rowIndex !== index));
                                                }}><Trash2 className="h-4 w-4" /></Button>
                                            </td>
                                        </tr>
                                    ))}
                                </tbody>
                            </table>
                        </fieldset>
                        <p className="settings-description shrink-0 pt-2" role="status">
                            {draft.length} 件{busy ? "・処理中…" : dirty ? "・未保存の変更" : ""}
                        </p>
                        <div className="grid shrink-0 grid-cols-2 gap-2 pt-3 sm:flex sm:flex-wrap" aria-busy={busy}>
                            <Button variant="outline" disabled={busy} onClick={() => { if (!busyRef.current) setConfirmation("reset"); }}>初期値に戻す</Button>
                            <Button variant="secondary" disabled={busy} onClick={() => {
                                if (busyRef.current) return;
                                setDraft((previous) => [...previous, emptyRow()]);
                                setFocusNewRow(true);
                            }}>行を追加</Button>
                            <Button variant="outline" className="sm:ml-auto" disabled={busy} onClick={requestClose}>キャンセル</Button>
                            <Button disabled={busy} onClick={() => void save()}>{busy ? "処理中…" : "保存"}</Button>
                        </div>
                        <AlertDialog open={confirmation !== null} onOpenChange={(nextOpen) => { if (!nextOpen) setConfirmation(null); }}>
                            <AlertDialogContent>
                                <AlertDialogHeader>
                                    <AlertDialogTitle>{confirmation === "reset" ? "初期値に戻しますか？" : "変更を破棄しますか？"}</AlertDialogTitle>
                                    <AlertDialogDescription>{confirmation === "reset" ? "編集中の内容を初期テーブルに置き換えます。保存するまでは設定に反映されません。" : "保存していない変更があります。"}</AlertDialogDescription>
                                </AlertDialogHeader>
                                <AlertDialogFooter>
                                    <AlertDialogCancel>編集を続ける</AlertDialogCancel>
                                    <AlertDialogAction onClick={() => {
                                        if (confirmation === "reset") void reset();
                                        else setOpen(false);
                                        setConfirmation(null);
                                    }}>{confirmation === "reset" ? "初期値に戻す" : "破棄する"}</AlertDialogAction>
                                </AlertDialogFooter>
                            </AlertDialogContent>
                        </AlertDialog>
                    </Dialog.Content>
                </Dialog.Portal>
            </Dialog.Root>
        </>
    );
}
