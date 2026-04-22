import { beforeEach, describe, expect, it, vi } from 'vitest';

const getRandomBytes = vi.fn(() => new Uint8Array([1, 2, 3]));
const openAuthSessionAsync = vi.fn();
const openExternalUrl = vi.fn();
const getInitialURL = vi.fn();
const addEventListener = vi.fn(() => ({ remove: vi.fn() }));
let platformOS: 'ios' | 'android' | 'web' = 'ios';

vi.mock('expo-crypto', () => ({
    getRandomBytes,
}));

vi.mock('expo-web-browser', () => ({
    openAuthSessionAsync,
}));

vi.mock('react-native', () => ({
    Platform: {
        get OS() {
            return platformOS;
        },
    },
    Linking: {
        getInitialURL,
        addEventListener,
    },
}));

vi.mock('@/sync/serverConfig', () => ({
    getServerUrl: () => 'https://server.test',
    requireServerUrl: () => 'https://server.test',
}));

vi.mock('@/utils/openExternalUrl', () => ({
    openExternalUrl,
}));

/**
 * Fixture for the 2.0 OAuth token-exchange response shape. All fields must be
 * present and plaintext — no more encryption envelopes.
 */
const tokenResponse = {
    accessToken: 'access-token',
    refreshToken: 'refresh-token',
    expiresIn: 3600,
    refreshExpiresIn: 30 * 24 * 3600,
    userId: 'user-123',
};

describe('authBrowser', () => {
    beforeEach(() => {
        vi.resetModules();
        vi.clearAllMocks();
        platformOS = 'ios';
        getRandomBytes.mockReturnValue(new Uint8Array([1, 2, 3]));
        getInitialURL.mockResolvedValue(null);
        addEventListener.mockReturnValue({ remove: vi.fn() });
        vi.stubGlobal('fetch', vi.fn());
    });

    it('uses expo-web-browser on native mobile and exchanges the returned code', async () => {
        openAuthSessionAsync.mockResolvedValue({
            type: 'success',
            url: 'cteno://auth/callback?code=oauth-code&state=AQID',
        });
        vi.mocked(fetch).mockResolvedValue({
            ok: true,
            status: 200,
            json: async () => tokenResponse,
        } as Response);

        const { loginWithBrowserOAuth } = await import('./authBrowser');

        await expect(loginWithBrowserOAuth()).resolves.toMatchObject({
            accessToken: 'access-token',
            refreshToken: 'refresh-token',
        });
        expect(openAuthSessionAsync).toHaveBeenCalledWith(
            'https://server.test/oauth/authorize?client_id=cteno-desktop&redirect_uri=cteno%3A%2F%2Fauth%2Fcallback&state=AQID',
            'cteno://auth/callback',
        );
        expect(openExternalUrl).not.toHaveBeenCalled();
        expect(fetch).toHaveBeenCalledWith(
            'https://server.test/v1/oauth/token',
            expect.objectContaining({ method: 'POST' }),
        );
    });

    it('uses the same providerless authorize URL for native browser login', async () => {
        openAuthSessionAsync.mockResolvedValue({
            type: 'success',
            url: 'cteno://auth/callback?code=oauth-code&state=AQID',
        });
        vi.mocked(fetch).mockResolvedValue({
            ok: true,
            status: 200,
            json: async () => tokenResponse,
        } as Response);

        const { loginWithBrowserOAuth } = await import('./authBrowser');

        await expect(loginWithBrowserOAuth()).resolves.toMatchObject({
            accessToken: 'access-token',
        });
        expect(openAuthSessionAsync).toHaveBeenCalledWith(
            'https://server.test/oauth/authorize?client_id=cteno-desktop&redirect_uri=cteno%3A%2F%2Fauth%2Fcallback&state=AQID',
            'cteno://auth/callback',
        );
    });

    it('keeps the listener-based flow for non-mobile platforms', async () => {
        platformOS = 'web';
        getInitialURL.mockResolvedValue('cteno://auth/callback?code=web-code&state=AQID');
        // Legacy snake_case path — server hasn't been updated yet.
        vi.mocked(fetch).mockResolvedValue({
            ok: true,
            status: 200,
            json: async () => ({
                access_token: 'web-token',
                refresh_token: 'web-refresh',
                expires_in: 3600,
                refresh_expires_in: 3600 * 24,
                userId: 'user-web',
            }),
        } as Response);

        const { loginWithBrowserOAuth } = await import('./authBrowser');

        await expect(loginWithBrowserOAuth()).resolves.toMatchObject({
            accessToken: 'web-token',
            refreshToken: 'web-refresh',
        });
        expect(openExternalUrl).toHaveBeenCalledWith(
            'https://server.test/oauth/authorize?client_id=cteno-desktop&redirect_uri=cteno%3A%2F%2Fauth%2Fcallback&state=AQID',
        );
        expect(openAuthSessionAsync).not.toHaveBeenCalled();
    });

    it('builds the landing register URL from the server base', async () => {
        const { buildLandingRegisterUrl } = await import('./authBrowser');

        expect(buildLandingRegisterUrl()).toBe('https://server.test/register');
    });
});
