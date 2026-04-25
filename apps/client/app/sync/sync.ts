import Constants from 'expo-constants';
import { apiSocket, getLocalHostInfo } from '@/sync/apiSocket';
import type { LocalHostInfo } from '@/sync/apiSocket';
import { AuthCredentials } from '@/auth/tokenStorage';
import { Encryption } from '@/sync/encryption/encryption';
import { encodeBase64 } from '@/encryption/base64';
import { storage } from './storage';
import { ApiEphemeralUpdateSchema, ApiMessage, ApiUpdateContainerSchema } from './apiTypes';
import type { ApiEphemeralActivityUpdate } from './apiTypes';
import { Session, Machine, Persona } from './storageTypes';
import { InvalidateSync } from '@/utils/sync';
import { ActivityUpdateAccumulator } from './reducer/activityUpdateAccumulator';
import { randomUUID } from 'expo-crypto';
import * as Notifications from 'expo-notifications';
import { registerPushToken } from './apiPush';
import { Platform, AppState } from 'react-native';
import { isRunningOnMac } from '@/utils/platform';
import { NormalizedMessage, normalizeRawMessage, RawRecord, RawRecordSchema } from './typesRaw';
import { applySettings, Settings, settingsDefaults, settingsParse, SUPPORTED_SCHEMA_VERSION } from './settings';
import { Profile, profileParse } from './profile';
import {
    nowForTypewriter,
    computeTypewriterChunkSize,
    TYPEWRITER_FRAME_MS,
} from './typewriter';
import { loadPendingSettings, savePendingSettings } from './persistence';
import { initializeTracking, tracking } from '@/track';
import { parseToken } from '@/utils/parseToken';
import { getServerUrl, isServerAvailable } from './serverConfig';
import { log } from '@/log';
import { gitStatusSync } from './gitStatusSync';
import { projectManager } from './projectManager';
import { frontendLog, isTauri as isTauriEnv } from '@/utils/tauri';
import { canUseCloudServerAccess } from '@/config/capabilities';

import { machineListSessions, machineReconnectSession, machineRefreshProxyProfiles } from './ops';
import { fetchVendorQuota } from './apiVendorQuota';
import { EncryptionCache } from './encryption/encryptionCache';
import { systemPrompt } from './prompt/systemPrompt';
import { getFriendsList, getUserProfile } from './apiFriends';
import { fetchFeed } from './apiFeed';
import { FeedItem } from './feedTypes';
import { UserProfile } from './friendTypes';
import { initializeTodoSync } from '../-zen/model/ops';
import type { SessionTaskLifecycleEntry } from './storageTypes';
import { loadCache as loadMessageCache, saveCache as saveMessageCache, type CachedSessionMessage, type SessionMessagesCacheEntry } from './messageCache';

const ENABLE_SYNC_UPDATE_DEBUG = false;
const LOCAL_SESSION_MESSAGES_PAGE_SIZE = 50;

/**
 * Best-effort JSON parse used for 2.0 plaintext payloads coming off the wire.
 * Returns `null` on parse failure rather than throwing so caller can fall
 * back to sensible defaults.
 */
function safeParseJson<T = unknown>(raw: string | null | undefined): T | null {
    if (raw == null) return null;
    try {
        return JSON.parse(raw) as T;
    } catch {
        return null;
    }
}

type LocalSessionMessage = CachedSessionMessage;

type LocalSessionMessagesPage = {
    messages: LocalSessionMessage[];
    hasMore: boolean;
};

type RelaySessionMessagesResponse = {
    requestId?: string;
    sessionId?: string;
    messages?: LocalSessionMessage[];
    hasMore?: boolean;
    offset?: number;
    error?: string;
};

type PendingRelaySessionMessagesRequest = {
    sessionId: string;
    timeout: ReturnType<typeof setTimeout>;
    resolve: (response: RelaySessionMessagesResponse) => void;
    reject: (error: Error) => void;
};

type PendingPersonaBinding = {
    personaId: string;
    pendingSessionId: string;
    machineId: string;
    attemptId?: string;
    vendor?: string;
    createdAt: number;
};

type QueuedPendingPersonaMessage = {
    localId: string;
    text: string;
    displayText?: string;
    images?: Array<{ media_type: string; data: string }>;
    createdAt: number;
};

type PersonaSessionReadyHostEvent = {
    type: 'persona-session-ready';
    personaId: string;
    pendingSessionId?: string;
    attemptId?: string;
    vendor?: string;
    machineId?: string;
    sessionId: string;
    session?: Omit<Session, 'presence'> & { presence?: 'online' | number };
};

type PersonaSessionFailedHostEvent = {
    type: 'persona-session-failed';
    personaId: string;
    pendingSessionId?: string;
    attemptId?: string;
    vendor?: string;
    machineId?: string;
    error?: string;
};

class RelaySessionMessagesError extends Error {
    code: string;

    constructor(code: string) {
        super(code);
        this.name = 'RelaySessionMessagesError';
        this.code = code;
    }
}

function toSessionMessagesErrorCode(error: unknown): string {
    if (error instanceof RelaySessionMessagesError) {
        return error.code;
    }
    if (error instanceof Error && error.message.includes('timed out')) {
        return 'request_timeout';
    }
    return 'load_failed';
}

function parseLocalShellRawRecord(text: string): RawRecord | null {
    const trimmed = text.trim();
    if (!trimmed.startsWith('{')) {
        return null;
    }

    const parsed = safeParseJson(trimmed);
    if (!parsed) {
        return null;
    }

    const rawRecord = RawRecordSchema.safeParse(parsed);
    return rawRecord.success ? (rawRecord.data as RawRecord) : null;
}

function normalizeLocalShellMessage(message: LocalSessionMessage): NormalizedMessage | null {
    if (message.role === 'assistant') {
        const rawRecord = parseLocalShellRawRecord(message.text);
        if (rawRecord) {
            return normalizeRawMessage(message.id, message.localId, message.createdAt, rawRecord);
        }
    }

    if (message.text.startsWith('BLOCKS:')) {
        try {
            const rawBlocks = JSON.parse(message.text.slice('BLOCKS:'.length)) as any[];
            const content: any[] = [];
            rawBlocks.forEach((block, index) => {
                const uuid = `${message.id}:block:${index}`;
                if (!block || typeof block !== 'object' || typeof block.type !== 'string') {
                    return;
                }
                if (block.type === 'text' && typeof block.text === 'string') {
                    content.push({
                        type: 'text' as const,
                        text: block.text,
                        uuid,
                        parentUUID: null,
                    });
                    return;
                }
                if (block.type === 'thinking' && typeof block.thinking === 'string') {
                    content.push({
                        type: 'thinking' as const,
                        thinking: block.thinking,
                        uuid,
                        parentUUID: null,
                    });
                    return;
                }
                if (block.type === 'tool_use' && typeof block.id === 'string' && typeof block.name === 'string') {
                    content.push({
                        type: 'tool-call' as const,
                        id: block.id,
                        name: block.name,
                        input: block.input,
                        description:
                            block.input && typeof block.input === 'object' && typeof block.input.description === 'string'
                                ? block.input.description
                                : null,
                        uuid,
                        parentUUID: null,
                    });
                    return;
                }
                if (block.type === 'tool_result' && typeof block.tool_use_id === 'string') {
                    content.push({
                        type: 'tool-result' as const,
                        tool_use_id: block.tool_use_id,
                        content: block.content ?? '',
                        is_error: !!block.is_error,
                        permissions: block.permissions,
                        uuid,
                        parentUUID: null,
                    });
                }
            });

            if (content.length > 0) {
                return {
                    id: message.id,
                    localId: message.localId,
                    createdAt: message.createdAt,
                    role: 'agent',
                    content,
                    isSidechain: false,
                };
            }
        } catch (error) {
            console.warn('Failed to parse local shell BLOCKS payload:', error);
        }
    }

    if (message.role === 'user') {
        return {
            id: message.id,
            localId: message.localId,
            createdAt: message.createdAt,
            role: 'user',
            content: {
                type: 'text',
                text: message.text,
            },
            isSidechain: false,
        };
    }

    return {
        id: message.id,
        localId: message.localId,
        createdAt: message.createdAt,
        role: 'agent',
        content: [{
            type: 'text',
            text: message.text,
            uuid: `${message.id}:text`,
            parentUUID: null,
        }],
        isSidechain: false,
    };
}

function normalizePersistedSessionMessages(
    sessionId: string,
    messages: LocalSessionMessage[],
    applySessions: (sessions: (Omit<Session, 'presence'> & { presence?: 'online' | number })[]) => void,
): NormalizedMessage[] {
    // `messages` arrives newest-first (the Rust loader applies `.rev()` so the
    // UI can render a reverse-scrolling chat). Lifecycle side effects (thinking,
    // promptSuggestions, tokenCount…) must be applied chronologically oldest→newest
    // so the final session state reflects the latest event rather than the
    // oldest one winning via iteration order.
    for (let i = messages.length - 1; i >= 0; i--) {
        const message = messages[i];
        if (message.role !== 'assistant') continue;
        const rawRecord = parseLocalShellRawRecord(message.text);
        if (rawRecord) {
            applyPersistedSessionEffects(
                sessionId,
                rawRecord,
                message.createdAt,
                applySessions,
            );
        }
    }

    return messages.reduce<NormalizedMessage[]>((acc, message) => {
        const normalized = normalizeLocalShellMessage(message);
        if (normalized) {
            acc.push(normalized);
        }
        return acc;
    }, []);
}

function pageMaxSeq(offset: number, count: number, fallback = 0): number {
    return Math.max(fallback, offset + count);
}


// Global event bus for agent push events (A2UI updates, persona session ready, etc.).
// Hooks subscribe to this instead of polling.
type AgentPushListener = (agentId: string, event: string) => void;
const agentPushListeners = new Set<AgentPushListener>();
type SessionUsageSnapshot = NonNullable<Session['latestUsage']>;

/** @deprecated Use onAgentPush instead */
export function onHypothesisPush(listener: AgentPushListener): () => void {
    return onAgentPush(listener);
}

export function onAgentPush(listener: AgentPushListener): () => void {
    agentPushListeners.add(listener);
    return () => agentPushListeners.delete(listener);
}

function sessionPatchFromAcpState(state: string) {
    switch (state) {
        case 'running':
            return {
                thinking: true,
                thinkingStatus: undefined,
            };
        case 'compressing':
            return {
                thinking: true,
                thinkingStatus: 'compressing',
            };
        case 'requires_action':
            return {
                thinking: false,
                thinkingStatus: 'requires_action',
                streamingText: undefined,
            };
        case 'idle':
            return {
                thinking: false,
                thinkingStatus: undefined,
                streamingText: undefined,
            };
        default:
            return null;
    }
}

function extractTaskLifecycleEntry(rawContent: any, createdAt: number): SessionTaskLifecycleEntry | null {
    const contentType = rawContent?.content?.type;
    const data = rawContent?.content?.data;
    if ((contentType !== 'acp' && contentType !== 'codex') || !data || typeof data !== 'object') {
        return null;
    }

    const taskId = typeof data.id === 'string' && data.id.trim().length > 0 ? data.id.trim() : null;
    if (!taskId) {
        return null;
    }

    if (data.type === 'task_started') {
        return {
            taskId,
            state: 'running',
            updatedAt: createdAt,
            startedAt: createdAt,
            description: typeof data.description === 'string' ? data.description : null,
            taskType: typeof data.taskType === 'string'
                ? data.taskType
                : typeof data.task_type === 'string'
                    ? data.task_type
                    : null,
        };
    }

    if (data.type === 'task_complete') {
        const status = typeof data.status === 'string' ? data.status.trim().toLowerCase() : 'completed';
        return {
            taskId,
            state: status === 'completed' ? 'completed' : 'error',
            updatedAt: createdAt,
            completedAt: createdAt,
            summary: typeof data.summary === 'string' ? data.summary : null,
            description: typeof data.description === 'string' ? data.description : null,
            taskType: typeof data.taskType === 'string'
                ? data.taskType
                : typeof data.task_type === 'string'
                    ? data.task_type
                    : null,
        };
    }

    if (data.type === 'turn_aborted') {
        return {
            taskId,
            state: 'error',
            updatedAt: createdAt,
            completedAt: createdAt,
        };
    }

    return null;
}

function formatTransientExecutorError(message: string, recoverable?: boolean): string {
    return recoverable ? `${message}\n\n可以修复后重试。` : message;
}

function transientNoticeFromRawRecord(rawRecord: RawRecord | null): string | null {
    const content =
        rawRecord?.content && !Array.isArray(rawRecord.content) && typeof rawRecord.content === 'object'
            ? (rawRecord.content as Record<string, unknown>)
            : null;
    if (content?.type !== 'acp') {
        return null;
    }
    const data =
        content.data && typeof content.data === 'object'
            ? (content.data as Record<string, unknown>)
            : null;
    if (data?.type !== 'error' || typeof data.message !== 'string') {
        return null;
    }
    return formatTransientExecutorError(
        data.message,
        typeof data.recoverable === 'boolean' ? data.recoverable : undefined,
    );
}

function normalizePromptSuggestions(suggestions: unknown): string[] | null {
    if (!Array.isArray(suggestions)) {
        return null;
    }

    const normalized = suggestions
        .filter((item): item is string => typeof item === 'string')
        .map((item) => item.trim())
        .filter((item) => item.length > 0);

    return normalized.length > 0 ? normalized : [];
}

function normalizeTokenCountPayload(
    payload: unknown,
    timestamp: number
): SessionUsageSnapshot | null {
    if (!payload || typeof payload !== 'object') {
        return null;
    }

    const data = payload as Record<string, unknown>;
    const inputTokens = typeof data.input_tokens === 'number' ? data.input_tokens : null;
    const outputTokens = typeof data.output_tokens === 'number' ? data.output_tokens : null;
    if (inputTokens == null || outputTokens == null) {
        return null;
    }

    const cacheCreation =
        typeof data.cache_creation_input_tokens === 'number'
            ? data.cache_creation_input_tokens
            : 0;
    const cacheRead =
        typeof data.cache_read_input_tokens === 'number'
            ? data.cache_read_input_tokens
            : 0;
    return {
        inputTokens,
        outputTokens,
        cacheCreation,
        cacheRead,
        contextSize: inputTokens + cacheCreation + cacheRead,
        timestamp,
    };
}

function applyTokenCountToSession(sessionId: string, usage: SessionUsageSnapshot) {
    storage.setState((state) => {
        const session = state.sessions[sessionId];
        if (!session) {
            return state;
        }

        const currentUsage =
            state.sessionMessages[sessionId]?.reducerState.latestUsage ?? session.latestUsage ?? null;
        if (currentUsage && currentUsage.timestamp > usage.timestamp) {
            return state;
        }

        const sessions = {
            ...state.sessions,
            [sessionId]: {
                ...session,
                latestUsage: { ...usage },
                contextTokens: usage.contextSize,
            },
        };

        const sessionMessages = state.sessionMessages[sessionId];
        if (!sessionMessages) {
            return {
                ...state,
                sessions,
            };
        }

        return {
            ...state,
            sessions,
            sessionMessages: {
                ...state.sessionMessages,
                [sessionId]: {
                    ...sessionMessages,
                    reducerState: {
                        ...sessionMessages.reducerState,
                        latestUsage: { ...usage },
                    },
                },
            },
        };
    });
}

type ContextUsageSnapshot = {
    contextTokens: number;
    contextWindowTokens?: number;
    autoCompactTokenLimit?: number;
    timestamp: number;
};

function normalizeContextUsagePayload(payload: unknown, timestamp: number): ContextUsageSnapshot | null {
    if (!payload || typeof payload !== 'object') {
        return null;
    }

    const data = payload as Record<string, unknown>;
    const totalTokens = typeof data.total_tokens === 'number' ? data.total_tokens : null;
    if (totalTokens == null || !Number.isFinite(totalTokens) || totalTokens < 0) {
        return null;
    }
    const maxTokens = typeof data.max_tokens === 'number' && Number.isFinite(data.max_tokens) && data.max_tokens > 0
        ? data.max_tokens
        : undefined;
    const autoCompactTokenLimit =
        typeof data.auto_compact_token_limit === 'number' &&
        Number.isFinite(data.auto_compact_token_limit) &&
        data.auto_compact_token_limit > 0
            ? data.auto_compact_token_limit
            : undefined;
    return {
        contextTokens: totalTokens,
        ...(maxTokens !== undefined ? { contextWindowTokens: maxTokens } : {}),
        ...(autoCompactTokenLimit !== undefined ? { autoCompactTokenLimit } : {}),
        timestamp,
    };
}

function applyContextUsageToSession(sessionId: string, usage: ContextUsageSnapshot) {
    storage.setState((state) => {
        const session = state.sessions[sessionId];
        if (!session) {
            return state;
        }

        const currentUsage =
            state.sessionMessages[sessionId]?.reducerState.latestUsage ?? session.latestUsage ?? null;
        const nextLatestUsage: SessionUsageSnapshot = {
            inputTokens: currentUsage?.inputTokens ?? 0,
            outputTokens: currentUsage?.outputTokens ?? 0,
            cacheCreation: currentUsage?.cacheCreation ?? 0,
            cacheRead: currentUsage?.cacheRead ?? 0,
            contextSize: usage.contextTokens,
            timestamp: usage.timestamp,
        };

        const sameLatestUsage =
            currentUsage?.contextSize === nextLatestUsage.contextSize &&
            currentUsage?.timestamp === nextLatestUsage.timestamp;
        const sameSessionSnapshot =
            session.contextTokens === usage.contextTokens &&
            (usage.contextWindowTokens === undefined || session.contextWindowTokens === usage.contextWindowTokens) &&
            (usage.autoCompactTokenLimit === undefined || session.autoCompactTokenLimit === usage.autoCompactTokenLimit);
        if (sameLatestUsage && sameSessionSnapshot) {
            return state;
        }

        const sessionMessages = state.sessionMessages[sessionId];

        return {
            ...state,
            sessions: {
                ...state.sessions,
                [sessionId]: {
                    ...session,
                    contextTokens: usage.contextTokens,
                    latestUsage: nextLatestUsage,
                    ...(usage.contextWindowTokens !== undefined ? { contextWindowTokens: usage.contextWindowTokens } : {}),
                    ...(usage.autoCompactTokenLimit !== undefined ? { autoCompactTokenLimit: usage.autoCompactTokenLimit } : {}),
                },
            },
            ...(sessionMessages ? {
                sessionMessages: {
                    ...state.sessionMessages,
                    [sessionId]: {
                        ...sessionMessages,
                        reducerState: {
                            ...sessionMessages.reducerState,
                            latestUsage: nextLatestUsage,
                        },
                    },
                },
            } : {}),
        };
    });
}

function applyPersistedSessionEffects(
    sessionId: string,
    rawContent: RawRecord,
    createdAt: number,
    applySessions: (sessions: (Omit<Session, 'presence'> & { presence?: 'online' | number })[]) => void,
) {
    const lifecycleEntry = extractTaskLifecycleEntry(rawContent, createdAt);
    if (lifecycleEntry) {
        storage.getState().applyTaskLifecycle(sessionId, lifecycleEntry);
    }

    const content =
        rawContent.content && !Array.isArray(rawContent.content) && typeof rawContent.content === 'object'
            ? (rawContent.content as Record<string, unknown>)
            : null;
    const contentType = typeof content?.type === 'string' ? content.type : undefined;
    const rawData = content?.data;
    const data =
        rawData && typeof rawData === 'object'
            ? (rawData as Record<string, unknown>)
            : null;
    const dataType = typeof data?.type === 'string' ? data.type : undefined;

    const isTaskComplete =
        ((contentType === 'acp' || contentType === 'codex') &&
            (dataType === 'task_complete' || dataType === 'turn_aborted'));
    const isTaskStarted =
        ((contentType === 'acp' || contentType === 'codex') && dataType === 'task_started');
    const isAcpErrorMessage = contentType === 'acp' && dataType === 'error';
    const acpSessionState =
        contentType === 'acp' && dataType === 'session-state' && typeof data?.state === 'string'
            ? data.state
            : undefined;
    const isRecoverableAcpError =
        isAcpErrorMessage && typeof data?.recoverable === 'boolean' && data.recoverable;
    const sessionStatePatch = acpSessionState
        ? sessionPatchFromAcpState(acpSessionState)
        : null;
    const promptSuggestions =
        contentType === 'acp' && dataType === 'prompt-suggestion'
            ? normalizePromptSuggestions(data?.suggestions)
            : null;
    const tokenCountUpdate =
        contentType === 'acp' && dataType === 'token_count'
            ? normalizeTokenCountPayload(data, createdAt)
            : null;
    const contextUsageUpdate =
        contentType === 'acp' && dataType === 'context_usage'
            ? normalizeContextUsagePayload(data, createdAt)
            : null;
    const isAcpMessage = contentType === 'acp' && dataType === 'message';
    const isThinkingMessage = contentType === 'acp' && dataType === 'thinking';

    const session = storage.getState().sessions[sessionId];
    if (session) {
        // Message reloads replay a page of persisted history on every local
        // append. During an active turn, older completed ACP records from
        // previous turns must not clear the current transient streaming
        // bubble. The current turn starts at `thinkingAt`; only persisted
        // records created after that point are allowed to affect live status.
        const isStaleForActiveTurn =
            session.thinking === true &&
            typeof session.thinkingAt === 'number' &&
            createdAt < session.thinkingAt;
        const shouldApplyLiveState = !isStaleForActiveTurn;
        const shouldClearStreaming =
            shouldApplyLiveState &&
            (isTaskComplete || isAcpMessage || isThinkingMessage || isAcpErrorMessage);
        const nextSession: Session = {
            ...session,
            updatedAt: Math.max(session.updatedAt, createdAt),
            ...(shouldApplyLiveState && isTaskComplete ? { thinking: false } : {}),
            ...(shouldApplyLiveState && isTaskStarted ? { thinking: true, promptSuggestions: [], streamingNotice: undefined } : {}),
            ...(shouldApplyLiveState ? (sessionStatePatch ?? {}) : {}),
            ...(shouldApplyLiveState && promptSuggestions !== null ? { promptSuggestions } : {}),
            ...(shouldApplyLiveState && (isAcpErrorMessage && !isRecoverableAcpError) ? { thinking: false } : {}),
            ...(shouldApplyLiveState && (isAcpMessage || isThinkingMessage || (isAcpErrorMessage && !isRecoverableAcpError)) ? { streamingNotice: undefined } : {}),
            // Streaming bubbles live in the list footer. Once the persisted
            // ACP record arrives, clear them so completed thinking does not
            // stay pinned below later messages.
            ...(shouldClearStreaming ? { streamingText: undefined, streamingThinking: undefined } : {}),
        };
        applySessions([nextSession]);
    }

    if (tokenCountUpdate) {
        applyTokenCountToSession(sessionId, tokenCountUpdate);
    }
    if (contextUsageUpdate) {
        applyContextUsageToSession(sessionId, contextUsageUpdate);
    }
}

class Sync {
    encryption!: Encryption;
    serverID!: string;
    anonID!: string;
    private credentials!: AuthCredentials;
    public encryptionCache = new EncryptionCache();
    private sessionsSync: InvalidateSync;
    private sessionDataKeys = new Map<string, Uint8Array>(); // Store session data encryption keys internally
    private machineDataKeys = new Map<string, Uint8Array>(); // Store machine data encryption keys internally
    private settingsSync: InvalidateSync;
    private profileSync: InvalidateSync;
    private machinesSync: InvalidateSync;
    private pushTokenSync: InvalidateSync;
    private nativeUpdateSync: InvalidateSync;
    private friendsSync: InvalidateSync;
    private friendRequestsSync: InvalidateSync;
    private feedSync: InvalidateSync;
    private todosSync: InvalidateSync;
    private activityAccumulator: ActivityUpdateAccumulator;
    private pendingSettings: Partial<Settings> = loadPendingSettings();
    private localShellMachineId: string | null = null;
    private pendingRelaySessionMessagesRequests = new Map<string, PendingRelaySessionMessagesRequest>();
    private pendingPersonaBindings = new Map<string, PendingPersonaBinding>();
    private pendingPersonaSessionByPersona = new Map<string, string>();
    private pendingPersonaOutbox = new Map<string, QueuedPendingPersonaMessage[]>();
    private completedPersonaSessions = new Map<string, { pendingSessionId: string; sessionId: string }>();
    private failedPersonaSessions = new Map<string, PersonaSessionFailedHostEvent>();

    // One-shot callbacks for first-load detection (bypasses InvalidateSync queue)
    private _onSessionsFirstLoad: (() => void) | null = null;
    private _onMachinesFirstLoad: (() => void) | null = null;

    // Generic locking mechanism
    private recalculationLockCount = 0;
    private lastRecalculationTime = 0;

    constructor() {
        this.sessionsSync = new InvalidateSync(this.fetchSessions);
        this.settingsSync = new InvalidateSync(this.syncSettings);
        this.profileSync = new InvalidateSync(this.fetchProfile);
        this.machinesSync = new InvalidateSync(this.fetchMachines);
        this.nativeUpdateSync = new InvalidateSync(this.fetchNativeUpdate);
        this.friendsSync = new InvalidateSync(this.fetchFriends);
        this.friendRequestsSync = new InvalidateSync(this.fetchFriendRequests);
        this.feedSync = new InvalidateSync(this.fetchFeed);
        this.todosSync = new InvalidateSync(this.fetchTodos);

        const registerPushToken = async () => {
            if (__DEV__) {
                return;
            }
            await this.registerPushToken();
        }
        this.pushTokenSync = new InvalidateSync(registerPushToken);
        this.activityAccumulator = new ActivityUpdateAccumulator(this.flushActivityUpdates.bind(this), 2000);

        // Local-shell sessions have no Socket.IO push. The Rust side fans out
        // broadcast emits through Tauri events (via `LocalEventSink`).
        if (isTauriEnv()) {
            (async () => {
                try {
                    const { listen } = await import('@tauri-apps/api/event');

                    await listen<{
                        sessionId: string;
                        agentState: string | null;
                        version: number;
                    }>('local-session:state-update', (event) => {
                        const { sessionId, agentState: stateJson, version } = event.payload ?? ({} as any);
                        if (!sessionId) return;
                        const session = storage.getState().sessions[sessionId];
                        if (!session) {
                            log.log(`📥 local-session:state-update ${sessionId}: session not loaded, skipping`);
                            return;
                        }
                        let agentState: any = null;
                        if (stateJson) {
                            try {
                                agentState = JSON.parse(stateJson);
                            } catch (e) {
                                log.log(`📥 local-session:state-update parse error: ${e}`);
                                return;
                            }
                        }
                        log.log(
                            `📥 local-session:state-update ${sessionId} v=${version} reqs=${Object.keys(agentState?.requests || {}).length}`
                        );
                        this.applySessions([{
                            ...session,
                            agentState,
                            agentStateVersion: version,
                        }]);
                    });

                    await listen<{
                        sessionId: string;
                        active: boolean;
                        activeAt: number;
                        thinking: boolean;
                        thinkingStatus?: string;
                    }>('local-session:alive', (event) => {
                        const update = event.payload;
                        if (!update?.sessionId) return;
                        this.activityAccumulator.addUpdate({
                            type: 'activity',
                            id: update.sessionId,
                            active: update.active,
                            activeAt: update.activeAt,
                            thinking: update.thinking,
                            thinkingStatus: update.thinkingStatus,
                        });
                    });

                    // Daemon persisted a new ACP record (assistant text, tool
                    // call, task_complete, error …). The streaming callback
                    // only paints transient deltas; without this listener the
                    // final assistant bubble vanishes when stream-end clears
                    // streamingText because the persisted row never reaches
                    // the store. Reloading messages merges the DB row back in.
                    await listen<{ sessionId: string }>(
                        'local-session:message-appended',
                        (event) => {
                            const sessionId = event.payload?.sessionId;
                            if (!sessionId) return;
                            log.log(`📥 local-session:message-appended ${sessionId}`);
                            this.reloadSessionMessages(sessionId);
                        }
                    );

                    await listen<{ sessionId: string; payload: string }>(
                        'local-session:transient',
                        (event) => {
                            const { sessionId, payload } = event.payload ?? ({} as any);
                            if (!sessionId || typeof payload !== 'string') return;
                            const notice = transientNoticeFromRawRecord(parseLocalShellRawRecord(payload));
                            if (!notice) return;
                            const session = storage.getState().sessions[sessionId];
                            if (!session) return;
                            this.applySessions([{
                                ...session,
                                streamingText: undefined,
                                streamingThinking: undefined,
                                streamingNotice: notice,
                                thinking: false,
                            }]);
                        }
                    );

                    // SubAgent lifecycle events from cteno-agent's
                    // SubAgentManager (Spawned / Started / Completed /
                    // Failed / Stopped). The Rust-side `subagent_mirror`
                    // already updated its in-memory registry when this
                    // event fires; we just notify any listening hooks
                    // (currently `useBackgroundTasks` for BackgroundRunsModal)
                    // so they re-fetch via the `list-subagents` RPC. The
                    // mirror is the canonical source — no client-side
                    // mirror needed.
                    await listen<{ sessionId: string }>(
                        'local-session:subagents-updated',
                        (event) => {
                            const sessionId = event.payload?.sessionId;
                            if (!sessionId) return;
                            log.log(`📥 local-session:subagents-updated ${sessionId}`);
                            try {
                                window.dispatchEvent(
                                    new CustomEvent('cteno:subagents-updated', {
                                        detail: { sessionId },
                                    })
                                );
                            } catch (e) {
                                log.log(`Failed to dispatch cteno:subagents-updated: ${e}`);
                            }
                        }
                    );

                    log.log('📥 local-session listeners registered (state-update, alive, message-appended, transient, subagents-updated)');
                } catch (e) {
                    log.log(`Failed to register local-session listeners: ${e}`);
                }
            })();

            // Host event bus: typed, transport-agnostic domain events emitted
            // by the daemon. The Rust side fans each event to every installed
            // HostEventSink; the Tauri sink forwards the serialised HostEvent
            // verbatim on `local-host-event` so both community (no socket)
            // and commercial builds receive updates locally.
            (async () => {
                try {
                    const { listen } = await import('@tauri-apps/api/event');

                    await listen<{
                        type: 'persona-session-ready' | 'persona-session-failed' | 'a2ui-updated' | 'background-task-updated';
                        personaId?: string;
                        pendingSessionId?: string;
                        attemptId?: string;
                        vendor?: string;
                        machineId?: string;
                        sessionId?: string;
                        session?: Omit<Session, 'presence'> & { presence?: 'online' | number };
                        agentId?: string;
                        task?: unknown;
                        error?: string;
                    }>('local-host-event', (event) => {
                        const payload = event.payload;
                        if (!payload || typeof payload !== 'object') return;
                        switch (payload.type) {
                            case 'persona-session-ready':
                                void this.handlePersonaSessionReady(payload as PersonaSessionReadyHostEvent);
                                if (payload.personaId) {
                                    agentPushListeners.forEach(listener =>
                                        listener(payload.personaId as string, 'persona_session_ready')
                                    );
                                }
                                break;
                            case 'persona-session-failed':
                                this.handlePersonaSessionFailed(payload as PersonaSessionFailedHostEvent);
                                if (payload.personaId) {
                                    agentPushListeners.forEach(listener =>
                                        listener(payload.personaId as string, 'persona_session_failed')
                                    );
                                }
                                break;
                            case 'a2ui-updated':
                                if (payload.agentId) {
                                    agentPushListeners.forEach(listener =>
                                        listener(payload.agentId as string, 'a2ui_updated')
                                    );
                                }
                                break;
                            case 'background-task-updated':
                                apiSocket.dispatchLocalMessage('background-task-update', payload.task ?? {});
                                break;
                            default:
                                log.log(`📥 local-host-event: unknown type ${(payload as any).type}`);
                        }
                    });

                    log.log('📥 local-host-event listener registered');
                } catch (e) {
                    log.log(`Failed to register local-host-event listener: ${e}`);
                }
            })();
        }

        // Listen for app state changes to refresh remote state.
        AppState.addEventListener('change', (nextAppState) => {
            if (nextAppState === 'active') {
                log.log('📱 App became active');
                this.profileSync.invalidate();
                this.machinesSync.invalidate();
                this.pushTokenSync.invalidate();
                this.sessionsSync.invalidate();
                this.nativeUpdateSync.invalidate();
                this.friendsSync.invalidate();
                this.friendRequestsSync.invalidate();
                this.feedSync.invalidate();
                this.todosSync.invalidate();
            } else {
                log.log(`📱 App state changed to: ${nextAppState}`);
            }
        });
    }

    async create(credentials: AuthCredentials, encryption: Encryption) {
        this.credentials = credentials;
        this.encryption = encryption;
        this.anonID = encryption.anonID;
        this.serverID = parseToken(credentials.token);
        await this.#init();

        // Await settings sync to have fresh settings
        await this.settingsSync.awaitQueue();

        // Await profile sync to have fresh profile
        await this.profileSync.awaitQueue();

        this.refreshLocalProxyProfiles();
    }

    async restore(credentials: AuthCredentials, encryption: Encryption) {
        // NOTE: No awaiting anything here, we're restoring from a disk (ie app restarted)
        this.credentials = credentials;
        this.encryption = encryption;
        this.anonID = encryption.anonID;
        this.serverID = parseToken(credentials.token);
        await this.#init();
        this.refreshLocalProxyProfiles();
    }

    /**
     * 重新加载 encryption（在添加/删除 imported secrets 后调用）
     */
    async reloadEncryption() {
        if (!this.credentials) {
            throw new Error('Not authenticated');
        }

        this.encryption = await Encryption.create();

        // Update apiSocket's encryption reference to avoid stale object
        apiSocket.updateEncryption(this.encryption);

        // 重新初始化（重新加载 machines 等）
        await this.#init();
    }

    async #init() {

        // Subscribe to updates
        this.subscribeToUpdates();

        // Sync initial PostHog opt-out state with stored settings
        if (tracking) {
            const currentSettings = storage.getState().settings;
            if (currentSettings.analyticsOptOut) {
                tracking.optOut();
            } else {
                tracking.optIn();
            }
        }

        // Invalidate sync
        frontendLog('🔄 #init: Invalidating all syncs');
        log.log('🔄 #init: Invalidating all syncs');
        this.sessionsSync.invalidate();
        this.settingsSync.invalidate();
        this.profileSync.invalidate();
        this.machinesSync.invalidate();
        this.pushTokenSync.invalidate();
        this.nativeUpdateSync.invalidate();
        this.friendsSync.invalidate();
        this.friendRequestsSync.invalidate();
        this.feedSync.invalidate();
        this.todosSync.invalidate();
        log.log('🔄 #init: All syncs invalidated');

        // Wait for both sessions and machines to load, then mark as ready.
        // Uses one-shot callbacks instead of awaitQueue() to avoid being delayed
        // by double-invalidation from socket reconnection or AppState events.
        const readyStart = Date.now();
        const sessionsFirstLoad = new Promise<void>(resolve => { this._onSessionsFirstLoad = resolve; });
        const machinesFirstLoad = new Promise<void>(resolve => { this._onMachinesFirstLoad = resolve; });
        Promise.all([
            sessionsFirstLoad.then(() => frontendLog(`✅ sessionsSync first load (${Date.now() - readyStart}ms)`)),
            machinesFirstLoad.then(() => frontendLog(`✅ machinesSync first load (${Date.now() - readyStart}ms)`))
        ]).then(() => {
            frontendLog(`✅ applyReady — isDataReady=true (${Date.now() - readyStart}ms)`);
            storage.getState().applyReady();
        }).catch((error) => {
            frontendLog(`❌ Failed to load initial data: ${error}`, 'error');
            console.error('Failed to load initial data:', error);
        });
    }

    private resolveLocalShellMachineId = async (): Promise<string | null> => {
        if (!isTauriEnv()) {
            return null;
        }
        if (this.localShellMachineId) {
            return this.localShellMachineId;
        }
        const info = await getLocalHostInfo();
        if (info?.machineId) {
            this.localShellMachineId = info.machineId;
            return info.machineId;
        }
        return null;
    }

    private buildLocalShellMachine = (info: LocalHostInfo): Machine => {
        const now = Date.now();
        return {
            id: info.machineId,
            seq: 0,
            createdAt: now,
            updatedAt: now,
            active: true,
            activeAt: now,
            metadata: {
                host: info.host,
                platform: info.platform,
                happyCliVersion: info.happyCliVersion,
                happyHomeDir: info.happyHomeDir,
                homeDir: info.homeDir,
                displayName: info.host,
            },
            metadataVersion: 0,
            daemonState: null,
            daemonStateVersion: 0,
        };
    }

    private loadLocalModeMachines = async (): Promise<Machine[]> => {
        const info = await getLocalHostInfo();
        if (!info?.machineId) {
            return [];
        }
        this.localShellMachineId = info.machineId;
        return [this.buildLocalShellMachine(info)];
    }

    private loadLocalModeSessions = async (): Promise<Session[]> => {
        const localMachineId = await this.resolveLocalShellMachineId();
        if (!localMachineId) {
            return [];
        }
        return await machineListSessions(localMachineId);
    }

    async initializeLocalMode() {
        frontendLog('🏠 initializeLocalMode: starting');
        const [machines, sessions] = await Promise.all([
            this.loadLocalModeMachines(),
            this.loadLocalModeSessions(),
        ]);
        storage.getState().applyMachines(machines, true);
        this.applySessions(sessions);
        storage.getState().applyReady();
        this.refreshLocalProxyProfiles();
        frontendLog(
            `🏠 initializeLocalMode: ready with ${machines.length} machines and ${sessions.length} sessions`,
        );
    }

    public refreshLocalProxyProfiles = () => {
        if (!this.credentials?.token?.trim() || !isServerAvailable()) {
            return;
        }

        void (async () => {
            const machineId = await this.resolveLocalShellMachineId();
            if (!machineId) {
                return;
            }

            try {
                const result = await machineRefreshProxyProfiles(machineId);
                frontendLog(`[proxyProfiles] refreshed local cache for ${machineId}: ${JSON.stringify(result)}`);
            } catch (error) {
                frontendLog(`[proxyProfiles] failed to refresh local cache: ${String(error)}`, 'warn');
            }
        })();
    }

    private hasServerAccess = (): boolean => {
        return canUseCloudServerAccess(this.credentials?.token);
    }

    private getAvailableServerUrl = (): string | null => {
        if (!this.hasServerAccess()) {
            return null;
        }
        return getServerUrl();
    }

    private clearPendingRelaySessionMessagesRequest = (
        requestId: string,
    ): PendingRelaySessionMessagesRequest | undefined => {
        const pending = this.pendingRelaySessionMessagesRequests.get(requestId);
        if (!pending) {
            return undefined;
        }
        clearTimeout(pending.timeout);
        this.pendingRelaySessionMessagesRequests.delete(requestId);
        return pending;
    }

    private handleRelaySessionMessagesResponse = (payload: unknown) => {
        if (!payload || typeof payload !== 'object') {
            return;
        }
        const response = payload as RelaySessionMessagesResponse;
        const requestId = typeof response.requestId === 'string' ? response.requestId : null;
        if (!requestId) {
            return;
        }

        const pending = this.clearPendingRelaySessionMessagesRequest(requestId);
        if (!pending) {
            return;
        }

        pending.resolve(response);
    }

    private handleMessageAppended = (payload: unknown) => {
        if (!payload || typeof payload !== 'object') {
            return;
        }
        const data = payload as { sid?: unknown; message?: unknown; transient?: unknown };
        if (data.transient !== true || typeof data.sid !== 'string' || typeof data.message !== 'string') {
            return;
        }
        const notice = transientNoticeFromRawRecord(parseLocalShellRawRecord(data.message));
        if (!notice) {
            return;
        }
        const session = storage.getState().sessions[data.sid];
        if (!session) {
            return;
        }
        this.applySessions([{
            ...session,
            streamingText: undefined,
            streamingThinking: undefined,
            streamingNotice: notice,
            thinking: false,
        }]);
    }

    private applyCachedSessionMessages = (
        sessionId: string,
        cached: SessionMessagesCacheEntry,
        options?: { keepSyncing?: boolean },
    ) => {
        const normalizedMessages = normalizePersistedSessionMessages(
            sessionId,
            cached.messages,
            this.applySessions.bind(this),
        );

        storage.getState().applySessionMessagesLocal(
            sessionId,
            normalizedMessages,
            Boolean(cached.meta.hasOlderMessages),
        );
        storage.getState().updateMessagesPagination(sessionId, {
            hasOlderMessages: Boolean(cached.meta.hasOlderMessages),
            oldestSeq: cached.meta.maxSeq,
            isLoadingOlder: false,
        });

        if (options?.keepSyncing) {
            storage.getState().setSessionMessagesStatus(sessionId, {
                isLoaded: true,
                isSyncing: true,
                relayError: null,
            });
        }
    }

    private persistSessionMessagesCache = async (
        sessionId: string,
        messages: LocalSessionMessage[],
        options: { offset?: number; hasOlderMessages?: boolean },
    ) => {
        try {
            await saveMessageCache(sessionId, messages, {
                maxSeq: pageMaxSeq(options.offset ?? 0, messages.length),
                hasOlderMessages: options.hasOlderMessages,
            });
        } catch (error) {
            log.log(`💬 persistSessionMessagesCache failed for ${sessionId}: ${error}`);
        }
    }

    loadSessionMessagesLocal = async (
        sessionId: string,
        options?: { limit?: number; offset?: number },
    ): Promise<void> => {
        if (!isTauriEnv()) {
            return;
        }

        const { invoke } = await import('@tauri-apps/api/core');
        const limit = options?.limit ?? LOCAL_SESSION_MESSAGES_PAGE_SIZE;
        const offset = options?.offset ?? 0;
        const result = await invoke('get_session_messages', {
            sessionId,
            limit,
            offset,
        }) as LocalSessionMessagesPage;

        const normalizedMessages = normalizePersistedSessionMessages(
            sessionId,
            result.messages,
            this.applySessions.bind(this),
        );

        storage.getState().applySessionMessagesLocal(sessionId, normalizedMessages, result.hasMore);
        storage.getState().updateMessagesPagination(sessionId, {
            hasOlderMessages: result.hasMore,
            oldestSeq: pageMaxSeq(offset, result.messages.length),
            isLoadingOlder: false,
        });

        await this.persistSessionMessagesCache(sessionId, result.messages, {
            offset,
            hasOlderMessages: result.hasMore,
        });
    }

    loadSessionMessagesRelay = async (
        sessionId: string,
        options?: { limit?: number; offset?: number },
    ): Promise<void> => {
        if (!this.hasServerAccess()) {
            throw new Error('Server unavailable');
        }

        const limit = options?.limit ?? LOCAL_SESSION_MESSAGES_PAGE_SIZE;
        const offset = options?.offset ?? 0;
        const requestId = randomUUID();

        try {
            await apiSocket.waitUntilConnected(10_000);

            const response = await new Promise<RelaySessionMessagesResponse>((resolve, reject) => {
                const timeout = setTimeout(() => {
                    this.pendingRelaySessionMessagesRequests.delete(requestId);
                    reject(new Error('relay session messages request timed out'));
                }, 10_000);

                this.pendingRelaySessionMessagesRequests.set(requestId, {
                    sessionId,
                    timeout,
                    resolve,
                    reject,
                });

                const sent = apiSocket.send('relay:session-messages-request', {
                    sessionId,
                    requestId,
                    limit,
                    offset,
                });

                if (!sent) {
                    const pending = this.clearPendingRelaySessionMessagesRequest(requestId);
                    pending?.reject(new Error('Socket not connected'));
                }
            });

            if (response.error) {
                throw new RelaySessionMessagesError(response.error);
            }

            const relayMessages = Array.isArray(response.messages) ? response.messages : [];
            const normalizedMessages = normalizePersistedSessionMessages(
                sessionId,
                relayMessages,
                this.applySessions.bind(this),
            );

            storage.getState().applySessionMessagesLocal(
                sessionId,
                normalizedMessages,
                Boolean(response.hasMore),
            );
            storage.getState().updateMessagesPagination(sessionId, {
                hasOlderMessages: Boolean(response.hasMore),
                oldestSeq: pageMaxSeq(offset, relayMessages.length),
                isLoadingOlder: false,
            });

            await this.persistSessionMessagesCache(sessionId, relayMessages, {
                offset,
                hasOlderMessages: Boolean(response.hasMore),
            });
            storage.getState().setSessionMessagesStatus(sessionId, {
                isLoaded: true,
                isSyncing: false,
                relayError: null,
            });
        } catch (error) {
            storage.getState().setSessionMessagesStatus(sessionId, {
                isLoaded: true,
                isSyncing: false,
                relayError: toSessionMessagesErrorCode(error),
            });
            throw error;
        }
    }

    private loadVisibleSessionMessages = async (
        sessionId: string,
        options?: { force?: boolean },
    ): Promise<void> => {
        const session = storage.getState().sessions[sessionId];
        if (!session) {
            return;
        }

        const existingState = storage.getState().sessionMessages[sessionId];
        if (!options?.force && existingState?.isLoaded && existingState.messages.length > 0 && !existingState.relayError) {
            return;
        }

        const localMachineId = await this.resolveLocalShellMachineId();
        if (session.metadata?.machineId && localMachineId && session.metadata.machineId === localMachineId) {
            storage.getState().setSessionMessagesStatus(sessionId, {
                isLoaded: (existingState?.messages.length ?? 0) > 0,
                isSyncing: true,
                relayError: null,
            });
            try {
                await this.loadSessionMessagesLocal(sessionId, {
                    limit: LOCAL_SESSION_MESSAGES_PAGE_SIZE,
                    offset: 0,
                });
            } catch (error) {
                storage.getState().setSessionMessagesStatus(sessionId, {
                    isLoaded: true,
                    isSyncing: false,
                    relayError: toSessionMessagesErrorCode(error),
                });
                log.log(`💬 loadSessionMessagesLocal failed for ${sessionId}: ${error}`);
            }
            return;
        }

        if (session.metadata?.machineId) {
            storage.getState().setSessionMessagesStatus(sessionId, {
                isLoaded: (existingState?.messages.length ?? 0) > 0,
                isSyncing: true,
                relayError: null,
            });

            let relayCompleted = false;
            let relaySucceeded = false;
            const relayPromise = (async () => {
                try {
                    await this.loadSessionMessagesRelay(sessionId, {
                        limit: LOCAL_SESSION_MESSAGES_PAGE_SIZE,
                        offset: 0,
                    });
                    relaySucceeded = true;
                } finally {
                    relayCompleted = true;
                }
            })();

            try {
                const cached = await loadMessageCache(sessionId);
                if (cached && cached.messages.length > 0 && (!relayCompleted || !relaySucceeded)) {
                    this.applyCachedSessionMessages(sessionId, cached, { keepSyncing: true });
                }
            } catch (error) {
                log.log(`💬 loadMessageCache failed for ${sessionId}: ${error}`);
            }

            try {
                await relayPromise;
            } catch (error) {
                storage.getState().setSessionMessagesStatus(sessionId, {
                    isLoaded: true,
                    isSyncing: false,
                    relayError: toSessionMessagesErrorCode(error),
                });
                log.log(`💬 loadSessionMessagesRelay failed for ${sessionId}: ${error}`);
            }
        }
    }

    onSessionVisible = (sessionId: string) => {
        void this.loadVisibleSessionMessages(sessionId);

        // Also invalidate git status sync for this session
        gitStatusSync.getSync(sessionId).invalidate();
    }

    reloadSessionMessages = (sessionId: string) => {
        void this.loadVisibleSessionMessages(sessionId, { force: true });
    }

    registerPendingPersonaSession(persona: Persona, machineId: string, options?: { pendingSessionId?: string; attemptId?: string; vendor?: string }) {
        const pendingSessionId = options?.pendingSessionId || persona.chatSessionId;
        if (!pendingSessionId?.startsWith('pending-')) {
            return;
        }

        const completed = this.completedPersonaSessions.get(persona.id);
        if (completed?.pendingSessionId === pendingSessionId) {
            const personas = storage.getState().cachedPersonas;
            if (personas.some((item) => item.id === persona.id && item.chatSessionId !== completed.sessionId)) {
                storage.getState().applyPersonas(personas.map((item) => (
                    item.id === persona.id
                        ? { ...item, chatSessionId: completed.sessionId }
                        : item
                )));
            }
            return;
        }

        const now = Date.now();
        const vendor = options?.vendor || persona.agent || 'cteno';
        this.pendingPersonaBindings.set(pendingSessionId, {
            personaId: persona.id,
            pendingSessionId,
            machineId,
            attemptId: options?.attemptId,
            vendor,
            createdAt: now,
        });
        this.pendingPersonaSessionByPersona.set(persona.id, pendingSessionId);

        if (!storage.getState().sessions[pendingSessionId]) {
            this.applySessions([{
                id: pendingSessionId,
                seq: 0,
                createdAt: now,
                updatedAt: now,
                active: true,
                activeAt: now,
                metadata: {
                    path: persona.workdir || '~',
                    host: 'local-shell',
                    name: persona.name || vendor,
                    machineId,
                    flavor: vendor,
                    vendor,
                    modelId: persona.modelId || undefined,
                    pending: true,
                    personaId: persona.id,
                    summary: {
                        text: '',
                        updatedAt: now,
                    },
                } as any,
                metadataVersion: 0,
                agentState: null,
                agentStateVersion: 0,
                thinking: true,
                thinkingAt: now,
                thinkingStatus: '启动中',
                permissionMode: 'default',
                runtimeEffort: 'default',
                sandboxPolicy: 'workspace_write',
                modelMode: vendor === 'gemini' ? 'gemini-2.5-pro' : 'default',
            }]);
            storage.getState().setSessionMessagesStatus(pendingSessionId, {
                isLoaded: true,
                isSyncing: false,
                relayError: null,
            });
        }

        const failed = this.failedPersonaSessions.get(persona.id);
        if (failed?.pendingSessionId === pendingSessionId) {
            this.handlePersonaSessionFailed(failed);
        }
    }

    private async handlePersonaSessionReady(payload: PersonaSessionReadyHostEvent) {
        const pendingSessionId = payload.pendingSessionId
            || this.pendingPersonaSessionByPersona.get(payload.personaId);
        if (!pendingSessionId || !payload.sessionId) {
            log.log(`persona-session-ready missing ids: ${JSON.stringify(payload)}`);
            return;
        }
        this.completedPersonaSessions.set(payload.personaId, {
            pendingSessionId,
            sessionId: payload.sessionId,
        });
        this.failedPersonaSessions.delete(payload.personaId);

        let session = payload.session;
        const machineId = payload.machineId || this.pendingPersonaBindings.get(pendingSessionId)?.machineId;
        if (!session && machineId) {
            try {
                const sessions = await machineListSessions(machineId);
                session = sessions.find((item) => item.id === payload.sessionId);
            } catch (error) {
                log.log(`persona-session-ready could not fetch session ${payload.sessionId}: ${error}`);
            }
        }

        if (session) {
            storage.getState().bindPendingPersonaSession(pendingSessionId, session);
        } else {
            const existing = storage.getState().sessions[pendingSessionId];
            if (existing) {
                storage.getState().bindPendingPersonaSession(pendingSessionId, {
                    ...existing,
                    id: payload.sessionId,
                    active: true,
                    activeAt: Date.now(),
                    updatedAt: Date.now(),
                    thinking: false,
                    thinkingStatus: undefined,
                    metadata: {
                        ...(existing.metadata ?? {}),
                        pending: false,
                        machineId,
                        vendor: payload.vendor || existing.metadata?.vendor,
                    } as any,
                } as any);
            }
        }

        const personas = storage.getState().cachedPersonas;
        const nextPersonas = personas.map((persona) => (
            persona.id === payload.personaId
                ? { ...persona, chatSessionId: payload.sessionId }
                : persona
        ));
        if (nextPersonas !== personas) {
            storage.getState().applyPersonas(nextPersonas);
        }

        this.pendingPersonaBindings.delete(pendingSessionId);
        this.pendingPersonaSessionByPersona.delete(payload.personaId);

        const queue = (this.pendingPersonaOutbox.get(pendingSessionId) ?? [])
            .slice()
            .sort((a, b) => a.createdAt - b.createdAt);
        this.pendingPersonaOutbox.delete(pendingSessionId);

        for (const item of queue) {
            await this.sendMessage(
                payload.sessionId,
                item.text,
                item.displayText,
                item.images,
                { localId: item.localId, skipOptimistic: true }
            );
        }
    }

    private handlePersonaSessionFailed(payload: PersonaSessionFailedHostEvent) {
        const pendingSessionId = payload.pendingSessionId
            || this.pendingPersonaSessionByPersona.get(payload.personaId);
        if (!pendingSessionId) {
            return;
        }
        this.failedPersonaSessions.set(payload.personaId, payload);
        const session = storage.getState().sessions[pendingSessionId];
        if (session) {
            this.applySessions([{
                ...session,
                active: false,
                activeAt: Date.now(),
                updatedAt: Date.now(),
                thinking: false,
                thinkingStatus: undefined,
                streamingNotice: payload.error ? `启动失败：${payload.error}` : '启动失败',
            }]);
        }
        log.log(`persona-session-failed ${payload.personaId}: ${payload.error || 'unknown error'}`);
    }


    async sendMessage(
        sessionId: string,
        text: string,
        displayText?: string,
        images?: Array<{ media_type: string; data: string }>,
        options: { localId?: string; skipOptimistic?: boolean } = {},
    ) {
        // Get session data from storage
        const session = storage.getState().sessions[sessionId];
        if (!session) {
            console.error(`Session ${sessionId} not found in storage`);
            return;
        }

        // Read permission mode from session state
        const permissionMode = session.permissionMode || 'default';
        const reasoningEffort = session.runtimeEffort || 'default';

        // Read model mode - for Gemini, default to gemini-2.5-pro if not set
        const flavor = session.metadata?.flavor;
        const isGemini = flavor === 'gemini' || session.metadata?.vendor === 'gemini';
        const modelMode = session.modelMode || (isGemini ? 'gemini-2.5-pro' : 'default');

        // Generate local ID
        const localId = options.localId || randomUUID();

        // Determine sentFrom based on platform
        let sentFrom: string;
        if (Platform.OS === 'web') {
            sentFrom = 'web';
        } else if (Platform.OS === 'android') {
            sentFrom = 'android';
        } else if (Platform.OS === 'ios') {
            // Check if running on Mac (Catalyst or Designed for iPad on Mac)
            if (isRunningOnMac()) {
                sentFrom = 'mac';
            } else {
                sentFrom = 'ios';
            }
        } else {
            sentFrom = 'web'; // fallback
        }

        // Model settings - for Gemini, we pass the selected model; for others, CLI handles it
        let model: string | null = null;
        if (isGemini && modelMode !== 'default') {
            // For Gemini ACP, pass the selected model to CLI
            model = modelMode;
        }
        const fallbackModel: string | null = null;

        // Build message content with image references
        let messageContent: any;
        if (images && images.length > 0) {
            const blocks: any[] = [];
            for (const img of images) {
                blocks.push({
                    type: 'image',
                    source: {
                        type: 'base64',
                        media_type: img.media_type,
                        data: img.data,
                    }
                });
            }
            blocks.push({ type: 'text', text });
            messageContent = blocks;
        } else {
            messageContent = { type: 'text', text };
        }

        const content: RawRecord = {
            role: 'user',
            content: messageContent,
            meta: {
                sentFrom,
                permissionMode: permissionMode || 'default',
                reasoningEffort,
                model,
                fallbackModel,
                appendSystemPrompt: systemPrompt,
                ...(displayText && { displayText }) // Add displayText if provided
            }
        };

        // Add to messages - normalize the raw record
        const createdAt = Date.now();
        const normalizedMessage = normalizeRawMessage(localId, localId, createdAt, content);
        if (normalizedMessage && !options.skipOptimistic) {
            this.upsertSessionMessages(sessionId, [normalizedMessage]);
        }

        if (sessionId.startsWith('pending-')) {
            const queue = this.pendingPersonaOutbox.get(sessionId) ?? [];
            queue.push({ localId, text, displayText, images, createdAt });
            this.pendingPersonaOutbox.set(sessionId, queue);
            this.applySessions([{
                ...session,
                updatedAt: createdAt,
                active: true,
                activeAt: createdAt,
                thinking: true,
                thinkingAt: createdAt,
                thinkingStatus: '启动中',
            }]);
            log.log(`queued message for pending persona session ${sessionId}`);
            return;
        }

        // Try local IPC path first (Tauri desktop — bypasses server entirely)
        // Uses Tauri Channel for streaming deltas back to frontend.
        const useTauriLocal = isTauriEnv();
        if (useTauriLocal) {
            try {
                const { invoke, Channel } = await import('@tauri-apps/api/core');
                // Ensure the session socket on the machine is alive
                const machineId = session.metadata?.machineId;
                if (machineId) {
                    try {
                        await machineReconnectSession(machineId, sessionId);
                    } catch (e) {
                        log.log(`reconnect-session failed for ${sessionId}: ${e}`);
                    }
                }

                // Create Tauri Channel for streaming deltas
                const onEvent = new Channel<{ event: string; data?: { text?: string; message?: string; recoverable?: boolean } }>();
                // Typewriter state: queue of pending characters to animate
                let typewriterQueue = '';
                let typewriterTimer: ReturnType<typeof setTimeout> | null = null;
                let lastTypewriterTickAt = nowForTypewriter();

                const scheduleTypewriterTick = (delayMs: number) => {
                    if (typewriterTimer) {
                        return;
                    }
                    typewriterTimer = setTimeout(tickTypewriter, delayMs);
                };

                const flushTypewriter = () => {
                    if (typewriterTimer) { clearTimeout(typewriterTimer); typewriterTimer = null; }
                    if (typewriterQueue.length > 0) {
                        const s = storage.getState().sessions[sessionId];
                        if (s) {
                            this.applySessions([{ ...s, streamingText: (s.streamingText || '') + typewriterQueue }]);
                        }
                        typewriterQueue = '';
                    }
                    lastTypewriterTickAt = nowForTypewriter();
                };

                const tickTypewriter = () => {
                    typewriterTimer = null;
                    if (typewriterQueue.length === 0) return;
                    const now = nowForTypewriter();
                    const elapsedMs = now - lastTypewriterTickAt;
                    lastTypewriterTickAt = now;
                    const chunkSize = computeTypewriterChunkSize({
                        pendingChars: typewriterQueue.length,
                        elapsedMs,
                    });
                    const chunk = typewriterQueue.slice(0, chunkSize);
                    typewriterQueue = typewriterQueue.slice(chunk.length);
                    const s = storage.getState().sessions[sessionId];
                    if (s) {
                        this.applySessions([{ ...s, streamingText: (s.streamingText || '') + chunk }]);
                    }
                    if (typewriterQueue.length > 0) {
                        scheduleTypewriterTick(TYPEWRITER_FRAME_MS);
                    }
                };

                onEvent.onmessage = (msg: { event: string; data?: { text?: string; message?: string; recoverable?: boolean } }) => {
                    const sess = storage.getState().sessions[sessionId];
                    if (!sess) return;
                    switch (msg.event) {
                        case 'stream-start':
                            typewriterQueue = '';
                            if (typewriterTimer) { clearTimeout(typewriterTimer); typewriterTimer = null; }
                            lastTypewriterTickAt = nowForTypewriter();
                            this.applySessions([{ ...sess, streamingText: undefined, streamingThinking: undefined, streamingNotice: undefined, thinking: true }]);
                            break;
                        case 'text-delta': {
                            const text = msg.data?.text || '';
                            typewriterQueue += text;
                            // Start animation if not already running
                            if (!typewriterTimer && typewriterQueue.length > 0) {
                                scheduleTypewriterTick(0);
                            }
                            break;
                        }
                        case 'thinking-delta':
                            this.applySessions([{ ...sess, streamingThinking: (sess.streamingThinking || '') + (msg.data?.text || '') }]);
                            break;
                        case 'stream-end': {
                            // Flush remaining typewriter queue before clearing
                            flushTypewriter();
                            const s = storage.getState().sessions[sessionId];
                            if (s) {
                                this.applySessions([{ ...s, streamingText: undefined, streamingThinking: undefined }]);
                            }
                            break;
                        }
                        case 'finished': {
                            flushTypewriter();
                            const s = storage.getState().sessions[sessionId];
                            if (s) {
                                this.applySessions([{ ...s, streamingText: undefined, streamingThinking: undefined, thinking: false }]);
                            }
                            break;
                        }
                        case 'error': {
                            flushTypewriter();
                            const s = storage.getState().sessions[sessionId];
                            if (s) {
                                this.applySessions([{
                                    ...s,
                                    streamingText: undefined,
                                    streamingThinking: undefined,
                                    streamingNotice: msg.data?.message && msg.data.recoverable
                                        ? formatTransientExecutorError(msg.data.message, true)
                                        : undefined,
                                    thinking: false,
                                }]);
                            }
                            break;
                        }
                    }
                };

                // invoke blocks until agent completes, Channel delivers streaming deltas meanwhile
                await invoke('send_message_local', {
                    sessionId,
                    text,
                    images: images || null,
                    permissionMode: permissionMode || 'default',
                    model,
                    localId,
                    onEvent,
                });
                log.log(`[LocalIPC] Message processed via Tauri Channel for session ${sessionId}`);
                return;
            } catch (e) {
                if (!this.credentials || !this.encryption) {
                    log.log(`[LocalIPC] Tauri invoke failed with no remote fallback available: ${e}`);
                    console.error(`[LocalIPC] Tauri invoke failed with no remote fallback available:`, e);
                    return;
                }
                log.log(`[LocalIPC] Tauri invoke failed, falling back to Socket.IO: ${e}`);
                // Fall through to Socket.IO path
            }
        }

        // 2.0 remote path: Socket.IO via server (mobile / web / fallback).
        // Payloads are plaintext JSON — no per-session key refresh needed.
        const machineId = session.metadata?.machineId;
        if (machineId) {
            try {
                await machineReconnectSession(machineId, sessionId);
            } catch (e) {
                log.log(`reconnect-session failed for ${sessionId}: ${e}`);
            }
        }

        apiSocket.send('message', {
            sid: sessionId,
            message: JSON.stringify(content),
            localId,
            sentFrom,
            permissionMode: permissionMode || 'default',
        });
    }

    applySettings = (delta: Partial<Settings>) => {
        storage.getState().applySettingsLocal(delta);

        // Save pending settings
        this.pendingSettings = { ...this.pendingSettings, ...delta };
        savePendingSettings(this.pendingSettings);

        // Sync PostHog opt-out state if it was changed
        if (tracking && 'analyticsOptOut' in delta) {
            const currentSettings = storage.getState().settings;
            if (currentSettings.analyticsOptOut) {
                tracking.optOut();
            } else {
                tracking.optIn();
            }
        }

        // Invalidate settings sync
        this.settingsSync.invalidate();
    }

    refreshProfile = async () => {
        await this.profileSync.invalidateAndAwait();
    }

    async assumeUsers(userIds: string[]): Promise<void> {
        if (!this.hasServerAccess() || userIds.length === 0) return;
        
        const state = storage.getState();
        // Filter out users we already have in cache (including null for 404s)
        const missingIds = userIds.filter(id => !(id in state.users));
        
        if (missingIds.length === 0) return;
        
        log.log(`👤 Fetching ${missingIds.length} missing users...`);
        
        // Fetch missing users in parallel
        const results = await Promise.all(
            missingIds.map(async (id) => {
                try {
                    const profile = await getUserProfile(this.credentials!, id);
                    return { id, profile };  // profile is null if 404
                } catch (error) {
                    console.error(`Failed to fetch user ${id}:`, error);
                    return { id, profile: null };  // Treat errors as 404
                }
            })
        );
        
        // Convert to Record<string, UserProfile | null>
        const usersMap: Record<string, UserProfile | null> = {};
        results.forEach(({ id, profile }) => {
            usersMap[id] = profile;
        });
        
        storage.getState().applyUsers(usersMap);
        log.log(`👤 Applied ${results.length} users to cache (${results.filter(r => r.profile).length} found, ${results.filter(r => !r.profile).length} not found)`);
    }

    //
    // Private
    //

    private fetchSessions = async () => {
        // Platform-split session source:
        //
        //   - Desktop (Tauri):  own-machine sessions come from the local
        //     daemon's SQLite via RPC; the server is used *only* to read
        //     sessions that belong to the user's OTHER machines. We never
        //     read our own machine's sessions over the socket — socket stays
        //     write-only for local sessions, preventing blocked UI when the
        //     server is slow or the account has many remote sessions.
        //
        //   - Mobile / web: there's no local daemon, so everything comes from
        //     the server (current socket-based path).
        //
        // For an unsigned desktop user, the local snapshot is the whole story;
        // for an unsigned web user, there's nothing to show.

        const desktop = isTauriEnv();
        const localMachineId = desktop ? await this.resolveLocalShellMachineId() : null;
        const localSessions = localMachineId
            ? await machineListSessions(localMachineId)
            : [];

        if (desktop) {
            // Apply local sessions immediately so the UI paints without
            // waiting on a possibly slow socket round-trip.
            this.applySessions(localSessions);
            frontendLog(`📡 fetchSessions: local snapshot — ${localSessions.length} sessions`);
            if (this._onSessionsFirstLoad) {
                this._onSessionsFirstLoad();
                this._onSessionsFirstLoad = null;
            }
        }

        // Server-as-relay: no HTTP /v1/sessions endpoint anymore. Session
        // lists only come from each owner daemon — desktop already hydrated
        // its local slice above; cross-machine views (mobile, or desktop
        // looking at another registered machine) should fire
        // `relay:list-sessions-request` per machine. That RPC is tracked as a
        // follow-up (see docs/server-relay-refactor.md) and is a no-op for
        // now; UI falls back to showing just local sessions.
        if (!desktop) {
            this.applySessions([]);
            if (this._onSessionsFirstLoad) {
                this._onSessionsFirstLoad();
                this._onSessionsFirstLoad = null;
            }
        }
    }

    public refreshMachines = async () => {
        return this.fetchMachines();
    }

    public refreshSessions = async () => {
        return this.sessionsSync.invalidateAndAwait();
    }

    public getCredentials() {
        return this.credentials;
    }

    public setLocalModeCredentials(credentials: AuthCredentials | null) {
        if (credentials) {
            this.credentials = credentials;
            this.serverID = parseToken(credentials.token);
            this.refreshLocalProxyProfiles();
        }
    }

    private fetchMachines = async () => {
        // Same split as fetchSessions: on desktop the local daemon's own
        // machine record is source-of-truth (socket is write-only for it),
        // while other machines under the same account come from the server.
        const desktop = isTauriEnv();
        const localMachines = desktop ? await this.loadLocalModeMachines() : [];

        if (desktop) {
            // Paint local machine immediately so the UI has something to
            // attach sessions to without waiting on the socket round-trip.
            storage.getState().applyMachines(localMachines, true);
            if (this._onMachinesFirstLoad) {
                this._onMachinesFirstLoad();
                this._onMachinesFirstLoad = null;
            }
        }

        const API_ENDPOINT = this.getAvailableServerUrl();
        if (!API_ENDPOINT) {
            if (!desktop) {
                storage.getState().applyMachines([], true);
                if (this._onMachinesFirstLoad) {
                    this._onMachinesFirstLoad();
                    this._onMachinesFirstLoad = null;
                }
            }
            return;
        }
        const _t0 = Date.now();
        frontendLog('📡 fetchMachines: starting...');
        console.log('📊 Sync: Fetching machines...');
        const response = await apiSocket.request('/v1/machines', {
            headers: { 'Content-Type': 'application/json' },
        });

        if (!response.ok) {
            console.error(`Failed to fetch machines: ${response.status}`);
            return;
        }

        const data = await response.json();
        console.log(`📊 Sync: Fetched ${Array.isArray(data) ? data.length : 0} machines from server`);
        if (Array.isArray(data)) {
            data.forEach((m: any, idx: number) => {
                console.log(`  Machine ${idx + 1}: ${m.id?.substring(0, 30)}... active=${m.active}`);
            });
        }
        const machines = data as Array<{
            id: string;
            metadata: string;
            metadataVersion: number;
            daemonState?: string | null;
            daemonStateVersion?: number;
            dataEncryptionKey?: string | null; // Add support for per-machine encryption keys
            seq: number;
            active: boolean;
            activeAt: number;  // Changed from lastActiveAt
            createdAt: number;
            updatedAt: number;
        }>;

        // 2.0 plaintext mode: machines arrive with plaintext metadata.  The
        // Encryption compat-layer accepts a null key and just JSON-parses the
        // payloads. No "decryption failed" state possible.
        const machineKeysMap = new Map<string, Uint8Array | null>();
        for (const machine of machines) {
            machineKeysMap.set(machine.id, null);
        }
        await this.encryption.initializeMachines(machineKeysMap);

        const decryptedMachines: Machine[] = [];
        for (const machine of machines) {
            const machineEncryption = this.encryption.getMachineEncryption(machine.id)!;
            const metadata = machine.metadata
                ? await machineEncryption.decryptMetadata(machine.metadataVersion, machine.metadata)
                : null;
            const daemonState = machine.daemonState
                ? await machineEncryption.decryptDaemonState(machine.daemonStateVersion || 0, machine.daemonState)
                : null;
            decryptedMachines.push({
                id: machine.id,
                seq: machine.seq,
                createdAt: machine.createdAt,
                updatedAt: machine.updatedAt,
                active: machine.active,
                activeAt: machine.activeAt,
                metadata,
                metadataVersion: machine.metadataVersion,
                daemonState,
                daemonStateVersion: machine.daemonStateVersion || 0,
            });
        }

        // Desktop merge: keep local machine record for our own ID (source-of-
        // truth is the daemon), and append server entries for every OTHER
        // machine under the account. Non-desktop just uses server data as-is.
        let finalMachines: Machine[];
        if (desktop) {
            const localIds = new Set(localMachines.map((m) => m.id));
            const remoteOnly = decryptedMachines.filter((m) => !localIds.has(m.id));
            finalMachines = [...localMachines, ...remoteOnly];
            console.log(
                `🖥️ Desktop merge: ${localMachines.length} local + ${remoteOnly.length} remote (from ${decryptedMachines.length} server)`,
            );
        } else {
            finalMachines = decryptedMachines;
        }

        console.log(`🖥️ About to apply ${finalMachines.length} machines to storage`);
        finalMachines.forEach((m, idx) => {
            console.log(`  Machine ${idx + 1}: ${m.id.substring(0, 30)}... active=${m.active} activeAt=${m.activeAt}`);
        });
        storage.getState().applyMachines(finalMachines, true);
        frontendLog(`📡 fetchMachines: completed — ${finalMachines.length} machines (total ${Date.now() - _t0}ms)`);
        log.log(`🖥️ fetchMachines completed - processed ${finalMachines.length} machines`);

        // Signal first-load completion (one-shot, bypasses InvalidateSync queue)
        if (this._onMachinesFirstLoad) {
            this._onMachinesFirstLoad();
            this._onMachinesFirstLoad = null;
        }
    }

    private fetchFriends = async () => {
        if (!this.hasServerAccess()) return;
        
        try {
            log.log('👥 Fetching friends list...');
            const friendsList = await getFriendsList(this.credentials);
            storage.getState().applyFriends(friendsList);
            log.log(`👥 fetchFriends completed - processed ${friendsList.length} friends`);
        } catch (error) {
            console.error('Failed to fetch friends:', error);
            // Silently handle error - UI will show appropriate state
        }
    }

    private fetchFriendRequests = async () => {
        // Friend requests are now included in the friends list with status='pending'
        // This method is kept for backward compatibility but does nothing
        log.log('👥 fetchFriendRequests called - now handled by fetchFriends');
    }

    private fetchTodos = async () => {
        if (!this.hasServerAccess()) return;

        try {
            log.log('📝 Fetching todos...');
            await initializeTodoSync();
            log.log('📝 Todos loaded');
        } catch (error) {
            log.log('📝 Failed to fetch todos:');
        }
    }

    private applyTodoSocketUpdates = async (changes: any[]) => {
        if (!this.credentials || !this.encryption) return;

        const currentState = storage.getState();
        const todoState = currentState.todoState;
        if (!todoState) {
            // No todo state yet, just refetch
            this.todosSync.invalidate();
            return;
        }

        const { todos, undoneOrder, doneOrder, versions } = todoState;
        let updatedTodos = { ...todos };
        let updatedVersions = { ...versions };
        let indexUpdated = false;
        let newUndoneOrder = undoneOrder;
        let newDoneOrder = doneOrder;

        // Process each change
        for (const change of changes) {
            try {
                const key = change.key;
                const version = change.version;

                // Update version tracking
                updatedVersions[key] = version;

                if (change.value === null) {
                    // Item was deleted
                    if (key.startsWith('todo.') && key !== 'todo.index') {
                        const todoId = key.substring(5); // Remove 'todo.' prefix
                        delete updatedTodos[todoId];
                        newUndoneOrder = newUndoneOrder.filter(id => id !== todoId);
                        newDoneOrder = newDoneOrder.filter(id => id !== todoId);
                    }
                } else {
                    // Item was added or updated
                    const decrypted = await this.encryption.decryptRaw(change.value);

                    if (key === 'todo.index') {
                        // Update the index
                        const index = decrypted as any;
                        newUndoneOrder = index.undoneOrder || [];
                        newDoneOrder = index.completedOrder || []; // Map completedOrder to doneOrder
                        indexUpdated = true;
                    } else if (key.startsWith('todo.')) {
                        // Update a todo item
                        const todoId = key.substring(5);
                        if (todoId && todoId !== 'index') {
                            updatedTodos[todoId] = decrypted as any;
                        }
                    }
                }
            } catch (error) {
                console.error(`Failed to process todo change for key ${change.key}:`, error);
            }
        }

        // Apply the updated state
        storage.getState().applyTodos({
            todos: updatedTodos,
            undoneOrder: newUndoneOrder,
            doneOrder: newDoneOrder,
            versions: updatedVersions
        });

        log.log('📝 Applied todo socket updates successfully');
    }

    private fetchFeed = async () => {
        if (!this.hasServerAccess()) return;

        try {
            log.log('📰 Fetching feed...');
            const state = storage.getState();
            const existingItems = state.feedItems;
            const head = state.feedHead;
            
            // Load feed items - if we have a head, load newer items
            let allItems: FeedItem[] = [];
            let hasMore = true;
            let cursor = head ? { after: head } : undefined;
            let loadedCount = 0;
            const maxItems = 500;
            
            // Keep loading until we reach known items or hit max limit
            while (hasMore && loadedCount < maxItems) {
                const response = await fetchFeed(this.credentials, {
                    limit: 100,
                    ...cursor
                });
                
                // Check if we reached known items
                const foundKnown = response.items.some(item => 
                    existingItems.some(existing => existing.id === item.id)
                );
                
                allItems.push(...response.items);
                loadedCount += response.items.length;
                hasMore = response.hasMore && !foundKnown;
                
                // Update cursor for next page
                if (response.items.length > 0) {
                    const lastItem = response.items[response.items.length - 1];
                    cursor = { after: lastItem.cursor };
                }
            }
            
            // If this is initial load (no head), also load older items
            if (!head && allItems.length < 100) {
                const response = await fetchFeed(this.credentials, {
                    limit: 100
                });
                allItems.push(...response.items);
            }
            
            // Collect user IDs from friend-related feed items
            const userIds = new Set<string>();
            allItems.forEach(item => {
                if (item.body && (item.body.kind === 'friend_request' || item.body.kind === 'friend_accepted')) {
                    userIds.add(item.body.uid);
                }
            });
            
            // Fetch missing users
            if (userIds.size > 0) {
                await this.assumeUsers(Array.from(userIds));
            }
            
            // Filter out items where user is not found (404)
            const users = storage.getState().users;
            const compatibleItems = allItems.filter(item => {
                // Keep text items
                if (item.body.kind === 'text') return true;
                
                // For friend-related items, check if user exists and is not null (404)
                if (item.body.kind === 'friend_request' || item.body.kind === 'friend_accepted') {
                    const userProfile = users[item.body.uid];
                    // Keep item only if user exists and is not null
                    return userProfile !== null && userProfile !== undefined;
                }
                
                return true;
            });
            
            // Apply only compatible items to storage
            storage.getState().applyFeedItems(compatibleItems);
            log.log(`📰 fetchFeed completed - loaded ${compatibleItems.length} compatible items (${allItems.length - compatibleItems.length} filtered)`);
        } catch (error) {
            console.error('Failed to fetch feed:', error);
        }
    }

    private syncSettings = async () => {
        const API_ENDPOINT = this.getAvailableServerUrl();
        if (!API_ENDPOINT) return;
        const maxRetries = 3;
        let retryCount = 0;

        // Apply pending settings
        if (Object.keys(this.pendingSettings).length > 0) {

            while (retryCount < maxRetries) {
                let version = storage.getState().settingsVersion;
                let settings = applySettings(storage.getState().settings, this.pendingSettings);
                const response = await apiSocket.request('/v1/account/settings', {
                    method: 'POST',
                    body: JSON.stringify({
                        // Plaintext JSON in 2.0 — server strips any encryption envelope.
                        settings: JSON.stringify(settings),
                        expectedVersion: version ?? 0,
                    }),
                    headers: { 'Content-Type': 'application/json' },
                });
                const data = await response.json() as {
                    success: false,
                    error: string,
                    currentVersion: number,
                    currentSettings: string | null
                } | {
                    success: true
                };
                if (data.success) {
                    this.pendingSettings = {};
                    savePendingSettings({});
                    break;
                }
                if (data.error === 'version-mismatch') {
                    // Parse server settings (plaintext JSON in 2.0)
                    const serverSettings = data.currentSettings
                        ? settingsParse(safeParseJson(data.currentSettings))
                        : { ...settingsDefaults };

                    // Merge: server base + our pending changes (our changes win)
                    const mergedSettings = applySettings(serverSettings, this.pendingSettings);

                    // Update local storage with merged result at server's version
                    storage.getState().applySettings(mergedSettings, data.currentVersion);

                    // Sync tracking state with merged settings
                    if (tracking) {
                        mergedSettings.analyticsOptOut ? tracking.optOut() : tracking.optIn();
                    }

                    // Log and retry
                    console.log('settings version-mismatch, retrying', {
                        serverVersion: data.currentVersion,
                        retry: retryCount + 1,
                        pendingKeys: Object.keys(this.pendingSettings)
                    });
                    retryCount++;
                    continue;
                } else {
                    throw new Error(`Failed to sync settings: ${data.error}`);
                }
            }
        }

        // If exhausted retries, throw to trigger outer backoff delay
        if (retryCount >= maxRetries) {
            throw new Error(`Settings sync failed after ${maxRetries} retries due to version conflicts`);
        }

        // Run request
        const response = await apiSocket.request('/v1/account/settings', {
            headers: { 'Content-Type': 'application/json' },
        });
        if (!response.ok) {
            throw new Error(`Failed to fetch settings: ${response.status}`);
        }
        const data = await response.json() as {
            settings: string | null,
            settingsVersion: number
        };

        // Parse response (plaintext JSON in 2.0)
        let parsedSettings: Settings;
        if (data.settings) {
            parsedSettings = settingsParse(safeParseJson(data.settings));
        } else {
            parsedSettings = { ...settingsDefaults };
        }

        // Log
        console.log('settings', JSON.stringify({
            settings: parsedSettings,
            version: data.settingsVersion
        }));

        // Apply settings to storage
        storage.getState().applySettings(parsedSettings, data.settingsVersion);

        // Sync PostHog opt-out state with settings
        if (tracking) {
            if (parsedSettings.analyticsOptOut) {
                tracking.optOut();
            } else {
                tracking.optIn();
            }
        }
    }

    private fetchProfile = async () => {
        if (!this.hasServerAccess()) return;
        const response = await apiSocket.request('/v1/account/profile', {
            headers: { 'Content-Type': 'application/json' },
        });

        if (!response.ok) {
            throw new Error(`Failed to fetch profile: ${response.status}`);
        }

        const data = await response.json();
        const parsedProfile = profileParse(data);

        // Log profile data for debugging
        console.log('profile', JSON.stringify({
            id: parsedProfile.id,
            timestamp: parsedProfile.timestamp,
            firstName: parsedProfile.firstName,
            lastName: parsedProfile.lastName,
            hasAvatar: !!parsedProfile.avatar,
            hasGitHub: !!parsedProfile.github
        }));

        // Apply profile to storage
        storage.getState().applyProfile(parsedProfile);
    }

    private fetchNativeUpdate = async () => {
        try {
            const serverUrl = this.getAvailableServerUrl();
            if (!serverUrl) {
                return;
            }
            // Skip in development
            if ((Platform.OS !== 'android' && Platform.OS !== 'ios') || !Constants.expoConfig?.version) {
                return;
            }
            if (Platform.OS === 'ios' && !Constants.expoConfig?.ios?.bundleIdentifier) {
                return;
            }
            if (Platform.OS === 'android' && !Constants.expoConfig?.android?.package) {
                return;
            }

            // Get platform and app identifiers
            const platform = Platform.OS;
            const version = Constants.expoConfig?.version!;
            const appId = (Platform.OS === 'ios' ? Constants.expoConfig?.ios?.bundleIdentifier! : Constants.expoConfig?.android?.package!);

            const response = await fetch(`${serverUrl}/v1/version`, {
                method: 'POST',
                headers: {
                    'Content-Type': 'application/json',
                },
                body: JSON.stringify({
                    platform,
                    version,
                    app_id: appId,
                }),
            });

            if (!response.ok) {
                console.log(`[fetchNativeUpdate] Request failed: ${response.status}`);
                return;
            }

            const data = await response.json();
            console.log('[fetchNativeUpdate] Data:', data);

            // Apply update status to storage
            if (data.update_required && data.update_url) {
                storage.getState().applyNativeUpdateStatus({
                    available: true,
                    updateUrl: data.update_url
                });
            } else {
                storage.getState().applyNativeUpdateStatus({
                    available: false
                });
            }
        } catch (error) {
            console.log('[fetchNativeUpdate] Error:', error);
            storage.getState().applyNativeUpdateStatus(null);
        }
    }

    loadOlderMessages = async (sessionId: string) => {
        const sessionState = storage.getState().sessionMessages[sessionId];
        if (!sessionState || !sessionState.hasOlderMessages || sessionState.isLoadingOlder) {
            return;
        }

        const session = storage.getState().sessions[sessionId];
        const localMachineId = await this.resolveLocalShellMachineId();
        if (!session?.metadata?.machineId) {
            return;
        }

        storage.getState().updateMessagesPagination(sessionId, { isLoadingOlder: true });
        storage.getState().setSessionMessagesStatus(sessionId, { relayError: null });
        try {
            if (localMachineId && session.metadata.machineId === localMachineId) {
                await this.loadSessionMessagesLocal(sessionId, {
                    limit: LOCAL_SESSION_MESSAGES_PAGE_SIZE,
                    offset: sessionState.oldestSeq ?? 0,
                });
            } else {
                await this.loadSessionMessagesRelay(sessionId, {
                    limit: LOCAL_SESSION_MESSAGES_PAGE_SIZE,
                    offset: sessionState.oldestSeq ?? 0,
                });
            }
        } catch (error) {
            log.log(`💬 loadOlderMessages failed for ${sessionId}: ${error}`);
            storage.getState().updateMessagesPagination(sessionId, { isLoadingOlder: false });
            storage.getState().setSessionMessagesStatus(sessionId, {
                relayError: toSessionMessagesErrorCode(error),
            });
        }
    }

    private registerPushToken = async () => {
        log.log('registerPushToken');
        // Only register on mobile platforms
        if (Platform.OS === 'web') {
            return;
        }

        // Request permission
        const { status: existingStatus } = await Notifications.getPermissionsAsync();
        let finalStatus = existingStatus;
        log.log('existingStatus: ' + JSON.stringify(existingStatus));

        if (existingStatus !== 'granted') {
            const { status } = await Notifications.requestPermissionsAsync();
            finalStatus = status;
        }
        log.log('finalStatus: ' + JSON.stringify(finalStatus));

        if (finalStatus !== 'granted') {
            console.log('Failed to get push token for push notification!');
            return;
        }

        // Get push token
        const projectId = Constants?.expoConfig?.extra?.eas?.projectId ?? Constants?.easConfig?.projectId;

        const tokenData = await Notifications.getExpoPushTokenAsync({ projectId });
        log.log('tokenData: ' + JSON.stringify(tokenData));

        // Register with server
        try {
            await registerPushToken(this.credentials, tokenData.data);
            log.log('Push token registered successfully');
        } catch (error) {
            log.log('Failed to register push token: ' + JSON.stringify(error));
        }
    }

    private subscribeToUpdates = () => {
        // Subscribe to message updates
        apiSocket.onMessage('update', this.handleUpdate.bind(this));
        apiSocket.onMessage('ephemeral', this.handleEphemeralUpdate.bind(this));
        apiSocket.onMessage('message-appended', this.handleMessageAppended.bind(this));
        apiSocket.onMessage('relay:session-messages-response', this.handleRelaySessionMessagesResponse);

        // Periodic machine status refresh (every 30 seconds)
        // This ensures machine online/offline status is updated even if ephemeral events fail
        setInterval(() => {
            this.machinesSync.invalidate();
        }, 30000);

        // Periodic vendor-quota poll (every 60s, aligned with the daemon's
        // own probe interval). Reads the daemon's cached snapshot for every
        // known machine — local first, remote once the server connection is up.
        const pollVendorQuota = async () => {
            const ids = new Set<string>(Object.keys(storage.getState().machines));
            // Resolve the local shell id on-demand — `subscribeToUpdates` may
            // run before the machine list has loaded during cold boot.
            const localId = await this.resolveLocalShellMachineId().catch(() => null);
            if (localId) ids.add(localId);
            for (const id of ids) {
                fetchVendorQuota(id).catch(() => {});
            }
        };
        pollVendorQuota();
        setInterval(pollVendorQuota, 60000);

        // Subscribe to connection state changes
        apiSocket.onReconnected(() => {
            log.log('🔌 Socket reconnected');
            this.sessionsSync.invalidate();
            this.machinesSync.invalidate();
            this.friendsSync.invalidate();
            this.friendRequestsSync.invalidate();
            this.feedSync.invalidate();
            const sessionsData = storage.getState().sessionsData;
            if (sessionsData) {
                for (const item of sessionsData) {
                    if (typeof item !== 'string') {
                        // Also invalidate git status on reconnection
                        gitStatusSync.invalidate(item.id);
                    }
                }
            }
        });
    }

    private handleUpdate = async (update: unknown) => {
        console.log('🔄 Sync: handleUpdate called with:', JSON.stringify(update).substring(0, 300));
        const validatedUpdate = ApiUpdateContainerSchema.safeParse(update);
        if (!validatedUpdate.success) {
            console.log('❌ Sync: Invalid update received:', validatedUpdate.error);
            console.error('❌ Sync: Invalid update data:', update);
            return;
        }
        const updateData = validatedUpdate.data;
        console.log(`🔄 Sync: Validated update type: ${updateData.body.t}`);

        if (updateData.body.t === 'new-session') {
            log.log('🆕 New session update received');
            this.sessionsSync.invalidate();
        } else if (updateData.body.t === 'delete-session') {
            log.log('🗑️ Delete session update received');
            const sessionId = updateData.body.sid;

            // Remove session from storage
            storage.getState().deleteSession(sessionId);

            // Remove encryption keys from memory
            this.encryption.removeSessionEncryption(sessionId);

            // Remove from project manager
            projectManager.removeSession(sessionId);

            // Clear any cached git status
            gitStatusSync.clearForSession(sessionId);

            log.log(`🗑️ Session ${sessionId} deleted from local storage`);
        } else if (updateData.body.t === 'update-session') {
            const session = storage.getState().sessions[updateData.body.id];
            if (session) {
                // Get session encryption
                const sessionEncryption = this.encryption.getSessionEncryption(updateData.body.id);
                if (!sessionEncryption) {
                    // Session key not available on this device
                    return;
                }

                const _dbg = (msg: string) => {
                    if (!ENABLE_SYNC_UPDATE_DEBUG) {
                        return;
                    }
                    const ts = new Date().toISOString().slice(11, 23);
                    console.debug(`[sync/update-session ${ts}] ${msg}`);
                };
                _dbg(`sync: update-session sid=${updateData.body.id.slice(-8)} hasAS=${!!updateData.body.agentState} ver=${updateData.body.agentState?.version} oldVer=${session.agentStateVersion}`);

                let agentState;
                try {
                    agentState = updateData.body.agentState && sessionEncryption
                        ? await sessionEncryption.decryptAgentState(updateData.body.agentState.version, updateData.body.agentState.value)
                        : session.agentState;
                } catch (e) {
                    _dbg(`sync: DECRYPT FAILED: ${e}`);
                    agentState = session.agentState;
                }

                if (agentState) {
                    _dbg(`sync: decrypted reqs=${JSON.stringify(Object.keys(agentState.requests || {}))} comp=${JSON.stringify(Object.keys(agentState.completedRequests || {}))}`);
                }

                const metadata = updateData.body.metadata && sessionEncryption
                    ? await sessionEncryption.decryptMetadata(updateData.body.metadata.version, updateData.body.metadata.value)
                    : session.metadata;

                this.applySessions([{
                    ...session,
                    agentState,
                    agentStateVersion: updateData.body.agentState
                        ? updateData.body.agentState.version
                        : session.agentStateVersion,
                    metadata,
                    metadataVersion: updateData.body.metadata
                        ? updateData.body.metadata.version
                        : session.metadataVersion,
                    updatedAt: updateData.createdAt,
                    seq: updateData.seq
                }]);

                // Invalidate git status when agent state changes (files may have been modified)
                if (updateData.body.agentState) {
                    gitStatusSync.invalidate(updateData.body.id);

                    // Check for new permission requests and notify voice assistant
                    if (agentState?.requests && Object.keys(agentState.requests).length > 0) {
                    }
                }
            }
        } else if (updateData.body.t === 'update-account') {
            const accountUpdate = updateData.body;
            const currentProfile = storage.getState().profile;

            // Build updated profile with new data
            const updatedProfile: Profile = {
                ...currentProfile,
                firstName: accountUpdate.firstName !== undefined ? accountUpdate.firstName : currentProfile.firstName,
                lastName: accountUpdate.lastName !== undefined ? accountUpdate.lastName : currentProfile.lastName,
                avatar: accountUpdate.avatar !== undefined ? accountUpdate.avatar : currentProfile.avatar,
                github: accountUpdate.github !== undefined ? accountUpdate.github : currentProfile.github,
                wechat: accountUpdate.wechat !== undefined ? accountUpdate.wechat : currentProfile.wechat,
                timestamp: updateData.createdAt // Update timestamp to latest
            };

            // Apply the updated profile to storage
            storage.getState().applyProfile(updatedProfile);

            // Handle settings updates (new for profile sync)
            if (accountUpdate.settings?.value) {
                try {
                    const decryptedSettings = await this.encryption.decryptRaw(accountUpdate.settings.value);
                    const parsedSettings = settingsParse(decryptedSettings);

                    // Version compatibility check
                    const settingsSchemaVersion = parsedSettings.schemaVersion ?? 1;
                    if (settingsSchemaVersion > SUPPORTED_SCHEMA_VERSION) {
                        console.warn(
                            `⚠️ Received settings schema v${settingsSchemaVersion}, ` +
                            `we support v${SUPPORTED_SCHEMA_VERSION}. Update app for full functionality.`
                        );
                    }

                    storage.getState().applySettings(parsedSettings, accountUpdate.settings.version);
                    log.log(`📋 Settings synced from server (schema v${settingsSchemaVersion}, version ${accountUpdate.settings.version})`);
                } catch (error) {
                    console.error('❌ Failed to process settings update:', error);
                    // Don't crash on settings sync errors, just log
                }
            }
        } else if (updateData.body.t === 'update-machine') {
            const machineUpdate = updateData.body;
            const machineId = machineUpdate.machineId;  // Changed from .id to .machineId
            const machine = storage.getState().machines[machineId];

            // Create or update machine with all required fields
            const updatedMachine: Machine = {
                id: machineId,
                seq: updateData.seq,
                createdAt: machine?.createdAt ?? updateData.createdAt,
                updatedAt: updateData.createdAt,
                active: machineUpdate.active ?? true,
                activeAt: machineUpdate.activeAt ?? updateData.createdAt,
                metadata: machine?.metadata ?? null,
                metadataVersion: machine?.metadataVersion ?? 0,
                daemonState: machine?.daemonState ?? null,
                daemonStateVersion: machine?.daemonStateVersion ?? 0
            };

            // Get machine-specific encryption (might not exist if machine wasn't initialized)
            const machineEncryption = this.encryption.getMachineEncryption(machineId);
            if (!machineEncryption) {
                console.error(`Machine encryption not found for ${machineId} - cannot decrypt updates`);
                return;
            }

            // If metadata is provided, decrypt and update it
            const metadataUpdate = machineUpdate.metadata;
            if (metadataUpdate) {
                try {
                    const metadata = await machineEncryption.decryptMetadata(metadataUpdate.version, metadataUpdate.value);
                    updatedMachine.metadata = metadata;
                    updatedMachine.metadataVersion = metadataUpdate.version;
                } catch (error) {
                    console.error(`Failed to decrypt machine metadata for ${machineId}:`, error);
                }
            }

            // If daemonState is provided, decrypt and update it
            const daemonStateUpdate = machineUpdate.daemonState;
            if (daemonStateUpdate) {
                try {
                    const daemonState = await machineEncryption.decryptDaemonState(daemonStateUpdate.version, daemonStateUpdate.value);
                    updatedMachine.daemonState = daemonState;
                    updatedMachine.daemonStateVersion = daemonStateUpdate.version;
                } catch (error) {
                    console.error(`Failed to decrypt machine daemonState for ${machineId}:`, error);
                }
            }

            // Update storage using applyMachines which rebuilds sessionListViewData
            storage.getState().applyMachines([updatedMachine]);
        } else if (updateData.body.t === 'relationship-updated') {
            log.log('👥 Received relationship-updated update');
            const relationshipUpdate = updateData.body;
            
            // Apply the relationship update to storage
            storage.getState().applyRelationshipUpdate({
                fromUserId: relationshipUpdate.fromUserId,
                toUserId: relationshipUpdate.toUserId,
                status: relationshipUpdate.status,
                action: relationshipUpdate.action,
                fromUser: relationshipUpdate.fromUser,
                toUser: relationshipUpdate.toUser,
                timestamp: relationshipUpdate.timestamp
            });
            
            // Invalidate friends data to refresh with latest changes
            this.friendsSync.invalidate();
            this.friendRequestsSync.invalidate();
            this.feedSync.invalidate();
        } else if (updateData.body.t === 'new-feed-post') {
            log.log('📰 Received new-feed-post update');
            const feedUpdate = updateData.body;
            
            // Convert to FeedItem with counter from cursor
            const feedItem: FeedItem = {
                id: feedUpdate.id,
                body: feedUpdate.body,
                cursor: feedUpdate.cursor,
                createdAt: feedUpdate.createdAt,
                repeatKey: feedUpdate.repeatKey,
                counter: parseInt(feedUpdate.cursor.substring(2), 10)
            };
            
            // Check if we need to fetch user for friend-related items
            if (feedItem.body && (feedItem.body.kind === 'friend_request' || feedItem.body.kind === 'friend_accepted')) {
                await this.assumeUsers([feedItem.body.uid]);
                
                // Check if user fetch failed (404) - don't store item if user not found
                const users = storage.getState().users;
                const userProfile = users[feedItem.body.uid];
                if (userProfile === null || userProfile === undefined) {
                    // User was not found or 404, don't store this item
                    log.log(`📰 Skipping feed item ${feedItem.id} - user ${feedItem.body.uid} not found`);
                    return;
                }
            }
            
            // Apply to storage (will handle repeatKey replacement)
            storage.getState().applyFeedItems([feedItem]);
        } else if (updateData.body.t === 'kv-batch-update') {
            log.log('📝 Received kv-batch-update');
            const kvUpdate = updateData.body;

            // Process KV changes for todos
            if (kvUpdate.changes && Array.isArray(kvUpdate.changes)) {
                const todoChanges = kvUpdate.changes.filter(change =>
                    change.key && change.key.startsWith('todo.')
                );

                if (todoChanges.length > 0) {
                    log.log(`📝 Processing ${todoChanges.length} todo KV changes from socket`);

                    // Apply the changes directly to avoid unnecessary refetch
                    try {
                        await this.applyTodoSocketUpdates(todoChanges);
                    } catch (error) {
                        console.error('Failed to apply todo socket updates:', error);
                        // Fallback to refetch on error
                        this.todosSync.invalidate();
                    }
                }
            }
        }
    }

    private flushActivityUpdates = (updates: Map<string, ApiEphemeralActivityUpdate>) => {
        // log.log(`🔄 Flushing activity updates for ${updates.size} sessions - acquiring lock`);


        const sessions: Session[] = [];

        for (const [sessionId, update] of updates) {
            const session = storage.getState().sessions[sessionId];
            if (session) {
                sessions.push({
                    ...session,
                    active: update.active,
                    activeAt: update.activeAt,
                    thinking: update.thinking ?? false,
                    thinkingAt: update.activeAt, // Always use activeAt for consistency
                    thinkingStatus: update.thinkingStatus,
                });
            }
        }

        if (sessions.length > 0) {
            // console.log('flushing activity updates ' + sessions.length);
            this.applySessions(sessions);
            // log.log(`🔄 Activity updates flushed - updated ${sessions.length} sessions`);
        }
    }

    private handleEphemeralUpdate = (update: unknown) => {
        const validatedUpdate = ApiEphemeralUpdateSchema.safeParse(update);
        if (!validatedUpdate.success) {
            console.log('Invalid ephemeral update received:', validatedUpdate.error);
            console.error('Invalid ephemeral update received:', update);
            return;
        } else {
            // console.log('Ephemeral update received:', update);
        }
        const updateData = validatedUpdate.data;

        // Process activity updates through smart debounce accumulator
        if (updateData.type === 'activity') {
            // console.log('adding activity update ' + updateData.id);
            this.activityAccumulator.addUpdate(updateData);
        }

        // Handle machine activity updates
        if (updateData.type === 'machine-activity') {
            // Update machine's active status and lastActiveAt
            const machine = storage.getState().machines[updateData.id];
            if (machine) {
                const updatedMachine: Machine = {
                    ...machine,
                    active: updateData.active,
                    activeAt: updateData.activeAt
                };
                storage.getState().applyMachines([updatedMachine]);
            }
        }

        if (updateData.type === 'usage') {
            storage.getState().applyLocalProxyUsage({
                key: updateData.key,
                sessionId: updateData.id,
                timestamp: updateData.timestamp,
                totalTokens: updateData.tokens.total,
                inputTokens: updateData.tokens.input,
                outputTokens: updateData.tokens.output,
                cacheCreationTokens: updateData.tokens.cache_creation,
                cacheReadTokens: updateData.tokens.cache_read,
                totalCostYuan: updateData.cost.total,
                inputCostYuan: updateData.cost.input,
                outputCostYuan: updateData.cost.output,
            });
        }

        // Agent push events (A2UI updates, persona session ready, etc.)
        if (updateData.type === 'hypothesis-push') {
            const { agentId, event } = updateData as { agentId: string; event: string };
            agentPushListeners.forEach(listener => listener(agentId, event));
        }

        // daemon-status ephemeral updates are deprecated, machine status is handled via machine-activity
    }

    //
    // Apply store
    //

    private upsertSessionMessages = (sessionId: string, messages: NormalizedMessage[]) => {
        storage.getState().upsertSessionMessages(sessionId, messages);
    }

    private applySessions = (sessions: (Omit<Session, "presence"> & {
        presence?: "online" | number;
    })[]) => {
        const active = storage.getState().getActiveSessions();
        storage.getState().applySessions(sessions);
        const newActive = storage.getState().getActiveSessions();
        this.applySessionDiff(active, newActive);
    }

    private applySessionDiff = (active: Session[], newActive: Session[]) => {
        let wasActive = new Set(active.map(s => s.id));
        let isActive = new Set(newActive.map(s => s.id));
        for (let s of active) {
            if (!isActive.has(s.id)) {
            }
        }
        for (let s of newActive) {
            if (!wasActive.has(s.id)) {
            }
        }
    }

}

// Global singleton instance
export const sync = new Sync();

// Expose sync to window for debugging (dev only)
if (__DEV__) {
    (window as any).__sync = sync;
}

//
// Init sequence
//

let isInitialized = false;
export async function syncCreate(credentials: AuthCredentials) {
    if (isInitialized) {
        console.warn('Sync already initialized: ignoring');
        return;
    }
    isInitialized = true;
    await syncInit(credentials, false);
}

export async function syncRestore(credentials: AuthCredentials) {
    if (isInitialized) {
        console.warn('Sync already initialized: ignoring');
        return;
    }
    isInitialized = true;
    await syncInit(credentials, true);
}

export async function syncInitLocalMode(credentials?: AuthCredentials | null) {
    if (isInitialized) {
        console.warn('Sync already initialized: ignoring');
        return;
    }
    isInitialized = true;
    sync.setLocalModeCredentials(credentials ?? null);
    await sync.initializeLocalMode();
}

export function syncSetLocalModeCredentials(credentials: AuthCredentials | null) {
    sync.setLocalModeCredentials(credentials);
}

async function syncInit(credentials: AuthCredentials, restore: boolean) {

    // Initialize sync engine
    const encryption = await Encryption.create();

    // Initialize tracking
    initializeTracking(encryption.anonID);

    // Initialize socket connection
    const API_ENDPOINT = getServerUrl();
    apiSocket.initialize({ endpoint: API_ENDPOINT, token: credentials.accessToken });

    // Wire socket status to storage
    apiSocket.onStatusChange((status) => {
        storage.getState().setSocketStatus(status);
    });

    // Initialize sessions engine
    if (restore) {
        await sync.restore(credentials, encryption);
    } else {
        await sync.create(credentials, encryption);
    }
}
