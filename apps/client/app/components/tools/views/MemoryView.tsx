import * as React from 'react';
import { View, Platform } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { Ionicons } from '@expo/vector-icons';
import type { ToolViewProps } from './_all';
import { Text } from '@/components/StyledText';

// Parse recall results from the formatted string output
function parseRecallResults(resultStr: string): Array<{ path: string; score: number; content: string }> {
    const results: Array<{ path: string; score: number; content: string }> = [];
    // Format: --- path (score: 0.85) ---\ncontent\n\n
    const blocks = resultStr.split(/^---\s+/m).filter(Boolean);
    for (const block of blocks) {
        const headerEnd = block.indexOf('---\n');
        if (headerEnd === -1) continue;
        const header = block.substring(0, headerEnd).trim();
        const content = block.substring(headerEnd + 4).trim();
        // Extract path and score from "path (score: 0.85)"
        const match = header.match(/^(.+?)\s+\(score:\s*([\d.]+)\)$/);
        if (match) {
            results.push({ path: match[1], score: parseFloat(match[2]), content });
        }
    }
    return results;
}

// Parse list results
function parseListResults(resultStr: string): string[] {
    const lines = resultStr.split('\n').filter(l => l.startsWith('- '));
    return lines.map(l => l.substring(2).trim());
}

// Scope badge component
function ScopeBadge({ path }: { path: string }) {
    const isPrivate = path.includes('[private');
    const nameMatch = path.match(/\[private:(.+?)\]/);
    const label = nameMatch ? nameMatch[1] : isPrivate ? 'private' : 'global';
    return (
        <View style={[scopeStyles.badge, isPrivate ? scopeStyles.privateBadge : scopeStyles.globalBadge]}>
            <Ionicons
                name={isPrivate ? 'person' : 'globe-outline'}
                size={10}
                color={isPrivate ? '#007AFF' : '#34C759'}
            />
            <Text style={[scopeStyles.badgeText, isPrivate ? scopeStyles.privateText : scopeStyles.globalText]}>
                {label}
            </Text>
        </View>
    );
}

// Clean path by removing scope prefix
function cleanPath(path: string): string {
    return path.replace(/^\[(private(:\w+)?|global)\]\s*/, '');
}

// Score bar component
function ScoreBar({ score }: { score: number }) {
    const pct = Math.round(score * 100);
    const color = score >= 0.7 ? '#34C759' : score >= 0.4 ? '#FF9500' : '#8E8E93';
    return (
        <View style={scoreStyles.container}>
            <View style={scoreStyles.track}>
                <View style={[scoreStyles.fill, { width: `${pct}%`, backgroundColor: color }]} />
            </View>
            <Text style={[scoreStyles.label, { color }]}>{pct}%</Text>
        </View>
    );
}

export const MemoryView = ({ tool }: ToolViewProps) => {
    const action = tool.input?.action as string;
    const resultStr = typeof tool.result === 'string' ? tool.result : String(tool.result || '');

    // === SAVE ===
    if (action === 'save') {
        const filePath = tool.input?.file_path || 'knowledge/general.md';
        const scope = tool.input?.scope || 'private';
        const isPrivate = scope === 'private';
        return (
            <View style={styles.container}>
                <View style={styles.row}>
                    <Ionicons name="checkmark-circle" size={18} color="#34C759" />
                    <Text style={styles.actionLabel}>Saved</Text>
                    <View style={[scopeStyles.badge, isPrivate ? scopeStyles.privateBadge : scopeStyles.globalBadge]}>
                        <Ionicons name={isPrivate ? 'person' : 'globe-outline'} size={10} color={isPrivate ? '#007AFF' : '#34C759'} />
                        <Text style={[scopeStyles.badgeText, isPrivate ? scopeStyles.privateText : scopeStyles.globalText]}>{scope}</Text>
                    </View>
                </View>
                <Text style={styles.filePath}>{filePath}</Text>
            </View>
        );
    }

    // === RECALL ===
    if (action === 'recall') {
        const query = tool.input?.query || '';

        if (tool.state === 'running') {
            return (
                <View style={styles.container}>
                    <View style={styles.queryRow}>
                        <Ionicons name="search" size={14} color="#8E8E93" />
                        <Text style={styles.queryText}>{query}</Text>
                    </View>
                </View>
            );
        }

        if (!resultStr || resultStr === 'No matching memories found.') {
            return (
                <View style={styles.container}>
                    <View style={styles.queryRow}>
                        <Ionicons name="search" size={14} color="#8E8E93" />
                        <Text style={styles.queryText}>{query}</Text>
                    </View>
                    <Text style={styles.emptyText}>No matching memories found.</Text>
                </View>
            );
        }

        const results = parseRecallResults(resultStr);
        if (results.length === 0) {
            // Fallback: can't parse, show raw
            return (
                <View style={styles.container}>
                    <View style={styles.queryRow}>
                        <Ionicons name="search" size={14} color="#8E8E93" />
                        <Text style={styles.queryText}>{query}</Text>
                    </View>
                    <Text style={styles.monoText} numberOfLines={20}>{resultStr}</Text>
                </View>
            );
        }

        return (
            <View style={styles.container}>
                <View style={styles.queryRow}>
                    <Ionicons name="search" size={14} color="#8E8E93" />
                    <Text style={styles.queryText}>{query}</Text>
                    <Text style={styles.countBadge}>{results.length}</Text>
                </View>
                {results.map((r, i) => (
                    <View key={i} style={styles.recallCard}>
                        <View style={styles.recallHeader}>
                            <ScopeBadge path={r.path} />
                            <Text style={styles.recallPath} numberOfLines={1}>{cleanPath(r.path)}</Text>
                            <ScoreBar score={r.score} />
                        </View>
                        <Text style={styles.recallContent} numberOfLines={4}>{r.content}</Text>
                    </View>
                ))}
            </View>
        );
    }

    // === READ ===
    if (action === 'read') {
        const filePath = tool.input?.file_path || '';
        return (
            <View style={styles.container}>
                <View style={styles.row}>
                    <Ionicons name="document-text-outline" size={16} color="#8E8E93" />
                    <Text style={styles.filePath}>{filePath}</Text>
                </View>
                {tool.state === 'completed' && resultStr && (
                    <Text style={styles.monoText} numberOfLines={15}>{resultStr}</Text>
                )}
            </View>
        );
    }

    // === LIST ===
    if (action === 'list') {
        if (tool.state !== 'completed' || !resultStr) return null;

        if (resultStr === 'No memory files found.') {
            return (
                <View style={styles.container}>
                    <Text style={styles.emptyText}>No memory files found.</Text>
                </View>
            );
        }

        const files = parseListResults(resultStr);
        return (
            <View style={styles.container}>
                <Text style={styles.listHeader}>{files.length} files</Text>
                <View style={styles.fileList}>
                    {files.map((f, i) => (
                        <View key={i} style={styles.fileItem}>
                            <ScopeBadge path={f} />
                            <Text style={styles.fileName} numberOfLines={1}>{cleanPath(f)}</Text>
                        </View>
                    ))}
                </View>
            </View>
        );
    }

    // Fallback
    return null;
};

const scopeStyles = StyleSheet.create((theme) => ({
    badge: {
        flexDirection: 'row',
        alignItems: 'center',
        gap: 3,
        paddingHorizontal: 6,
        paddingVertical: 2,
        borderRadius: 4,
    },
    privateBadge: {
        backgroundColor: 'rgba(0, 122, 255, 0.1)',
    },
    globalBadge: {
        backgroundColor: 'rgba(52, 199, 89, 0.1)',
    },
    badgeText: {
        fontSize: 11,
        fontWeight: '500',
    },
    privateText: {
        color: '#007AFF',
    },
    globalText: {
        color: '#34C759',
    },
}));

const scoreStyles = StyleSheet.create((theme) => ({
    container: {
        flexDirection: 'row',
        alignItems: 'center',
        gap: 4,
        minWidth: 60,
    },
    track: {
        flex: 1,
        height: 3,
        backgroundColor: theme.colors.surfaceHigh,
        borderRadius: 2,
        overflow: 'hidden',
    },
    fill: {
        height: '100%',
        borderRadius: 2,
    },
    label: {
        fontSize: 10,
        fontWeight: '600',
        fontFamily: Platform.select({ ios: 'Menlo', default: 'monospace' }),
    },
}));

const styles = StyleSheet.create((theme) => ({
    container: {
        paddingHorizontal: 12,
        paddingVertical: 8,
        gap: 8,
    },
    row: {
        flexDirection: 'row',
        alignItems: 'center',
        gap: 8,
    },
    actionLabel: {
        fontSize: 14,
        fontWeight: '500',
        color: '#34C759',
    },
    filePath: {
        fontSize: 13,
        color: theme.colors.textSecondary,
        fontFamily: Platform.select({ ios: 'Menlo', default: 'monospace' }),
    },
    queryRow: {
        flexDirection: 'row',
        alignItems: 'center',
        gap: 6,
    },
    queryText: {
        fontSize: 13,
        color: theme.colors.text,
        fontWeight: '500',
        flex: 1,
    },
    countBadge: {
        fontSize: 11,
        fontWeight: '600',
        color: theme.colors.textSecondary,
        backgroundColor: theme.colors.surfaceHigh,
        paddingHorizontal: 6,
        paddingVertical: 1,
        borderRadius: 8,
        overflow: 'hidden',
    },
    recallCard: {
        backgroundColor: theme.colors.surfaceHigh,
        borderRadius: 6,
        padding: 10,
        gap: 6,
    },
    recallHeader: {
        flexDirection: 'row',
        alignItems: 'center',
        gap: 6,
    },
    recallPath: {
        fontSize: 12,
        color: theme.colors.textSecondary,
        fontFamily: Platform.select({ ios: 'Menlo', default: 'monospace' }),
        flex: 1,
    },
    recallContent: {
        fontSize: 13,
        color: theme.colors.text,
        lineHeight: 18,
    },
    monoText: {
        fontSize: 12,
        color: theme.colors.text,
        fontFamily: Platform.select({ ios: 'Menlo', default: 'monospace' }),
        lineHeight: 17,
    },
    emptyText: {
        fontSize: 13,
        color: theme.colors.textSecondary,
        fontStyle: 'italic',
    },
    listHeader: {
        fontSize: 12,
        color: theme.colors.textSecondary,
        fontWeight: '500',
    },
    fileList: {
        gap: 4,
    },
    fileItem: {
        flexDirection: 'row',
        alignItems: 'center',
        gap: 6,
    },
    fileName: {
        fontSize: 13,
        color: theme.colors.text,
        fontFamily: Platform.select({ ios: 'Menlo', default: 'monospace' }),
        flex: 1,
    },
}));
