import React from 'react';
import { Modal as RNModal, Pressable, ScrollView, View, useWindowDimensions } from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { useUnistyles } from 'react-native-unistyles';

import { Text } from '@/components/StyledText';
import { CodeView } from '@/components/CodeView';
import { Typography } from '@/constants/Typography';
import type { BackgroundTaskRecord } from '@/hooks/useBackgroundTasks';
import type { ToolCallMessage } from '@/sync/typesMessage';
import { ToolFullView } from '@/components/tools/ToolFullView';
import { ToolHeader } from '@/components/tools/ToolHeader';
import { ToolStatusIndicator } from '@/components/tools/ToolStatusIndicator';

interface BackgroundTaskDetailSheetProps {
    visible: boolean;
    task: BackgroundTaskRecord | null;
    toolMessage?: ToolCallMessage | null;
    onClose: () => void;
}

function formatTaskStatus(status: BackgroundTaskRecord['status']) {
    switch (status) {
        case 'running':
            return '运行中';
        case 'completed':
            return '已完成';
        case 'failed':
            return '失败';
        case 'cancelled':
            return '已取消';
        case 'paused':
            return '已暂停';
        default:
            return '未知状态';
    }
}

function formatTaskTime(task: BackgroundTaskRecord) {
    const startedAt = task.startedAt > 0 ? new Date(task.startedAt).toLocaleString() : '未知';
    const completedAt = task.completedAt ? new Date(task.completedAt).toLocaleString() : null;
    return completedAt ? `${startedAt} -> ${completedAt}` : startedAt;
}

function InfoRow(props: { label: string; value?: string | null }) {
    const { theme } = useUnistyles();

    if (!props.value) {
        return null;
    }

    return (
        <View style={{
            flexDirection: 'row',
            alignItems: 'flex-start',
            gap: 12,
            marginBottom: 12,
        }}>
            <Text style={{
                width: 88,
                fontSize: 13,
                color: theme.colors.textSecondary,
                ...Typography.default('semiBold'),
            }}>
                {props.label}
            </Text>
            <Text style={{
                flex: 1,
                fontSize: 14,
                color: theme.colors.text,
                lineHeight: 20,
                ...Typography.default(),
            }}>
                {props.value}
            </Text>
        </View>
    );
}

export function BackgroundTaskDetailSheet({ visible, task, toolMessage, onClose }: BackgroundTaskDetailSheetProps) {
    const { theme } = useUnistyles();
    const { width: windowWidth, height: windowHeight } = useWindowDimensions();

    if (!visible || !task) {
        return null;
    }

    return (
        <RNModal
            visible={visible}
            transparent
            animationType="slide"
            onRequestClose={onClose}
        >
            <View style={{
                flex: 1,
                backgroundColor: 'rgba(0, 0, 0, 0.28)',
                justifyContent: 'flex-end',
            }}>
                <Pressable style={{ flex: 1 }} onPress={onClose} />
                <View style={{
                    alignSelf: 'center',
                    width: Math.min(windowWidth * 0.96, 760),
                    maxHeight: windowHeight * 0.85,
                    backgroundColor: theme.colors.surface,
                    borderTopLeftRadius: 20,
                    borderTopRightRadius: 20,
                    overflow: 'hidden',
                    shadowColor: theme.colors.shadow.color,
                    shadowOffset: { width: 0, height: -2 },
                    shadowOpacity: 0.2,
                    shadowRadius: 10,
                    elevation: 8,
                }}>
                    <View style={{
                        paddingHorizontal: 20,
                        paddingTop: 16,
                        paddingBottom: 12,
                        borderBottomWidth: 0.5,
                        borderBottomColor: theme.colors.divider,
                    }}>
                        <View style={{
                            flexDirection: 'row',
                            alignItems: 'flex-start',
                            justifyContent: 'space-between',
                            gap: 12,
                        }}>
                            <View style={{ flex: 1 }}>
                                <Text style={{
                                    fontSize: 17,
                                    color: theme.colors.text,
                                    ...Typography.default('semiBold'),
                                }} numberOfLines={2}>
                                    {task.summary ?? task.description ?? '后台任务详情'}
                                </Text>
                                <Text style={{
                                    fontSize: 12,
                                    color: theme.colors.textSecondary,
                                    marginTop: 4,
                                    ...Typography.default(),
                                }} selectable>
                                    {task.taskId}
                                </Text>
                            </View>
                            <Pressable
                                onPress={onClose}
                                style={({ pressed }) => ({
                                    width: 32,
                                    height: 32,
                                    borderRadius: 16,
                                    alignItems: 'center',
                                    justifyContent: 'center',
                                    backgroundColor: pressed ? theme.colors.surfaceRipple : theme.colors.surfaceHighest,
                                })}
                            >
                                <Ionicons name="close" size={18} color={theme.colors.textSecondary} />
                            </Pressable>
                        </View>

                        {toolMessage ? (
                            <View style={{
                                marginTop: 12,
                                flexDirection: 'row',
                                alignItems: 'center',
                                justifyContent: 'space-between',
                                gap: 12,
                            }}>
                                <ToolHeader tool={toolMessage.tool} />
                                <ToolStatusIndicator tool={toolMessage.tool} />
                            </View>
                        ) : (
                            <Text style={{
                                fontSize: 13,
                                color: theme.colors.textSecondary,
                                marginTop: 8,
                                lineHeight: 18,
                                ...Typography.default(),
                            }}>
                                当前任务还没有可复用的 tool-call 详情视图，先展示跟踪记录。
                            </Text>
                        )}
                    </View>

                    {toolMessage ? (
                        <ToolFullView tool={toolMessage.tool} messages={toolMessage.children} />
                    ) : (
                        <ScrollView contentContainerStyle={{ padding: 20 }}>
                            <InfoRow label="状态" value={formatTaskStatus(task.status)} />
                            <InfoRow label="类型" value={task.taskType} />
                            <InfoRow label="Vendor" value={task.vendor || 'unknown'} />
                            <InfoRow label="时间" value={formatTaskTime(task)} />
                            <InfoRow label="描述" value={task.description ?? null} />
                            <InfoRow label="摘要" value={task.summary ?? null} />
                            {task.outputFile ? <InfoRow label="输出文件" value={task.outputFile} /> : null}

                            <View style={{ marginTop: 8 }}>
                                <Text style={{
                                    fontSize: 13,
                                    color: theme.colors.textSecondary,
                                    marginBottom: 8,
                                    ...Typography.default('semiBold'),
                                }}>
                                    原始记录
                                </Text>
                                <CodeView code={JSON.stringify(task, null, 2)} />
                            </View>
                        </ScrollView>
                    )}
                </View>
            </View>
        </RNModal>
    );
}
