import React, { createContext, useContext, useState, useEffect, ReactNode } from 'react';
import {
    AuthCredentials,
    AuthSuccessPayload,
    clearCredentials,
    credentialsFromAuthResponse,
    saveCredentials,
} from '@/auth/tokenStorage';
import { installRustAuthSync, onTokensRotated } from '@/auth/rustAuthSync';
import { syncCreate, syncSetLocalModeCredentials } from '@/sync/sync';
import * as Updates from 'expo-updates';
import { clearPersistence } from '@/sync/persistence';
import { Platform } from 'react-native';
import { trackLogout } from '@/track';

interface AuthContextType {
    isAuthenticated: boolean;
    isLocalMode: boolean;
    hasAppAccess: boolean;
    credentials: AuthCredentials | null;
    /**
     * Accepts either the unified 2.0 auth response shape, or (for legacy
     * OAuth flows that still hand us a bare access token) a raw string. In
     * the bare-string case we fabricate a minimum-viable credentials object
     * with a far-future expiry so the first real request refreshes it.
     */
    login: (response: AuthSuccessPayload | string, extras?: { machineId?: string }) => Promise<void>;
    /**
     * Store credentials for local-mode agent/proxy auth without turning on
     * cloud sync or leaving local mode.
     */
    loginForLocalToken: (response: AuthSuccessPayload | string, extras?: { machineId?: string }) => Promise<void>;
    logout: () => Promise<void>;
    /**
     * Kept for back-compat with components that used to force an encryption
     * reload after importing secrets. In 2.0 there is nothing to reload,
     * so this is a no-op.
     */
    reloadEncryption: () => Promise<void>;
}

const AuthContext = createContext<AuthContextType | undefined>(undefined);

/** Fallback TTLs used when a legacy OAuth token is surfaced without `expiresIn`. */
const FALLBACK_ACCESS_TTL_SEC = 60 * 60; // 1 hour
const FALLBACK_REFRESH_TTL_SEC = 60 * 24 * 3600; // 60 days

function coerceAuthResponse(response: AuthSuccessPayload | string): AuthSuccessPayload {
    if (typeof response !== 'string') return response;
    // Legacy path: accept a bare access token, mark refresh as an empty string
    // so `ensureFreshAccess` cannot do anything — UI will force a real login
    // when the short-lived access token expires.
    return {
        accessToken: response,
        refreshToken: '',
        expiresIn: FALLBACK_ACCESS_TTL_SEC,
        refreshExpiresIn: FALLBACK_REFRESH_TTL_SEC,
        userId: '',
    };
}

export function AuthProvider({
    children,
    initialCredentials,
    initialLocalMode = false,
}: {
    children: ReactNode;
    initialCredentials: AuthCredentials | null;
    initialLocalMode?: boolean;
}) {
    const [isAuthenticated, setIsAuthenticated] = useState(!!initialCredentials);
    const [isLocalMode, setIsLocalMode] = useState(initialLocalMode);
    const [credentials, setCredentials] = useState<AuthCredentials | null>(initialCredentials);
    const hasAppAccess = isAuthenticated || isLocalMode;

    useEffect(() => {
        if (!hasAppAccess) {
            setCurrentAuth(null);
            return;
        }

        setCurrentAuth({
            isAuthenticated,
            isLocalMode,
            hasAppAccess,
            credentials,
            login,
            loginForLocalToken,
            logout,
            reloadEncryption,
        });
    }, [hasAppAccess, isAuthenticated, isLocalMode, credentials]);

    // Plan A: Rust AuthStore is authoritative. Listen for its rotation events
    // so localStorage + React state mirror the daemon's token family without
    // us ever holding a revoked refresh token.
    useEffect(() => {
        installRustAuthSync().catch((err) => {
            console.warn('[AuthContext] installRustAuthSync failed:', err);
        });
        const unsubscribe = onTokensRotated((rotated) => {
            syncSetLocalModeCredentials(rotated);
            setCredentials(rotated);
            setIsAuthenticated(true);
        });
        return unsubscribe;
    }, []);

    const login = async (
        response: AuthSuccessPayload | string,
        extras?: { machineId?: string },
    ) => {
        const payload = coerceAuthResponse(response);
        const newCredentials = credentialsFromAuthResponse(payload, {
            machineId: extras?.machineId,
        });
        await saveCredentials(newCredentials);
        await syncCreate(newCredentials);
        setCredentials(newCredentials);
        setIsAuthenticated(true);
        setIsLocalMode(false);
    };

    const loginForLocalToken = async (
        response: AuthSuccessPayload | string,
        extras?: { machineId?: string },
    ) => {
        const payload = coerceAuthResponse(response);
        const newCredentials = credentialsFromAuthResponse(payload, {
            machineId: extras?.machineId,
        });
        await saveCredentials(newCredentials);
        syncSetLocalModeCredentials(newCredentials);
        setCredentials(newCredentials);
        setIsAuthenticated(true);
        setIsLocalMode(true);
    };

    const logout = async () => {
        trackLogout();

        // Signal Machine daemon to tear down and re-enter auth flow
        if (Platform.OS === 'web' && typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
            try {
                const { invoke } = await import('@tauri-apps/api/core');
                await invoke('trigger_machine_reauth');
                console.log('[Logout] Machine reauth signal sent');
            } catch (e) {
                console.warn('[Logout] Failed to trigger machine reauth:', e);
            }
        }

        clearPersistence();
        await clearCredentials();

        setCredentials(null);
        setIsAuthenticated(false);
        setIsLocalMode(false);

        if (Platform.OS === 'ios') {
            await new Promise(resolve => setTimeout(resolve, 300));
        }

        if (Platform.OS === 'web') {
            window.location.reload();
        } else {
            try {
                await Updates.reloadAsync();
            } catch (error) {
                console.log('Reload failed (expected in dev mode):', error);
            }
        }
    };

    const reloadEncryption = async () => {
        // no-op in 2.0 — encryption layer has been removed
    };

    return (
        <AuthContext.Provider
            value={{
                isAuthenticated,
                isLocalMode,
                hasAppAccess,
                credentials,
                login,
                loginForLocalToken,
                logout,
                reloadEncryption,
            }}
        >
            {children}
        </AuthContext.Provider>
    );
}

export function useAuth() {
    const context = useContext(AuthContext);
    if (context === undefined) {
        throw new Error('useAuth must be used within an AuthProvider');
    }
    return context;
}

// Helper to get current auth state for non-React contexts
let currentAuthState: AuthContextType | null = null;

export function setCurrentAuth(auth: AuthContextType | null) {
    currentAuthState = auth;
}

export function getCurrentAuth(): AuthContextType | null {
    return currentAuthState;
}
