import React from 'react';
import { View, ScrollView, ActivityIndicator, Pressable, TextInput, Switch } from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { useAllMachines } from '@/sync/storage';
import { useUnistyles } from 'react-native-unistyles';
import { Typography } from '@/constants/Typography';
import { t } from '@/text';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { useWindowDimensions } from 'react-native';
import { isMachineOnline } from '@/utils/machineUtils';
import { useLocalSearchParams } from 'expo-router';
import { machineListMCPServers, machineAddMCPServer, machineRemoveMCPServer, machineToggleMCPServer, type MCPServerItem } from '@/sync/ops';
import { Modal } from '@/modal';
import { Text } from '@/components/StyledText';

// Module-level cache so server list persists across page navigations
let cachedServers: MCPServerItem[] = [];
let cachedMachineId: string | null = null;

function MCPManager() {
    const { theme } = useUnistyles();
    const machines = useAllMachines();
    const safeArea = useSafeAreaInsets();
    const screenWidth = useWindowDimensions().width;
    const { machineId: routeMachineId } = useLocalSearchParams<{ machineId?: string }>();

    // Add form state
    const [showAddForm, setShowAddForm] = React.useState(false);
    const [saving, setSaving] = React.useState(false);
    const [transportType, setTransportType] = React.useState<'stdio' | 'http_sse'>('stdio');
    const [newName, setNewName] = React.useState('');
    const [newCommand, setNewCommand] = React.useState('');
    const [newArgs, setNewArgs] = React.useState('');
    const [newEnv, setNewEnv] = React.useState('');
    const [newUrl, setNewUrl] = React.useState('');
    const [newHeaders, setNewHeaders] = React.useState('');

    // Select machine — if machineId is passed via route param, use it directly
    const [selectedMachineId, setSelectedMachineId] = React.useState<string | null>(() => {
        if (routeMachineId) return routeMachineId;
        if (cachedMachineId && machines.find(m => m.id === cachedMachineId)) return cachedMachineId;
        const online = machines.find(m => isMachineOnline(m));
        if (online) return online.id;
        return machines.length > 0 ? machines[0].id : null;
    });

    // Auto-select when machines become available (e.g., store hydration after mount)
    React.useEffect(() => {
        if (routeMachineId) return;
        if (!selectedMachineId && machines.length > 0) {
            const online = machines.find(m => isMachineOnline(m));
            setSelectedMachineId(online ? online.id : machines[0].id);
        }
    }, [machines, selectedMachineId, routeMachineId]);

    // Persist selection to module cache
    React.useEffect(() => {
        cachedMachineId = selectedMachineId;
    }, [selectedMachineId]);

    const selectedMachine = React.useMemo(() => {
        if (!selectedMachineId) return null;
        return machines.find(m => m.id === selectedMachineId) || null;
    }, [selectedMachineId, machines]);

    // Server list — use cache for instant display, refresh in background
    const [servers, setServers] = React.useState<MCPServerItem[]>(
        cachedMachineId === selectedMachineId ? cachedServers : []
    );
    const [refreshing, setRefreshing] = React.useState(false);

    const loadServers = React.useCallback(async () => {
        if (!selectedMachineId) return;
        setRefreshing(true);
        try {
            const result = await machineListMCPServers(selectedMachineId);
            const list = result.servers || [];
            setServers(list);
            cachedServers = list;
            cachedMachineId = selectedMachineId;
        } catch (e) {
            console.warn('Failed to load MCP servers:', e);
            // Keep showing cached/current data, don't clear
        } finally {
            setRefreshing(false);
        }
    }, [selectedMachineId]);

    // Background refresh on mount and machine change
    React.useEffect(() => {
        loadServers();
    }, [loadServers]);

    const resetForm = () => {
        setNewName('');
        setNewCommand('');
        setNewArgs('');
        setNewEnv('');
        setNewUrl('');
        setNewHeaders('');
        setTransportType('stdio');
    };

    const handleAdd = async () => {
        console.log('[MCP] handleAdd: selectedMachineId=', selectedMachineId, 'newName=', newName);
        if (!selectedMachineId) {
            Modal.alert(t('common.error'), t('mcp.addFailed') + ' (no machine selected)');
            return;
        }
        if (!newName.trim()) {
            Modal.alert(t('common.error'), t('mcp.nameRequired'));
            return;
        }

        setSaving(true);
        try {
            const transport = transportType === 'stdio'
                ? {
                    type: 'stdio' as const,
                    command: newCommand.trim(),
                    args: newArgs.trim().split(/\s+/).filter(Boolean),
                    env: parseKeyValuePairs(newEnv),
                }
                : {
                    type: 'http_sse' as const,
                    url: newUrl.trim(),
                    headers: parseKeyValuePairs(newHeaders),
                };

            console.log('[MCP] calling machineAddMCPServer...');
            const result = await machineAddMCPServer(selectedMachineId, {
                name: newName.trim(),
                transport,
            });
            console.log('[MCP] machineAddMCPServer result:', result);

            if (result.success) {
                resetForm();
                setShowAddForm(false);
                await loadServers();
            } else {
                Modal.alert(t('common.error'), result.error || t('mcp.addFailed'));
            }
        } catch (e) {
            console.error('[MCP] handleAdd error:', e);
            Modal.alert(t('common.error'), t('mcp.addFailed'));
        } finally {
            setSaving(false);
        }
    };

    const handleDelete = async (server: MCPServerItem) => {
        if (!selectedMachineId) return;

        const confirmed = await Modal.confirm(
            t('mcp.deleteConfirmTitle'),
            t('mcp.deleteConfirmMessage'),
            { destructive: true, confirmText: t('common.delete') }
        );
        if (!confirmed) return;

        const result = await machineRemoveMCPServer(selectedMachineId, server.id);
        if (result.success) {
            await loadServers();
        } else {
            Modal.alert(t('common.error'), t('mcp.deleteFailed'));
        }
    };

    const handleToggle = async (server: MCPServerItem, enabled: boolean) => {
        if (!selectedMachineId) return;
        await machineToggleMCPServer(selectedMachineId, server.id, enabled);
        await loadServers();
    };

    const contentWidth = Math.min(screenWidth - 32, 600);

    // No machines at all
    if (machines.length === 0) {
        return (
            <View style={{ flex: 1, alignItems: 'center', justifyContent: 'center', backgroundColor: theme.colors.surface }}>
                <Ionicons name="desktop-outline" size={48} color={theme.colors.textSecondary} />
                <Text style={{ fontSize: 16, color: theme.colors.textSecondary, marginTop: 12, ...Typography.default() }}>
                    {t('mcp.noServers')}
                </Text>
            </View>
        );
    }

    return (
        <ScrollView
            style={{ flex: 1, backgroundColor: theme.colors.surface }}
            contentContainerStyle={{ alignItems: 'center', paddingTop: safeArea.top + 16, paddingBottom: safeArea.bottom + 32 }}
        >
            <View style={{ width: contentWidth }}>
                {/* Header */}
                <View style={{ marginBottom: 24 }}>
                    <Text style={{ fontSize: 28, color: theme.colors.text, ...Typography.default('semiBold') }}>
                        {t('mcp.title')}
                    </Text>
                    <Text style={{ fontSize: 14, color: theme.colors.textSecondary, marginTop: 4, ...Typography.default() }}>
                        {t('mcp.subtitle')}
                    </Text>
                </View>

                {/* Machine Selector (when multiple machines, hidden when navigated from device detail) */}
                {!routeMachineId && machines.length > 1 && (
                    <View style={{ marginBottom: 16 }}>
                        <Text style={{ fontSize: 13, color: theme.colors.textSecondary, marginBottom: 8, ...Typography.default('semiBold') }}>
                            {t('mcp.selectDevice')}
                        </Text>
                        <ScrollView horizontal showsHorizontalScrollIndicator={false} style={{ flexDirection: 'row' }}>
                            {machines.map((m) => {
                                const isOnline = isMachineOnline(m);
                                const isSelected = m.id === selectedMachineId;
                                return (
                                    <Pressable
                                        key={m.id}
                                        onPress={() => setSelectedMachineId(m.id)}
                                        style={{
                                            flexDirection: 'row',
                                            alignItems: 'center',
                                            paddingVertical: 8,
                                            paddingHorizontal: 12,
                                            borderRadius: 8,
                                            backgroundColor: isSelected ? theme.colors.textLink : 'transparent',
                                            borderWidth: 1,
                                            borderColor: isSelected ? theme.colors.textLink : theme.colors.divider,
                                            marginRight: 8,
                                        }}
                                    >
                                        <View style={{
                                            width: 6,
                                            height: 6,
                                            borderRadius: 3,
                                            backgroundColor: isOnline ? '#34C759' : '#8E8E93',
                                            marginRight: 6,
                                        }} />
                                        <Text style={{
                                            fontSize: 13,
                                            color: isSelected ? '#fff' : theme.colors.text,
                                            ...Typography.default('semiBold'),
                                        }}>
                                            {m.metadata?.displayName || m.metadata?.host || m.id.slice(0, 8)}
                                        </Text>
                                    </Pressable>
                                );
                            })}
                        </ScrollView>
                    </View>
                )}

                {/* Machine status indicator (single machine, hidden when navigated from device detail) */}
                {!routeMachineId && machines.length === 1 && selectedMachine && (
                    <View style={{ flexDirection: 'row', alignItems: 'center', marginBottom: 16 }}>
                        <View style={{
                            width: 8,
                            height: 8,
                            borderRadius: 4,
                            backgroundColor: isMachineOnline(selectedMachine) ? '#34C759' : '#8E8E93',
                            marginRight: 8,
                        }} />
                        <Text style={{ fontSize: 13, color: theme.colors.textSecondary, ...Typography.default() }}>
                            {(selectedMachine.metadata?.displayName || selectedMachine.metadata?.host || selectedMachine.id.slice(0, 8))} · {isMachineOnline(selectedMachine) ? t('mcp.connected') : t('mcp.disconnected')}
                        </Text>
                    </View>
                )}

                {/* Add Server Button */}
                <Pressable
                    onPress={() => setShowAddForm(!showAddForm)}
                    style={({ pressed }) => ({
                        flexDirection: 'row',
                        alignItems: 'center',
                        paddingVertical: 12,
                        paddingHorizontal: 16,
                        backgroundColor: pressed ? theme.colors.surfaceRipple : theme.colors.surface,
                        borderRadius: 12,
                        marginBottom: 16,
                    })}
                >
                    <Ionicons name={showAddForm ? 'chevron-up' : 'add-circle-outline'} size={20} color={theme.colors.textLink} />
                    <Text style={{ fontSize: 15, color: theme.colors.textLink, marginLeft: 8, ...Typography.default('semiBold') }}>
                        {t('mcp.addServer')}
                    </Text>
                </Pressable>

                {/* Add Form — always available, no heartbeat dependency */}
                {showAddForm && (
                    <View style={{
                        backgroundColor: theme.colors.input.background,
                        borderRadius: 12,
                        padding: 16,
                        marginBottom: 16,
                    }}>
                        {/* Transport Type Selector */}
                        <Text style={{ fontSize: 13, color: theme.colors.textSecondary, marginBottom: 8, ...Typography.default('semiBold') }}>
                            {t('mcp.transportType')}
                        </Text>
                        <View style={{ flexDirection: 'row', marginBottom: 16, gap: 8 }}>
                            {(['stdio', 'http_sse'] as const).map((type) => (
                                <Pressable
                                    key={type}
                                    onPress={() => setTransportType(type)}
                                    style={{
                                        flex: 1,
                                        paddingVertical: 8,
                                        paddingHorizontal: 12,
                                        borderRadius: 8,
                                        backgroundColor: transportType === type ? theme.colors.textLink : 'transparent',
                                        borderWidth: 1,
                                        borderColor: transportType === type ? theme.colors.textLink : theme.colors.divider,
                                        alignItems: 'center',
                                    }}
                                >
                                    <Text style={{
                                        fontSize: 13,
                                        color: transportType === type ? '#fff' : theme.colors.text,
                                        ...Typography.default('semiBold'),
                                    }}>
                                        {type === 'stdio' ? t('mcp.stdio') : t('mcp.httpSse')}
                                    </Text>
                                </Pressable>
                            ))}
                        </View>

                        {/* Name */}
                        <Text style={{ fontSize: 13, color: theme.colors.textSecondary, marginBottom: 4, ...Typography.default('semiBold') }}>
                            {t('mcp.serverName')}
                        </Text>
                        <TextInput
                            value={newName}
                            onChangeText={setNewName}
                            placeholder="My MCP Server"
                            placeholderTextColor={theme.colors.textSecondary}
                            style={{
                                backgroundColor: theme.colors.surface,
                                borderRadius: 8,
                                padding: 10,
                                fontSize: 14,
                                color: theme.colors.text,
                                marginBottom: 12,
                            }}
                        />

                        {transportType === 'stdio' ? (
                            <>
                                <Text style={{ fontSize: 13, color: theme.colors.textSecondary, marginBottom: 4, ...Typography.default('semiBold') }}>
                                    {t('mcp.command')}
                                </Text>
                                <TextInput
                                    value={newCommand}
                                    onChangeText={setNewCommand}
                                    placeholder={t('mcp.commandPlaceholder')}
                                    placeholderTextColor={theme.colors.textSecondary}
                                    style={{
                                        backgroundColor: theme.colors.surface,
                                        borderRadius: 8,
                                        padding: 10,
                                        fontSize: 14,
                                        color: theme.colors.text,
                                        marginBottom: 12,
                                    }}
                                />
                                <Text style={{ fontSize: 13, color: theme.colors.textSecondary, marginBottom: 4, ...Typography.default('semiBold') }}>
                                    {t('mcp.args')}
                                </Text>
                                <TextInput
                                    value={newArgs}
                                    onChangeText={setNewArgs}
                                    placeholder={t('mcp.argsPlaceholder')}
                                    placeholderTextColor={theme.colors.textSecondary}
                                    style={{
                                        backgroundColor: theme.colors.surface,
                                        borderRadius: 8,
                                        padding: 10,
                                        fontSize: 14,
                                        color: theme.colors.text,
                                        marginBottom: 12,
                                    }}
                                />
                                <Text style={{ fontSize: 13, color: theme.colors.textSecondary, marginBottom: 4, ...Typography.default('semiBold') }}>
                                    {t('mcp.envVars')}
                                </Text>
                                <TextInput
                                    value={newEnv}
                                    onChangeText={setNewEnv}
                                    placeholder="KEY=value (one per line)"
                                    placeholderTextColor={theme.colors.textSecondary}
                                    multiline
                                    style={{
                                        backgroundColor: theme.colors.surface,
                                        borderRadius: 8,
                                        padding: 10,
                                        fontSize: 14,
                                        color: theme.colors.text,
                                        marginBottom: 12,
                                        minHeight: 60,
                                    }}
                                />
                            </>
                        ) : (
                            <>
                                <Text style={{ fontSize: 13, color: theme.colors.textSecondary, marginBottom: 4, ...Typography.default('semiBold') }}>
                                    {t('mcp.url')}
                                </Text>
                                <TextInput
                                    value={newUrl}
                                    onChangeText={setNewUrl}
                                    placeholder={t('mcp.urlPlaceholder')}
                                    placeholderTextColor={theme.colors.textSecondary}
                                    autoCapitalize="none"
                                    style={{
                                        backgroundColor: theme.colors.surface,
                                        borderRadius: 8,
                                        padding: 10,
                                        fontSize: 14,
                                        color: theme.colors.text,
                                        marginBottom: 12,
                                    }}
                                />
                                <Text style={{ fontSize: 13, color: theme.colors.textSecondary, marginBottom: 4, ...Typography.default('semiBold') }}>
                                    {t('mcp.headers')}
                                </Text>
                                <TextInput
                                    value={newHeaders}
                                    onChangeText={setNewHeaders}
                                    placeholder="Authorization=Bearer xxx (one per line)"
                                    placeholderTextColor={theme.colors.textSecondary}
                                    multiline
                                    style={{
                                        backgroundColor: theme.colors.surface,
                                        borderRadius: 8,
                                        padding: 10,
                                        fontSize: 14,
                                        color: theme.colors.text,
                                        marginBottom: 12,
                                        minHeight: 60,
                                    }}
                                />
                            </>
                        )}

                        {/* Buttons */}
                        <View style={{ flexDirection: 'row', gap: 8, marginTop: 4 }}>
                            <Pressable
                                onPress={() => { resetForm(); setShowAddForm(false); }}
                                style={({ pressed }) => ({
                                    flex: 1,
                                    paddingVertical: 10,
                                    borderRadius: 8,
                                    backgroundColor: pressed ? theme.colors.surfaceRipple : 'transparent',
                                    borderWidth: 1,
                                    borderColor: theme.colors.divider,
                                    alignItems: 'center',
                                })}
                            >
                                <Text style={{ fontSize: 14, color: theme.colors.text, ...Typography.default('semiBold') }}>
                                    {t('common.cancel')}
                                </Text>
                            </Pressable>
                            <Pressable
                                onPress={handleAdd}
                                disabled={saving}
                                style={({ pressed }) => ({
                                    flex: 1,
                                    paddingVertical: 10,
                                    borderRadius: 8,
                                    backgroundColor: pressed ? '#0056b3' : theme.colors.textLink,
                                    alignItems: 'center',
                                    opacity: saving ? 0.6 : 1,
                                })}
                            >
                                {saving ? (
                                    <ActivityIndicator size="small" color="#fff" />
                                ) : (
                                    <Text style={{ fontSize: 14, color: '#fff', ...Typography.default('semiBold') }}>
                                        {t('common.save')}
                                    </Text>
                                )}
                            </Pressable>
                        </View>
                    </View>
                )}

                {/* Server List — show cached data instantly, refresh indicator is inline */}
                {servers.length === 0 && !showAddForm && !refreshing && (
                    <View style={{ paddingVertical: 48, alignItems: 'center' }}>
                        <Ionicons name="git-network-outline" size={48} color={theme.colors.textSecondary} style={{ opacity: 0.5 }} />
                        <Text style={{ fontSize: 16, color: theme.colors.textSecondary, marginTop: 12, ...Typography.default() }}>
                            {t('mcp.noServers')}
                        </Text>
                        <Text style={{ fontSize: 13, color: theme.colors.textSecondary, marginTop: 4, ...Typography.default() }}>
                            {t('mcp.noServersDescription')}
                        </Text>
                    </View>
                )}

                {/* Inline refresh indicator — never blocks the page */}
                {refreshing && servers.length === 0 && (
                    <View style={{ paddingVertical: 16, alignItems: 'center' }}>
                        <ActivityIndicator size="small" color={theme.colors.textSecondary} />
                    </View>
                )}

                {servers.map((server) => (
                    <View
                        key={server.id}
                        style={{
                            backgroundColor: theme.colors.input.background,
                            borderRadius: 12,
                            padding: 16,
                            marginBottom: 8,
                        }}
                    >
                        <View style={{ flexDirection: 'row', alignItems: 'center', marginBottom: 8 }}>
                            <View style={{
                                width: 8,
                                height: 8,
                                borderRadius: 4,
                                backgroundColor: server.status === 'connected' ? '#34C759' :
                                    server.status === 'error' ? '#FF3B30' : '#8E8E93',
                                marginRight: 8,
                            }} />
                            <Text style={{ flex: 1, fontSize: 16, color: theme.colors.text, ...Typography.default('semiBold') }}>
                                {server.name}
                            </Text>
                            <Switch
                                value={server.enabled}
                                onValueChange={(val) => handleToggle(server, val)}
                            />
                        </View>
                        <View style={{ flexDirection: 'row', alignItems: 'center', gap: 8, marginBottom: 4 }}>
                            <Text style={{
                                fontSize: 11,
                                color: '#fff',
                                backgroundColor: server.transport === 'stdio' ? '#5856D6' : '#FF9500',
                                paddingHorizontal: 6,
                                paddingVertical: 2,
                                borderRadius: 4,
                                overflow: 'hidden',
                                ...Typography.default('semiBold'),
                            }}>
                                {server.transport === 'stdio' ? 'stdio' : 'HTTP SSE'}
                            </Text>
                            <Text style={{ fontSize: 12, color: theme.colors.textSecondary, ...Typography.default() }}>
                                {server.command || server.url || ''}
                            </Text>
                        </View>
                        <View style={{ flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between', marginTop: 8 }}>
                            <Text style={{ fontSize: 12, color: theme.colors.textSecondary, ...Typography.default() }}>
                                {server.toolCount} {t('mcp.tools')} · {server.status === 'connected' ? t('mcp.connected') :
                                    server.status === 'error' ? t('mcp.error') : t('mcp.disconnected')}
                            </Text>
                            <Pressable
                                onPress={() => handleDelete(server)}
                                hitSlop={8}
                            >
                                <Ionicons name="trash-outline" size={18} color="#FF3B30" />
                            </Pressable>
                        </View>
                        {server.error && (
                            <Text style={{ fontSize: 11, color: '#FF3B30', marginTop: 4, ...Typography.default() }}>
                                {server.error}
                            </Text>
                        )}
                    </View>
                ))}
            </View>
        </ScrollView>
    );
}

/** Parse "KEY=value" lines into a Record */
function parseKeyValuePairs(input: string): Record<string, string> {
    const result: Record<string, string> = {};
    for (const line of input.split('\n')) {
        const trimmed = line.trim();
        if (!trimmed) continue;
        const eqIdx = trimmed.indexOf('=');
        if (eqIdx > 0) {
            result[trimmed.slice(0, eqIdx).trim()] = trimmed.slice(eqIdx + 1).trim();
        }
    }
    return result;
}

export default function MCPScreen() {
    return <MCPManager />;
}
