import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { RefreshCcw, Server } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";

export const DebugSettings = () => {
    const [isRestartingServer, setIsRestartingServer] = useState(false);

    const restartServer = async () => {
        if (isRestartingServer) {
            return;
        }

        setIsRestartingServer(true);
        try {
            await invoke("restart_server");
            toast("サーバーを再起動しました");
        } catch (error) {
            const message = error instanceof Error ? error.message : String(error);
            toast("サーバーの再起動に失敗しました", {
                description: message,
                duration: 10000,
            });
        } finally {
            setIsRestartingServer(false);
        }
    };

    return (
        <section className="space-y-3">
            <h1 className="text-sm font-bold text-foreground">デバッグ用設定</h1>
            <div className="flex items-center gap-4 rounded-md border p-4">
                <Server />
                <div className="flex-1 space-y-1">
                    <p className="text-sm font-medium leading-none">サーバー再起動</p>
                    <p className="text-xs text-muted-foreground">
                        変換サーバーを停止して起動し直します
                    </p>
                </div>
                <Button
                    variant="secondary"
                    onClick={() => void restartServer()}
                    disabled={isRestartingServer}
                >
                    <RefreshCcw />
                    {isRestartingServer ? "再起動中" : "サーバー再起動"}
                </Button>
            </div>
        </section>
    );
};
