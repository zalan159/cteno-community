import { afterEach, describe, expect, it, vi } from 'vitest';

afterEach(() => {
    vi.resetModules();
    vi.unstubAllEnvs();
});

describe('runtime config', () => {
    it('returns null for optional config when env is absent', async () => {
        const { getOptionalHappyServerUrl } = await import('./runtime');

        expect(getOptionalHappyServerUrl()).toBeNull();
    });

    it('parses cloud sync override booleans', async () => {
        vi.stubEnv('EXPO_PUBLIC_CLOUD_SYNC_ENABLED', 'false');
        let runtime = await import('./runtime');
        expect(runtime.getOptionalCloudSyncEnabled()).toBe(false);

        vi.resetModules();
        vi.stubEnv('EXPO_PUBLIC_CLOUD_SYNC_ENABLED', 'true');
        runtime = await import('./runtime');
        expect(runtime.getOptionalCloudSyncEnabled()).toBe(true);
    });
});
