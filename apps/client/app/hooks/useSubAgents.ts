import { useState, useEffect, useCallback, useMemo } from 'react';
import { machineListSubAgents, machineStopSubAgent } from '../sync/ops';
import type { SubAgent } from '../sync/ops';

interface UseSubAgentsOptions {
    sessionId: string;
    machineId: string | undefined;
    /**
     * Polling interval in milliseconds
     * @default 5000 (5 seconds)
     */
    pollingInterval?: number;
    /**
     * Only show active SubAgents (running or pending)
     * @default false
     */
    activeOnly?: boolean;
}

interface UseSubAgentsReturn {
    subagents: SubAgent[];
    loading: boolean;
    error: string | null;
    stopSubAgent: (id: string) => Promise<void>;
    refresh: () => Promise<void>;
}

/**
 * Hook to manage SubAgent data for a session
 *
 * Automatically polls for updates while SubAgents are active
 */
export function useSubAgents(options: UseSubAgentsOptions): UseSubAgentsReturn {
    const { sessionId, machineId, pollingInterval = 5000, activeOnly = false } = options;
    const [subagents, setSubagents] = useState<SubAgent[]>([]);
    const [loading, setLoading] = useState(false);
    const [error, setError] = useState<string | null>(null);

    // Fetch SubAgents from machine
    const fetchSubagents = useCallback(async () => {
        if (!machineId) {
            setSubagents([]);
            return;
        }

        try {
            setLoading(true);
            setError(null);

            const result = await machineListSubAgents(machineId, {
                parentSessionId: sessionId,
                activeOnly,
            });

            setSubagents(result);
        } catch (err) {
            console.error('Failed to fetch subagents:', err);
            setError(err instanceof Error ? err.message : 'Unknown error');
        } finally {
            setLoading(false);
        }
    }, [machineId, sessionId, activeOnly]);

    // Stop a SubAgent
    const stopSubAgent = useCallback(async (id: string) => {
        if (!machineId) return;

        try {
            const result = await machineStopSubAgent(machineId, id);
            if (!result.success) {
                throw new Error(result.error || 'Failed to stop SubAgent');
            }

            // Refresh list immediately
            await fetchSubagents();
        } catch (err) {
            console.error('Failed to stop subagent:', err);
            throw err;
        }
    }, [machineId, fetchSubagents]);

    // Check if there are active SubAgents that need polling
    const hasActiveSubAgents = useMemo(() => {
        return subagents.some(
            sa => sa.status === 'running' || sa.status === 'pending'
        );
    }, [subagents]);

    // Initial fetch
    useEffect(() => {
        if (machineId) {
            fetchSubagents();
        }
    }, [machineId, sessionId]);

    // Polling: fast when active SubAgents exist, slow background polling otherwise
    // (background polling catches Scheduler-spawned SubAgents that the frontend doesn't know about)
    useEffect(() => {
        if (!machineId) return;

        const interval = hasActiveSubAgents ? pollingInterval : 30000;
        const timer = setInterval(() => {
            fetchSubagents();
        }, interval);

        return () => clearInterval(timer);
    }, [machineId, hasActiveSubAgents, pollingInterval, fetchSubagents]);

    return {
        subagents,
        loading,
        error,
        stopSubAgent,
        refresh: fetchSubagents,
    };
}

/**
 * Get filtered SubAgents that should be displayed
 *
 * Shows:
 * - All running/pending SubAgents
 * - Recently completed/failed SubAgents (within last 5 minutes)
 */
export function getDisplayableSubAgents(subagents: SubAgent[]): SubAgent[] {
    const now = Date.now();
    const fiveMinutesAgo = now - 5 * 60 * 1000;

    return subagents.filter(sa => {
        // Always show running/pending
        if (sa.status === 'running' || sa.status === 'pending') {
            return true;
        }

        // Show recently completed/failed/stopped
        if (sa.status === 'completed' || sa.status === 'failed' || sa.status === 'stopped' || sa.status === 'timed_out') {
            return (sa.completed_at || 0) > fiveMinutesAgo;
        }

        return false;
    });
}
