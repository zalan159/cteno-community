import * as React from 'react';
import { View } from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { ToolCall } from '@/sync/typesMessage';
import { knownTools } from '@/components/tools/knownTools';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Text } from '@/components/StyledText';
import { HostToolBadge } from './HostToolBadge';
import { getHostToolSubtitle, isHostOwnedTool } from './hostTool';

interface ToolHeaderProps {
    tool: ToolCall;
}

export function ToolHeader({ tool }: ToolHeaderProps) {
    const { theme } = useUnistyles();
    const toolName = tool.name || 'unknown';
    const knownTool = knownTools[toolName as keyof typeof knownTools] as any;
    const isHostOwned = isHostOwnedTool(tool);

    // Extract status first for Bash tool to potentially use as title
    let status: string | null = null;
    if (knownTool && typeof knownTool.extractStatus === 'function') {
        const extractedStatus = knownTool.extractStatus({ tool, metadata: null });
        if (typeof extractedStatus === 'string' && extractedStatus) {
            status = extractedStatus;
        }
    }

    // Handle optional title and function type
    let toolTitle = toolName;
    if (knownTool?.title) {
        if (typeof knownTool.title === 'function') {
            toolTitle = knownTool.title({ tool, metadata: null });
        } else {
            toolTitle = knownTool.title;
        }
    }

    const icon = knownTool?.icon ? knownTool.icon(18, theme.colors.header.tint) : <Ionicons name="construct-outline" size={18} color={theme.colors.header.tint} />;

    // Extract subtitle using the same logic as ToolView
    let subtitle = null;
    if (knownTool && typeof knownTool.extractSubtitle === 'function') {
        const extractedSubtitle = knownTool.extractSubtitle({ tool, metadata: null });
        if (typeof extractedSubtitle === 'string' && extractedSubtitle) {
            subtitle = extractedSubtitle;
        }
    }
    subtitle = getHostToolSubtitle(tool, subtitle);

    return (
        <View style={styles.container}>
            <View style={styles.titleContainer}>
                <View style={styles.titleRow}>
                    {icon}
                    <Text style={styles.title} numberOfLines={1}>{toolTitle}</Text>
                    {isHostOwned ? <HostToolBadge /> : null}
                </View>
                {subtitle && (
                    <Text style={styles.subtitle} numberOfLines={1}>{subtitle}</Text>
                )}
            </View>
        </View>
    );
}

const styles = StyleSheet.create((theme) => ({
    container: {
        flexDirection: 'row',
        justifyContent: 'center',
        alignItems: 'center',
        flexGrow: 1,
        flexBasis: 0,
        paddingHorizontal: 4,
    },
    titleContainer: {
        flexDirection: 'column',
        alignItems: 'center',
        flexGrow: 1,
        flexBasis: 0
    },
    titleRow: {
        flexDirection: 'row',
        alignItems: 'center',
        gap: 6,
        justifyContent: 'center',
    },
    title: {
        fontSize: 14,
        fontWeight: '500',
        color: theme.colors.text,
        textAlign: 'center',
        flexShrink: 1,
    },
    subtitle: {
        fontSize: 11,
        color: theme.colors.textSecondary,
        textAlign: 'center',
        marginTop: 2,
    },
}));
