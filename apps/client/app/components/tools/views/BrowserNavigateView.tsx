import * as React from 'react';
import { View, ActivityIndicator, Pressable, Linking } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { Ionicons } from '@expo/vector-icons';
import { ToolViewProps } from './_all';
import { Text } from '@/components/StyledText';

/**
 * browser_navigate output format:
 *   "Navigated to: {url}\nTitle: {title}\n\nPage structure (first 50 elements):\n{tree}\n\n{count} elements indexed."
 */
function parseNavigateResult(result: unknown): { url: string; title: string; elementCount: number } | null {
    if (!result) return null;
    const raw = typeof result === 'string' ? result : String(result);
    const urlMatch = raw.match(/Navigated to:\s*(.+)/);
    const titleMatch = raw.match(/Title:\s*(.+)/);
    const countMatch = raw.match(/(\d+)\s*elements?\s*indexed/);
    if (!urlMatch) return null;
    return {
        url: urlMatch[1].trim(),
        title: titleMatch?.[1]?.trim() || '',
        elementCount: countMatch ? parseInt(countMatch[1], 10) : 0,
    };
}

export const BrowserNavigateView = React.memo<ToolViewProps>(({ tool }) => {
    const url = typeof tool.input?.url === 'string' ? tool.input.url : null;

    if (tool.state === 'running') {
        return (
            <View style={styles.container}>
                <View style={styles.row}>
                    <ActivityIndicator size="small" />
                    <Text style={styles.runningText} numberOfLines={1}>
                        {url ? `Loading ${url}` : 'Navigating...'}
                    </Text>
                </View>
            </View>
        );
    }

    if (tool.state === 'completed') {
        const parsed = parseNavigateResult(tool.result);
        if (!parsed) return null;

        return (
            <View style={styles.container}>
                <Pressable
                    style={({ pressed }) => [styles.urlRow, pressed && styles.pressed]}
                    onPress={() => Linking.openURL(parsed.url).catch(() => {})}
                >
                    <Ionicons name="globe-outline" size={14} color="#007AFF" />
                    <Text style={styles.urlText} numberOfLines={1}>{parsed.url}</Text>
                </Pressable>
                {parsed.title ? (
                    <Text style={styles.titleText} numberOfLines={1}>{parsed.title}</Text>
                ) : null}
                {parsed.elementCount > 0 && (
                    <Text style={styles.metaText}>{parsed.elementCount} elements indexed</Text>
                )}
            </View>
        );
    }

    return null;
});

const styles = StyleSheet.create((theme) => ({
    container: {
        paddingVertical: 4,
        paddingBottom: 8,
    },
    row: {
        flexDirection: 'row',
        alignItems: 'center',
        gap: 8,
    },
    runningText: {
        fontSize: 13,
        color: theme.colors.textSecondary,
        flex: 1,
    },
    urlRow: {
        flexDirection: 'row',
        alignItems: 'center',
        gap: 6,
    },
    pressed: {
        opacity: 0.6,
    },
    urlText: {
        fontSize: 13,
        color: '#007AFF',
        flex: 1,
    },
    titleText: {
        fontSize: 13,
        color: theme.colors.text,
        marginTop: 2,
        fontWeight: '500',
    },
    metaText: {
        fontSize: 12,
        color: theme.colors.textSecondary,
        marginTop: 4,
    },
}));
