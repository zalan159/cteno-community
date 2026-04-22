import React, { useState, useCallback, useEffect, useMemo } from 'react';
import { View, ActivityIndicator, Pressable, ScrollView, useWindowDimensions } from 'react-native';
import { useLocalSearchParams } from 'expo-router';
import { useAllMachines, useAllSessions } from '@/sync/storage';
import { useUnistyles } from 'react-native-unistyles';
import { isMachineOnline } from '@/utils/machineUtils';
import { Typography } from '@/constants/Typography';
import { ScheduledTaskCard } from '@/components/ScheduledTaskCard';
import { ScheduledTaskDetailModal } from '@/components/ScheduledTaskDetailModal';
import { useScheduledTasks } from '@/hooks/useScheduledTasks';
import type { ScheduledTask, UpdateScheduledTaskInput } from '@/sync/ops';
import { Text } from '@/components/StyledText';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { t } from '@/text';

export default function ScheduledTasksScreen() {
    const { theme } = useUnistyles();
    const machines = useAllMachines();
    const sessions = useAllSessions();
    const safeArea = useSafeAreaInsets();
    const screenWidth = useWindowDimensions().width;
    const contentWidth = Math.min(screenWidth - 32, 600);
    const { machineId: routeMachineId } = useLocalSearchParams<{ machineId?: string }>();

    // Derive unique machine IDs that have sessions (most recently active first)
    const machineIdsWithSessions = useMemo(() => {
        const seen = new Set<string>();
        const result: string[] = [];
        for (const session of sessions) {
            const mid = session.metadata?.machineId;
            if (mid && !seen.has(mid)) {
                seen.add(mid);
                result.push(mid);
            }
        }
        return result;
    }, [sessions]);

    // Pick machine from most recent session first, then fall back to first online machine
    const defaultMachineId = useMemo(() => {
        if (routeMachineId) return routeMachineId;
        for (const mid of machineIdsWithSessions) {
            if (machines.some(m => m.id === mid && isMachineOnline(m))) {
                return mid;
            }
        }
        if (machineIdsWithSessions.length > 0) return machineIdsWithSessions[0];
        const online = machines.find((m) => isMachineOnline(m));
        if (online) return online.id;
        return machines.length > 0 ? machines[0].id : null;
    }, [machines, machineIdsWithSessions, routeMachineId]);

    const [selectedMachineId, setSelectedMachineId] = useState<string | null>(null);

    useEffect(() => {
        if (!selectedMachineId && defaultMachineId) {
            setSelectedMachineId(defaultMachineId);
        }
    }, [defaultMachineId, selectedMachineId]);

    const onlineMachines = useMemo(() => {
        return machines.filter(m => isMachineOnline(m)).map(m => ({
            id: m.id,
            name: m.metadata?.displayName || m.metadata?.host || m.id.slice(0, 16),
        }));
    }, [machines]);

    const { tasks, loading, error, toggleTask, deleteTask, updateTask } = useScheduledTasks({
        machineId: selectedMachineId || undefined,
    });

    const [selectedTask, setSelectedTask] = useState<ScheduledTask | null>(null);
    const [modalVisible, setModalVisible] = useState(false);

    const openDetail = useCallback((task: ScheduledTask) => {
        setSelectedTask(task);
        setModalVisible(true);
    }, []);

    const closeDetail = useCallback(() => {
        setModalVisible(false);
        setSelectedTask(null);
    }, []);

    const handleToggle = useCallback(async (id: string, enabled: boolean) => {
        try {
            await toggleTask(id, enabled);
        } catch {
            // error already logged in hook
        }
    }, [toggleTask]);

    const handleDelete = useCallback(async (id: string) => {
        try {
            await deleteTask(id);
        } catch {
            // error already logged in hook
        }
    }, [deleteTask]);

    const handleUpdate = useCallback(async (id: string, updates: UpdateScheduledTaskInput) => {
        try {
            await updateTask(id, updates);
        } catch {
            // error already logged in hook
        }
    }, [updateTask]);

    // Refresh the selected task in the modal after list updates
    useEffect(() => {
        if (selectedTask) {
            const updated = tasks.find(t => t.id === selectedTask.id);
            if (updated) {
                setSelectedTask(updated);
            } else {
                setModalVisible(false);
                setSelectedTask(null);
            }
        }
    }, [tasks]);

    if (!selectedMachineId) {
        return (
            <View style={{ flex: 1, justifyContent: 'center', alignItems: 'center', backgroundColor: theme.colors.surface }}>
                <Text style={{ color: theme.colors.textSecondary, fontSize: 16 }}>
                    {t('settingsScheduledTasks.noMachineAvailable')}
                </Text>
            </View>
        );
    }

    if (loading && tasks.length === 0) {
        return (
            <View style={{ flex: 1, justifyContent: 'center', alignItems: 'center', backgroundColor: theme.colors.surface }}>
                <ActivityIndicator size="large" />
            </View>
        );
    }

    if (error && tasks.length === 0) {
        return (
            <View style={{ flex: 1, justifyContent: 'center', alignItems: 'center', backgroundColor: theme.colors.surface }}>
                <Text style={{ color: theme.colors.textSecondary, fontSize: 16 }}>
                    {error}
                </Text>
            </View>
        );
    }

    return (
        <ScrollView
            style={{ flex: 1, backgroundColor: theme.colors.surface }}
            contentContainerStyle={{
                alignItems: 'center',
                paddingTop: 16,
                paddingBottom: safeArea.bottom + 32,
            }}
        >
            <View style={{ width: contentWidth }}>
                {/* Machine selector — only show when multiple machines are online and not navigated from device detail */}
                {!routeMachineId && onlineMachines.length > 1 && (
                    <View style={{
                        flexDirection: 'row',
                        marginBottom: 16,
                        gap: 8,
                    }}>
                        {onlineMachines.map(m => (
                            <Pressable
                                key={m.id}
                                onPress={() => setSelectedMachineId(m.id)}
                                style={({ pressed }) => ({
                                    paddingHorizontal: 12,
                                    paddingVertical: 6,
                                    borderRadius: 16,
                                    backgroundColor: m.id === selectedMachineId
                                        ? theme.colors.textLink
                                        : pressed ? theme.colors.surfacePressed : theme.colors.surfaceHighest,
                                })}
                            >
                                <Text style={{
                                    fontSize: 13,
                                    color: m.id === selectedMachineId ? '#fff' : theme.colors.text,
                                    ...Typography.default('semiBold'),
                                }}>{m.name}</Text>
                            </Pressable>
                        ))}
                    </View>
                )}

                {tasks.length === 0 ? (
                    <View style={{ paddingVertical: 60, alignItems: 'center' }}>
                        <Text style={{
                            fontSize: 18,
                            color: theme.colors.text,
                            marginBottom: 8,
                            ...Typography.default('semiBold'),
                        }}>
                            {t('settingsScheduledTasks.emptyTitle')}
                        </Text>
                        <Text style={{
                            fontSize: 14,
                            color: theme.colors.textSecondary,
                            textAlign: 'center',
                            lineHeight: 20,
                            ...Typography.default(),
                        }}>
                            {t('settingsScheduledTasks.emptySubtitle')}
                        </Text>
                    </View>
                ) : (
                    tasks.map(item => (
                        <ScheduledTaskCard
                            key={item.id}
                            task={item}
                            onViewDetails={() => openDetail(item)}
                            onToggle={(enabled) => handleToggle(item.id, enabled)}
                            onDelete={() => handleDelete(item.id)}
                        />
                    ))
                )}
            </View>

            <ScheduledTaskDetailModal
                task={selectedTask}
                visible={modalVisible}
                onClose={closeDetail}
                onToggle={handleToggle}
                onDelete={handleDelete}
                onUpdate={handleUpdate}
            />
        </ScrollView>
    );
}
