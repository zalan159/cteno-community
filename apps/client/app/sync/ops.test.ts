import { beforeEach, describe, expect, it, vi } from 'vitest';

// `./ops` pulls in a broad RPC surface (sync, storage, apiSocket, Tauri
// helpers, etc). Stub the transitive React-Native / expo / Tauri imports so
// vitest's node environment can load the module purely for the vendor-meta
// normalization logic we want to test.
vi.mock('react-native', () => ({
    Platform: { OS: 'web', select: (spec: any) => spec.default ?? spec.web ?? spec.ios },
}));
vi.mock('@/utils/tauri', () => ({
    frontendLog: vi.fn(),
    isTauri: () => false,
    isMacOS: () => false,
}));
vi.mock('./apiSocket', () => ({
    apiSocket: { machineRPC: vi.fn() },
}));
vi.mock('./apiBalance', () => ({ fetchPublicProxyModels: vi.fn() }));
vi.mock('./modelCatalogCache', () => ({
    loadCachedVendorModelCatalog: vi.fn(),
    saveCachedVendorModelCatalog: vi.fn(),
}));
vi.mock('./sync', () => ({ sync: {} }));
vi.mock('./storage', () => ({ storage: { getState: () => ({ sessions: {} }) } }));
vi.mock('./apiKv', () => ({ kvGet: vi.fn(), kvSet: vi.fn() }));

const { resolveVendorMeta, listAvailableVendors, probeVendorConnection } = await import('./ops');
const { apiSocket } = await import('./apiSocket');
type VendorMeta = Parameters<typeof resolveVendorMeta>[0];
const machineRPC = vi.mocked(apiSocket.machineRPC);

const baseCapabilities = {
    setModel: true,
    setPermissionMode: true,
    setSandboxPolicy: true,
    abort: true,
    compact: true,
    runtimeControls: {
        model: { outcome: 'applied' as const, reason: null },
        permissionMode: { outcome: 'applied' as const, reason: null },
    },
};

describe('resolveVendorMeta connection defaulting', () => {
    beforeEach(() => {
        machineRPC.mockReset();
    });

    it('populates a default `unknown` connection when the daemon omits it', () => {
        const input: VendorMeta = {
            name: 'cteno',
            available: true,
            installed: true,
            loggedIn: true,
            capabilities: baseCapabilities,
            status: {
                installState: 'installed',
                authState: 'loggedIn',
            },
        };

        const resolved = resolveVendorMeta(input);
        expect(resolved.status.connection).toEqual({
            state: 'unknown',
            checkedAtUnixMs: 0,
        });
        expect(resolved.status.connection.reason).toBeUndefined();
    });

    it('preserves an explicit `connected` payload verbatim (including latency)', () => {
        const connection = {
            state: 'connected' as const,
            checkedAtUnixMs: 1_744_000_000_000,
            latencyMs: 42,
        };
        const input: VendorMeta = {
            name: 'codex',
            available: true,
            installed: true,
            loggedIn: true,
            capabilities: baseCapabilities,
            status: {
                installState: 'installed',
                authState: 'loggedIn',
                connection,
            },
        };

        const resolved = resolveVendorMeta(input);
        expect(resolved.status.connection).toBe(connection);
    });

    it('carries the `reason` string through for failed probes', () => {
        const input: VendorMeta = {
            name: 'codex',
            available: true,
            installed: true,
            loggedIn: true,
            capabilities: baseCapabilities,
            status: {
                installState: 'installed',
                authState: 'loggedIn',
                connection: {
                    state: 'failed',
                    reason: 'codex spawn timed out after 10s',
                    checkedAtUnixMs: 1_744_000_000_123,
                },
            },
        };

        const resolved = resolveVendorMeta(input);
        expect(resolved.status.connection.state).toBe('failed');
        expect(resolved.status.connection.reason).toBe('codex spawn timed out after 10s');
        expect(resolved.status.connection.latencyMs).toBeUndefined();
    });

    it('does not regress when only install/auth state are present (Phase-1 payload shape)', () => {
        const input: VendorMeta = {
            name: 'gemini',
            available: false,
            installed: false,
            loggedIn: null,
            capabilities: baseCapabilities,
            status: {
                installState: 'notInstalled',
                authState: 'unknown',
            },
        };

        const resolved = resolveVendorMeta(input);
        expect(resolved.installed).toBe(false);
        expect(resolved.status.installState).toBe('notInstalled');
        expect(resolved.status.connection).toEqual({
            state: 'unknown',
            checkedAtUnixMs: 0,
        });
    });

    it('normalizes the daemon top-level `connection` field into status.connection', async () => {
        machineRPC.mockResolvedValueOnce({
            vendors: [
                {
                    name: 'claude',
                    available: true,
                    installed: true,
                    loggedIn: true,
                    capabilities: baseCapabilities,
                    status: {
                        installState: 'installed',
                        authState: 'loggedIn',
                    },
                    connection: {
                        state: 'connected',
                        checkedAtUnixMs: 1_744_000_000_456,
                        latencyMs: 17,
                    },
                },
            ],
        });

        const resolved = await listAvailableVendors('machine-1');
        expect(machineRPC).toHaveBeenCalledWith('machine-1', 'list_available_vendors', {});
        expect(resolved[0].status.connection).toEqual({
            state: 'connected',
            checkedAtUnixMs: 1_744_000_000_456,
            latencyMs: 17,
        });
        expect('connection' in resolved[0]).toBe(false);
    });

    it('still lets local selector fixtures pass top-level connection through resolveVendorMeta', () => {
        const resolved = resolveVendorMeta(
            {
                name: 'claude',
                available: true,
                installed: true,
                loggedIn: true,
                capabilities: baseCapabilities,
                status: {
                    installState: 'installed',
                    authState: 'loggedIn',
                },
                connection: {
                    state: 'connected',
                    checkedAtUnixMs: 1_744_000_000_456,
                    latencyMs: 17,
                },
            },
        );

        expect(resolved.status.connection).toEqual({
            state: 'connected',
            checkedAtUnixMs: 1_744_000_000_456,
            latencyMs: 17,
        });
        expect('connection' in resolved).toBe(false);
    });

    it('normalizes remote probe wrapper responses before updating the selector', async () => {
        machineRPC.mockResolvedValueOnce({
            success: true,
            vendor: 'gemini',
            connection: {
                state: 'failed',
                reason: 'gemini auth probe failed',
                checkedAtUnixMs: 1_744_000_000_789,
            },
        });

        const result = await probeVendorConnection('machine-1', 'gemini');
        expect(machineRPC).toHaveBeenCalledWith(
            'machine-1',
            'probe_vendor_connection',
            { vendor: 'gemini' },
        );
        expect(result).toEqual({
            state: 'failed',
            reason: 'gemini auth probe failed',
            checkedAtUnixMs: 1_744_000_000_789,
        });
    });

    it('throws the daemon probe error when the remote wrapper reports failure', async () => {
        machineRPC.mockResolvedValueOnce({
            success: false,
            vendor: 'codex',
            error: 'codex app-server connection is closed',
        });

        await expect(probeVendorConnection('machine-1', 'codex')).rejects.toThrow(
            'codex app-server connection is closed',
        );
    });
});
