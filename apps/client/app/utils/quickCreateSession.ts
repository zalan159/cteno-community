/**
 * Quick session creation — resolves sensible defaults and spawns a session
 * in one call. Used by the "+" button modal flow (vendor select → create).
 *
 * Extracted from new/index.tsx wizard logic.
 */
import { storage } from '@/sync/storage';
import { machineSpawnNewSession, type VendorName } from '@/sync/ops';
import { sync } from '@/sync/sync';
import { isMachineOnline } from '@/utils/machineUtils';

export interface QuickCreateOptions {
    /** Which vendor to spawn with. */
    vendor: VendorName;
    /** Override machine (e.g. from selectedMachineIdFilter). Resolved automatically if omitted. */
    machineId?: string | null;
    /** Override working directory. Resolved automatically if omitted. */
    path?: string | null;
    /** Initial prompt to send after creation (optional). */
    prompt?: string;
}

export type QuickCreateResult = {
    ok: true;
    sessionId: string;
} | {
    ok: false;
    error: string;
}

/**
 * Resolve the best machine ID from settings + storage state.
 * Priority: explicit override → most recent from recentMachinePaths → first online machine → first machine.
 */
function resolveMachineId(override?: string | null): string | null {
    const state = storage.getState();
    const machines = Object.values(state.machines);

    if (override && machines.some(m => m.id === override)) {
        return override;
    }

    const recentPaths = state.settings.recentMachinePaths ?? [];
    for (const recent of recentPaths) {
        if (machines.find(m => m.id === recent.machineId)) {
            return recent.machineId;
        }
    }

    // Prefer first online machine
    const online = machines.find(m => isMachineOnline(m));
    if (online) return online.id;

    return machines[0]?.id ?? null;
}

/**
 * Resolve the best path for a given machine.
 * Priority: explicit override → most recent session path → machine homeDir.
 */
function resolvePath(machineId: string, override?: string | null): string {
    if (override) return override;

    const state = storage.getState();

    // Check recentMachinePaths first
    const recentPaths = state.settings.recentMachinePaths ?? [];
    const fromRecent = recentPaths.find(rp => rp.machineId === machineId);
    if (fromRecent) return fromRecent.path;

    // Fall back to most recent session for this machine
    const sessions = Object.values(state.sessions);
    let bestPath = '';
    let bestTs = 0;
    for (const session of sessions) {
        if (session.metadata?.machineId === machineId && session.metadata?.path) {
            const ts = session.createdAt ?? 0;
            if (ts > bestTs) {
                bestTs = ts;
                bestPath = session.metadata.path;
            }
        }
    }
    if (bestPath) return bestPath;

    // Fall back to machine homeDir
    const machine = state.machines[machineId];
    return machine?.metadata?.homeDir ?? '/home';
}

/**
 * Create a session with sensible defaults, save settings, and return the sessionId.
 */
export async function quickCreateSession(options: QuickCreateOptions): Promise<QuickCreateResult> {
    const { vendor, prompt } = options;

    // 1. Resolve machine
    const machineId = resolveMachineId(options.machineId);
    if (!machineId) {
        return { ok: false, error: 'No machine available' };
    }

    // 2. Resolve path
    const directory = resolvePath(machineId, options.path);

    // 3. Spawn session
    try {
        const result = await machineSpawnNewSession({
            machineId,
            directory,
            approvedNewDirectoryCreation: true,
            agent: vendor,
        });

        if (!('sessionId' in result) || !result.sessionId) {
            const msg = ('errorMessage' in result && result.errorMessage) ? result.errorMessage : 'Session spawn failed';
            return { ok: false, error: msg };
        }

        const sessionId = result.sessionId;

        // 4. Save settings
        const recentPaths = storage.getState().settings.recentMachinePaths ?? [];
        const updatedPaths = [
            { machineId, path: directory },
            ...recentPaths.filter(rp => !(rp.machineId === machineId && rp.path === directory)),
        ].slice(0, 10);

        sync.applySettings({
            recentMachinePaths: updatedPaths,
            lastUsedAgent: vendor,
        });

        // 5. Refresh sessions list
        await sync.refreshSessions();

        // 6. Send initial prompt if provided
        if (prompt?.trim()) {
            await sync.sendMessage(sessionId, prompt);
        }

        return { ok: true, sessionId };
    } catch (error) {
        const msg = error instanceof Error ? error.message : 'Unknown error';
        return { ok: false, error: msg };
    }
}
