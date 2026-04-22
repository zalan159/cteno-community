import React, { useMemo, useEffect, useState } from 'react';
import { View, ActivityIndicator } from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { useLocalSearchParams } from 'expo-router';
import { useUnistyles } from 'react-native-unistyles';
import { Typography } from '@/constants/Typography';
import { Text } from '@/components/StyledText';
import { ItemList } from '@/components/ItemList';
import { ItemGroup } from '@/components/ItemGroup';
import { Item } from '@/components/Item';
import { useAllMachines, useLocalSettingMutable } from '@/sync/storage';
import { isMachineOnline } from '@/utils/machineUtils';
import { machineGetAgent } from '@/sync/ops';
import { layout } from '@/components/layout';
import { t } from '@/text';
import type { AgentConfig } from '@/sync/storageTypes';

export default function AgentDetailPage() {
    const { id } = useLocalSearchParams<{ id: string }>();
    const { theme } = useUnistyles();
    const machines = useAllMachines();
    const [selectedMachineIdFilter] = useLocalSettingMutable('selectedMachineIdFilter');
    const [agent, setAgent] = useState<AgentConfig | null>(null);
    const [loading, setLoading] = useState(true);

    const machineId = useMemo(() => {
        if (selectedMachineIdFilter) return selectedMachineIdFilter;
        const online = machines.find(m => isMachineOnline(m));
        if (online) return online.id;
        return machines.length > 0 ? machines[0].id : undefined;
    }, [selectedMachineIdFilter, machines]);

    useEffect(() => {
        if (!machineId || !id) {
            setLoading(false);
            return;
        }
        setLoading(true);
        machineGetAgent(machineId, id)
            .then(result => setAgent(result))
            .catch(err => console.error('Failed to fetch agent:', err))
            .finally(() => setLoading(false));
    }, [machineId, id]);

    if (loading) {
        return (
            <View style={{ flex: 1, alignItems: 'center', justifyContent: 'center', backgroundColor: theme.colors.groupped.background }}>
                <ActivityIndicator size="small" color={theme.colors.textSecondary} />
                <Text style={{ marginTop: 12, color: theme.colors.textSecondary, ...Typography.default() }}>
                    {t('agent.loading')}
                </Text>
            </View>
        );
    }

    if (!agent) {
        return (
            <View style={{ flex: 1, alignItems: 'center', justifyContent: 'center', backgroundColor: theme.colors.groupped.background }}>
                <Ionicons name="alert-circle-outline" size={48} color={theme.colors.textSecondary} />
                <Text style={{ marginTop: 12, fontSize: 16, color: theme.colors.textSecondary, ...Typography.default() }}>
                    {t('agent.notFound')}
                </Text>
            </View>
        );
    }

    const sourceLabelText = agent.source === 'builtin'
        ? t('agent.sourceBuiltin')
        : agent.source === 'global'
            ? t('agent.sourceGlobal')
            : t('agent.sourceWorkspace');

    return (
        <ItemList>
            {/* Agent header */}
            <View style={{
                maxWidth: layout.maxWidth,
                alignSelf: 'center',
                width: '100%',
                paddingHorizontal: 16,
                paddingTop: 16,
                paddingBottom: 8,
                alignItems: 'center',
            }}>
                <View style={{
                    width: 64,
                    height: 64,
                    borderRadius: 16,
                    backgroundColor: theme.colors.surfaceHigh,
                    alignItems: 'center',
                    justifyContent: 'center',
                    marginBottom: 12,
                }}>
                    <Ionicons name="construct-outline" size={32} color={theme.colors.text} />
                </View>
                <Text style={{
                    fontSize: 22,
                    color: theme.colors.text,
                    textAlign: 'center',
                    ...Typography.default('semiBold'),
                }}>
                    {agent.name}
                </Text>
                {!!agent.description && (
                    <Text style={{
                        fontSize: 14,
                        color: theme.colors.textSecondary,
                        textAlign: 'center',
                        marginTop: 4,
                        ...Typography.default(),
                    }}>
                        {agent.description}
                    </Text>
                )}
                {/* Source badge */}
                <View style={{
                    marginTop: 8,
                    paddingHorizontal: 10,
                    paddingVertical: 4,
                    borderRadius: 6,
                    backgroundColor: theme.colors.surfaceHigh,
                }}>
                    <Text style={{
                        fontSize: 12,
                        color: theme.colors.textSecondary,
                        ...Typography.default('semiBold'),
                    }}>
                        {sourceLabelText}
                    </Text>
                </View>
            </View>

            {/* Info section */}
            <ItemGroup title={t('agent.detailInfo')}>
                <Item
                    title={t('agent.id')}
                    detail={agent.id}
                    showChevron={false}
                    copy={agent.id}
                />
                <Item
                    title={t('agent.model')}
                    detail={agent.model || '--'}
                    showChevron={false}
                />
                {agent.version && (
                    <Item
                        title={t('agent.version')}
                        detail={agent.version}
                        showChevron={false}
                    />
                )}
                {agent.agent_type && (
                    <Item
                        title={t('agent.type')}
                        detail={agent.agent_type}
                        showChevron={false}
                    />
                )}
            </ItemGroup>

            {/* Tools section */}
            {(agent.allowed_tools.length > 0 || agent.excluded_tools.length > 0 || agent.tools.length > 0) && (
                <ItemGroup title={t('agent.detailTools')}>
                    {agent.allowed_tools.length > 0 && (
                        <Item
                            title={t('agent.allowedTools')}
                            subtitle={agent.allowed_tools.join(', ')}
                            subtitleLines={0}
                            showChevron={false}
                        />
                    )}
                    {agent.excluded_tools.length > 0 && (
                        <Item
                            title={t('agent.excludedTools')}
                            subtitle={agent.excluded_tools.join(', ')}
                            subtitleLines={0}
                            showChevron={false}
                        />
                    )}
                    {agent.tools.length > 0 && (
                        <Item
                            title={t('agent.tools')}
                            subtitle={agent.tools.join(', ')}
                            subtitleLines={0}
                            showChevron={false}
                        />
                    )}
                </ItemGroup>
            )}

            {/* Skills section */}
            {agent.skills.length > 0 && (
                <ItemGroup title={t('agent.detailSkills')}>
                    {agent.skills.map((skill) => (
                        <Item
                            key={skill}
                            title={skill}
                            icon={<Ionicons name="flash-outline" size={18} color={theme.colors.textSecondary} />}
                            showChevron={false}
                        />
                    ))}
                </ItemGroup>
            )}

            {/* Instructions section */}
            {!!agent.instructions && (
                <ItemGroup title={t('agent.detailInstructions')}>
                    <View style={{ padding: 16 }}>
                        <Text style={{
                            fontSize: 14,
                            color: theme.colors.text,
                            lineHeight: 20,
                            ...Typography.default(),
                        }}>
                            {agent.instructions}
                        </Text>
                    </View>
                </ItemGroup>
            )}
        </ItemList>
    );
}
