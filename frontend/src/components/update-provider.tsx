import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { toast } from "sonner";

type UpdateCheck = {
    current_version: string;
    latest_version: string;
    release_url: string;
    update_available: boolean;
};
type UpdateState = {
    version: string | null;
    result: UpdateCheck | null;
    status: "idle" | "checking" | "starting" | "launched";
    error: string | null;
    check: () => Promise<void>;
    start: () => Promise<void>;
};
const UpdateContext = createContext<UpdateState | null>(null);

export function UpdateProvider({ children }: { children: ReactNode }) {
    const [version, setVersion] = useState<string | null>(null);
    const [result, setResult] = useState<UpdateCheck | null>(null);
    const [status, setStatus] = useState<UpdateState["status"]>("idle");
    const [error, setError] = useState<string | null>(null);
    const busy = useRef(false);
    const checkedOnStartup = useRef(false);

    const check = useCallback(async () => {
        if (busy.current) return;
        busy.current = true;
        setStatus("checking");
        setError(null);
        try {
            setResult(await invoke<UpdateCheck>("check_for_updates"));
        } catch (error) {
            // Keep a known available update when a later check fails.
            setError(String(error));
        } finally {
            busy.current = false;
            setStatus("idle");
        }
    }, []);

    const start = useCallback(async () => {
        if (busy.current) return;
        busy.current = true;
        setStatus("starting");
        setError(null);
        try {
            await invoke("start_update");
            // The helper can still be running; keep this session locked until restart.
            setStatus("launched");
            toast("アップデートを開始しました");
        } catch (error) {
            busy.current = false;
            setStatus("idle");
            setError(String(error));
            toast("アップデートを開始できませんでした");
        }
    }, []);

    useEffect(() => {
        if (checkedOnStartup.current) return;
        checkedOnStartup.current = true;
        void getVersion().then(setVersion).catch(() => setVersion(null));
        void check();
    }, [check]);

    return <UpdateContext.Provider value={{ version, result, status, error, check, start }}>{children}</UpdateContext.Provider>;
}

export function useUpdates() {
    const value = useContext(UpdateContext);
    if (!value) throw new Error("UpdateProvider is required");
    return value;
}
