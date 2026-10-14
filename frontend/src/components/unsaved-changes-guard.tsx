import { useEffect, useRef, useState } from "react";
import { useBlocker } from "react-router";
import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { toast } from "sonner";
import {
    AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent,
    AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle,
} from "@/components/ui/alert-dialog";

export function UnsavedChangesGuard({ dirty }: { dirty: boolean }) {
    const blocker = useBlocker(dirty);
    const [closing, setClosing] = useState(false);
    const dirtyRef = useRef(dirty);
    dirtyRef.current = dirty;

    useEffect(() => {
        if (!isTauri()) return;
        let disposed = false;
        let unlisten: (() => void) | undefined;
        void getCurrentWindow().onCloseRequested((event) => {
            if (disposed || dirtyRef.current) {
                event.preventDefault();
                if (!disposed) setClosing(true);
            }
        }).then((stop) => {
            if (disposed) stop();
            else unlisten = stop;
        }).catch(() => toast("ウィンドウ終了の確認を設定できませんでした"));
        return () => { disposed = true; unlisten?.(); };
    }, []);

    useEffect(() => {
        if (!dirty) return;
        const onBeforeUnload = (event: BeforeUnloadEvent) => {
            event.preventDefault();
            event.returnValue = "";
        };
        window.addEventListener("beforeunload", onBeforeUnload);
        return () => window.removeEventListener("beforeunload", onBeforeUnload);
    }, [dirty]);

    const cancel = () => {
        setClosing(false);
        if (blocker.state === "blocked") blocker.reset();
    };

    return (
        <AlertDialog open={closing || blocker.state === "blocked"}>
            <AlertDialogContent onEscapeKeyDown={(event) => { event.preventDefault(); cancel(); }}>
                <AlertDialogHeader>
                    <AlertDialogTitle>変更を破棄しますか？</AlertDialogTitle>
                    <AlertDialogDescription>保存していない変更があります。</AlertDialogDescription>
                </AlertDialogHeader>
                <AlertDialogFooter>
                    <AlertDialogCancel onClick={cancel}>編集を続ける</AlertDialogCancel>
                    <AlertDialogAction onClick={() => {
                        if (closing) {
                            void getCurrentWindow().destroy().catch(() => toast("ウィンドウを閉じられませんでした"));
                        } else if (blocker.state === "blocked") blocker.proceed();
                    }}>
                        {closing ? "破棄して閉じる" : "破棄して移動"}
                    </AlertDialogAction>
                </AlertDialogFooter>
            </AlertDialogContent>
        </AlertDialog>
    );
}
