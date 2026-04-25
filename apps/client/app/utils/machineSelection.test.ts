import { describe, expect, it } from 'vitest';
import { resolveVisibleMachineId } from './machineSelection';
import type { Machine, Session } from '@/sync/storageTypes';

function machine(id: string, active = true): Machine {
    return {
        id,
        seq: 0,
        createdAt: 0,
        updatedAt: 0,
        active,
        activeAt: 0,
        metadata: {
            host: id,
            platform: 'darwin-arm64',
            happyCliVersion: '0.0.0',
            happyHomeDir: `/tmp/${id}/cteno`,
            homeDir: `/tmp/${id}`,
        },
        metadataVersion: 0,
        daemonState: null,
        daemonStateVersion: 0,
    };
}

function session(machineId?: string): Session {
    return {
        id: `session-${machineId ?? 'none'}`,
        seq: 0,
        createdAt: 0,
        updatedAt: 0,
        active: false,
        activeAt: 0,
        metadata: machineId ? { path: '~', host: 'host', machineId } : null,
        metadataVersion: 0,
        agentState: null,
        agentStateVersion: 0,
        thinking: false,
        thinkingAt: 0,
        presence: 0,
    };
}

describe('resolveVisibleMachineId', () => {
    it('ignores a stale filter so community local mode can still browse the visible machine', () => {
        expect(resolveVisibleMachineId('cloud-machine', [machine('local-machine')], [])).toBe('local-machine');
    });

    it('keeps a valid selected filter even when another machine has recent sessions', () => {
        expect(
            resolveVisibleMachineId(
                'selected-machine',
                [machine('selected-machine'), machine('session-machine')],
                [session('session-machine')],
            ),
        ).toBe('selected-machine');
    });

    it('falls back to an offline visible machine when no online machine is available', () => {
        expect(resolveVisibleMachineId(null, [machine('offline-machine', false)], [])).toBe('offline-machine');
    });
});
