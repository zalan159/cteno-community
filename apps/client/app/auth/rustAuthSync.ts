/**
 * Plan A wiring: Rust `AuthStore` is the single authority on the server-side
 * refresh-token family. When its 60s-tick guard rotates tokens, it emits
 * `auth-tokens-rotated` over Tauri IPC; this module listens for that event
 * and mirrors the fresh pair into webview localStorage so subsequent
 * `authedFetch` calls carry the rotated Authorization header without
 * re-hitting `/v1/auth/refresh` (which would submit a now-revoked token).
 *
 * Install exactly once during app boot from the AuthContext provider. Safe to
 * re-install — the underlying unlisten handle is replaced.
 */
import type { AuthCredentials } from '@/auth/tokenStorage';
import { clearCredentials, saveCredentialsLocal } from '@/auth/tokenStorage';
import { isTauri } from '@/utils/tauri';

interface RotatedPayload {
    accessToken: string;
    refreshToken: string;
    userId: string;
    accessExpiresAtMs: number;
    refreshExpiresAtMs: number;
    machineId?: string | null;
}

type RotationListener = (c: AuthCredentials) => void;
type RequireLoginListener = (reason: string) => void;

let unlistenRotated: (() => void) | null = null;
let unlistenRequireLogin: (() => void) | null = null;
const listeners = new Set<RotationListener>();
const requireLoginListeners = new Set<RequireLoginListener>();

/**
 * Subscribe to the Rust-emitted `auth-require-login` signal. Fires when the
 * daemon's refresh guard hits a terminal error (refresh-token revoked /
 * invalid / mismatch) and had to clear its own AuthStore.
 */
export function onRustRequireLogin(listener: RequireLoginListener): () => void {
    requireLoginListeners.add(listener);
    return () => {
        requireLoginListeners.delete(listener);
    };
}

/**
 * Subscribe to mirror-after-rotation callbacks. Called from the
 * `AuthProvider` so it can refresh React state once localStorage is updated.
 */
export function onTokensRotated(listener: RotationListener): () => void {
    listeners.add(listener);
    return () => {
        listeners.delete(listener);
    };
}

export async function installRustAuthSync(): Promise<void> {
    if (!isTauri()) return;
    const { listen } = await import('@tauri-apps/api/event');

    if (!unlistenRotated) {
        unlistenRotated = await listen<RotatedPayload>(
            'auth-tokens-rotated',
            async (event) => {
                const p = event.payload;
                if (!p?.accessToken || !p?.refreshToken) {
                    console.warn('[auth-tokens-rotated] payload missing tokens', p);
                    return;
                }
                const creds: AuthCredentials = {
                    accessToken: p.accessToken,
                    refreshToken: p.refreshToken,
                    accessExpiresAt: p.accessExpiresAtMs,
                    refreshExpiresAt: p.refreshExpiresAtMs,
                    userId: p.userId,
                    machineId: p.machineId ?? undefined,
                    token: p.accessToken,
                };
                try {
                    await saveCredentialsLocal(creds);
                } catch (err) {
                    console.warn('[auth-tokens-rotated] saveCredentialsLocal failed:', err);
                    return;
                }
                for (const fn of Array.from(listeners)) {
                    try {
                        fn(creds);
                    } catch (err) {
                        console.warn('[auth-tokens-rotated] listener threw:', err);
                    }
                }
            },
        );
    }

    if (!unlistenRequireLogin) {
        unlistenRequireLogin = await listen<null>(
            'auth-require-login',
            async () => {
                console.warn('[auth-require-login] Rust daemon signaled terminal auth failure');
                try {
                    await clearCredentials();
                } catch (err) {
                    console.warn('[auth-require-login] clearCredentials failed:', err);
                }
                for (const fn of Array.from(requireLoginListeners)) {
                    try {
                        fn('rust_require_login');
                    } catch (err) {
                        console.warn('[auth-require-login] listener threw:', err);
                    }
                }
            },
        );
    }
}

export function uninstallRustAuthSync(): void {
    if (unlistenRotated) {
        try { unlistenRotated(); } catch { /* noop */ }
        unlistenRotated = null;
    }
    if (unlistenRequireLogin) {
        try { unlistenRequireLogin(); } catch { /* noop */ }
        unlistenRequireLogin = null;
    }
    listeners.clear();
    requireLoginListeners.clear();
}
