export type ConfigSaveState = "dirty" | "saving" | "saved" | "error";

export declare const createSerialTaskQueue: () => <T>(
    task: () => T | Promise<T>,
) => Promise<T>;

export declare const createDebouncedSaver: <T, R>({
    save,
    onStateChange,
    delayMs,
}: {
    save: (value: T) => R | Promise<R>;
    onStateChange?: (state: ConfigSaveState) => void;
    delayMs?: number;
}) => {
    schedule: (value: T) => void;
    flush: () => Promise<R | undefined>;
    resume: () => void;
    dispose: () => Promise<R | undefined>;
};
