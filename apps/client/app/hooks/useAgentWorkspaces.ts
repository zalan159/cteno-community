import React from 'react';
import { machineListAgentWorkspaces } from '@/sync/ops';
import type { WorkspaceSummary } from '@/sync/storageTypes';
import { storage } from '@/sync/storage';
import { sync } from '@/sync/sync';

interface UseAgentWorkspacesOptions {
    machineId: string | undefined;
    pollingInterval?: number;
}

export function useAgentWorkspaces(options: UseAgentWorkspacesOptions) {
    const { machineId, pollingInterval = 30000 } = options;
    const [workspaces, setWorkspaces] = React.useState<WorkspaceSummary[]>([]);
    const [loading, setLoading] = React.useState(false);

    const refresh = React.useCallback(async () => {
        if (!machineId) {
            setWorkspaces([]);
            setLoading(false);
            return;
        }
        setLoading(true);
        try {
            const result = await machineListAgentWorkspaces(machineId);
            setWorkspaces(result);
            storage.getState().applyAgentWorkspaces(result);
            const knownSessions = storage.getState().sessions;
            const hasMissingMemberSessions = result.some((workspace) =>
                workspace.members.some((member) => !knownSessions[member.sessionId])
            );
            if (hasMissingMemberSessions) {
                sync.refreshSessions().catch((error) => {
                    console.warn('Failed to refresh unified sessions after workspace sync:', error);
                });
            }
        } finally {
            setLoading(false);
        }
    }, [machineId]);

    React.useEffect(() => {
        if (!machineId) return;
        refresh();
    }, [machineId, refresh]);

    React.useEffect(() => {
        if (!machineId) return;
        const timer = setInterval(() => {
            refresh().catch((error) => {
                console.warn('Failed to refresh workspaces:', error);
            });
        }, pollingInterval);
        return () => clearInterval(timer);
    }, [machineId, pollingInterval, refresh]);

    return {
        workspaces,
        loading,
        refresh,
    };
}
