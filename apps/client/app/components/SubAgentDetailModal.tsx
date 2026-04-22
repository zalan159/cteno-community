import React, { useState } from 'react';
import { View, ScrollView, Pressable, Modal, useWindowDimensions } from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { useUnistyles } from 'react-native-unistyles';
import { Typography } from '@/constants/Typography';
import { Text } from '@/components/StyledText';
import type { SubAgent, SubAgentStatus } from '../sync/ops';
import * as Clipboard from 'expo-clipboard';

interface SubAgentDetailModalProps {
    subagent: SubAgent | null;
    visible: boolean;
    onClose: () => void;
    onStop?: (id: string) => Promise<void>;
    onRetry?: (subagent: SubAgent) => void;
}

const getStatusColor = (status: SubAgentStatus) => {
    switch (status) {
        case 'running': return '#007AFF';
        case 'pending': return '#8E8E93';
        case 'completed': return '#34C759';
        case 'failed': return '#FF3B30';
        case 'stopped': return '#FF9500';
        case 'timed_out': return '#FF9500';
        default: return '#8E8E93';
    }
};

const getStatusIcon = (status: SubAgentStatus): string => {
    switch (status) {
        case 'running': return 'sync-circle';
        case 'pending': return 'time-outline';
        case 'completed': return 'checkmark-circle';
        case 'failed': return 'close-circle';
        case 'stopped': return 'stop-circle';
        case 'timed_out': return 'time';
        default: return 'help-circle';
    }
};

const getStatusText = (status: SubAgentStatus) => {
    switch (status) {
        case 'pending': return '等待启动';
        case 'running': return '运行中';
        case 'completed': return '已完成';
        case 'failed': return '失败';
        case 'stopped': return '已停止';
        case 'timed_out': return '超时';
        default: return status;
    }
};

export const SubAgentDetailModal: React.FC<SubAgentDetailModalProps> = ({
    subagent,
    visible,
    onClose,
    onStop,
    onRetry,
}) => {
    const { theme } = useUnistyles();
    const { width: windowWidth, height: windowHeight } = useWindowDimensions();
    const [copied, setCopied] = useState(false);
    const modalWidth = Math.min(windowWidth * 0.9, 600);
    const modalMaxHeight = windowHeight * 0.8;

    if (!subagent) return null;

    const formatTime = (timestamp: number) => {
        const date = new Date(timestamp);
        return date.toLocaleString();
    };

    const formatDuration = (sa: SubAgent) => {
        if (!sa.started_at) return '-';
        const end = sa.completed_at || Date.now();
        const duration = Math.floor((end - sa.started_at) / 1000);
        const mins = Math.floor(duration / 60);
        const secs = duration % 60;
        if (mins === 0) return `${secs} 秒`;
        return `${mins} 分 ${secs} 秒`;
    };

    const statusColor = getStatusColor(subagent.status);

    const copyResult = async () => {
        if (subagent.result) {
            await Clipboard.setStringAsync(subagent.result);
            setCopied(true);
            setTimeout(() => setCopied(false), 2000);
        }
    };

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
                            SubAgent 详情
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
                        {/* Status + Label header */}
                        <View style={{ flexDirection: 'row', alignItems: 'center', gap: 10, marginBottom: 16 }}>
                            <Ionicons
                                name={getStatusIcon(subagent.status) as any}
                                size={28}
                                color={statusColor}
                            />
                            <View style={{ flex: 1 }}>
                                <Text style={{
                                    fontSize: 16,
                                    color: theme.colors.text,
                                    ...Typography.default('semiBold'),
                                }} numberOfLines={2}>
                                    {subagent.label || '(无标签)'}
                                </Text>
                                <Text style={{
                                    fontSize: 13,
                                    color: statusColor,
                                    marginTop: 2,
                                    ...Typography.default('semiBold'),
                                }}>
                                    {getStatusText(subagent.status)}
                                </Text>
                            </View>
                        </View>

                        {/* Info rows */}
                        <View style={{ marginBottom: 16 }}>
                            <InfoRow label="Agent ID" value={subagent.agent_id} theme={theme} />
                            <InfoRow label="创建时间" value={formatTime(subagent.created_at)} theme={theme} />
                            {subagent.started_at && (
                                <InfoRow label="开始时间" value={formatTime(subagent.started_at)} theme={theme} />
                            )}
                            {subagent.completed_at && (
                                <InfoRow label="完成时间" value={formatTime(subagent.completed_at)} theme={theme} />
                            )}
                            <InfoRow label="运行时长" value={formatDuration(subagent)} theme={theme} />
                            <InfoRow label="迭代次数" value={`${subagent.iteration_count} 轮`} theme={theme} />
                        </View>

                        {/* Task Description */}
                        <View style={{ marginBottom: 16 }}>
                            <Text style={{
                                fontSize: 13,
                                color: theme.colors.textSecondary,
                                marginBottom: 6,
                                ...Typography.default('semiBold'),
                                textTransform: 'uppercase',
                                letterSpacing: 0.5,
                            }}>
                                任务描述
                            </Text>
                            <Text style={{
                                fontSize: 14,
                                color: theme.colors.text,
                                lineHeight: 20,
                                ...Typography.default(),
                            }}>
                                {subagent.task}
                            </Text>
                        </View>

                        {/* Result (if completed) */}
                        {subagent.result && subagent.status === 'completed' && (
                            <View style={{ marginBottom: 16 }}>
                                <View style={{
                                    flexDirection: 'row',
                                    justifyContent: 'space-between',
                                    alignItems: 'center',
                                    marginBottom: 6,
                                }}>
                                    <Text style={{
                                        fontSize: 13,
                                        color: theme.colors.textSecondary,
                                        ...Typography.default('semiBold'),
                                        textTransform: 'uppercase',
                                        letterSpacing: 0.5,
                                    }}>
                                        执行结果
                                    </Text>
                                    <Pressable
                                        onPress={copyResult}
                                        style={({ pressed }) => ({
                                            paddingHorizontal: 10,
                                            paddingVertical: 4,
                                            borderRadius: 10,
                                            backgroundColor: pressed
                                                ? theme.colors.surfacePressed
                                                : theme.colors.surface,
                                            borderWidth: 1,
                                            borderColor: theme.colors.divider,
                                        })}
                                    >
                                        <Text style={{
                                            fontSize: 12,
                                            color: theme.colors.textLink,
                                            ...Typography.default('semiBold'),
                                        }}>
                                            {copied ? '已复制' : '复制'}
                                        </Text>
                                    </Pressable>
                                </View>
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
                                        fontSize: 13,
                                        color: theme.colors.text,
                                        fontFamily: 'Courier New',
                                        lineHeight: 18,
                                    }}>
                                        {subagent.result}
                                    </Text>
                                </ScrollView>
                            </View>
                        )}

                        {/* Error (if failed) */}
                        {subagent.error && (subagent.status === 'failed' || subagent.status === 'timed_out') && (
                            <View style={{ marginBottom: 16 }}>
                                <Text style={{
                                    fontSize: 13,
                                    color: theme.colors.textSecondary,
                                    marginBottom: 6,
                                    ...Typography.default('semiBold'),
                                    textTransform: 'uppercase',
                                    letterSpacing: 0.5,
                                }}>
                                    错误信息
                                </Text>
                                <View style={{
                                    backgroundColor: 'rgba(255, 59, 48, 0.1)',
                                    borderRadius: 8,
                                    padding: 12,
                                    borderLeftWidth: 3,
                                    borderLeftColor: '#FF3B30',
                                }}>
                                    <Text style={{
                                        fontSize: 13,
                                        color: '#FF3B30',
                                        fontFamily: 'Courier New',
                                        lineHeight: 18,
                                    }}>
                                        {subagent.error}
                                    </Text>
                                </View>
                            </View>
                        )}
                    </ScrollView>

                    {/* Bottom actions */}
                    <View style={{
                        flexDirection: 'row',
                        gap: 8,
                        paddingHorizontal: 20,
                        paddingVertical: 12,
                        borderTopWidth: 0.5,
                        borderTopColor: theme.colors.divider,
                    }}>
                        {subagent.status === 'running' && onStop && (
                            <Pressable
                                onPress={() => onStop(subagent.id)}
                                style={({ pressed }) => ({
                                    flex: 1,
                                    paddingVertical: 10,
                                    borderRadius: 10,
                                    alignItems: 'center',
                                    backgroundColor: pressed
                                        ? theme.colors.surfacePressed
                                        : theme.colors.surface,
                                    borderWidth: 1,
                                    borderColor: theme.colors.divider,
                                })}
                            >
                                <Text style={{
                                    fontSize: 15,
                                    color: theme.colors.textDestructive,
                                    ...Typography.default('semiBold'),
                                }}>
                                    停止任务
                                </Text>
                            </Pressable>
                        )}
                        {subagent.status === 'failed' && onRetry && (
                            <Pressable
                                onPress={() => onRetry(subagent)}
                                style={({ pressed }) => ({
                                    flex: 1,
                                    paddingVertical: 10,
                                    borderRadius: 10,
                                    alignItems: 'center',
                                    backgroundColor: pressed
                                        ? theme.colors.surfacePressed
                                        : theme.colors.surface,
                                    borderWidth: 1,
                                    borderColor: theme.colors.divider,
                                })}
                            >
                                <Text style={{
                                    fontSize: 15,
                                    color: theme.colors.textLink,
                                    ...Typography.default('semiBold'),
                                }}>
                                    重新运行
                                </Text>
                            </Pressable>
                        )}
                        <Pressable
                            onPress={onClose}
                            style={({ pressed }) => ({
                                flex: 1,
                                paddingVertical: 10,
                                borderRadius: 10,
                                alignItems: 'center',
                                backgroundColor: pressed ? theme.colors.surfaceRipple : 'transparent',
                            })}
                        >
                            <Text style={{
                                fontSize: 15,
                                color: theme.colors.textLink,
                                ...Typography.default('semiBold'),
                            }}>
                                关闭
                            </Text>
                        </Pressable>
                    </View>
                </View>
            </View>
        </Modal>
    );
};

function InfoRow({ label, value, theme }: { label: string; value: string; theme: any }) {
    return (
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
            }}>
                {label}
            </Text>
            <Text style={{
                fontSize: 14,
                color: theme.colors.text,
                flex: 2,
                textAlign: 'right',
                ...Typography.default(),
            }}>
                {value}
            </Text>
        </View>
    );
}
