import { useState, useEffect, useCallback } from 'react';
import { machineListScheduledTasks, machineToggleScheduledTask, machineDeleteScheduledTask, machineUpdateScheduledTask } from '../sync/ops';
import type { ScheduledTask, UpdateScheduledTaskInput } from '../sync/ops';

interface UseScheduledTasksOptions {
    machineId: string | undefined;
    /**
     * Polling interval in milliseconds
     * @default 30000 (30 seconds)
     */
    pollingInterval?: number;
}

interface UseScheduledTasksReturn {
    tasks: ScheduledTask[];
    loading: boolean;
    error: string | null;
    toggleTask: (id: string, enabled: boolean) => Promise<void>;
    deleteTask: (id: string) => Promise<void>;
    updateTask: (id: string, updates: UpdateScheduledTaskInput) => Promise<void>;
    refresh: () => Promise<void>;
}

/**
 * Hook to manage scheduled tasks for a machine
 *
 * Polls for updates at a slower interval (30s) since scheduled tasks
 * change less frequently than SubAgents.
 */
export function useScheduledTasks(options: UseScheduledTasksOptions): UseScheduledTasksReturn {
    const { machineId, pollingInterval = 30000 } = options;
    const [tasks, setTasks] = useState<ScheduledTask[]>([]);
    const [loading, setLoading] = useState(false);
    const [error, setError] = useState<string | null>(null);

    const fetchTasks = useCallback(async () => {
        if (!machineId) {
            setTasks([]);
            return;
        }

        try {
            setLoading(true);
            setError(null);

            // Add timeout to prevent infinite hanging if RPC never returns
            const result = await Promise.race([
                machineListScheduledTasks(machineId),
                new Promise<ScheduledTask[]>((_, reject) =>
                    setTimeout(() => reject(new Error('Request timeout')), 15000)
                ),
            ]);
            setTasks(result);
        } catch (err) {
            console.error('Failed to fetch scheduled tasks:', err);
            setError(err instanceof Error ? err.message : 'Unknown error');
        } finally {
            setLoading(false);
        }
    }, [machineId]);

    const toggleTask = useCallback(async (id: string, enabled: boolean) => {
        if (!machineId) return;

        try {
            const result = await machineToggleScheduledTask(machineId, id, enabled);
            if (!result.success) {
                throw new Error(result.error || 'Failed to toggle task');
            }
            await fetchTasks();
        } catch (err) {
            console.error('Failed to toggle scheduled task:', err);
            throw err;
        }
    }, [machineId, fetchTasks]);

    const deleteTask = useCallback(async (id: string) => {
        if (!machineId) return;

        try {
            const result = await machineDeleteScheduledTask(machineId, id);
            if (!result.success) {
                throw new Error(result.error || 'Failed to delete task');
            }
            // Optimistically remove from UI so stale fetches don't leave a ghost row.
            setTasks((prev) => prev.filter((task) => task.id !== id));
            fetchTasks().catch((err) => {
                console.warn('Failed to refresh scheduled tasks after delete:', err);
            });
        } catch (err) {
            console.error('Failed to delete scheduled task:', err);
            throw err;
        }
    }, [machineId, fetchTasks]);

    const updateTask = useCallback(async (id: string, updates: UpdateScheduledTaskInput) => {
        if (!machineId) return;

        try {
            const result = await machineUpdateScheduledTask(machineId, id, updates);
            if (!result.success) {
                throw new Error(result.error || 'Failed to update task');
            }
            await fetchTasks();
        } catch (err) {
            console.error('Failed to update scheduled task:', err);
            throw err;
        }
    }, [machineId, fetchTasks]);

    // Initial fetch
    useEffect(() => {
        if (machineId) {
            fetchTasks();
        }
    }, [machineId]);

    // Polling
    useEffect(() => {
        if (!machineId) return;

        const interval = setInterval(() => {
            fetchTasks();
        }, pollingInterval);

        return () => clearInterval(interval);
    }, [machineId, pollingInterval, fetchTasks]);

    return {
        tasks,
        loading,
        error,
        toggleTask,
        deleteTask,
        updateTask,
        refresh: fetchTasks,
    };
}
