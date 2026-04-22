import * as React from 'react';
import { SessionListViewItem, useSessionListViewData, useSetting, useLocalSetting } from '@/sync/storage';

export function useVisibleSessionListViewData(): SessionListViewItem[] | null {
    const data = useSessionListViewData();
    const hideInactiveSessions = useSetting('hideInactiveSessions');
    const selectedMachineIdFilter = useLocalSetting('selectedMachineIdFilter');

    return React.useMemo(() => {
        if (!data) {
            return data;
        }

        const filtered: SessionListViewItem[] = [];
        let pendingProjectGroup: SessionListViewItem | null = null;

        for (const item of data) {
            if (item.type === 'project-group') {
                // Filter project-group by machine
                if (selectedMachineIdFilter && item.machine.id !== selectedMachineIdFilter) {
                    continue;
                }
                pendingProjectGroup = item;
                continue;
            }

            if (item.type === 'session') {
                // Filter by machine
                if (selectedMachineIdFilter && item.session.metadata?.machineId !== selectedMachineIdFilter) {
                    continue;
                }

                // Filter by active status
                if (hideInactiveSessions && !item.session.active) {
                    continue;
                }

                if (pendingProjectGroup) {
                    filtered.push(pendingProjectGroup);
                    pendingProjectGroup = null;
                }
                filtered.push(item);
                continue;
            }

            pendingProjectGroup = null;

            if (item.type === 'active-sessions') {
                // Filter active sessions by machine
                if (selectedMachineIdFilter) {
                    const filteredSessions = item.sessions.filter(
                        s => s.metadata?.machineId === selectedMachineIdFilter
                    );
                    if (filteredSessions.length > 0) {
                        filtered.push({ ...item, sessions: filteredSessions });
                    }
                } else {
                    filtered.push(item);
                }
            }
        }

        return filtered;
    }, [data, hideInactiveSessions, selectedMachineIdFilter]);
}
