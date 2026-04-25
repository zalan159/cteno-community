import { afterEach, describe, expect, it, vi } from 'vitest';

const isTauriMock = vi.fn();
const isServerAvailableMock = vi.fn();

vi.mock('@/auth/local_mode', () => ({
    isDesktopLocalModeEnabled: () => isTauriMock(),
}));

vi.mock('@/sync/serverConfig', () => ({
    isServerAvailable: () => isServerAvailableMock(),
}));

async function importCapabilities() {
    return await import('./capabilities');
}

afterEach(() => {
    vi.resetModules();
    vi.unstubAllEnvs();
    isTauriMock.mockReset();
    isServerAvailableMock.mockReset();
});

describe('runtime capabilities', () => {
    it('defaults cloud sync on when a hosted server is configured', async () => {
        vi.stubEnv('EXPO_PUBLIC_HAPPY_SERVER_URL', 'https://cteno.example');
        isServerAvailableMock.mockReturnValue(true);

        const { isCloudSyncEnabled } = await importCapabilities();

        expect(isCloudSyncEnabled()).toBe(true);
    });

    it('allows account auth without cloud sync for local-token desktop login', async () => {
        vi.stubEnv('EXPO_PUBLIC_HAPPY_SERVER_URL', 'https://cteno.example');
        vi.stubEnv('EXPO_PUBLIC_CLOUD_SYNC_ENABLED', 'false');
        isTauriMock.mockReturnValue(true);
        isServerAvailableMock.mockReturnValue(true);

        const { getRuntimeCapabilities, shouldUseLocalTokenLogin } = await importCapabilities();

        expect(getRuntimeCapabilities()).toEqual({
            localSessions: true,
            accountAuth: true,
            cloudSync: false,
        });
        expect(shouldUseLocalTokenLogin()).toBe(true);
    });

    it('blocks cloud server access when cloud sync is explicitly disabled', async () => {
        vi.stubEnv('EXPO_PUBLIC_HAPPY_SERVER_URL', 'https://cteno.example');
        vi.stubEnv('EXPO_PUBLIC_CLOUD_SYNC_ENABLED', 'false');
        isServerAvailableMock.mockReturnValue(true);

        const { canUseCloudServerAccess } = await importCapabilities();

        expect(canUseCloudServerAccess('token')).toBe(false);
    });
});
