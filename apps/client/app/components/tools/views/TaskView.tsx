import * as React from 'react';
import { ToolViewProps } from './_all';
import { View, StyleSheet } from 'react-native';
import { knownTools } from '../../tools/knownTools';
import { Ionicons } from '@expo/vector-icons';
import { ToolCall } from '@/sync/typesMessage';
import { useUnistyles } from 'react-native-unistyles';
import { t } from '@/text';
import { Text } from '@/components/StyledText';
import { Message } from '@/sync/typesMessage';

interface FilteredTool {
    tool: ToolCall;
    title: string;
    state: 'running' | 'completed' | 'error';
}

export const TaskView = React.memo<ToolViewProps>(({ tool, metadata, messages }) => {
    const { theme } = useUnistyles();
    const timeline: Array<
        | { kind: 'text'; id: string; text: string; isThinking: boolean }
        | { kind: 'tool'; id: string; item: FilteredTool }
    > = [];

    function toolTitle(messageTool: ToolCall): string {
        const tName = messageTool.name || 'unknown';
        const knownTool = knownTools[tName as keyof typeof knownTools] as any;

        if (knownTool) {
            if ('extractDescription' in knownTool && typeof knownTool.extractDescription === 'function') {
                return knownTool.extractDescription({ tool: messageTool, metadata });
            }
            if (knownTool.title) {
                if (typeof knownTool.title === 'function') {
                    return knownTool.title({ tool: messageTool, metadata });
                }
                return knownTool.title;
            }
        }

        return tName;
    }

    function appendMessage(message: Message) {
        if (message.kind === 'agent-text') {
            const text = message.text.trim();
            if (text.length > 0) {
                timeline.push({
                    kind: 'text',
                    id: message.id,
                    text,
                    isThinking: !!message.isThinking,
                });
            }
            return;
        }

        if (message.kind === 'user-text') {
            const text = message.text.trim();
            if (text.length > 0) {
                timeline.push({
                    kind: 'text',
                    id: message.id,
                    text,
                    isThinking: false,
                });
            }
            return;
        }

        if (message.kind === 'tool-call') {
            if (message.tool.state === 'running' || message.tool.state === 'completed' || message.tool.state === 'error') {
                timeline.push({
                    kind: 'tool',
                    id: message.id,
                    item: {
                        tool: message.tool,
                        title: toolTitle(message.tool),
                        state: message.tool.state,
                    },
                });
            }
            for (const child of message.children) {
                appendMessage(child);
            }
        }
    }

    for (let m of messages) {
        appendMessage(m);
    }

    if (typeof tool.input?.summary === 'string' && tool.input.summary.trim().length > 0) {
        timeline.push({
            kind: 'text',
            id: 'task-summary',
            text: tool.input.summary.trim(),
            isThinking: false,
        });
    }

    const styles = StyleSheet.create({
        container: {
            paddingVertical: 4,
            paddingBottom: 12
        },
        toolItem: {
            flexDirection: 'row',
            alignItems: 'center',
            paddingVertical: 4,
            paddingLeft: 4,
            paddingRight: 2
        },
        toolTitle: {
            fontSize: 14,
            fontWeight: '500',
            color: theme.colors.textSecondary,
            fontFamily: 'monospace',
            flex: 1,
        },
        statusContainer: {
            marginLeft: 'auto',
            paddingLeft: 8,
        },
        loadingItem: {
            flexDirection: 'row',
            alignItems: 'center',
            paddingVertical: 8,
            paddingHorizontal: 4,
        },
        loadingText: {
            marginLeft: 8,
            fontSize: 14,
            color: theme.colors.textSecondary,
        },
        moreToolsItem: {
            paddingVertical: 4,
            paddingHorizontal: 4,
        },
        moreToolsText: {
            fontSize: 14,
            color: theme.colors.textSecondary,
            fontStyle: 'italic',
            opacity: 0.7,
        },
        textItem: {
            paddingVertical: 6,
            paddingHorizontal: 4,
        },
        textBody: {
            fontSize: 14,
            lineHeight: 20,
            color: theme.colors.text,
        },
        thinkingText: {
            color: theme.colors.textSecondary,
            fontStyle: 'italic',
        },
    });

    if (timeline.length === 0) {
        return null;
    }

    const visibleItems = timeline.slice(Math.max(0, timeline.length - 8));
    const remainingCount = timeline.length - visibleItems.length;

    return (
        <View style={styles.container}>
            {visibleItems.map((entry, index) => (
                entry.kind === 'text' ? (
                    <View key={`${entry.id}-${index}`} style={styles.textItem}>
                        <Text
                            style={[styles.textBody, entry.isThinking && styles.thinkingText]}
                            numberOfLines={entry.isThinking ? 3 : 6}
                        >
                            {entry.text}
                        </Text>
                    </View>
                ) : (
                    <View key={`${entry.id}-${index}`} style={styles.toolItem}>
                        <Text style={styles.toolTitle}>{entry.item.title}</Text>
                        <View style={styles.statusContainer}>
                            {entry.item.state === 'completed' && (
                                <Ionicons name="checkmark-circle" size={16} color={theme.colors.success} />
                            )}
                            {entry.item.state === 'error' && (
                                <Ionicons name="close-circle" size={16} color={theme.colors.textDestructive} />
                            )}
                        </View>
                    </View>
                )
            ))}
            {remainingCount > 0 && (
                <View style={styles.moreToolsItem}>
                    <Text style={styles.moreToolsText}>
                        {t('tools.taskView.moreTools', { count: remainingCount })}
                    </Text>
                </View>
            )}
        </View>
    );
});
