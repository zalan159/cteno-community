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
});
