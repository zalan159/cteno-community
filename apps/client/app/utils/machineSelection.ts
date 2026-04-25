import type { Machine, Session } from '@/sync/storageTypes';
import { isMachineOnline } from '@/utils/machineUtils';

export function resolveVisibleMachineId(
    selectedMachineIdFilter: string | null | undefined,
    machines: Machine[],
    sessions: Session[],
): string | undefined {
    if (selectedMachineIdFilter && machines.some((machine) => machine.id === selectedMachineIdFilter)) {
        return selectedMachineIdFilter;
    }

    const machineIdsWithSessions = new Set<string>();
    for (const session of sessions) {
        const machineId = session.metadata?.machineId;
        if (machineId) machineIdsWithSessions.add(machineId);
    }

    for (const machineId of machineIdsWithSessions) {
        if (machines.some((machine) => machine.id === machineId && isMachineOnline(machine))) {
            return machineId;
        }
    }

    const online = machines.find((machine) => isMachineOnline(machine));
    if (online) return online.id;
    return machines.length > 0 ? machines[0].id : undefined;
}
