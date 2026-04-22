import * as React from 'react';
import { View, Pressable, Linking } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { Ionicons } from '@expo/vector-icons';
import { ToolViewProps } from './_all';
import { Text } from '@/components/StyledText';
import { CodeView } from '@/components/CodeView';
import { ToolSectionView } from '../ToolSectionView';

const MAX_PREVIEW_LENGTH = 800;

export const FetchView = React.memo<ToolViewProps>(({ tool }) => {
    const [expanded, setExpanded] = React.useState(false);

    const url = typeof tool.input?.url === 'string' ? tool.input.url : null;
    const prompt = typeof tool.input?.prompt === 'string' ? tool.input.prompt : null;
    const result = typeof tool.result === 'string' ? tool.result : null;

    const handleOpenUrl = React.useCallback(() => {
        if (url) {
            Linking.openURL(url).catch(() => { });
        }
    }, [url]);

    // Running state: show URL + prompt
    if (tool.state === 'running') {
        return (
            <View style={styles.container}>
                {url && (
                    <Pressable onPress={handleOpenUrl} style={styles.urlRow}>
                        <Ionicons name="link-outline" size={14} color="#007AFF" />
                        <Text style={styles.urlText} numberOfLines={1}>{url}</Text>
                    </Pressable>
                )}
                {prompt && (
                    <Text style={styles.promptText} numberOfLines={2}>{prompt}</Text>
                )}
            </View>
        );
    }

    // Completed: show result preview
    if (tool.state === 'completed' && result) {
        const isLong = result.length > MAX_PREVIEW_LENGTH;
        const displayText = expanded ? result : result.slice(0, MAX_PREVIEW_LENGTH);

        return (
            <View style={styles.container}>
                {url && (
                    <Pressable onPress={handleOpenUrl} style={styles.urlRow}>
                        <Ionicons name="link-outline" size={14} color="#007AFF" />
                        <Text style={styles.urlText} numberOfLines={1}>{url}</Text>
                    </Pressable>
                )}
                <ToolSectionView>
                    <CodeView code={displayText} />
                </ToolSectionView>
                {isLong && !expanded && (
                    <Pressable onPress={() => setExpanded(true)} style={styles.expandRow}>
                        <Text style={styles.expandText}>
                            {`${(result.length / 1000).toFixed(1)}k chars — tap to expand`}
                        </Text>
                    </Pressable>
                )}
            </View>
        );
    }

    // Error or other states: let default error rendering handle it
    return null;
});

const styles = StyleSheet.create((theme) => ({
    container: {
        paddingBottom: 4,
    },
    urlRow: {
        flexDirection: 'row',
        alignItems: 'center',
        gap: 6,
        marginBottom: 6,
    },
    urlText: {
        fontSize: 13,
        color: '#007AFF',
        flex: 1,
    },
    promptText: {
        fontSize: 13,
        color: theme.colors.textSecondary,
        marginBottom: 6,
        lineHeight: 18,
    },
    expandRow: {
        alignItems: 'center',
        paddingVertical: 6,
    },
    expandText: {
        fontSize: 12,
        color: theme.colors.textSecondary,
    },
}));
