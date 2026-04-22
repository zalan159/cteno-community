import * as React from 'react';
import { useState, useMemo, useCallback } from 'react';
import { View, Pressable, ActivityIndicator } from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { useUnistyles } from 'react-native-unistyles';
import { useRouter } from 'expo-router';
import { Typography } from '@/constants/Typography';
import { Text } from '@/components/StyledText';
import { ItemList } from '@/components/ItemList';
import { ItemGroup } from '@/components/ItemGroup';
import { Item } from '@/components/Item';
import { CreateAgentModal } from '@/components/CreateAgentModal';
import { useAgents } from '@/hooks/useAgents';
import { useAllMachines, useLocalSettingMutable } from '@/sync/storage';
import { isMachineOnline } from '@/utils/machineUtils';
import { Modal } from '@/modal';
import { layout } from '@/components/layout';
import { t } from '@/text';
import type { AgentConfig } from '@/sync/storageTypes';

/** Source badge label */
function sourceLabel(source: AgentConfig['source']): string {
    switch (source) {
        case 'builtin': return t('agent.sourceBuiltin');
        case 'global': return t('agent.sourceGlobal');
        case 'workspace': return t('agent.sourceWorkspace');
        default: return source;
    }
}

/** Source badge color */
function sourceBadgeColor(source: AgentConfig['source'], theme: any): { bg: string; text: string } {
    switch (source) {
        case 'builtin':
            return { bg: theme.colors.surfaceHigh, text: theme.colors.textSecondary };
        case 'global':
            return { bg: '#14b8a620', text: '#14b8a6' };
        case 'workspace':
            return { bg: '#3b82f620', text: '#3b82f6' };
        default:
            return { bg: theme.colors.surfaceHigh, text: theme.colors.textSecondary };
    }
}

/** Group agents by source */
function groupBySource(agents: AgentConfig[]): { workspace: AgentConfig[]; global: AgentConfig[]; builtin: AgentConfig[] } {
    const workspace: AgentConfig[] = [];
    const global: AgentConfig[] = [];
    const builtin: AgentConfig[] = [];
    for (const agent of agents) {
        switch (agent.source) {
            case 'workspace': workspace.push(agent); break;
            case 'global': global.push(agent); break;
            case 'builtin': builtin.push(agent); break;
        }
    }
    return { workspace, global, builtin };
}

/** Source icon */
function sourceIcon(source: AgentConfig['source']): string {
    switch (source) {
        case 'builtin': return 'cube-outline';
        case 'global': return 'globe-outline';
        case 'workspace': return 'folder-outline';
        default: return 'cube-outline';
    }
}

export const AgentListView = React.memo(() => {
    const { theme } = useUnistyles();
    const router = useRouter();
    const machines = useAllMachines();
    const [showCreateModal, setShowCreateModal] = useState(false);
    const [selectedMachineIdFilter] = useLocalSettingMutable('selectedMachineIdFilter');

    const machineId = useMemo(() => {
        if (selectedMachineIdFilter) return selectedMachineIdFilter;
        const online = machines.find(m => isMachineOnline(m));
        if (online) return online.id;
        return machines.length > 0 ? machines[0].id : undefined;
    }, [selectedMachineIdFilter, machines]);

    const { agents, loading, createAgent, deleteAgent, refresh } = useAgents({
        machineId,
    });

    const groups = useMemo(() => groupBySource(agents), [agents]);

    const handleCreate = useCallback(async (params: {
        id: string;
        name: string;
        description: string;
        model?: string;
        scope: 'global' | 'workspace';
    }) => {
        await createAgent(params);
    }, [createAgent]);

    const handleDelete = useCallback(async (agent: AgentConfig) => {
        Modal.alert(
            t('agent.deleteTitle'),
            t('agent.deleteMessage', { name: agent.name }),
            [
                { text: t('common.cancel'), style: 'cancel' },
                {
                    text: t('common.delete'),
                    style: 'destructive',
                    onPress: async () => {
                        try {
                            await deleteAgent(agent.id);
                        } catch (err) {
                            console.error('Failed to delete agent:', err);
                        }
                    },
                },
            ]
        );
    }, [deleteAgent]);

    const handleAgentPress = useCallback((agentId: string) => {
        router.push(`/agent/${agentId}` as any);
    }, [router]);

    if (!machineId) {
        return (
            <ItemList style={{ paddingTop: 0 }}>
                <View style={{
                    maxWidth: layout.maxWidth,
                    alignSelf: 'center',
                    width: '100%',
                    padding: 24,
                    alignItems: 'center',
                }}>
                    <Ionicons name="cloud-offline-outline" size={40} color={theme.colors.textSecondary} />
                    <Text style={{
                        marginTop: 12,
                        fontSize: 16,
                        color: theme.colors.textSecondary,
                        textAlign: 'center',
                        ...Typography.default(),
                    }}>
                        {t('agent.noMachine')}
                    </Text>
                </View>
            </ItemList>
        );
    }

    const renderAgentGroup = (
        title: string,
        agentList: AgentConfig[],
        canDelete: boolean,
    ) => {
        if (agentList.length === 0) return null;
        return (
            <ItemGroup title={title} key={title}>
                {agentList.map((agent) => {
                    const badge = sourceBadgeColor(agent.source, theme);
                    return (
                        <Item
                            key={agent.id}
                            title={agent.name}
                            subtitle={agent.description || agent.id}
                            onPress={() => handleAgentPress(agent.id)}
                            onLongPress={canDelete ? () => handleDelete(agent) : undefined}
                            icon={
                                <View style={{
                                    width: 32,
                                    height: 32,
                                    borderRadius: 8,
                                    backgroundColor: badge.bg,
                                    alignItems: 'center',
                                    justifyContent: 'center',
                                }}>
                                    <Ionicons
                                        name={sourceIcon(agent.source) as any}
                                        size={18}
                                        color={badge.text}
                                    />
                                </View>
                            }
                            rightElement={
                                <View style={{
                                    paddingHorizontal: 6,
                                    paddingVertical: 2,
                                    borderRadius: 4,
                                    backgroundColor: badge.bg,
                                }}>
                                    <Text style={{
                                        fontSize: 10,
                                        color: badge.text,
                                        ...Typography.default('semiBold'),
                                    }}>
                                        {sourceLabel(agent.source)}
                                    </Text>
                                </View>
                            }
                        />
                    );
                })}
            </ItemGroup>
        );
    };

    return (
        <ItemList style={{ paddingTop: 0 }}>
            {/* Create button */}
            <View style={{
                maxWidth: layout.maxWidth,
                alignSelf: 'center',
                width: '100%',
                paddingHorizontal: 16,
                paddingTop: 16,
                paddingBottom: 8,
            }}>
                <Pressable
                    onPress={() => setShowCreateModal(true)}
                    style={({ pressed }) => ({
                        flexDirection: 'row',
                        alignItems: 'center',
                        justifyContent: 'center',
                        paddingVertical: 10,
                        borderRadius: 10,
                        backgroundColor: pressed
                            ? theme.colors.surfacePressed
                            : theme.colors.button.primary.background,
                    })}
                >
                    <Ionicons name="add" size={18} color={theme.colors.button.primary.tint} />
                    <Text style={{
                        fontSize: 14,
                        color: theme.colors.button.primary.tint,
                        marginLeft: 4,
                        ...Typography.default('semiBold'),
                    }}>
                        {t('agent.newAgent')}
                    </Text>
                </Pressable>
            </View>

            {loading && agents.length === 0 ? (
                <View style={{ padding: 40, alignItems: 'center' }}>
                    <ActivityIndicator size="small" color={theme.colors.textSecondary} />
                </View>
            ) : agents.length === 0 ? (
                <View style={{
                    maxWidth: layout.maxWidth,
                    alignSelf: 'center',
                    width: '100%',
                    alignItems: 'center',
                    paddingVertical: 40,
                    paddingHorizontal: 20,
                }}>
                    <Ionicons name="construct-outline" size={48} color={theme.colors.textSecondary} />
                    <Text style={{
                        marginTop: 12,
                        fontSize: 16,
                        color: theme.colors.textSecondary,
                        textAlign: 'center',
                        ...Typography.default(),
                    }}>
                        {t('agent.emptyTitle')}
                    </Text>
                    <Text style={{
                        marginTop: 8,
                        fontSize: 14,
                        color: theme.colors.textSecondary,
                        textAlign: 'center',
                        lineHeight: 20,
                        ...Typography.default(),
                    }}>
                        {t('agent.emptyDescription')}
                    </Text>
                </View>
            ) : (
                <>
                    {renderAgentGroup(t('agent.sourceWorkspace'), groups.workspace, true)}
                    {renderAgentGroup(t('agent.sourceGlobal'), groups.global, true)}
                    {renderAgentGroup(t('agent.sourceBuiltin'), groups.builtin, false)}
                </>
            )}

            <CreateAgentModal
                visible={showCreateModal}
                onClose={() => setShowCreateModal(false)}
                onCreate={handleCreate}
            />
        </ItemList>
    );
});
