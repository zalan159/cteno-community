import * as React from 'react';
import { View, ActivityIndicator } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { Ionicons } from '@expo/vector-icons';
import { ToolViewProps } from './_all';
import { Text } from '@/components/StyledText';

const ACTION_LABELS: Record<string, string> = {
    list_tabs: 'Listing tabs',
    switch_tab: 'Switching tab',
    new_tab: 'Opening tab',
    close_tab: 'Closing tab',
    close_browser: 'Closing browser',
};

/**
 * browser_manage outputs:
 * - list_tabs: "N tab(s):\n[0] Title - url (active)\n[1] Title - url"
 * - switch_tab: "Switched to tab: Title\nURL: url\nN elements indexed."
 * - new_tab: similar to navigate output
 * - close_tab: "Closed tab: ..."
 * - close_browser: "Browser closed and cleaned up."
 */

interface TabInfo {
    index: number;
    text: string;
    active: boolean;
}

function parseTabList(result: string): TabInfo[] | null {
    const lines = result.split('\n').filter(l => l.trim());
    if (!lines[0]?.match(/\d+\s*tab/)) return null;

    const tabs: TabInfo[] = [];
    for (let i = 1; i < lines.length; i++) {
        const m = lines[i].match(/^\[(\d+)]\s*(.+)/);
        if (m) {
            tabs.push({
                index: parseInt(m[1], 10),
                text: m[2].replace(/\s*\(active\)\s*$/, ''),
                active: lines[i].includes('(active)'),
            });
        }
    }
    return tabs.length > 0 ? tabs : null;
}

export const BrowserManageView = React.memo<ToolViewProps>(({ tool }) => {
    const action = typeof tool.input?.action === 'string' ? tool.input.action : null;

    if (tool.state === 'running') {
        const label = action ? (ACTION_LABELS[action] || action) : 'Managing browser';
        return (
            <View style={styles.container}>
                <View style={styles.row}>
                    <ActivityIndicator size="small" />
                    <Text style={styles.runningText}>{label}...</Text>
                </View>
            </View>
        );
    }

    if (tool.state === 'completed' && tool.result) {
        const raw = typeof tool.result === 'string' ? tool.result : String(tool.result);

        // Tab list rendering
        if (action === 'list_tabs') {
            const tabs = parseTabList(raw);
            if (tabs) {
                return (
                    <View style={styles.container}>
                        <Text style={styles.headerText}>{tabs.length} tab{tabs.length !== 1 ? 's' : ''}</Text>
                        {tabs.map((tab) => (
                            <View key={tab.index} style={styles.tabRow}>
                                <Ionicons
                                    name={tab.active ? 'radio-button-on' : 'radio-button-off'}
                                    size={12}
                                    color={tab.active ? '#007AFF' : '#8E8E93'}
                                />
                                <Text
                                    style={[styles.tabText, tab.active && styles.tabTextActive]}
                                    numberOfLines={1}
                                >
                                    {tab.text}
                                </Text>
                            </View>
                        ))}
                    </View>
                );
            }
        }

        // Generic text result
        return (
            <View style={styles.container}>
                <Text style={styles.resultText} numberOfLines={3}>{raw}</Text>
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
    headerText: {
        fontSize: 13,
        color: theme.colors.text,
        fontWeight: '500',
        marginBottom: 4,
    },
    tabRow: {
        flexDirection: 'row',
        alignItems: 'center',
        gap: 6,
        paddingVertical: 3,
    },
    tabText: {
        fontSize: 13,
        color: theme.colors.textSecondary,
        flex: 1,
    },
    tabTextActive: {
        color: theme.colors.text,
        fontWeight: '500',
    },
    resultText: {
        fontSize: 13,
        color: theme.colors.textSecondary,
        lineHeight: 18,
    },
}));
