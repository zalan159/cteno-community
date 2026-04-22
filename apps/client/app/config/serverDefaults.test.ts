import { afterEach, describe, expect, it, vi } from 'vitest';

async function importServerDefaults() {
    return await import('./serverDefaults');
}

afterEach(() => {
    vi.resetModules();
    vi.unstubAllEnvs();
});

describe('serverDefaults', () => {
    it('uses configured happy server url when present', async () => {
        vi.stubEnv('EXPO_PUBLIC_HAPPY_SERVER_URL', 'https://example.com///');

        const { getDefaultServerUrl } = await importServerDefaults();

        expect(getDefaultServerUrl()).toBe('https://example.com');
    });

    it('falls back to an empty url when env is missing', async () => {
        const { getDefaultServerUrl } = await importServerDefaults();

        expect(getDefaultServerUrl()).toBe('');
    });
});
