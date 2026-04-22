import AsyncStorage from '@react-native-async-storage/async-storage';

import { isTauri } from '@/utils/tauri';

const AUTH_KEY = 'auth_credentials_v2';

// Cache for synchronous access
let credentialsCache: string | null = null;

/**
 * Cteno 2.0 unified auth credentials.
 *
 * All fields are plaintext. There is **no** end-to-end encryption layer and
 * **no** machine/bootstrap token any more — server responses give us a single
 * (accessToken, refreshToken) pair plus user id. The client stores absolute
 * expiry timestamps (ms since epoch) for fast freshness checks.
 */
export interface AuthCredentials {
    accessToken: string;
    refreshToken: string;
    accessExpiresAt: number;   // ms epoch — absolute expiry of accessToken
    refreshExpiresAt: number;  // ms epoch — absolute expiry of refreshToken
    userId: string;
    machineId?: string;
    /**
     * Back-compat alias. Tons of call sites across the Expo app still read
     * `credentials.token` for the Authorization header. We keep it pointing at
     * the current accessToken so the 2.0 rewrite does not ripple into every
     * API file.
     */
    token: string;
}

/**
 * Shape returned by `/v1/auth/login`, `/v1/auth/register`, `/v1/auth/refresh`,
 * `/oauth/token`, etc.
 */
export interface AuthSuccessPayload {
    accessToken: string;
    refreshToken: string;
    expiresIn: number;         // seconds
    refreshExpiresIn: number;  // seconds
    userId: string;
}

/**
 * Build a full `AuthCredentials` from a server auth response. Computes
 * absolute expiry timestamps using the provided wall clock (defaults to now).
 */
export function credentialsFromAuthResponse(
    payload: AuthSuccessPayload,
    options: { nowMs?: number; machineId?: string } = {},
): AuthCredentials {
    const nowMs = options.nowMs ?? Date.now();
    return {
        accessToken: payload.accessToken,
        refreshToken: payload.refreshToken,
        accessExpiresAt: nowMs + payload.expiresIn * 1000,
        refreshExpiresAt: nowMs + payload.refreshExpiresIn * 1000,
        userId: payload.userId,
        machineId: options.machineId,
        token: payload.accessToken,
    };
}

/**
 * Produce a new credentials object with refreshed tokens, preserving
 * `machineId` from the original.
 */
export function rotateCredentials(
    previous: AuthCredentials,
    payload: AuthSuccessPayload,
    nowMs: number = Date.now(),
): AuthCredentials {
    return credentialsFromAuthResponse(payload, {
        nowMs,
        machineId: previous.machineId,
    });
}

function getBrowserStorage(): Storage | null {
    return typeof globalThis.localStorage === 'undefined' ? null : globalThis.localStorage;
}

async function readStoredCredentials(): Promise<string | null> {
    const browserStorage = getBrowserStorage();
    if (browserStorage) {
        return browserStorage.getItem(AUTH_KEY);
    }
    return AsyncStorage.getItem(AUTH_KEY);
}

async function writeStoredCredentials(value: string): Promise<void> {
    const browserStorage = getBrowserStorage();
    if (browserStorage) {
        browserStorage.setItem(AUTH_KEY, value);
        return;
    }
    await AsyncStorage.setItem(AUTH_KEY, value);
}

async function clearStoredCredentials(): Promise<void> {
    const browserStorage = getBrowserStorage();
    if (browserStorage) {
        browserStorage.removeItem(AUTH_KEY);
        return;
    }
    await AsyncStorage.removeItem(AUTH_KEY);
}

/**
 * Re-materialize the `token` alias after loading from storage or after a
 * rotation — guards against manual edits that leave the mirror stale.
 */
function withTokenAlias(raw: AuthCredentials): AuthCredentials {
    if (raw.token !== raw.accessToken) {
        return { ...raw, token: raw.accessToken };
    }
    return raw;
}

function parseStored(raw: string): AuthCredentials | null {
    try {
        const parsed = JSON.parse(raw) as AuthCredentials;
        if (!parsed.accessToken || !parsed.refreshToken) {
            return null;
        }
        return withTokenAlias(parsed);
    } catch {
        return null;
    }
}

export const TokenStorage = {
    peekCredentials(): AuthCredentials | null {
        try {
            if (credentialsCache) {
                return parseStored(credentialsCache);
            }

            const browserStorage = getBrowserStorage();
            if (!browserStorage) {
                return null;
            }

            const stored = browserStorage.getItem(AUTH_KEY);
            if (!stored) return null;
            credentialsCache = stored;
            return parseStored(stored);
        } catch (error) {
            console.error('Error peeking credentials:', error);
            return null;
        }
    },

    async getCredentials(): Promise<AuthCredentials | null> {
        try {
            const cached = this.peekCredentials();
            if (cached) {
                return cached;
            }

            const stored = await readStoredCredentials();
            if (!stored) {
                return null;
            }

            credentialsCache = stored;
            return parseStored(stored);
        } catch (error) {
            console.error('Error getting credentials:', error);
            return null;
        }
    },

    async setCredentials(credentials: AuthCredentials): Promise<boolean> {
        try {
            const normalized = withTokenAlias(credentials);
            const json = JSON.stringify(normalized);
            await writeStoredCredentials(json);
            credentialsCache = json;
            return true;
        } catch (error) {
            console.error('Error setting credentials:', error);
            return false;
        }
    },

    async removeCredentials(): Promise<boolean> {
        try {
            await clearStoredCredentials();
            credentialsCache = null;
            return true;
        } catch (error) {
            console.error('Error removing credentials:', error);
            return false;
        }
    },
};

//
// Convenience helpers used by the refresh pipeline + fetch wrapper.
//

export async function loadCredentials(): Promise<AuthCredentials | null> {
    return TokenStorage.getCredentials();
}

/**
 * When running in Tauri, also push the credentials to the daemon's AuthStore
 * via the `cteno_auth_*` Tauri commands. This is what lets the Rust side
 * subscribe to login/logout transitions (refresh guard, user/machine socket
 * boot, first-login machine register). Fail-safe: bridge errors don't block
 * the JS-side save/clear, so logout still works if the daemon is down.
 */
async function bridgeToTauriAuthStore(c: AuthCredentials | null): Promise<void> {
    if (!isTauri()) return;
    try {
        const { invoke } = await import('@tauri-apps/api/core');
        if (c === null) {
            await invoke('cteno_auth_clear_credentials');
        } else {
            await invoke('cteno_auth_save_credentials', {
                args: {
                    accessToken: c.accessToken,
                    refreshToken: c.refreshToken,
                    userId: c.userId,
                    accessExpiresAtMs: c.accessExpiresAt,
                    refreshExpiresAtMs: c.refreshExpiresAt,
                    machineId: c.machineId,
                },
            });
        }
    } catch (e) {
        console.warn('[auth] tauri bridge failed (daemon unavailable?):', e);
    }
}

export async function saveCredentials(c: AuthCredentials): Promise<void> {
    const ok = await TokenStorage.setCredentials(c);
    if (!ok) {
        throw new Error('Failed to persist credentials');
    }
    await bridgeToTauriAuthStore(c);
}

/**
 * Mirror a rotated credentials pair from the Rust `AuthStore` into the
 * webview's own localStorage copy WITHOUT bridging it back to Tauri. The
 * Rust side is already the source of truth (Plan A) — pushing back would
 * either duplicate work or create a write loop.
 *
 * Used by the `auth-tokens-rotated` Tauri event listener and by the JS
 * stand-in for `/v1/auth/refresh` (which delegates to Rust).
 */
export async function saveCredentialsLocal(c: AuthCredentials): Promise<void> {
    const ok = await TokenStorage.setCredentials(c);
    if (!ok) {
        throw new Error('Failed to persist credentials locally');
    }
}

export async function clearCredentials(): Promise<void> {
    const stack = new Error('[auth-clear-trace] clearCredentials() called').stack ?? '(no stack)';
    console.warn('[auth-clear-trace]', stack);
    try {
        const { frontendLog } = await import('@/utils/tauri');
        frontendLog(`[auth-clear-trace] clearCredentials() stack:\n${stack}`, 'warn');
    } catch {
        // non-Tauri environment — console.warn above is already captured
    }
    await TokenStorage.removeCredentials();
    await bridgeToTauriAuthStore(null);
}

export async function getAccessToken(): Promise<string | null> {
    const c = await TokenStorage.getCredentials();
    return c?.accessToken ?? null;
}

export function isAccessValid(c: AuthCredentials, nowMs: number = Date.now()): boolean {
    return c.accessExpiresAt > nowMs;
}

export function accessRemainingMs(c: AuthCredentials, nowMs: number = Date.now()): number {
    return Math.max(0, c.accessExpiresAt - nowMs);
}

export function refreshRemainingMs(c: AuthCredentials, nowMs: number = Date.now()): number {
    return Math.max(0, c.refreshExpiresAt - nowMs);
}
