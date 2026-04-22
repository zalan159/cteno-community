import { useState, useEffect, useCallback, useMemo } from 'react';
import { machineGetOrchestrationFlow } from '../sync/ops';
import type { OrchestrationFlow } from '../sync/storageTypes';

interface UseOrchestrationFlowOptions {
    personaId: string | undefined;
    machineId: string | undefined;
    /** Polling interval in ms (default 5000) */
    pollingInterval?: number;
}

interface UseOrchestrationFlowReturn {
    flow: OrchestrationFlow | null;
    loading: boolean;
    refresh: () => Promise<void>;
}

/**
 * Hook to fetch and poll an orchestration flow for a persona.
 *
 * Polls every 5s when there are running nodes, every 30s otherwise.
 */
export function useOrchestrationFlow(options: UseOrchestrationFlowOptions): UseOrchestrationFlowReturn {
    const { personaId, machineId, pollingInterval = 5000 } = options;
    const [flow, setFlow] = useState<OrchestrationFlow | null>(null);
    const [loading, setLoading] = useState(false);

    const fetchFlow = useCallback(async () => {
        if (!machineId || !personaId) {
            setFlow(null);
            return;
        }

        try {
            setLoading(true);
            const result = await machineGetOrchestrationFlow(machineId, personaId);
            setFlow(result);
        } catch (err) {
            console.warn('Failed to fetch orchestration flow:', err);
        } finally {
            setLoading(false);
        }
    }, [machineId, personaId]);

    // Check if there are active nodes that need fast polling
    const hasActiveNodes = useMemo(() => {
        if (!flow) return false;
        return flow.nodes.some(n => n.status === 'running' || n.status === 'pending');
    }, [flow]);

    // Initial fetch
    useEffect(() => {
        if (machineId && personaId) {
            fetchFlow();
        }
    }, [machineId, personaId]);

    // Polling: fast when active, slow background otherwise
    useEffect(() => {
        if (!machineId || !personaId) return;

        // If no flow was ever returned, use slow polling
        const interval = hasActiveNodes ? pollingInterval : 30000;
        const timer = setInterval(() => {
            fetchFlow();
        }, interval);

        return () => clearInterval(timer);
    }, [machineId, personaId, hasActiveNodes, pollingInterval, fetchFlow]);

    return {
        flow,
        loading,
        refresh: fetchFlow,
    };
}
