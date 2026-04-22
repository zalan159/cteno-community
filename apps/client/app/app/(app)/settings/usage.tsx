import React, { useState } from 'react';
import { View, Pressable, Platform } from 'react-native';
import { Text } from '@/components/StyledText';
import { StyleSheet } from 'react-native-unistyles';
import { ProxyUsagePanel } from '@/components/usage/ProxyUsagePanel';
import { LocalUsagePanel } from '@/components/usage/LocalUsagePanel';
import { useAuth } from '@/auth/AuthContext';
import { t } from '@/text';

type UsageTab = 'local' | 'proxy';

const styles = StyleSheet.create((theme) => ({
    container: {
        flex: 1,
    },
    tabBar: {
        flexDirection: 'row',
        padding: 16,
        paddingBottom: 0,
        gap: 8,
    },
    tabButton: {
        flex: 1,
        paddingVertical: 8,
        paddingHorizontal: 12,
        borderRadius: 8,
        backgroundColor: theme.colors.surface,
        alignItems: 'center',
    },
    tabButtonActive: {
        backgroundColor: '#007AFF',
    },
    tabText: {
        fontSize: 14,
        color: theme.colors.text,
        fontWeight: '500',
    },
    tabTextActive: {
        color: '#FFFFFF',
    },
    content: {
        flex: 1,
    },
}));

export default function UsageSettingsScreen() {
    const auth = useAuth();
    const localOnlyMode = !auth.credentials?.token?.trim();
    const [tab, setTab] = useState<UsageTab>(localOnlyMode ? 'local' : 'proxy');

    if (localOnlyMode) {
        return (
            <View style={styles.container}>
                <View style={styles.content}>
                    <LocalUsagePanel />
                </View>
            </View>
        );
    }

    return (
        <View style={styles.container}>
            <View style={styles.tabBar}>
                <Pressable
                    style={[styles.tabButton, tab === 'local' && styles.tabButtonActive]}
                    onPress={() => setTab('local')}
                >
                    <Text style={[styles.tabText, tab === 'local' && styles.tabTextActive]}>
                        {t('usage.localUsage')}
                    </Text>
                </Pressable>
                <Pressable
                    style={[styles.tabButton, tab === 'proxy' && styles.tabButtonActive]}
                    onPress={() => setTab('proxy')}
                >
                    <Text style={[styles.tabText, tab === 'proxy' && styles.tabTextActive]}>
                        {t('usage.proxyUsage')}
                    </Text>
                </Pressable>
            </View>
            <View style={styles.content}>
                {tab === 'local' ? <LocalUsagePanel /> : <ProxyUsagePanel />}
            </View>
        </View>
    );
}
