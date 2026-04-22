import * as React from 'react';
import { View, ActivityIndicator, Pressable } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { Ionicons } from '@expo/vector-icons';
import { ToolViewProps } from './_all';
import { Text } from '@/components/StyledText';
import { CodeView } from '@/components/CodeView';
import { ToolSectionView } from '../ToolSectionView';

const MAX_PREVIEW_LINES = 15;

/**
 * browser_state output:
 *   "URL: {url}\nTitle: {title}\n\n[0] button "Submit"\n[1] ...\n\nN elements indexed."
 */
function parseStateResult(result: unknown): { url: string; title: string; tree: string; elementCount: number } | null {
    if (!result) return null;
    const raw = typeof result === 'string' ? result : String(result);
    const urlMatch = raw.match(/URL:\s*(.+)/);
    const titleMatch = raw.match(/Title:\s*(.+)/);
    const countMatch = raw.match(/(\d+)\s*elements?\s*indexed/);
    if (!urlMatch) return null;

    // Extract tree: everything between title line and the "N elements" line
    const titleEnd = raw.indexOf('\n', raw.indexOf('Title:'));
    const countStart = raw.lastIndexOf('\n', raw.search(/\d+\s*elements?\s*indexed/));
    const tree = titleEnd > 0 && countStart > titleEnd
        ? raw.slice(titleEnd + 1, countStart).trim()
        : '';

    return {
        url: urlMatch[1].trim(),
        title: titleMatch?.[1]?.trim() || '',
        tree,
        elementCount: countMatch ? parseInt(countMatch[1], 10) : 0,
    };
}

export const BrowserStateView = React.memo<ToolViewProps>(({ tool }) => {
    const [expanded, setExpanded] = React.useState(false);
    const query = typeof tool.input?.query === 'string' ? tool.input.query : null;
    const interactiveOnly = tool.input?.interactive_only === true;

    if (tool.state === 'running') {
        return (
            <View style={styles.container}>
                <View style={styles.row}>
                    <ActivityIndicator size="small" />
                    <Text style={styles.runningText}>
                        {query ? `Searching for "${query}"...` : 'Getting page state...'}
                    </Text>
                </View>
            </View>
        );
    }

    if (tool.state === 'completed') {
        const parsed = parseStateResult(tool.result);
        if (!parsed) return null;

        const treeLines = parsed.tree.split('\n');
        const isLong = treeLines.length > MAX_PREVIEW_LINES;
        const displayTree = expanded ? parsed.tree : treeLines.slice(0, MAX_PREVIEW_LINES).join('\n');

        const filters: string[] = [];
        if (query) filters.push(`query: "${query}"`);
        if (interactiveOnly) filters.push('interactive only');

        return (
            <View style={styles.container}>
                <View style={styles.header}>
                    <Ionicons name="globe-outline" size={14} color="#007AFF" />
                    <Text style={styles.urlText} numberOfLines={1}>{parsed.url}</Text>
                </View>
                {parsed.title ? (
                    <Text style={styles.titleText} numberOfLines={1}>{parsed.title}</Text>
                ) : null}
                <View style={styles.metaRow}>
                    <Text style={styles.metaText}>{parsed.elementCount} elements</Text>
                    {filters.length > 0 && (
                        <Text style={styles.filterText}>{filters.join(', ')}</Text>
                    )}
                </View>
                {displayTree.length > 0 && (
                    <ToolSectionView>
                        <CodeView code={displayTree} />
                    </ToolSectionView>
                )}
                {isLong && !expanded && (
                    <Pressable onPress={() => setExpanded(true)} style={styles.expandRow}>
                        <Text style={styles.expandText}>
                            {`${treeLines.length} lines — tap to expand`}
                        </Text>
                    </Pressable>
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
    },
    header: {
        flexDirection: 'row',
        alignItems: 'center',
        gap: 6,
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
    metaRow: {
        flexDirection: 'row',
        alignItems: 'center',
        gap: 8,
        marginTop: 4,
        marginBottom: 4,
    },
    metaText: {
        fontSize: 12,
        color: theme.colors.textSecondary,
    },
    filterText: {
        fontSize: 12,
        color: theme.colors.textSecondary,
        fontStyle: 'italic',
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
