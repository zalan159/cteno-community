import { useMemo } from 'react';
import { useSessionMessages } from '@/sync/storage';
import { storage } from '@/sync/storage';
import { useShallow } from 'zustand/react/shallow';

export interface PersonaUnreadInfo {
    unreadCount: number;
    lastMessage: { text: string; isUser: boolean; createdAt: number } | null;
}

export function usePersonaUnread(chatSessionId: string): PersonaUnreadInfo {
    const { messages } = useSessionMessages(chatSessionId);
    const lastReadAt = storage(useShallow((state) =>
        state.personaReadTimestamps[chatSessionId] ?? 0
    ));

    return useMemo(() => {
        // Find the last meaningful message (skip agent-event)
        let lastMessage: PersonaUnreadInfo['lastMessage'] = null;
        for (const msg of messages) {
            if (msg.kind === 'agent-event') continue;
            const isUser = msg.kind === 'user-text';
            let text: string;
            if (msg.kind === 'user-text' || msg.kind === 'agent-text') {
                text = msg.text.length > 100 ? msg.text.slice(0, 100) + '…' : msg.text;
            } else if (msg.kind === 'tool-call') {
                text = msg.tool.name;
            } else {
                continue;
            }
            lastMessage = { text, isUser, createdAt: msg.createdAt };
            break;
        }

        // Count unread: messages newer than lastReadAt, excluding user-text
        let unreadCount = 0;
        for (const msg of messages) {
            if (msg.createdAt <= lastReadAt) break;
            if (msg.kind === 'user-text') continue;
            if (msg.kind === 'agent-event') continue;
            unreadCount++;
        }

        return { unreadCount, lastMessage };
    }, [messages, lastReadAt]);
}
