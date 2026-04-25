import * as React from 'react';
import { View, Pressable } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { Ionicons, Octicons } from '@expo/vector-icons';
import { ToolViewProps } from './_all';
import { Text } from '@/components/StyledText';
import { CodeView } from '@/components/CodeView';
import { ToolSectionView } from '../ToolSectionView';

const MAX_PREVIEW_LENGTH = 1200;

function resultToString(result: unknown): string {
    if (typeof result === 'string') return result;
    if (result == null) return '';
    return JSON.stringify(result, null, 2);
}

function compactValue(value: unknown): string | null {
    if (typeof value === 'string' && value.trim()) return value;
    if (typeof value === 'number' || typeof value === 'boolean') return String(value);
    return null;
}

function SearchParams({ tool }: Pick<ToolViewProps, 'tool'>) {
    const entries = [
        ['pattern', compactValue(tool.input?.pattern)],
        ['path', compactValue(tool.input?.path)],
        ['glob', compactValue(tool.input?.glob)],
        ['type', compactValue(tool.input?.type)],
        ['mode', compactValue(tool.input?.output_mode)],
    ].filter((entry): entry is [string, string] => Boolean(entry[1]));

    if (entries.length === 0) return null;

    return (
        <View style={styles.params}>
            {entries.map(([label, value]) => (
                <View key={label} style={styles.param}>
                    <Text style={styles.paramLabel}>{label}</Text>
                    <Text style={styles.paramValue} numberOfLines={1}>{value}</Text>
                </View>
            ))}
        </View>
    );
}

function SearchOutput({ result }: { result: unknown }) {
    const [expanded, setExpanded] = React.useState(false);
    const text = resultToString(result).trim();

    if (!text) return null;

    const isEmpty = text === 'No matches found' || text === 'No files found';
    if (isEmpty) {
        return (
            <View style={styles.emptyRow}>
                <Ionicons name="search-outline" size={14} color="#8E8E93" />
                <Text style={styles.emptyText}>{text}</Text>
            </View>
        );
    }

    const lines = text.split('\n').filter(Boolean);
    const visibleText = expanded || text.length <= MAX_PREVIEW_LENGTH
        ? text
        : text.slice(0, MAX_PREVIEW_LENGTH);

    return (
        <View>
            <View style={styles.resultMeta}>
                <Octicons name="file-directory" size={13} color="#8E8E93" />
                <Text style={styles.resultMetaText}>
                    {`${lines.length} ${lines.length === 1 ? 'line' : 'lines'}`}
                </Text>
            </View>
            <ToolSectionView>
                <CodeView code={visibleText} />
            </ToolSectionView>
            {text.length > MAX_PREVIEW_LENGTH && !expanded ? (
                <Pressable onPress={() => setExpanded(true)} style={styles.expandRow}>
                    <Text style={styles.expandText}>
                        {`${(text.length / 1000).toFixed(1)}k chars - tap to expand`}
                    </Text>
                </Pressable>
            ) : null}
        </View>
    );
}

export const GrepView = React.memo<ToolViewProps>(({ tool }) => {
    if (tool.state === 'running') {
        return (
            <View style={styles.container}>
                <View style={styles.runningRow}>
                    <Text style={styles.runningText} numberOfLines={1}>
                        {compactValue(tool.input?.pattern) ? `Searching ${tool.input.pattern}` : 'Searching...'}
                    </Text>
                </View>
                <SearchParams tool={tool} />
            </View>
        );
    }

    if (tool.state === 'completed') {
        return (
            <View style={styles.container}>
                <SearchParams tool={tool} />
                <SearchOutput result={tool.result} />
            </View>
        );
    }

    return null;
});

export const GlobView = React.memo<ToolViewProps>(({ tool }) => {
    if (tool.state === 'running') {
        return (
            <View style={styles.container}>
                <View style={styles.runningRow}>
                    <Text style={styles.runningText} numberOfLines={1}>
                        {compactValue(tool.input?.pattern) ? `Finding ${tool.input.pattern}` : 'Finding files...'}
                    </Text>
                </View>
                <SearchParams tool={tool} />
            </View>
        );
    }

    if (tool.state === 'completed') {
        return (
            <View style={styles.container}>
                <SearchParams tool={tool} />
                <SearchOutput result={tool.result} />
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
    runningRow: {
        flexDirection: 'row',
        alignItems: 'center',
        gap: 8,
        marginBottom: 8,
    },
    runningText: {
        fontSize: 13,
        color: theme.colors.textSecondary,
        flex: 1,
    },
    params: {
        flexDirection: 'row',
        flexWrap: 'wrap',
        gap: 6,
        marginBottom: 8,
    },
    param: {
        flexDirection: 'row',
        alignItems: 'center',
        maxWidth: '100%',
        borderWidth: 1,
        borderColor: theme.colors.divider,
        borderRadius: 6,
        paddingHorizontal: 7,
        paddingVertical: 3,
        gap: 5,
    },
    paramLabel: {
        fontSize: 11,
        color: theme.colors.textSecondary,
        fontWeight: '600',
    },
    paramValue: {
        fontSize: 11,
        color: theme.colors.text,
        maxWidth: 260,
    },
    resultMeta: {
        flexDirection: 'row',
        alignItems: 'center',
        gap: 6,
        marginBottom: 6,
    },
    resultMetaText: {
        fontSize: 12,
        color: theme.colors.textSecondary,
    },
    emptyRow: {
        flexDirection: 'row',
        alignItems: 'center',
        gap: 6,
        paddingVertical: 6,
    },
    emptyText: {
        fontSize: 13,
        color: theme.colors.textSecondary,
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
