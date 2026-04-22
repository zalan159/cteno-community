import React from 'react';
import { View, Pressable, ScrollView, ActivityIndicator, useWindowDimensions } from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { useUnistyles } from 'react-native-unistyles';
import { Typography } from '@/constants/Typography';
import type { RunRecord, RunStatus, SubAgent, SubAgentStatus, ScheduledTask } from '@/sync/ops';
import { machineStopRun, machineGetRunLogs, machineListRuns } from '@/sync/ops';
import { Text } from '@/components/StyledText';
import type { OrchestrationFlow, PersonaTaskSummary } from '@/sync/storageTypes';
import { OrchestrationFlowCompact } from '@/components/OrchestrationFlowView';
import { useSession, useSessionMessages, useSessionTaskLifecycle } from '@/sync/storage';
import { useBackgroundTasks, type BackgroundTaskRecord } from '@/hooks/useBackgroundTasks';
import { deriveAgentBackgroundTasks, type AgentBackgroundTaskItem } from '@/utils/agentBackgroundTasks';
import type { Message, ToolCallMessage } from '@/sync/typesMessage';
import { BackgroundTaskDetailSheet } from '@/components/BackgroundTaskDetailSheet';

export interface TaskSessionItem extends PersonaTaskSummary {
    session?: { thinking?: boolean; presence?: string; metadata?: any };
}

interface BackgroundRunsModalProps {
    machineId: string;
    sessionId: string;
    runs: RunRecord[];
    subagents?: SubAgent[];
    scheduledTasks?: ScheduledTask[];
    taskSessions?: TaskSessionItem[];
    agentTasks?: AgentBackgroundTaskItem[];
    orchestrationFlow?: OrchestrationFlow | null;
    onStopSubAgent?: (id: string) => Promise<void>;
    onViewSubAgentDetail?: (subagent: SubAgent) => void;
    onViewScheduledTaskDetail?: (task: ScheduledTask) => void;
    onToggleScheduledTask?: (id: string, enabled: boolean) => Promise<void>;
    onViewTaskSession?: (sessionId: string) => void;
    onViewAgentTask?: (task: AgentBackgroundTaskItem) => void;
    onClose: () => void;
    onRefresh: () => void;
}

const BACKGROUND_TASK_BUCKETS: Array<{ type: BackgroundTaskRecord['taskType']; label: string }> = [
    { type: 'agent', label: 'Agent 任务' },
    { type: 'bash', label: 'Bash 任务' },
    { type: 'workflow', label: 'Workflow 任务' },
    { type: 'remote_agent', label: 'Remote Agent 任务' },
    { type: 'teammate', label: 'Teammate 任务' },
    { type: 'scheduled_job', label: 'Scheduled Job 任务' },
    { type: 'background_session', label: 'Background Session' },
    { type: 'other', label: '其他后台任务' },
];

function normalizeCompatTaskType(taskType: string | null): BackgroundTaskRecord['taskType'] {
    switch (taskType) {
        case 'agent':
        case 'bash':
        case 'workflow':
        case 'remote_agent':
        case 'teammate':
        case 'scheduled_job':
        case 'background_session':
        case 'other':
            return taskType;
        default:
            return 'other';
    }
}

function mapLegacyAgentTaskToRecord(task: AgentBackgroundTaskItem): BackgroundTaskRecord {
    return {
        taskId: task.taskId ?? task.callId ?? task.key,
        sessionId: task.sessionId,
        vendor: task.vendor,
        category: 'execution',
        taskType: normalizeCompatTaskType(task.taskType),
        description: task.title,
        summary: task.subtitle ?? undefined,
        status: task.state === 'running' ? 'running' : task.state === 'error' ? 'failed' : 'completed',
        startedAt: task.startedAt ?? task.createdAt,
        completedAt: task.completedAt ?? undefined,
        toolUseId: task.callId ?? undefined,
        vendorExtra: {
            compatSource: 'legacy_agent_background_tasks',
            messageId: task.messageId,
        },
    };
}

function flattenToolCallMessages(messages: Message[]): ToolCallMessage[] {
    const flattened: ToolCallMessage[] = [];
    const visit = (message: Message) => {
        if (message.kind !== 'tool-call') {
            return;
        }
        flattened.push(message);
        message.children.forEach(visit);
    };
    messages.forEach(visit);
    return flattened;
}

export function BackgroundRunsModal({ machineId, sessionId, runs, subagents = [], scheduledTasks = [], taskSessions = [], agentTasks = [], orchestrationFlow, onStopSubAgent, onViewSubAgentDetail, onViewScheduledTaskDetail, onToggleScheduledTask, onViewTaskSession, onClose, onRefresh }: BackgroundRunsModalProps) {
    const { theme } = useUnistyles();
    const { width: windowWidth, height: windowHeight } = useWindowDimensions();
    const modalWidth = Math.min(windowWidth * 0.9, 500);
    const modalMaxHeight = windowHeight * 0.8;
    const [stoppingIds, setStoppingIds] = React.useState<Set<string>>(new Set());
    const [stoppingSubAgentIds, setStoppingSubAgentIds] = React.useState<Set<string>>(new Set());
    const [viewingLogs, setViewingLogs] = React.useState<{ runId: string; logs: string } | null>(null);
    const [loadingLogs, setLoadingLogs] = React.useState(false);
    const [selectedBackgroundTaskId, setSelectedBackgroundTaskId] = React.useState<string | null>(null);
    const session = useSession(sessionId);
    const { messages } = useSessionMessages(sessionId);
    const taskLifecycle = useSessionTaskLifecycle(sessionId);
    const { tasks: canonicalTasks } = useBackgroundTasks(machineId, sessionId);
    const hasCanonicalTasks = canonicalTasks.length > 0;
    const fallbackAgentTasks = React.useMemo<BackgroundTaskRecord[]>(() => {
        // Compat fallback only: bg-12 removes the legacy session-message reconstruction path.
        if (hasCanonicalTasks) {
            return [];
        }

        const compatTasks = !sessionId || !session
            ? agentTasks
            : deriveAgentBackgroundTasks(messages, sessionId, session.metadata, taskLifecycle, canonicalTasks);
        return compatTasks.map(mapLegacyAgentTaskToRecord);
    }, [agentTasks, canonicalTasks, hasCanonicalTasks, messages, session, sessionId, taskLifecycle]);
    const displayTasks = hasCanonicalTasks ? canonicalTasks : fallbackAgentTasks;
    const toolMessages = React.useMemo(() => flattenToolCallMessages(messages), [messages]);
    const groupedDisplayTasks = React.useMemo(
        () => BACKGROUND_TASK_BUCKETS
            .map((bucket) => ({
                ...bucket,
                tasks: displayTasks.filter((task) => task.taskType === bucket.type),
            }))
            .filter((bucket) => bucket.tasks.length > 0),
        [displayTasks],
    );
    const selectedBackgroundTask = React.useMemo(
        () => displayTasks.find((task) => task.taskId === selectedBackgroundTaskId) ?? null,
        [displayTasks, selectedBackgroundTaskId],
    );
    const selectedBackgroundToolMessage = React.useMemo(() => {
        if (!selectedBackgroundTask) {
            return null;
        }

        if (selectedBackgroundTask.toolUseId) {
            const byToolUseId = toolMessages.find((message) =>
                message.tool.callId === selectedBackgroundTask.toolUseId || message.id === selectedBackgroundTask.toolUseId,
            );
            if (byToolUseId) {
                return byToolUseId;
            }
        }

        const compatMessageId = typeof selectedBackgroundTask.vendorExtra?.messageId === 'string'
            ? selectedBackgroundTask.vendorExtra.messageId
            : null;
        if (compatMessageId) {
            return toolMessages.find((message) => message.id === compatMessageId) ?? null;
        }

        return null;
    }, [selectedBackgroundTask, toolMessages]);

    React.useEffect(() => {
        if (selectedBackgroundTaskId && !selectedBackgroundTask) {
            setSelectedBackgroundTaskId(null);
        }
    }, [selectedBackgroundTask, selectedBackgroundTaskId]);

    // Live-refresh runs inside modal (props are a snapshot)
    const [liveRuns, setLiveRuns] = React.useState(runs);
    React.useEffect(() => {
        if (!machineId || !sessionId) return;
        let mounted = true;
        const poll = async () => {
            try {
                const latest = await machineListRuns(machineId, sessionId);
                if (mounted) setLiveRuns(latest);
            } catch { /* ignore */ }
        };
        const interval = setInterval(poll, 5000);
        return () => { mounted = false; clearInterval(interval); };
    }, [machineId, sessionId]);

    const handleStop = async (runId: string) => {
        setStoppingIds(prev => new Set(prev).add(runId));
        try {
            await machineStopRun(machineId, runId);
            // Refresh runs immediately after stopping
            try {
                const latest = await machineListRuns(machineId, sessionId);
                setLiveRuns(latest);
            } catch { /* ignore */ }
            onRefresh();
        } catch (error) {
            console.error('Failed to stop run:', error);
        } finally {
            setStoppingIds(prev => {
                const next = new Set(prev);
                next.delete(runId);
                return next;
            });
        }
    };

    const handleViewLogs = async (runId: string) => {
        setLoadingLogs(true);
        try {
            const logs = await machineGetRunLogs(machineId, runId, 200);
            setViewingLogs({ runId, logs });
        } catch (error) {
            console.error('Failed to load logs:', error);
        } finally {
            setLoadingLogs(false);
        }
    };

    const normalizeEpochMs = (timestamp: number) => {
        // Compatibility: some backends return Unix seconds, others milliseconds.
        return timestamp > 0 && timestamp < 100000000000 ? timestamp * 1000 : timestamp;
    };

    const formatTime = (startTimestamp: number, endTimestamp: number = Date.now()) => {
        const startMs = normalizeEpochMs(startTimestamp);
        const endMs = normalizeEpochMs(endTimestamp);
        const diff = Math.max(0, endMs - startMs);
        const minutes = Math.floor(diff / 60000);
        const hours = Math.floor(minutes / 60);

        if (hours > 0) {
            return `${hours}h ${minutes % 60}m`;
        } else if (minutes > 0) {
            return `${minutes}m`;
        } else {
            return '<1m';
        }
    };

    const formatRunTime = (run: RunRecord) => {
        const endTimestamp = run.status === 'Running' ? Date.now() : (run.finished_at ?? Date.now());
        return formatTime(run.started_at, endTimestamp);
    };

    const getStatusColor = (status: RunStatus) => {
        switch (status) {
            case 'Running':
                return '#34C759'; // Green
            case 'Exited':
                return '#007AFF'; // Blue
            case 'Failed':
                return '#FF3B30'; // Red
            case 'Killed':
                return '#8E8E93'; // Gray
            case 'TimedOut':
                return '#FF9500'; // Orange
            default:
                return '#8E8E93';
        }
    };

    const getStatusIcon = (status: RunStatus) => {
        switch (status) {
            case 'Running':
                return 'play-circle';
            case 'Exited':
                return 'checkmark-circle';
            case 'Failed':
                return 'close-circle';
            case 'Killed':
                return 'stop-circle';
            case 'TimedOut':
                return 'time';
            default:
                return 'help-circle';
        }
    };

    const getStatusText = (status: RunStatus) => {
        switch (status) {
            case 'Running':
                return '运行中';
            case 'Exited':
                return '已完成';
            case 'Failed':
                return '失败';
            case 'Killed':
                return '已停止';
            case 'TimedOut':
                return '超时';
            default:
                return status;
        }
    };

    const formatToolName = (toolId: string) => {
        const names: Record<string, string> = {
            'shell': 'Shell 命令',
            'image_generation': '图片生成',
        };
        return names[toolId] || toolId;
    };

    // SubAgent helpers
    const getSubAgentStatusColor = (status: SubAgentStatus) => {
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

    const getSubAgentStatusIcon = (status: SubAgentStatus): string => {
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

    const getSubAgentStatusText = (status: SubAgentStatus) => {
        switch (status) {
            case 'running': return '运行中';
            case 'pending': return '等待启动';
            case 'completed': return '已完成';
            case 'failed': return '失败';
            case 'stopped': return '已停止';
            case 'timed_out': return '超时';
            default: return status;
        }
    };

    const formatSubAgentTime = (subagent: SubAgent) => {
        const now = Date.now();
        let elapsed: number;
        if (subagent.status === 'running' && subagent.started_at) {
            elapsed = now - subagent.started_at;
        } else if (subagent.completed_at && subagent.started_at) {
            elapsed = subagent.completed_at - subagent.started_at;
        } else if (subagent.started_at) {
            elapsed = now - subagent.started_at;
        } else {
            elapsed = now - subagent.created_at;
        }
        const seconds = Math.floor(elapsed / 1000);
        const minutes = Math.floor(seconds / 60);
        const hours = Math.floor(minutes / 60);
        if (hours > 0) return `${hours}h ${minutes % 60}m`;
        if (minutes > 0) return `${minutes}m ${seconds % 60}s`;
        return `${seconds}s`;
    };

    const formatBackgroundTaskTime = (task: BackgroundTaskRecord) => {
        const now = Date.now();
        const startedAt = task.startedAt;
        const endAt = task.status === 'running' ? now : (task.completedAt ?? now);
        return formatTime(startedAt, endAt);
    };

    const getBackgroundTaskStatusColor = (task: BackgroundTaskRecord) => {
        switch (task.status) {
            case 'failed':
            case 'cancelled':
                return '#FF3B30';
            case 'running':
                return '#007AFF';
            case 'paused':
                return '#FF9500';
            case 'completed':
                return '#34C759';
            default:
                return '#8E8E93';
        }
    };

    const getBackgroundTaskStatusIcon = (task: BackgroundTaskRecord): string => {
        switch (task.status) {
            case 'failed':
            case 'cancelled':
                return 'alert-circle';
            case 'running':
                return 'sync-circle';
            case 'paused':
                return 'pause-circle';
            case 'completed':
                return 'checkmark-circle';
            default:
                return 'help-circle';
        }
    };

    const getBackgroundTaskStatusText = (task: BackgroundTaskRecord) => {
        switch (task.status) {
            case 'failed':
                return '执行失败';
            case 'cancelled':
                return '已取消';
            case 'running':
                return '运行中';
            case 'paused':
                return '已暂停';
            case 'completed':
                return '已完成';
            default:
                return '状态未知';
        }
    };

    const getBackgroundTaskVendorLabel = (task: BackgroundTaskRecord) => {
        switch (task.vendor) {
            case 'claude':
                return 'Claude';
            case 'codex':
                return 'Codex';
            case 'gemini':
                return 'Gemini';
            case 'cteno':
                return 'Cteno';
            case 'persona':
                return 'Persona';
            default:
                return 'Agent';
        }
    };

    const handleStopSubAgent = async (id: string) => {
        if (!onStopSubAgent) return;
        setStoppingSubAgentIds(prev => new Set(prev).add(id));
        try {
            await onStopSubAgent(id);
            onRefresh();
        } catch (error) {
            console.error('Failed to stop SubAgent:', error);
        } finally {
            setStoppingSubAgentIds(prev => {
                const next = new Set(prev);
                next.delete(id);
                return next;
            });
        }
    };

    // Scheduled task helpers
    const getScheduledTaskStatusColor = (task: ScheduledTask) => {
        if (task.state.running_since) return '#007AFF';
        if (!task.enabled) return '#8E8E93';
        if (task.state.consecutive_errors > 0) return '#FF9500';
        return '#34C759';
    };

    const getScheduledTaskStatusIcon = (task: ScheduledTask): string => {
        if (task.state.running_since) return 'sync-circle';
        if (!task.enabled) return 'pause-circle';
        if (task.state.consecutive_errors > 0) return 'warning';
        return 'timer-outline';
    };

    const getScheduledTaskStatusText = (task: ScheduledTask) => {
        if (task.state.running_since) return '执行中';
        if (!task.enabled) return '已禁用';
        if (task.state.consecutive_errors > 0) return `错误 x${task.state.consecutive_errors}`;
        if (task.delete_after_run) return '一次性';
        return '已启用';
    };

    const formatScheduleType = (task: ScheduledTask) => {
        const s = task.schedule;
        if (s.kind === 'at') return `单次: ${s.at}`;
        if (s.kind === 'every') {
            const secs = s.every_seconds;
            if (secs < 60) return `每 ${secs}s`;
            if (secs < 3600) return `每 ${Math.floor(secs / 60)} 分钟`;
            if (secs < 86400) return `每 ${Math.floor(secs / 3600)} 小时`;
            return `每 ${Math.floor(secs / 86400)} 天`;
        }
        if (s.kind === 'cron') return `Cron: ${s.expr}`;
        return '';
    };

    const formatNextRun = (task: ScheduledTask) => {
        if (!task.state.next_run_at || !task.enabled) return null;
        const diff = task.state.next_run_at - Date.now();
        if (diff <= 0) return '即将执行';
        const minutes = Math.floor(diff / 60000);
        const hours = Math.floor(minutes / 60);
        if (hours > 0) return `${hours}h ${minutes % 60}m 后`;
        if (minutes > 0) return `${minutes}m 后`;
        return '<1m 后';
    };

    const [togglingTaskIds, setTogglingTaskIds] = React.useState<Set<string>>(new Set());

    const handleToggleScheduledTask = async (id: string, enabled: boolean) => {
        if (!onToggleScheduledTask) return;
        setTogglingTaskIds(prev => new Set(prev).add(id));
        try {
            await onToggleScheduledTask(id, enabled);
            onRefresh();
        } catch (error) {
            console.error('Failed to toggle scheduled task:', error);
        } finally {
            setTogglingTaskIds(prev => {
                const next = new Set(prev);
                next.delete(id);
                return next;
            });
        }
    };

    const totalCount = liveRuns.length + subagents.length + scheduledTasks.length + taskSessions.length + displayTasks.length;
    const runningBackgroundTaskCount = displayTasks.filter((task) => task.status === 'running').length;
    const sectionCount = (subagents.length > 0 ? 1 : 0) + (liveRuns.length > 0 ? 1 : 0) + (scheduledTasks.length > 0 ? 1 : 0) + (taskSessions.length > 0 ? 1 : 0) + groupedDisplayTasks.length;
    const showSectionHeaders = sectionCount > 1;

    // Logs viewer modal
    if (viewingLogs) {
        return (
            <View style={{
                backgroundColor: theme.colors.surface,
                borderRadius: 14,
                width: Math.min(windowWidth * 0.9, 600),
                minHeight: 200,
                maxHeight: modalMaxHeight,
                overflow: 'hidden',
                shadowColor: theme.colors.shadow.color,
                shadowOffset: { width: 0, height: 2 },
                shadowOpacity: 0.25,
                shadowRadius: 4,
                elevation: 5,
                flexDirection: 'column',
            }}>
                {/* Header */}
                <View style={{
                    paddingHorizontal: 20,
                    paddingTop: 20,
                    paddingBottom: 12,
                    borderBottomWidth: 0.5,
                    borderBottomColor: theme.colors.divider,
                    flexShrink: 0,
                }}>
                    <Text style={{
                        fontSize: 17,
                        color: theme.colors.text,
                        ...Typography.default('semiBold'),
                    }}>
                        任务日志
                    </Text>
                    <Text style={{
                        fontSize: 13,
                        color: theme.colors.textSecondary,
                        marginTop: 4,
                        ...Typography.default(),
                    }} selectable>
                        {viewingLogs.runId}
                    </Text>
                </View>

                {/* Logs content */}
                <ScrollView style={{ flexShrink: 1, flexGrow: 1 }} contentContainerStyle={{ padding: 16 }}>
                    <Text style={{
                        fontSize: 12,
                        fontFamily: 'Courier New',
                        color: theme.colors.text,
                        lineHeight: 18,
                    }} selectable>
                        {viewingLogs.logs || '(无日志)'}
                    </Text>
                </ScrollView>

                {/* Close button */}
                <Pressable
                    onPress={() => setViewingLogs(null)}
                    style={({ pressed }) => ({
                        borderTopWidth: 0.5,
                        borderTopColor: theme.colors.divider,
                        paddingVertical: 14,
                        alignItems: 'center',
                        justifyContent: 'center',
                        flexShrink: 0,
                        backgroundColor: pressed ? theme.colors.surfaceRipple : 'transparent',
                    })}
                >
                    <Text style={{
                        fontSize: 17,
                        color: theme.colors.textLink,
                        ...Typography.default('semiBold'),
                    }}>
                        关闭
                    </Text>
                </Pressable>
            </View>
        );
    }

    // Main runs list
    return (
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
                paddingHorizontal: 20,
                paddingTop: 20,
                paddingBottom: 12,
                flexDirection: 'row',
                alignItems: 'center',
                justifyContent: 'space-between',
            }}>
                <View style={{ flex: 1 }}>
                    <View style={{ flexDirection: 'row', alignItems: 'center', gap: 8 }}>
                        <Text style={{
                            fontSize: 17,
                            color: theme.colors.text,
                            ...Typography.default('semiBold'),
                        }}>
                            后台任务
                        </Text>
                        {runningBackgroundTaskCount > 0 && (
                            <View style={{
                                minWidth: 24,
                                height: 24,
                                paddingHorizontal: 8,
                                borderRadius: 12,
                                alignItems: 'center',
                                justifyContent: 'center',
                                backgroundColor: theme.colors.button.primary.background,
                            }}>
                                <Text style={{
                                    fontSize: 12,
                                    color: theme.colors.button.primary.tint,
                                    ...Typography.default('semiBold'),
                                }}>
                                    {runningBackgroundTaskCount}
                                </Text>
                            </View>
                        )}
                    </View>
                    <Text style={{
                        fontSize: 13,
                        color: theme.colors.textSecondary,
                        marginTop: 4,
                        lineHeight: 18,
                        ...Typography.default(),
                    }}>
                        {totalCount === 0
                            ? '当前没有后台任务'
                            : `共 ${totalCount} 个任务`}
                    </Text>
                </View>
                {/* Refresh button */}
                <Pressable
                    onPress={onRefresh}
                    style={({ pressed }) => ({
                        padding: 8,
                        opacity: pressed ? 0.6 : 1,
                    })}
                >
                    <Ionicons
                        name="refresh"
                        size={20}
                        color={theme.colors.textLink}
                    />
                </Pressable>
            </View>

            {/* Combined list */}
            <ScrollView style={{ maxHeight: 400 }}>
                {totalCount === 0 ? (
                    <View style={{
                        paddingVertical: 40,
                        alignItems: 'center',
                    }}>
                        <Ionicons
                            name="checkmark-circle-outline"
                            size={48}
                            color={theme.colors.textSecondary}
                        />
                        <Text style={{
                            fontSize: 15,
                            color: theme.colors.textSecondary,
                            marginTop: 12,
                            ...Typography.default(),
                        }}>
                            暂无后台任务
                        </Text>
                    </View>
                ) : (
                    <>
                        {/* Orchestration flow section */}
                        {orchestrationFlow && (
                            <View style={{
                                paddingHorizontal: 20,
                                paddingVertical: 12,
                                borderBottomWidth: 0.5,
                                borderBottomColor: theme.colors.divider,
                            }}>
                                <OrchestrationFlowCompact flow={orchestrationFlow} />
                            </View>
                        )}

                        {/* SubAgent section */}
                        {subagents.length > 0 && (
                            <>
                                {showSectionHeaders && (
                                    <View style={{
                                        paddingHorizontal: 20,
                                        paddingVertical: 8,
                                        backgroundColor: theme.colors.surfaceHighest,
                                        borderTopWidth: 0.5,
                                        borderTopColor: theme.colors.divider,
                                    }}>
                                        <Text style={{
                                            fontSize: 12,
                                            color: theme.colors.textSecondary,
                                            ...Typography.default('semiBold'),
                                            textTransform: 'uppercase',
                                            letterSpacing: 0.5,
                                        }}>
                                            SubAgent 任务
                                        </Text>
                                    </View>
                                )}
                                {subagents.map((sa) => {
                                    const isStopping = stoppingSubAgentIds.has(sa.id);
                                    const statusColor = getSubAgentStatusColor(sa.status);
                                    const isActive = sa.status === 'running' || sa.status === 'pending';
                                    const displayLabel = sa.label || sa.task.substring(0, 50);

                                    return (
                                        <View
                                            key={sa.id}
                                            style={{
                                                paddingHorizontal: 20,
                                                paddingVertical: 12,
                                                borderTopWidth: 0.5,
                                                borderTopColor: theme.colors.divider,
                                            }}
                                        >
                                            <View style={{ flexDirection: 'row', alignItems: 'flex-start', gap: 12 }}>
                                                <Ionicons
                                                    name={getSubAgentStatusIcon(sa.status) as any}
                                                    size={24}
                                                    color={statusColor}
                                                    style={{ marginTop: 2 }}
                                                />
                                                <View style={{ flex: 1 }}>
                                                    <Text style={{
                                                        fontSize: 15,
                                                        color: theme.colors.text,
                                                        ...Typography.default('semiBold'),
                                                    }} numberOfLines={1}>
                                                        {displayLabel}
                                                    </Text>

                                                    <View style={{
                                                        flexDirection: 'row',
                                                        alignItems: 'center',
                                                        marginTop: 4,
                                                        gap: 8,
                                                    }}>
                                                        <Text style={{
                                                            fontSize: 12,
                                                            color: statusColor,
                                                            ...Typography.default('semiBold'),
                                                        }}>
                                                            {getSubAgentStatusText(sa.status)}
                                                        </Text>
                                                        <Text style={{
                                                            fontSize: 12,
                                                            color: theme.colors.textSecondary,
                                                            ...Typography.default(),
                                                        }}>
                                                            • {formatSubAgentTime(sa)}
                                                        </Text>
                                                        {sa.status === 'running' && sa.iteration_count > 0 && (
                                                            <Text style={{
                                                                fontSize: 12,
                                                                color: theme.colors.textSecondary,
                                                                ...Typography.default(),
                                                            }}>
                                                                • {sa.iteration_count} 轮
                                                            </Text>
                                                        )}
                                                    </View>

                                                    {/* Result summary for completed */}
                                                    {sa.result && sa.status === 'completed' && (
                                                        <Text style={{
                                                            fontSize: 12,
                                                            color: '#34C759',
                                                            marginTop: 4,
                                                            ...Typography.default(),
                                                        }} numberOfLines={2}>
                                                            {sa.result.substring(0, 100)}{sa.result.length > 100 ? '...' : ''}
                                                        </Text>
                                                    )}

                                                    {/* Error for failed */}
                                                    {sa.error && (sa.status === 'failed' || sa.status === 'timed_out') && (
                                                        <Text style={{
                                                            fontSize: 12,
                                                            color: '#FF3B30',
                                                            marginTop: 4,
                                                            ...Typography.default(),
                                                        }} numberOfLines={2}>
                                                            {sa.error}
                                                        </Text>
                                                    )}

                                                    {/* Action buttons */}
                                                    <View style={{
                                                        flexDirection: 'row',
                                                        marginTop: 8,
                                                        gap: 8,
                                                    }}>
                                                        <Pressable
                                                            onPress={() => onViewSubAgentDetail?.(sa)}
                                                            style={({ pressed }) => ({
                                                                paddingHorizontal: 12,
                                                                paddingVertical: 6,
                                                                borderRadius: 12,
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
                                                                查看详情
                                                            </Text>
                                                        </Pressable>

                                                        {isActive && onStopSubAgent && (
                                                            <Pressable
                                                                onPress={() => handleStopSubAgent(sa.id)}
                                                                disabled={isStopping}
                                                                style={({ pressed }) => ({
                                                                    paddingHorizontal: 12,
                                                                    paddingVertical: 6,
                                                                    borderRadius: 12,
                                                                    backgroundColor: pressed
                                                                        ? theme.colors.surfacePressed
                                                                        : theme.colors.surface,
                                                                    borderWidth: 1,
                                                                    borderColor: theme.colors.divider,
                                                                    opacity: isStopping ? 0.5 : 1,
                                                                })}
                                                            >
                                                                <Text style={{
                                                                    fontSize: 12,
                                                                    color: theme.colors.textDestructive,
                                                                    ...Typography.default('semiBold'),
                                                                }}>
                                                                    {isStopping ? '停止中...' : '停止'}
                                                                </Text>
                                                            </Pressable>
                                                        )}
                                                    </View>
                                                </View>
                                            </View>
                                        </View>
                                    );
                                })}
                            </>
                        )}

                        {/* Background Runs section */}
                        {liveRuns.length > 0 && (
                            <>
                                {showSectionHeaders && (
                                    <View style={{
                                        paddingHorizontal: 20,
                                        paddingVertical: 8,
                                        backgroundColor: theme.colors.surfaceHighest,
                                        borderTopWidth: 0.5,
                                        borderTopColor: theme.colors.divider,
                                    }}>
                                        <Text style={{
                                            fontSize: 12,
                                            color: theme.colors.textSecondary,
                                            ...Typography.default('semiBold'),
                                            textTransform: 'uppercase',
                                            letterSpacing: 0.5,
                                        }}>
                                            后台运行
                                        </Text>
                                    </View>
                                )}
                                {liveRuns.map((run, index) => {
                                    const isStopping = stoppingIds.has(run.run_id);
                                    const statusColor = getStatusColor(run.status);
                                    const isRunning = run.status === 'Running';

                                    return (
                                        <View
                                            key={run.run_id}
                                            style={{
                                                paddingHorizontal: 20,
                                                paddingVertical: 12,
                                                borderTopWidth: 0.5,
                                                borderTopColor: theme.colors.divider,
                                            }}
                                        >
                                            <View style={{ flexDirection: 'row', alignItems: 'flex-start', gap: 12 }}>
                                                <Ionicons
                                                    name={getStatusIcon(run.status) as any}
                                                    size={24}
                                                    color={statusColor}
                                                    style={{ marginTop: 2 }}
                                                />
                                                <View style={{ flex: 1 }}>
                                                    <Text style={{
                                                        fontSize: 15,
                                                        color: theme.colors.text,
                                                        ...Typography.default('semiBold'),
                                                    }}>
                                                        {formatToolName(run.tool_id)}
                                                    </Text>

                                                    {run.command && (
                                                        <Text
                                                            style={{
                                                                fontSize: 12,
                                                                color: theme.colors.textSecondary,
                                                                marginTop: 2,
                                                                ...Typography.default(),
                                                            }}
                                                            numberOfLines={1}
                                                        >
                                                            {run.command}
                                                        </Text>
                                                    )}

                                                    <View style={{
                                                        flexDirection: 'row',
                                                        alignItems: 'center',
                                                        marginTop: 4,
                                                        gap: 8,
                                                    }}>
                                                        <Text style={{
                                                            fontSize: 12,
                                                            color: statusColor,
                                                            ...Typography.default('semiBold'),
                                                        }}>
                                                            {getStatusText(run.status)}
                                                        </Text>
                                                        <Text style={{
                                                            fontSize: 12,
                                                            color: theme.colors.textSecondary,
                                                            ...Typography.default(),
                                                        }}>
                                                            • {formatRunTime(run)}
                                                        </Text>
                                                    </View>

                                                    {run.error && (
                                                        <Text style={{
                                                            fontSize: 12,
                                                            color: '#FF3B30',
                                                            marginTop: 4,
                                                            ...Typography.default(),
                                                        }}>
                                                            错误: {run.error}
                                                        </Text>
                                                    )}

                                                    <View style={{
                                                        flexDirection: 'row',
                                                        marginTop: 8,
                                                        gap: 8,
                                                    }}>
                                                        {run.log_path && (
                                                            <Pressable
                                                                onPress={() => handleViewLogs(run.run_id)}
                                                                disabled={loadingLogs}
                                                                style={({ pressed }) => ({
                                                                    paddingHorizontal: 12,
                                                                    paddingVertical: 6,
                                                                    borderRadius: 12,
                                                                    backgroundColor: pressed
                                                                        ? theme.colors.surfacePressed
                                                                        : theme.colors.surface,
                                                                    borderWidth: 1,
                                                                    borderColor: theme.colors.divider,
                                                                    opacity: loadingLogs ? 0.5 : 1,
                                                                })}
                                                            >
                                                                <Text style={{
                                                                    fontSize: 12,
                                                                    color: theme.colors.textLink,
                                                                    ...Typography.default('semiBold'),
                                                                }}>
                                                                    查看日志
                                                                </Text>
                                                            </Pressable>
                                                        )}

                                                        {isRunning && (
                                                            <Pressable
                                                                onPress={() => handleStop(run.run_id)}
                                                                disabled={isStopping}
                                                                style={({ pressed }) => ({
                                                                    paddingHorizontal: 12,
                                                                    paddingVertical: 6,
                                                                    borderRadius: 12,
                                                                    backgroundColor: pressed
                                                                        ? theme.colors.surfacePressed
                                                                        : theme.colors.surface,
                                                                    borderWidth: 1,
                                                                    borderColor: theme.colors.divider,
                                                                    opacity: isStopping ? 0.5 : 1,
                                                                })}
                                                            >
                                                                <Text style={{
                                                                    fontSize: 12,
                                                                    color: theme.colors.textDestructive,
                                                                    ...Typography.default('semiBold'),
                                                                }}>
                                                                    {isStopping ? '停止中...' : '停止'}
                                                                </Text>
                                                            </Pressable>
                                                        )}
                                                    </View>
                                                </View>
                                            </View>
                                        </View>
                                    );
                                })}
                            </>
                        )}

                        {/* Scheduled Tasks section */}
                        {scheduledTasks.length > 0 && (
                            <>
                                {showSectionHeaders && (
                                    <View style={{
                                        paddingHorizontal: 20,
                                        paddingVertical: 8,
                                        backgroundColor: theme.colors.surfaceHighest,
                                        borderTopWidth: 0.5,
                                        borderTopColor: theme.colors.divider,
                                    }}>
                                        <Text style={{
                                            fontSize: 12,
                                            color: theme.colors.textSecondary,
                                            ...Typography.default('semiBold'),
                                            textTransform: 'uppercase',
                                            letterSpacing: 0.5,
                                        }}>
                                            定时任务
                                        </Text>
                                    </View>
                                )}
                                {scheduledTasks.map((task) => {
                                    const statusColor = getScheduledTaskStatusColor(task);
                                    const isToggling = togglingTaskIds.has(task.id);
                                    const nextRun = formatNextRun(task);

                                    return (
                                        <View
                                            key={task.id}
                                            style={{
                                                paddingHorizontal: 20,
                                                paddingVertical: 12,
                                                borderTopWidth: 0.5,
                                                borderTopColor: theme.colors.divider,
                                            }}
                                        >
                                            <View style={{ flexDirection: 'row', alignItems: 'flex-start', gap: 12 }}>
                                                <Ionicons
                                                    name={getScheduledTaskStatusIcon(task) as any}
                                                    size={24}
                                                    color={statusColor}
                                                    style={{ marginTop: 2 }}
                                                />
                                                <View style={{ flex: 1 }}>
                                                    <View style={{ flexDirection: 'row', alignItems: 'center', gap: 6 }}>
                                                        <Text style={{
                                                            fontSize: 15,
                                                            color: theme.colors.text,
                                                            ...Typography.default('semiBold'),
                                                            flex: 1,
                                                        }} numberOfLines={1}>
                                                            {task.name}
                                                        </Text>
                                                        {task.task_type && task.task_type !== 'dispatch' && (
                                                            <View style={{
                                                                backgroundColor: task.task_type === 'script' ? '#5856D6' : '#FF9500',
                                                                paddingHorizontal: 6,
                                                                paddingVertical: 1,
                                                                borderRadius: 4,
                                                            }}>
                                                                <Text style={{
                                                                    fontSize: 10,
                                                                    color: '#fff',
                                                                    ...Typography.default('semiBold'),
                                                                }}>
                                                                    {task.task_type === 'script' ? 'Script' : 'Task'}
                                                                </Text>
                                                            </View>
                                                        )}
                                                    </View>

                                                    <Text style={{
                                                        fontSize: 12,
                                                        color: theme.colors.textSecondary,
                                                        marginTop: 2,
                                                        ...Typography.default(),
                                                    }}>
                                                        {formatScheduleType(task)}
                                                    </Text>

                                                    <View style={{
                                                        flexDirection: 'row',
                                                        alignItems: 'center',
                                                        marginTop: 4,
                                                        gap: 8,
                                                    }}>
                                                        <Text style={{
                                                            fontSize: 12,
                                                            color: statusColor,
                                                            ...Typography.default('semiBold'),
                                                        }}>
                                                            {getScheduledTaskStatusText(task)}
                                                        </Text>
                                                        {nextRun && (
                                                            <Text style={{
                                                                fontSize: 12,
                                                                color: theme.colors.textSecondary,
                                                                ...Typography.default(),
                                                            }}>
                                                                • {nextRun}
                                                            </Text>
                                                        )}
                                                        {task.state.total_runs > 0 && (
                                                            <Text style={{
                                                                fontSize: 12,
                                                                color: theme.colors.textSecondary,
                                                                ...Typography.default(),
                                                            }}>
                                                                • {task.state.total_runs} 次
                                                            </Text>
                                                        )}
                                                    </View>

                                                    {task.state.last_result_summary && (
                                                        <Text style={{
                                                            fontSize: 12,
                                                            color: task.state.last_status === 'failed' ? '#FF3B30' : '#34C759',
                                                            marginTop: 4,
                                                            ...Typography.default(),
                                                        }} numberOfLines={2}>
                                                            {task.state.last_result_summary.substring(0, 100)}
                                                            {task.state.last_result_summary.length > 100 ? '...' : ''}
                                                        </Text>
                                                    )}

                                                    <View style={{
                                                        flexDirection: 'row',
                                                        marginTop: 8,
                                                        gap: 8,
                                                    }}>
                                                        <Pressable
                                                            onPress={() => onViewScheduledTaskDetail?.(task)}
                                                            style={({ pressed }) => ({
                                                                paddingHorizontal: 12,
                                                                paddingVertical: 6,
                                                                borderRadius: 12,
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
                                                                查看详情
                                                            </Text>
                                                        </Pressable>

                                                        {onToggleScheduledTask && (
                                                            <Pressable
                                                                onPress={() => handleToggleScheduledTask(task.id, !task.enabled)}
                                                                disabled={isToggling}
                                                                style={({ pressed }) => ({
                                                                    paddingHorizontal: 12,
                                                                    paddingVertical: 6,
                                                                    borderRadius: 12,
                                                                    backgroundColor: pressed
                                                                        ? theme.colors.surfacePressed
                                                                        : theme.colors.surface,
                                                                    borderWidth: 1,
                                                                    borderColor: theme.colors.divider,
                                                                    opacity: isToggling ? 0.5 : 1,
                                                                })}
                                                            >
                                                                <Text style={{
                                                                    fontSize: 12,
                                                                    color: task.enabled ? theme.colors.textDestructive : '#34C759',
                                                                    ...Typography.default('semiBold'),
                                                                }}>
                                                                    {isToggling ? '...' : task.enabled ? '禁用' : '启用'}
                                                                </Text>
                                                            </Pressable>
                                                        )}
                                                    </View>
                                                </View>
                                            </View>
                                        </View>
                                    );
                                })}
                            </>
                        )}

                        {/* Task Sessions section (dispatch tasks) */}
                        {taskSessions.length > 0 && (
                            <>
                                {showSectionHeaders && (
                                    <View style={{
                                        paddingHorizontal: 20,
                                        paddingVertical: 8,
                                        backgroundColor: theme.colors.surfaceHighest,
                                        borderTopWidth: 0.5,
                                        borderTopColor: theme.colors.divider,
                                    }}>
                                        <Text style={{
                                            fontSize: 12,
                                            color: theme.colors.textSecondary,
                                            ...Typography.default('semiBold'),
                                            textTransform: 'uppercase',
                                            letterSpacing: 0.5,
                                        }}>
                                            Dispatch 任务
                                        </Text>
                                    </View>
                                )}
                                {taskSessions.map((ts) => {
                                    const isThinking = ts.session?.thinking ?? false;
                                    const isOnline = ts.session?.presence === 'online';
                                    const statusColor = isThinking ? '#007AFF' : isOnline ? '#34C759' : '#8E8E93';
                                    const statusText = isThinking ? '思考中' : isOnline ? '在线' : '离线';
                                    const statusIcon = isThinking ? 'sync-circle' : isOnline ? 'checkmark-circle' : 'ellipse-outline';
                                    const summary = ts.session?.metadata?.summary?.text
                                        || ts.taskDescription
                                        || ts.sessionId.slice(0, 8);

                                    return (
                                        <Pressable
                                            key={ts.sessionId}
                                            onPress={() => onViewTaskSession?.(ts.sessionId)}
                                            style={({ pressed }) => ({
                                                paddingHorizontal: 20,
                                                paddingVertical: 12,
                                                borderTopWidth: 0.5,
                                                borderTopColor: theme.colors.divider,
                                                backgroundColor: pressed ? theme.colors.surfaceRipple : 'transparent',
                                            })}
                                        >
                                            <View style={{ flexDirection: 'row', alignItems: 'flex-start', gap: 12 }}>
                                                <Ionicons
                                                    name={statusIcon as any}
                                                    size={24}
                                                    color={statusColor}
                                                    style={{ marginTop: 2 }}
                                                />
                                                <View style={{ flex: 1 }}>
                                                    <Text style={{
                                                        fontSize: 15,
                                                        color: theme.colors.text,
                                                        ...Typography.default('semiBold'),
                                                    }} numberOfLines={2}>
                                                        {summary}
                                                    </Text>

                                                    <View style={{
                                                        flexDirection: 'row',
                                                        alignItems: 'center',
                                                        marginTop: 4,
                                                        gap: 8,
                                                    }}>
                                                        <Text style={{
                                                            fontSize: 12,
                                                            color: statusColor,
                                                            ...Typography.default('semiBold'),
                                                        }}>
                                                            {statusText}
                                                        </Text>
                                                    </View>
                                                </View>
                                                <Ionicons name="chevron-forward" size={16} color={theme.colors.textSecondary} style={{ marginTop: 4 }} />
                                            </View>
                                        </Pressable>
                                    );
                                })}
                            </>
                        )}

                        {/* Canonical background tasks */}
                        {groupedDisplayTasks.length > 0 && (
                            <>
                                {groupedDisplayTasks.map((bucket) => (
                                    <React.Fragment key={bucket.type}>
                                        <View style={{
                                            paddingHorizontal: 20,
                                            paddingVertical: 8,
                                            backgroundColor: theme.colors.surfaceHighest,
                                            borderTopWidth: 0.5,
                                            borderTopColor: theme.colors.divider,
                                        }}>
                                            <Text style={{
                                                fontSize: 12,
                                                color: theme.colors.textSecondary,
                                                ...Typography.default('semiBold'),
                                                textTransform: 'uppercase',
                                                letterSpacing: 0.5,
                                            }}>
                                                {bucket.label}
                                            </Text>
                                        </View>
                                        {bucket.tasks.map((task) => {
                                            const statusColor = getBackgroundTaskStatusColor(task);
                                            return (
                                                <Pressable
                                                    key={task.taskId}
                                                    onPress={() => setSelectedBackgroundTaskId(task.taskId)}
                                                    style={({ pressed }) => ({
                                                        paddingHorizontal: 20,
                                                        paddingVertical: 12,
                                                        borderTopWidth: 0.5,
                                                        borderTopColor: theme.colors.divider,
                                                        backgroundColor: pressed ? theme.colors.surfaceRipple : 'transparent',
                                                    })}
                                                >
                                                    <View style={{ flexDirection: 'row', alignItems: 'flex-start', gap: 12 }}>
                                                        <Ionicons
                                                            name={getBackgroundTaskStatusIcon(task) as any}
                                                            size={24}
                                                            color={statusColor}
                                                            style={{ marginTop: 2 }}
                                                        />
                                                        <View style={{ flex: 1 }}>
                                                            <Text style={{
                                                                fontSize: 15,
                                                                color: theme.colors.text,
                                                                ...Typography.default('semiBold'),
                                                            }} numberOfLines={2}>
                                                                {task.description ?? task.summary ?? `${bucket.label} #${task.taskId}`}
                                                            </Text>

                                                            <View style={{
                                                                flexDirection: 'row',
                                                                alignItems: 'center',
                                                                marginTop: 4,
                                                                gap: 8,
                                                            }}>
                                                                <Text style={{
                                                                    fontSize: 12,
                                                                    color: statusColor,
                                                                    ...Typography.default('semiBold'),
                                                                }}>
                                                                    {getBackgroundTaskStatusText(task)}
                                                                </Text>
                                                                <Text style={{
                                                                    fontSize: 12,
                                                                    color: theme.colors.textSecondary,
                                                                    ...Typography.default(),
                                                                }}>
                                                                    • {formatBackgroundTaskTime(task)}
                                                                </Text>
                                                                <Text style={{
                                                                    fontSize: 12,
                                                                    color: theme.colors.textSecondary,
                                                                    ...Typography.default(),
                                                                }}>
                                                                    • {getBackgroundTaskVendorLabel(task)}
                                                                </Text>
                                                            </View>

                                                            <Text style={{
                                                                fontSize: 12,
                                                                color: theme.colors.textSecondary,
                                                                marginTop: 4,
                                                                ...Typography.default(),
                                                            }} numberOfLines={2}>
                                                                {task.summary ?? task.outputFile ?? '查看任务详情'}
                                                            </Text>
                                                        </View>
                                                        <Ionicons name="chevron-forward" size={16} color={theme.colors.textSecondary} style={{ marginTop: 4 }} />
                                                    </View>
                                                </Pressable>
                                            );
                                        })}
                                    </React.Fragment>
                                ))}
                            </>
                        )}
                    </>
                )}
            </ScrollView>

            <BackgroundTaskDetailSheet
                key={selectedBackgroundTask?.taskId ?? 'background-task-detail'}
                visible={Boolean(selectedBackgroundTask)}
                task={selectedBackgroundTask}
                toolMessage={selectedBackgroundToolMessage}
                onClose={() => setSelectedBackgroundTaskId(null)}
            />

            {/* Close button */}
            <Pressable
                onPress={onClose}
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
                    关闭
                </Text>
            </Pressable>
        </View>
    );
}
