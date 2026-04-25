/**
 * Cteno 2.0 Socket.IO client — plaintext payloads + refresh-aware auth.
 *
 * Notable changes from 1.x:
 *   • No encryption envelope. All RPC params and results are plain JSON.
 *   • Access-token refresh hooks: handles `token:near-expiry` from the server
 *     and re-attempts connect once on `Invalid authentication token`.
 *   • Local IPC path preserved as an optimisation for Tauri desktop builds,
 *     but now delegates entirely to the new `rpc/client.ts` abstraction for
 *     cross-machine calls.
 *   • 401 on `request()` auto-triggers a refresh + retry.
 */
import { io, Socket } from 'socket.io-client';
import { isTauri } from '@/utils/tauri';
import { isServerAvailable } from './serverConfig';
import {
    ensureFreshAccess,
    forceRefresh,
    AuthExpiredError,
} from '@/auth/refresh';
import { TokenStorage } from '@/auth/tokenStorage';

// Cached Tauri invoke function for local RPC gateway.
let _tauriInvoke: ((cmd: string, args: any) => Promise<any>) | null | false = null;
let _localHostInfoPromise: Promise<LocalHostInfo | null> | null = null;
let _localHostInfoCache: LocalHostInfo | null = null;

/**
 * Call an RPC method directly via Tauri IPC, bypassing Happy Server.
 * Throws if not in Tauri environment or if the RPC handler is not registered.
 */
async function localRpc<R>(scopeId: string, method: string, params: any): Promise<R> {
    if (_tauriInvoke === false) throw new Error('not tauri');
    if (!_tauriInvoke) {
        if (!isTauri()) { _tauriInvoke = false; throw new Error('not tauri'); }
        const { invoke } = await import('@tauri-apps/api/core');
        _tauriInvoke = invoke;
    }
    return await _tauriInvoke('local_rpc', { method, scopeId, params }) as R;
}

export interface LocalHostInfo {
    machineId: string;
    shellKind: string;
    localRpcEnvTag: string;
    appDataDir: string;
    host: string;
    platform: string;
    happyCliVersion: string;
    happyHomeDir: string;
    homeDir: string;
}

export async function getLocalHostInfo(): Promise<LocalHostInfo | null> {
    if (_tauriInvoke === false) return null;
    if (_localHostInfoCache) return _localHostInfoCache;
    if (_localHostInfoPromise) return _localHostInfoPromise;
    _localHostInfoPromise = (async () => {
        if (!_tauriInvoke) {
            if (!isTauri()) {
                _tauriInvoke = false;
                return null;
            }
            const { invoke } = await import('@tauri-apps/api/core');
            _tauriInvoke = invoke;
        }
        try {
            const info = await _tauriInvoke('get_local_host_info', {}) as LocalHostInfo;
            _localHostInfoCache = info;
            return info;
        } catch {
            return null;
        } finally {
            _localHostInfoPromise = null;
        }
    })();
    return _localHostInfoPromise;
}

//
// Types
//

export interface SyncSocketConfig {
    endpoint: string;
    token: string;
}

export interface SyncSocketState {
    isConnected: boolean;
    connectionStatus: 'disconnected' | 'connecting' | 'connected' | 'error';
    lastError: Error | null;
}

export type SyncSocketListener = (state: SyncSocketState) => void;

/**
 * Normalise an RPC response payload — some servers still wrap JSON in a
 * `{t:'plaintext', c:'<json>'}` envelope for backwards compatibility.
 */
function normalizeRPCPayload(payload: any): unknown {
    if (payload === null || payload === undefined) return payload;
    if (typeof payload === 'string') {
        const trimmed = payload.trim();
        if (trimmed.startsWith('{') || trimmed.startsWith('[')) {
            try {
                return JSON.parse(payload);
            } catch {
                return payload;
            }
        }
        return payload;
    }
    if (typeof payload === 'object') {
        if (payload?.t === 'plaintext') {
            return typeof payload.c === 'string' ? safeParseJson(payload.c) : payload.c;
        }
        if (payload?.t === 'encrypted') {
            // Server should never send this in 2.0; pass the raw string through
            // so the caller notices rather than silently receiving undefined.
            console.warn('[apiSocket] Dropping legacy encrypted RPC payload');
            return typeof payload.c === 'string' ? payload.c : null;
        }
    }
    return payload;
}

function safeParseJson(raw: string): unknown {
    try {
        return JSON.parse(raw);
    } catch {
        return raw;
    }
}

function formatRpcError(error: unknown): string {
    if (error instanceof Error) {
        return `${error.name}: ${error.message}`;
    }
    return String(error);
}

function logLocalRpcFailure(kind: 'sessionRPC' | 'machineRPC', scopeId: string, method: string, error: unknown) {
    console.warn(`[apiSocket] ${kind} localRpc failed for ${scopeId}:${method}: ${formatRpcError(error)}`, error);
}

function getMachineRpcOfflineFallback(method: string): unknown {
    switch (method) {
        case 'list-agents':
            return { agents: [] };
        case 'get-notification-subscriptions':
            return { subscriptions: [] };
        case 'list_available_vendors':
            return [
                {
                    name: 'cteno',
                    available: true,
                    capabilities: {
                        setModel: true,
                        setPermissionMode: false,
                        setSandboxPolicy: true,
                        abort: true,
                        compact: true,
                    },
                },
                {
                    name: 'claude',
                    available: true,
                    capabilities: {
                        setModel: true,
                        setPermissionMode: true,
                        setSandboxPolicy: true,
                        abort: true,
                        compact: true,
                    },
                },
                {
                    name: 'codex',
                    available: false,
                    capabilities: {
                        setModel: false,
                        setPermissionMode: true,
                        setSandboxPolicy: true,
                        abort: true,
                        compact: false,
                    },
                },
            ];
        default:
            return undefined;
    }
}

//
// Main Class
//

class ApiSocket {

    // State
    private socket: Socket | null = null;
    private config: SyncSocketConfig | null = null;
    private messageHandlers: Map<string, (data: any) => void> = new Map();
    private reconnectedListeners: Set<() => void> = new Set();
    private statusListeners: Set<(status: 'disconnected' | 'connecting' | 'connected' | 'error') => void> = new Set();
    private currentStatus: 'disconnected' | 'connecting' | 'connected' | 'error' = 'disconnected';
    private authRetryInflight = false;

    //
    // Initialization
    //

    /**
     * Initialise the socket with the given endpoint + access token.  The
     * optional `_encryption` argument is accepted (and ignored) purely for
     * backward compatibility with call sites that still pass the old
     * Encryption singleton.
     */
    initialize(config: SyncSocketConfig, _encryption?: unknown) {
        console.log('[apiSocket] Initializing with endpoint:', config.endpoint);
        this.config = config;
        if (!this.canUseServerConnection(config)) {
            this.disconnect();
            return;
        }
        this.connect();
    }

    /**
     * Legacy API kept for compatibility. Encryption is a no-op in 2.0, so
     * this is intentionally empty.
     */
    updateEncryption(_encryption: unknown) {
        // noop — encryption was removed in 2.0
    }

    //
    // Connection Management
    //

    connect() {
        if (!this.canUseServerConnection()) {
            this.disconnect();
            return;
        }

        if (!this.config || this.socket) {
            console.log('[apiSocket] Skip connect: config=', !!this.config, 'socket=', !!this.socket);
            return;
        }

        console.log('[apiSocket] Connecting to:', this.config.endpoint, 'clientType: user-scoped');
        this.updateStatus('connecting');

        this.socket = io(this.config.endpoint, {
            path: '/v1/updates',
            auth: {
                token: this.config.token,
                clientType: 'user-scoped' as const,
            },
            transports: ['websocket'],
            reconnection: true,
            reconnectionDelay: 1000,
            reconnectionDelayMax: 5000,
            reconnectionAttempts: Infinity,
            timeout: 30000,
            ackTimeout: 30000,
        });

        this.setupEventHandlers();
        console.log('[apiSocket] Socket created, handlers set up');
    }

    disconnect() {
        if (this.socket) {
            this.socket.disconnect();
            this.socket = null;
        }
        this.updateStatus('disconnected');
    }

    //
    // Listener Management
    //

    onReconnected = (listener: () => void) => {
        this.reconnectedListeners.add(listener);
        return () => this.reconnectedListeners.delete(listener);
    };

    onStatusChange = (listener: (status: 'disconnected' | 'connecting' | 'connected' | 'error') => void) => {
        this.statusListeners.add(listener);
        listener(this.currentStatus);
        return () => this.statusListeners.delete(listener);
    };

    //
    // Message Handling
    //

    onMessage(event: string, handler: (data: any) => void) {
        this.messageHandlers.set(event, handler);
        return () => this.messageHandlers.delete(event);
    }

    offMessage(event: string, _handler: (data: any) => void) {
        this.messageHandlers.delete(event);
    }

    /**
     * Route a locally-sourced event (e.g. from the Tauri `local-host-event`
     * bridge) through the same handler registry used by Socket.IO messages.
     * Lets callers add a single subscriber via `onMessage` and receive both
     * remote (socket) and local (Tauri) deliveries without double wiring.
     */
    dispatchLocalMessage(event: string, data: any) {
        const handler = this.messageHandlers.get(event);
        if (handler) {
            handler(data);
        }
    }

    /**
     * RPC call for sessions. Tries local IPC first (desktop) and falls back
     * to Socket.IO. Payloads are plaintext JSON.
     */
    async sessionRPC<R, A>(sessionId: string, method: string, params: A): Promise<R> {
        let localRpcError: unknown = null;
        try {
            return await localRpc<R>(sessionId, method, params);
        } catch (error) {
            localRpcError = error;
            logLocalRpcFailure('sessionRPC', sessionId, method, error);
        }

        if (!this.canUseServerConnection()) {
            if (isTauri() && localRpcError) {
                throw localRpcError instanceof Error
                    ? localRpcError
                    : new Error(`Local RPC failed: ${formatRpcError(localRpcError)}`);
            }
            throw new Error('Server unavailable');
        }

        const result = await this.socket!.emitWithAck('rpc-call', {
            method: `${sessionId}:${method}`,
            params,
        });

        if (result?.ok) {
            return normalizeRPCPayload(result.result) as R;
        }
        const err = String(result?.error || result?.message || '');
        throw new Error(err || `RPC call failed: ${method}`);
    }

    /**
     * RPC call for machines. Tries local IPC first (desktop) and falls back
     * to Socket.IO. Payloads are plaintext JSON.
     */
    async machineRPC<R, A>(machineId: string, method: string, params: A): Promise<R> {
        let localRpcError: unknown = null;

        try {
            return await localRpc<R>(machineId, method, params);
        } catch (error) {
            localRpcError = error;
            logLocalRpcFailure('machineRPC', machineId, method, error);
        }

        if (!this.canUseServerConnection()) {
            const fallback = getMachineRpcOfflineFallback(method);
            if (fallback !== undefined) {
                console.warn(`[machineRPC] ${method} local RPC failed and server is unavailable; returning offline fallback`);
                return fallback as R;
            }
            if (isTauri() && localRpcError) {
                throw localRpcError instanceof Error
                    ? localRpcError
                    : new Error(`Local RPC failed: ${formatRpcError(localRpcError)}`);
            }
            throw new Error('Server unavailable');
        }

        await this.waitForSocketConnected();

        const maxAttempts = 10;
        for (let attempt = 0; attempt < maxAttempts; attempt += 1) {
            const result = await this.socket!.emitWithAck('rpc-call', {
                method: `${machineId}:${method}`,
                params,
            });

            if (result?.ok) {
                return normalizeRPCPayload(result.result) as R;
            }

            const err = String(result?.error || result?.message || '');
            const retriableRpcError =
                /RPC method not available/i.test(err) ||
                /Target socket disconnected/i.test(err);
            if (retriableRpcError && attempt < (maxAttempts - 1)) {
                try {
                    await this.waitForSocketConnected(15000);
                } catch (_e) {
                    // fall through to backoff + retry
                }
                await new Promise((resolve) =>
                    setTimeout(resolve, Math.min(500 * (attempt + 1), 2500)),
                );
                continue;
            }

            console.error(`[machineRPC] ${method} failed:`, JSON.stringify(result));
            throw new Error(err || `RPC call failed: ${method}`);
        }

        throw new Error(`RPC call failed: ${method}`);
    }

    send(event: string, data: any) {
        if (!this.socket) {
            return false;
        }
        this.socket!.emit(event, data);
        return true;
    }

    async emitWithAck<T = any>(event: string, data: any): Promise<T> {
        if (!this.socket) {
            throw new Error('Socket not connected');
        }
        return await this.socket.emitWithAck(event, data);
    }

    async waitUntilConnected(timeoutMs: number = 10000): Promise<void> {
        await this.waitForSocketConnected(timeoutMs);
    }

    //
    // HTTP Requests — access-token aware.
    //

    async request(path: string, options?: RequestInit): Promise<Response> {
        if (!this.config || !this.canUseServerConnection()) {
            throw new Error('SyncSocket not initialized');
        }

        // Pro-actively refresh if we're close to expiry.
        let token: string | null;
        try {
            token = await ensureFreshAccess();
        } catch (err) {
            if (err instanceof AuthExpiredError) {
                this.disconnect();
                throw err;
            }
            token = (await TokenStorage.getCredentials())?.accessToken ?? null;
        }

        if (!token?.trim()) {
            this.disconnect();
            throw new Error('No authentication credentials');
        }

        // Keep config.token in sync so reconnects use the fresh value.
        if (token !== this.config.token) {
            this.updateToken(token);
        }

        const url = `${this.config.endpoint}${path}`;
        const buildInit = (authToken: string): RequestInit => ({
            ...options,
            headers: {
                ...options?.headers,
                Authorization: `Bearer ${authToken}`,
            },
        });

        let response = await fetch(url, buildInit(token));
        if (response.status === 401) {
            try {
                const fresh = await forceRefresh();
                if (fresh?.accessToken) {
                    this.updateToken(fresh.accessToken);
                    response = await fetch(url, buildInit(fresh.accessToken));
                }
            } catch (err) {
                if (err instanceof AuthExpiredError) {
                    this.disconnect();
                    throw err;
                }
                console.warn('[apiSocket] request refresh failed:', err);
            }
        }
        return response;
    }

    //
    // Token Management
    //

    updateToken(newToken: string) {
        if (this.config && this.config.token !== newToken) {
            this.config.token = newToken;

            if (!this.canUseServerConnection()) {
                this.disconnect();
                return;
            }

            if (this.socket) {
                this.socket.auth = { token: newToken, clientType: 'user-scoped' as const } as any;
                this.socket.disconnect();
                this.socket.connect();
            } else {
                this.connect();
            }
        }
    }

    //
    // Private Methods
    //

    private updateStatus(status: 'disconnected' | 'connecting' | 'connected' | 'error') {
        if (this.currentStatus !== status) {
            this.currentStatus = status;
            this.statusListeners.forEach(listener => listener(status));
        }
    }

    private setupEventHandlers() {
        if (!this.socket) return;

        this.socket.on('connect', () => {
            this.updateStatus('connected');
            if (!this.socket?.recovered) {
                this.reconnectedListeners.forEach(listener => listener());
            }
        });

        this.socket.on('disconnect', (_reason) => {
            this.updateStatus('disconnected');
        });

        this.socket.on('connect_error', async (error) => {
            this.updateStatus('error');
            const message = String((error as any)?.message || error || '');
            if (this.authRetryInflight) return;
            if (/Invalid authentication token/i.test(message)) {
                this.authRetryInflight = true;
                try {
                    const fresh = await forceRefresh();
                    if (fresh?.accessToken) {
                        this.updateToken(fresh.accessToken);
                    }
                } catch (refreshError) {
                    if (refreshError instanceof AuthExpiredError) {
                        this.disconnect();
                    } else {
                        console.warn('[apiSocket] auth refresh failed after connect_error:', refreshError);
                    }
                } finally {
                    this.authRetryInflight = false;
                }
            }
        });

        this.socket.on('error', (_error) => {
            this.updateStatus('error');
        });

        // 2.0: Server proactively notifies the client when the access token
        // is <5 minutes from expiry. Force-refresh and reconnect with the
        // new token so long-lived sockets never break mid-session.
        this.socket.on('token:near-expiry', async () => {
            try {
                const fresh = await forceRefresh();
                if (fresh?.accessToken) {
                    this.updateToken(fresh.accessToken);
                }
            } catch (err) {
                if (err instanceof AuthExpiredError) {
                    this.disconnect();
                } else {
                    console.warn('[apiSocket] token:near-expiry refresh failed:', err);
                }
            }
        });

        // Generic message fan-out
        this.socket.onAny((event, data) => {
            const handler = this.messageHandlers.get(event);
            if (handler) {
                handler(data);
            }
        });
    }

    private canUseServerConnection(config: SyncSocketConfig | null = this.config): boolean {
        return !!config?.token?.trim() && isServerAvailable(config.endpoint);
    }

    private async waitForSocketConnected(timeoutMs: number = 10000): Promise<void> {
        if (!this.canUseServerConnection()) {
            throw new Error('Server unavailable');
        }
        const start = Date.now();
        while (Date.now() - start < timeoutMs) {
            if (this.socket?.connected) {
                return;
            }
            if (!this.socket && this.config) {
                this.connect();
            }
            await new Promise((resolve) => setTimeout(resolve, 120));
        }
        throw new Error('Socket not connected');
    }
}

//
// Singleton Export
//

export const apiSocket = new ApiSocket();
