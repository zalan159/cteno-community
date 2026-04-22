import { useState, useEffect, useCallback } from 'react';
import { machineListAgents, machineCreateAgent, machineDeleteAgent } from '../sync/ops';
import type { AgentConfig } from '../sync/storageTypes';

interface UseAgentsOptions {
    machineId: string | undefined;
    /**
     * Polling interval in milliseconds
     * @default 30000 (30 seconds)
     */
    pollingInterval?: number;
}

interface UseAgentsReturn {
    agents: AgentConfig[];
    loading: boolean;
    error: string | null;
    createAgent: (params: {
        id: string;
        name: string;
        description?: string;
        instructions?: string;
        model?: string;
        allowed_tools?: string[];
        excluded_tools?: string[];
        scope?: 'global' | 'workspace';
        workdir?: string;
    }) => Promise<{ success: boolean; id?: string; error?: string }>;
    deleteAgent: (id: string) => Promise<void>;
    refresh: () => Promise<void>;
}

/**
 * Hook to manage agents for a machine.
 * Fetches agent configs via RPC and polls for updates.
 */
export function useAgents(options: UseAgentsOptions): UseAgentsReturn {
    const { machineId, pollingInterval = 30000 } = options;
    const [agents, setAgents] = useState<AgentConfig[]>([]);
    const [loading, setLoading] = useState(true);
    const [error, setError] = useState<string | null>(null);

    const fetchAgents = useCallback(async (isInitial = false) => {
        if (!machineId) {
            setAgents([]);
            setLoading(false);
            return;
        }

        try {
            if (isInitial) {
                setLoading(true);
            }
            setError(null);

            const result = await Promise.race([
                machineListAgents(machineId),
                new Promise<AgentConfig[]>((_, reject) =>
                    setTimeout(() => reject(new Error('Request timeout')), 15000)
                ),
            ]);
            setAgents(result);
        } catch (err) {
            console.error('Failed to fetch agents:', err);
            setError(err instanceof Error ? err.message : 'Unknown error');
        } finally {
            setLoading(false);
        }
    }, [machineId]);

    const createAgent = useCallback(async (params: {
        id: string;
        name: string;
        description?: string;
        instructions?: string;
        model?: string;
        allowed_tools?: string[];
        excluded_tools?: string[];
        scope?: 'global' | 'workspace';
        workdir?: string;
    }): Promise<{ success: boolean; id?: string; error?: string }> => {
        if (!machineId) return { success: false, error: 'No machine connected' };

        try {
            const result = await machineCreateAgent(machineId, params);
            if (!result.success) {
                throw new Error(result.error || 'Failed to create agent');
            }
            await fetchAgents();
            return result;
        } catch (err) {
            console.error('Failed to create agent:', err);
            throw err;
        }
    }, [machineId, fetchAgents]);

    const deleteAgent = useCallback(async (id: string) => {
        if (!machineId) return;

        try {
            const result = await machineDeleteAgent(machineId, id);
            if (!result.success) {
                throw new Error(result.error || 'Failed to delete agent');
            }
            // Optimistic update
            setAgents(prev => prev.filter(a => a.id !== id));
            // Refresh in background
            fetchAgents().catch((err) => {
                console.warn('Failed to refresh agents after delete:', err);
            });
        } catch (err) {
            console.error('Failed to delete agent:', err);
            throw err;
        }
    }, [machineId, fetchAgents]);

    // Initial fetch
    useEffect(() => {
        if (machineId) {
            fetchAgents(true);
        }
    }, [machineId]);

    // Polling
    useEffect(() => {
        if (!machineId) return;

        const interval = setInterval(() => {
            fetchAgents();
        }, pollingInterval);

        return () => clearInterval(interval);
    }, [machineId, pollingInterval, fetchAgents]);

    return {
        agents,
        loading,
        error,
        createAgent,
        deleteAgent,
        refresh: fetchAgents,
    };
}
