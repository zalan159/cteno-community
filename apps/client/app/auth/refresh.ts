/**
 * Unified access-token refresh pipeline for Cteno 2.0.
 *
 * Responsibilities:
 *   • Call `POST /v1/auth/refresh` against the current server.
 *   • Persist rotated (accessToken, refreshToken) pair on success.
 *   • Distinguish *user must re-login* errors from transient network errors.
 *   • Expose a single-flight `ensureFreshAccess` helper used by HTTP/RPC layers.
 *
 * The Expo client used to piggyback on an end-to-end encryption handshake for
 * "token freshness"; that handshake is gone in 2.0, so refresh is plain JSON.
 */
import {
    AuthCredentials,
    accessRemainingMs,
    clearCredentials,
    loadCredentials,
    rotateCredentials,
    saveCredentials,
    saveCredentialsLocal,
    AuthSuccessPayload,
} from '@/auth/tokenStorage';
import { requireServerUrl } from '@/sync/serverConfig';
import { isTauri } from '@/utils/tauri';

const DEFAULT_REFRESH_THRESHOLD_MS = 60_000; // refresh when <60s of life left
const REFRESH_TIMEOUT_MS = 15_000;

/**
 * Thrown when the refresh-token itself is invalid / revoked and the client
 * must force the user through login again. Caller is expected to clear
 * credentials (we already do that before throwing) and redirect to the
 * auth screen.
 */
export class AuthExpiredError extends Error {
    public readonly reason: string;
    constructor(reason: string) {
        super(`Auth expired: ${reason}`);
        this.name = 'AuthExpiredError';
        this.reason = reason;
    }
}

/**
 * Subset of `/v1/auth/refresh` error codes that mean "we will never succeed
 * with this refresh token, clear it". Anything else (network, 5xx, shape
 * surprise) is treated as retriable.
 */
const HARD_EXPIRY_CODES = new Set([
    'refresh_token_invalid',
    'refresh_token_not_found',
    'refresh_token_mismatch',
    'refresh_token_revoked',
]);

type AuthExpiredListener = (reason: string) => void;
const authExpiredListeners = new Set<AuthExpiredListener>();

/**
 * Subscribe to the "user needs to re-login" signal. Used by the UI shell to
 * route back to the login screen, disconnect sockets, flush caches, etc.
 */
export function onAuthExpired(listener: AuthExpiredListener): () => void {
    authExpiredListeners.add(listener);
    return () => {
        authExpiredListeners.delete(listener);
    };
}

function notifyAuthExpired(reason: string): void {
    for (const listener of Array.from(authExpiredListeners)) {
        try {
            listener(reason);
        } catch (err) {
            console.warn('[auth/refresh] onAuthExpired listener threw:', err);
        }
    }
}

async function fetchWithTimeout(
    input: RequestInfo,
    init: RequestInit,
    timeoutMs: number,
): Promise<Response> {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), timeoutMs);
    try {
        return await fetch(input, { ...init, signal: controller.signal });
    } finally {
        clearTimeout(timer);
    }
}

/**
 * Exchange a refresh token for a fresh (accessToken, refreshToken) pair.
 * On hard-expiry responses (401 with known code) we clear local credentials
 * so subsequent requests short-circuit to login.
 *
 * Under Plan A (Rust-as-authority), Tauri environments delegate to the Rust
 * AuthStore so the server-side refresh-token family stays single-sourced;
 * pure-web fallback still hits `/v1/auth/refresh` directly.
 */
export async function refreshTokens(
    baseUrl: string,
    refreshToken: string,
    previous: AuthCredentials,
): Promise<AuthCredentials> {
    if (isTauri()) {
        return refreshTokensViaDaemon(previous);
    }

    let response: Response;
    try {
        response = await fetchWithTimeout(
            `${baseUrl}/v1/auth/refresh`,
            {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ refreshToken }),
            },
            REFRESH_TIMEOUT_MS,
        );
    } catch (networkError) {
        // Network failure — don't wipe credentials, let caller keep using the
        // current (possibly stale) access token and try again later.
        throw networkError;
    }

    if (response.status === 401) {
        const body = await response.json().catch(() => null) as { error?: string } | null;
        const code = body?.error ?? 'refresh_token_invalid';
        console.warn('[auth-clear-trace] /v1/auth/refresh 401', { code, body });
        try {
            const { frontendLog } = await import('@/utils/tauri');
            frontendLog(
                `[auth-clear-trace] refresh 401 code=${code} refreshExpiresAt=${new Date(previous.refreshExpiresAt).toISOString()} now=${new Date().toISOString()} refreshToken_prefix=${previous.refreshToken.slice(0, 24)}`,
                'warn',
            );
        } catch {}
        if (HARD_EXPIRY_CODES.has(code)) {
            await clearCredentials();
            notifyAuthExpired(code);
            throw new AuthExpiredError(code);
        }
        // Some other 401 — let caller decide; still surface as error.
        throw new Error(`Refresh failed (401): ${code}`);
    }

    if (!response.ok) {
        throw new Error(`Refresh failed (${response.status})`);
    }

    const payload = (await response.json()) as AuthSuccessPayload;
    if (!payload?.accessToken || !payload?.refreshToken) {
        throw new Error('Refresh response missing tokens');
    }

    const rotated = rotateCredentials(previous, payload);
    await saveCredentials(rotated);
    return rotated;
}

interface DaemonForceRefreshResult {
    accessToken: string;
    refreshToken: string;
    userId: string;
    accessExpiresAtMs: number;
    refreshExpiresAtMs: number;
    machineId?: string | null;
}

/**
 * Delegate the refresh cycle to the Rust daemon. The daemon's guard is the
 * single authority on the server-side refresh-token family; the JS webview
 * just mirrors the rotated pair into its own localStorage so subsequent
 * Authorization headers pick it up.
 *
 * The daemon's `cteno_auth_force_refresh_now` is idempotent — if it already
 * rotated recently (still fresh), it returns the current snapshot without a
 * network round-trip.
 */
async function refreshTokensViaDaemon(
    previous: AuthCredentials,
): Promise<AuthCredentials> {
    let result: DaemonForceRefreshResult;
    try {
        const { invoke } = await import('@tauri-apps/api/core');
        result = await invoke<DaemonForceRefreshResult>(
            'cteno_auth_force_refresh_now',
        );
    } catch (err) {
        // Daemon unreachable / refresh failed at the Rust layer. Treat this
        // as transient; the guard's next tick will retry. Do NOT wipe
        // credentials on network errors.
        const msg = err instanceof Error ? err.message : String(err);
        // Daemon-side terminal errors (refresh_token revoked/invalid/mismatch)
        // show up here as Err strings. We still forward them for logging and
        // call clearCredentials only on the stable "refresh terminal" prefix
        // coming from `auth_store_boot.rs`.
        if (typeof msg === 'string' && msg.startsWith('refresh terminal:')) {
            try {
                const { frontendLog } = await import('@/utils/tauri');
                frontendLog(
                    `[auth-clear-trace] daemon refresh terminal: ${msg}`,
                    'warn',
                );
            } catch {}
            await clearCredentials();
            notifyAuthExpired('refresh_token_invalid');
            throw new AuthExpiredError('refresh_token_invalid');
        }
        throw new Error(`Daemon refresh failed: ${msg}`);
    }

    const rotated: AuthCredentials = {
        accessToken: result.accessToken,
        refreshToken: result.refreshToken,
        accessExpiresAt: result.accessExpiresAtMs,
        refreshExpiresAt: result.refreshExpiresAtMs,
        userId: result.userId,
        machineId: result.machineId ?? previous.machineId,
        token: result.accessToken,
    };
    // Rust already persisted the new pair into `auth.json` and will emit
    // `auth-tokens-rotated` for any other subscribers. We only need to
    // mirror into localStorage; skipping the bridge avoids pinging the
    // daemon with state it just handed us.
    await saveCredentialsLocal(rotated);
    return rotated;
}

//
// ensureFreshAccess — single-flight wrapper used by HTTP / socket layers.
//

let inflight: Promise<AuthCredentials | null> | null = null;

interface EnsureFreshOptions {
    thresholdMs?: number;
    /** Force refresh even if access token is still valid. */
    force?: boolean;
}

/**
 * Load current credentials and return the access token, refreshing proactively
 * if it is within `thresholdMs` of expiry (default 60s). Returns `null` when
 * the user is not logged in.
 *
 * Semantics:
 *   • Single-flight: concurrent callers share one refresh round-trip.
 *   • Network errors do not clear credentials.
 *   • Hard refresh errors (401 + known code) clear credentials and throw
 *     `AuthExpiredError`.
 */
export async function ensureFreshAccess(
    options: EnsureFreshOptions = {},
): Promise<string | null> {
    const credentials = await ensureFreshCredentials(options);
    return credentials?.accessToken ?? null;
}

/**
 * Same as `ensureFreshAccess` but returns the full credentials object.
 */
export async function ensureFreshCredentials(
    options: EnsureFreshOptions = {},
): Promise<AuthCredentials | null> {
    const threshold = options.thresholdMs ?? DEFAULT_REFRESH_THRESHOLD_MS;
    const current = await loadCredentials();
    if (!current) {
        return null;
    }
    const now = Date.now();
    if (!options.force && accessRemainingMs(current, now) > threshold) {
        return current;
    }

    if (current.refreshExpiresAt <= now) {
        console.warn('[auth-clear-trace] refresh token locally expired', {
            now: new Date(now).toISOString(),
            refreshExpiresAt: new Date(current.refreshExpiresAt).toISOString(),
        });
        try {
            const { frontendLog } = await import('@/utils/tauri');
            frontendLog(
                `[auth-clear-trace] local-expiry now=${new Date(now).toISOString()} refreshExpiresAt=${new Date(current.refreshExpiresAt).toISOString()}`,
                'warn',
            );
        } catch {}
        // Refresh token itself expired — nothing we can do, user must login.
        await clearCredentials();
        notifyAuthExpired('refresh_token_expired_local');
        throw new AuthExpiredError('refresh_token_expired_local');
    }

    if (!inflight) {
        const baseUrl = safeServerUrl();
        if (!baseUrl) {
            // Server unavailable — return current credentials so the caller can
            // still attempt the request.  We won't know if it's stale or not.
            return current;
        }
        inflight = refreshTokens(baseUrl, current.refreshToken, current)
            .catch((err) => {
                if (err instanceof AuthExpiredError) {
                    throw err;
                }
                console.warn('[auth/refresh] refresh failed, reusing current credentials:', err);
                return current;
            })
            .finally(() => {
                inflight = null;
            });
    }

    return inflight;
}

function safeServerUrl(): string | null {
    try {
        return requireServerUrl();
    } catch {
        return null;
    }
}

/**
 * Force a refresh unconditionally. Used by socket layer when it receives
 * `token:near-expiry` or a handshake `Invalid authentication token`.
 */
export async function forceRefresh(): Promise<AuthCredentials | null> {
    return ensureFreshCredentials({ force: true });
}
