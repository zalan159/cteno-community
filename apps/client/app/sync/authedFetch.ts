/**
 * Authenticated fetch wrapper for Cteno 2.0.
 *
 * All server HTTP calls go through this helper so the access token is:
 *   1. Refreshed proactively before the request (via `ensureFreshAccess`)
 *      when it is within the refresh threshold (default 60s of its TTL).
 *   2. Re-refreshed and retried exactly once on a 401 response — this
 *      catches edge cases where the token was accepted by our local clock
 *      but rejected by the server (clock skew, server-side revocation,
 *      cache staleness after a server restart).
 *
 * This replaces the old pattern of call sites caching an `AuthCredentials`
 * object and embedding `credentials.token` in the Authorization header
 * directly, which broke as soon as the 30-minute access token TTL elapsed.
 *
 * On hard-expiry (refresh token revoked/invalid), `ensureFreshAccess`
 * throws `AuthExpiredError` — this is surfaced unchanged so the UI shell
 * can route back to the login screen via the `onAuthExpired` listener
 * registered in `refresh.ts`.
 */
import { AuthExpiredError, ensureFreshAccess, forceRefresh } from '@/auth/refresh';

export interface AuthedFetchOptions extends RequestInit {
    /**
     * Skip the initial `ensureFreshAccess` call and use this token directly.
     * The 401 retry path still refreshes. Useful for transitional call sites
     * that already hold a credentials object and want to participate in the
     * retry logic without eagerly refreshing.
     */
    tokenOverride?: string;
}

export class NotAuthenticatedError extends Error {
    constructor() {
        super('Not authenticated');
        this.name = 'NotAuthenticatedError';
    }
}

function attachAuthHeader(init: RequestInit | undefined, token: string): RequestInit {
    const headers = new Headers(init?.headers);
    headers.set('Authorization', `Bearer ${token}`);
    return { ...(init ?? {}), headers };
}

/**
 * Authenticated `fetch`. Throws `NotAuthenticatedError` if the user is not
 * logged in, `AuthExpiredError` if the refresh token itself is dead.
 * Otherwise returns the final `Response` (could still be non-2xx; caller
 * decides how to interpret server-level errors other than 401).
 */
export async function authedFetch(
    input: RequestInfo | URL,
    init: AuthedFetchOptions = {},
): Promise<Response> {
    const { tokenOverride, ...rest } = init;

    let token: string | null;
    if (tokenOverride && tokenOverride.trim()) {
        token = tokenOverride;
    } else {
        token = await ensureFreshAccess();
    }
    if (!token) {
        throw new NotAuthenticatedError();
    }

    const firstResponse = await fetch(input, attachAuthHeader(rest, token));
    if (firstResponse.status !== 401) {
        return firstResponse;
    }

    // 401 on what we thought was a fresh token. Force a refresh and retry
    // once. `forceRefresh` will throw AuthExpiredError if the refresh
    // token itself is invalid — let that propagate.
    let rotated: string | null;
    try {
        const refreshed = await forceRefresh();
        rotated = refreshed?.accessToken ?? null;
    } catch (err) {
        if (err instanceof AuthExpiredError) {
            throw err;
        }
        // Network / transient refresh error — return the original 401 so the
        // caller can handle it as a normal request failure.
        return firstResponse;
    }

    if (!rotated || rotated === token) {
        return firstResponse;
    }

    return fetch(input, attachAuthHeader(rest, rotated));
}
