import React, { useState, useEffect, useCallback } from 'react';
import { machineListPersonas, machineCreatePersona, machineUpdatePersona, machineDeletePersona, machineGetPersonaTasks } from '../sync/ops';
import type { Persona, PersonaTaskSummary } from '../sync/storageTypes';
import type { VendorName } from '../sync/ops';
import { loadCachedVendorDefaultModelId } from '../sync/modelCatalogCache';
import { storage, useCachedPersonas } from '../sync/storage';
import { sync } from '../sync/sync';
import { frontendLog } from '@/utils/tauri';

interface UsePersonasOptions {
    machineId: string | undefined;
    /**
     * Polling interval in milliseconds
     * @default 30000 (30 seconds)
     */
    pollingInterval?: number;
}

interface UsePersonasReturn {
    personas: Persona[];
    loading: boolean;
    error: string | null;
    createPersona: (params: {
        name?: string;
        description?: string;
        model?: string;
        avatarId?: string;
        modelId?: string;
        workdir?: string;
        agent?: VendorName;
    }) => Promise<Persona | null>;
    updatePersona: (params: {
        id: string;
        name?: string;
        description?: string;
        model?: string;
        avatarId?: string;
        modelId?: string;
        continuousBrowsing?: boolean;
    }) => Promise<void>;
    deletePersona: (id: string) => Promise<void>;
    getPersonaTasks: (personaId: string) => Promise<PersonaTaskSummary[]>;
    refresh: () => Promise<void>;
}

/**
 * Hook to manage personas for a machine.
 * Returns cached personas immediately while fetching fresh data via RPC.
 */
export function usePersonas(options: UsePersonasOptions): UsePersonasReturn {
    const { machineId, pollingInterval = 30000 } = options;
    const personas = useCachedPersonas();
    const [loading, setLoading] = useState(personas.length === 0);
    const [error, setError] = useState<string | null>(null);

    const fetchPersonas = useCallback(async (isInitial = false) => {
        if (!machineId) {
            storage.getState().applyPersonas([]);
            setLoading(false);
            return;
        }

        try {
            if (isInitial && storage.getState().cachedPersonas.length === 0) {
                setLoading(true);
            }
            setError(null);

            const _t0 = Date.now();
            const result = await Promise.race([
                machineListPersonas(machineId),
                new Promise<Persona[]>((_, reject) =>
                    setTimeout(() => reject(new Error('Request timeout')), 15000)
                ),
            ]);
            frontendLog(`📡 usePersonas: RPC returned ${result.length} personas (${Date.now() - _t0}ms)`);
            storage.getState().applyPersonas(result);
        } catch (err) {
            console.error('Failed to fetch personas:', err);
            setError(err instanceof Error ? err.message : 'Unknown error');
        } finally {
            setLoading(false);
        }
    }, [machineId]);

    const createPersona = useCallback(async (params: {
        name?: string;
        description?: string;
        model?: string;
        avatarId?: string;
        modelId?: string;
        workdir?: string;
        agent?: VendorName;
    }): Promise<Persona | null> => {
        if (!machineId) return null;

        try {
            const resolvedModelId = params.modelId
                || (params.agent && params.agent !== 'cteno'
                    ? loadCachedVendorDefaultModelId(machineId, params.agent)
                    : null)
                || undefined;
            const result = await machineCreatePersona(machineId, {
                ...params,
                ...(resolvedModelId ? { modelId: resolvedModelId } : {}),
            });
            if (!result.success) {
                throw new Error(result.error || 'Failed to create persona');
            }
            if (result.persona?.chatSessionId?.startsWith('pending-')) {
                sync.registerPendingPersonaSession(result.persona, machineId, {
                    pendingSessionId: result.pendingSessionId,
                    attemptId: result.attemptId,
                    vendor: result.persona.agent ?? params.agent,
                });
            }
            await fetchPersonas();
            return result.persona || null;
        } catch (err) {
            console.error('Failed to create persona:', err);
            throw err;
        }
    }, [machineId, fetchPersonas]);

    const updatePersona = useCallback(async (params: {
        id: string;
        name?: string;
        description?: string;
        model?: string;
        avatarId?: string;
        modelId?: string;
        continuousBrowsing?: boolean;
    }) => {
        if (!machineId) return;

        try {
            const result = await machineUpdatePersona(machineId, params);
            if (!result.success) {
                throw new Error(result.error || 'Failed to update persona');
            }
            await fetchPersonas();
        } catch (err) {
            console.error('Failed to update persona:', err);
            throw err;
        }
    }, [machineId, fetchPersonas]);

    const deletePersona = useCallback(async (id: string) => {
        if (!machineId) return;

        try {
            const result = await machineDeletePersona(machineId, id);
            if (!result.success) {
                throw new Error(result.error || 'Failed to delete persona');
            }
            // Optimistic update
            const updated = storage.getState().cachedPersonas.filter((p) => p.id !== id);
            storage.getState().applyPersonas(updated);
            // Refresh in background
            fetchPersonas().catch((err) => {
                console.warn('Failed to refresh personas after delete:', err);
            });
        } catch (err) {
            console.error('Failed to delete persona:', err);
            throw err;
        }
    }, [machineId, fetchPersonas]);

    const getPersonaTasks = useCallback(async (personaId: string): Promise<PersonaTaskSummary[]> => {
        if (!machineId) return [];
        return machineGetPersonaTasks(machineId, personaId);
    }, [machineId]);

    // Initial fetch
    useEffect(() => {
        if (machineId) {
            fetchPersonas(true);
        }
    }, [machineId]);

    // Polling
    useEffect(() => {
        if (!machineId) return;

        const interval = setInterval(() => {
            fetchPersonas();
        }, pollingInterval);

        return () => clearInterval(interval);
    }, [machineId, pollingInterval, fetchPersonas]);

    return {
        personas,
        loading,
        error,
        createPersona,
        updatePersona,
        deletePersona,
        getPersonaTasks,
        refresh: fetchPersonas,
    };
}
