import React, { useMemo, useState } from 'react';
import { Pressable, ScrollView, View } from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';

import { ItemGroup } from '@/components/ItemGroup';
import { Text } from '@/components/StyledText';
import { buildLocalProxyUsageSummary, type LocalProxyUsagePeriod } from '@/sync/localProxyUsage';
import { useAllSessions, useLocalProxyUsage } from '@/sync/storage';
import { t } from '@/text';

import { UsageBar } from './UsageBar';

type TimePeriod = LocalProxyUsagePeriod;

const styles = StyleSheet.create((theme) => ({
    container: {
        flex: 1,
    },
    periodSelector: {
        flexDirection: 'row',
        paddingHorizontal: 16,
        paddingTop: 16,
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
    totalCostCard: {
        margin: 16,
        marginTop: 8,
        padding: 16,
        backgroundColor: theme.colors.surface,
        borderRadius: 12,
        flexDirection: 'row',
        justifyContent: 'space-between',
        alignItems: 'center',
    },
    totalCostLabel: {
        fontSize: 16,
        color: theme.colors.text,
    },
    totalCostValue: {
        fontSize: 20,
        fontWeight: '700',
        color: '#FF9500',
    },
    statLabel: {
        fontSize: 12,
        color: theme.colors.textSecondary,
        marginTop: 2,
    },
    statGrid: {
        margin: 16,
        marginTop: 0,
        padding: 16,
        backgroundColor: theme.colors.surface,
        borderRadius: 12,
        gap: 12,
    },
    statRow: {
        flexDirection: 'row',
        justifyContent: 'space-between',
        alignItems: 'center',
    },
    statRowLabel: {
        fontSize: 14,
        color: theme.colors.textSecondary,
    },
    statRowValue: {
        fontSize: 16,
        fontWeight: '600',
        color: theme.colors.text,
    },
    sectionTitle: {
        fontSize: 18,
        fontWeight: '600',
        color: theme.colors.text,
        marginHorizontal: 16,
        marginBottom: 8,
        marginTop: 16,
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
        backgroundColor: '#FF9500',
    },
    dayValue: {
        fontSize: 12,
        color: theme.colors.textSecondary,
        width: 64,
        textAlign: 'right',
    },
    ledgerItem: {
        flexDirection: 'row',
        justifyContent: 'space-between',
        alignItems: 'center',
        paddingVertical: 10,
        paddingHorizontal: 16,
        borderBottomWidth: 0.5,
        borderBottomColor: theme.colors.divider,
    },
    ledgerLeft: {
        flex: 1,
        gap: 2,
    },
    ledgerModel: {
        fontSize: 14,
        color: theme.colors.text,
    },
    ledgerTime: {
        fontSize: 12,
        color: theme.colors.textSecondary,
    },
    ledgerAmount: {
        fontSize: 14,
        fontWeight: '600',
        color: '#FF3B30',
    },
    emptyContainer: {
        padding: 32,
        alignItems: 'center',
    },
    emptyText: {
        fontSize: 14,
        color: theme.colors.textSecondary,
        textAlign: 'center',
        marginTop: 8,
    },
}));

const formatYuan = (value: number): string => `¥${value.toFixed(4)}`;
const formatTokens = (value: number): string => {
    if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
    if (value >= 1_000) return `${(value / 1_000).toFixed(1)}K`;
    return String(value);
};

function formatTimestamp(timestamp: number): string {
    const date = new Date(timestamp);
    const mm = String(date.getMonth() + 1).padStart(2, '0');
    const dd = String(date.getDate()).padStart(2, '0');
    const hh = String(date.getHours()).padStart(2, '0');
    const min = String(date.getMinutes()).padStart(2, '0');
    return `${mm}-${dd} ${hh}:${min}`;
}

function getSessionLabel(sessionId: string, sessionLabels: Map<string, string>): string {
    return sessionLabels.get(sessionId) ?? `Session ${sessionId.slice(-6)}`;
}

export const ProxyUsagePanel: React.FC = () => {
    const { theme } = useUnistyles();
    const [period, setPeriod] = useState<TimePeriod>('7days');
    const localProxyUsage = useLocalProxyUsage();
    const sessions = useAllSessions();

    const summary = useMemo(
        () => buildLocalProxyUsageSummary(localProxyUsage, period),
        [localProxyUsage, period],
    );
    const sessionLabels = useMemo(() => {
        return new Map(
            sessions.map((session) => [
                session.id,
                session.metadata?.path
                    ?? session.metadata?.proxyModelId
                    ?? session.metadata?.host
                    ?? `Session ${session.id.slice(-6)}`,
            ]),
        );
    }, [sessions]);
    const bySession = useMemo(() => {
        const grouped = new Map<string, { costYuan: number; tokens: number }>();
        for (const record of summary.records) {
            const current = grouped.get(record.sessionId);
            if (current) {
                current.costYuan += record.totalCostYuan;
                current.tokens += record.totalTokens;
            } else {
                grouped.set(record.sessionId, {
                    costYuan: record.totalCostYuan,
                    tokens: record.totalTokens,
                });
            }
        }
        return Array.from(grouped.entries())
            .map(([sessionId, value]) => ({
                sessionId,
                label: getSessionLabel(sessionId, sessionLabels),
                ...value,
            }))
            .sort((a, b) => b.costYuan - a.costYuan);
    }, [sessionLabels, summary.records]);

    const periodLabels: Record<TimePeriod, string> = {
        today: t('usage.today'),
        '7days': t('usage.last7Days'),
        '30days': t('usage.last30Days'),
    };
    const maxSessionCost = Math.max(...bySession.map((entry) => entry.costYuan), 0.0001);

    return (
        <ScrollView style={styles.container}>
            <View style={styles.periodSelector}>
                {(['today', '7days', '30days'] as TimePeriod[]).map((candidate) => (
                    <Pressable
                        key={candidate}
                        style={[styles.periodButton, period === candidate && styles.periodButtonActive]}
                        onPress={() => setPeriod(candidate)}
                    >
                        <Text style={[styles.periodText, period === candidate && styles.periodTextActive]}>
                            {periodLabels[candidate]}
                        </Text>
                    </Pressable>
                ))}
            </View>

            {summary.records.length > 0 && (
                <>
                    <View style={styles.totalCostCard}>
                        <View>
                            <Text style={styles.totalCostLabel}>{t('usage.periodCost')}</Text>
                            <Text style={styles.statLabel}>{summary.requestCount} requests</Text>
                        </View>
                        <Text style={styles.totalCostValue}>{formatYuan(summary.totalCostYuan)}</Text>
                    </View>

                    <View style={styles.statGrid}>
                        <View style={styles.statRow}>
                            <Text style={styles.statRowLabel}>{t('usage.totalTokens')}</Text>
                            <Text style={styles.statRowValue}>{formatTokens(summary.totalTokens)}</Text>
                        </View>
                        <View style={styles.statRow}>
                            <Text style={styles.statRowLabel}>{t('usage.totalInput')}</Text>
                            <Text style={styles.statRowValue}>{formatTokens(summary.totalInputTokens)}</Text>
                        </View>
                        <View style={styles.statRow}>
                            <Text style={styles.statRowLabel}>{t('usage.totalOutput')}</Text>
                            <Text style={styles.statRowValue}>{formatTokens(summary.totalOutputTokens)}</Text>
                        </View>
                        {summary.totalCacheReadTokens > 0 && (
                            <View style={styles.statRow}>
                                <Text style={styles.statRowLabel}>{t('usage.cacheRead')}</Text>
                                <Text style={styles.statRowValue}>{formatTokens(summary.totalCacheReadTokens)}</Text>
                            </View>
                        )}
                        {summary.totalCacheCreationTokens > 0 && (
                            <View style={styles.statRow}>
                                <Text style={styles.statRowLabel}>{t('usage.cacheCreation')}</Text>
                                <Text style={styles.statRowValue}>{formatTokens(summary.totalCacheCreationTokens)}</Text>
                            </View>
                        )}
                    </View>
                </>
            )}

            {bySession.length > 0 && (
                <ItemGroup title={t('usage.byMachine')}>
                    <View style={{ padding: 16 }}>
                        {bySession.map((entry) => (
                            <UsageBar
                                key={entry.sessionId}
                                label={entry.label}
                                value={entry.costYuan}
                                maxValue={maxSessionCost}
                                color="#5856D6"
                                formatValue={(value) => `${formatYuan(value)}  ${formatTokens(entry.tokens)}`}
                            />
                        ))}
                    </View>
                </ItemGroup>
            )}

            {summary.byDay.length > 1 && (
                <>
                    <Text style={styles.sectionTitle}>{t('usage.costOverTime')}</Text>
                    <View style={styles.dayBarContainer}>
                        {summary.byDay.map((day) => {
                            const maxDayCost = Math.max(...summary.byDay.map((entry) => entry.costYuan), 0.0001);
                            const pct = (day.costYuan / maxDayCost) * 100;
                            return (
                                <View key={day.date} style={styles.dayRow}>
                                    <Text style={styles.dayLabel}>{day.date.slice(5)}</Text>
                                    <View style={styles.dayBarOuter}>
                                        <View style={[styles.dayBarFill, { width: `${Math.min(pct, 100)}%` }]} />
                                    </View>
                                    <Text style={styles.dayValue}>{formatYuan(day.costYuan)}</Text>
                                </View>
                            );
                        })}
                    </View>
                </>
            )}

            {summary.records.length > 0 && (
                <>
                    <Text style={styles.sectionTitle}>{t('usage.transactionHistory')}</Text>
                    {summary.records.slice(0, 20).map((record) => (
                        <View key={record.key} style={styles.ledgerItem}>
                            <View style={styles.ledgerLeft}>
                                <Text style={styles.ledgerModel}>
                                    {getSessionLabel(record.sessionId, sessionLabels)}
                                </Text>
                                <Text style={styles.ledgerTime}>
                                    {formatTimestamp(record.timestamp)}  {formatTokens(record.totalTokens)} tokens
                                </Text>
                            </View>
                            <Text style={styles.ledgerAmount}>{formatYuan(record.totalCostYuan)}</Text>
                        </View>
                    ))}
                </>
            )}

            {summary.records.length === 0 && (
                <View style={styles.emptyContainer}>
                    <Ionicons name="wallet-outline" size={48} color={theme.colors.textSecondary} />
                    <Text style={styles.emptyText}>{t('usage.noProxyData')}</Text>
                </View>
            )}
        </ScrollView>
    );
};
