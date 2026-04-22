/**
 * Cteno 2.0 unified RPC client abstraction.
 *
 * Two target flavours:
 *   • `'local'`  — invoke the host `RpcRegistry` over Tauri IPC. Used on the
 *                  desktop build whenever the caller wants to talk to the
 *                  current machine's daemon.
 *   • `{ machineId }` — route through Socket.IO `rpc-call` to the machine-
 *                       scoped socket identified by `machineId`. Works for
 *                       mobile / web, and for the desktop when the user has
 *                       "pinned" another machine via the machine switcher.
 *
 * All payloads are plaintext JSON. There is no longer any per-session or
 * per-machine encryption layer — the 2.0 server handles that for us.
 */
import { apiSocket, getLocalHostInfo } from '@/sync/apiSocket';
import { isTauri } from '@/utils/tauri';

export type RpcTarget = 'local' | { machineId: string };

const RPC_TIMEOUT_MS = 30_000;

let _tauriInvoke: ((cmd: string, args: unknown) => Promise<unknown>) | null | false = null;

async function getTauriInvoke(): Promise<((cmd: string, args: unknown) => Promise<unknown>) | null> {
    if (_tauriInvoke === false) return null;
    if (_tauriInvoke) return _tauriInvoke;
    if (!isTauri()) {
        _tauriInvoke = false;
        return null;
    }
    const { invoke } = await import('@tauri-apps/api/core');
    _tauriInvoke = invoke as (cmd: string, args: unknown) => Promise<unknown>;
    return _tauriInvoke;
}

async function callLocalIpc<R>(scopeId: string, method: string, params: unknown): Promise<R> {
    const invoke = await getTauriInvoke();
    if (!invoke) {
        throw new Error('Local RPC unavailable: not in Tauri environment');
    }
    return (await invoke('local_rpc', { method, scopeId, params })) as R;
}

async function resolveLocalScopeId(): Promise<string> {
    const info = await getLocalHostInfo();
    if (!info?.machineId) {
        throw new Error('Local RPC unavailable: machine identity not yet known');
    }
    return info.machineId;
}

/**
 * Invoke an RPC method on the requested target. Throws on transport errors,
 * timeouts, or when the target socket reports `ok: false`.
 */
export async function rpcCall<T = unknown>(
    method: string,
    params: unknown,
    target: RpcTarget = 'local',
): Promise<T> {
    if (target === 'local') {
        // Prefer Tauri IPC; fall back to Socket.IO using the local machine id
        // (needed on mobile where Tauri IPC does not exist).
        if (isTauri()) {
            const scopeId = await resolveLocalScopeId();
            return await callLocalIpc<T>(scopeId, method, params);
        }
        const scopeId = await resolveLocalScopeId();
        return await rpcCallViaSocket<T>(scopeId, method, params);
    }
    return await rpcCallViaSocket<T>(target.machineId, method, params);
}

async function rpcCallViaSocket<T>(
    machineId: string,
    method: string,
    params: unknown,
): Promise<T> {
    const scopedMethod = `${machineId}:${method}`;
    const result = await withTimeout(
        apiSocket.emitWithAck('rpc-call', { method: scopedMethod, params }),
        RPC_TIMEOUT_MS,
        `RPC call timed out: ${method}`,
    );

    const raw = result as { ok: boolean; result?: unknown; error?: string; message?: string };
    if (raw?.ok) {
        return normalizeResponsePayload(raw.result) as T;
    }
    const errMessage = String(raw?.error || raw?.message || `RPC call failed: ${method}`);
    throw new Error(errMessage);
}

function normalizeResponsePayload(payload: unknown): unknown {
    if (payload === null || payload === undefined) return payload;
    if (typeof payload === 'string') {
        // Server-side still wraps some legacy payloads in {t:'plaintext', c:'<json>'}
        // envelopes; unwrap transparently so callers always see the plain object.
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
        const obj = payload as { t?: string; c?: unknown };
        if (obj?.t === 'plaintext') {
            return typeof obj.c === 'string' ? safeParseJson(obj.c) : obj.c;
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

async function withTimeout<T>(promise: Promise<T>, ms: number, message: string): Promise<T> {
    let timer: ReturnType<typeof setTimeout> | null = null;
    const timeoutPromise = new Promise<T>((_, reject) => {
        timer = setTimeout(() => reject(new Error(message)), ms);
    });
    try {
        return await Promise.race([promise, timeoutPromise]);
    } finally {
        if (timer) clearTimeout(timer);
    }
}
