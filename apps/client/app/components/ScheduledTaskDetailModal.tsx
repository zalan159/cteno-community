import React, { useState, useEffect, useMemo, useCallback } from 'react';
import { View, ScrollView, Pressable, Modal, TextInput, useWindowDimensions } from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { useUnistyles } from 'react-native-unistyles';
import { Typography } from '@/constants/Typography';
import { Text } from '@/components/StyledText';
import { t } from '@/text';
import type { ScheduledTask, ScheduleType, UpdateScheduledTaskInput } from '../sync/ops';

interface ScheduledTaskDetailModalProps {
    task: ScheduledTask | null;
    visible: boolean;
    onClose: () => void;
    onToggle?: (id: string, enabled: boolean) => Promise<void>;
    onDelete?: (id: string) => Promise<void>;
    onUpdate?: (id: string, updates: UpdateScheduledTaskInput) => Promise<void>;
}

export const ScheduledTaskDetailModal: React.FC<ScheduledTaskDetailModalProps> = ({
    task,
    visible,
    onClose,
    onToggle,
    onDelete,
    onUpdate,
}) => {
    const { theme } = useUnistyles();
    const { width: windowWidth, height: windowHeight } = useWindowDimensions();
    const [confirming, setConfirming] = useState(false);
    const modalWidth = Math.min(windowWidth * 0.9, 600);
    const modalMaxHeight = windowHeight * 0.8;
    const [isEditing, setIsEditing] = useState(false);
    const [editName, setEditName] = useState('');
    const [editPrompt, setEditPrompt] = useState('');
    const [editTimezone, setEditTimezone] = useState('');
    const [editCron, setEditCron] = useState('');
    const [editEveryValue, setEditEveryValue] = useState('');
    const [editEveryUnit, setEditEveryUnit] = useState<'min' | 'hr' | 'day'>('min');
    const [editAt, setEditAt] = useState('');
    const [saving, setSaving] = useState(false);

    // Reset edit state when task changes or modal closes
    useEffect(() => {
        if (task && visible) {
            setEditName(task.name);
            setEditPrompt(task.task_prompt);
            setEditTimezone(task.timezone);
            setEditCron(task.schedule.kind === 'cron' ? task.schedule.expr : '');
            if (task.schedule.kind === 'every') {
                const s = task.schedule.every_seconds;
                if (s >= 86400 && s % 86400 === 0) {
                    setEditEveryValue(String(s / 86400));
                    setEditEveryUnit('day');
                } else if (s >= 3600 && s % 3600 === 0) {
                    setEditEveryValue(String(s / 3600));
                    setEditEveryUnit('hr');
                } else {
                    setEditEveryValue(String(Math.round(s / 60)));
                    setEditEveryUnit('min');
                }
            }
            if (task.schedule.kind === 'at') {
                setEditAt(task.schedule.at);
            }
        }
        if (!visible) {
            setIsEditing(false);
            setConfirming(false);
            setSaving(false);
        }
    }, [task, visible]);

    if (!task) return null;

    const formatTime = (timestamp: number) => {
        return new Date(timestamp).toLocaleString(undefined, { timeZone: task.timezone });
    };

    const formatScheduleDetail = (schedule: ScheduleType): string => {
        if (schedule.kind === 'at') return t('settingsScheduledTasks.scheduleOneTime', { time: schedule.at });
        if (schedule.kind === 'every') {
            const s = schedule.every_seconds;
            let interval = t('settingsScheduledTasks.seconds', { count: s });
            if (s >= 86400) interval = t('settingsScheduledTasks.days', { count: Math.round(s / 86400) });
            else if (s >= 3600) interval = t('settingsScheduledTasks.hours', { count: Math.round(s / 3600) });
            else if (s >= 60) interval = t('settingsScheduledTasks.minutes', { count: Math.round(s / 60) });
            return t('settingsScheduledTasks.scheduleRecurring', { interval, anchor: schedule.anchor || '' }).trim();
        }
        if (schedule.kind === 'cron') return t('settingsScheduledTasks.scheduleCron', { expr: schedule.expr });
        return t('status.unknown');
    };

    const handleDelete = async () => {
        if (!confirming) {
            setConfirming(true);
            return;
        }
        if (onDelete) {
            await onDelete(task.id);
            setConfirming(false);
            onClose();
        }
    };

    const handleToggle = async () => {
        if (onToggle) {
            await onToggle(task.id, !task.enabled);
        }
    };

    const buildEditedSchedule = (): ScheduleType | undefined => {
        if (task.schedule.kind === 'cron') {
            if (editCron !== task.schedule.expr) return { kind: 'cron', expr: editCron };
        } else if (task.schedule.kind === 'every') {
            const multiplier = editEveryUnit === 'day' ? 86400 : editEveryUnit === 'hr' ? 3600 : 60;
            const newSeconds = Math.max(1, Math.round(Number(editEveryValue) || 0)) * multiplier;
            if (newSeconds !== task.schedule.every_seconds) {
                return { kind: 'every', every_seconds: newSeconds, anchor: task.schedule.anchor };
            }
        } else if (task.schedule.kind === 'at') {
            if (editAt !== task.schedule.at) return { kind: 'at', at: editAt };
        }
        return undefined;
    };

    const handleSave = async () => {
        if (!onUpdate) return;
        setSaving(true);
        try {
            const updates: UpdateScheduledTaskInput = {};
            if (editName !== task.name) updates.name = editName;
            if (editPrompt !== task.task_prompt) updates.task_prompt = editPrompt;
            if (editTimezone !== task.timezone) updates.timezone = editTimezone;
            const newSchedule = buildEditedSchedule();
            if (newSchedule) updates.schedule = newSchedule;
            if (Object.keys(updates).length > 0) {
                await onUpdate(task.id, updates);
            }
            setIsEditing(false);
        } catch (err) {
            console.error('[ScheduledTasks] Failed to save task:', err);
        } finally {
            setSaving(false);
        }
    };

    const handleCancelEdit = () => {
        setEditName(task.name);
        setEditPrompt(task.task_prompt);
        setEditTimezone(task.timezone);
        setEditCron(task.schedule.kind === 'cron' ? task.schedule.expr : '');
        if (task.schedule.kind === 'every') {
            const s = task.schedule.every_seconds;
            if (s >= 86400 && s % 86400 === 0) { setEditEveryValue(String(s / 86400)); setEditEveryUnit('day'); }
            else if (s >= 3600 && s % 3600 === 0) { setEditEveryValue(String(s / 3600)); setEditEveryUnit('hr'); }
            else { setEditEveryValue(String(Math.round(s / 60))); setEditEveryUnit('min'); }
        }
        if (task.schedule.kind === 'at') setEditAt(task.schedule.at);
        setIsEditing(false);
    };

    const statusColor = task.state.last_status === 'success' ? '#34C759'
        : task.state.last_status === 'failed' ? '#FF3B30'
        : undefined;

    return (
        <Modal
            visible={visible}
            animationType="slide"
            transparent={true}
            onRequestClose={onClose}
        >
            <View style={{
                flex: 1,
                backgroundColor: 'rgba(0, 0, 0, 0.5)',
                justifyContent: 'center',
                alignItems: 'center',
            }}>
                <View style={{
                    backgroundColor: theme.colors.surface,
                    borderRadius: 14,
                    width: modalWidth,
                    maxHeight: modalMaxHeight,
                    overflow: 'hidden',
                    shadowColor: theme.colors.shadow.color,
                    shadowOffset: { width: 0, height: 2 },
                    shadowOpacity: 0.25,
                    shadowRadius: 4,
                    elevation: 5,
                }}>
                    {/* Header */}
                    <View style={{
                        flexDirection: 'row',
                        justifyContent: 'space-between',
                        alignItems: 'center',
                        paddingHorizontal: 20,
                        paddingTop: 20,
                        paddingBottom: 12,
                        borderBottomWidth: 0.5,
                        borderBottomColor: theme.colors.divider,
                    }}>
                        <Text style={{
                            fontSize: 17,
                            color: theme.colors.text,
                            ...Typography.default('semiBold'),
                        }}>
                            {isEditing ? t('settingsScheduledTasks.editTask') : t('settingsScheduledTasks.taskDetails')}
                        </Text>
                        <Pressable
                            onPress={onClose}
                            style={({ pressed }) => ({
                                padding: 4,
                                opacity: pressed ? 0.6 : 1,
                            })}
                        >
                            <Ionicons name="close" size={22} color={theme.colors.textSecondary} />
                        </Pressable>
                    </View>

                    <ScrollView style={{ flex: 1, padding: 20 }}>
                        {/* Basic Info */}
                        <View style={{ marginBottom: 20 }}>
                            <Text style={{
                                fontSize: 15,
                                color: theme.colors.text,
                                marginBottom: 12,
                                ...Typography.default('semiBold'),
                            }}>{t('settingsScheduledTasks.basicInfo')}</Text>

                            {isEditing ? (
                                <View style={{ marginBottom: 8 }}>
                                    <Text style={{
                                        fontSize: 14,
                                        color: theme.colors.textSecondary,
                                        marginBottom: 4,
                                        ...Typography.default(),
                                    }}>{t('settingsAccount.name')}</Text>
                                    <TextInput
                                        value={editName}
                                        onChangeText={setEditName}
                                        style={{
                                            backgroundColor: theme.colors.surfaceHighest,
                                            borderRadius: 8,
                                            padding: 10,
                                            fontSize: 14,
                                            color: theme.colors.text,
                                            ...Typography.default(),
                                        }}
                                        placeholderTextColor={theme.colors.textSecondary}
                                    />
                                </View>
                            ) : (
                                <InfoRow label={t('settingsAccount.name')} value={task.name} theme={theme} />
                            )}

                            <InfoRow
                                label={t('settingsAccount.status')}
                                value={task.enabled ? t('settingsScheduledTasks.enabled') : t('settingsScheduledTasks.disabled')}
                                valueColor={task.enabled ? '#34C759' : theme.colors.textSecondary}
                                theme={theme}
                            />

                            {isEditing ? (
                                <ScheduleEditor
                                    schedule={task.schedule}
                                    editCron={editCron}
                                    setEditCron={setEditCron}
                                    editEveryValue={editEveryValue}
                                    setEditEveryValue={setEditEveryValue}
                                    editEveryUnit={editEveryUnit}
                                    setEditEveryUnit={setEditEveryUnit}
                                    editAt={editAt}
                                    setEditAt={setEditAt}
                                    theme={theme}
                                />
                            ) : (
                                <InfoRow label={t('settingsScheduledTasks.schedule')} value={formatScheduleDetail(task.schedule)} theme={theme} />
                            )}

                            {isEditing ? (
                                <View style={{ marginBottom: 8 }}>
                                    <Text style={{
                                        fontSize: 14,
                                        color: theme.colors.textSecondary,
                                        marginBottom: 4,
                                        ...Typography.default(),
                                    }}>{t('settingsScheduledTasks.timezone')}</Text>
                                    <TextInput
                                        value={editTimezone}
                                        onChangeText={setEditTimezone}
                                        style={{
                                            backgroundColor: theme.colors.surfaceHighest,
                                            borderRadius: 8,
                                            padding: 10,
                                            fontSize: 14,
                                            color: theme.colors.text,
                                            ...Typography.default(),
                                        }}
                                        placeholderTextColor={theme.colors.textSecondary}
                                    />
                                </View>
                            ) : (
                                <InfoRow label={t('settingsScheduledTasks.timezone')} value={task.timezone} theme={theme} />
                            )}

                            <InfoRow label={t('settingsScheduledTasks.deleteAfterRun')} value={task.delete_after_run ? t('settingsScheduledTasks.yesAutoDelete') : t('settingsScheduledTasks.noRecurring')} theme={theme} />
                            <InfoRow label={t('settingsScheduledTasks.created')} value={formatTime(task.created_at)} theme={theme} />
                            <InfoRow label={t('settingsScheduledTasks.updated')} value={formatTime(task.updated_at)} theme={theme} />
                        </View>

                        {/* Task Prompt */}
                        <View style={{ marginBottom: 20 }}>
                            <Text style={{
                                fontSize: 15,
                                color: theme.colors.text,
                                marginBottom: 12,
                                ...Typography.default('semiBold'),
                            }}>{t('settingsScheduledTasks.taskPrompt')}</Text>

                            {isEditing ? (
                                <TextInput
                                    value={editPrompt}
                                    onChangeText={setEditPrompt}
                                    multiline
                                    style={{
                                        backgroundColor: theme.colors.surfaceHighest,
                                        borderRadius: 8,
                                        padding: 12,
                                        fontSize: 14,
                                        color: theme.colors.text,
                                        minHeight: 120,
                                        maxHeight: 200,
                                        textAlignVertical: 'top',
                                        ...Typography.default(),
                                    }}
                                    placeholderTextColor={theme.colors.textSecondary}
                                />
                            ) : (
                                <ScrollView
                                    style={{
                                        maxHeight: 200,
                                        backgroundColor: theme.colors.surfaceHighest,
                                        borderRadius: 8,
                                        padding: 12,
                                    }}
                                    nestedScrollEnabled={true}
                                >
                                    <Text style={{
                                        fontSize: 14,
                                        color: theme.colors.text,
                                        lineHeight: 20,
                                        ...Typography.default(),
                                    }}>{task.task_prompt}</Text>
                                </ScrollView>
                            )}
                        </View>

                        {/* Execution Stats */}
                        <View style={{ marginBottom: 20 }}>
                            <Text style={{
                                fontSize: 15,
                                color: theme.colors.text,
                                marginBottom: 12,
                                ...Typography.default('semiBold'),
                            }}>{t('settingsScheduledTasks.executionStats')}</Text>

                            <InfoRow label={t('settingsScheduledTasks.totalRuns')} value={`${task.state.total_runs}`} theme={theme} />
                            <InfoRow
                                label={t('settingsScheduledTasks.lastStatus')}
                                value={formatLastStatus(task.state.last_status)}
                                valueColor={statusColor}
                                theme={theme}
                            />
                            <InfoRow
                                label={t('settingsScheduledTasks.lastRun')}
                                value={task.state.last_run_at ? formatTime(task.state.last_run_at) : t('settingsScheduledTasks.none')}
                                theme={theme}
                            />
                            <InfoRow
                                label={t('settingsScheduledTasks.nextRun')}
                                value={task.state.next_run_at ? formatTime(task.state.next_run_at) : t('settingsScheduledTasks.none')}
                                theme={theme}
                            />
                            {task.state.consecutive_errors > 0 && (
                                <InfoRow
                                    label={t('settingsScheduledTasks.consecutiveErrors')}
                                    value={`${task.state.consecutive_errors}`}
                                    valueColor="#FF3B30"
                                    theme={theme}
                                />
                            )}
                        </View>

                        {/* Last Result */}
                        {task.state.last_result_summary && (
                            <View style={{ marginBottom: 20 }}>
                                <Text style={{
                                    fontSize: 15,
                                    color: theme.colors.text,
                                    marginBottom: 12,
                                    ...Typography.default('semiBold'),
                                }}>{t('settingsScheduledTasks.lastResult')}</Text>
                                <View style={{
                                    backgroundColor: theme.colors.surfaceHighest,
                                    borderRadius: 8,
                                    padding: 12,
                                }}>
                                    <Text style={{
                                        fontSize: 14,
                                        color: theme.colors.text,
                                        fontFamily: 'monospace',
                                        lineHeight: 20,
                                    }}>{task.state.last_result_summary}</Text>
                                </View>
                            </View>
                        )}
                    </ScrollView>

                    {/* Actions */}
                    <View style={{
                        flexDirection: 'row',
                        gap: 8,
                        padding: 16,
                        borderTopWidth: 0.5,
                        borderTopColor: theme.colors.divider,
                    }}>
                        {isEditing ? (
                            <>
                                <Pressable
                                    onPress={handleCancelEdit}
                                    style={({ pressed }) => ({
                                        flex: 1,
                                        backgroundColor: pressed ? theme.colors.surfacePressed : theme.colors.surfaceHighest,
                                        paddingVertical: 12,
                                        borderRadius: 8,
                                        alignItems: 'center' as const,
                                    })}
                                >
                                    <Text style={{
                                        fontSize: 14,
                                        color: theme.colors.text,
                                        ...Typography.default('semiBold'),
                                    }}>{t('common.cancel')}</Text>
                                </Pressable>
                                <Pressable
                                    onPress={handleSave}
                                    disabled={saving}
                                    style={({ pressed }) => ({
                                        flex: 1,
                                        backgroundColor: pressed ? '#0066CC' : theme.colors.textLink,
                                        paddingVertical: 12,
                                        borderRadius: 8,
                                        alignItems: 'center' as const,
                                        opacity: saving ? 0.6 : 1,
                                    })}
                                >
                                    <Text style={{
                                        fontSize: 14,
                                        color: '#fff',
                                        ...Typography.default('semiBold'),
                                    }}>{saving ? t('settingsScheduledTasks.saving') : t('common.save')}</Text>
                                </Pressable>
                            </>
                        ) : (
                            <>
                                {onUpdate && (
                                    <Pressable
                                        onPress={() => setIsEditing(true)}
                                        style={({ pressed }) => ({
                                            flex: 1,
                                            backgroundColor: pressed ? theme.colors.surfacePressed : theme.colors.surfaceHighest,
                                            paddingVertical: 12,
                                            borderRadius: 8,
                                            alignItems: 'center' as const,
                                        })}
                                    >
                                        <Text style={{
                                            fontSize: 14,
                                            color: theme.colors.textLink,
                                            ...Typography.default('semiBold'),
                                        }}>{t('settingsScheduledTasks.edit')}</Text>
                                    </Pressable>
                                )}
                                {onToggle && (
                                    <Pressable
                                        onPress={handleToggle}
                                        style={({ pressed }) => ({
                                            flex: 1,
                                            backgroundColor: pressed ? theme.colors.surfacePressed : theme.colors.surfaceHighest,
                                            paddingVertical: 12,
                                            borderRadius: 8,
                                            alignItems: 'center' as const,
                                        })}
                                    >
                                        <Text style={{
                                            fontSize: 14,
                                            color: task.enabled ? '#FF9500' : '#34C759',
                                            ...Typography.default('semiBold'),
                                        }}>
                                            {task.enabled ? t('settingsScheduledTasks.disable') : t('settingsScheduledTasks.enable')}
                                        </Text>
                                    </Pressable>
                                )}
                                {onDelete && (
                                    <Pressable
                                        onPress={handleDelete}
                                        style={({ pressed }) => ({
                                            flex: 1,
                                            backgroundColor: pressed ? theme.colors.surfacePressed : theme.colors.surfaceHighest,
                                            paddingVertical: 12,
                                            borderRadius: 8,
                                            alignItems: 'center' as const,
                                        })}
                                    >
                                        <Text style={{
                                            fontSize: 14,
                                            color: theme.colors.textDestructive,
                                            ...Typography.default('semiBold'),
                                        }}>
                                            {confirming ? t('settingsScheduledTasks.confirm') : t('common.delete')}
                                        </Text>
                                    </Pressable>
                                )}
                                <Pressable
                                    onPress={() => { setConfirming(false); onClose(); }}
                                    style={({ pressed }) => ({
                                        flex: 1,
                                        backgroundColor: pressed ? theme.colors.surfacePressed : theme.colors.surfaceHighest,
                                        paddingVertical: 12,
                                        borderRadius: 8,
                                        alignItems: 'center' as const,
                                    })}
                                >
                                    <Text style={{
                                        fontSize: 14,
                                        color: theme.colors.text,
                                        ...Typography.default('semiBold'),
                                    }}>{t('settingsScheduledTasks.close')}</Text>
                                </Pressable>
                            </>
                        )}
                    </View>
                </View>
            </View>
        </Modal>
    );
};

// --- Helpers ---

function parseCronExpr(expr: string): { mode: 'daily' | 'weekly' | 'monthly' | 'custom'; hour: number; minute: number; dow: number; dom: number } {
    const defaults = { hour: 9, minute: 0, dow: 1, dom: 1 };
    const parts = expr.trim().split(/\s+/);
    if (parts.length !== 5) return { mode: 'custom', ...defaults };
    const [minStr, hourStr, domStr, monStr, dowStr] = parts;
    if (monStr !== '*') return { mode: 'custom', ...defaults };
    const min = parseInt(minStr), hour = parseInt(hourStr);
    if (isNaN(min) || isNaN(hour)) return { mode: 'custom', ...defaults };
    if (dowStr !== '*') {
        const dow = parseInt(dowStr);
        if (isNaN(dow)) return { mode: 'custom', hour, minute: min, dow: 1, dom: 1 };
        return { mode: 'weekly', hour, minute: min, dow, dom: 1 };
    }
    if (domStr !== '*') {
        const dom = parseInt(domStr);
        if (isNaN(dom)) return { mode: 'custom', hour, minute: min, dow: 1, dom: 1 };
        return { mode: 'monthly', hour, minute: min, dow: 1, dom };
    }
    return { mode: 'daily', hour, minute: min, dow: 1, dom: 1 };
}

function parseAtString(at: string): { date: string; hour: number; minute: number } {
    const match = at.match(/^(\d{4}-\d{2}-\d{2})T(\d{2}):(\d{2})/);
    if (match) return { date: match[1], hour: parseInt(match[2]), minute: parseInt(match[3]) };
    return { date: '', hour: 9, minute: 0 };
}

const pad2 = (n: number) => String(n).padStart(2, '0');

// --- Shared sub-components ---

const inputStyle = (theme: any) => ({
    backgroundColor: theme.colors.surfaceHighest,
    borderRadius: 8,
    padding: 10,
    fontSize: 14,
    color: theme.colors.text,
    ...Typography.default(),
});

const TimeSelector: React.FC<{
    hour: number; minute: number;
    onChange: (h: number, m: number) => void;
    theme: any;
}> = ({ hour, minute, onChange, theme }) => (
    <View style={{ flexDirection: 'row', alignItems: 'center', gap: 4 }}>
        <TextInput
            value={pad2(hour)}
            onChangeText={(v) => onChange(Math.min(23, Math.max(0, parseInt(v) || 0)), minute)}
            keyboardType="numeric"
            maxLength={2}
            selectTextOnFocus
            style={{ ...inputStyle(theme), width: 48, textAlign: 'center' as const }}
        />
        <Text style={{ fontSize: 16, color: theme.colors.text, ...Typography.default('semiBold') }}>:</Text>
        <TextInput
            value={pad2(minute)}
            onChangeText={(v) => onChange(hour, Math.min(59, Math.max(0, parseInt(v) || 0)))}
            keyboardType="numeric"
            maxLength={2}
            selectTextOnFocus
            style={{ ...inputStyle(theme), width: 48, textAlign: 'center' as const }}
        />
    </View>
);

const PillSelector: React.FC<{
    options: { key: string; label: string }[];
    selected: string;
    onSelect: (key: string) => void;
    theme: any;
    circular?: boolean;
}> = ({ options, selected, onSelect, theme, circular }) => (
    <View style={{ flexDirection: 'row', gap: circular ? 4 : 6, flexWrap: 'wrap' }}>
        {options.map(o => (
            <Pressable
                key={o.key}
                onPress={() => onSelect(o.key)}
                style={({ pressed }) => ({
                    ...(circular
                        ? { width: 36, height: 36, borderRadius: 18, justifyContent: 'center' as const, alignItems: 'center' as const }
                        : { paddingHorizontal: 12, paddingVertical: 6, borderRadius: 16 }),
                    backgroundColor: o.key === selected
                        ? theme.colors.textLink
                        : pressed ? theme.colors.surfacePressed : theme.colors.surfaceHighest,
                })}
            >
                <Text style={{
                    fontSize: 13,
                    color: o.key === selected ? '#fff' : theme.colors.text,
                    ...Typography.default('semiBold'),
                }}>{o.label}</Text>
            </Pressable>
        ))}
    </View>
);

// --- Schedule Editor ---

const UNIT_KEYS: Array<'min' | 'hr' | 'day'> = ['min', 'hr', 'day'];

interface ScheduleEditorProps {
    schedule: ScheduleType;
    editCron: string;
    setEditCron: (v: string) => void;
    editEveryValue: string;
    setEditEveryValue: (v: string) => void;
    editEveryUnit: 'min' | 'hr' | 'day';
    setEditEveryUnit: (v: 'min' | 'hr' | 'day') => void;
    editAt: string;
    setEditAt: (v: string) => void;
    theme: any;
}

const ScheduleEditor: React.FC<ScheduleEditorProps> = ({
    schedule, editCron, setEditCron, editEveryValue, setEditEveryValue,
    editEveryUnit, setEditEveryUnit, editAt, setEditAt, theme,
}) => {
    const labelStyle = { fontSize: 14, color: theme.colors.textSecondary, marginBottom: 6, ...Typography.default() };

    // --- Cron preset state ---
    const parsedCron = useMemo(() => parseCronExpr(editCron), []);
    const [cronMode, setCronMode] = useState(parsedCron.mode);
    const [cronHour, setCronHour] = useState(parsedCron.hour);
    const [cronMinute, setCronMinute] = useState(parsedCron.minute);
    const [cronDow, setCronDow] = useState(parsedCron.dow);
    const [cronDom, setCronDom] = useState(parsedCron.dom);

    // --- At split state ---
    const parsedAt = useMemo(() => parseAtString(editAt), []);
    const [atDate, setAtDate] = useState(parsedAt.date);
    const [atHour, setAtHour] = useState(parsedAt.hour);
    const [atMinute, setAtMinute] = useState(parsedAt.minute);

    const generateCron = useCallback((mode: string, h: number, m: number, dow: number, dom: number) => {
        if (mode === 'daily') setEditCron(`${m} ${h} * * *`);
        else if (mode === 'weekly') setEditCron(`${m} ${h} * * ${dow}`);
        else if (mode === 'monthly') setEditCron(`${m} ${h} ${dom} * *`);
    }, [setEditCron]);

    const updateAt = useCallback((date: string, h: number, m: number) => {
        if (date) setEditAt(`${date}T${pad2(h)}:${pad2(m)}:00`);
    }, [setEditAt]);

    const CRON_MODES = useMemo(() => [
        { key: 'daily', label: t('settingsScheduledTasks.cronDaily') },
        { key: 'weekly', label: t('settingsScheduledTasks.cronWeekly') },
        { key: 'monthly', label: t('settingsScheduledTasks.cronMonthly') },
        { key: 'custom', label: t('settingsScheduledTasks.cronCustom') },
    ], []);

    const DOW_OPTIONS = useMemo(() => [
        { key: '0', label: t('settingsScheduledTasks.dowSun') },
        { key: '1', label: t('settingsScheduledTasks.dowMon') },
        { key: '2', label: t('settingsScheduledTasks.dowTue') },
        { key: '3', label: t('settingsScheduledTasks.dowWed') },
        { key: '4', label: t('settingsScheduledTasks.dowThu') },
        { key: '5', label: t('settingsScheduledTasks.dowFri') },
        { key: '6', label: t('settingsScheduledTasks.dowSat') },
    ], []);

    // --- Cron editor ---
    if (schedule.kind === 'cron') {
        return (
            <View style={{ marginBottom: 8 }}>
                <Text style={labelStyle}>{t('settingsScheduledTasks.schedule')}</Text>

                {/* Mode selector */}
                <View style={{ marginBottom: 12 }}>
                    <PillSelector
                        options={CRON_MODES}
                        selected={cronMode}
                        onSelect={(key) => {
                            const mode = key as typeof cronMode;
                            setCronMode(mode);
                            if (mode !== 'custom') generateCron(mode, cronHour, cronMinute, cronDow, cronDom);
                        }}
                        theme={theme}
                    />
                </View>

                {cronMode === 'custom' ? (
                    <TextInput
                        value={editCron}
                        onChangeText={setEditCron}
                        style={{ ...inputStyle(theme), fontFamily: 'monospace' }}
                        placeholderTextColor={theme.colors.textSecondary}
                        placeholder={t('settingsScheduledTasks.cronPlaceholder')}
                    />
                ) : (
                    <>
                        {/* Time */}
                        <View style={{ marginBottom: 10 }}>
                            <Text style={labelStyle}>{t('settingsScheduledTasks.time')}</Text>
                            <TimeSelector
                                hour={cronHour}
                                minute={cronMinute}
                                onChange={(h, m) => {
                                    setCronHour(h); setCronMinute(m);
                                    generateCron(cronMode, h, m, cronDow, cronDom);
                                }}
                                theme={theme}
                            />
                        </View>

                        {/* Weekly: day of week */}
                        {cronMode === 'weekly' && (
                            <View style={{ marginBottom: 10 }}>
                                <Text style={labelStyle}>{t('settingsScheduledTasks.dayOfWeek')}</Text>
                                <PillSelector
                                    options={DOW_OPTIONS}
                                    selected={String(cronDow)}
                                    onSelect={(key) => {
                                        const d = parseInt(key);
                                        setCronDow(d);
                                        generateCron(cronMode, cronHour, cronMinute, d, cronDom);
                                    }}
                                    theme={theme}
                                    circular
                                />
                            </View>
                        )}

                        {/* Monthly: day of month */}
                        {cronMode === 'monthly' && (
                            <View style={{ marginBottom: 10 }}>
                                <Text style={labelStyle}>{t('settingsScheduledTasks.dayOfMonth')}</Text>
                                <View style={{ flexDirection: 'row', alignItems: 'center', gap: 8 }}>
                                    <TextInput
                                        value={String(cronDom)}
                                        onChangeText={(v) => {
                                            const d = Math.min(31, Math.max(1, parseInt(v) || 1));
                                            setCronDom(d);
                                            generateCron(cronMode, cronHour, cronMinute, cronDow, d);
                                        }}
                                        keyboardType="numeric"
                                        maxLength={2}
                                        selectTextOnFocus
                                        style={{ ...inputStyle(theme), width: 56, textAlign: 'center' as const }}
                                    />
                                    <Text style={{ fontSize: 14, color: theme.colors.textSecondary, ...Typography.default() }}>
                                        {t('settingsScheduledTasks.dayOfMonthSuffix')}
                                    </Text>
                                </View>
                            </View>
                        )}
                    </>
                )}
            </View>
        );
    }

    // --- Every editor (unchanged) ---
    if (schedule.kind === 'every') {
        const unitLabelMap: Record<'min' | 'hr' | 'day', string> = {
            min: t('settingsScheduledTasks.unitMin'),
            hr: t('settingsScheduledTasks.unitHr'),
            day: t('settingsScheduledTasks.unitDay'),
        };
        return (
            <View style={{ marginBottom: 8 }}>
                <Text style={labelStyle}>{t('settingsScheduledTasks.interval')}</Text>
                <View style={{ flexDirection: 'row', gap: 8, alignItems: 'center' }}>
                    <TextInput
                        value={editEveryValue}
                        onChangeText={setEditEveryValue}
                        keyboardType="numeric"
                        style={{ ...inputStyle(theme), flex: 1 }}
                        placeholderTextColor={theme.colors.textSecondary}
                        placeholder="30"
                    />
                    <View style={{ flexDirection: 'row', gap: 4 }}>
                        {UNIT_KEYS.map(unitKey => (
                            <Pressable
                                key={unitKey}
                                onPress={() => setEditEveryUnit(unitKey)}
                                style={({ pressed }) => ({
                                    paddingHorizontal: 12,
                                    paddingVertical: 8,
                                    borderRadius: 8,
                                    backgroundColor: unitKey === editEveryUnit
                                        ? theme.colors.textLink
                                        : pressed ? theme.colors.surfacePressed : theme.colors.surfaceHighest,
                                })}
                            >
                                <Text style={{
                                    fontSize: 13,
                                    color: unitKey === editEveryUnit ? '#fff' : theme.colors.text,
                                    ...Typography.default('semiBold'),
                                }}>{unitLabelMap[unitKey]}</Text>
                            </Pressable>
                        ))}
                    </View>
                </View>
            </View>
        );
    }

    // --- At editor ---
    if (schedule.kind === 'at') {
        return (
            <View style={{ marginBottom: 8 }}>
                <Text style={labelStyle}>{t('settingsScheduledTasks.runAt')}</Text>

                {/* Date */}
                <View style={{ marginBottom: 10 }}>
                    <Text style={labelStyle}>{t('settingsScheduledTasks.date')}</Text>
                    <TextInput
                        value={atDate}
                        onChangeText={(v) => {
                            setAtDate(v);
                            if (/^\d{4}-\d{2}-\d{2}$/.test(v)) updateAt(v, atHour, atMinute);
                        }}
                        style={inputStyle(theme)}
                        placeholderTextColor={theme.colors.textSecondary}
                        placeholder="2026-03-01"
                    />
                </View>

                {/* Time */}
                <View style={{ marginBottom: 10 }}>
                    <Text style={labelStyle}>{t('settingsScheduledTasks.time')}</Text>
                    <TimeSelector
                        hour={atHour}
                        minute={atMinute}
                        onChange={(h, m) => {
                            setAtHour(h); setAtMinute(m);
                            updateAt(atDate, h, m);
                        }}
                        theme={theme}
                    />
                </View>
            </View>
        );
    }

    return null;
};

interface InfoRowProps {
    label: string;
    value: string;
    valueColor?: string;
    theme: any;
}

const InfoRow: React.FC<InfoRowProps> = ({ label, value, valueColor, theme }) => (
    <View style={{
        flexDirection: 'row',
        justifyContent: 'space-between',
        paddingVertical: 8,
        borderBottomWidth: 0.5,
        borderBottomColor: theme.colors.divider,
    }}>
        <Text style={{
            fontSize: 14,
            color: theme.colors.textSecondary,
            flex: 1,
            ...Typography.default(),
        }}>{label}</Text>
        <Text style={{
            fontSize: 14,
            color: valueColor || theme.colors.text,
            flex: 2,
            textAlign: 'right',
            ...Typography.default(),
        }}>{value}</Text>
    </View>
);

function formatLastStatus(status?: string | null): string {
    if (status === 'success') return t('settingsScheduledTasks.ok');
    if (status === 'failed') return t('settingsScheduledTasks.fail');
    if (status === 'running') return t('settingsScheduledTasks.running');
    return status || t('settingsScheduledTasks.none');
}
