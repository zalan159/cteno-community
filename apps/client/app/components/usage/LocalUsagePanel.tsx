import React, { useState, useEffect, useCallback } from 'react';
import { View, ActivityIndicator, ScrollView, Pressable } from 'react-native';
import { Text } from '@/components/StyledText';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { ItemGroup } from '@/components/ItemGroup';
import { UsageBar } from './UsageBar';
import { machineGetLocalUsage, LocalUsageSummary } from '@/sync/ops';
import { useAllMachines } from '@/sync/storage';
import { isMachineOnline } from '@/utils/machineUtils';
import { Ionicons } from '@expo/vector-icons';
import { t } from '@/text';
import type { Machine } from '@/sync/storageTypes';

type TimePeriod = 'today' | '7days' | '30days';

const styles = StyleSheet.create((theme) => ({
    container: {
        flex: 1,
    },
    machineSelector: {
        flexDirection: 'row',
        paddingHorizontal: 16,
        paddingTop: 12,
        gap: 8,
        flexWrap: 'wrap',
    },
    machineButton: {
        paddingVertical: 6,
        paddingHorizontal: 12,
        borderRadius: 16,
        backgroundColor: theme.colors.surface,
        flexDirection: 'row',
        alignItems: 'center',
        gap: 6,
    },
    machineButtonActive: {
        backgroundColor: '#007AFF',
    },
    machineText: {
        fontSize: 13,
        color: theme.colors.text,
        fontWeight: '500',
    },
    machineTextActive: {
        color: '#FFFFFF',
    },
    onlineDot: {
        width: 6,
        height: 6,
        borderRadius: 3,
        backgroundColor: '#34C759',
    },
    offlineDot: {
        width: 6,
        height: 6,
        borderRadius: 3,
        backgroundColor: theme.colors.textSecondary,
    },
    periodSelector: {
        flexDirection: 'row',
        padding: 16,
        gap: 8,
    },
    periodButton: {
        flex: 1,
        paddingVertical: 8,
        paddingHorizontal: 12,
        borderRadius: 8,
        backgroundColor: theme.colors.surface,
        alignItems: 'center',
    },
    periodButtonActive: {
        backgroundColor: '#007AFF',
    },
    periodText: {
        fontSize: 14,
        color: theme.colors.text,
        fontWeight: '500',
    },
    periodTextActive: {
        color: '#FFFFFF',
    },
    statsContainer: {
        padding: 16,
        backgroundColor: theme.colors.surface,
        margin: 16,
        borderRadius: 12,
        gap: 12,
    },
    statRow: {
        flexDirection: 'row',
        justifyContent: 'space-between',
        alignItems: 'center',
    },
    statLabel: {
        fontSize: 16,
        color: theme.colors.text,
    },
    statValue: {
        fontSize: 20,
        fontWeight: '700',
        color: theme.colors.text,
    },
    loadingContainer: {
        flex: 1,
        justifyContent: 'center',
        alignItems: 'center',
        padding: 32,
    },
    emptyContainer: {
        padding: 32,
        alignItems: 'center',
    },
    emptyText: {
        fontSize: 14,
        color: theme.colors.textSecondary,
        textAlign: 'center',
    },
    descriptionText: {
        fontSize: 13,
        color: theme.colors.textSecondary,
        textAlign: 'center',
        paddingHorizontal: 16,
        paddingBottom: 8,
    },
    dayBarContainer: {
        padding: 16,
        gap: 4,
    },
    dayRow: {
        flexDirection: 'row',
        alignItems: 'center',
        gap: 8,
    },
    dayLabel: {
        fontSize: 12,
        color: theme.colors.textSecondary,
        width: 72,
    },
    dayBarOuter: {
        flex: 1,
        height: 8,
        backgroundColor: theme.colors.divider,
        borderRadius: 4,
        overflow: 'hidden',
    },
    dayBarFill: {
        height: '100%',
        borderRadius: 4,
        backgroundColor: '#007AFF',
    },
    dayValue: {
        fontSize: 12,
        color: theme.colors.textSecondary,
        width: 56,
        textAlign: 'right',
    },
    sectionTitle: {
        fontSize: 18,
        fontWeight: '600',
        color: theme.colors.text,
        marginHorizontal: 16,
        marginBottom: 8,
        marginTop: 16,
    },
}));

const formatTokens = (tokens: number): string => {
    if (tokens >= 1000000) {
        return `${(tokens / 1000000).toFixed(2)}M`;
    } else if (tokens >= 1000) {
        return `${(tokens / 1000).toFixed(1)}K`;
    }
    return tokens.toLocaleString();
};

function getMachineDisplayName(machine: Machine): string {
    return machine.metadata?.displayName
        || machine.metadata?.host
        || machine.id.slice(-8);
}

export const LocalUsagePanel: React.FC = () => {
    const { theme } = useUnistyles();
    const machines = useAllMachines();
    const onlineMachines = machines.filter(m => isMachineOnline(m));
    const allCandidates = onlineMachines.length > 0 ? onlineMachines : machines.slice(0, 1);

    const [selectedMachineId, setSelectedMachineId] = useState<string | null>(null);
    const [period, setPeriod] = useState<TimePeriod>('7days');
    const [loading, setLoading] = useState(false);
    const [summary, setSummary] = useState<LocalUsageSummary | null>(null);

    // Auto-select first machine
    const machineId = selectedMachineId
        ?? allCandidates[0]?.id
        ?? null;

    const loadData = useCallback(async () => {
        if (!machineId) return;
        setLoading(true);
        try {
            const data = await machineGetLocalUsage(machineId, period);
            setSummary(data);
        } catch (e) {
            console.error('[LocalUsagePanel] load error:', e);
            setSummary(null);
        } finally {
            setLoading(false);
        }
    }, [machineId, period]);

    useEffect(() => {
        loadData();
    }, [loadData]);

    const periodLabels: Record<TimePeriod, string> = {
        'today': t('usage.today'),
        '7days': t('usage.last7Days'),
        '30days': t('usage.last30Days'),
    };

    if (!machineId) {
        return (
            <View style={styles.emptyContainer}>
                <Ionicons name="desktop-outline" size={48} color={theme.colors.textSecondary} />
                <Text style={styles.emptyText}>{t('usage.noMachineOnline')}</Text>
            </View>
        );
    }

    if (loading) {
        return (
            <View style={styles.loadingContainer}>
                <ActivityIndicator size="large" color="#007AFF" />
            </View>
        );
    }

    const hasData = summary && (summary.totalInput > 0 || summary.totalOutput > 0);

    return (
        <ScrollView style={styles.container}>
            {/* Machine Selector (only if multiple machines) */}
            {machines.length > 1 && (
                <View style={styles.machineSelector}>
                    {machines.map((m) => {
                        const isOnline = isMachineOnline(m);
                        const isSelected = m.id === machineId;
                        return (
                            <Pressable
                                key={m.id}
                                style={[styles.machineButton, isSelected && styles.machineButtonActive]}
                                onPress={() => setSelectedMachineId(m.id)}
                                disabled={!isOnline}
                            >
                                <View style={isOnline ? styles.onlineDot : styles.offlineDot} />
                                <Text style={[styles.machineText, isSelected && styles.machineTextActive]}>
                                    {getMachineDisplayName(m)}
                                </Text>
                            </Pressable>
                        );
                    })}
                </View>
            )}

            {/* Period Selector */}
            <View style={styles.periodSelector}>
                {(['today', '7days', '30days'] as TimePeriod[]).map((p) => (
                    <Pressable
                        key={p}
                        style={[styles.periodButton, period === p && styles.periodButtonActive]}
                        onPress={() => setPeriod(p)}
                    >
                        <Text style={[styles.periodText, period === p && styles.periodTextActive]}>
                            {periodLabels[p]}
                        </Text>
                    </Pressable>
                ))}
            </View>

            <Text style={styles.descriptionText}>{t('usage.localDescription')}</Text>

            {!hasData ? (
                <View style={styles.emptyContainer}>
                    <Ionicons name="analytics-outline" size={48} color={theme.colors.textSecondary} />
                    <Text style={styles.emptyText}>{t('usage.noLocalData')}</Text>
                </View>
            ) : (
                <>
                    {/* Token Overview */}
                    <View style={styles.statsContainer}>
                        <View style={styles.statRow}>
                            <Text style={styles.statLabel}>{t('usage.totalInput')}</Text>
                            <Text style={styles.statValue}>{formatTokens(summary!.totalInput)}</Text>
                        </View>
                        <View style={styles.statRow}>
                            <Text style={styles.statLabel}>{t('usage.totalOutput')}</Text>
                            <Text style={styles.statValue}>{formatTokens(summary!.totalOutput)}</Text>
                        </View>
                        {summary!.totalCacheRead > 0 && (
                            <View style={styles.statRow}>
                                <Text style={styles.statLabel}>{t('usage.cacheRead')}</Text>
                                <Text style={styles.statValue}>{formatTokens(summary!.totalCacheRead)}</Text>
                            </View>
                        )}
                        {summary!.totalCacheCreation > 0 && (
                            <View style={styles.statRow}>
                                <Text style={styles.statLabel}>{t('usage.cacheCreation')}</Text>
                                <Text style={styles.statValue}>{formatTokens(summary!.totalCacheCreation)}</Text>
                            </View>
                        )}
                    </View>

                    {/* By Profile */}
                    {summary!.byProfile.length > 0 && (
                        <ItemGroup title={t('usage.byProfile')}>
                            <View style={{ padding: 16 }}>
                                {summary!.byProfile.map((profile) => (
                                    <UsageBar
                                        key={profile.profileId}
                                        label={profile.profileName}
                                        value={profile.totalTokens}
                                        maxValue={Math.max(...summary!.byProfile.map(p => p.totalTokens), 1)}
                                        color="#007AFF"
                                    />
                                ))}
                            </View>
                        </ItemGroup>
                    )}

                    {/* By Day Timeline */}
                    {summary!.byDay.length > 1 && (
                        <>
                            <Text style={styles.sectionTitle}>{t('usage.usageOverTime')}</Text>
                            <View style={styles.dayBarContainer}>
                                {summary!.byDay.map((day) => {
                                    const dayTotal = day.input + day.output;
                                    const maxDayTotal = Math.max(
                                        ...summary!.byDay.map(d => d.input + d.output),
                                        1
                                    );
                                    const pct = (dayTotal / maxDayTotal) * 100;
                                    return (
                                        <View key={day.date} style={styles.dayRow}>
                                            <Text style={styles.dayLabel}>{day.date.slice(5)}</Text>
                                            <View style={styles.dayBarOuter}>
                                                <View
                                                    style={[
                                                        styles.dayBarFill,
                                                        { width: `${Math.min(pct, 100)}%` },
                                                    ]}
                                                />
                                            </View>
                                            <Text style={styles.dayValue}>{formatTokens(dayTotal)}</Text>
                                        </View>
                                    );
                                })}
                            </View>
                        </>
                    )}
                </>
            )}
        </ScrollView>
    );
};
