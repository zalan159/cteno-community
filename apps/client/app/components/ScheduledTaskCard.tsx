import React from 'react';
import { View, Pressable } from 'react-native';
import { useUnistyles } from 'react-native-unistyles';
import { Typography } from '@/constants/Typography';
import { Text } from '@/components/StyledText';
import { t } from '@/text';
import type { ScheduledTask, ScheduleType } from '../sync/ops';

interface ScheduledTaskCardProps {
    task: ScheduledTask;
    onViewDetails: () => void;
    onToggle?: (enabled: boolean) => void;
    onDelete?: () => void;
}

export const ScheduledTaskCard: React.FC<ScheduledTaskCardProps> = ({
    task,
    onViewDetails,
    onToggle,
    onDelete,
}) => {
    const { theme } = useUnistyles();
    const statusConfig = getStatusConfig(task);

    return (
        <View style={{
            backgroundColor: theme.colors.surfaceHighest,
            borderRadius: 12,
            borderWidth: 2,
            borderColor: statusConfig.borderColor,
            padding: 16,
            marginVertical: 8,
            marginHorizontal: 12,
        }}>
            {/* Header */}
            <View style={{ flexDirection: 'row', alignItems: 'center', marginBottom: 8 }}>
                <Text style={{ fontSize: 20, marginRight: 8 }}>{statusConfig.icon}</Text>
                <Text style={{
                    fontSize: 16,
                    color: theme.colors.text,
                    flex: 1,
                    ...Typography.default('semiBold'),
                }} numberOfLines={1}>{task.name}</Text>
            </View>

            {/* Schedule description */}
            <View style={{ flexDirection: 'row', alignItems: 'center', marginBottom: 4 }}>
                <Text style={{
                    fontSize: 14,
                    color: statusConfig.color,
                    ...Typography.default('semiBold'),
                }}>
                    {statusConfig.text}
                </Text>
                <Text style={{
                    fontSize: 14,
                    color: theme.colors.textSecondary,
                    ...Typography.default(),
                }}> | {formatSchedule(task.schedule, task.timezone)}</Text>
            </View>

            {/* Next run time */}
            {task.enabled && task.state.next_run_at && (
                <Text style={{
                    fontSize: 12,
                    color: theme.colors.textSecondary,
                    marginBottom: 4,
                    fontStyle: 'italic',
                    ...Typography.default(),
                }}>
                    {formatNextRun(task.state.next_run_at)}
                </Text>
            )}

            {/* Last run status */}
            {task.state.last_run_at && (
                <Text style={{
                    fontSize: 12,
                    color: theme.colors.textSecondary,
                    marginBottom: 4,
                    ...Typography.default(),
                }}>
                    {task.state.last_status === 'success' ? t('settingsScheduledTasks.ok') : t('settingsScheduledTasks.fail')}{' '}
                    {formatRelativeTime(task.state.last_run_at)}
                    {task.state.total_runs > 0 && ` (${t('settingsScheduledTasks.runCount', { count: task.state.total_runs })})`}
                </Text>
            )}

            {/* Last result summary */}
            {task.state.last_result_summary && (
                <Text style={{
                    fontSize: 13,
                    color: task.state.last_status === 'success' ? '#34C759' : theme.colors.textSecondary,
                    marginTop: 4,
                    marginBottom: 8,
                    ...Typography.default(),
                }} numberOfLines={2}>
                    {task.state.last_result_summary}
                </Text>
            )}

            {/* Actions */}
            <View style={{ flexDirection: 'row', gap: 8, marginTop: 8 }}>
                <Pressable
                    onPress={onViewDetails}
                    style={({ pressed }) => ({
                        flex: 1,
                        backgroundColor: pressed ? theme.colors.surfacePressed : theme.colors.divider,
                        paddingVertical: 8,
                        paddingHorizontal: 12,
                        borderRadius: 8,
                        alignItems: 'center' as const,
                    })}
                >
                    <Text style={{
                        fontSize: 13,
                        color: theme.colors.text,
                        ...Typography.default('semiBold'),
                    }}>{t('settingsScheduledTasks.details')}</Text>
                </Pressable>

                {onToggle && (
                    <Pressable
                        onPress={() => onToggle(!task.enabled)}
                        style={({ pressed }) => ({
                            flex: 1,
                            backgroundColor: pressed ? theme.colors.surfacePressed : theme.colors.divider,
                            paddingVertical: 8,
                            paddingHorizontal: 12,
                            borderRadius: 8,
                            alignItems: 'center' as const,
                        })}
                    >
                        <Text style={{
                            fontSize: 13,
                            color: task.enabled ? '#FF9500' : '#34C759',
                            ...Typography.default('semiBold'),
                        }}>
                            {task.enabled ? t('settingsScheduledTasks.disable') : t('settingsScheduledTasks.enable')}
                        </Text>
                    </Pressable>
                )}

                {onDelete && (
                    <Pressable
                        onPress={onDelete}
                        style={({ pressed }) => ({
                            flex: 1,
                            backgroundColor: pressed ? theme.colors.surfacePressed : theme.colors.divider,
                            paddingVertical: 8,
                            paddingHorizontal: 12,
                            borderRadius: 8,
                            alignItems: 'center' as const,
                        })}
                    >
                        <Text style={{
                            fontSize: 13,
                            color: '#FF3B30',
                            ...Typography.default('semiBold'),
                        }}>{t('common.delete')}</Text>
                    </Pressable>
                )}
            </View>
        </View>
    );
};

function getStatusConfig(task: ScheduledTask) {
    if (task.state.running_since) {
        return {
            icon: '\u{1F504}',
            text: t('settingsScheduledTasks.running'),
            color: '#007AFF',
            borderColor: '#007AFF',
        };
    }
    if (!task.enabled) {
        return {
            icon: '\u{23F8}\uFE0F',
            text: t('settingsScheduledTasks.disabled'),
            color: '#8E8E93',
            borderColor: '#8E8E93',
        };
    }
    if (task.state.last_status === 'failed' && task.state.consecutive_errors > 0) {
        return {
            icon: '\u{26A0}\uFE0F',
            text: t('settingsScheduledTasks.errors', { count: task.state.consecutive_errors }),
            color: '#FF9500',
            borderColor: '#FF9500',
        };
    }
    if (task.delete_after_run) {
        return {
            icon: '\u{1F552}',
            text: t('settingsScheduledTasks.oneTime'),
            color: '#AF52DE',
            borderColor: '#AF52DE',
        };
    }
    return {
        icon: '\u{2705}',
        text: t('settingsScheduledTasks.active'),
        color: '#34C759',
        borderColor: '#34C759',
    };
}

function formatSchedule(schedule: ScheduleType, timezone?: string): string {
    if (schedule.kind === 'at') return t('settingsScheduledTasks.scheduleOnce', { time: formatDatetime(schedule.at, timezone) });
    if (schedule.kind === 'every') {
        const s = schedule.every_seconds;
        if (s < 60) return t('settingsScheduledTasks.everySeconds', { count: s });
        if (s < 3600) return t('settingsScheduledTasks.everyMinutes', { count: Math.round(s / 60) });
        if (s < 86400) return t('settingsScheduledTasks.everyHours', { count: Math.round(s / 3600) });
        return t('settingsScheduledTasks.everyDays', { count: Math.round(s / 86400) });
    }
    if (schedule.kind === 'cron') return t('settingsScheduledTasks.scheduleCron', { expr: schedule.expr });
    return t('status.unknown');
}

function formatDatetime(isoStr: string, timezone?: string): string {
    try {
        const date = new Date(isoStr);
        return date.toLocaleString(undefined, {
            month: 'numeric',
            day: 'numeric',
            hour: '2-digit',
            minute: '2-digit',
            timeZone: timezone,
        });
    } catch {
        return isoStr;
    }
}

function formatNextRun(timestamp: number): string {
    const now = Date.now();
    const diff = timestamp - now;
    if (diff < 0) return t('settingsScheduledTasks.overdue');
    if (diff < 60000) return t('settingsScheduledTasks.nextUnderMinute');
    if (diff < 3600000) return t('settingsScheduledTasks.nextMinutes', { count: Math.round(diff / 60000) });
    if (diff < 86400000) return t('settingsScheduledTasks.nextHours', { count: Math.round(diff / 3600000) });

    const date = new Date(timestamp);
    return t('settingsScheduledTasks.nextAt', {
        time: date.toLocaleString(undefined, {
        month: 'numeric',
        day: 'numeric',
        hour: '2-digit',
        minute: '2-digit',
    })});
}

function formatRelativeTime(timestamp: number): string {
    const now = Date.now();
    const diff = now - timestamp;
    if (diff < 60000) return t('time.justNow');
    if (diff < 3600000) return t('settingsScheduledTasks.minutesAgo', { count: Math.round(diff / 60000) });
    if (diff < 86400000) return t('settingsScheduledTasks.hoursAgo', { count: Math.round(diff / 3600000) });
    return t('settingsScheduledTasks.daysAgo', { count: Math.round(diff / 86400000) });
}
