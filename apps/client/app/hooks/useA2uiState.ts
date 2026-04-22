/**
 * Hook for fetching and subscribing to A2UI state updates.
 */
import { useState, useEffect, useCallback } from 'react';
import { machineGetA2uiState } from '@/sync/ops';
import { onHypothesisPush } from '@/sync/sync';
import type { A2uiState } from '@/a2ui/types';

export function useA2uiState({
  machineId,
  agentId,
}: {
  machineId: string | null;
  agentId: string | null;
}) {
  const [surfaces, setSurfaces] = useState<A2uiState>([]);
  const [loading, setLoading] = useState(false);

  const fetchState = useCallback(async () => {
    if (!machineId || !agentId) return;
    setLoading(true);
    try {
      const result = await machineGetA2uiState(machineId, agentId);
      if (result) setSurfaces(result);
    } finally {
      setLoading(false);
    }
  }, [machineId, agentId]);

  // Initial fetch
  useEffect(() => {
    fetchState();
  }, [fetchState]);

  // Subscribe to a2ui_updated push events
  useEffect(() => {
    if (!agentId) return;
    return onHypothesisPush((pushAgentId, event) => {
      if (pushAgentId === agentId && event === 'a2ui_updated') {
        fetchState();
      }
    });
  }, [agentId, fetchState]);

  return { surfaces, loading, refresh: fetchState };
}
