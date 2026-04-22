import React from 'react';
import { View, Pressable, ScrollView } from 'react-native';
import { useRouter } from 'expo-router';
import { useUnistyles } from 'react-native-unistyles';
import { Ionicons } from '@expo/vector-icons';
import { Typography } from '@/constants/Typography';
import { Text } from '@/components/StyledText';
import { useAllSessions } from '@/sync/storage';
import { machineGetPersonaTasks } from '@/sync/ops';
import type { Session, PersonaTaskSummary } from '@/sync/storageTypes';

interface PersonaTaskSessionsProps {
    personaId: string;
    machineId?: string;
}

export const PersonaTaskSessions = React.memo(({ personaId, machineId }: PersonaTaskSessionsProps) => {
    const sessions = useAllSessions();
    const router = useRouter();
    const { theme } = useUnistyles();
    const [taskSummaries, setTaskSummaries] = React.useState<PersonaTaskSummary[]>([]);

    // Fetch task sessions via RPC (poll every 10s)
    React.useEffect(() => {
        if (!machineId) return;

        let mounted = true;
        const fetch = async () => {
            const tasks = await machineGetPersonaTasks(machineId, personaId);
            if (mounted) setTaskSummaries(tasks);
        };
        fetch();
        const interval = setInterval(fetch, 10_000);
        return () => { mounted = false; clearInterval(interval); };
    }, [machineId, personaId]);

    // Match task session IDs to frontend Session objects for live status
    const taskItems = React.useMemo(() => {
        return taskSummaries.map(task => {
            const session = sessions.find((s: Session) => s.id === task.sessionId);
            return { ...task, session };
        });
    }, [taskSummaries, sessions]);

    if (taskItems.length === 0) return null;

    return (
        <View style={{
            paddingHorizontal: 16,
            paddingVertical: 6,
        }}>
            <Text style={{
                fontSize: 11,
                color: theme.colors.textSecondary,
                marginBottom: 4,
                ...Typography.default('semiBold'),
            }}>
                Tasks ({taskItems.length})
            </Text>
            <ScrollView horizontal showsHorizontalScrollIndicator={false}>
                {taskItems.map((item) => {
                    const isThinking = item.session?.thinking ?? false;
                    const isOnline = item.session?.presence === 'online';
                    const statusColor = isThinking ? '#007AFF' : isOnline ? '#34C759' : '#8E8E93';
                    const summary = item.session?.metadata?.summary?.text
                        || item.taskDescription
                        || item.sessionId.slice(0, 8);

                    return (
                        <Pressable
                            key={item.sessionId}
                            onPress={() => router.push(`/session/${item.sessionId}`)}
                            style={({ pressed }) => ({
                                backgroundColor: pressed
                                    ? theme.colors.input.background
                                    : theme.colors.surface,
                                borderWidth: 1,
                                borderColor: theme.colors.divider,
                                borderRadius: 8,
                                paddingHorizontal: 10,
                                paddingVertical: 6,
                                marginRight: 8,
                                maxWidth: 200,
                                flexDirection: 'row',
                                alignItems: 'center',
                                gap: 6,
                            })}
                        >
                            <View style={{
                                width: 6,
                                height: 6,
                                borderRadius: 3,
                                backgroundColor: statusColor,
                            }} />
                            <Text
                                numberOfLines={1}
                                style={{
                                    fontSize: 12,
                                    color: theme.colors.text,
                                    flex: 1,
                                    ...Typography.default(),
                                }}
                            >
                                {summary}
                            </Text>
                            <Ionicons name="chevron-forward" size={12} color={theme.colors.textSecondary} />
                        </Pressable>
                    );
                })}
            </ScrollView>
        </View>
    );
});

PersonaTaskSessions.displayName = 'PersonaTaskSessions';
