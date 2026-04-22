import { useCallback, useEffect, useRef, useState } from 'react';

import { apiSocket } from './apiSocket';
import {
    machineGetBackgroundTask,
    machineListBackgroundTasks,
} from './ops';
import type { BackgroundTaskRecord } from './ops';

const DEFAULT_POLL_MS = 3000;

type BackgroundTaskUpdate = {
    sessionId?: string;
};

type BackgroundTaskUpdateListener = (task: BackgroundTaskUpdate) => void;

const updateListeners = new Set<BackgroundTaskUpdateListener>();
let removeBackgroundTaskUpdateHandler: (() => void) | null = null;

function normalizeBackgroundTaskUpdate(payload: unknown): BackgroundTaskUpdate {
    if (!payload || typeof payload !== 'object') {
        return {};
    }

    return {
        sessionId: typeof (payload as { sessionId?: unknown }).sessionId === 'string'
            ? (payload as { sessionId: string }).sessionId
            : undefined,
    };
}

function subscribeToBackgroundTaskUpdates(listener: BackgroundTaskUpdateListener): () => void {
    updateListeners.add(listener);

    if (!removeBackgroundTaskUpdateHandler) {
        removeBackgroundTaskUpdateHandler = apiSocket.onMessage('background-task-update', (payload) => {
            const normalizedPayload = normalizeBackgroundTaskUpdate(payload);
            updateListeners.forEach((currentListener) => {
                currentListener(normalizedPayload);
            });
        });
    }

    return () => {
        updateListeners.delete(listener);
        if (updateListeners.size === 0 && removeBackgroundTaskUpdateHandler) {
            removeBackgroundTaskUpdateHandler();
            removeBackgroundTaskUpdateHandler = null;
        }
    };
}

export type { BackgroundTaskRecord } from './ops';
export { machineListBackgroundTasks, machineGetBackgroundTask };

export function useBackgroundTasks(
    machineId: string,
    sessionId?: string,
    opts?: { pollMs?: number }
): {
    tasks: BackgroundTaskRecord[];
    loading: boolean;
    refresh: () => Promise<void>;
} {
    const [tasks, setTasks] = useState<BackgroundTaskRecord[]>([]);
    const [loading, setLoading] = useState(Boolean(machineId));
    const mountedRef = useRef(true);
    const requestIdRef = useRef(0);
    const pollMs = opts?.pollMs ?? DEFAULT_POLL_MS;

    useEffect(() => {
        return () => {
            mountedRef.current = false;
        };
    }, []);

    const refresh = useCallback(async () => {
        if (!machineId) {
            if (mountedRef.current) {
                setTasks([]);
                setLoading(false);
            }
            return;
        }

        const requestId = ++requestIdRef.current;
        if (mountedRef.current) {
            setLoading(true);
        }

        try {
            const nextTasks = await machineListBackgroundTasks(machineId, sessionId ? { sessionId } : undefined);
            if (!mountedRef.current || requestId !== requestIdRef.current) {
                return;
            }
            setTasks(nextTasks);
        } finally {
            if (mountedRef.current && requestId === requestIdRef.current) {
                setLoading(false);
            }
        }
    }, [machineId, sessionId]);

    useEffect(() => {
        mountedRef.current = true;
        void refresh();
    }, [refresh]);

    useEffect(() => {
        if (!machineId || !sessionId || pollMs <= 0) {
            return;
        }

        const interval = setInterval(() => {
            void refresh();
        }, pollMs);

        return () => clearInterval(interval);
    }, [machineId, pollMs, refresh, sessionId]);

    useEffect(() => {
        if (!machineId) {
            return;
        }

        return subscribeToBackgroundTaskUpdates((task) => {
            if (sessionId && task.sessionId !== sessionId) {
                return;
            }
            void refresh();
        });
    }, [machineId, refresh, sessionId]);

    return {
        tasks,
        loading,
        refresh,
    };
}
