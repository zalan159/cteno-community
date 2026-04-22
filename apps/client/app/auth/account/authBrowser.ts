import { getRandomBytes } from 'expo-crypto';
import * as WebBrowser from 'expo-web-browser';
import { Linking, Platform } from 'react-native';

import { encodeBase64 } from '@/encryption/base64';
import { requireServerUrl } from '@/sync/serverConfig';
import { openExternalUrl } from '@/utils/openExternalUrl';

const OAUTH_CLIENT_ID = 'cteno-desktop';
// Mobile WebBrowser.openAuthSessionAsync expects an OS-registered scheme.
// Desktop Tauri instead spins up an ephemeral loopback HTTP listener (see
// `oauth_loopback_start` Tauri command / RFC 8252 §7.3) and overrides this.
const OAUTH_REDIRECT_URI_NATIVE = 'cteno://auth/callback';
const CALLBACK_TIMEOUT_MS = 5 * 60 * 1000;

function isTauriEnv(): boolean {
    return (
        Platform.OS === 'web'
        && typeof window !== 'undefined'
        && '__TAURI_INTERNALS__' in window
    );
}

type OAuthCallback = {
    code?: string;
    state?: string;
    error?: string;
};

type OAuthCallbackResolution =
    | { handled: false }
    | { handled: true; code: string }
    | { handled: true; error: Error };

function parseOAuthCallback(url: string): OAuthCallback | null {
    try {
        const parsed = new URL(url, 'http://loopback.invalid');
        const callbackPath = parsed.pathname.replace(/\/+$/, '') || '/';

        // Mobile deep-link form: cteno://auth/callback?...
        if (parsed.protocol === 'cteno:' && parsed.host === 'auth' && callbackPath === '/callback') {
            return extractCallbackParams(parsed);
        }

        // Desktop loopback form: http://127.0.0.1:<port>/callback?...
        // (The Rust listener only emits the path+query, but we still normalize
        // via URL above using the dummy base.)
        if (callbackPath === '/callback') {
            return extractCallbackParams(parsed);
        }

        return null;
    } catch {
        return null;
    }
}

function extractCallbackParams(parsed: URL): OAuthCallback {
    return {
        code: parsed.searchParams.get('code') ?? undefined,
        state: parsed.searchParams.get('state') ?? undefined,
        error: parsed.searchParams.get('error') ?? undefined,
    };
}

function normalizeDeepLinkPayload(payload: unknown): string[] {
    if (typeof payload === 'string') {
        return [payload];
    }

    if (Array.isArray(payload)) {
        return payload.filter((value): value is string => typeof value === 'string');
    }

    return [];
}

function resolveOAuthCallback(
    url: string,
    expectedState: string,
    strictState: boolean,
): OAuthCallbackResolution {
    const callback = parseOAuthCallback(url);
    if (!callback) {
        return { handled: false };
    }

    if (callback.error) {
        return { handled: true, error: new Error('Authorization was denied.') };
    }

    if (!callback.code || !callback.state) {
        return { handled: false };
    }

    if (callback.state !== expectedState) {
        if (strictState) {
            return { handled: true, error: new Error('Security validation failed. Please try again.') };
        }

        return { handled: false };
    }

    return { handled: true, code: callback.code };
}

export function generateBrowserOAuthState(): string {
    return encodeBase64(getRandomBytes(32), 'base64url');
}

export function buildBrowserAuthorizeUrl(state: string, redirectUri: string = OAUTH_REDIRECT_URI_NATIVE): string {
    const serverUrl = requireServerUrl();
    const params = new URLSearchParams({
        client_id: OAUTH_CLIENT_ID,
        redirect_uri: redirectUri,
        state,
    });

    return `${serverUrl}/oauth/authorize?${params.toString()}`;
}

function buildLandingUrl(path: string): string {
    return `${requireServerUrl()}${path}`;
}

export function buildLandingRegisterUrl(): string {
    return buildLandingUrl('/register');
}

export function buildLandingForgotPasswordUrl(): string {
    return buildLandingUrl('/reset-password');
}

export function buildLandingTermsUrl(): string {
    return buildLandingUrl('/terms');
}

export function buildLandingPrivacyUrl(): string {
    return buildLandingUrl('/privacy');
}

async function waitForOAuthCallback(expectedState: string): Promise<string> {
    return new Promise(async (resolve, reject) => {
        let settled = false;
        let timeoutId: ReturnType<typeof setTimeout> | null = null;
        let removeLinkingListener: { remove(): void } | null = null;
        let removeTauriListener: (() => void) | null = null;

        const cleanup = () => {
            if (timeoutId) {
                clearTimeout(timeoutId);
                timeoutId = null;
            }
            removeLinkingListener?.remove();
            removeLinkingListener = null;
            removeTauriListener?.();
            removeTauriListener = null;
        };

        const finish = (result: { code?: string; error?: Error }) => {
            if (settled) {
                return;
            }

            settled = true;
            cleanup();

            if (result.error) {
                reject(result.error);
                return;
            }

            if (!result.code) {
                reject(new Error('OAuth callback did not include an authorization code.'));
                return;
            }

            resolve(result.code);
        };

        const handleUrl = (url: string, strictState: boolean) => {
            const resolved = resolveOAuthCallback(url, expectedState, strictState);
            if (!resolved.handled) {
                return false;
            }

            if ('error' in resolved) {
                finish({ error: resolved.error });
            } else {
                finish({ code: resolved.code });
            }
            return true;
        };

        timeoutId = setTimeout(() => {
            finish({ error: new Error('Timed out waiting for the login callback.') });
        }, CALLBACK_TIMEOUT_MS);

        try {
            if (Platform.OS === 'web' && typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
                const [{ listen }, { invoke }] = await Promise.all([
                    import('@tauri-apps/api/event'),
                    import('@tauri-apps/api/core'),
                ]);

                try {
                    await invoke('plugin:deep-link|register', { protocol: 'cteno' });
                } catch {
                    // Ignore registration failures and still listen for callbacks.
                }

                const currentLinks = await invoke<unknown>('plugin:deep-link|get_current').catch(() => null);
                for (const url of normalizeDeepLinkPayload(currentLinks)) {
                    if (handleUrl(url, false)) {
                        return;
                    }
                }

                removeTauriListener = await listen<unknown>('deep-link://new-url', (event) => {
                    for (const url of normalizeDeepLinkPayload(event.payload)) {
                        if (handleUrl(url, true)) {
                            return;
                        }
                    }
                });
            }

            const initialUrl = await Linking.getInitialURL().catch(() => null);
            if (initialUrl && handleUrl(initialUrl, false)) {
                return;
            }

            removeLinkingListener = Linking.addEventListener('url', ({ url }) => {
                handleUrl(url, true);
            });
        } catch (error) {
            finish({
                error: error instanceof Error
                    ? error
                    : new Error(`Failed to initialize OAuth flow: ${String(error)}`),
            });
        }
    });
}

async function waitForNativeOAuthCallback(expectedState: string): Promise<string> {
    const result = await WebBrowser.openAuthSessionAsync(
        buildBrowserAuthorizeUrl(expectedState, OAUTH_REDIRECT_URI_NATIVE),
        OAUTH_REDIRECT_URI_NATIVE,
    );

    if (result.type === 'success') {
        const resolved = resolveOAuthCallback(result.url, expectedState, true);
        if (!resolved.handled) {
            throw new Error('OAuth callback did not include an authorization code.');
        }

        if ('error' in resolved) {
            throw resolved.error;
        }

        return resolved.code;
    }

    if (result.type === 'cancel' || result.type === 'dismiss') {
        throw new Error('Login was cancelled.');
    }

    if (result.type === 'locked') {
        throw new Error('Another login is already in progress. Please try again.');
    }

    throw new Error('Browser login did not return to the app.');
}

/**
 * Raw OAuth/token-exchange response shape.  The 2.0 server emits the unified
 * `{ accessToken, refreshToken, expiresIn, refreshExpiresIn, userId }` payload;
 * older servers (and some social providers we proxy through) still use the
 * snake-case `access_token` / `refresh_token` names. We accept both so the
 * flow keeps working while backends roll forward.
 */
export interface BrowserOAuthResponse {
    accessToken: string;
    refreshToken: string;
    expiresIn: number;
    refreshExpiresIn: number;
    userId: string;
}

function normalizeOAuthResponse(payload: any): BrowserOAuthResponse | null {
    if (!payload || typeof payload !== 'object') return null;
    const accessToken = payload.accessToken ?? payload.access_token ?? payload.token;
    const refreshToken = payload.refreshToken ?? payload.refresh_token ?? '';
    if (typeof accessToken !== 'string' || !accessToken) {
        return null;
    }
    const expiresIn = toFiniteNumber(payload.expiresIn ?? payload.expires_in) ?? 60 * 60;
    const refreshExpiresIn =
        toFiniteNumber(payload.refreshExpiresIn ?? payload.refresh_expires_in) ?? 60 * 24 * 3600;
    const userId = typeof payload.userId === 'string' ? payload.userId : '';
    return {
        accessToken,
        refreshToken: typeof refreshToken === 'string' ? refreshToken : '',
        expiresIn,
        refreshExpiresIn,
        userId,
    };
}

function toFiniteNumber(value: unknown): number | null {
    if (typeof value === 'number' && Number.isFinite(value) && value > 0) {
        return value;
    }
    if (typeof value === 'string' && value.trim()) {
        const parsed = Number(value);
        if (Number.isFinite(parsed) && parsed > 0) {
            return parsed;
        }
    }
    return null;
}

export async function exchangeOAuthCodeForToken(code: string, redirectUri: string = OAUTH_REDIRECT_URI_NATIVE): Promise<BrowserOAuthResponse> {
    const serverUrl = requireServerUrl();
    const response = await fetch(`${serverUrl}/v1/oauth/token`, {
        method: 'POST',
        headers: {
            'Content-Type': 'application/json',
        },
        body: JSON.stringify({
            code,
            client_id: OAUTH_CLIENT_ID,
            redirect_uri: redirectUri,
            grant_type: 'authorization_code',
        }),
    });

    const payload = await response.json().catch(() => null) as
        | Record<string, unknown>
        | null;

    if (!response.ok) {
        const errorMessage = (payload && typeof payload.error === 'string' && payload.error)
            || `Token exchange failed with status ${response.status}.`;
        throw new Error(errorMessage);
    }

    const normalized = normalizeOAuthResponse(payload);
    if (!normalized) {
        throw new Error('Token exchange succeeded but no access token was returned.');
    }
    return normalized;
}

export async function loginWithBrowserOAuth(): Promise<BrowserOAuthResponse> {
    const state = generateBrowserOAuthState();

    if (Platform.OS === 'ios' || Platform.OS === 'android') {
        const code = await waitForNativeOAuthCallback(state);
        return exchangeOAuthCodeForToken(code, OAUTH_REDIRECT_URI_NATIVE);
    }

    // Desktop Tauri: prefer loopback HTTP listener (RFC 8252 §7.3). The Rust
    // side spawns a one-shot listener and we await the captured path through
    // the command's own Promise — this survives webview HMR / reload, unlike
    // Tauri event listeners which are torn down with the JS context.
    if (isTauriEnv()) {
        try {
            const code = await loopbackOAuthFlow(state);
            const redirectUri = loopbackRedirectUriCache ?? OAUTH_REDIRECT_URI_NATIVE;
            return exchangeOAuthCodeForToken(code, redirectUri);
        } catch (loopbackError) {
            // If the Tauri command isn't available (older daemon build),
            // fall through to the legacy deep-link flow below.
            if (!isLoopbackUnavailable(loopbackError)) {
                throw loopbackError;
            }
        }
    }

    // Legacy deep-link fallback (also the path for non-Tauri web).
    const callbackPromise = waitForOAuthCallback(state);
    await openExternalUrl(buildBrowserAuthorizeUrl(state, OAUTH_REDIRECT_URI_NATIVE));
    const code = await callbackPromise;
    return exchangeOAuthCodeForToken(code, OAUTH_REDIRECT_URI_NATIVE);
}

let loopbackRedirectUriCache: string | null = null;

async function loopbackOAuthFlow(state: string): Promise<string> {
    const { invoke } = await import('@tauri-apps/api/core');
    const started = await invoke<{ handle: string; port: number; redirectUri: string }>(
        'oauth_loopback_start',
    );
    loopbackRedirectUriCache = started.redirectUri;

    // Start the server round-trip before we begin awaiting the loopback —
    // the Rust listener is already bound and accepting.
    await openExternalUrl(buildBrowserAuthorizeUrl(state, started.redirectUri));

    const rawPath = await invoke<string>('oauth_loopback_wait', { handle: started.handle });
    const resolved = resolveOAuthCallback(rawPath, state, true);
    if (!resolved.handled) {
        throw new Error('Loopback callback did not include an authorization code.');
    }
    if ('error' in resolved) {
        throw resolved.error;
    }
    return resolved.code;
}

function isLoopbackUnavailable(err: unknown): boolean {
    const message = err instanceof Error ? err.message : String(err);
    return /oauth_loopback_start|oauth_loopback_wait|command .* not found|Command not allowed/i
        .test(message);
}
