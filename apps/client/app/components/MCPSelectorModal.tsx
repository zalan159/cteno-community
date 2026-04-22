import React from 'react';
import { View, Pressable, ScrollView } from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { useUnistyles } from 'react-native-unistyles';
import { Typography } from '@/constants/Typography';
import { t } from '@/text';
import type { MCPServerItem } from '@/sync/ops';
import { Text } from '@/components/StyledText';

interface MCPSelectorModalProps {
    servers: MCPServerItem[];
    activeServerIds: string[];
    onSelectionChange: (serverIds: string[]) => void;
    onClose: () => void;
}

export function MCPSelectorModal({ servers, activeServerIds, onSelectionChange, onClose }: MCPSelectorModalProps) {
    const { theme } = useUnistyles();
    const [selectedIds, setSelectedIds] = React.useState<string[]>(activeServerIds);

    const toggleServer = (prefix: string) => {
        if (selectedIds.includes(prefix)) {
            setSelectedIds(selectedIds.filter(id => id !== prefix));
        } else {
            setSelectedIds([...selectedIds, prefix]);
        }
    };

    const selectAll = () => {
        setSelectedIds(servers.map(s => s.toolNamePrefix));
    };

    const clearAll = () => {
        setSelectedIds([]);
    };

    const allSelected = servers.length > 0 && selectedIds.length === servers.length;

    const handleDone = () => {
        onSelectionChange(selectedIds);
        onClose();
    };

    const isServerActive = (serverId: string) => selectedIds.includes(serverId);

    return (
        <View style={{
            backgroundColor: theme.colors.surface,
            borderRadius: 14,
            width: 320,
            maxHeight: 480,
            overflow: 'hidden',
            shadowColor: theme.colors.shadow.color,
            shadowOffset: { width: 0, height: 2 },
            shadowOpacity: 0.25,
            shadowRadius: 4,
            elevation: 5,
        }}>
            {/* Header */}
            <View style={{
                paddingHorizontal: 20,
                paddingTop: 20,
                paddingBottom: 12,
            }}>
                <Text style={{
                    fontSize: 17,
                    textAlign: 'center',
                    color: theme.colors.text,
                    ...Typography.default('semiBold'),
                }}>
                    {t('mcp.selectMcp')}
                </Text>
                <Text style={{
                    fontSize: 13,
                    textAlign: 'center',
                    color: theme.colors.textSecondary,
                    marginTop: 4,
                    lineHeight: 18,
                    ...Typography.default(),
                }}>
                    {t('mcp.selectMcpDescription')}
                </Text>
            </View>

            {/* Server list */}
            <ScrollView style={{ maxHeight: 320 }}>
                {/* Select All / Clear All toggle */}
                <Pressable
                    onPress={allSelected ? clearAll : selectAll}
                    style={({ pressed }) => ({
                        flexDirection: 'row',
                        alignItems: 'center',
                        paddingHorizontal: 20,
                        paddingVertical: 12,
                        borderTopWidth: 0.5,
                        borderTopColor: theme.colors.divider,
                        backgroundColor: pressed ? theme.colors.surfaceRipple : 'transparent',
                    })}
                >
                    <View style={{
                        width: 22,
                        height: 22,
                        borderRadius: 11,
                        backgroundColor: allSelected ? theme.colors.textLink : 'transparent',
                        borderWidth: allSelected ? 0 : 1.5,
                        borderColor: theme.colors.textSecondary,
                        alignItems: 'center',
                        justifyContent: 'center',
                        marginRight: 12,
                    }}>
                        {allSelected && (
                            <Ionicons name="checkmark" size={14} color="#fff" />
                        )}
                    </View>
                    <Text style={{
                        fontSize: 15,
                        color: theme.colors.text,
                        flex: 1,
                        ...Typography.default('semiBold'),
                    }}>
                        {allSelected ? t('mcp.clearAll') : t('mcp.selectAll')}
                    </Text>
                </Pressable>

                {/* Individual servers */}
                {servers.map((server) => {
                    const active = isServerActive(server.toolNamePrefix);
                    return (
                        <Pressable
                            key={server.id}
                            onPress={() => toggleServer(server.toolNamePrefix)}
                            style={({ pressed }) => ({
                                flexDirection: 'row',
                                alignItems: 'center',
                                paddingHorizontal: 20,
                                paddingVertical: 12,
                                borderTopWidth: 0.5,
                                borderTopColor: theme.colors.divider,
                                backgroundColor: pressed ? theme.colors.surfaceRipple : 'transparent',
                            })}
                        >
                            <View style={{
                                width: 22,
                                height: 22,
                                borderRadius: 6,
                                backgroundColor: active ? theme.colors.textLink : 'transparent',
                                borderWidth: active ? 0 : 1.5,
                                borderColor: theme.colors.textSecondary,
                                alignItems: 'center',
                                justifyContent: 'center',
                                marginRight: 12,
                            }}>
                                {active && (
                                    <Ionicons name="checkmark" size={14} color="#fff" />
                                )}
                            </View>
                            <View style={{ flex: 1 }}>
                                <View style={{ flexDirection: 'row', alignItems: 'center', gap: 6 }}>
                                    <View style={{
                                        width: 6,
                                        height: 6,
                                        borderRadius: 3,
                                        backgroundColor: server.status === 'connected' ? '#34C759' :
                                            server.status === 'error' ? '#FF3B30' : '#8E8E93',
                                    }} />
                                    <Text style={{
                                        fontSize: 15,
                                        color: theme.colors.text,
                                        ...Typography.default('semiBold'),
                                    }}>
                                        {server.name}
                                    </Text>
                                </View>
                                <Text style={{
                                    fontSize: 12,
                                    color: theme.colors.textSecondary,
                                    marginTop: 2,
                                    ...Typography.default(),
                                }} numberOfLines={1}>
                                    {server.scope ? `${server.scope} · ` : ''}{server.toolCount} {t('mcp.tools')} · {server.transport === 'stdio' ? 'stdio' : 'HTTP SSE'}
                                </Text>
                            </View>
                        </Pressable>
                    );
                })}
            </ScrollView>

            {/* Done button */}
            <Pressable
                onPress={handleDone}
                style={({ pressed }) => ({
                    borderTopWidth: 0.5,
                    borderTopColor: theme.colors.divider,
                    paddingVertical: 12,
                    alignItems: 'center',
                    justifyContent: 'center',
                    backgroundColor: pressed ? theme.colors.surfaceRipple : 'transparent',
                })}
            >
                <Text style={{
                    fontSize: 17,
                    color: theme.colors.textLink,
                    ...Typography.default('semiBold'),
                }}>
                    {t('common.ok')}
                </Text>
            </Pressable>
        </View>
    );
}
