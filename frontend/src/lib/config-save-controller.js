export const createSerialTaskQueue = () => {
    let tail = Promise.resolve();

    return (task) => {
        const result = tail.then(task, task);
        tail = result.then(
            () => undefined,
            () => undefined,
        );
        return result;
    };
};

export const createDebouncedSaver = ({
    save,
    onStateChange = () => {},
    delayMs = 500,
}) => {
    let timer = null;
    let pending = null;
    let revision = 0;
    let disposed = false;

    const clearTimer = () => {
        if (timer !== null) {
            clearTimeout(timer);
            timer = null;
        }
    };

    const runSave = ({ value, revision: requestRevision }, notify) => {
        if (notify) {
            onStateChange("saving");
        }

        let saveResult;
        try {
            saveResult = save(value);
        } catch (error) {
            saveResult = Promise.reject(error);
        }

        return Promise.resolve(saveResult)
            .then(
                (result) => {
                    if (
                        notify &&
                        !disposed &&
                        requestRevision === revision
                    ) {
                        onStateChange(result === null ? "error" : "saved");
                    }
                    return result;
                },
                (error) => {
                    if (
                        notify &&
                        !disposed &&
                        requestRevision === revision
                    ) {
                        onStateChange("error");
                    }
                    throw error;
                },
            );
    };

    const flush = () => {
        clearTimer();
        if (pending === null) {
            return Promise.resolve(undefined);
        }

        const request = pending;
        pending = null;
        return runSave(request, !disposed);
    };

    return {
        schedule(value) {
            disposed = false;
            revision += 1;
            pending = { value, revision };
            clearTimer();
            onStateChange("dirty");
            timer = setTimeout(() => {
                timer = null;
                void flush().catch(() => undefined);
            }, delayMs);
        },
        flush,
        resume() {
            disposed = false;
        },
        dispose() {
            clearTimer();
            disposed = true;
            if (pending === null) {
                return Promise.resolve(undefined);
            }

            const request = pending;
            pending = null;
            return runSave(request, false);
        },
    };
};
