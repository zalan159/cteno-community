/**
 * Session operations for remote procedure calls
 * Provides strictly typed functions for all session-related RPC operations
 */

import { apiSocket } from './apiSocket';
import { fetchPublicProxyModels } from './apiBalance';
import { filterProxyModelsForAuth, mergeModelsWithServerProxyModels } from './modelOptions';
import {
    loadCachedVendorModelCatalog,
    saveCachedVendorModelCatalog,
} from './modelCatalogCache';
import { sync } from './sync';
import { isServerAvailable } from './serverConfig';
import { TokenStorage } from '@/auth/tokenStorage';
import { storage } from './storage';
import { kvGet, kvSet } from './apiKv';
import { frontendLog } from '@/utils/tauri';
import type {
    MachineMetadata,
    Persona,
    PersonaTaskSummary,
    Session,
    WorkspaceDispatch,
    WorkspaceEvent,
    WorkspaceRuntimeState,
    WorkspaceTurnPlan,
    WorkspaceSummary,
    WorkspaceWorkflowVoteResponse,
    WorkspaceWorkflowVoteWindow,
} from './storageTypes';

// Strict type definitions for all operations

// Permission operation types
interface SessionPermissionRequest {
    id: string;
    approved: boolean;
    reason?: string;
    mode?: 'default' | 'auto' | 'acceptEdits' | 'plan' | 'dontAsk' | 'bypassPermissions';
    allowTools?: string[];
    decision?: 'approved' | 'approved_for_session' | 'denied' | 'abort';
    // Vendor-specific option id echoed back from `_vendor_options[]` in the
    // permission-request tool input. Today only gemini's ACP adapter uses
    // this (proceed_once / proceed_always / cancel / ...). When set, the
    // Rust side bypasses the Allow/Deny mapping and forwards the id as-is.
    vendorOption?: string;
}

interface SessionElicitationRequest {
    id: string;
    response: {
        action: 'accept' | 'decline' | 'cancel';
        content?: Record<string, unknown>;
    };
}

// Mode change operation types
interface SessionModeChangeRequest {
    to: 'remote' | 'local';
}

// Bash operation types
interface SessionBashRequest {
    command: string;
    cwd?: string;
    timeout?: number;
}

interface SessionBashResponse {
    success: boolean;
    stdout: string;
    stderr: string;
    exitCode: number;
    error?: string;
}

// Read file operation types
interface SessionReadFileRequest {
    path: string;
}

interface SessionReadFileResponse {
    success: boolean;
    content?: string; // base64 encoded
    error?: string;
}

// Write file operation types
interface SessionWriteFileRequest {
    path: string;
    content: string; // base64 encoded
    expectedHash?: string | null;
}

interface SessionWriteFileResponse {
    success: boolean;
    hash?: string;
    error?: string;
}

// List directory operation types
interface SessionListDirectoryRequest {
    path: string;
}

interface DirectoryEntry {
    name: string;
    type: 'file' | 'directory' | 'other';
    size?: number;
    modified?: number;
}

interface SessionListDirectoryResponse {
    success: boolean;
    entries?: DirectoryEntry[];
    error?: string;
}

// Directory tree operation types
interface SessionGetDirectoryTreeRequest {
    path: string;
    maxDepth: number;
}

interface TreeNode {
    name: string;
    path: string;
    type: 'file' | 'directory';
    size?: number;
    modified?: number;
    children?: TreeNode[];
}

interface SessionGetDirectoryTreeResponse {
    success: boolean;
    tree?: TreeNode;
    error?: string;
}

// Ripgrep operation types
interface SessionRipgrepRequest {
    args: string[];
    cwd?: string;
}

interface SessionRipgrepResponse {
    success: boolean;
    exitCode?: number;
    stdout?: string;
    stderr?: string;
    error?: string;
}

// Kill session operation types
interface SessionKillRequest {
    // No parameters needed
}

interface SessionKillResponse {
    success: boolean;
    message: string;
}

// Response types for spawn session
export type SpawnSessionResult =
    | { type: 'success'; sessionId: string }
    | { type: 'requestToApproveDirectoryCreation'; directory: string }
    | { type: 'error'; errorMessage: string };

// Options for spawning a session
export interface SpawnSessionOptions {
    machineId: string;
    directory: string;
    approvedNewDirectoryCreation?: boolean;
    token?: string;
    agent?: VendorName;
    // Selected model ID for the session
    modelId?: string;
    reasoningEffort?: 'low' | 'medium' | 'high';
    // Environment variables from AI backend profile (legacy)
    environmentVariables?: Record<string, string>;
}

// Exported session operation functions

/**
 * Spawn a new remote session on a specific machine
 */
export async function machineSpawnNewSession(options: SpawnSessionOptions): Promise<SpawnSessionResult> {

    const { machineId, directory, approvedNewDirectoryCreation = false, token, agent, modelId, reasoningEffort, environmentVariables } = options;

    try {
        const result = await apiSocket.machineRPC<SpawnSessionResult, {
            type: 'spawn-in-directory'
            directory: string
            approvedNewDirectoryCreation?: boolean,
            token?: string,
            agent?: VendorName,
            modelId?: string,
            reasoningEffort?: 'low' | 'medium' | 'high',
            environmentVariables?: Record<string, string>;
        }>(
            machineId,
            'spawn-happy-session',
            { type: 'spawn-in-directory', directory, approvedNewDirectoryCreation, token, agent, modelId, reasoningEffort, environmentVariables }
        );
        return result;
    } catch (error) {
        // Handle RPC errors
        return {
            type: 'error',
            errorMessage: error instanceof Error ? error.message : 'Failed to spawn session'
        };
    }
}

/**
 * Ensure a session's Socket.IO connection on the machine is alive.
 * Called before sending a message so that if the session socket died
 * (e.g. laptop sleep, network drop), it gets reconnected first.
 */
export async function machineReconnectSession(machineId: string, sessionId: string, modelId?: string): Promise<{ status: string; dataEncryptionKey?: string; newSessionId?: string }> {
    try {
        return await apiSocket.machineRPC<{ status: string; dataEncryptionKey?: string; newSessionId?: string }, { sessionId: string; modelId?: string }>(
            machineId,
            'reconnect-session',
            { sessionId, ...(modelId ? { modelId } : {}) }
        );
    } catch (error) {
        console.warn('reconnect-session RPC failed:', error);
        return { status: 'error' };
    }
}

/**
 * Stop the daemon on a specific machine
 */
export async function machineStopDaemon(machineId: string): Promise<{ message: string }> {
    const result = await apiSocket.machineRPC<{ message: string }, {}>(
        machineId,
        'stop-daemon',
        {}
    );
    return result;
}

/**
 * Execute a bash command on a specific machine
 */
export async function machineBash(
    machineId: string,
    command: string,
    cwd: string
): Promise<{
    success: boolean;
    stdout: string;
    stderr: string;
    exitCode: number;
}> {
    try {
        const result = await apiSocket.machineRPC<{
            success: boolean;
            stdout: string;
            stderr: string;
            exitCode: number;
        }, {
            command: string;
            cwd: string;
        }>(
            machineId,
            'bash',
            { command, cwd }
        );
        return result;
    } catch (error) {
        return {
            success: false,
            stdout: '',
            stderr: error instanceof Error ? error.message : 'Unknown error',
            exitCode: -1
        };
    }
}

/**
 * Update machine metadata with optimistic concurrency control and automatic retry
 */
export async function machineUpdateMetadata(
    machineId: string,
    metadata: MachineMetadata,
    expectedVersion: number,
    maxRetries: number = 3
): Promise<{ version: number; metadata: string }> {
    let currentVersion = expectedVersion;
    let currentMetadata = { ...metadata };
    let retryCount = 0;

    const machineEncryption = sync.encryption.getMachineEncryption(machineId);
    if (!machineEncryption) {
        throw new Error(`Machine encryption not found for ${machineId}`);
    }

    while (retryCount < maxRetries) {
        const encryptedMetadata = await machineEncryption.encryptRaw(currentMetadata);

        const result = await apiSocket.emitWithAck<{
            result: 'success' | 'version-mismatch' | 'error';
            version?: number;
            metadata?: string;
            message?: string;
        }>('machine-update-metadata', {
            machineId,
            metadata: encryptedMetadata,
            expectedVersion: currentVersion
        });

        if (result.result === 'success') {
            return {
                version: result.version!,
                metadata: result.metadata!
            };
        } else if (result.result === 'version-mismatch') {
            // Get the latest version and metadata from the response
            currentVersion = result.version!;
            const latestMetadata = await machineEncryption.decryptRaw(result.metadata!) as MachineMetadata;

            // Merge our changes with the latest metadata
            // Preserve the displayName we're trying to set, but use latest values for other fields
            currentMetadata = {
                ...latestMetadata,
                displayName: metadata.displayName // Keep our intended displayName change
            };

            retryCount++;

            // If we've exhausted retries, throw error
            if (retryCount >= maxRetries) {
                throw new Error(`Failed to update after ${maxRetries} retries due to version conflicts`);
            }

            // Otherwise, loop will retry with updated version and merged metadata
        } else {
            throw new Error(result.message || 'Failed to update machine metadata');
        }
    }

    throw new Error('Unexpected error in machineUpdateMetadata');
}

/**
 * Abort the current session operation
 */
export async function sessionAbort(sessionId: string): Promise<void> {
    await apiSocket.sessionRPC(sessionId, 'abort', {
        reason: `The user doesn't want to proceed with this tool use. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). STOP what you are doing and wait for the user to tell you how to proceed.`
    });
}

/**
 * Send a running sync tool execution to background.
 * The process continues running and the agent receives a run_id for tracking.
 */
export async function sessionSendToBackground(sessionId: string, callId: string): Promise<void> {
    await apiSocket.sessionRPC(sessionId, 'send-to-background', { callId });
}

/**
 * Allow a permission request
 */
export async function sessionAllow(sessionId: string, id: string, mode?: 'default' | 'auto' | 'acceptEdits' | 'plan' | 'dontAsk' | 'bypassPermissions', allowedTools?: string[], decision?: 'approved' | 'approved_for_session', vendorOption?: string): Promise<void> {
    const request: SessionPermissionRequest = { id, approved: true, mode, allowTools: allowedTools, decision, vendorOption };
    await apiSocket.sessionRPC(sessionId, 'permission', request);
}

export async function sessionRespondToElicitation(
    sessionId: string,
    id: string,
    response: SessionElicitationRequest['response']
): Promise<void> {
    const request: SessionElicitationRequest = { id, response };
    await apiSocket.sessionRPC(sessionId, 'elicitation', request);
}

/**
 * Deny a permission request
 */
export async function sessionDeny(sessionId: string, id: string, mode?: 'default' | 'auto' | 'acceptEdits' | 'plan' | 'dontAsk' | 'bypassPermissions', allowedTools?: string[], decision?: 'denied' | 'abort', vendorOption?: string): Promise<void> {
    const request: SessionPermissionRequest = { id, approved: false, mode, allowTools: allowedTools, decision, vendorOption };
    await apiSocket.sessionRPC(sessionId, 'permission', request);
}

/**
 * Set the permission mode on the backend (standalone, without a permission approval)
 */
export async function sessionSetPermissionMode(
    sessionId: string,
    mode: 'default' | 'auto' | 'acceptEdits' | 'plan' | 'dontAsk' | 'bypassPermissions' | 'read-only' | 'safe-yolo' | 'yolo',
): Promise<void> {
    await apiSocket.sessionRPC(sessionId, 'set-permission-mode', { mode });
}

/**
 * Set the sandbox policy on the backend
 */
export async function sessionSetSandboxPolicy(sessionId: string, policy: 'workspace_write' | 'unrestricted'): Promise<void> {
    try {
        await apiSocket.sessionRPC(sessionId, 'set-sandbox-policy', { policy });
    } catch (error) {
        console.warn('set-sandbox-policy RPC failed:', error);
    }
}

/**
 * Request mode change for a session
 */
export async function sessionSwitch(sessionId: string, to: 'remote' | 'local'): Promise<boolean> {
    const request: SessionModeChangeRequest = { to };
    const response = await apiSocket.sessionRPC<boolean, SessionModeChangeRequest>(
        sessionId,
        'switch',
        request,
    );
    return response;
}

/**
 * Execute a bash command in the session
 */
export async function sessionBash(sessionId: string, request: SessionBashRequest): Promise<SessionBashResponse> {
    try {
        const response = await apiSocket.sessionRPC<SessionBashResponse, SessionBashRequest>(
            sessionId,
            'bash',
            request
        );
        return response;
    } catch (error) {
        return {
            success: false,
            stdout: '',
            stderr: error instanceof Error ? error.message : 'Unknown error',
            exitCode: -1,
            error: error instanceof Error ? error.message : 'Unknown error'
        };
    }
}

/**
 * Read a file from the session
 */
export async function sessionReadFile(sessionId: string, path: string): Promise<SessionReadFileResponse> {
    try {
        const request: SessionReadFileRequest = { path };
        const response = await apiSocket.sessionRPC<SessionReadFileResponse, SessionReadFileRequest>(
            sessionId,
            'readFile',
            request
        );
        return response;
    } catch (error) {
        return {
            success: false,
            error: error instanceof Error ? error.message : 'Unknown error'
        };
    }
}

/**
 * Write a file to the session
 */
export async function sessionWriteFile(
    sessionId: string,
    path: string,
    content: string,
    expectedHash?: string | null
): Promise<SessionWriteFileResponse> {
    try {
        const request: SessionWriteFileRequest = { path, content, expectedHash };
        const response = await apiSocket.sessionRPC<SessionWriteFileResponse, SessionWriteFileRequest>(
            sessionId,
            'writeFile',
            request
        );
        return response;
    } catch (error) {
        return {
            success: false,
            error: error instanceof Error ? error.message : 'Unknown error'
        };
    }
}

/**
 * List directory contents in the session
 */
export async function sessionListDirectory(sessionId: string, path: string): Promise<SessionListDirectoryResponse> {
    try {
        const request: SessionListDirectoryRequest = { path };
        const response = await apiSocket.sessionRPC<SessionListDirectoryResponse, SessionListDirectoryRequest>(
            sessionId,
            'listDirectory',
            request
        );
        return response;
    } catch (error) {
        return {
            success: false,
            error: error instanceof Error ? error.message : 'Unknown error'
        };
    }
}

/**
 * Get directory tree from the session
 */
export async function sessionGetDirectoryTree(
    sessionId: string,
    path: string,
    maxDepth: number
): Promise<SessionGetDirectoryTreeResponse> {
    try {
        const request: SessionGetDirectoryTreeRequest = { path, maxDepth };
        const response = await apiSocket.sessionRPC<SessionGetDirectoryTreeResponse, SessionGetDirectoryTreeRequest>(
            sessionId,
            'getDirectoryTree',
            request
        );
        return response;
    } catch (error) {
        return {
            success: false,
            error: error instanceof Error ? error.message : 'Unknown error'
        };
    }
}

/**
 * Run ripgrep in the session
 */
export async function sessionRipgrep(
    sessionId: string,
    args: string[],
    cwd?: string
): Promise<SessionRipgrepResponse> {
    try {
        const request: SessionRipgrepRequest = { args, cwd };
        const response = await apiSocket.sessionRPC<SessionRipgrepResponse, SessionRipgrepRequest>(
            sessionId,
            'ripgrep',
            request
        );
        return response;
    } catch (error) {
        return {
            success: false,
            error: error instanceof Error ? error.message : 'Unknown error'
        };
    }
}

/**
 * Kill the session process immediately.
 * Tries session-scoped RPC first; falls back to machine-scoped RPC
 * whenever session-scoped RPC is unavailable/unhealthy.
 */
export async function sessionKill(sessionId: string): Promise<SessionKillResponse> {
    // Try session-scoped RPC first (requires session encryption)
    try {
        const response = await apiSocket.sessionRPC<SessionKillResponse, {}>(
            sessionId,
            'killSession',
            {}
        );
        return response;
    } catch (error) {
        const msg = error instanceof Error ? error.message : '';
        // Fallback to machine RPC when session channel is unavailable
        // (e.g. missing encryption, dead session socket, RPC method not registered yet).
        const session = storage.getState().sessions[sessionId];
        const machineId = session?.metadata?.machineId;
        if (machineId) {
            console.warn('[sessionKill] sessionRPC failed, fallback to machineRPC:', msg || 'unknown');
            try {
                return await apiSocket.machineRPC<SessionKillResponse, { sessionId: string }>(
                    machineId,
                    'kill-session',
                    { sessionId }
                );
            } catch (machineError) {
                return {
                    success: false,
                    message: machineError instanceof Error ? machineError.message : 'Machine RPC failed'
                };
            }
        }
        return {
            success: false,
            message: msg || 'Unknown error'
        };
    }
}

/**
 * Permanently delete a session from the server
 * This will remove the session and all its associated data (messages, usage reports, access keys)
 * The session should be inactive/archived before deletion
 */
export async function machineDelete(machineId: string): Promise<{ success: boolean; message?: string }> {
    try {
        const response = await apiSocket.request(`/v1/machines/${machineId}`, {
            method: 'DELETE'
        });

        if (response.ok) {
            return { success: true };
        } else {
            const error = await response.text();
            return {
                success: false,
                message: error || 'Failed to delete machine'
            };
        }
    } catch (error) {
        return {
            success: false,
            message: error instanceof Error ? error.message : 'Unknown error'
        };
    }
}

export async function sessionDelete(sessionId: string): Promise<{ success: boolean; message?: string }> {
    // Relay-only architecture: happy-server no longer owns session state.
    // Route deletion through the owner daemon via machine RPC so the session
    // row is removed from the daemon's local SQLite + any live connection is
    // torn down. Falls back gracefully when the machine id is missing on the
    // cached session record.
    const session = storage.getState().sessions[sessionId];
    const machineId = session?.metadata?.machineId;
    if (!machineId) {
        return {
            success: false,
            message: 'Missing machineId on session — cannot route delete to daemon',
        };
    }
    try {
        const result = await apiSocket.machineRPC<{ success: boolean; message?: string; rowDeleted?: boolean }, { sessionId: string }>(
            machineId,
            'delete-session',
            { sessionId },
        );
        return { success: result.success !== false, message: result.message };
    } catch (error) {
        return {
            success: false,
            message: error instanceof Error ? error.message : 'Unknown error',
        };
    }
}

export async function machineListSessions(machineId: string): Promise<Session[]> {
    try {
        const result = await apiSocket.machineRPC<{ success: boolean; sessions?: Session[] }, {}>(
            machineId,
            'list-sessions',
            {}
        );
        return result.sessions || [];
    } catch (error) {
        console.warn('list-sessions RPC failed:', error);
        return [];
    }
}

export async function machineGetSession(machineId: string, sessionId: string): Promise<Session | null> {
    try {
        const result = await apiSocket.machineRPC<{ success: boolean; session?: Session | null }, { id: string }>(
            machineId,
            'get-session',
            { id: sessionId }
        );
        return result.session || null;
    } catch (error) {
        console.warn('get-session RPC failed:', error);
        return null;
    }
}

// ========== Executor Vendor RPCs (Wave A backend integration) ==========

/**
 * Vendor names surfaced by `ExecutorRegistry::available_vendors` on the
 * Rust side. `cteno` is always available; `claude`, `codex`, and `gemini`
 * depend on whether the corresponding CLI is installed on the host.
 */
export type VendorName = 'cteno' | 'claude' | 'codex' | 'gemini';

export type WorkspaceTemplateId = 'group-chat' | 'gated-tasks' | 'autoresearch';

export type WorkspaceRoleVendorOverrides = Partial<Record<string, VendorName>>;

export type RuntimeControlOutcome = 'applied' | 'restart_required' | 'unsupported' | 'failed';

export interface RuntimeControlCapability {
    outcome: RuntimeControlOutcome;
    reason?: string | null;
}

export interface RuntimeControls {
    model: RuntimeControlCapability;
    permissionMode: RuntimeControlCapability;
}

/**
 * Per-vendor capability flags. Keys mirror backend `AgentCapabilities`.
 * Values default to `false` so the UI degrades gracefully when the backend
 * has not yet landed a given capability.
 */
export interface AgentCapabilities {
    setModel?: boolean;
    setPermissionMode?: boolean;
    setSandboxPolicy?: boolean;
    abort?: boolean;
    compact?: boolean;
    runtimeControls?: RuntimeControls;
    // Catch-all for future capabilities — the UI reads arbitrary string keys
    // through the `useCapability` hook.
    [key: string]: boolean | RuntimeControls | undefined;
}

export type VendorInstallState = 'installed' | 'notInstalled';
export type VendorAuthState = 'unknown' | 'notRequired' | 'loggedOut' | 'loggedIn';
export type VendorConnectionState = 'unknown' | 'probing' | 'connected' | 'failed';

export interface VendorConnectionMeta {
    state: VendorConnectionState;
    reason?: string;
    checkedAtUnixMs: number;
    latencyMs?: number;
}

export interface VendorStatusMeta {
    installState?: VendorInstallState;
    authState?: VendorAuthState;
    accountAuthenticated?: boolean;
    machineAuthenticated?: boolean;
    connection?: VendorConnectionMeta;
}

export interface VendorMeta {
    name: VendorName;
    available: boolean;
    installed?: boolean;
    loggedIn?: boolean | null;
    capabilities: AgentCapabilities;
    status?: VendorStatusMeta;
    connection?: VendorConnectionMeta;
}

export interface ResolvedVendorStatusMeta {
    installState: VendorInstallState;
    authState: VendorAuthState;
    accountAuthenticated?: boolean;
    machineAuthenticated?: boolean;
    connection: VendorConnectionMeta;
}

export interface ResolvedVendorMeta extends Omit<VendorMeta, 'installed' | 'loggedIn' | 'status' | 'connection'> {
    installed: boolean;
    loggedIn: boolean | null;
    status: ResolvedVendorStatusMeta;
}

export function resolveVendorMeta(vendor: VendorMeta): ResolvedVendorMeta {
    const { connection: topLevelConnection, ...vendorWithoutTopLevelConnection } = vendor;
    const installed = vendor.installed ?? vendor.available;
    const installState = vendor.status?.installState ?? (installed ? 'installed' : 'notInstalled');
    const authState = vendor.status?.authState
        ?? (vendor.loggedIn === true
            ? 'loggedIn'
            : vendor.loggedIn === false
                ? 'loggedOut'
                : 'unknown');
    const loggedIn = vendor.loggedIn
        ?? (authState === 'loggedIn'
            ? true
            : authState === 'loggedOut'
                ? false
                : null);

    return {
        ...vendorWithoutTopLevelConnection,
        available: installed,
        installed,
        loggedIn,
        status: {
            installState,
            authState,
            accountAuthenticated: vendor.status?.accountAuthenticated,
            machineAuthenticated: vendor.status?.machineAuthenticated,
            connection: vendor.status?.connection ?? topLevelConnection ?? {
                state: 'unknown',
                checkedAtUnixMs: 0,
            },
        },
    };
}

export function normalizeVendorList(vendors: VendorMeta[]): ResolvedVendorMeta[] {
    return vendors.map(resolveVendorMeta);
}

interface RuntimeControlCapabilityLike {
    outcome?: string;
    reason?: string | null;
}

interface RuntimeControlsLike {
    model?: RuntimeControlCapabilityLike | null;
    permissionMode?: RuntimeControlCapabilityLike | null;
    permission_mode?: RuntimeControlCapabilityLike | null;
}

interface VendorConnectionMetaLike {
    state?: string;
    reason?: string | null;
    checkedAtUnixMs?: number;
    checked_at_unix_ms?: number;
    latencyMs?: number | null;
    latency_ms?: number | null;
}

interface VendorStatusMetaLike {
    installState?: VendorInstallState;
    install_state?: VendorInstallState;
    authState?: VendorAuthState;
    auth_state?: VendorAuthState;
    accountAuthenticated?: boolean;
    account_authenticated?: boolean;
    machineAuthenticated?: boolean;
    machine_authenticated?: boolean;
    connection?: VendorConnectionMetaLike | null;
}

interface VendorMetaLike {
    name: VendorName;
    available?: boolean;
    installed?: boolean;
    loggedIn?: boolean | null;
    logged_in?: boolean | null;
    connection?: VendorConnectionMetaLike | null;
    status?: VendorStatusMetaLike | null;
    capabilities?: AgentCapabilities & {
        runtimeControls?: RuntimeControlsLike;
        runtime_controls?: RuntimeControlsLike;
    };
}

interface ProbeVendorConnectionEnvelope {
    success?: boolean;
    vendor?: VendorName;
    connection?: VendorConnectionMetaLike | null;
    error?: string | null;
}

type ProbeVendorConnectionRpcResponse = VendorConnectionMetaLike | ProbeVendorConnectionEnvelope;

function isProbeVendorConnectionEnvelope(
    raw: ProbeVendorConnectionRpcResponse,
): raw is ProbeVendorConnectionEnvelope {
    return 'success' in raw || 'connection' in raw || 'vendor' in raw || 'error' in raw;
}

function normalizeVendorConnectionMeta(
    raw: VendorConnectionMetaLike | null | undefined,
): VendorConnectionMeta | undefined {
    if (!raw) return undefined;
    const state = raw.state;
    const normalizedState: VendorConnectionState =
        state === 'probing' || state === 'connected' || state === 'failed' || state === 'unknown'
            ? state
            : 'unknown';
    const checkedAtUnixMs =
        typeof raw.checkedAtUnixMs === 'number'
            ? raw.checkedAtUnixMs
            : typeof raw.checked_at_unix_ms === 'number'
                ? raw.checked_at_unix_ms
                : 0;
    const latency = raw.latencyMs ?? raw.latency_ms;
    return {
        state: normalizedState,
        reason: typeof raw.reason === 'string' ? raw.reason : undefined,
        checkedAtUnixMs,
        latencyMs: typeof latency === 'number' ? latency : undefined,
    };
}

function normalizeVendorStatusMeta(
    raw: VendorStatusMetaLike | null | undefined,
): VendorStatusMeta | undefined {
    if (!raw) return undefined;
    const installState = raw.installState ?? raw.install_state;
    const authState = raw.authState ?? raw.auth_state;
    const accountAuthenticated = raw.accountAuthenticated ?? raw.account_authenticated;
    const machineAuthenticated = raw.machineAuthenticated ?? raw.machine_authenticated;
    const connection = normalizeVendorConnectionMeta(raw.connection);
    const status: VendorStatusMeta = {};
    if (installState) status.installState = installState;
    if (authState) status.authState = authState;
    if (typeof accountAuthenticated === 'boolean') status.accountAuthenticated = accountAuthenticated;
    if (typeof machineAuthenticated === 'boolean') status.machineAuthenticated = machineAuthenticated;
    if (connection) status.connection = connection;
    return Object.keys(status).length > 0 ? status : undefined;
}

function normalizeRuntimeControlOutcome(value: string | undefined): RuntimeControlOutcome | null {
    if (value === 'applied' || value === 'restart_required' || value === 'unsupported') {
        return value;
    }
    return null;
}

function normalizeRuntimeControlCapability(
    raw: RuntimeControlCapabilityLike | null | undefined,
    fallback: RuntimeControlCapability,
): RuntimeControlCapability {
    const normalizedOutcome = normalizeRuntimeControlOutcome(raw?.outcome);
    if (!normalizedOutcome) {
        return fallback;
    }
    return {
        outcome: normalizedOutcome,
        reason: typeof raw?.reason === 'string' ? raw.reason : null,
    };
}

function legacyModelControl(vendor: VendorName, setModel?: boolean): RuntimeControlCapability {
    if (setModel) {
        return { outcome: 'applied', reason: null };
    }
    if (vendor === 'codex') {
        return {
            outcome: 'restart_required',
            reason: 'Codex needs the session to close and resume before the new model applies.',
        };
    }
    if (vendor === 'gemini') {
        return {
            outcome: 'restart_required',
            reason: 'Gemini keeps the new model for the next session start instead of hot-swapping.',
        };
    }
    return {
        outcome: 'unsupported',
        reason: 'This executor does not support model changes in the current session.',
    };
}

function legacyPermissionControl(setPermissionMode?: boolean): RuntimeControlCapability {
    if (setPermissionMode) {
        return { outcome: 'applied', reason: null };
    }
    return {
        outcome: 'unsupported',
        reason: 'Permission mode is fixed when the session starts for this executor.',
    };
}

function normalizeVendorMetaFromRpc(raw: VendorMetaLike): VendorMeta {
    const capabilities = raw.capabilities ?? {};
    const runtimeControlsRaw = capabilities.runtimeControls ?? capabilities.runtime_controls;
    const modelFallback = legacyModelControl(raw.name, capabilities.setModel);
    const permissionFallback = legacyPermissionControl(capabilities.setPermissionMode);

    const status = normalizeVendorStatusMeta(raw.status);
    const connection = normalizeVendorConnectionMeta(raw.connection);
    const meta: VendorMeta = {
        name: raw.name,
        available: raw.available !== false,
        capabilities: {
            setModel: capabilities.setModel === true,
            setPermissionMode: capabilities.setPermissionMode === true,
            setSandboxPolicy: capabilities.setSandboxPolicy === true,
            abort: capabilities.abort === true,
            compact: capabilities.compact === true,
            runtimeControls: {
                model: normalizeRuntimeControlCapability(runtimeControlsRaw?.model, modelFallback),
                permissionMode: normalizeRuntimeControlCapability(
                    runtimeControlsRaw?.permissionMode ?? runtimeControlsRaw?.permission_mode,
                    permissionFallback,
                ),
            },
        },
    };
    if (typeof raw.installed === 'boolean') meta.installed = raw.installed;
    const loggedIn = raw.loggedIn ?? raw.logged_in;
    if (loggedIn === true || loggedIn === false || loggedIn === null) {
        meta.loggedIn = loggedIn;
    }
    if (status || connection) {
        meta.status = {
            ...(status ?? {}),
            ...(connection ? { connection } : {}),
        };
    }
    return meta;
}

function normalizeProbeVendorConnectionResponse(
    raw: ProbeVendorConnectionRpcResponse,
    vendor: VendorName,
): VendorConnectionMeta {
    if (isProbeVendorConnectionEnvelope(raw)) {
        if (raw.success === false) {
            throw new Error(raw.error || `Failed to probe ${vendor} connection`);
        }
        return normalizeVendorConnectionMeta(raw.connection) ?? {
            state: 'unknown',
            checkedAtUnixMs: 0,
        };
    }
    return normalizeVendorConnectionMeta(raw) ?? {
        state: 'unknown',
        checkedAtUnixMs: 0,
    };
}

function getCurrentSession(sessionId: string): Session | null {
    return storage.getState().sessions[sessionId] ?? null;
}

function updateLocalSessionModelId(sessionId: string, modelId: string) {
    const session = getCurrentSession(sessionId);
    if (!session || !session.metadata) {
        return;
    }
    storage.getState().applySessions([{
        ...session,
        metadata: {
            ...session.metadata,
            modelId,
        },
    }]);
}

function updateLocalSessionRuntimeEffort(
    sessionId: string,
    effort: import('./storageTypes').RuntimeEffort,
) {
    storage.getState().updateSessionRuntimeEffort(sessionId, effort);
}

export interface RuntimeControlActionResult {
    outcome: RuntimeControlOutcome;
    message: string;
}

function describeRuntimeControlResult(
    label: 'model' | 'permission mode',
    capability: RuntimeControlCapability,
    failure?: string,
): RuntimeControlActionResult {
    if (failure) {
        return {
            outcome: 'failed',
            message: failure,
        };
    }
    if (capability.outcome === 'applied') {
        return {
            outcome: capability.outcome,
            message: capability.reason ?? `Updated the session ${label}.`,
        };
    }
    if (capability.outcome === 'restart_required') {
        return {
            outcome: capability.outcome,
            message: capability.reason ?? `Saved the new ${label}, but this session must restart before it applies.`,
        };
    }
    return {
        outcome: capability.outcome,
        message: capability.reason ?? `This session cannot change its ${label} at runtime.`,
    };
}

function isReconnectableSessionRpcError(error: unknown): boolean {
    const message = error instanceof Error ? error.message : String(error);
    return (
        /Unknown method/i.test(message) ||
        /not connected/i.test(message) ||
        /No handler registered/i.test(message)
    );
}

export async function sessionApplyPermissionModeChange(
    sessionId: string,
    mode: 'default' | 'auto' | 'acceptEdits' | 'plan' | 'dontAsk' | 'bypassPermissions' | 'read-only' | 'safe-yolo' | 'yolo',
    capability: RuntimeControlCapability,
): Promise<RuntimeControlActionResult> {
    if (capability.outcome !== 'applied') {
        return describeRuntimeControlResult('permission mode', capability);
    }

    try {
        await sessionSetPermissionMode(sessionId, mode);
        storage.getState().updateSessionPermissionMode(sessionId, mode);
        return describeRuntimeControlResult('permission mode', capability);
    } catch (error) {
        const session = storage.getState().sessions[sessionId];
        const machineId = session?.metadata?.machineId ?? null;
        if (machineId && isReconnectableSessionRpcError(error)) {
            try {
                const reconnect = await machineReconnectSession(machineId, sessionId);
                const targetSessionId =
                    reconnect.status === 'session_replaced' && reconnect.newSessionId
                        ? reconnect.newSessionId
                        : sessionId;
                await sessionSetPermissionMode(targetSessionId, mode);
                storage.getState().updateSessionPermissionMode(targetSessionId, mode);
                return describeRuntimeControlResult('permission mode', capability);
            } catch (retryError) {
                return describeRuntimeControlResult(
                    'permission mode',
                    capability,
                    retryError instanceof Error ? retryError.message : 'Failed to update the session permission mode.',
                );
            }
        }
        return describeRuntimeControlResult(
            'permission mode',
            capability,
            error instanceof Error ? error.message : 'Failed to update the session permission mode.',
        );
    }
}

export async function sessionApplyRuntimeModelChange(
    sessionId: string,
    machineId: string | null | undefined,
    modelId: string,
    reasoningEffort: import('./storageTypes').RuntimeEffort,
    capability: RuntimeControlCapability,
): Promise<RuntimeControlActionResult> {
    if (capability.outcome === 'unsupported') {
        return describeRuntimeControlResult('model', capability);
    }

    const applyLocalState = () => {
        updateLocalSessionModelId(sessionId, modelId);
        updateLocalSessionRuntimeEffort(sessionId, reasoningEffort);
    };

    try {
        const result = await apiSocket.sessionRPC<{
            status: 'ok';
            outcome?: RuntimeControlOutcome;
            reason?: string | null;
        }, {
            modelId: string;
            reasoningEffort?: string;
        }>(
            sessionId,
            'set-model',
            {
                modelId,
                ...(reasoningEffort !== 'default'
                    ? { reasoningEffort }
                    : {}),
            }
        );

        applyLocalState();

        return describeRuntimeControlResult('model', {
            outcome: result.outcome ?? capability.outcome,
            reason: result.reason ?? capability.reason,
        });
    } catch (error) {
        if (!machineId) {
            return describeRuntimeControlResult(
                'model',
                capability,
                error instanceof Error ? error.message : 'Failed to update the session model selection.',
            );
        }
    }

    const fallback = await machineSwitchSessionModel(machineId, sessionId, modelId, reasoningEffort);
    if (!fallback.success) {
        return describeRuntimeControlResult(
            'model',
            capability,
            fallback.error || 'Failed to update the session model selection.',
        );
    }

    applyLocalState();
    return describeRuntimeControlResult('model', {
        outcome: fallback.outcome ?? capability.outcome,
        reason: fallback.reason ?? capability.reason,
    });
}

export const sessionApplyRuntimeProfileChange = sessionApplyRuntimeModelChange;

/**
 * List the executor vendors wired into the current machine's
 * ExecutorRegistry. Falls back to a local mock until the backend RPC
 * lands so the UI remains functional.
 *
 * TODO(wave-a): once `list_available_vendors` is registered on the Rust
 * side, drop the mock branch and surface RPC errors normally.
 */
export async function listAvailableVendors(machineId?: string | null): Promise<ResolvedVendorMeta[]> {
    const mock: VendorMeta[] = [
        {
            name: 'cteno',
            available: true,
            installed: true,
            loggedIn: null,
            capabilities: {
                setModel: true,
                setPermissionMode: true,
                setSandboxPolicy: true,
                abort: true,
                compact: true,
                runtimeControls: {
                    model: {
                        outcome: 'applied',
                        reason: null,
                    },
                    permissionMode: {
                        outcome: 'applied',
                        reason: null,
                    },
                },
            },
            status: {
                installState: 'installed',
                authState: 'unknown',
            },
        },
        {
            name: 'claude',
            available: true,
            installed: true,
            loggedIn: null,
            capabilities: {
                setModel: true,
                setPermissionMode: true,
                setSandboxPolicy: true,
                abort: true,
                compact: true,
                runtimeControls: {
                    model: { outcome: 'applied', reason: null },
                    permissionMode: { outcome: 'applied', reason: null },
                },
            },
            status: {
                installState: 'installed',
                authState: 'unknown',
            },
        },
        {
            name: 'codex',
            available: false,
            installed: false,
            loggedIn: null,
            capabilities: {
                setModel: true,
                setPermissionMode: true,
                setSandboxPolicy: true,
                abort: true,
                compact: false,
                runtimeControls: {
                    model: { outcome: 'applied', reason: null },
                    permissionMode: { outcome: 'applied', reason: null },
                },
            },
            status: {
                installState: 'notInstalled',
                authState: 'unknown',
            },
        },
        {
            name: 'gemini',
            available: false,
            installed: false,
            loggedIn: null,
            capabilities: {
                setModel: true,
                setPermissionMode: true,
                setSandboxPolicy: true,
                abort: true,
                compact: true,
                runtimeControls: {
                    model: { outcome: 'applied', reason: null },
                    permissionMode: { outcome: 'applied', reason: null },
                },
            },
            status: {
                installState: 'notInstalled',
                authState: 'unknown',
            },
        },
    ];

    if (!machineId) {
        return normalizeVendorList(mock);
    }

    try {
        const result = await apiSocket.machineRPC<
            { vendors?: VendorMetaLike[] } | VendorMetaLike[],
            {}
        >(machineId, 'list_available_vendors', {});
        if (Array.isArray(result)) {
            return normalizeVendorList(result.map(normalizeVendorMetaFromRpc));
        }
        if (result && Array.isArray(result.vendors)) {
            return normalizeVendorList(result.vendors.map(normalizeVendorMetaFromRpc));
        }
        return normalizeVendorList(mock);
    } catch (error) {
        // RPC not registered yet on the daemon — fall back to the mock list.
        console.warn('[listAvailableVendors] RPC not available, using mock:', error);
        return normalizeVendorList(mock);
    }
}

/**
 * Probe a vendor's pre-warmed connection. Returns a freshly minted
 * `VendorConnectionMeta` after the daemon finishes its liveness check.
 *
 * Transport split matches `listAvailableVendors`: when no machineId is
 * supplied (local Tauri daemon path) we invoke the Tauri command directly;
 * otherwise we go through the machine-scoped socket RPC.
 */
export async function probeVendorConnection(
    machineId: string | null,
    vendor: VendorName,
): Promise<VendorConnectionMeta> {
    if (!machineId) {
        const { invoke } = await import('@tauri-apps/api/core');
        const raw = (await invoke('probe_vendor_connection', { vendor })) as ProbeVendorConnectionRpcResponse;
        return normalizeProbeVendorConnectionResponse(raw, vendor);
    }
    const raw = await apiSocket.machineRPC<ProbeVendorConnectionRpcResponse, { vendor: VendorName }>(
        machineId,
        'probe_vendor_connection',
        { vendor },
    );
    return normalizeProbeVendorConnectionResponse(raw, vendor);
}

// ========== LLM Profile Management RPCs ==========

// Types for LLM profile system (stored on Machine)
export interface LlmEndpointDisplay {
    api_key_masked: string;
    base_url: string;
    model: string;
    temperature: number;
    max_tokens: number;
    context_window_tokens?: number;
}

export interface ModelOptionDisplay {
    id: string;
    name: string;
    isProxy?: boolean;
    isFree?: boolean;
    sourceType?: 'proxy' | 'vendor' | 'byok';
    vendor?: VendorName;
    supportsVision?: boolean;
    supportsComputerUse?: boolean;
    apiFormat?: 'anthropic' | 'openai' | 'gemini';
    thinking?: boolean;
    supportsFunctionCalling?: boolean;
    supportsImageOutput?: boolean;
    description?: string;
    isDefault?: boolean;
    defaultReasoningEffort?: import('./storageTypes').RuntimeEffort | null;
    supportedReasoningEfforts?: import('./storageTypes').RuntimeEffort[];
    chat: LlmEndpointDisplay;
    compress: LlmEndpointDisplay;
}

export interface LlmEndpointInput {
    api_key: string;
    base_url: string;
    model: string;
    temperature: number;
    max_tokens: number;
    context_window_tokens?: number;
}

export interface LlmProfileInput {
    id: string;
    name: string;
    chat: LlmEndpointInput;
    compress: LlmEndpointInput;
    supports_vision?: boolean;
    supports_computer_use?: boolean;
    thinking?: boolean;
    supports_function_calling?: boolean;
    supports_image_output?: boolean;
    api_format?: 'anthropic' | 'openai' | 'gemini';
}

export interface ListProfilesResponse {
    profiles: ModelOptionDisplay[];
    defaultProfileId: string;
}

export interface ListModelsResponse {
    models: ModelOptionDisplay[];
    defaultModelId: string;
}

interface VendorModelInfoRpc {
    id: string;
    model: string;
    displayName: string;
    description?: string | null;
    vendor: VendorName;
    apiFormat: 'anthropic' | 'openai' | 'gemini';
    isDefault?: boolean;
    defaultReasoningEffort?: import('./storageTypes').RuntimeEffort | null;
    supportedReasoningEfforts?: string[];
    supportsVision?: boolean;
    supportsComputerUse?: boolean;
}

interface RefreshProxyProfilesResponse {
    success: boolean;
    count: number;
    defaultProfileId: string;
}

function inferCtenoReasoningEfforts(model: ModelOptionDisplay): import('./storageTypes').RuntimeEffort[] {
    if (model.supportedReasoningEfforts?.length) {
        return model.supportedReasoningEfforts;
    }

    if (model.thinking !== true) {
        return ['default'];
    }

    const modelId = (model.chat?.model || model.id || '').toLowerCase();
    if (modelId.includes('deepseek-v4')) {
        return ['default', 'high', 'max'];
    }

    return ['default', 'high', 'max'];
}

export async function machineRefreshProxyProfiles(machineId: string): Promise<RefreshProxyProfilesResponse> {
    const timeout = new Promise<never>((_, reject) =>
        setTimeout(() => reject(new Error('Proxy model refresh timed out')), 4000)
    );
    return Promise.race([
        apiSocket.machineRPC<RefreshProxyProfilesResponse, {}>(
            machineId,
            'refresh-proxy-profiles',
            {}
        ),
        timeout
    ]);
}

/** Full profile with plaintext API keys (for cross-device migration) */
export interface LlmProfileFull {
    id: string;
    name: string;
    chat: LlmEndpointInput;
    compress: LlmEndpointInput;
    supports_vision?: boolean;
    supports_computer_use?: boolean;
    thinking?: boolean;
    supports_function_calling?: boolean;
    supports_image_output?: boolean;
    api_format?: 'anthropic' | 'openai' | 'gemini';
}

export interface ExportProfilesResponse {
    profiles: LlmProfileFull[];
    defaultProfileId: string;
}

/**
 * List LLM profiles stored on the Machine
 * Uses a 8-second timeout since profile listing should be near-instant
 */
export async function machineListProfiles(machineId: string): Promise<ListProfilesResponse> {
    const timeout = new Promise<never>((_, reject) =>
        setTimeout(() => reject(new Error('Profile loading timed out')), 8000)
    );
    return Promise.race([
        apiSocket.machineRPC<ListProfilesResponse, {}>(
            machineId,
            'list-profiles',
            {}
        ),
        timeout
    ]);
}

/**
 * List model options available to the UI. This keeps `profile` as an
 * implementation detail while exposing model selection semantics to the app.
 */
function normalizeVendorModelOption(model: VendorModelInfoRpc): ModelOptionDisplay {
    const supportedReasoningEfforts = (model.supportedReasoningEfforts || []).filter(
        (effort): effort is import('./storageTypes').RuntimeEffort =>
            effort === 'default' ||
            effort === 'low' ||
            effort === 'medium' ||
            effort === 'high' ||
            effort === 'xhigh' ||
            effort === 'max'
    );

    return {
        id: model.id,
        name: model.displayName || model.model,
        isProxy: false,
        sourceType: 'vendor',
        vendor: model.vendor,
        supportsVision: model.supportsVision === true,
        supportsComputerUse: model.supportsComputerUse === true,
        apiFormat: model.apiFormat,
        description: model.description || undefined,
        isDefault: model.isDefault === true,
        defaultReasoningEffort: model.defaultReasoningEffort || null,
        supportedReasoningEfforts,
        chat: {
            api_key_masked: '',
            base_url: '',
            model: model.model,
            temperature: 0,
            max_tokens: 0,
        },
        compress: {
            api_key_masked: '',
            base_url: '',
            model: model.model,
            temperature: 0,
            max_tokens: 0,
        },
    };
}

export async function machineListModels(
    machineId: string,
    vendor?: VendorName | null,
): Promise<ListModelsResponse> {
    const normalizedVendor = vendor ?? 'cteno';
    const timeout = new Promise<never>((_, reject) =>
        setTimeout(() => reject(new Error('Model loading timed out')), 12000)
    );

    return Promise.race([
        (async () => {
            if (normalizedVendor !== 'cteno') {
                const cached = loadCachedVendorModelCatalog(machineId, normalizedVendor);
                const cacheIsFresh = cached && (Date.now() - cached.cachedAt) < 5 * 60 * 1000;
                if (cacheIsFresh) {
                    frontendLog(`[machineListModels.vendor.cache] ${JSON.stringify({
                        machineId,
                        vendor: normalizedVendor,
                        returnedModelCount: cached.models.length,
                        defaultModelId: cached.defaultModelId,
                        cachedAt: cached.cachedAt,
                    })}`);
                    return {
                        models: cached.models as ModelOptionDisplay[],
                        defaultModelId: cached.defaultModelId,
                    };
                }

                try {
                    const result = await apiSocket.machineRPC<{
                        success?: boolean;
                        vendor?: VendorName;
                        models?: VendorModelInfoRpc[];
                        defaultModelId?: string;
                        error?: string;
                    }, {
                        vendor: VendorName;
                    }>(
                        machineId,
                        'list-vendor-models',
                        { vendor: normalizedVendor }
                    );
                    if (result.success === false) {
                        throw new Error(result.error || `Failed to load ${normalizedVendor} models`);
                    }
                    const models = (result.models || []).map(normalizeVendorModelOption);
                    const defaultModelId =
                        (result.defaultModelId && models.some((model) => model.id === result.defaultModelId)
                            ? result.defaultModelId
                            : models.find((model) => model.isDefault)?.id) ||
                        models[0]?.id ||
                        cached?.defaultModelId ||
                        'default';

                    saveCachedVendorModelCatalog({
                        machineId,
                        vendor: normalizedVendor,
                        models,
                        defaultModelId,
                        cachedAt: Date.now(),
                    });

                    frontendLog(`[machineListModels.vendor] ${JSON.stringify({
                        machineId,
                        vendor: normalizedVendor,
                        returnedModelCount: models.length,
                        returnedModelIds: models.slice(0, 16).map((model) => ({
                            id: model.id,
                            model: model.chat?.model,
                            vendor: model.vendor,
                            sourceType: model.sourceType,
                            supportedReasoningEfforts: model.supportedReasoningEfforts || [],
                        })),
                        defaultModelId,
                    })}`);

                    return {
                        models,
                        defaultModelId,
                    };
                } catch (error) {
                    if (cached) {
                        frontendLog(`[machineListModels.vendor.cacheFallback] ${JSON.stringify({
                            machineId,
                            vendor: normalizedVendor,
                            cachedModelCount: cached.models.length,
                            defaultModelId: cached.defaultModelId,
                            error: error instanceof Error ? error.message : String(error),
                        })}`);
                        return {
                            models: cached.models as ModelOptionDisplay[],
                            defaultModelId: cached.defaultModelId,
                        };
                    }
                    throw error;
                };
            }

            const authToken = sync.getCredentials()?.token ?? TokenStorage.peekCredentials()?.token;
            const includeProxyModels = !!authToken?.trim() && isServerAvailable();
            const publicModelsPromise = includeProxyModels
                ? fetchPublicProxyModels().catch((error) => {
                    console.warn('Failed to fetch models from app server:', error);
                    return null;
                })
                : Promise.resolve(null);

            if (includeProxyModels) {
                await machineRefreshProxyProfiles(machineId).catch((error) => {
                    console.warn('Failed to refresh proxy models on machine:', error);
                });
            }

            const profileResult = await machineListProfiles(machineId);
            const publicModels = await publicModelsPromise;
            const proxyModelsToMerge = publicModels?.models || [];
            const mergedModels = proxyModelsToMerge.length
                ? mergeModelsWithServerProxyModels(profileResult.profiles || [], proxyModelsToMerge)
                : (profileResult.profiles || []);
            const authFilteredModels = filterProxyModelsForAuth(mergedModels, includeProxyModels);
            const models = authFilteredModels
                .map((model) => ({
                    ...model,
                    sourceType: (model.isProxy ? 'proxy' : 'byok') as 'proxy' | 'byok',
                    supportedReasoningEfforts: inferCtenoReasoningEfforts(model),
                }));
            const fallbackDefaultModelId = models[0]?.id
                || (includeProxyModels ? profileResult.defaultProfileId : undefined)
                || 'default';
            const defaultModelId = models.some((model) => model.id === profileResult.defaultProfileId)
                ? profileResult.defaultProfileId
                : fallbackDefaultModelId;

            frontendLog(`[machineListModels] ${JSON.stringify({
                machineId,
                vendor: normalizedVendor,
                includeProxyModels,
                publicModelCount: proxyModelsToMerge.length,
                publicModelIds: proxyModelsToMerge.slice(0, 12).map((model) => model.id),
                profileCount: (profileResult.profiles || []).length,
                profileIds: (profileResult.profiles || []).slice(0, 12).map((profile) => profile.id),
                mergedModelCount: mergedModels.length,
                mergedModelIds: mergedModels.slice(0, 12).map((model) => ({
                    id: model.id,
                    isProxy: model.isProxy === true,
                    model: model.chat?.model,
                })),
                returnedModelCount: models.length,
                returnedModelIds: models.slice(0, 16).map((model) => ({
                    id: model.id,
                    model: model.chat?.model,
                })),
                defaultModelId,
            })}`);

            return {
                models,
                defaultModelId,
            };
        })(),
        timeout,
    ]);
}

/**
 * Export full LLM profiles (with plaintext API keys) for cross-device migration.
 * RPC channel is end-to-end encrypted, so keys are never exposed to the server.
 */
export async function machineExportProfiles(machineId: string): Promise<ExportProfilesResponse> {
    const timeout = new Promise<never>((_, reject) =>
        setTimeout(() => reject(new Error('Profile export timed out')), 10000)
    );
    return Promise.race([
        apiSocket.machineRPC<ExportProfilesResponse, {}>(
            machineId,
            'export-profiles',
            {}
        ),
        timeout
    ]);
}

/**
 * Save (create or update) an LLM profile on the Machine
 */
export async function machineSaveProfile(machineId: string, profile: LlmProfileInput): Promise<{ success: boolean; id: string; error?: string }> {
    return await apiSocket.machineRPC<{ success: boolean; id: string; error?: string }, { profile: LlmProfileInput }>(
        machineId,
        'save-profile',
        { profile }
    );
}

/**
 * Save a Coding Plan model group atomically and set the recommended profile as default.
 */
export async function machineSaveCodingPlanProfiles(
    machineId: string,
    profiles: LlmProfileInput[],
    defaultProfileId: string
): Promise<{ success: boolean; count?: number; defaultProfileId?: string; error?: string }> {
    return await apiSocket.machineRPC<
        { success: boolean; count?: number; defaultProfileId?: string; error?: string },
        { profiles: LlmProfileInput[]; defaultProfileId: string }
    >(
        machineId,
        'save-coding-plan-profiles',
        { profiles, defaultProfileId }
    );
}

/**
 * Delete an LLM profile from the Machine
 */
export async function machineDeleteProfile(machineId: string, profileId: string): Promise<{ success: boolean; error?: string }> {
    return await apiSocket.machineRPC<{ success: boolean; error?: string }, { profileId: string }>(
        machineId,
        'delete-profile',
        { profileId }
    );
}

/**
 * Switch a session's LLM profile (takes effect on next message)
 */
export async function machineSwitchSessionModel(
    machineId: string,
    sessionId: string,
    modelId: string,
    reasoningEffort?: import('./storageTypes').RuntimeEffort,
): Promise<{ success: boolean; error?: string; outcome?: RuntimeControlOutcome; reason?: string | null }> {
    const result = await apiSocket.machineRPC<
        { success: boolean; error?: string; outcome?: RuntimeControlOutcome; reason?: string | null },
        { sessionId: string; modelId: string; reasoningEffort?: string }
    >(
        machineId,
        'switch-session-model',
        {
            sessionId,
            modelId,
            ...(reasoningEffort && reasoningEffort !== 'default'
                ? { reasoningEffort }
                : {}),
        }
    );
    // If session not connected, trigger reconnect with the desired profile
    if (!result.success && result.error?.includes('not found')) {
        console.log('switch-session-model: session not connected, reconnecting with model', modelId);
        await machineReconnectSession(machineId, sessionId, modelId);
        return { success: true, outcome: 'applied' };
    }
    return result;
}


// ========== Skill Management RPCs ==========

export interface SkillListItem {
    id: string;
    name: string;
    description: string;
    version: string;
    source: 'builtin' | 'community' | 'user' | 'installed';
    instructions?: string;
    path?: string;
    hasScripts?: boolean;
}

/**
 * List all skills installed on the machine (builtin + user)
 */
export async function machineListSkills(machineId: string): Promise<{ skills: SkillListItem[]; error?: string }> {
    try {
        return await apiSocket.machineRPC<{ skills: SkillListItem[] }, {}>(
            machineId,
            'list-skills',
            {}
        );
    } catch (error) {
        console.warn('list-skills RPC failed:', error);
        return {
            skills: [],
            error: error instanceof Error ? error.message : 'Unknown error',
        };
    }
}

// ========== Skill CRUD RPCs ==========

export interface CreateSkillInput {
    name: string;
    description: string;
    instructions?: string;
}

/**
 * Create a new user skill on the machine
 */
export async function machineCreateSkill(machineId: string, skill: CreateSkillInput): Promise<{ success: boolean; skillId?: string; error?: string }> {
    try {
        return await apiSocket.machineRPC<{ success: boolean; skillId?: string; error?: string }, CreateSkillInput>(
            machineId,
            'create-skill',
            skill
        );
    } catch (error) {
        console.warn('create-skill RPC failed:', error);
        return { success: false, error: error instanceof Error ? error.message : 'Unknown error' };
    }
}

/**
 * Delete a user skill from the machine
 */
export async function machineDeleteSkill(machineId: string, skillId: string): Promise<{ success: boolean; error?: string }> {
    try {
        return await apiSocket.machineRPC<{ success: boolean; error?: string }, { skillId: string }>(
            machineId,
            'delete-skill',
            { skillId }
        );
    } catch (error) {
        console.warn('delete-skill RPC failed:', error);
        return { success: false, error: error instanceof Error ? error.message : 'Unknown error' };
    }
}


// ========== SkillHub RPCs ==========

export interface SkillHubItem {
    slug: string;
    name: string;
    description: string;
    version: string;
    homepage: string;
    tags: string[];
    stats: { downloads: number; stars: number };
    installed: boolean;
}

export async function machineSkillhubFeatured(machineId: string): Promise<{ skills: SkillHubItem[]; error?: string }> {
    try {
        return await apiSocket.machineRPC<{ skills: SkillHubItem[] }, {}>(
            machineId,
            'skillhub-featured',
            {}
        );
    } catch (error) {
        console.warn('skillhub-featured RPC failed:', error);
        return { skills: [], error: error instanceof Error ? error.message : 'Unknown error' };
    }
}

export async function machineSkillhubSearch(machineId: string, query: string, limit = 30): Promise<{ skills: SkillHubItem[]; error?: string }> {
    try {
        return await apiSocket.machineRPC<{ skills: SkillHubItem[] }, { query: string; limit: number }>(
            machineId,
            'skillhub-search',
            { query, limit }
        );
    } catch (error) {
        console.warn('skillhub-search RPC failed:', error);
        return { skills: [], error: error instanceof Error ? error.message : 'Unknown error' };
    }
}

export async function machineSkillhubInstall(machineId: string, slug: string, displayName?: string): Promise<{ success: boolean; skillId?: string; error?: string }> {
    try {
        return await apiSocket.machineRPC<{ success: boolean; skillId?: string; error?: string }, { slug: string; displayName?: string }>(
            machineId,
            'skillhub-install',
            { slug, displayName }
        );
    } catch (error) {
        console.warn('skillhub-install RPC failed:', error);
        return { success: false, error: error instanceof Error ? error.message : 'Unknown error' };
    }
}


// ========== MCP Server Management RPCs ==========

export interface MCPServerItem {
    id: string;
    name: string;
    enabled: boolean;
    transport: 'stdio' | 'http_sse';
    /** Sanitized name prefix matching tool name format mcp__{toolNamePrefix}__{toolName} */
    toolNamePrefix: string;
    command?: string;
    args?: string[];
    url?: string;
    status: 'connected' | 'disconnected' | 'error';
    scope?: 'global' | 'project' | string;
    toolCount: number;
    error?: string;
}

export interface SessionMCPResponse {
    allServers: MCPServerItem[];
    activeServerIds: string[];
}

export interface AddMCPServerInput {
    name: string;
    transport: {
        type: 'stdio';
        command: string;
        args: string[];
        env: Record<string, string>;
    } | {
        type: 'http_sse';
        url: string;
        headers: Record<string, string>;
    };
}

/**
 * List all MCP servers on the machine
 */
export async function machineListMCPServers(machineId: string): Promise<{ servers: MCPServerItem[] }> {
    try {
        return await apiSocket.machineRPC<{ servers: MCPServerItem[] }, {}>(
            machineId,
            'list-mcp-servers',
            {}
        );
    } catch (error) {
        console.warn('list-mcp-servers RPC failed:', error);
        return { servers: [] };
    }
}

/**
 * Add a new MCP server to the machine
 */
export async function machineAddMCPServer(machineId: string, config: AddMCPServerInput): Promise<{ success: boolean; serverId?: string; error?: string }> {
    try {
        return await apiSocket.machineRPC<{ success: boolean; serverId?: string; error?: string }, AddMCPServerInput>(
            machineId,
            'add-mcp-server',
            config
        );
    } catch (error) {
        console.warn('add-mcp-server RPC failed:', error);
        return { success: false, error: error instanceof Error ? error.message : 'Unknown error' };
    }
}

/**
 * Remove an MCP server from the machine
 */
export async function machineRemoveMCPServer(machineId: string, serverId: string): Promise<{ success: boolean; error?: string }> {
    try {
        return await apiSocket.machineRPC<{ success: boolean; error?: string }, { serverId: string }>(
            machineId,
            'remove-mcp-server',
            { serverId }
        );
    } catch (error) {
        console.warn('remove-mcp-server RPC failed:', error);
        return { success: false, error: error instanceof Error ? error.message : 'Unknown error' };
    }
}

/**
 * Toggle MCP server enabled/disabled
 */
export async function machineToggleMCPServer(machineId: string, serverId: string, enabled: boolean): Promise<{ success: boolean }> {
    try {
        return await apiSocket.machineRPC<{ success: boolean }, { serverId: string; enabled: boolean }>(
            machineId,
            'toggle-mcp-server',
            { serverId, enabled }
        );
    } catch (error) {
        console.warn('toggle-mcp-server RPC failed:', error);
        return { success: false };
    }
}

/**
 * Get all MCP servers available for a session + which are currently active.
 * Prefer sessionRPC for the merged global+project server list, then fall back
 * to machineRPC when the session is offline. KV remains the source of truth for
 * explicit per-session active selection.
 */
export async function sessionGetMCPServers(sessionId: string, machineId: string): Promise<SessionMCPResponse> {
    try {
        let servers: MCPServerItem[] = [];

        try {
            const sessionResponse = await apiSocket.sessionRPC<SessionMCPResponse, Record<string, never>>(
                sessionId,
                'get-session-mcp-servers',
                {}
            );
            servers = Array.isArray(sessionResponse.allServers) ? sessionResponse.allServers : [];
        } catch (_) {
            // Offline or legacy sessions only expose machine-level global MCP.
        }

        if (servers.length === 0) {
            const machineResponse = await machineListMCPServers(machineId);
            servers = machineResponse.servers;
        }

        // Get session-specific active selection from KV store
        let activeServerIds: string[] | null = null;
        try {
            const kv = await kvGet(`session.${sessionId}.mcpServerIds`);
            if (kv) {
                activeServerIds = JSON.parse(kv.value);
            }
        } catch (e) {
            // No saved selection: default to enabled MCP servers from the
            // merged global+project config, so new sessions inherit project MCP.
        }

        return {
            allServers: servers,
            activeServerIds: activeServerIds ?? servers
                .filter((server) => server.enabled)
                .map((server) => server.toolNamePrefix),
        };
    } catch (error) {
        console.warn('sessionGetMCPServers failed:', error);
        return { allServers: [], activeServerIds: [] };
    }
}

/**
 * Set the active MCP servers for a session (empty array = NO MCP tools loaded).
 * Values should be toolNamePrefix strings (sanitized server names), NOT server IDs.
 * Persists to KV store on server. Also updates in-memory state via sessionRPC if session is online.
 */
export async function sessionSetMCPServers(sessionId: string, serverIds: string[]): Promise<{ success: boolean }> {
    try {
        // Persist to KV store (survives session offline/restart)
        // Try to get existing version for optimistic concurrency
        let version = -1;
        try {
            const existing = await kvGet(`session.${sessionId}.mcpServerIds`);
            if (existing) {
                version = existing.version;
            }
        } catch (_) { /* key doesn't exist yet */ }

        await kvSet(`session.${sessionId}.mcpServerIds`, JSON.stringify(serverIds), version);

        // Also try to update in-memory state if session is online
        try {
            await apiSocket.sessionRPC<{ success: boolean }, { serverIds: string[] }>(
                sessionId,
                'set-session-mcp-servers',
                { serverIds }
            );
        } catch (_) {
            // Session offline — that's fine, KV store is the source of truth
        }

        return { success: true };
    } catch (error) {
        console.warn('sessionSetMCPServers failed:', error);
        return { success: false };
    }
}

/**
 * Background Runs (Tools 后台任务) management types and functions
 */

// Run status
export type RunStatus = 'Running' | 'Exited' | 'Failed' | 'Killed' | 'TimedOut';

// Run exit info
export interface RunExit {
    exit_code: number;
}

// Run record returned from the API
export interface RunRecord {
    run_id: string;
    session_id: string;
    tool_id: string;  // e.g., "shell", "image_generation"
    command?: string;
    workdir?: string;
    status: RunStatus;
    started_at: number;  // Unix timestamp (seconds or milliseconds, depending on machine version)
    finished_at?: number;
    pid?: number;
    exit?: RunExit;
    error?: string;
    log_path?: string;
    notify: boolean;
    hard_timeout_secs?: number;
}

// Response from list runs API
interface ListRunsResponse {
    success: boolean;
    data: RunRecord[];
    error?: string;
}

// Response from get run API
interface GetRunResponse {
    success: boolean;
    data: RunRecord;
    error?: string;
}

// Response from stop run API
interface StopRunResponse {
    success: boolean;
    error?: string;
}

// Response from get logs API
interface GetLogsResponse {
    success: boolean;
    data: string;
    error?: string;
}

export interface BackgroundTaskRecord {
    taskId: string;
    sessionId: string;
    vendor: string;
    category: 'execution' | 'scheduled_job' | 'background_session';
    taskType: 'agent' | 'bash' | 'workflow' | 'remote_agent' | 'teammate' | 'scheduled_job' | 'background_session' | 'other';
    description?: string;
    summary?: string;
    status: 'running' | 'completed' | 'failed' | 'cancelled' | 'paused' | 'unknown';
    startedAt: number;
    completedAt?: number;
    toolUseId?: string;
    outputFile?: string;
    vendorExtra?: any;
}

interface ListBackgroundTasksResponse {
    success: boolean;
    data?: BackgroundTaskRecord[];
    error?: string;
}

interface GetBackgroundTaskResponse {
    success: boolean;
    data?: BackgroundTaskRecord;
    error?: string;
}

const BACKGROUND_TASK_TYPE_VALUES: BackgroundTaskRecord['taskType'][] = [
    'agent',
    'bash',
    'workflow',
    'remote_agent',
    'teammate',
    'scheduled_job',
    'background_session',
    'other',
];

const BACKGROUND_TASK_STATUS_VALUES: BackgroundTaskRecord['status'][] = [
    'running',
    'completed',
    'failed',
    'cancelled',
    'paused',
    'unknown',
];

function normalizeBackgroundTaskCategory(value: unknown): BackgroundTaskRecord['category'] {
    switch (value) {
        case 'execution':
        case 'executionTask':
            return 'execution';
        case 'scheduled':
        case 'scheduled_job':
        case 'scheduledJob':
            return 'scheduled_job';
        case 'background_session':
        case 'backgroundSession':
            return 'background_session';
        default:
            return 'execution';
    }
}

function normalizeBackgroundTaskRecord(value: any): BackgroundTaskRecord {
    const taskType = BACKGROUND_TASK_TYPE_VALUES.includes(value?.taskType)
        ? value.taskType
        : 'other';
    const status = BACKGROUND_TASK_STATUS_VALUES.includes(value?.status)
        ? value.status
        : 'unknown';

    return {
        taskId: typeof value?.taskId === 'string' ? value.taskId : '',
        sessionId: typeof value?.sessionId === 'string' ? value.sessionId : '',
        vendor: typeof value?.vendor === 'string' ? value.vendor : '',
        category: normalizeBackgroundTaskCategory(value?.category),
        taskType,
        description: typeof value?.description === 'string' ? value.description : undefined,
        summary: typeof value?.summary === 'string' ? value.summary : undefined,
        status,
        startedAt: typeof value?.startedAt === 'number' ? value.startedAt : 0,
        completedAt: typeof value?.completedAt === 'number' ? value.completedAt : undefined,
        toolUseId: typeof value?.toolUseId === 'string' ? value.toolUseId : undefined,
        outputFile: typeof value?.outputFile === 'string' ? value.outputFile : undefined,
        vendorExtra: value?.vendorExtra,
    };
}

/**
 * List all background runs (optionally filter by session_id)
 * Uses machineRPC to proxy through Happy Server to the desktop Machine.
 */
export async function machineListRuns(machineId: string, sessionId?: string): Promise<RunRecord[]> {
    try {
        const result = await apiSocket.machineRPC<ListRunsResponse, { sessionId?: string }>(
            machineId,
            'list-runs',
            { sessionId }
        );
        return result.data || [];
    } catch (error) {
        console.warn('machineListRuns RPC failed:', error);
        return [];
    }
}

export async function machineListBackgroundTasks(
    machineId: string,
    filter?: { sessionId?: string; category?: string; status?: string }
): Promise<BackgroundTaskRecord[]> {
    try {
        const result = await apiSocket.machineRPC<
            ListBackgroundTasksResponse,
            { sessionId?: string; category?: string; status?: string }
        >(
            machineId,
            'list-background-tasks',
            filter ?? {}
        );
        if (!result.success || !Array.isArray(result.data)) {
            return [];
        }
        return result.data.map(normalizeBackgroundTaskRecord);
    } catch (error) {
        console.warn('machineListBackgroundTasks RPC failed:', error);
        return [];
    }
}

export async function machineGetBackgroundTask(
    machineId: string,
    taskId: string
): Promise<BackgroundTaskRecord | null> {
    try {
        const result = await apiSocket.machineRPC<GetBackgroundTaskResponse, { taskId: string }>(
            machineId,
            'get-background-task',
            { taskId }
        );
        if (!result.success || !result.data) {
            return null;
        }
        return normalizeBackgroundTaskRecord(result.data);
    } catch (error) {
        console.warn('machineGetBackgroundTask RPC failed:', error);
        return null;
    }
}

/**
 * Get a single run by ID
 * Uses machineRPC to proxy through Happy Server to the desktop Machine.
 */
export async function machineGetRun(machineId: string, runId: string): Promise<RunRecord | null> {
    try {
        const result = await apiSocket.machineRPC<GetRunResponse, { runId: string }>(
            machineId,
            'get-run',
            { runId }
        );
        if (!result.success || !result.data) {
            return null;
        }
        return result.data;
    } catch (error) {
        console.warn('machineGetRun RPC failed:', error);
        return null;
    }
}

/**
 * Stop a running background task
 * Uses machineRPC to proxy through Happy Server to the desktop Machine.
 */
export async function machineStopRun(machineId: string, runId: string): Promise<{ success: boolean; error?: string }> {
    try {
        const result = await apiSocket.machineRPC<StopRunResponse, { runId: string }>(
            machineId,
            'stop-run',
            { runId }
        );
        return { success: result.success, error: result.error };
    } catch (error) {
        const message = error instanceof Error ? error.message : 'Unknown error';
        console.warn('machineStopRun RPC failed:', error);
        return { success: false, error: message };
    }
}

/**
 * Get logs for a background task
 * Uses machineRPC to proxy through Happy Server to the desktop Machine.
 */
export async function machineGetRunLogs(machineId: string, runId: string, lines: number = 100): Promise<string> {
    try {
        const result = await apiSocket.machineRPC<GetLogsResponse, { runId: string; lines: number }>(
            machineId,
            'get-run-logs',
            { runId, lines }
        );
        if (!result.success) {
            return `获取日志失败: ${result.error || 'Unknown error'}`;
        }
        return result.data || '';
    } catch (error) {
        console.warn('machineGetRunLogs RPC failed:', error);
        return `获取日志失败: ${error instanceof Error ? error.message : 'Unknown error'}`;
    }
}

// ========== SubAgent Management RPCs ==========

export type SubAgentStatus = 'pending' | 'running' | 'completed' | 'failed' | 'stopped' | 'timed_out';

export interface SubAgent {
    id: string;
    parent_session_id: string;
    agent_id: string;
    task: string;
    label?: string;
    status: SubAgentStatus;
    created_at: number;
    started_at?: number;
    completed_at?: number;
    result?: string;
    error?: string;
    iteration_count: number;
    cleanup: 'keep' | 'delete';
}

export interface ListSubAgentsOptions {
    status?: SubAgentStatus;
    parentSessionId?: string;
    activeOnly?: boolean;
}

export interface ListSubAgentsResponse {
    subagents: SubAgent[];
}

/**
 * List SubAgents on a machine with optional filters
 */
export async function machineListSubAgents(
    machineId: string,
    options?: ListSubAgentsOptions
): Promise<SubAgent[]> {
    try {
        const result = await apiSocket.machineRPC<ListSubAgentsResponse, ListSubAgentsOptions>(
            machineId,
            'list-subagents',
            options || {}
        );
        return result.subagents || [];
    } catch (error) {
        console.warn('list-subagents RPC failed:', error);
        return [];
    }
}

/**
 * Get a specific SubAgent by ID
 */
export async function machineGetSubAgent(
    machineId: string,
    subagentId: string
): Promise<SubAgent | null> {
    try {
        const result = await apiSocket.machineRPC<SubAgent, { id: string }>(
            machineId,
            'get-subagent',
            { id: subagentId }
        );
        return result;
    } catch (error) {
        console.warn('get-subagent RPC failed:', error);
        return null;
    }
}

/**
 * Stop a running SubAgent
 */
export async function machineStopSubAgent(
    machineId: string,
    subagentId: string
): Promise<{ success: boolean; error?: string }> {
    try {
        const result = await apiSocket.machineRPC<{ success: boolean; error?: string }, { id: string }>(
            machineId,
            'stop-subagent',
            { id: subagentId }
        );
        return result;
    } catch (error) {
        console.warn('stop-subagent RPC failed:', error);
        return {
            success: false,
            error: error instanceof Error ? error.message : 'Unknown error'
        };
    }
}

// ========== Scheduled Task Management RPCs ==========

export type ScheduleType =
    | { kind: 'at'; at: string }
    | { kind: 'every'; every_seconds: number; anchor?: string }
    | { kind: 'cron'; expr: string };

export type TaskRunStatus = 'success' | 'failed' | 'timed_out' | 'skipped';

export interface TaskState {
    next_run_at: number | null;
    running_since: number | null;
    last_run_at: number | null;
    last_status: TaskRunStatus | null;
    last_result_summary: string | null;
    consecutive_errors: number;
    total_runs: number;
}

export type TaskExecutionType = 'dispatch' | 'script';

export interface ScheduledTask {
    id: string;
    name: string;
    task_prompt: string;
    enabled: boolean;
    delete_after_run: boolean;
    schedule: ScheduleType;
    timezone: string;
    session_id: string;
    persona_id?: string;
    task_type?: TaskExecutionType;
    state: TaskState;
    created_at: number;
    updated_at: number;
}

/**
 * List all scheduled tasks on a machine
 */
export async function machineListScheduledTasks(
    machineId: string
): Promise<ScheduledTask[]> {
    try {
        const result = await apiSocket.machineRPC<{ tasks: ScheduledTask[] }, {}>(
            machineId,
            'list-scheduled-tasks',
            {}
        );
        return result.tasks || [];
    } catch (error) {
        console.warn('list-scheduled-tasks RPC failed:', error);
        return [];
    }
}

/**
 * Toggle a scheduled task enabled/disabled
 */
export async function machineToggleScheduledTask(
    machineId: string,
    taskId: string,
    enabled: boolean
): Promise<{ success: boolean; error?: string }> {
    try {
        return await apiSocket.machineRPC<{ success: boolean; error?: string }, { id: string; enabled: boolean }>(
            machineId,
            'toggle-scheduled-task',
            { id: taskId, enabled }
        );
    } catch (error) {
        console.warn('toggle-scheduled-task RPC failed:', error);
        return { success: false, error: error instanceof Error ? error.message : 'Unknown error' };
    }
}

/**
 * Delete a scheduled task
 */
export async function machineDeleteScheduledTask(
    machineId: string,
    taskId: string
): Promise<{ success: boolean; error?: string }> {
    try {
        return await apiSocket.machineRPC<{ success: boolean; error?: string }, { id: string }>(
            machineId,
            'delete-scheduled-task',
            { id: taskId }
        );
    } catch (error) {
        console.warn('delete-scheduled-task RPC failed:', error);
        return { success: false, error: error instanceof Error ? error.message : 'Unknown error' };
    }
}

/**
 * Update a scheduled task (partial update)
 */
export interface UpdateScheduledTaskInput {
    name?: string;
    task_prompt?: string;
    schedule?: ScheduleType;
    timezone?: string;
    enabled?: boolean;
    delete_after_run?: boolean;
}

export async function machineUpdateScheduledTask(
    machineId: string,
    taskId: string,
    updates: UpdateScheduledTaskInput
): Promise<{ success: boolean; task?: ScheduledTask; error?: string }> {
    try {
        return await apiSocket.machineRPC<
            { success: boolean; task?: ScheduledTask; error?: string },
            UpdateScheduledTaskInput & { id: string }
        >(
            machineId,
            'update-scheduled-task',
            { id: taskId, ...updates }
        );
    } catch (error) {
        console.warn('update-scheduled-task RPC failed:', error);
        return { success: false, error: error instanceof Error ? error.message : 'Unknown error' };
    }
}

/**
 * Delete all scheduled tasks belonging to a session
 */
export async function machineDeleteScheduledTasksBySession(
    machineId: string,
    sessionId: string
): Promise<{ success: boolean; deleted_count?: number; error?: string }> {
    try {
        return await apiSocket.machineRPC<
            { success: boolean; deleted_count?: number; error?: string },
            { sessionId: string }
        >(
            machineId,
            'delete-scheduled-tasks-by-session',
            { sessionId }
        );
    } catch (error) {
        console.warn('delete-scheduled-tasks-by-session RPC failed:', error);
        return { success: false, error: error instanceof Error ? error.message : 'Unknown error' };
    }
}

// ============================================================================
// Persona Operations
// ============================================================================

function normalizePersonaAgent(persona: Persona): Persona {
    return {
        ...persona,
        agent: persona.agent ?? 'cteno',
        modelId: persona.modelId ?? null,
    };
}

/**
 * List all personas
 */
export async function machineListPersonas(
    machineId: string
): Promise<Persona[]> {
    try {
        const result = await apiSocket.machineRPC<{ success?: boolean; personas?: Persona[]; error?: string }, {}>(
            machineId,
            'list-personas',
            {}
        );
        if (result.success === false) {
            throw new Error(result.error || 'list-personas returned success=false');
        }
        return (result.personas || []).map(normalizePersonaAgent);
    } catch (error) {
        console.warn('list-personas RPC failed:', error);
        throw error;
    }
}

/**
 * Create a new persona
 */
export async function machineCreatePersona(
    machineId: string,
    params: {
        name?: string;
        description?: string;
        model?: string;
        avatarId?: string;
        modelId?: string;
        workdir?: string;
        agent?: VendorName;
    }
): Promise<{ success: boolean; persona?: Persona; error?: string; pendingSessionId?: string; attemptId?: string; lifecycle?: 'creating' | 'ready' }> {
    try {
        const result = await apiSocket.machineRPC<
            { success: boolean; persona?: Persona; error?: string; pendingSessionId?: string; attemptId?: string; lifecycle?: 'creating' | 'ready' },
            typeof params
        >(
            machineId,
            'create-persona',
            params
        );
        return {
            ...result,
            persona: result.persona ? normalizePersonaAgent(result.persona) : result.persona,
        };
    } catch (error) {
        console.warn('create-persona RPC failed:', error);
        return { success: false, error: error instanceof Error ? error.message : 'Unknown error' };
    }
}

/**
 * Update an existing persona
 */
export async function machineUpdatePersona(
    machineId: string,
    params: {
        id: string;
        name?: string;
        description?: string;
        model?: string;
        avatarId?: string;
        modelId?: string;
        continuousBrowsing?: boolean;
    }
): Promise<{ success: boolean; persona?: Persona; error?: string }> {
    try {
        const result = await apiSocket.machineRPC<
            { success: boolean; persona?: Persona; error?: string },
            typeof params
        >(
            machineId,
            'update-persona',
            params
        );
        return {
            ...result,
            persona: result.persona ? normalizePersonaAgent(result.persona) : result.persona,
        };
    } catch (error) {
        console.warn('update-persona RPC failed:', error);
        return { success: false, error: error instanceof Error ? error.message : 'Unknown error' };
    }
}

/**
 * Delete a persona
 */
export async function machineDeletePersona(
    machineId: string,
    personaId: string
): Promise<{ success: boolean; error?: string }> {
    try {
        return await apiSocket.machineRPC<
            { success: boolean; error?: string },
            { id: string }
        >(
            machineId,
            'delete-persona',
            { id: personaId }
        );
    } catch (error) {
        console.warn('delete-persona RPC failed:', error);
        return { success: false, error: error instanceof Error ? error.message : 'Unknown error' };
    }
}

/**
 * Get task sessions for a persona
 */
export async function machineGetPersonaTasks(
    machineId: string,
    personaId: string
): Promise<PersonaTaskSummary[]> {
    try {
        const result = await apiSocket.machineRPC<
            { tasks: PersonaTaskSummary[] },
            { personaId: string }
        >(
            machineId,
            'get-persona-tasks',
            { personaId }
        );
        return result.tasks || [];
    } catch (error) {
        console.warn('get-persona-tasks RPC failed:', error);
        return [];
    }
}

export async function machineBootstrapWorkspace(
    machineId: string,
    params: {
        templateId: WorkspaceTemplateId;
        name?: string;
        workdir?: string;
        model?: string;
        id?: string;
        roleVendorOverrides?: WorkspaceRoleVendorOverrides;
    }
): Promise<{
    success: boolean;
    workspace?: {
        id: string;
        name: string;
        templateId: string;
        personaId: string;
        sessionId: string;
        roles: Array<{
            roleId: string;
            agentId: string;
            sessionId: string;
        }>;
    };
    events?: any[];
    error?: string;
}> {
    try {
        const backendTemplateId = ({
            'group-chat': 'coding-studio',
            'gated-tasks': 'task-gate-coding-manual',
            autoresearch: 'autoresearch',
        } as const)[params.templateId];
        return await apiSocket.machineRPC(machineId, 'bootstrap-workspace', {
            ...params,
            templateId: backendTemplateId,
        });
    } catch (error) {
        console.warn('bootstrap-workspace RPC failed:', error);
        return { success: false, error: error instanceof Error ? error.message : 'Unknown error' };
    }
}

export async function machineListAgentWorkspaces(
    machineId: string
): Promise<WorkspaceSummary[]> {
    try {
        const result = await apiSocket.machineRPC<{ success?: boolean; workspaces?: WorkspaceSummary[]; error?: string }, {}>(
            machineId,
            'list-agent-workspaces',
            {}
        );
        if (result.success === false) {
            throw new Error(result.error || 'list-agent-workspaces returned success=false');
        }
        return result.workspaces || [];
    } catch (error) {
        console.warn('list-agent-workspaces RPC failed:', error);
        return [];
    }
}

export async function machineGetAgentWorkspace(
    machineId: string,
    personaId: string
): Promise<WorkspaceSummary | null> {
    try {
        const result = await apiSocket.machineRPC<{ success: boolean; workspace?: WorkspaceSummary }, { personaId: string }>(
            machineId,
            'get-agent-workspace',
            { personaId }
        );
        return result.workspace || null;
    } catch (error) {
        console.warn('get-agent-workspace RPC failed:', error);
        return null;
    }
}

export async function machineWorkspaceSendMessage(
    machineId: string,
    params: {
        personaId: string;
        message: string;
        roleId?: string;
    }
): Promise<{
    success: boolean;
    plan?: WorkspaceTurnPlan;
    workflowVoteWindow?: WorkspaceWorkflowVoteWindow | null;
    workflowVoteResponses?: WorkspaceWorkflowVoteResponse[];
    dispatches?: WorkspaceDispatch[];
    sessionId?: string;
    personaId?: string;
    roleId?: string | null;
    dispatch?: WorkspaceDispatch | null;
    events?: WorkspaceEvent[];
    state?: WorkspaceRuntimeState;
    error?: string;
}> {
    try {
        return await apiSocket.machineRPC(machineId, 'workspace-send-message', params);
    } catch (error) {
        console.warn('workspace-send-message RPC failed:', error);
        return { success: false, error: error instanceof Error ? error.message : 'Unknown error' };
    }
}

export async function machineDeleteAgentWorkspace(
    machineId: string,
    personaId: string
): Promise<{ success: boolean; error?: string }> {
    try {
        return await apiSocket.machineRPC(machineId, 'delete-agent-workspace', { personaId });
    } catch (error) {
        console.warn('delete-agent-workspace RPC failed:', error);
        return {
            success: false,
            error: error instanceof Error ? error.message : 'Unknown error',
        };
    }
}

/**
 * Reset a persona's chat session — archive old session and create a fresh one
 */
export async function machineResetPersonaSession(
    machineId: string,
    personaId: string
): Promise<{ success: boolean; newSessionId?: string; oldSessionId?: string; error?: string }> {
    try {
        return await apiSocket.machineRPC<
            { success: boolean; newSessionId?: string; oldSessionId?: string; error?: string },
            { personaId: string }
        >(
            machineId,
            'reset-persona-session',
            { personaId }
        );
    } catch (error) {
        console.warn('reset-persona-session RPC failed:', error);
        return { success: false, error: error instanceof Error ? error.message : 'Unknown error' };
    }
}

// ============================================================================
// Orchestration Flow Operations
// ============================================================================

import type { OrchestrationFlow } from './storageTypes';

/**
 * Get orchestration flow for a persona
 */
export async function machineGetOrchestrationFlow(
    machineId: string,
    personaId: string
): Promise<OrchestrationFlow | null> {
    try {
        const result = await apiSocket.machineRPC<
            { success: boolean; flow: OrchestrationFlow | null; error?: string },
            { personaId: string }
        >(
            machineId,
            'get-orchestration-flow',
            { personaId }
        );
        return result.flow || null;
    } catch (error) {
        console.warn('get-orchestration-flow RPC failed:', error);
        return null;
    }
}

/**
 * Create an orchestration flow
 */
export async function machineCreateOrchestrationFlow(
    machineId: string,
    params: {
        personaId: string;
        sessionId?: string;
        title: string;
        nodes: Array<{ id: string; label: string; agentType?: string; maxIterations?: number }>;
        edges: Array<{ from: string; to: string; condition?: string; edgeType?: string }>;
    }
): Promise<{ success: boolean; flowId?: string; error?: string }> {
    try {
        return await apiSocket.machineRPC<
            { success: boolean; flowId?: string; error?: string },
            typeof params
        >(
            machineId,
            'create-orchestration-flow',
            params
        );
    } catch (error) {
        console.warn('create-orchestration-flow RPC failed:', error);
        return { success: false, error: error instanceof Error ? error.message : 'Unknown error' };
    }
}

/**
 * Delete an orchestration flow
 */
export async function machineDeleteOrchestrationFlow(
    machineId: string,
    flowId: string
): Promise<{ success: boolean; error?: string }> {
    try {
        return await apiSocket.machineRPC<
            { success: boolean; deleted?: boolean; error?: string },
            { flowId: string }
        >(
            machineId,
            'delete-orchestration-flow',
            { flowId }
        );
    } catch (error) {
        console.warn('delete-orchestration-flow RPC failed:', error);
        return { success: false, error: error instanceof Error ? error.message : 'Unknown error' };
    }
}

// ── Memory management RPC ──────────────────────────────────────────

interface MemoryListResponse {
    success: boolean;
    data?: string[];
    error?: string;
}

interface MemoryReadResponse {
    success: boolean;
    data?: string | null;
    error?: string;
}

interface MemoryWriteResponse {
    success: boolean;
    data?: string;
    error?: string;
}

export interface WorkspaceEntry {
    name: string;
    path: string;
    type: 'file' | 'directory' | 'symlink' | 'other';
    size?: number | null;
    modifiedAt?: number | null;
}

export interface WorkspaceListResult {
    path: string;
    entries: WorkspaceEntry[];
    hasMore: boolean;
    total: number;
}

export interface WorkspaceStatItem {
    path: string;
    exists: boolean;
    type?: 'file' | 'directory' | 'symlink' | 'other';
    size?: number | null;
    modifiedAt?: number | null;
    error?: string;
}

interface WorkspaceListResponse {
    success: boolean;
    path?: string;
    entries?: WorkspaceEntry[];
    hasMore?: boolean;
    total?: number;
    error?: string;
}

export interface WorkspaceReadResponse {
    success: boolean;
    path?: string;
    encoding?: 'utf8' | 'base64';
    data?: string;
    bytesRead?: number;
    offset?: number;
    nextOffset?: number;
    size?: number;
    eof?: boolean;
    modifiedAt?: number;
    error?: string;
}

interface WorkspaceStatResponse {
    success: boolean;
    items?: WorkspaceStatItem[];
    error?: string;
}

export async function machineMemoryListFiles(machineId: string, ownerId?: string): Promise<string[]> {
    try {
        const params: Record<string, string> = {};
        if (ownerId) params.persona_id = ownerId; // RPC param name unchanged for backward compat
        const result = await apiSocket.machineRPC<MemoryListResponse, Record<string, string>>(
            machineId,
            'memory-list-files',
            params,
        );
        return result.data ?? [];
    } catch (e) {
        console.error('[machineMemoryListFiles] error:', e);
        return [];
    }
}

export async function machineMemoryRead(machineId: string, filePath: string, ownerId?: string, scope?: 'private' | 'global'): Promise<string | null> {
    try {
        const params: Record<string, string> = { file_path: filePath };
        if (ownerId) params.persona_id = ownerId; // RPC param name unchanged for backward compat
        if (scope) params.scope = scope;
        const result = await apiSocket.machineRPC<MemoryReadResponse, Record<string, string>>(
            machineId,
            'memory-read',
            params,
        );
        return result.data ?? null;
    } catch (e) {
        console.error('[machineMemoryRead] error:', e);
        return null;
    }
}

export async function machineMemoryWrite(machineId: string, filePath: string, content: string, ownerId?: string, scope?: 'private' | 'global'): Promise<boolean> {
    try {
        const params: Record<string, string> = { file_path: filePath, content };
        if (ownerId) params.persona_id = ownerId; // RPC param name unchanged for backward compat
        if (scope) params.scope = scope;
        const result = await apiSocket.machineRPC<MemoryWriteResponse, Record<string, string>>(
            machineId,
            'memory-write',
            params,
        );
        return result.success;
    } catch (e) {
        console.error('[machineMemoryWrite] error:', e);
        return false;
    }
}

export async function machineMemoryDelete(machineId: string, filePath: string, ownerId?: string, scope?: 'private' | 'global'): Promise<boolean> {
    try {
        const params: Record<string, string> = { file_path: filePath };
        if (ownerId) params.persona_id = ownerId; // RPC param name unchanged for backward compat
        if (scope) params.scope = scope;
        const result = await apiSocket.machineRPC<MemoryWriteResponse, Record<string, string>>(
            machineId,
            'memory-delete',
            params,
        );
        return result.success;
    } catch (e) {
        console.error('[machineMemoryDelete] error:', e);
        return false;
    }
}

export async function machineWorkspaceList(
    machineId: string,
    path: string = '.',
    options?: { includeHidden?: boolean; limit?: number; workspaceRoot?: string }
): Promise<WorkspaceListResult | null> {
    try {
        const result = await apiSocket.machineRPC<WorkspaceListResponse, {
            path: string;
            include_hidden?: boolean;
            limit?: number;
            workspace_root?: string;
        }>(
            machineId,
            'workspace-list',
            {
                path,
                include_hidden: options?.includeHidden,
                limit: options?.limit,
                workspace_root: options?.workspaceRoot,
            },
        );
        if (!result.success) {
            return null;
        }
        return {
            path: result.path ?? path,
            entries: result.entries ?? [],
            hasMore: result.hasMore ?? false,
            total: result.total ?? (result.entries?.length ?? 0),
        };
    } catch (e) {
        console.error('[machineWorkspaceList] error:', e);
        return null;
    }
}

export async function machineWorkspaceRead(
    machineId: string,
    path: string,
    options?: { offset?: number; length?: number; encoding?: 'utf8' | 'base64'; workspaceRoot?: string }
): Promise<WorkspaceReadResponse | null> {
    try {
        const result = await apiSocket.machineRPC<WorkspaceReadResponse, {
            path: string;
            offset?: number;
            length?: number;
            encoding?: 'utf8' | 'base64';
            workspace_root?: string;
        }>(
            machineId,
            'workspace-read',
            {
                path,
                offset: options?.offset,
                length: options?.length,
                encoding: options?.encoding,
                workspace_root: options?.workspaceRoot,
            },
        );
        return result.success ? result : null;
    } catch (e) {
        console.error('[machineWorkspaceRead] error:', e);
        return null;
    }
}

export async function machineWorkspaceStat(
    machineId: string,
    paths: string[],
    options?: { workspaceRoot?: string }
): Promise<WorkspaceStatItem[]> {
    try {
        const result = await apiSocket.machineRPC<WorkspaceStatResponse, { paths: string[]; workspace_root?: string }>(
            machineId,
            'workspace-stat',
            {
                paths,
                workspace_root: options?.workspaceRoot,
            },
        );
        if (!result.success) {
            return [];
        }
        return result.items ?? [];
    } catch (e) {
        console.error('[machineWorkspaceStat] error:', e);
        return [];
    }
}

// ===== Local Usage =====

export interface LocalModelUsage {
    model: string;
    input: number;
    output: number;
}

export interface LocalProfileUsage {
    profileId: string;
    profileName: string;
    totalTokens: number;
    models: LocalModelUsage[];
}

export interface LocalDayUsage {
    date: string;
    input: number;
    output: number;
}

export interface LocalUsageSummary {
    totalInput: number;
    totalOutput: number;
    totalCacheRead: number;
    totalCacheCreation: number;
    byProfile: LocalProfileUsage[];
    byDay: LocalDayUsage[];
}

/**
 * Get local usage statistics from the machine's SQLite database.
 */
export async function machineGetLocalUsage(
    machineId: string,
    period: 'today' | '7days' | '30days'
): Promise<LocalUsageSummary | null> {
    try {
        const result = await apiSocket.machineRPC<
            { success: boolean; summary?: LocalUsageSummary; error?: string },
            { period: string }
        >(machineId, 'get-local-usage', { period });

        if (result.success && result.summary) {
            return result.summary;
        }
        console.warn('[machineGetLocalUsage] failed:', result.error);
        return null;
    } catch (e) {
        console.error('[machineGetLocalUsage] error:', e);
        return null;
    }
}

// ── Notification Watcher RPC ──────────────────────────────────────────

export interface NotificationApp {
    appId: number;
    identifier: string;
    displayName: string;
}

export interface NotificationSubscription {
    id: string;
    personaId: string;
    appIdentifier: string;
    appDisplayName: string;
    enabled: boolean;
    createdAt: number;
}

/**
 * List all apps from macOS notification center database
 */
export async function machineListNotificationApps(
    machineId: string
): Promise<NotificationApp[]> {
    try {
        const result = await apiSocket.machineRPC<
            { success: boolean; apps?: NotificationApp[]; error?: string },
            {}
        >(machineId, 'list-notification-apps', {});
        return result.apps || [];
    } catch (error) {
        console.warn('list-notification-apps RPC failed:', error);
        return [];
    }
}

/**
 * Get notification subscriptions for a persona
 */
export async function machineGetNotificationSubscriptions(
    machineId: string,
    personaId: string
): Promise<NotificationSubscription[]> {
    try {
        const result = await apiSocket.machineRPC<
            { success: boolean; subscriptions?: NotificationSubscription[]; error?: string },
            { personaId: string }
        >(machineId, 'get-notification-subscriptions', { personaId });
        return result.subscriptions || [];
    } catch (error) {
        console.warn('get-notification-subscriptions RPC failed:', error);
        return [];
    }
}

/**
 * Update notification subscription (add/remove/toggle)
 */
export async function machineUpdateNotificationSubscription(
    machineId: string,
    params: {
        action: 'add' | 'remove' | 'toggle';
        personaId: string;
        appIdentifier?: string;
        appDisplayName?: string;
        subscriptionId?: string;
        enabled?: boolean;
    }
): Promise<{ success: boolean; error?: string }> {
    try {
        return await apiSocket.machineRPC<
            { success: boolean; error?: string },
            typeof params
        >(machineId, 'update-notification-subscription', params);
    } catch (error) {
        console.warn('update-notification-subscription RPC failed:', error);
        return { success: false, error: error instanceof Error ? error.message : 'Unknown error' };
    }
}

// ============================================================================
// Agent RPCs
// ============================================================================

import type { TargetPage, GoalTreePage, AgentNotification } from './storageTypes';

export async function machineGetAgentLatestText(
    machineId: string, agentId: string
): Promise<string | null> {
    try {
        const result = await apiSocket.machineRPC<{ success: boolean; text?: string | null }, { agentId: string }>(
            machineId, 'get-agent-latest-text', { agentId }
        );
        return result.success ? (result.text ?? null) : null;
    } catch { return null; }
}

export async function machineGetDashboard(
    machineId: string, agentId: string
): Promise<TargetPage | null> {
    try {
        const result = await apiSocket.machineRPC<{ success: boolean; page?: TargetPage; error?: string }, { agentId: string }>(
            machineId, 'get-dashboard', { agentId }
        );
        return result.success ? (result.page ?? null) : null;
    } catch { return null; }
}

export async function machineGetA2uiState(
    machineId: string, agentId: string
): Promise<any[] | null> {
    try {
        const result = await apiSocket.machineRPC<{ success: boolean; surfaces?: any[] }, { agentId: string }>(
            machineId, 'get-a2ui-state', { agentId }
        );
        return result?.success ? (result.surfaces ?? []) : null;
    } catch { return null; }
}

export async function machineA2uiAction(
    machineId: string, agentId: string, surfaceId: string, componentId: string, event: any
): Promise<void> {
    try {
        await apiSocket.machineRPC<{ success: boolean }, any>(
            machineId, 'a2ui-action', { agentId, surfaceId, componentId, event }
        );
    } catch { /* fire-and-forget */ }
}

export async function machineGetTargetPage(
    machineId: string, agentId: string
): Promise<TargetPage | null> {
    try {
        const result = await apiSocket.machineRPC<{ success: boolean; page?: TargetPage; error?: string }, { agentId: string }>(
            machineId, 'get-target-page', { agentId }
        );
        return result.success ? (result.page ?? null) : null;
    } catch {
        return null;
    }
}

export async function machineTargetPageResponse(
    machineId: string, agentId: string, response: any
): Promise<{ success: boolean; error?: string }> {
    try {
        return await apiSocket.machineRPC(machineId, 'target-page-response', { agentId, response });
    } catch (e: any) {
        return { success: false, error: e.message };
    }
}

export async function machineGetGoalTreePage(
    machineId: string, agentId: string
): Promise<GoalTreePage | null> {
    try {
        const result = await apiSocket.machineRPC<{ success: boolean; page?: GoalTreePage; error?: string }, { agentId: string }>(
            machineId, 'get-goal-tree-page', { agentId }
        );
        return result.success ? (result.page ?? null) : null;
    } catch {
        return null;
    }
}

export async function machineListNotifications(
    machineId: string, agentId?: string
): Promise<AgentNotification[]> {
    try {
        const result = await apiSocket.machineRPC<{ success: boolean; notifications?: AgentNotification[] }, { agentId?: string; limit?: number }>(
            machineId, 'list-notifications', { agentId, limit: 50 }
        );
        return result.success ? (result.notifications ?? []) : [];
    } catch {
        return [];
    }
}

export async function machineMarkNotificationRead(
    machineId: string, id: string
): Promise<{ success: boolean; error?: string }> {
    try {
        return await apiSocket.machineRPC(machineId, 'mark-notification-read', { id });
    } catch (e: any) {
        return { success: false, error: e.message };
    }
}

// ============================================================================
// Agent Config Operations
// ============================================================================

import type { AgentConfig } from './storageTypes';

/**
 * List all agents for a machine
 */
export async function machineListAgents(
    machineId: string,
    workdir?: string
): Promise<AgentConfig[]> {
    try {
        const result = await apiSocket.machineRPC<
            { success: boolean; agents: AgentConfig[] },
            { workdir?: string }
        >(
            machineId,
            'list-agents',
            { workdir }
        );
        return result.agents || [];
    } catch (error) {
        console.warn('list-agents RPC failed:', error);
        return [];
    }
}

/**
 * Get a single agent by ID
 */
export async function machineGetAgent(
    machineId: string,
    agentId: string,
    workdir?: string
): Promise<AgentConfig | null> {
    try {
        const result = await apiSocket.machineRPC<
            { success: boolean; agent: AgentConfig },
            { id: string; workdir?: string }
        >(
            machineId,
            'get-agent',
            { id: agentId, workdir }
        );
        return result.agent || null;
    } catch (error) {
        console.warn('get-agent RPC failed:', error);
        return null;
    }
}

/**
 * Create a new agent
 */
export async function machineCreateAgent(
    machineId: string,
    params: {
        id: string;
        name: string;
        description?: string;
        instructions?: string;
        model?: string;
        allowed_tools?: string[];
        excluded_tools?: string[];
        scope?: 'global' | 'workspace';
        workdir?: string;
    }
): Promise<{ success: boolean; id?: string; path?: string; error?: string }> {
    try {
        return await apiSocket.machineRPC<
            { success: boolean; id?: string; path?: string; error?: string },
            typeof params
        >(
            machineId,
            'create-agent',
            params
        );
    } catch (error) {
        console.warn('create-agent RPC failed:', error);
        return { success: false, error: error instanceof Error ? error.message : 'Unknown error' };
    }
}

/**
 * Delete an agent
 */
export async function machineDeleteAgent(
    machineId: string,
    agentId: string,
    workdir?: string
): Promise<{ success: boolean; error?: string }> {
    try {
        return await apiSocket.machineRPC<
            { success: boolean; error?: string },
            { id: string; workdir?: string }
        >(
            machineId,
            'delete-agent',
            { id: agentId, workdir }
        );
    } catch (error) {
        console.warn('delete-agent RPC failed:', error);
        return { success: false, error: error instanceof Error ? error.message : 'Unknown error' };
    }
}

// Export types for external use
export type {
    SessionBashRequest,
    SessionBashResponse,
    SessionReadFileResponse,
    SessionWriteFileResponse,
    SessionListDirectoryResponse,
    DirectoryEntry,
    SessionGetDirectoryTreeResponse,
    TreeNode,
    SessionRipgrepResponse,
    SessionKillResponse
};
