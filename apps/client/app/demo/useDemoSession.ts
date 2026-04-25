/**
 * Hook that manages a demo chat session.
 *
 * Creates a fake session in the Zustand store and handles
 * simulated message sending with canned AI responses.
 */

import { useCallback, useEffect, useRef } from 'react';
import { storage } from '@/sync/storage';
import { createReducer } from '@/sync/reducer/reducer';
import { Message, AgentTextMessage, UserTextMessage, ToolCallMessage } from '@/sync/typesMessage';
import { DEMO_SESSION_ID } from './demoMode';
import { getDemoResponse, resetDemoResponses } from './demoResponses';

let messageCounter = 0;

function makeId(): string {
    return `demo-msg-${Date.now()}-${messageCounter++}`;
}

function makeUserMessage(text: string): UserTextMessage {
    return {
        kind: 'user-text',
        id: makeId(),
        localId: makeId(),
        createdAt: Date.now(),
        text,
    };
}

function makeAgentMessage(text: string): AgentTextMessage {
    return {
        kind: 'agent-text',
        id: makeId(),
        localId: null,
        createdAt: Date.now(),
        text,
    };
}

function makeToolCallMessage(
    name: string,
    description: string,
    input: any,
    result: string,
): ToolCallMessage {
    const now = Date.now();
    return {
        kind: 'tool-call',
        id: makeId(),
        localId: null,
        createdAt: now,
        tool: {
            name,
            state: 'completed',
            input,
            createdAt: now,
            startedAt: now,
            completedAt: now + 500,
            description,
            result,
        },
        children: [],
    };
}

function pushMessages(newMessages: Message[]) {
    storage.setState((state) => {
        const existing = state.sessionMessages[DEMO_SESSION_ID];
        if (!existing) return state;

        const messagesMap = { ...existing.messagesMap };
        for (const msg of newMessages) {
            messagesMap[msg.id] = msg;
        }

        // Combine and sort descending by createdAt (newest first)
        const allMessages = [...existing.messages, ...newMessages].sort(
            (a, b) => b.createdAt - a.createdAt,
        );

        return {
            ...state,
            sessionMessages: {
                ...state.sessionMessages,
                [DEMO_SESSION_ID]: {
                    ...existing,
                    messages: allMessages,
                    messagesMap,
                },
            },
        };
    });
}

/**
 * Set up the demo session in the store and return a sendMessage callback.
 */
export function useDemoSession() {
    const thinkingTimerRef = useRef<NodeJS.Timeout | null>(null);

    // Initialize demo session in store on mount
    useEffect(() => {
        resetDemoResponses();
        messageCounter = 0;

        // Create demo session entry in sessions store
        storage.setState((state) => ({
            ...state,
            sessions: {
                ...state.sessions,
                [DEMO_SESSION_ID]: {
                    id: DEMO_SESSION_ID,
                    seq: 0,
                    createdAt: Date.now(),
                    updatedAt: Date.now(),
                    active: true,
                    activeAt: Date.now(),
                    metadata: {
                        version: '1.0.0',
                        flavor: 'claude',
                        host: 'Demo',
                        path: '/demo',
                        homeDir: '/demo',
                    },
                    metadataVersion: 0,
                    agentState: null,
                    agentStateVersion: 0,
                    thinking: false,
                    thinkingAt: 0,
                    presence: 'online',
                },
            },
            sessionMessages: {
                ...state.sessionMessages,
                [DEMO_SESSION_ID]: {
                    messages: [],
                    messagesMap: {},
                    taskLifecycle: {},
                    reducerState: createReducer(),
                    isLoaded: true,
                    isSyncing: false,
                    hasOlderMessages: false,
                    isLoadingOlder: false,
                },
            },
        }));

        // Cleanup on unmount
        return () => {
            if (thinkingTimerRef.current) {
                clearTimeout(thinkingTimerRef.current);
            }
            storage.setState((state) => {
                const { [DEMO_SESSION_ID]: _s, ...restSessions } = state.sessions;
                const { [DEMO_SESSION_ID]: _m, ...restMessages } = state.sessionMessages;
                return {
                    ...state,
                    sessions: restSessions,
                    sessionMessages: restMessages,
                };
            });
        };
    }, []);

    // Set thinking state on the demo session
    const setThinking = useCallback((thinking: boolean) => {
        storage.setState((state) => {
            const session = state.sessions[DEMO_SESSION_ID];
            if (!session) return state;
            return {
                ...state,
                sessions: {
                    ...state.sessions,
                    [DEMO_SESSION_ID]: {
                        ...session,
                        thinking,
                        thinkingAt: thinking ? Date.now() : session.thinkingAt,
                    },
                },
            };
        });
    }, []);

    const sendMessage = useCallback(
        (text: string) => {
            if (!text.trim()) return;

            // Add user message immediately
            const userMsg = makeUserMessage(text.trim());
            pushMessages([userMsg]);

            // Show "thinking" state
            setThinking(true);

            // Generate response after a brief delay
            const response = getDemoResponse(text.trim());
            const delay = response.toolCall ? 1500 : 1000;

            if (thinkingTimerRef.current) {
                clearTimeout(thinkingTimerRef.current);
            }

            thinkingTimerRef.current = setTimeout(() => {
                const responseMessages: Message[] = [];

                if (response.toolCall) {
                    responseMessages.push(
                        makeToolCallMessage(
                            response.toolCall.name,
                            response.toolCall.description,
                            response.toolCall.input,
                            response.toolCall.result,
                        ),
                    );
                }

                responseMessages.push(makeAgentMessage(response.text));
                pushMessages(responseMessages);
                setThinking(false);
            }, delay);
        },
        [setThinking],
    );

    return { sendMessage, sessionId: DEMO_SESSION_ID };
}
