import { create } from "zustand";
import { useShallow } from 'zustand/react/shallow'
import { Session, Machine, GitStatus, Persona, PersonaProject, WorkspaceSummary, SessionTaskLifecycleEntry } from "./storageTypes";
import { createReducer, reducer, ReducerState } from "./reducer/reducer";
import { Message } from "./typesMessage";
import { NormalizedMessage } from "./typesRaw";
import { isMachineOnline } from '@/utils/machineUtils';
import { applySettings, Settings } from "./settings";
import { LocalSettings, applyLocalSettings } from "./localSettings";
import { LocalProxyUsage, LocalProxyUsageRecord, upsertLocalProxyUsageRecord } from "./localProxyUsage";
import { TodoState } from "../-zen/model/ops";
import { Profile } from "./profile";
import { UserProfile, RelationshipUpdatedEvent } from "./friendTypes";
import { loadSettings, loadLocalSettings, saveLocalSettings, saveSettings, loadLocalProxyUsage, saveLocalProxyUsage, loadProfile, saveProfile, loadSessionDrafts, saveSessionDrafts, loadSessionPermissionModes, saveSessionPermissionModes, loadSessionRuntimeEfforts, saveSessionRuntimeEfforts, loadSessionSandboxPolicies, saveSessionSandboxPolicies, loadPersonaReadTimestamps, savePersonaReadTimestamps, loadCachedPersonas, saveCachedPersonas, loadCachedPersonaProjects, saveCachedPersonaProjects, loadCachedAgentWorkspaces, saveCachedAgentWorkspaces } from "./persistence";
import type { PermissionMode } from '@/components/PermissionModeSelector';
import React from "react";
import { sync } from "./sync";
import { getCurrentRealtimeSessionId, getVoiceSession } from '@/realtime/RealtimeSession';
import { isMutableTool } from "@/components/tools/knownTools";
import { projectManager } from "./projectManager";
import { FeedItem } from "./feedTypes";

// Keep disabled by default: piping debug logs through /tools/shell/execute can
// create heavy local traffic and interfere with normal agent execution.
const ENABLE_SYNC_DEBUG_LOG = false;
function debugLog(msg: string) {
    if (!ENABLE_SYNC_DEBUG_LOG) {
        return;
    }
    const ts = new Date().toISOString().slice(11, 23);
    console.debug(`[sync/storage ${ts}] ${msg}`);
}

type SessionTodo = {
    content: string;
    status: 'pending' | 'in_progress' | 'completed';
    priority?: 'high' | 'medium' | 'low';
    id?: string;
};

function isPlanToolName(name: string): boolean {
    const normalized = name.trim().toLowerCase();
    return normalized === 'update_plan' || normalized === 'update plan' || normalized === 'todowrite';
}

function planItemsFromInput(input: any): unknown {
    return input?.todos ?? input?.newTodos ?? input?.items ?? input?.plan;
}

function latestTodosFromStoredMessages(messages: Message[]): SessionTodo[] | null {
    for (let i = messages.length - 1; i >= 0; i -= 1) {
        const message = messages[i];
        if (message.kind !== 'tool-call' || !isPlanToolName(message.tool.name)) {
            continue;
        }
        const rawTodos = planItemsFromInput(message.tool.input);
        if (!Array.isArray(rawTodos)) {
            continue;
        }
        if (rawTodos.length === 0) {
            return [];
        }
        const todos = rawTodos
            .map((todo, index): SessionTodo | null => {
                const content = typeof todo?.content === 'string'
                    ? todo.content
                    : typeof todo?.step === 'string'
                        ? todo.step
                        : typeof todo?.text === 'string'
                            ? todo.text
                            : typeof todo?.title === 'string'
                                ? todo.title
                                : typeof todo?.task === 'string'
                                    ? todo.task
                                    : '';
                if (!content.trim()) {
                    return null;
                }
                const rawStatus = typeof todo?.status === 'string' ? todo.status.trim().toLowerCase() : '';
                const status = rawStatus === 'completed' || rawStatus === 'complete' || rawStatus === 'done'
                    ? 'completed'
                    : rawStatus === 'in_progress' || rawStatus === 'in-progress' || rawStatus === 'inprogress' || rawStatus === 'running'
                        ? 'in_progress'
                        : rawStatus === 'pending' || rawStatus === 'queued'
                            ? 'pending'
                        : 'pending';
                return {
                    content,
                    status,
                    priority: todo?.priority === 'high' || todo?.priority === 'medium' || todo?.priority === 'low'
                        ? todo.priority
                        : undefined,
                    id: typeof todo?.id === 'string' ? todo.id : `stored-plan-${message.id}-${index}`,
                };
            })
            .filter((todo): todo is SessionTodo => todo !== null);
        if (todos.length > 0) {
            return todos;
        }
    }
    return null;
}

function buildHiddenWorkspaceMemberSessionIds(workspaces: WorkspaceSummary[]): Set<string> {
    return new Set(
        workspaces.flatMap((workspace) => workspace.members.map((member) => member.sessionId))
    );
}



/**
 * Centralized session online state resolver
 * Returns either "online" (string) or a timestamp (number) for last seen
 */
function resolveSessionOnlineState(session: { active: boolean; activeAt: number }): "online" | number {
    // Session is online if the active flag is true
    return session.active ? "online" : session.activeAt;
}

/**
 * Checks if a session should be shown in the active sessions group
 */
function isSessionActive(session: { active: boolean; activeAt: number }): boolean {
    // Use the active flag directly, no timeout checks
    return session.active;
}

interface SessionMessages {
    messages: Message[];
    messagesMap: Record<string, Message>;
    taskLifecycle: Record<string, SessionTaskLifecycleEntry>;
    reducerState: ReducerState;
    isLoaded: boolean;
    lastReducerAgentStateVersion?: number;
    hasOlderMessages: boolean;
    oldestSeq?: number;
    isLoadingOlder: boolean;
    isSyncing: boolean;
    relayError?: string | null;
}

function createEmptySessionMessages(): SessionMessages {
    return {
        messages: [],
        messagesMap: {},
        taskLifecycle: {},
        reducerState: createReducer(),
        isLoaded: false,
        hasOlderMessages: false,
        oldestSeq: 0,
        isLoadingOlder: false,
        isSyncing: false,
        relayError: null,
    };
}

// Machine type is now imported from storageTypes - represents persisted machine data

// Unified list item type for SessionsList component
export type SessionListViewItem =
    | { type: 'header'; title: string }
    | { type: 'active-sessions'; sessions: Session[] }
    | { type: 'project-group'; displayPath: string; machine: Machine }
    | { type: 'session'; session: Session; variant?: 'default' | 'no-path' };

// Legacy type for backward compatibility - to be removed
export type SessionListItem = string | Session;

interface StorageState {
    settings: Settings;
    settingsVersion: number | null;
    localSettings: LocalSettings;
    localProxyUsage: LocalProxyUsage;
    profile: Profile;
    sessions: Record<string, Session>;
    sessionsData: SessionListItem[] | null;  // Legacy - to be removed
    sessionListViewData: SessionListViewItem[] | null;
    sessionMessages: Record<string, SessionMessages>;
    sessionGitStatus: Record<string, GitStatus | null>;
    personaReadTimestamps: Record<string, number>;
    cachedPersonas: Persona[];
    cachedPersonasLoadedAt: number; // timestamp ms, 0 = never loaded
    cachedPersonaProjects: PersonaProject[];
    cachedAgentWorkspaces: WorkspaceSummary[];
    machines: Record<string, Machine>;
    /**
     * Per-machine vendor quota snapshot, keyed by `${machineId}:${vendor}`.
     * Populated by the 60s poll against the daemon's `quota-read` RPC.
     */
    vendorQuota: Record<string, import('./storageTypes').VendorQuota>;
    friends: Record<string, UserProfile>;  // All relationships (friends, pending, requested, etc.)
    users: Record<string, UserProfile | null>;  // Global user cache, null = 404/failed fetch
    feedItems: FeedItem[];  // Simple list of feed items
    feedHead: string | null;  // Newest cursor
    feedTail: string | null;  // Oldest cursor
    feedHasMore: boolean;
    feedLoaded: boolean;  // True after initial feed fetch
    friendsLoaded: boolean;  // True after initial friends fetch
    realtimeStatus: 'disconnected' | 'connecting' | 'connected' | 'error';
    realtimeMode: 'idle' | 'speaking';
    socketStatus: 'disconnected' | 'connecting' | 'connected' | 'error';
    socketLastConnectedAt: number | null;
    socketLastDisconnectedAt: number | null;
    isDataReady: boolean;
    nativeUpdateStatus: { available: boolean; updateUrl?: string } | null;
    showDesktopUpdateModal: boolean;
    desktopUpdateStatus: {
        available: boolean;
        version?: string;
        notes?: string;
        downloading?: boolean;
        progress?: number;
    } | null;
    todoState: TodoState | null;
    todosLoaded: boolean;
    applySessions: (sessions: (Omit<Session, 'presence'> & { presence?: "online" | number })[]) => void;
    bindPendingPersonaSession: (pendingSessionId: string, session: Omit<Session, 'presence'> & { presence?: "online" | number }) => void;
    applyMachines: (machines: Machine[], replace?: boolean) => void;
    applyVendorQuota: (
        machineId: string,
        entries: Record<string, import('./storageTypes').VendorQuota>,
    ) => void;
    applyLoaded: () => void;
    applyReady: () => void;
    upsertSessionMessages: (sessionId: string, messages: NormalizedMessage[]) => { changed: string[], hasReadyEvent: boolean };
    applySessionMessagesLocal: (sessionId: string, messages: NormalizedMessage[], hasMore: boolean) => { changed: string[], hasReadyEvent: boolean };
    applyTaskLifecycle: (sessionId: string, entry: SessionTaskLifecycleEntry) => void;
    updateMessagesPagination: (sessionId: string, update: { hasOlderMessages?: boolean; oldestSeq?: number; isLoadingOlder?: boolean }) => void;
    setSessionMessagesStatus: (sessionId: string, update: { isLoaded?: boolean; isSyncing?: boolean; relayError?: string | null }) => void;
    applySettings: (settings: Settings, version: number) => void;
    applySettingsLocal: (settings: Partial<Settings>) => void;
    applyLocalSettings: (settings: Partial<LocalSettings>) => void;
    applyLocalProxyUsage: (record: LocalProxyUsageRecord) => void;
    applyProfile: (profile: Profile) => void;
    applyTodos: (todoState: TodoState) => void;
    applyGitStatus: (sessionId: string, status: GitStatus | null) => void;
    applyNativeUpdateStatus: (status: { available: boolean; updateUrl?: string } | null) => void;
    applyDesktopUpdateStatus: (status: StorageState['desktopUpdateStatus']) => void;
    setShowDesktopUpdateModal: (show: boolean) => void;
    isMutableToolCall: (sessionId: string, callId: string) => boolean;
    setRealtimeStatus: (status: 'disconnected' | 'connecting' | 'connected' | 'error') => void;
    setRealtimeMode: (mode: 'idle' | 'speaking', immediate?: boolean) => void;
    setSocketStatus: (status: 'disconnected' | 'connecting' | 'connected' | 'error') => void;
    getActiveSessions: () => Session[];
    updateSessionDraft: (sessionId: string, draft: string | null) => void;
    updateSessionPermissionMode: (sessionId: string, mode: 'default' | 'auto' | 'acceptEdits' | 'plan' | 'dontAsk' | 'bypassPermissions' | 'read-only' | 'safe-yolo' | 'yolo') => void;
    updateSessionRuntimeEffort: (sessionId: string, effort: import('./storageTypes').RuntimeEffort) => void;
    updateSessionSandboxPolicy: (sessionId: string, policy: 'workspace_write' | 'unrestricted') => void;
    updateSessionModelMode: (sessionId: string, mode: 'default' | 'gemini-2.5-pro' | 'gemini-2.5-flash' | 'gemini-2.5-flash-lite') => void;
    deleteSession: (sessionId: string) => void;
    deleteMachine: (machineId: string) => void;
    markPersonaRead: (chatSessionId: string) => void;
    applyPersonas: (personas: Persona[]) => void;
    upsertPersonaProject: (project: Pick<PersonaProject, 'machineId' | 'workdir'>) => void;
    deletePersonaProject: (machineId: string, workdir: string) => void;
    applyAgentWorkspaces: (workspaces: WorkspaceSummary[]) => void;
    // Project management methods
    getProjects: () => import('./projectManager').Project[];
    getProject: (projectId: string) => import('./projectManager').Project | null;
    getProjectForSession: (sessionId: string) => import('./projectManager').Project | null;
    getProjectSessions: (projectId: string) => string[];
    // Project git status methods
    getProjectGitStatus: (projectId: string) => import('./storageTypes').GitStatus | null;
    getSessionProjectGitStatus: (sessionId: string) => import('./storageTypes').GitStatus | null;
    updateSessionProjectGitStatus: (sessionId: string, status: import('./storageTypes').GitStatus | null) => void;
    // Friend management methods
    applyFriends: (friends: UserProfile[]) => void;
    applyRelationshipUpdate: (event: RelationshipUpdatedEvent) => void;
    getFriend: (userId: string) => UserProfile | undefined;
    getAcceptedFriends: () => UserProfile[];
    // User cache methods
    applyUsers: (users: Record<string, UserProfile | null>) => void;
    getUser: (userId: string) => UserProfile | null | undefined;
    assumeUsers: (userIds: string[]) => Promise<void>;
    // Feed methods
    applyFeedItems: (items: FeedItem[]) => void;
    clearFeed: () => void;
}

// Helper function to build unified list view data from sessions and machines
function buildSessionListViewData(
    sessions: Record<string, Session>,
    hiddenSessionIds: Set<string> = new Set(),
): SessionListViewItem[] {
    // Separate active and inactive sessions
    const activeSessions: Session[] = [];
    const inactiveSessions: Session[] = [];

    Object.values(sessions).forEach(session => {
        if (hiddenSessionIds.has(session.id)) return;
        // Hide persona chat sessions from the main task list
        if (session.metadata?.flavor === 'persona') return;

        if (isSessionActive(session)) {
            activeSessions.push(session);
        } else {
            inactiveSessions.push(session);
        }
    });

    // Sort sessions by updated date (newest first)
    activeSessions.sort((a, b) => b.updatedAt - a.updatedAt);
    inactiveSessions.sort((a, b) => b.updatedAt - a.updatedAt);

    // Build unified list view data
    const listData: SessionListViewItem[] = [];

    // Add active sessions as a single item at the top (if any)
    if (activeSessions.length > 0) {
        listData.push({ type: 'active-sessions', sessions: activeSessions });
    }

    // Group inactive sessions by date
    const now = new Date();
    const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
    const yesterday = new Date(today.getTime() - 24 * 60 * 60 * 1000);

    let currentDateGroup: Session[] = [];
    let currentDateString: string | null = null;

    for (const session of inactiveSessions) {
        const sessionDate = new Date(session.updatedAt);
        const dateString = sessionDate.toDateString();

        if (currentDateString !== dateString) {
            // Process previous group
            if (currentDateGroup.length > 0 && currentDateString) {
                const groupDate = new Date(currentDateString);
                const sessionDateOnly = new Date(groupDate.getFullYear(), groupDate.getMonth(), groupDate.getDate());

                let headerTitle: string;
                if (sessionDateOnly.getTime() === today.getTime()) {
                    headerTitle = 'Today';
                } else if (sessionDateOnly.getTime() === yesterday.getTime()) {
                    headerTitle = 'Yesterday';
                } else {
                    const diffTime = today.getTime() - sessionDateOnly.getTime();
                    const diffDays = Math.floor(diffTime / (1000 * 60 * 60 * 24));
                    headerTitle = `${diffDays} days ago`;
                }

                listData.push({ type: 'header', title: headerTitle });
                currentDateGroup.forEach(sess => {
                    listData.push({ type: 'session', session: sess });
                });
            }

            // Start new group
            currentDateString = dateString;
            currentDateGroup = [session];
        } else {
            currentDateGroup.push(session);
        }
    }

    // Process final group
    if (currentDateGroup.length > 0 && currentDateString) {
        const groupDate = new Date(currentDateString);
        const sessionDateOnly = new Date(groupDate.getFullYear(), groupDate.getMonth(), groupDate.getDate());

        let headerTitle: string;
        if (sessionDateOnly.getTime() === today.getTime()) {
            headerTitle = 'Today';
        } else if (sessionDateOnly.getTime() === yesterday.getTime()) {
            headerTitle = 'Yesterday';
        } else {
            const diffTime = today.getTime() - sessionDateOnly.getTime();
            const diffDays = Math.floor(diffTime / (1000 * 60 * 60 * 24));
            headerTitle = `${diffDays} days ago`;
        }

        listData.push({ type: 'header', title: headerTitle });
        currentDateGroup.forEach(sess => {
            listData.push({ type: 'session', session: sess });
        });
    }

    return listData;
}

export const storage = create<StorageState>()((set, get) => {
    let { settings, version } = loadSettings();
    let localSettings = loadLocalSettings();
    let localProxyUsage = loadLocalProxyUsage();
    let profile = loadProfile();
    let sessionDrafts = loadSessionDrafts();
    let sessionPermissionModes = loadSessionPermissionModes();
    let sessionRuntimeEfforts = loadSessionRuntimeEfforts();
    let sessionSandboxPolicies = loadSessionSandboxPolicies();
    let personaReadTimestamps = loadPersonaReadTimestamps();
    return {
        settings,
        settingsVersion: version,
        localSettings,
        localProxyUsage,
        profile,
        sessions: {},
        machines: {},
        vendorQuota: {},
        friends: {},  // Initialize relationships cache
        users: {},  // Initialize global user cache
        feedItems: [],  // Initialize feed items list
        feedHead: null,
        feedTail: null,
        feedHasMore: false,
        feedLoaded: false,  // Initialize as false
        friendsLoaded: false,  // Initialize as false
        todoState: null,  // Initialize todo state
        todosLoaded: false,  // Initialize todos loaded state
        sessionsData: null,  // Legacy - to be removed
        sessionListViewData: null,
        sessionMessages: {},
        sessionGitStatus: {},
        personaReadTimestamps,
        cachedPersonas: loadCachedPersonas(),
        cachedPersonasLoadedAt: 0,
        cachedPersonaProjects: loadCachedPersonaProjects(),
        cachedAgentWorkspaces: loadCachedAgentWorkspaces(),
        realtimeStatus: 'disconnected',
        realtimeMode: 'idle',
        socketStatus: 'disconnected',
        socketLastConnectedAt: null,
        socketLastDisconnectedAt: null,
        isDataReady: false,
        nativeUpdateStatus: null,
        showDesktopUpdateModal: false,
        desktopUpdateStatus: null,
        isMutableToolCall: (sessionId: string, callId: string) => {
            const sessionMessages = get().sessionMessages[sessionId];
            if (!sessionMessages) {
                return true;
            }
            const toolCall = sessionMessages.reducerState.toolIdToMessageId.get(callId);
            if (!toolCall) {
                return true;
            }
            const toolCallMessage = sessionMessages.messagesMap[toolCall];
            if (!toolCallMessage || toolCallMessage.kind !== 'tool-call') {
                return true;
            }
            return toolCallMessage.tool?.name ? isMutableTool(toolCallMessage.tool?.name) : true;
        },
        getActiveSessions: () => {
            const state = get();
            return Object.values(state.sessions).filter(s => s.active);
        },
        applySessions: (sessions: (Omit<Session, 'presence'> & { presence?: "online" | number })[]) => set((state) => {
            // Load drafts and permission modes if sessions are empty (initial load)
            const savedDrafts = Object.keys(state.sessions).length === 0 ? sessionDrafts : {};
            const savedPermissionModes = Object.keys(state.sessions).length === 0 ? sessionPermissionModes : {};
            const savedRuntimeEfforts = Object.keys(state.sessions).length === 0 ? sessionRuntimeEfforts : {};
            const savedSandboxPolicies = Object.keys(state.sessions).length === 0 ? sessionSandboxPolicies : {};

            // Merge new sessions with existing ones
            const mergedSessions: Record<string, Session> = { ...state.sessions };

            // Update sessions with calculated presence using centralized resolver
            sessions.forEach(session => {
                // Use centralized resolver for consistent state management
                const presence = resolveSessionOnlineState(session);

                // Preserve existing draft and permission mode if they exist, or load from saved data
                const existingDraft = state.sessions[session.id]?.draft;
                const savedDraft = savedDrafts[session.id];
                const existingPermissionMode = state.sessions[session.id]?.permissionMode;
                const savedPermissionMode = savedPermissionModes[session.id];
                const existingRuntimeEffort = state.sessions[session.id]?.runtimeEffort;
                const savedRuntimeEffort = savedRuntimeEfforts[session.id];
                const existingSandboxPolicy = state.sessions[session.id]?.sandboxPolicy;
                const savedSandboxPolicy = savedSandboxPolicies[session.id];
                const existingPromptSuggestions = state.sessions[session.id]?.promptSuggestions;
                mergedSessions[session.id] = {
                    ...session,
                    presence,
                    draft: existingDraft || savedDraft || session.draft || null,
                    permissionMode: existingPermissionMode || savedPermissionMode || session.permissionMode || 'default',
                    runtimeEffort: existingRuntimeEffort || savedRuntimeEffort || session.runtimeEffort || 'default',
                    sandboxPolicy: (existingSandboxPolicy || savedSandboxPolicy || session.sandboxPolicy || 'workspace_write') as any,
                    promptSuggestions: session.promptSuggestions ?? existingPromptSuggestions,
                };
            });

            // Build active set from all sessions (including existing ones)
            const activeSet = new Set<string>();
            Object.values(mergedSessions).forEach(session => {
                if (isSessionActive(session)) {
                    activeSet.add(session.id);
                }
            });

            // Separate active and inactive sessions
            const activeSessions: Session[] = [];
            const inactiveSessions: Session[] = [];

            // Process all sessions from merged set
            Object.values(mergedSessions).forEach(session => {
                if (activeSet.has(session.id)) {
                    activeSessions.push(session);
                } else {
                    inactiveSessions.push(session);
                }
            });

            // Sort both arrays by creation date for stable ordering
            activeSessions.sort((a, b) => b.createdAt - a.createdAt);
            inactiveSessions.sort((a, b) => b.createdAt - a.createdAt);

            // Build flat list data for FlashList
            const listData: SessionListItem[] = [];

            if (activeSessions.length > 0) {
                listData.push('online');
                listData.push(...activeSessions);
            }

            // Legacy sessionsData - to be removed
            // Machines are now integrated into sessionListViewData

            if (inactiveSessions.length > 0) {
                listData.push('offline');
                listData.push(...inactiveSessions);
            }

            // console.log(`📊 Storage: applySessions called with ${sessions.length} sessions, active: ${activeSessions.length}, inactive: ${inactiveSessions.length}`);

            // Process AgentState updates for sessions that already have messages loaded
            const updatedSessionMessages = { ...state.sessionMessages };

            sessions.forEach(session => {
                const oldSession = state.sessions[session.id];
                const newSession = mergedSessions[session.id];

                // Check if sessionMessages exists AND agentStateVersion is newer than last reducer-processed version
                const existingSessionMessages = updatedSessionMessages[session.id];
                const lastReducerVersion = existingSessionMessages?.lastReducerAgentStateVersion || 0;
                const versionCheck = existingSessionMessages && newSession.agentState &&
                    (newSession.agentStateVersion > lastReducerVersion);
                if (newSession.agentState) {
                    debugLog(`applySessions: sid=${session.id.slice(-8)}, hasMsgs=${!!existingSessionMessages}, newV=${newSession.agentStateVersion}, reducerV=${lastReducerVersion}, pass=${versionCheck}`);
                }
                if (versionCheck) {

                    // Check for NEW permission requests before processing
                    const currentRealtimeSessionId = getCurrentRealtimeSessionId();
                    const voiceSession = getVoiceSession();

                    // console.log('[REALTIME DEBUG] Permission check:', {
                    //     currentRealtimeSessionId,
                    //     sessionId: session.id,
                    //     match: currentRealtimeSessionId === session.id,
                    //     hasVoiceSession: !!voiceSession,
                    //     oldRequests: Object.keys(oldSession?.agentState?.requests || {}),
                    //     newRequests: Object.keys(newSession.agentState?.requests || {})
                    // });

                    if (currentRealtimeSessionId === session.id && voiceSession) {
                        const oldRequests = oldSession?.agentState?.requests || {};
                        const newRequests = newSession.agentState?.requests || {};

                        // Find NEW permission requests only
                        for (const [requestId, request] of Object.entries(newRequests)) {
                            if (!oldRequests[requestId]) {
                                // This is a NEW permission request
                                const toolName = request.tool;
                                // console.log('[REALTIME DEBUG] Sending permission notification for:', toolName);
                                voiceSession.sendTextMessage?.(
                                    `Claude is requesting permission to use the ${toolName} tool`
                                );
                            }
                        }
                    }

                    // Process new AgentState through reducer
                    debugLog(`reducer: v=${newSession.agentStateVersion} reqs=${JSON.stringify(Object.keys(newSession.agentState?.requests || {}))} comp=${JSON.stringify(Object.keys(newSession.agentState?.completedRequests || {}))}`);
                    const reducerResult = reducer(existingSessionMessages.reducerState, [], newSession.agentState);
                    const processedMessages = reducerResult.messages;
                    debugLog(`reducer result: ${processedMessages.length} changed msgs, kinds=${processedMessages.map(m => m.kind + (m.kind === 'tool-call' ? ':' + (m as any).tool?.permission?.status : '')).join(',')}`);

                    // Always update the session messages, even if no new messages were created
                    // This ensures the reducer state is updated with the new AgentState
                    const mergedMessagesMap = { ...existingSessionMessages.messagesMap };
                    processedMessages.forEach(message => {
                        mergedMessagesMap[message.id] = message;
                    });

                    const messagesArray = Object.values(mergedMessagesMap)
                        .sort((a, b) => b.createdAt - a.createdAt);

                    updatedSessionMessages[session.id] = {
                        ...existingSessionMessages,
                        messages: messagesArray,
                        messagesMap: mergedMessagesMap,
                        reducerState: existingSessionMessages.reducerState,
                        isLoaded: existingSessionMessages.isLoaded,
                        lastReducerAgentStateVersion: newSession.agentStateVersion,
                    };

                    // IMPORTANT: Copy latestUsage from reducerState to Session for immediate availability
                    if (existingSessionMessages.reducerState.latestUsage) {
                        mergedSessions[session.id] = {
                            ...mergedSessions[session.id],
                            latestUsage: { ...existingSessionMessages.reducerState.latestUsage },
                            contextTokens:
                                mergedSessions[session.id].contextTokens ??
                                existingSessionMessages.reducerState.latestUsage.contextSize,
                        };
                    }
                }
            });

            // Build new unified list view data
            const sessionListViewData = buildSessionListViewData(
                mergedSessions,
                buildHiddenWorkspaceMemberSessionIds(state.cachedAgentWorkspaces)
            );

            // Update project manager with current sessions and machines
            const machineMetadataMap = new Map<string, any>();
            Object.values(state.machines).forEach(machine => {
                if (machine.metadata) {
                    machineMetadataMap.set(machine.id, machine.metadata);
                }
            });
            projectManager.updateSessions(Object.values(mergedSessions), machineMetadataMap);

            return {
                ...state,
                sessions: mergedSessions,
                sessionsData: listData,  // Legacy - to be removed
                sessionListViewData,
                sessionMessages: updatedSessionMessages
            };
        }),
        bindPendingPersonaSession: (pendingSessionId: string, session: Omit<Session, 'presence'> & { presence?: "online" | number }) => set((state) => {
            if (!pendingSessionId || pendingSessionId === session.id) {
                return state;
            }

            const pendingSession = state.sessions[pendingSessionId];
            const existingRealSession = state.sessions[session.id];
            const presence = resolveSessionOnlineState(session);
            const realSession: Session = {
                ...session,
                presence,
                draft: pendingSession?.draft || existingRealSession?.draft || session.draft || null,
                permissionMode: pendingSession?.permissionMode || existingRealSession?.permissionMode || session.permissionMode || 'default',
                runtimeEffort: pendingSession?.runtimeEffort || existingRealSession?.runtimeEffort || session.runtimeEffort || 'default',
                sandboxPolicy: (pendingSession?.sandboxPolicy || existingRealSession?.sandboxPolicy || session.sandboxPolicy || 'workspace_write') as any,
                modelMode: pendingSession?.modelMode || existingRealSession?.modelMode || session.modelMode || null,
                promptSuggestions: session.promptSuggestions ?? existingRealSession?.promptSuggestions ?? pendingSession?.promptSuggestions,
            };

            const { [pendingSessionId]: _pendingSession, ...sessionsWithoutPending } = state.sessions;
            const mergedSessions: Record<string, Session> = {
                ...sessionsWithoutPending,
                [session.id]: realSession,
            };

            const pendingMessages = state.sessionMessages[pendingSessionId];
            const realMessages = state.sessionMessages[session.id];
            const { [pendingSessionId]: _pendingMessages, ...messagesWithoutPending } = state.sessionMessages;
            let mergedMessages = messagesWithoutPending;
            if (pendingMessages || realMessages) {
                const mergedMessagesMap = {
                    ...(realMessages?.messagesMap ?? {}),
                    ...(pendingMessages?.messagesMap ?? {}),
                };
                mergedMessages = {
                    ...messagesWithoutPending,
                    [session.id]: {
                        ...(realMessages ?? pendingMessages ?? createEmptySessionMessages()),
                        messages: Object.values(mergedMessagesMap).sort((a, b) => a.createdAt - b.createdAt),
                        messagesMap: mergedMessagesMap,
                        taskLifecycle: {
                            ...(realMessages?.taskLifecycle ?? {}),
                            ...(pendingMessages?.taskLifecycle ?? {}),
                        },
                        reducerState: realMessages?.reducerState ?? pendingMessages?.reducerState ?? createReducer(),
                        isLoaded: realMessages?.isLoaded ?? pendingMessages?.isLoaded ?? true,
                        hasOlderMessages: realMessages?.hasOlderMessages ?? pendingMessages?.hasOlderMessages ?? false,
                        oldestSeq: realMessages?.oldestSeq ?? pendingMessages?.oldestSeq ?? 0,
                        isLoadingOlder: false,
                        isSyncing: realMessages?.isSyncing ?? false,
                        relayError: realMessages?.relayError ?? pendingMessages?.relayError ?? null,
                        lastReducerAgentStateVersion: Math.max(
                            realMessages?.lastReducerAgentStateVersion ?? 0,
                            pendingMessages?.lastReducerAgentStateVersion ?? 0,
                        ),
                    },
                };
            }

            const { [pendingSessionId]: pendingGitStatus, ...gitStatusWithoutPending } = state.sessionGitStatus;
            const sessionGitStatus = pendingGitStatus !== undefined
                ? { ...gitStatusWithoutPending, [session.id]: state.sessionGitStatus[session.id] ?? pendingGitStatus }
                : gitStatusWithoutPending;

            const drafts = loadSessionDrafts();
            if (drafts[pendingSessionId] && !drafts[session.id]) {
                drafts[session.id] = drafts[pendingSessionId];
            }
            delete drafts[pendingSessionId];
            saveSessionDrafts(drafts);

            const modes = loadSessionPermissionModes();
            if (modes[pendingSessionId] && !modes[session.id]) {
                modes[session.id] = modes[pendingSessionId];
            }
            delete modes[pendingSessionId];
            saveSessionPermissionModes(modes);

            const efforts = loadSessionRuntimeEfforts();
            if (efforts[pendingSessionId] && !efforts[session.id]) {
                efforts[session.id] = efforts[pendingSessionId];
            }
            delete efforts[pendingSessionId];
            saveSessionRuntimeEfforts(efforts);

            const policies = loadSessionSandboxPolicies();
            if (policies[pendingSessionId] && !policies[session.id]) {
                policies[session.id] = policies[pendingSessionId];
            }
            delete policies[pendingSessionId];
            saveSessionSandboxPolicies(policies);

            const personaReadTimestamps = { ...state.personaReadTimestamps };
            if (personaReadTimestamps[pendingSessionId] && !personaReadTimestamps[session.id]) {
                personaReadTimestamps[session.id] = personaReadTimestamps[pendingSessionId];
            }
            delete personaReadTimestamps[pendingSessionId];
            savePersonaReadTimestamps(personaReadTimestamps);

            const sessionListViewData = buildSessionListViewData(
                mergedSessions,
                buildHiddenWorkspaceMemberSessionIds(state.cachedAgentWorkspaces)
            );

            const machineMetadataMap = new Map<string, any>();
            Object.values(state.machines).forEach(machine => {
                if (machine.metadata) {
                    machineMetadataMap.set(machine.id, machine.metadata);
                }
            });
            projectManager.updateSessions(Object.values(mergedSessions), machineMetadataMap);

            return {
                ...state,
                sessions: mergedSessions,
                sessionMessages: mergedMessages,
                sessionGitStatus,
                personaReadTimestamps,
                sessionListViewData,
            };
        }),
        applyLoaded: () => set((state) => {
            const result = {
                ...state,
                sessionsData: []
            };
            return result;
        }),
        applyReady: () => set((state) => ({
            ...state,
            isDataReady: true
        })),
        applyVendorQuota: (
            machineId: string,
            entries: Record<string, import('./storageTypes').VendorQuota>,
        ) => set((state) => {
            const next = { ...state.vendorQuota };
            // Replace every entry for this machine. The daemon always returns
            // its full known set in one shot, so we can drop stale vendors
            // (e.g. user logged out of Gemini) in the same pass.
            for (const key of Object.keys(next)) {
                if (key.startsWith(`${machineId}:`)) {
                    delete next[key];
                }
            }
            for (const [vendor, usage] of Object.entries(entries)) {
                next[`${machineId}:${vendor}`] = usage;
            }
            return { ...state, vendorQuota: next };
        }),
        upsertSessionMessages: (sessionId: string, messages: NormalizedMessage[]) => {
            let changed = new Set<string>();
            let hasReadyEvent = false;
            set((state) => {

                // Resolve session messages state
                const existingSession = state.sessionMessages[sessionId] || createEmptySessionMessages();

                // Get the session's agentState if available
                const session = state.sessions[sessionId];
                const agentState = session?.agentState;

                // Messages are already normalized, no need to process them again
                const normalizedMessages = messages;

                // Run reducer with agentState
                const reducerResult = reducer(existingSession.reducerState, normalizedMessages, agentState);
                const processedMessages = reducerResult.messages;
                for (let message of processedMessages) {
                    changed.add(message.id);
                }
                if (reducerResult.hasReadyEvent) {
                    hasReadyEvent = true;
                }

                // Merge messages
                const mergedMessagesMap = { ...existingSession.messagesMap };
                processedMessages.forEach(message => {
                    mergedMessagesMap[message.id] = message;
                });

                // Render oldest-to-newest so prepending older pages keeps chronology stable.
                const messagesArray = Object.values(mergedMessagesMap)
                    .sort((a, b) => a.createdAt - b.createdAt);

                // Update session with todos and latestUsage
                // IMPORTANT: We extract latestUsage from the mutable reducerState and copy it to the Session object
                // This ensures latestUsage is available immediately on load, even before messages are fully loaded
                let updatedSessions = state.sessions;
                const needsUpdate = (reducerResult.todos !== undefined || existingSession.reducerState.latestUsage) && session;

                if (needsUpdate) {
                    const latestUsage = existingSession.reducerState.latestUsage
                        ? { ...existingSession.reducerState.latestUsage }
                        : session.latestUsage;
                    updatedSessions = {
                        ...state.sessions,
                        [sessionId]: {
                            ...session,
                            ...(reducerResult.todos !== undefined && { todos: reducerResult.todos }),
                            // Copy latestUsage from reducerState to make it immediately available
                            latestUsage,
                            contextTokens: session.contextTokens ?? latestUsage?.contextSize,
                        }
                    };
                }

                return {
                    ...state,
                    sessions: updatedSessions,
                    sessionMessages: {
                        ...state.sessionMessages,
                        [sessionId]: {
                            ...existingSession,
                            messages: messagesArray,
                            messagesMap: mergedMessagesMap,
                            taskLifecycle: existingSession.taskLifecycle,
                            reducerState: existingSession.reducerState, // Explicitly include the mutated reducer state
                            isLoaded: true,
                            hasOlderMessages: existingSession.hasOlderMessages ?? false,
                            lastReducerAgentStateVersion: session?.agentStateVersion || existingSession.lastReducerAgentStateVersion || 0,
                            isSyncing: false,
                            relayError: existingSession.relayError ?? null,
                        }
                    }
                };
            });

            return { changed: Array.from(changed), hasReadyEvent };
        },
        applySessionMessagesLocal: (sessionId: string, messages: NormalizedMessage[], hasMore: boolean) => {
            const result = get().upsertSessionMessages(sessionId, messages);
            set((state) => {
                const existingSession = state.sessionMessages[sessionId] || createEmptySessionMessages();

                return {
                    ...state,
                    sessionMessages: {
                        ...state.sessionMessages,
                        [sessionId]: {
                            ...existingSession,
                            isLoaded: true,
                            hasOlderMessages: hasMore,
                            isLoadingOlder: false,
                            isSyncing: false,
                            relayError: null,
                        },
                    },
                };
            });
            return result;
        },
        applyTaskLifecycle: (sessionId: string, entry: SessionTaskLifecycleEntry) => set((state) => {
            const existingSession = state.sessionMessages[sessionId] || createEmptySessionMessages();
            const existingEntry = existingSession.taskLifecycle[entry.taskId];
            if (existingEntry && existingEntry.updatedAt > entry.updatedAt) {
                return state;
            }
            const mergedEntry: SessionTaskLifecycleEntry = existingEntry ? {
                ...existingEntry,
                ...entry,
                startedAt: entry.startedAt ?? existingEntry.startedAt ?? null,
                completedAt: entry.completedAt ?? existingEntry.completedAt ?? null,
                summary: entry.summary ?? existingEntry.summary ?? null,
                description: entry.description ?? existingEntry.description ?? null,
                taskType: entry.taskType ?? existingEntry.taskType ?? null,
            } : entry;
            return {
                ...state,
                sessionMessages: {
                    ...state.sessionMessages,
                    [sessionId]: {
                        ...existingSession,
                        taskLifecycle: {
                            ...existingSession.taskLifecycle,
                            [entry.taskId]: mergedEntry,
                        },
                    },
                },
            };
        }),
        updateMessagesPagination: (sessionId: string, update: { hasOlderMessages?: boolean; oldestSeq?: number; isLoadingOlder?: boolean }) => set((state) => {
            const existing = state.sessionMessages[sessionId] || createEmptySessionMessages();
            return {
                ...state,
                sessionMessages: {
                    ...state.sessionMessages,
                    [sessionId]: {
                        ...existing,
                        ...(update.hasOlderMessages !== undefined && { hasOlderMessages: update.hasOlderMessages }),
                        ...(update.oldestSeq !== undefined && { oldestSeq: update.oldestSeq }),
                        ...(update.isLoadingOlder !== undefined && { isLoadingOlder: update.isLoadingOlder }),
                    }
                }
            };
        }),
        setSessionMessagesStatus: (sessionId: string, update: { isLoaded?: boolean; isSyncing?: boolean; relayError?: string | null }) => set((state) => {
            const existing = state.sessionMessages[sessionId] || createEmptySessionMessages();
            return {
                ...state,
                sessionMessages: {
                    ...state.sessionMessages,
                    [sessionId]: {
                        ...existing,
                        ...(update.isLoaded !== undefined && { isLoaded: update.isLoaded }),
                        ...(update.isSyncing !== undefined && { isSyncing: update.isSyncing }),
                        ...(update.relayError !== undefined && { relayError: update.relayError }),
                    },
                },
            };
        }),
        applySettingsLocal: (settings: Partial<Settings>) => set((state) => {
            saveSettings(applySettings(state.settings, settings), state.settingsVersion ?? 0);
            return {
                ...state,
                settings: applySettings(state.settings, settings)
            };
        }),
        applySettings: (settings: Settings, version: number) => set((state) => {
            if (state.settingsVersion === null || state.settingsVersion < version) {
                saveSettings(settings, version);
                return {
                    ...state,
                    settings,
                    settingsVersion: version
                };
            } else {
                return state;
            }
        }),
        applyLocalSettings: (delta: Partial<LocalSettings>) => set((state) => {
            const updatedLocalSettings = applyLocalSettings(state.localSettings, delta);
            saveLocalSettings(updatedLocalSettings);
            return {
                ...state,
                localSettings: updatedLocalSettings
            };
        }),
        applyLocalProxyUsage: (record: LocalProxyUsageRecord) => set((state) => {
            const localProxyUsage = upsertLocalProxyUsageRecord(state.localProxyUsage, record);
            saveLocalProxyUsage(localProxyUsage);
            return {
                ...state,
                localProxyUsage,
            };
        }),
        applyProfile: (profile: Profile) => set((state) => {
            // Always save and update profile
            saveProfile(profile);
            return {
                ...state,
                profile
            };
        }),
        applyTodos: (todoState: TodoState) => set((state) => {
            return {
                ...state,
                todoState,
                todosLoaded: true
            };
        }),
        applyGitStatus: (sessionId: string, status: GitStatus | null) => set((state) => {
            // Update project git status as well
            projectManager.updateSessionProjectGitStatus(sessionId, status);

            return {
                ...state,
                sessionGitStatus: {
                    ...state.sessionGitStatus,
                    [sessionId]: status
                }
            };
        }),
        applyNativeUpdateStatus: (status: { available: boolean; updateUrl?: string } | null) => set((state) => ({
            ...state,
            nativeUpdateStatus: status
        })),
        applyDesktopUpdateStatus: (status) => set((state) => ({
            ...state,
            desktopUpdateStatus: status
        })),
        setShowDesktopUpdateModal: (show) => set((state) => ({
            ...state,
            showDesktopUpdateModal: show
        })),
        setRealtimeStatus: (status: 'disconnected' | 'connecting' | 'connected' | 'error') => set((state) => ({
            ...state,
            realtimeStatus: status
        })),
        setRealtimeMode: (mode: 'idle' | 'speaking') => set((state) => ({
            ...state,
            realtimeMode: mode
        })),
        setSocketStatus: (status: 'disconnected' | 'connecting' | 'connected' | 'error') => set((state) => {
            const now = Date.now();
            const updates: Partial<StorageState> = {
                socketStatus: status
            };

            // Update timestamp based on status
            if (status === 'connected') {
                updates.socketLastConnectedAt = now;
            } else if (status === 'disconnected' || status === 'error') {
                updates.socketLastDisconnectedAt = now;
            }

            return {
                ...state,
                ...updates
            };
        }),
        updateSessionDraft: (sessionId: string, draft: string | null) => set((state) => {
            const session = state.sessions[sessionId];
            if (!session) return state;

            // Don't store empty strings, convert to null
            const normalizedDraft = draft?.trim() ? draft : null;

            // Collect all drafts for persistence
            const allDrafts: Record<string, string> = {};
            Object.entries(state.sessions).forEach(([id, sess]) => {
                if (id === sessionId) {
                    if (normalizedDraft) {
                        allDrafts[id] = normalizedDraft;
                    }
                } else if (sess.draft) {
                    allDrafts[id] = sess.draft;
                }
            });

            // Persist drafts
            saveSessionDrafts(allDrafts);

            const updatedSessions = {
                ...state.sessions,
                [sessionId]: {
                    ...session,
                    draft: normalizedDraft
                }
            };

            // Rebuild sessionListViewData to update the UI immediately
            const sessionListViewData = buildSessionListViewData(
                updatedSessions,
                buildHiddenWorkspaceMemberSessionIds(state.cachedAgentWorkspaces)
            );

            return {
                ...state,
                sessions: updatedSessions,
                sessionListViewData
            };
        }),
        updateSessionPermissionMode: (sessionId: string, mode: 'default' | 'auto' | 'acceptEdits' | 'plan' | 'dontAsk' | 'bypassPermissions' | 'read-only' | 'safe-yolo' | 'yolo') => set((state) => {
            const session = state.sessions[sessionId];
            if (!session) return state;

            // Update the session with the new permission mode
            const updatedSessions = {
                ...state.sessions,
                [sessionId]: {
                    ...session,
                    permissionMode: mode
                }
            };

            // Collect all permission modes for persistence
            const allModes: Record<string, PermissionMode> = {};
            Object.entries(updatedSessions).forEach(([id, sess]) => {
                if (sess.permissionMode && sess.permissionMode !== 'default') {
                    allModes[id] = sess.permissionMode;
                }
            });

            // Persist permission modes (only non-default values to save space)
            saveSessionPermissionModes(allModes);

            // No need to rebuild sessionListViewData since permission mode doesn't affect the list display
            return {
                ...state,
                sessions: updatedSessions
            };
        }),
        updateSessionRuntimeEffort: (sessionId: string, effort: import('./storageTypes').RuntimeEffort) => set((state) => {
            const session = state.sessions[sessionId];
            if (!session) return state;

            const updatedSessions = {
                ...state.sessions,
                [sessionId]: {
                    ...session,
                    runtimeEffort: effort
                }
            };

            const allEfforts: Record<string, import('./storageTypes').RuntimeEffort> = {};
            Object.entries(updatedSessions).forEach(([id, sess]) => {
                if (sess.runtimeEffort && sess.runtimeEffort !== 'default') {
                    allEfforts[id] = sess.runtimeEffort;
                }
            });

            saveSessionRuntimeEfforts(allEfforts);

            return {
                ...state,
                sessions: updatedSessions
            };
        }),
        updateSessionSandboxPolicy: (sessionId: string, policy: 'workspace_write' | 'unrestricted') => set((state) => {
            const session = state.sessions[sessionId];
            if (!session) return state;

            const updatedSessions = {
                ...state.sessions,
                [sessionId]: {
                    ...session,
                    sandboxPolicy: policy
                }
            };

            // Persist sandbox policies (only non-default values)
            const allPolicies: Record<string, string> = {};
            Object.entries(updatedSessions).forEach(([id, sess]) => {
                if (sess.sandboxPolicy && sess.sandboxPolicy !== 'workspace_write') {
                    allPolicies[id] = sess.sandboxPolicy;
                }
            });
            saveSessionSandboxPolicies(allPolicies);

            return {
                ...state,
                sessions: updatedSessions
            };
        }),
        updateSessionModelMode: (sessionId: string, mode: 'default' | 'gemini-2.5-pro' | 'gemini-2.5-flash' | 'gemini-2.5-flash-lite') => set((state) => {
            const session = state.sessions[sessionId];
            if (!session) return state;

            // Update the session with the new model mode
            const updatedSessions = {
                ...state.sessions,
                [sessionId]: {
                    ...session,
                    modelMode: mode
                }
            };

            // No need to rebuild sessionListViewData since model mode doesn't affect the list display
            return {
                ...state,
                sessions: updatedSessions
            };
        }),
        // Project management methods
        getProjects: () => projectManager.getProjects(),
        getProject: (projectId: string) => projectManager.getProject(projectId),
        getProjectForSession: (sessionId: string) => projectManager.getProjectForSession(sessionId),
        getProjectSessions: (projectId: string) => projectManager.getProjectSessions(projectId),
        // Project git status methods
        getProjectGitStatus: (projectId: string) => projectManager.getProjectGitStatus(projectId),
        getSessionProjectGitStatus: (sessionId: string) => projectManager.getSessionProjectGitStatus(sessionId),
        updateSessionProjectGitStatus: (sessionId: string, status: GitStatus | null) => {
            projectManager.updateSessionProjectGitStatus(sessionId, status);
            // Trigger a state update to notify hooks
            set((state) => ({ ...state }));
        },
        applyMachines: (machines: Machine[], replace: boolean = false) => set((state) => {
            console.log(`[Storage] applyMachines called: ${machines.length} machines, replace=${replace}`);
            // Either replace all machines or merge updates
            let mergedMachines: Record<string, Machine>;

            if (replace) {
                // Replace entire machine state (used by fetchMachines)
                console.log(`[Storage] Replacing all machines (clearing ${Object.keys(state.machines).length} existing)`);
                mergedMachines = {};
                machines.forEach(machine => {
                    console.log(`  Adding machine: ${machine.id.substring(0, 30)}... active=${machine.active}`);
                    mergedMachines[machine.id] = machine;
                });
            } else {
                // Merge individual updates (used by update-machine)
                mergedMachines = { ...state.machines };
                machines.forEach(machine => {
                    mergedMachines[machine.id] = machine;
                });
            }

            // Rebuild sessionListViewData to reflect machine changes
            const sessionListViewData = buildSessionListViewData(
                state.sessions,
                buildHiddenWorkspaceMemberSessionIds(state.cachedAgentWorkspaces)
            );

            return {
                ...state,
                machines: mergedMachines,
                sessionListViewData
            };
        }),
        deleteSession: (sessionId: string) => set((state) => {
            // Remove session from sessions
            const { [sessionId]: deletedSession, ...remainingSessions } = state.sessions;
            
            // Remove session messages if they exist
            const { [sessionId]: deletedMessages, ...remainingSessionMessages } = state.sessionMessages;
            
            // Remove session git status if it exists
            const { [sessionId]: deletedGitStatus, ...remainingGitStatus } = state.sessionGitStatus;

            // Clear drafts and permission modes from persistent storage
            const drafts = loadSessionDrafts();
            delete drafts[sessionId];
            saveSessionDrafts(drafts);
            
            const modes = loadSessionPermissionModes();
            delete modes[sessionId];
            saveSessionPermissionModes(modes);

            const efforts = loadSessionRuntimeEfforts();
            delete efforts[sessionId];
            saveSessionRuntimeEfforts(efforts);

            const policies = loadSessionSandboxPolicies();
            delete policies[sessionId];
            saveSessionSandboxPolicies(policies);

            // Rebuild sessionListViewData without the deleted session
            const sessionListViewData = buildSessionListViewData(
                remainingSessions,
                buildHiddenWorkspaceMemberSessionIds(state.cachedAgentWorkspaces)
            );
            
            return {
                ...state,
                sessions: remainingSessions,
                sessionMessages: remainingSessionMessages,
                sessionGitStatus: remainingGitStatus,
                sessionListViewData
            };
        }),
        deleteMachine: (machineId: string) => set((state) => {
            const { [machineId]: _, ...remainingMachines } = state.machines;
            return {
                ...state,
                machines: remainingMachines
            };
        }),
        markPersonaRead: (chatSessionId: string) => set((state) => {
            const updated = { ...state.personaReadTimestamps, [chatSessionId]: Date.now() };
            savePersonaReadTimestamps(updated);
            return { ...state, personaReadTimestamps: updated };
        }),
        applyPersonas: (personas: Persona[]) => {
            saveCachedPersonas(personas);
            return set((state) => {
                const prev = state.cachedPersonas;
                if (prev.length === personas.length) {
                    let equivalent = true;
                    for (let i = 0; i < personas.length; i++) {
                        const a = prev[i];
                        const b = personas[i];
                        if (
                            a.id !== b.id ||
                            a.name !== b.name ||
                            a.avatarId !== b.avatarId ||
                            a.modelId !== b.modelId ||
                            a.agent !== b.agent ||
                            a.workdir !== b.workdir ||
                            a.chatSessionId !== b.chatSessionId ||
                            a.continuousBrowsing !== b.continuousBrowsing ||
                            a.updatedAt !== b.updatedAt
                        ) { equivalent = false; break; }
                    }
                    if (equivalent) return state;
                }
                return {
                    ...state,
                    cachedPersonas: personas,
                    cachedPersonasLoadedAt: Date.now(),
                };
            });
        },
        upsertPersonaProject: (project: Pick<PersonaProject, 'machineId' | 'workdir'>) => set((state) => {
            const workdir = project.workdir.trim();
            if (!project.machineId || !workdir) return state;

            const now = Date.now();
            let found = false;
            const projects = state.cachedPersonaProjects.map((existing) => {
                if (existing.machineId === project.machineId && existing.workdir === workdir) {
                    found = true;
                    return { ...existing, updatedAt: now };
                }
                return existing;
            });

            if (!found) {
                projects.push({
                    machineId: project.machineId,
                    workdir,
                    createdAt: now,
                    updatedAt: now,
                });
            }

            saveCachedPersonaProjects(projects);
            return {
                ...state,
                cachedPersonaProjects: projects,
            };
        }),
        deletePersonaProject: (machineId: string, workdir: string) => set((state) => {
            const trimmed = workdir.trim();
            const projects = state.cachedPersonaProjects.filter(
                (project) => !(project.machineId === machineId && project.workdir === trimmed)
            );
            if (projects.length === state.cachedPersonaProjects.length) return state;
            saveCachedPersonaProjects(projects);
            return {
                ...state,
                cachedPersonaProjects: projects,
            };
        }),
        applyAgentWorkspaces: (workspaces: WorkspaceSummary[]) => set((state) => {
            saveCachedAgentWorkspaces(workspaces);
            const hiddenSessionIds = new Set(
                workspaces.flatMap((workspace) => workspace.members.map((member) => member.sessionId))
            );
            return {
                ...state,
                cachedAgentWorkspaces: workspaces,
                sessionListViewData: buildSessionListViewData(state.sessions, hiddenSessionIds),
            };
        }),
        // Friend management methods
        applyFriends: (friends: UserProfile[]) => set((state) => {
            const mergedFriends = { ...state.friends };
            friends.forEach(friend => {
                mergedFriends[friend.id] = friend;
            });
            return {
                ...state,
                friends: mergedFriends,
                friendsLoaded: true  // Mark as loaded after first fetch
            };
        }),
        applyRelationshipUpdate: (event: RelationshipUpdatedEvent) => set((state) => {
            const { fromUserId, toUserId, status, action, fromUser, toUser } = event;
            const currentUserId = state.profile.id;
            
            // Update friends cache
            const updatedFriends = { ...state.friends };
            
            // Determine which user profile to update based on perspective
            const otherUserId = fromUserId === currentUserId ? toUserId : fromUserId;
            const otherUser = fromUserId === currentUserId ? toUser : fromUser;
            
            if (action === 'deleted' || status === 'none') {
                // Remove from friends if deleted or status is none
                delete updatedFriends[otherUserId];
            } else if (otherUser) {
                // Update or add the user profile with current status
                updatedFriends[otherUserId] = otherUser;
            }
            
            return {
                ...state,
                friends: updatedFriends
            };
        }),
        getFriend: (userId: string) => {
            return get().friends[userId];
        },
        getAcceptedFriends: () => {
            const friends = get().friends;
            return Object.values(friends).filter(friend => friend.status === 'friend');
        },
        // User cache methods
        applyUsers: (users: Record<string, UserProfile | null>) => set((state) => ({
            ...state,
            users: { ...state.users, ...users }
        })),
        getUser: (userId: string) => {
            return get().users[userId];  // Returns UserProfile | null | undefined
        },
        assumeUsers: async (userIds: string[]) => {
            // This will be implemented in sync.ts as it needs access to credentials
            // Just a placeholder here for the interface
            const { sync } = await import('./sync');
            return sync.assumeUsers(userIds);
        },
        // Feed methods
        applyFeedItems: (items: FeedItem[]) => set((state) => {
            // Always mark feed as loaded even if empty
            if (items.length === 0) {
                return {
                    ...state,
                    feedLoaded: true  // Mark as loaded even when empty
                };
            }

            // Create a map of existing items for quick lookup
            const existingMap = new Map<string, FeedItem>();
            state.feedItems.forEach(item => {
                existingMap.set(item.id, item);
            });

            // Process new items
            const updatedItems = [...state.feedItems];
            let head = state.feedHead;
            let tail = state.feedTail;

            items.forEach(newItem => {
                // Remove items with same repeatKey if it exists
                if (newItem.repeatKey) {
                    const indexToRemove = updatedItems.findIndex(item =>
                        item.repeatKey === newItem.repeatKey
                    );
                    if (indexToRemove !== -1) {
                        updatedItems.splice(indexToRemove, 1);
                    }
                }

                // Add new item if it doesn't exist
                if (!existingMap.has(newItem.id)) {
                    updatedItems.push(newItem);
                }

                // Update head/tail cursors
                if (!head || newItem.counter > parseInt(head.substring(2), 10)) {
                    head = newItem.cursor;
                }
                if (!tail || newItem.counter < parseInt(tail.substring(2), 10)) {
                    tail = newItem.cursor;
                }
            });

            // Sort by counter (desc - newest first)
            updatedItems.sort((a, b) => b.counter - a.counter);

            return {
                ...state,
                feedItems: updatedItems,
                feedHead: head,
                feedTail: tail,
                feedLoaded: true  // Mark as loaded after first fetch
            };
        }),
        clearFeed: () => set((state) => ({
            ...state,
            feedItems: [],
            feedHead: null,
            feedTail: null,
            feedHasMore: false,
            feedLoaded: false,  // Reset loading flag
            friendsLoaded: false  // Reset loading flag
        })),
    }
});

export function useSessions() {
    return storage(useShallow((state) => state.isDataReady ? state.sessionsData : null));
}

export function useSession(id: string): Session | null {
    return storage(useShallow((state) => state.sessions[id] ?? null));
}

const emptyArray: unknown[] = [];
const emptyStringArray: string[] = [];

export function useSessionMessages(sessionId: string): {
    messages: Message[];
    isLoaded: boolean;
    hasOlderMessages: boolean;
    isLoadingOlder: boolean;
    isSyncing: boolean;
    relayError: string | null;
} {
    return storage(useShallow((state) => {
        const session = state.sessionMessages[sessionId];
        const hasSessionId = sessionId.length > 0;
        return {
            messages: session?.messages ?? emptyArray,
            isLoaded: session?.isLoaded ?? !hasSessionId,
            hasOlderMessages: session?.hasOlderMessages ?? false,
            isLoadingOlder: session?.isLoadingOlder ?? false,
            isSyncing: session?.isSyncing ?? false,
            relayError: session?.relayError ?? null,
        };
    }));
}

export function useSessionTaskLifecycle(sessionId: string): Record<string, SessionTaskLifecycleEntry> {
    return storage(useShallow((state) => state.sessionMessages[sessionId]?.taskLifecycle ?? {}));
}


export function useMessage(sessionId: string, messageId: string): Message | null {
    return storage(useShallow((state) => {
        const session = state.sessionMessages[sessionId];
        return session?.messagesMap[messageId] ?? null;
    }));
}

export function useSessionUsage(sessionId: string) {
    return storage(useShallow((state) => {
        const session = state.sessionMessages[sessionId];
        return session?.reducerState?.latestUsage ?? null;
    }));
}

export function useSessionTodos(sessionId: string) {
    return storage(useShallow((state) => {
        const sessionMessages = state.sessionMessages[sessionId];
        return sessionMessages?.reducerState?.latestTodos?.todos
            ?? latestTodosFromStoredMessages(sessionMessages?.messages ?? [])
            ?? state.sessions[sessionId]?.todos
            ?? null;
    }));
}

export function useSettings(): Settings {
    return storage(useShallow((state) => state.settings));
}

export function useSettingMutable<K extends keyof Settings>(name: K): [Settings[K], (value: Settings[K]) => void] {
    const setValue = React.useCallback((value: Settings[K]) => {
        sync.applySettings({ [name]: value });
    }, [name]);
    const value = useSetting(name);
    return [value, setValue];
}

export function useSetting<K extends keyof Settings>(name: K): Settings[K] {
    return storage(useShallow((state) => state.settings[name]));
}

export function useLocalSettings(): LocalSettings {
    return storage(useShallow((state) => state.localSettings));
}

export function useAllMachines(): Machine[] {
    return storage(useShallow((state) => {
        if (!state.isDataReady) return [];
        // Sort: online machines first, then by createdAt (stable order)
        // Using activeAt causes unstable ordering because heartbeats update it every 20s
        return Object.values(state.machines).sort((a, b) => {
            const aOnline = isMachineOnline(a) ? 1 : 0;
            const bOnline = isMachineOnline(b) ? 1 : 0;
            if (aOnline !== bOnline) return bOnline - aOnline;
            return b.createdAt - a.createdAt;
        });
    }));
}

export function useMachine(machineId: string): Machine | null {
    return storage(useShallow((state) => state.machines[machineId] ?? null));
}

export function useSessionListViewData(): SessionListViewItem[] | null {
    return storage((state) => state.isDataReady ? state.sessionListViewData : null);
}

export function useAllSessions(): Session[] {
    return storage(useShallow((state) => {
        if (!state.isDataReady) return [];
        return Object.values(state.sessions).sort((a, b) => b.updatedAt - a.updatedAt);
    }));
}

export function useCachedPersonas(): Persona[] {
    return storage((state) => state.cachedPersonas);
}

export function useCachedPersonaProjects(): PersonaProject[] {
    return storage((state) => state.cachedPersonaProjects);
}

export function useCachedAgentWorkspaces(): WorkspaceSummary[] {
    return storage((state) => state.cachedAgentWorkspaces);
}

export function useLocalSettingMutable<K extends keyof LocalSettings>(name: K): [LocalSettings[K], (value: LocalSettings[K]) => void] {
    const setValue = React.useCallback((value: LocalSettings[K]) => {
        storage.getState().applyLocalSettings({ [name]: value });
    }, [name]);
    const value = useLocalSetting(name);
    return [value, setValue];
}

// Project management hooks
export function useProjects() {
    return storage(useShallow((state) => state.getProjects()));
}

export function useProject(projectId: string | null) {
    return storage(useShallow((state) => projectId ? state.getProject(projectId) : null));
}

export function useProjectForSession(sessionId: string | null) {
    return storage(useShallow((state) => sessionId ? state.getProjectForSession(sessionId) : null));
}

export function useProjectSessions(projectId: string | null) {
    return storage(useShallow((state) => projectId ? state.getProjectSessions(projectId) : []));
}

export function useProjectGitStatus(projectId: string | null) {
    return storage(useShallow((state) => projectId ? state.getProjectGitStatus(projectId) : null));
}

export function useSessionProjectGitStatus(sessionId: string | null) {
    return storage(useShallow((state) => sessionId ? state.getSessionProjectGitStatus(sessionId) : null));
}

export function useLocalSetting<K extends keyof LocalSettings>(name: K): LocalSettings[K] {
    return storage(useShallow((state) => state.localSettings[name]));
}

export function useLocalProxyUsage(): LocalProxyUsage {
    return storage(useShallow((state) => state.localProxyUsage));
}

export function useRealtimeStatus(): 'disconnected' | 'connecting' | 'connected' | 'error' {
    return storage(useShallow((state) => state.realtimeStatus));
}

export function useRealtimeMode(): 'idle' | 'speaking' {
    return storage(useShallow((state) => state.realtimeMode));
}

export function useSocketStatus() {
    return storage(useShallow((state) => ({
        status: state.socketStatus,
        lastConnectedAt: state.socketLastConnectedAt,
        lastDisconnectedAt: state.socketLastDisconnectedAt
    })));
}

export function useSessionGitStatus(sessionId: string): GitStatus | null {
    return storage(useShallow((state) => state.sessionGitStatus[sessionId] ?? null));
}

export function useIsDataReady(): boolean {
    return storage(useShallow((state) => state.isDataReady));
}

export function useProfile() {
    return storage(useShallow((state) => state.profile));
}

export function useFriends() {
    return storage(useShallow((state) => state.friends));
}

export function useFriendRequests() {
    return storage(useShallow((state) => {
        // Filter friends to get pending requests (where status is 'pending')
        return Object.values(state.friends).filter(friend => friend.status === 'pending');
    }));
}

export function useAcceptedFriends() {
    return storage(useShallow((state) => {
        return Object.values(state.friends).filter(friend => friend.status === 'friend');
    }));
}

export function useFeedItems() {
    return storage(useShallow((state) => state.feedItems));
}
export function useFeedLoaded() {
    return storage((state) => state.feedLoaded);
}
export function useFriendsLoaded() {
    return storage((state) => state.friendsLoaded);
}

export function useFriend(userId: string | undefined) {
    return storage(useShallow((state) => userId ? state.friends[userId] : undefined));
}

export function useUser(userId: string | undefined) {
    return storage(useShallow((state) => userId ? state.users[userId] : undefined));
}

export function useRequestedFriends() {
    return storage(useShallow((state) => {
        // Filter friends to get sent requests (where status is 'requested')
        return Object.values(state.friends).filter(friend => friend.status === 'requested');
    }));
}

// Expose storage to window for debugging (dev only)
if (__DEV__) {
    (window as any).__storage = storage;
    (window as any).__sync = null; // Will be set by sync.ts
}
