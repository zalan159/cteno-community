import React, { useMemo, useCallback, useRef, useState } from 'react';
import { Ionicons } from '@expo/vector-icons';
import { View, ActivityIndicator, Platform, Pressable, ScrollView, StyleSheet } from 'react-native';
import { useLocalSearchParams, useRouter } from 'expo-router';
import { useUnistyles } from 'react-native-unistyles';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { LinearGradient } from 'expo-linear-gradient';
import { Typography } from '@/constants/Typography';
import { Text } from '@/components/StyledText';
import { AgentInput } from '@/components/AgentInput';
import { getSuggestions } from '@/components/autocomplete/suggestions';
import { usePersonas } from '@/hooks/usePersonas';
import { useAgentWorkspaces } from '@/hooks/useAgentWorkspaces';
import { useAllMachines, useAllSessions, useCachedPersonas, useIsDataReady, useLocalSetting, useRealtimeStatus, useSession, useSessionMessages, useSessionTaskLifecycle, useSessionUsage, useSetting, storage } from '@/sync/storage';
import { isMachineOnline } from '@/utils/machineUtils';
import { formatPathRelativeToHome, getSessionAvatarId, getSessionName, useSessionStatus } from '@/utils/sessionUtils';
import { sync, onHypothesisPush } from '@/sync/sync';
import { gitStatusSync } from '@/sync/gitStatusSync';
import { sessionAbort, sessionApplyPermissionModeChange, sessionApplyRuntimeModelChange, sessionSetSandboxPolicy, sessionGetMCPServers, sessionSetMCPServers, machineListRuns, machineListScheduledTasks, machineGetPersonaTasks, machineListModels, machineUpdatePersona, machineReconnectSession, machineWorkspaceSendMessage, machineDeleteAgentWorkspace } from '@/sync/ops';
import type { MCPServerItem, ModelOptionDisplay, RunRecord, ScheduledTask, VendorName } from '@/sync/ops';
import { loadCachedVendorDefaultModelId } from '@/sync/modelCatalogCache';
import { startSpeechToText, stopSpeechToText } from '@/realtime/RealtimeSession';
import { Modal } from '@/modal';
import { AgentListModal } from '@/components/AgentListModal';
import { SkillListModal } from '@/components/SkillListModal';
import { machineListAgents, machineListSkills } from '@/sync/ops';
import type { SkillListItem } from '@/sync/ops';
import { inferSessionVendor, useSessionRuntimeControls } from '@/hooks/useCapability';
import { useDraft } from '@/hooks/useDraft';
import { useSubAgents, getDisplayableSubAgents } from '@/hooks/useSubAgents';
import { useOrchestrationFlow } from '@/hooks/useOrchestrationFlow';
import type {
    AgentConfig,
    Session,
    WorkspaceActivity,
    WorkspaceEvent,
    WorkspaceRuntimeMember,
    WorkspaceRuntimeSummary,
    WorkspaceSummary,
} from '@/sync/storageTypes';
import type { Message } from '@/sync/typesMessage';
import { tracking, trackMessageSent } from '@/track';
import { isRunningOnMac } from '@/utils/platform';
import { useDeviceType, useIsLandscape, useIsTablet } from '@/utils/responsive';
import { isVersionSupported, MINIMUM_CLI_VERSION } from '@/utils/versionUtils';
import { LlmProfileList } from '@/components/LlmProfileList';
import { t } from '@/text';
import { frontendLog } from '@/utils/tauri';
import { getVendorAvatarId } from '@/utils/vendorIcons';

import { AgentContentView } from '@/components/AgentContentView';
import { ChatHeaderView } from '@/components/ChatHeaderView';
import { ChatList } from '@/components/ChatList';
import { Deferred } from '@/components/Deferred';
import { EmptyMessages } from '@/components/EmptyMessages';
import { PersonaEmptyState } from '@/components/PersonaEmptyState';
import { PersonaChatInput, PendingImage, PickedImage } from '@/components/PersonaChatInput';
import { EffortSelector, type RuntimeEffort } from '@/components/EffortSelector';
import { BackgroundRunsModal } from '@/components/BackgroundRunsModal';
import { MemoryEditorModal } from '@/components/MemoryEditorModal';
import { WorkspaceBrowserModal } from '@/components/WorkspaceBrowserModal';
import { ScheduledTaskDetailModal } from '@/components/ScheduledTaskDetailModal';
import { SubAgentDetailModal } from '@/components/SubAgentDetailModal';
import { VoiceAssistantStatusBar } from '@/components/VoiceAssistantStatusBar';
import { OrchestrationFlowView } from '@/components/OrchestrationFlowView';
import type { TaskSessionItem } from '@/components/BackgroundRunsModal';
import { useScheduledTasks } from '@/hooks/useScheduledTasks';
import { useHeaderHeight } from '@/utils/responsive';
import { layout } from '@/components/layout';
import { hapticsLight } from '@/components/haptics';
import { FloatingOverlay } from '@/components/FloatingOverlay';
import { TouchableWithoutFeedback } from 'react-native';
import { MCPSelectorModal } from '@/components/MCPSelectorModal';
import { useBackgroundTasks } from '@/hooks/useBackgroundTasks';
import { countActiveAgentBackgroundTasks, deriveAgentBackgroundTasks } from '@/utils/agentBackgroundTasks';
import type { AgentBackgroundTaskItem } from '@/utils/agentBackgroundTasks';
import {
    type PermissionMode,
    permissionModeIcon,
    permissionModeLabel,
    permissionModesForVendor,
} from '@/utils/permissionModes';

const CODER_MODEL_RESTART_HINT = 'Restart required';
const CODER_MODEL_RESTART_DESCRIPTION = 'Changes apply after restart.';

function runtimeEffortLabel(effort: RuntimeEffort): string {
    switch (effort) {
        case 'low':
            return '低推理';
        case 'medium':
            return '中推理';
        case 'high':
            return '高推理';
        case 'xhigh':
            return '超高推理';
        case 'max':
            return '最大推理';
        default:
            return '默认推理';
    }
}

/** Toolbar rendered below chat input — permission mode, background runs, notifications */
const sandboxLabels: Record<string, string> = {
    workspace_write: 'Workspace',
    unrestricted: 'Full Access',
};

function workspaceTemplateDisplayName(templateId: string | null | undefined): string {
    switch (templateId) {
        case 'group-chat':
        case 'coding-studio':
            return '群聊';
        case 'gated-tasks':
        case 'task-gate-coding':
        case 'task-gate-coding-manual':
            return '门控任务';
        case 'autoresearch':
            return '自主研究';
        default:
            return templateId || 'Workspace';
    }
}

function isGatedTasksTemplate(templateId: string | null | undefined): boolean {
    return (
        templateId === 'gated-tasks' ||
        templateId === 'task-gate-coding' ||
        templateId === 'task-gate-coding-manual'
    );
}

function formatWorkspaceTimestamp(iso: string | null | undefined): string {
    if (!iso) return '';
    const date = new Date(iso);
    if (Number.isNaN(date.getTime())) return '';
    return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
}

type GatedTaskBackendStatus =
    | 'pending'
    | 'coding'
    | 'reviewing'
    | 'committing'
    | 'completed'
    | 'skipped';

type GatedTaskLifecycleStatus =
    | 'pending'
    | 'running'
    | 'approved'
    | 'rejected'
    | 'committed'
    | 'skipped';

type GatedTasksPhaseValue = 'idle' | 'coding' | 'reviewing' | 'committing';

interface GatedTaskTemplateItem {
    title: string;
    instruction: string;
    status: GatedTaskBackendStatus | string;
    feedback?: string | null;
    coderResult?: string | null;
    reviewResult?: string | null;
    commitResult?: string | null;
}

interface GatedTasksTemplateState {
    type: 'gated_tasks';
    currentPhase?: GatedTasksPhaseValue | string | null;
    currentTaskIndex?: number | null;
    reviewerRoleId?: string | null;
    coderRoleId?: string | null;
    tasks?: GatedTaskTemplateItem[] | null;
}

type AutoresearchHypothesisStatus = 'proposed' | 'testing' | 'kept' | 'discarded' | 'split';
type AutoresearchExperimentStatus = 'running' | 'awaiting_gate' | 'keep' | 'discard';

interface AutoresearchHypothesisNode {
    id: string;
    parentId?: string | null;
    text: string;
    confidence: number;
    status: AutoresearchHypothesisStatus | string;
    children?: string[] | null;
}

interface AutoresearchExperimentRecord {
    id: string;
    hypothesisId: string;
    hypothesisText: string;
    description: string;
    metric?: number | null;
    status: AutoresearchExperimentStatus | string;
    workerResult?: string | null;
    gateReason?: string | null;
}

interface AutoresearchTemplateState {
    type: 'autoresearch';
    hypotheses?: AutoresearchHypothesisNode[] | null;
    experiments?: AutoresearchExperimentRecord[] | null;
    bestMetric?: number | null;
    activeHypothesisId?: string | null;
    activeExperimentId?: string | null;
    pendingGateExperimentId?: string | null;
}

type WorkspaceRuntimeWithTemplateState = WorkspaceRuntimeSummary & {
    templateState?: GatedTasksTemplateState | AutoresearchTemplateState | Record<string, unknown> | null;
};

function asWorkspaceRuntimeWithTemplateState(
    runtime: WorkspaceRuntimeSummary | null | undefined,
): WorkspaceRuntimeWithTemplateState | null {
    if (!runtime) return null;
    return runtime as WorkspaceRuntimeWithTemplateState;
}

function getGatedTasksTemplateState(
    workspace: WorkspaceSummary | null | undefined,
): GatedTasksTemplateState | null {
    const templateState = asWorkspaceRuntimeWithTemplateState(workspace?.runtime)?.templateState;
    if (!templateState || typeof templateState !== 'object') return null;
    if ((templateState as { type?: unknown }).type !== 'gated_tasks') return null;
    return templateState as GatedTasksTemplateState;
}

function getAutoresearchTemplateState(
    workspace: WorkspaceSummary | null | undefined,
): AutoresearchTemplateState | null {
    const templateState = asWorkspaceRuntimeWithTemplateState(workspace?.runtime)?.templateState;
    if (!templateState || typeof templateState !== 'object') return null;
    if ((templateState as { type?: unknown }).type !== 'autoresearch') return null;
    return templateState as AutoresearchTemplateState;
}

function mergeWorkspaceRuntimeWithTemplateState(
    nextRuntime: WorkspaceRuntimeSummary | null | undefined,
    previousRuntime: WorkspaceRuntimeWithTemplateState | null | undefined,
): WorkspaceRuntimeWithTemplateState | null {
    const next = asWorkspaceRuntimeWithTemplateState(nextRuntime);
    if (!next) return null;
    const nextTemplateState = next.templateState;
    if (nextTemplateState !== undefined) return next;
    return {
        ...next,
        templateState: previousRuntime?.templateState,
    };
}

function normalizeWorkspaceTemplateState(
    templateState: unknown,
): GatedTasksTemplateState | AutoresearchTemplateState | Record<string, unknown> | null | undefined {
    if (templateState == null) return templateState;
    if (typeof templateState !== 'object') return undefined;
    return templateState as GatedTasksTemplateState | AutoresearchTemplateState | Record<string, unknown>;
}

function formatAutoresearchMetric(metric: number | null | undefined): string {
    if (typeof metric !== 'number' || Number.isNaN(metric)) return '—';
    return Number.isInteger(metric) ? metric.toString() : metric.toFixed(2);
}

function formatAutoresearchStatus(status: string | null | undefined): string {
    switch (status) {
        case 'proposed':
            return 'Proposed';
        case 'testing':
            return 'Testing';
        case 'kept':
            return 'Kept';
        case 'discarded':
            return 'Discarded';
        case 'split':
            return 'Split';
        case 'running':
            return 'Running';
        case 'awaiting_gate':
            return 'Awaiting Gate';
        case 'keep':
            return 'Keep';
        case 'discard':
            return 'Discard';
        default:
            return status || 'Unknown';
    }
}

function gatedTasksPhaseLabel(phase: string | null | undefined): string {
    switch (phase) {
        case 'coding':
            return 'Coding';
        case 'reviewing':
            return 'Reviewing';
        case 'committing':
            return 'Committing';
        case 'idle':
        default:
            return 'Idle';
    }
}

function resolveGatedTaskLifecycle(task: GatedTaskTemplateItem): GatedTaskLifecycleStatus {
    if (task.status === 'completed') return 'committed';
    if (task.status === 'skipped') return 'skipped';
    if (task.status === 'committing') return 'approved';
    if (task.status === 'coding' && !!task.feedback) return 'rejected';
    if (task.status === 'reviewing' || task.status === 'coding') return 'running';
    return 'pending';
}

function gatedTaskLifecycleMeta(
    lifecycle: GatedTaskLifecycleStatus,
    theme: ReturnType<typeof useUnistyles>['theme'],
) {
    switch (lifecycle) {
        case 'running':
            return {
                label: 'Running',
                icon: 'sync-outline' as const,
                color: '#0A84FF',
                backgroundColor: `${theme.colors.button.primary.background}14`,
            };
        case 'approved':
            return {
                label: 'Approved',
                icon: 'checkmark-circle-outline' as const,
                color: '#34C759',
                backgroundColor: '#34C75914',
            };
        case 'rejected':
            return {
                label: 'Rejected',
                icon: 'close-circle-outline' as const,
                color: '#FF9F0A',
                backgroundColor: '#FF9F0A14',
            };
        case 'committed':
            return {
                label: 'Committed',
                icon: 'git-commit-outline' as const,
                color: '#34C759',
                backgroundColor: '#34C75914',
            };
        case 'skipped':
            return {
                label: 'Skipped',
                icon: 'play-skip-forward-outline' as const,
                color: theme.colors.textSecondary,
                backgroundColor: theme.colors.surface,
            };
        case 'pending':
        default:
            return {
                label: 'Pending',
                icon: 'ellipse-outline' as const,
                color: theme.colors.textSecondary,
                backgroundColor: theme.colors.surface,
            };
    }
}

function workspaceActivityLabel(kind: WorkspaceActivity['kind']): string {
    switch (kind) {
        case 'user_message':
            return '用户';
        case 'coordinator_message':
            return '协调';
        case 'workflow_vote_opened':
            return '流程投票';
        case 'workflow_vote_approved':
            return '投票通过';
        case 'workflow_vote_rejected':
            return '投票驳回';
        case 'workflow_started':
            return '流程启动';
        case 'workflow_stage_started':
            return '阶段开始';
        case 'workflow_stage_completed':
            return '阶段完成';
        case 'workflow_completed':
            return '流程完成';
        case 'claim_window_opened':
            return '认领开始';
        case 'claim_window_closed':
            return '认领结束';
        case 'member_claimed':
            return '认领';
        case 'member_supporting':
            return '协助';
        case 'member_declined':
            return '放弃';
        case 'member_progress':
            return '进展';
        case 'member_blocked':
            return '阻塞';
        case 'member_delivered':
            return '交付';
        case 'member_summary':
            return '总结';
        case 'dispatch_started':
            return '开始';
        case 'dispatch_progress':
            return '推进';
        case 'dispatch_completed':
            return '完成';
        case 'system_notice':
            return '系统';
        default:
            return '动态';
    }
}

function getWorkspaceRuntimeMember(
    workspace: WorkspaceSummary | null | undefined,
    roleId: string | null | undefined,
): WorkspaceRuntimeMember | null {
    if (!workspace?.runtime?.state?.members || !roleId) return null;
    return (
        Object.values(workspace.runtime.state.members).find((member) => member.roleId === roleId) ||
        null
    );
}

function getLatestWorkspaceActivityForRole(
    workspace: WorkspaceSummary | null | undefined,
    roleId: string | null | undefined,
): WorkspaceActivity | null {
    if (!workspace?.runtime?.recentActivities?.length || !roleId) return null;
    for (let index = workspace.runtime.recentActivities.length - 1; index >= 0; index -= 1) {
        const activity = workspace.runtime.recentActivities[index];
        if (activity.roleId === roleId) return activity;
    }
    return null;
}

const WorkspaceBanner = React.memo(({
    workspace,
    onDelete,
    deleting = false,
}: {
    workspace: WorkspaceSummary;
    onDelete?: () => void;
    deleting?: boolean;
}) => {
    const { theme } = useUnistyles();
    const activeMembers = workspace.runtime
        ? Object.values(workspace.runtime.state.members).filter((member) => member.status === 'active').length
        : 0;
    const latestActivity = workspace.runtime?.recentActivities?.length
        ? workspace.runtime.recentActivities[workspace.runtime.recentActivities.length - 1]
        : null;
    const coordinatorRoleId =
        workspace.binding.defaultRoleId ||
        workspace.runtime?.state?.activities.find((activity) => activity.kind === 'coordinator_message')?.roleId ||
        workspace.members[0]?.roleId ||
        'pm';

    return (
        <View
            style={{
                marginHorizontal: 16,
                marginTop: 12,
                marginBottom: 8,
                padding: 14,
                borderRadius: 14,
                backgroundColor: theme.colors.surfaceHigh,
                borderWidth: 1,
                borderColor: theme.colors.divider,
                gap: 10,
            }}
        >
            <View style={{ flexDirection: 'row', alignItems: 'center' }}>
                <View
                    style={{
                        width: 34,
                        height: 34,
                        borderRadius: 17,
                        alignItems: 'center',
                        justifyContent: 'center',
                        backgroundColor: theme.colors.button.primary.background,
                        marginRight: 10,
                    }}
                >
                    <Ionicons name="people-outline" size={17} color={theme.colors.button.primary.tint} />
                </View>
                <View style={{ flex: 1 }}>
                    <Text style={{ fontSize: 14, color: theme.colors.text, ...Typography.default('semiBold') }}>
                        群聊工作间
                    </Text>
                    <Text style={{ fontSize: 12, color: theme.colors.textSecondary, marginTop: 2, ...Typography.default() }}>
                        {workspaceTemplateDisplayName(workspace.binding.templateId)} · {workspace.members.length} 个角色
                    </Text>
                    <View style={{ flexDirection: 'row', alignItems: 'center', gap: 8, marginTop: 6, flexWrap: 'wrap' }}>
                        <Text style={{ fontSize: 11, color: theme.colors.textSecondary, ...Typography.default() }}>
                            协调者 @{coordinatorRoleId}
                        </Text>
                        <Text style={{ fontSize: 11, color: theme.colors.textSecondary, ...Typography.default() }}>
                            活跃成员 {activeMembers}
                        </Text>
                    </View>
                    <Text style={{ fontSize: 12, color: theme.colors.textSecondary, marginTop: 4, ...Typography.default() }}>
                        不写 `@` 会广播给整个工作间并进入认领流程；用 `@角色` 可直接派发
                    </Text>
                    {latestActivity?.text ? (
                        <Text style={{ fontSize: 12, color: theme.colors.textSecondary, marginTop: 6, ...Typography.default() }}>
                            最新{workspaceActivityLabel(latestActivity.kind)}: {latestActivity.text}
                        </Text>
                    ) : null}
                </View>
                {onDelete && (
                    <Pressable
                        onPress={onDelete}
                        disabled={deleting}
                        style={({ pressed }) => ({
                            width: 34,
                            height: 34,
                            borderRadius: 17,
                            marginLeft: 10,
                            alignItems: 'center',
                            justifyContent: 'center',
                            backgroundColor: pressed ? theme.colors.surfacePressed : theme.colors.surface,
                            borderWidth: 1,
                            borderColor: theme.colors.divider,
                            opacity: deleting ? 0.6 : 1,
                        })}
                    >
                        {deleting ? (
                            <ActivityIndicator size={14} color={theme.colors.deleteAction} />
                        ) : (
                            <Ionicons name="trash-outline" size={16} color={theme.colors.deleteAction} />
                        )}
                    </Pressable>
                )}
            </View>
        </View>
    );
});

const GatedTasksPanel = React.memo(({
    templateState,
    onPause,
    onSkip,
    onApprove,
    onReject,
    controlsDisabled = false,
}: {
    templateState: GatedTasksTemplateState;
    onPause?: () => void;
    onSkip?: () => void;
    onApprove?: () => void;
    onReject?: () => void;
    controlsDisabled?: boolean;
}) => {
    const { theme } = useUnistyles();
    const tasks = templateState.tasks || [];
    const currentTaskIndex = templateState.currentTaskIndex ?? null;
    const isReviewing = templateState.currentPhase === 'reviewing';

    return (
        <View style={{ paddingHorizontal: 16, paddingBottom: 8 }}>
            <View style={{
                borderRadius: 16,
                backgroundColor: theme.colors.surface,
                borderWidth: 1,
                borderColor: theme.colors.divider,
                overflow: 'hidden',
            }}>
                <View style={{
                    paddingHorizontal: 14,
                    paddingTop: 12,
                    paddingBottom: 10,
                    gap: 8,
                }}>
                    <View style={{ flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between', gap: 12 }}>
                        <View style={{ flex: 1, gap: 3 }}>
                            <Text style={{ fontSize: 13, color: theme.colors.text, ...Typography.default('semiBold') }}>
                                门控任务
                            </Text>
                            <Text style={{ fontSize: 11, color: theme.colors.textSecondary, ...Typography.default() }}>
                                Reviewer @{templateState.reviewerRoleId || 'reviewer'} · Coder @{templateState.coderRoleId || 'coder'}
                            </Text>
                        </View>
                        <View style={{
                            paddingHorizontal: 10,
                            paddingVertical: 5,
                            borderRadius: 999,
                            backgroundColor: theme.colors.surfaceHigh,
                            borderWidth: 1,
                            borderColor: theme.colors.divider,
                        }}>
                            <Text style={{ fontSize: 11, color: theme.colors.textSecondary, ...Typography.default('semiBold') }}>
                                {gatedTasksPhaseLabel(templateState.currentPhase)}
                            </Text>
                        </View>
                    </View>

                    <View style={{ flexDirection: 'row', gap: 8 }}>
                        <Pressable
                            onPress={onApprove}
                            disabled={!isReviewing || controlsDisabled}
                            style={({ pressed }) => ({
                                flex: 1,
                                minHeight: 36,
                                borderRadius: 10,
                                alignItems: 'center',
                                justifyContent: 'center',
                                flexDirection: 'row',
                                gap: 6,
                                backgroundColor: isReviewing ? '#34C75914' : theme.colors.surfaceHigh,
                                borderWidth: 1,
                                borderColor: isReviewing ? '#34C75944' : theme.colors.divider,
                                opacity: !isReviewing || controlsDisabled ? 0.45 : (pressed ? 0.8 : 1),
                            })}
                        >
                            <Ionicons name="checkmark" size={14} color={isReviewing ? '#34C759' : theme.colors.textSecondary} />
                            <Text style={{ fontSize: 12, color: isReviewing ? '#34C759' : theme.colors.textSecondary, ...Typography.default('semiBold') }}>
                                Approve
                            </Text>
                        </Pressable>
                        <Pressable
                            onPress={onReject}
                            disabled={!isReviewing || controlsDisabled}
                            style={({ pressed }) => ({
                                flex: 1,
                                minHeight: 36,
                                borderRadius: 10,
                                alignItems: 'center',
                                justifyContent: 'center',
                                flexDirection: 'row',
                                gap: 6,
                                backgroundColor: isReviewing ? '#FF9F0A14' : theme.colors.surfaceHigh,
                                borderWidth: 1,
                                borderColor: isReviewing ? '#FF9F0A44' : theme.colors.divider,
                                opacity: !isReviewing || controlsDisabled ? 0.45 : (pressed ? 0.8 : 1),
                            })}
                        >
                            <Ionicons name="close" size={14} color={isReviewing ? '#FF9F0A' : theme.colors.textSecondary} />
                            <Text style={{ fontSize: 12, color: isReviewing ? '#FF9F0A' : theme.colors.textSecondary, ...Typography.default('semiBold') }}>
                                Reject
                            </Text>
                        </Pressable>
                    </View>

                    <View style={{ flexDirection: 'row', gap: 8 }}>
                        <Pressable
                            onPress={onPause}
                            disabled={controlsDisabled}
                            style={({ pressed }) => ({
                                flex: 1,
                                minHeight: 34,
                                borderRadius: 10,
                                alignItems: 'center',
                                justifyContent: 'center',
                                flexDirection: 'row',
                                gap: 6,
                                backgroundColor: theme.colors.surfaceHigh,
                                borderWidth: 1,
                                borderColor: theme.colors.divider,
                                opacity: controlsDisabled ? 0.5 : (pressed ? 0.8 : 1),
                            })}
                        >
                            <Ionicons name="pause-outline" size={14} color={theme.colors.textSecondary} />
                            <Text style={{ fontSize: 12, color: theme.colors.textSecondary, ...Typography.default('semiBold') }}>
                                Pause
                            </Text>
                        </Pressable>
                        <Pressable
                            onPress={onSkip}
                            disabled={controlsDisabled}
                            style={({ pressed }) => ({
                                flex: 1,
                                minHeight: 34,
                                borderRadius: 10,
                                alignItems: 'center',
                                justifyContent: 'center',
                                flexDirection: 'row',
                                gap: 6,
                                backgroundColor: theme.colors.surfaceHigh,
                                borderWidth: 1,
                                borderColor: theme.colors.divider,
                                opacity: controlsDisabled ? 0.5 : (pressed ? 0.8 : 1),
                            })}
                        >
                            <Ionicons name="play-skip-forward-outline" size={14} color={theme.colors.textSecondary} />
                            <Text style={{ fontSize: 12, color: theme.colors.textSecondary, ...Typography.default('semiBold') }}>
                                Skip
                            </Text>
                        </Pressable>
                    </View>
                </View>

                <View style={{ paddingHorizontal: 12, paddingBottom: 12, gap: 8 }}>
                    {tasks.map((task, index) => {
                        const lifecycle = resolveGatedTaskLifecycle(task);
                        const meta = gatedTaskLifecycleMeta(lifecycle, theme);
                        const isCurrent = currentTaskIndex === index;
                        return (
                            <View
                                key={`${task.title}-${index}`}
                                style={{
                                    borderRadius: 12,
                                    borderWidth: 1,
                                    borderColor: isCurrent ? theme.colors.button.primary.background : theme.colors.divider,
                                    backgroundColor: isCurrent ? `${theme.colors.button.primary.background}10` : theme.colors.surfaceHigh,
                                    paddingHorizontal: 12,
                                    paddingVertical: 10,
                                    gap: 6,
                                }}
                            >
                                <View style={{ flexDirection: 'row', alignItems: 'center', gap: 10 }}>
                                    <View style={{
                                        width: 28,
                                        height: 28,
                                        borderRadius: 14,
                                        alignItems: 'center',
                                        justifyContent: 'center',
                                        backgroundColor: meta.backgroundColor,
                                    }}>
                                        <Ionicons name={meta.icon} size={15} color={meta.color} />
                                    </View>
                                    <View style={{ flex: 1, gap: 2 }}>
                                        <Text style={{ fontSize: 12, color: theme.colors.text, ...Typography.default('semiBold') }}>
                                            {index + 1}. {task.title}
                                        </Text>
                                        <Text style={{ fontSize: 11, color: theme.colors.textSecondary, ...Typography.default() }}>
                                            {meta.label}{isCurrent ? ' · Current task' : ''}
                                        </Text>
                                    </View>
                                </View>
                                <Text style={{ fontSize: 12, color: theme.colors.textSecondary, ...Typography.default() }}>
                                    {task.instruction}
                                </Text>
                                {!!task.feedback && (
                                    <Text style={{ fontSize: 11, color: '#FF9F0A', ...Typography.default() }}>
                                        Reviewer feedback: {task.feedback}
                                    </Text>
                                )}
                            </View>
                        );
                    })}
                    {tasks.length === 0 && (
                        <Text style={{ fontSize: 12, color: theme.colors.textSecondary, ...Typography.default() }}>
                            No gated tasks queued yet.
                        </Text>
                    )}
                </View>
            </View>
        </View>
    );
});

const AutoresearchPanel = React.memo(({
    templateState,
}: {
    templateState: AutoresearchTemplateState;
}) => {
    const { theme } = useUnistyles();
    const hypotheses = templateState.hypotheses || [];
    const experiments = templateState.experiments || [];
    const [expandedNodes, setExpandedNodes] = React.useState<Record<string, boolean>>({});

    const hypothesisMap = React.useMemo(
        () => new Map(hypotheses.map((hypothesis) => [hypothesis.id, hypothesis])),
        [hypotheses],
    );

    const childrenByParentId = React.useMemo(() => {
        const next = new Map<string, AutoresearchHypothesisNode[]>();
        for (const hypothesis of hypotheses) {
            if (!hypothesis.parentId) continue;
            const siblings = next.get(hypothesis.parentId) || [];
            siblings.push(hypothesis);
            next.set(hypothesis.parentId, siblings);
        }
        return next;
    }, [hypotheses]);

    const rootHypotheses = React.useMemo(
        () => hypotheses.filter((hypothesis) => !hypothesis.parentId || !hypothesisMap.has(hypothesis.parentId)),
        [hypotheses, hypothesisMap],
    );

    React.useEffect(() => {
        setExpandedNodes((previous) => {
            const next = { ...previous };
            let changed = false;

            for (const hypothesis of rootHypotheses) {
                if (next[hypothesis.id] !== true) {
                    next[hypothesis.id] = true;
                    changed = true;
                }
            }

            if (templateState.activeHypothesisId) {
                let current = hypothesisMap.get(templateState.activeHypothesisId) || null;
                while (current) {
                    if (next[current.id] !== true) {
                        next[current.id] = true;
                        changed = true;
                    }
                    current = current.parentId ? hypothesisMap.get(current.parentId) || null : null;
                }
            }

            return changed ? next : previous;
        });
    }, [hypothesisMap, rootHypotheses, templateState.activeHypothesisId]);

    const toggleNode = React.useCallback((id: string) => {
        setExpandedNodes((previous) => ({
            ...previous,
            [id]: !previous[id],
        }));
    }, []);

    const bestExperiment = React.useMemo(() => {
        let current: AutoresearchExperimentRecord | null = null;
        for (const experiment of experiments) {
            if (typeof experiment.metric !== 'number' || Number.isNaN(experiment.metric)) continue;
            if (!current || (current.metric ?? Number.NEGATIVE_INFINITY) < experiment.metric) {
                current = experiment;
            }
        }
        return current;
    }, [experiments]);

    const bestMetric =
        typeof templateState.bestMetric === 'number' && !Number.isNaN(templateState.bestMetric)
            ? templateState.bestMetric
            : bestExperiment?.metric ?? null;

    const runningExperiment =
        (templateState.activeExperimentId
            ? experiments.find((experiment) => experiment.id === templateState.activeExperimentId)
            : null) ||
        experiments.find((experiment) => experiment.status === 'running') ||
        null;

    const pendingGateExperiment =
        (templateState.pendingGateExperimentId
            ? experiments.find((experiment) => experiment.id === templateState.pendingGateExperimentId)
            : null) ||
        null;

    const renderHypothesisNode = (hypothesis: AutoresearchHypothesisNode, depth = 0): React.ReactNode => {
        const childNodes = childrenByParentId.get(hypothesis.id)
            || (hypothesis.children || [])
                .map((childId) => hypothesisMap.get(childId))
                .filter((child): child is AutoresearchHypothesisNode => !!child);
        const isExpanded = expandedNodes[hypothesis.id] ?? depth === 0;
        const confidence = Math.max(0, Math.min(1, hypothesis.confidence || 0));
        const isActive = templateState.activeHypothesisId === hypothesis.id;

        return (
            <View key={hypothesis.id} style={{ marginLeft: depth * 14, gap: 8 }}>
                <Pressable
                    onPress={childNodes.length > 0 ? () => toggleNode(hypothesis.id) : undefined}
                    style={({ pressed }) => ({
                        borderRadius: 12,
                        borderWidth: 1,
                        borderColor: isActive ? theme.colors.button.primary.background : theme.colors.divider,
                        backgroundColor: isActive ? `${theme.colors.button.primary.background}10` : theme.colors.surfaceHigh,
                        paddingHorizontal: 12,
                        paddingVertical: 10,
                        opacity: pressed ? 0.85 : 1,
                    })}
                >
                    <View style={{ flexDirection: 'row', alignItems: 'flex-start', gap: 8 }}>
                        <Ionicons
                            name={childNodes.length > 0 ? (isExpanded ? 'chevron-down' : 'chevron-forward') : 'ellipse'}
                            size={14}
                            color={theme.colors.textSecondary}
                            style={{ marginTop: 2 }}
                        />
                        <View style={{ flex: 1, gap: 6 }}>
                            <View style={{ flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between', gap: 8 }}>
                                <Text style={{ fontSize: 12, color: theme.colors.text, ...Typography.default('semiBold') }}>
                                    {hypothesis.id}
                                </Text>
                                <Text style={{ fontSize: 11, color: theme.colors.textSecondary, ...Typography.default() }}>
                                    {formatAutoresearchStatus(hypothesis.status)}
                                </Text>
                            </View>
                            <Text style={{ fontSize: 12, color: theme.colors.textSecondary, ...Typography.default() }}>
                                {hypothesis.text}
                            </Text>
                            <View style={{ gap: 4 }}>
                                <View style={{ flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between' }}>
                                    <Text style={{ fontSize: 11, color: theme.colors.textSecondary, ...Typography.default() }}>
                                        Confidence
                                    </Text>
                                    <Text style={{ fontSize: 11, color: theme.colors.textSecondary, ...Typography.default('semiBold') }}>
                                        {Math.round(confidence * 100)}%
                                    </Text>
                                </View>
                                <View style={{ height: 6, borderRadius: 999, backgroundColor: theme.colors.surface }}>
                                    <View style={{
                                        width: `${Math.max(confidence * 100, 4)}%`,
                                        height: '100%',
                                        borderRadius: 999,
                                        backgroundColor: isActive ? theme.colors.button.primary.background : '#34C759',
                                    }} />
                                </View>
                            </View>
                        </View>
                    </View>
                </Pressable>
                {isExpanded && childNodes.map((child) => renderHypothesisNode(child, depth + 1))}
            </View>
        );
    };

    return (
        <View style={{ paddingHorizontal: 16, paddingBottom: 8 }}>
            <View style={{
                borderRadius: 16,
                backgroundColor: theme.colors.surface,
                borderWidth: 1,
                borderColor: theme.colors.divider,
                overflow: 'hidden',
            }}>
                <View style={{
                    paddingHorizontal: 14,
                    paddingTop: 12,
                    paddingBottom: 10,
                    gap: 10,
                }}>
                    <View style={{ flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between', gap: 12 }}>
                        <View style={{ flex: 1, gap: 3 }}>
                            <Text style={{ fontSize: 13, color: theme.colors.text, ...Typography.default('semiBold') }}>
                                自主研究
                            </Text>
                            <Text style={{ fontSize: 11, color: theme.colors.textSecondary, ...Typography.default() }}>
                                Hypothesis graph and experiment log
                            </Text>
                        </View>
                        {bestMetric != null && (
                            <View style={{
                                paddingHorizontal: 10,
                                paddingVertical: 6,
                                borderRadius: 12,
                                backgroundColor: '#34C75914',
                                borderWidth: 1,
                                borderColor: '#34C75933',
                                gap: 2,
                            }}>
                                <Text style={{ fontSize: 10, color: '#34C759', ...Typography.default('semiBold') }}>
                                    BEST METRIC
                                </Text>
                                <Text style={{ fontSize: 13, color: theme.colors.text, ...Typography.default('semiBold') }}>
                                    {formatAutoresearchMetric(bestMetric)}
                                </Text>
                            </View>
                        )}
                    </View>

                    {(runningExperiment || pendingGateExperiment) && (
                        <View style={{
                            flexDirection: 'row',
                            alignItems: 'center',
                            gap: 8,
                            borderRadius: 12,
                            borderWidth: 1,
                            borderColor: theme.colors.divider,
                            backgroundColor: theme.colors.surfaceHigh,
                            paddingHorizontal: 12,
                            paddingVertical: 10,
                        }}>
                            <ActivityIndicator size="small" color={theme.colors.button.primary.background} />
                            <View style={{ flex: 1, gap: 2 }}>
                                <Text style={{ fontSize: 12, color: theme.colors.text, ...Typography.default('semiBold') }}>
                                    {runningExperiment
                                        ? `Running ${runningExperiment.id} on ${runningExperiment.hypothesisId}`
                                        : `Gate review pending for ${pendingGateExperiment?.id || 'experiment'}`}
                                </Text>
                                <Text style={{ fontSize: 11, color: theme.colors.textSecondary, ...Typography.default() }}>
                                    {(runningExperiment || pendingGateExperiment)?.description || 'Awaiting the next research update.'}
                                </Text>
                            </View>
                        </View>
                    )}
                </View>

                <View style={{ paddingHorizontal: 12, paddingBottom: 12, gap: 12 }}>
                    <View style={{ gap: 8 }}>
                        <Text style={{ fontSize: 12, color: theme.colors.text, ...Typography.default('semiBold') }}>
                            Hypotheses
                        </Text>
                        {rootHypotheses.length > 0 ? rootHypotheses.map((hypothesis) => renderHypothesisNode(hypothesis)) : (
                            <Text style={{ fontSize: 12, color: theme.colors.textSecondary, ...Typography.default() }}>
                                No hypotheses yet.
                            </Text>
                        )}
                    </View>

                    <View style={{ gap: 8 }}>
                        <Text style={{ fontSize: 12, color: theme.colors.text, ...Typography.default('semiBold') }}>
                            Experiment Log
                        </Text>
                        <ScrollView horizontal showsHorizontalScrollIndicator={false}>
                            <View style={{ minWidth: 720 }}>
                                <View style={{
                                    flexDirection: 'row',
                                    paddingHorizontal: 10,
                                    paddingVertical: 8,
                                    borderTopLeftRadius: 12,
                                    borderTopRightRadius: 12,
                                    backgroundColor: theme.colors.surfaceHigh,
                                    borderWidth: 1,
                                    borderBottomWidth: 0,
                                    borderColor: theme.colors.divider,
                                }}>
                                    {[
                                        ['ID', 70],
                                        ['Hypothesis', 120],
                                        ['Metric', 90],
                                        ['Status', 120],
                                        ['Description', 320],
                                    ].map(([label, width]) => (
                                        <Text
                                            key={label}
                                            style={{
                                                width: Number(width),
                                                fontSize: 11,
                                                color: theme.colors.textSecondary,
                                                ...Typography.default('semiBold'),
                                            }}
                                        >
                                            {label}
                                        </Text>
                                    ))}
                                </View>

                                {experiments.length > 0 ? experiments.map((experiment, index) => (
                                    <View
                                        key={experiment.id}
                                        style={{
                                            flexDirection: 'row',
                                            paddingHorizontal: 10,
                                            paddingVertical: 10,
                                            borderWidth: 1,
                                            borderTopWidth: 0,
                                            borderBottomLeftRadius: index === experiments.length - 1 ? 12 : 0,
                                            borderBottomRightRadius: index === experiments.length - 1 ? 12 : 0,
                                            borderColor: theme.colors.divider,
                                            backgroundColor: experiment.id === bestExperiment?.id ? '#34C75910' : theme.colors.surface,
                                        }}
                                    >
                                        <Text style={{ width: 70, fontSize: 11, color: theme.colors.text, ...Typography.default('semiBold') }}>
                                            {experiment.id}
                                        </Text>
                                        <Text style={{ width: 120, fontSize: 11, color: theme.colors.textSecondary, ...Typography.default() }}>
                                            {experiment.hypothesisId}
                                        </Text>
                                        <Text style={{ width: 90, fontSize: 11, color: theme.colors.textSecondary, ...Typography.default() }}>
                                            {formatAutoresearchMetric(experiment.metric)}
                                        </Text>
                                        <Text style={{ width: 120, fontSize: 11, color: theme.colors.textSecondary, ...Typography.default() }}>
                                            {formatAutoresearchStatus(experiment.status)}
                                        </Text>
                                        <Text style={{ width: 320, fontSize: 11, color: theme.colors.textSecondary, ...Typography.default() }}>
                                            {experiment.description}
                                        </Text>
                                    </View>
                                )) : (
                                    <View style={{
                                        borderWidth: 1,
                                        borderTopWidth: 0,
                                        borderColor: theme.colors.divider,
                                        borderBottomLeftRadius: 12,
                                        borderBottomRightRadius: 12,
                                        paddingHorizontal: 10,
                                        paddingVertical: 12,
                                        backgroundColor: theme.colors.surface,
                                    }}>
                                        <Text style={{ fontSize: 11, color: theme.colors.textSecondary, ...Typography.default() }}>
                                            No experiments recorded yet.
                                        </Text>
                                    </View>
                                )}
                            </View>
                        </ScrollView>
                    </View>
                </View>
            </View>
        </View>
    );
});

const WorkspaceActivityStrip = React.memo(({
    workspace,
    onRolePress,
    onOpenSession,
    highlightedRoleId,
}: {
    workspace: WorkspaceSummary;
    onRolePress?: (roleId: string) => void;
    onOpenSession?: (sessionId: string) => void;
    highlightedRoleId?: string | null;
}) => {
    const { theme } = useUnistyles();
    const scrollRef = useRef<ScrollView | null>(null);
    const [containerWidth, setContainerWidth] = useState(0);
    const [contentWidth, setContentWidth] = useState(0);
    const [scrollX, setScrollX] = useState(0);
    const allSessions = useAllSessions();
    const sessionMap = React.useMemo(
        () => new Map(allSessions.map((session) => [session.id, session])),
        [allSessions],
    );
    if (workspace.members.length === 0) return null;

    const orderedMembers = React.useMemo(() => {
        return [...workspace.members].sort((a, b) => {
            const aSession = sessionMap.get(a.sessionId);
            const bSession = sessionMap.get(b.sessionId);
            const aActive = (aSession?.presence === 'online' || !!aSession?.streamingText || !!aSession?.streamingThinking) ? 1 : 0;
            const bActive = (bSession?.presence === 'online' || !!bSession?.streamingText || !!bSession?.streamingThinking) ? 1 : 0;
            if (aActive !== bActive) return bActive - aActive;
            return (bSession?.updatedAt ?? 0) - (aSession?.updatedAt ?? 0);
        });
    }, [workspace.members, sessionMap]);

    const canScroll = contentWidth > containerWidth + 8;
    const showLeftHint = canScroll && scrollX > 12;
    const showRightHint = canScroll && scrollX < contentWidth - containerWidth - 12;

    const scrollByPage = useCallback((direction: 'left' | 'right') => {
        if (!scrollRef.current || !containerWidth) return;
        const nextX = direction === 'right'
            ? Math.min(scrollX + Math.max(containerWidth * 0.72, 180), Math.max(contentWidth - containerWidth, 0))
            : Math.max(scrollX - Math.max(containerWidth * 0.72, 180), 0);
        scrollRef.current.scrollTo({ x: nextX, animated: true });
    }, [containerWidth, contentWidth, scrollX]);

    return (
        <View style={{ paddingHorizontal: 16, paddingBottom: 8 }}>
            <View style={{
                borderRadius: 16,
                backgroundColor: theme.colors.surface,
                borderWidth: 1,
                borderColor: theme.colors.divider,
                overflow: 'hidden',
            }}>
                <View style={{
                    paddingHorizontal: 14,
                    paddingTop: 10,
                    paddingBottom: 6,
                    flexDirection: 'row',
                    alignItems: 'center',
                    justifyContent: 'space-between',
                }}>
                    <Text style={{ fontSize: 12, color: theme.colors.textSecondary, ...Typography.default('semiBold') }}>
                        群成员
                    </Text>
                    <Text style={{ fontSize: 11, color: theme.colors.textSecondary, ...Typography.default() }}>
                        点击可 @ 角色
                    </Text>
                </View>
                <View
                    onLayout={(event) => setContainerWidth(event.nativeEvent.layout.width)}
                    style={{ position: 'relative', paddingBottom: 12 }}
                >
                    <ScrollView
                        ref={scrollRef}
                        horizontal
                        showsHorizontalScrollIndicator={false}
                        onContentSizeChange={(width) => setContentWidth(width)}
                        onScroll={(event) => setScrollX(event.nativeEvent.contentOffset.x)}
                        scrollEventThrottle={16}
                        contentContainerStyle={{ gap: 8, paddingHorizontal: 12 }}
                    >
                        {orderedMembers.map((member) => {
                            const runtimeMember = getWorkspaceRuntimeMember(workspace, member.roleId);
                            const latestActivity = getLatestWorkspaceActivityForRole(workspace, member.roleId);
                            const session = sessionMap.get(member.sessionId);
                            const isActive = runtimeMember?.status === 'active' || session?.presence === 'online';
                            const isHighlighted = highlightedRoleId === member.roleId;
                            return (
                                <Pressable
                                    key={member.sessionId}
                                    onPress={() => member.roleId && onRolePress?.(member.roleId)}
                                    style={{
                                        flexDirection: 'row',
                                        alignItems: 'center',
                                        gap: 8,
                                        width: 188,
                                        flexShrink: 0,
                                        paddingHorizontal: 10,
                                        paddingVertical: 8,
                                        borderRadius: 999,
                                        backgroundColor: isHighlighted ? `${theme.colors.button.primary.background}14` : theme.colors.surfaceHigh,
                                        borderWidth: 1,
                                        borderColor: isHighlighted ? theme.colors.button.primary.background : theme.colors.divider,
                                    }}
                                >
                                    <View
                                        style={{
                                            width: 26,
                                            height: 26,
                                            borderRadius: 13,
                                            alignItems: 'center',
                                            justifyContent: 'center',
                                            backgroundColor: isHighlighted ? theme.colors.button.primary.background : theme.colors.surfaceHighest,
                                        }}
                                    >
                                        <Text style={{ fontSize: 11, color: isHighlighted ? theme.colors.button.primary.tint : theme.colors.text, ...Typography.default('semiBold') }}>
                                            {(member.roleId || '?').slice(0, 1).toUpperCase()}
                                        </Text>
                                    </View>
                                    <View style={{ gap: 1, flex: 1, minWidth: 0 }}>
                                        <Text style={{ fontSize: 12, color: theme.colors.text, ...Typography.default('semiBold') }}>
                                            @{member.roleId || 'role'}
                                        </Text>
                                        <Text numberOfLines={1} style={{ fontSize: 11, color: theme.colors.textSecondary, ...Typography.default() }}>
                                            {latestActivity?.text || runtimeMember?.publicStateSummary || (isActive ? '在线' : '待机')}
                                        </Text>
                                    </View>
                                    <View
                                        style={{
                                            width: 7,
                                            height: 7,
                                            borderRadius: 3.5,
                                            marginLeft: 2,
                                            backgroundColor: isActive ? '#34C759' : theme.colors.textSecondary,
                                            opacity: isActive ? 1 : 0.4,
                                        }}
                                    />
                                    {onOpenSession && (
                                        <Pressable
                                            onPress={(event) => {
                                                event.stopPropagation();
                                                onOpenSession(member.sessionId);
                                            }}
                                            hitSlop={6}
                                        >
                                            <Ionicons name="open-outline" size={13} color={theme.colors.textSecondary} />
                                        </Pressable>
                                    )}
                                </Pressable>
                            );
                        })}
                    </ScrollView>
                    {showLeftHint && (
                        <>
                            <LinearGradient
                                colors={[theme.colors.surface, `${theme.colors.surface}00`]}
                                start={{ x: 0, y: 0 }}
                                end={{ x: 1, y: 0 }}
                                pointerEvents="none"
                                style={{
                                    position: 'absolute',
                                    left: 0,
                                    top: 0,
                                    bottom: 12,
                                    width: 34,
                                }}
                            />
                            <Pressable
                                onPress={() => scrollByPage('left')}
                                style={{
                                    position: 'absolute',
                                    left: 8,
                                    top: '50%',
                                    marginTop: -14,
                                    width: 28,
                                    height: 28,
                                    borderRadius: 14,
                                    alignItems: 'center',
                                    justifyContent: 'center',
                                    backgroundColor: `${theme.colors.surfaceHighest}EE`,
                                    borderWidth: 1,
                                    borderColor: theme.colors.divider,
                                }}
                            >
                                <Ionicons name="chevron-back" size={14} color={theme.colors.textSecondary} />
                            </Pressable>
                        </>
                    )}
                    {showRightHint && (
                        <>
                            <LinearGradient
                                colors={[`${theme.colors.surface}00`, theme.colors.surface]}
                                start={{ x: 0, y: 0 }}
                                end={{ x: 1, y: 0 }}
                                pointerEvents="none"
                                style={{
                                    position: 'absolute',
                                    right: 0,
                                    top: 0,
                                    bottom: 12,
                                    width: 40,
                                }}
                            />
                            <Pressable
                                onPress={() => scrollByPage('right')}
                                style={{
                                    position: 'absolute',
                                    right: 8,
                                    top: '50%',
                                    marginTop: -14,
                                    width: 28,
                                    height: 28,
                                    borderRadius: 14,
                                    alignItems: 'center',
                                    justifyContent: 'center',
                                    backgroundColor: `${theme.colors.surfaceHighest}EE`,
                                    borderWidth: 1,
                                    borderColor: theme.colors.divider,
                                }}
                            >
                                <Ionicons name="chevron-forward" size={14} color={theme.colors.textSecondary} />
                            </Pressable>
                        </>
                    )}
                </View>
            </View>
        </View>
    );
});

const WORKSPACE_CHAT_MAX_WIDTH = 860;
const WORKSPACE_CHAT_BUBBLE_MAX_WIDTH = 680;

function roleAccent(roleId: string | null | undefined) {
    const palette = ['#0A84FF', '#34C759', '#FF9F0A', '#FF375F', '#5E5CE6', '#64D2FF'];
    const key = roleId || 'coordinator';
    let hash = 0;
    for (let index = 0; index < key.length; index += 1) {
        hash = (hash * 31 + key.charCodeAt(index)) >>> 0;
    }
    return palette[hash % palette.length];
}

function workspaceRoleLabel(roleId: string | null | undefined, workspace: WorkspaceSummary): string {
    if (roleId) return `@${roleId}`;
    return `@${workspace.binding.defaultRoleId || 'coordinator'}`;
}

function workspaceAvatarText(roleId: string | null | undefined, fallback: string): string {
    if (!roleId) return fallback.slice(0, 1).toUpperCase();
    return roleId.slice(0, 1).toUpperCase();
}

function summarizeLiveWorkspaceMember(
    session: ReturnType<typeof useSession>,
    messages: Message[],
): string | null {
    if (session?.streamingThinking) return '正在思考…';
    if (session?.streamingText) return session.streamingText.trim() || '正在回复…';
    const latest = messages[0];
    if (!latest) return null;
    if (latest.kind === 'tool-call') {
        const toolName = latest.tool.description?.trim() || latest.tool.name;
        return `正在调用 ${toolName}`;
    }
    if (latest.kind === 'agent-text') {
        return latest.text?.trim() || null;
    }
    return null;
}

function getWorkspaceDispatchResultText(workspace: WorkspaceSummary, activity: WorkspaceActivity): string | null {
    const dispatchId = activity.dispatchId;
    if (!dispatchId) return null;
    const dispatch = workspace.runtime?.state?.dispatches?.[dispatchId];
    return dispatch?.resultText?.trim() || null;
}

function getWorkspaceMemberByRole(
    workspace: WorkspaceSummary,
    roleId: string | null | undefined,
): WorkspaceSummary['members'][number] | null {
    if (!roleId) return null;
    return workspace.members.find((member) => member.roleId === roleId) || null;
}

function getLatestWorkspaceActivityForDispatch(
    workspace: WorkspaceSummary,
    dispatchId: string,
): WorkspaceActivity | null {
    const activities = workspace.runtime?.recentActivities || [];
    for (let index = activities.length - 1; index >= 0; index -= 1) {
        const activity = activities[index];
        if (activity.dispatchId === dispatchId) return activity;
    }
    return null;
}

function isWorkspaceChatBubbleActivity(kind: WorkspaceActivity['kind']): boolean {
    switch (kind) {
        case 'user_message':
        case 'coordinator_message':
        case 'member_delivered':
        case 'member_summary':
        case 'dispatch_completed':
        case 'system_notice':
            return true;
        default:
            return false;
    }
}

function getWorkspaceDispatchLiveStatus(
    workspace: WorkspaceSummary,
    dispatch: WorkspaceRuntimeSummary['state']['dispatches'][string],
    member: WorkspaceSummary['members'][number] | null,
    session: ReturnType<typeof useSession>,
    messages: Message[],
): string | null {
    const directSessionSummary = summarizeLiveWorkspaceMember(session, messages);
    if (directSessionSummary) return directSessionSummary;

    const latestActivity = getLatestWorkspaceActivityForDispatch(workspace, dispatch.dispatchId);
    if (latestActivity) {
        switch (latestActivity.kind) {
            case 'member_claimed':
                return '已认领，准备处理中…';
            case 'member_supporting':
                return '正在协助处理中…';
            case 'dispatch_started':
                return '开始处理…';
            case 'dispatch_progress':
            case 'member_progress':
                return latestActivity.text || '处理中…';
            case 'member_blocked':
                return latestActivity.text || '处理中遇到阻塞';
            case 'member_declined':
                return '已放弃认领';
            default:
                break;
        }
    }

    switch (dispatch.status) {
        case 'queued':
            return dispatch.claimStatus === 'claimed' ? '已认领，等待开始…' : '等待处理…';
        case 'started':
        case 'running':
            return dispatch.lastSummary?.trim() || dispatch.summary?.trim() || '处理中…';
        default:
            break;
    }

    if (member?.roleId) {
        const activities = workspace.runtime?.recentActivities || [];
        for (let index = activities.length - 1; index >= 0; index -= 1) {
            const activity = activities[index];
            if ((activity.roleId || activity.memberId) !== member.roleId) continue;
            if (activity.kind === 'member_claimed') return '已认领，准备处理中…';
            if (activity.kind === 'dispatch_started') return '开始处理…';
            if (activity.kind === 'dispatch_progress' || activity.kind === 'member_progress') {
                return activity.text || '处理中…';
            }
        }
    }

    return null;
}

const WorkspacePendingBubble = React.memo(({
    dispatch,
    member,
    workspace,
}: {
    dispatch: WorkspaceRuntimeSummary['state']['dispatches'][string];
    member: WorkspaceSummary['members'][number] | null;
    workspace: WorkspaceSummary;
}) => {
    const { theme } = useUnistyles();
    const session = useSession(member?.sessionId || '');
    const { messages } = useSessionMessages(member?.sessionId || '');
    const summary = React.useMemo(
        () => getWorkspaceDispatchLiveStatus(workspace, dispatch, member, session, messages),
        [workspace, dispatch, member, messages, session],
    );

    if (!summary) return null;

    const accent = roleAccent(dispatch.roleId || member?.roleId);
    const label = member?.roleId ? `@${member.roleId}` : `@${dispatch.roleId}`;
    const timestamp = dispatch.startedAt || dispatch.createdAt;

    return (
        <View style={{ width: '100%', flexDirection: 'row', alignItems: 'flex-start', gap: 10 }}>
            <View
                style={{
                    width: 30,
                    height: 30,
                    borderRadius: 15,
                    alignItems: 'center',
                    justifyContent: 'center',
                    backgroundColor: `${accent}18`,
                    borderWidth: 1,
                    borderColor: `${accent}33`,
                    marginTop: 2,
                }}
            >
                <Text style={{ fontSize: 11, color: accent, ...Typography.default('semiBold') }}>
                    {workspaceAvatarText(member?.roleId || dispatch.roleId, 'A')}
                </Text>
            </View>
            <View style={{ maxWidth: WORKSPACE_CHAT_BUBBLE_MAX_WIDTH, flex: 1, minWidth: 0 }}>
                <View style={{ flexDirection: 'row', alignItems: 'center', gap: 8, marginBottom: 4 }}>
                    <Text style={{ fontSize: 12, color: theme.colors.textSecondary, ...Typography.default('semiBold') }}>
                        {label}
                    </Text>
                    <Text style={{ fontSize: 11, color: theme.colors.textSecondary, ...Typography.default() }}>
                        {formatWorkspaceTimestamp(timestamp)}
                    </Text>
                </View>
                <View
                    style={{
                        alignSelf: 'flex-start',
                        paddingHorizontal: 12,
                        paddingVertical: 8,
                        borderRadius: 14,
                        backgroundColor: theme.colors.surfaceHigh,
                        borderWidth: 1,
                        borderColor: theme.colors.divider,
                    }}
                >
                    <Text numberOfLines={1} style={{ fontSize: 13, color: theme.colors.textSecondary, ...Typography.default() }}>
                        {summary}
                    </Text>
                </View>
            </View>
        </View>
    );
});

const WorkspaceChatFeed = React.memo(({
    workspace,
}: {
    workspace: WorkspaceSummary;
}) => {
    const { theme } = useUnistyles();
    const publicActivities = React.useMemo(() => {
        return (workspace.runtime?.recentActivities || [])
            .filter((activity) => activity.visibility === 'public' && isWorkspaceChatBubbleActivity(activity.kind))
            .slice(-32);
    }, [workspace.runtime?.recentActivities]);
    const pendingDispatches = React.useMemo(() => {
        const dispatches = Object.values(workspace.runtime?.state?.dispatches || {});
        return dispatches
            .filter((dispatch) => {
                if (!dispatch.roleId) return false;
                if (dispatch.visibility && dispatch.visibility !== 'public') return false;
                if (dispatch.status === 'completed' || dispatch.status === 'failed' || dispatch.status === 'stopped') return false;
                return true;
            })
            .sort((a, b) => {
                const aTime = new Date(a.startedAt || a.createdAt).getTime();
                const bTime = new Date(b.startedAt || b.createdAt).getTime();
                return aTime - bTime;
            });
    }, [workspace.runtime?.state?.dispatches]);
    return (
        <ScrollView
            style={{ flex: 1, minHeight: 0 }}
            contentContainerStyle={{
                paddingHorizontal: 16,
                paddingTop: 8,
                paddingBottom: 18,
                alignItems: 'stretch',
                gap: 12,
            }}
            keyboardShouldPersistTaps="handled"
            showsVerticalScrollIndicator
        >
            {publicActivities.map((activity) => {
                const isUser = activity.kind === 'user_message';
                const accent = roleAccent(activity.roleId || (isUser ? 'user' : workspace.binding.defaultRoleId));
                const label = isUser ? '你' : workspaceRoleLabel(activity.roleId, workspace);
                const bubbleText =
                    getWorkspaceDispatchResultText(workspace, activity)
                    || activity.text;
                return (
                    <View
                        key={activity.activityId}
                        style={{
                            width: '100%',
                            maxWidth: WORKSPACE_CHAT_MAX_WIDTH,
                            flexDirection: 'row',
                            justifyContent: isUser ? 'flex-end' : 'flex-start',
                        }}
                    >
                        <View
                            style={{
                                gap: 10,
                                maxWidth: WORKSPACE_CHAT_MAX_WIDTH,
                                flexShrink: 1,
                                alignItems: 'flex-start',
                                flexDirection: isUser ? 'row-reverse' : 'row',
                            }}
                        >
                            <View
                                style={{
                                    width: 30,
                                    height: 30,
                                    borderRadius: 15,
                                    alignItems: 'center',
                                    justifyContent: 'center',
                                    backgroundColor: isUser ? theme.colors.button.primary.background : `${accent}18`,
                                    borderWidth: 1,
                                    borderColor: isUser ? `${theme.colors.button.primary.background}44` : `${accent}33`,
                                    marginTop: 2,
                                }}
                            >
                                <Text style={{ fontSize: 11, color: isUser ? theme.colors.button.primary.tint : accent, ...Typography.default('semiBold') }}>
                                    {isUser ? '你' : workspaceAvatarText(activity.roleId, 'C')}
                                </Text>
                            </View>
                            <View style={{ flex: 1, minWidth: 0, maxWidth: WORKSPACE_CHAT_BUBBLE_MAX_WIDTH }}>
                                <View style={{
                                    flexDirection: 'row',
                                    alignItems: 'center',
                                    justifyContent: isUser ? 'flex-end' : 'flex-start',
                                    gap: 8,
                                    marginBottom: 4,
                                }}>
                                    <Text style={{ fontSize: 12, color: theme.colors.textSecondary, ...Typography.default('semiBold') }}>
                                        {label}
                                    </Text>
                                    <Text style={{ fontSize: 11, color: theme.colors.textSecondary, ...Typography.default() }}>
                                        {formatWorkspaceTimestamp(activity.createdAt)}
                                    </Text>
                                </View>
                                <View
                                    style={{
                                        alignSelf: isUser ? 'flex-end' : 'flex-start',
                                        paddingHorizontal: 14,
                                        paddingVertical: 11,
                                        borderRadius: 18,
                                        backgroundColor: isUser ? theme.colors.button.primary.background : theme.colors.surfaceHigh,
                                        borderWidth: 1,
                                        borderColor: isUser ? `${theme.colors.button.primary.background}44` : theme.colors.divider,
                                    }}
                                >
                                    <Text
                                        selectable
                                        style={{
                                            fontSize: 14,
                                            lineHeight: 22,
                                            color: isUser ? theme.colors.button.primary.tint : theme.colors.text,
                                            ...Typography.default(),
                                        }}
                                    >
                                        {bubbleText}
                                    </Text>
                                </View>
                            </View>
                        </View>
                    </View>
                );
            })}

            {pendingDispatches.map((dispatch) => (
                <View key={`dispatch-${dispatch.dispatchId}`} style={{ width: '100%', maxWidth: WORKSPACE_CHAT_MAX_WIDTH }}>
                    <WorkspacePendingBubble
                        dispatch={dispatch}
                        member={getWorkspaceMemberByRole(workspace, dispatch.roleId)}
                        workspace={workspace}
                    />
                </View>
            ))}

            {publicActivities.length === 0 && (
                <View
                    style={{
                        width: '100%',
                        maxWidth: WORKSPACE_CHAT_MAX_WIDTH,
                        borderRadius: 18,
                        borderWidth: 1,
                        borderColor: theme.colors.divider,
                        backgroundColor: theme.colors.surface,
                        paddingHorizontal: 16,
                        paddingVertical: 14,
                    }}
                >
                    <Text style={{ fontSize: 13, color: theme.colors.textSecondary, lineHeight: 20, ...Typography.default() }}>
                        这里会像群聊一样显示公开动态。你发一句话后，协调者和成员会在这里认领、推进并回复。
                    </Text>
                </View>
            )}
        </ScrollView>
    );
});

function parseWorkspaceRoleMention(
    text: string,
    workspace: WorkspaceSummary | null | undefined,
): { roleId: string; instruction: string } | null {
    if (!workspace) return null;
    const trimmed = text.trim();
    if (!trimmed.startsWith('@')) return null;

    const match = trimmed.match(/^@([a-zA-Z0-9_-]+)\s*(.*)$/s);
    if (!match) return null;

    const roleId = match[1];
    const instruction = (match[2] || '').trim();
    if (!instruction) return null;

    const valid = workspace.members.some((member) => member.roleId === roleId);
    if (!valid) return null;

    return { roleId, instruction };
}

function summarizeWorkspaceTurnResult(result: {
    plan?: { responseText?: string | null };
    workflowVoteWindow?: { reason?: string | null } | null;
    workflowVoteResponses?: Array<{ decision?: string | null }>;
    dispatches?: Array<{ roleId?: string | null }>;
    roleId?: string | null;
}): string | null {
    const assignedRoleIds = result.dispatches?.map((dispatch) => dispatch.roleId).filter(Boolean) as string[] | undefined;
    if (assignedRoleIds?.length && assignedRoleIds.length > 1) {
        return `已分派给 ${assignedRoleIds.map((roleId) => `@${roleId}`).join('、')}`;
    }
    if (result.roleId) {
        return `已由 @${result.roleId} 认领`;
    }
    if (result.workflowVoteWindow) {
        const approvals = result.workflowVoteResponses?.filter((response) => response.decision === 'approve').length ?? 0;
        const rejects = result.workflowVoteResponses?.filter((response) => response.decision === 'reject').length ?? 0;
        if (approvals > rejects) {
            return `已发起流程并通过投票：${result.workflowVoteWindow.reason || '进入 workflow'}`;
        }
        return `已发起流程投票：${result.workflowVoteWindow.reason || '等待团队表决'}`;
    }
    if (result.plan?.responseText) {
        return result.plan.responseText;
    }
    return null;
}

function extractFocusedWorkspaceRole(
    text: string,
    workspace: WorkspaceSummary | null | undefined,
): string | null {
    if (!workspace) return null;
    const trimmed = text.trim();
    if (!trimmed.startsWith('@')) return null;
    const match = trimmed.match(/^@([a-zA-Z0-9_-]+)/);
    if (!match) return null;
    const roleId = match[1];
    return workspace.members.some((member) => member.roleId === roleId) ? roleId : null;
}

const PersonaToolbar = React.memo(({
    vendor,
    permissionMode,
    onPermissionModeChange,
    sandboxPolicy,
    onSandboxPolicyChange,
    activeMcpCount,
    onMcpClick,
    activeRunCount,
    onRunsClick,
}: {
    vendor?: VendorName | null;
    permissionMode?: PermissionMode;
    onPermissionModeChange?: (mode: PermissionMode) => void;
    sandboxPolicy?: 'workspace_write' | 'unrestricted';
    onSandboxPolicyChange?: (policy: 'workspace_write' | 'unrestricted') => void;
    activeMcpCount?: number;
    onMcpClick?: () => void;
    activeRunCount?: number;
    onRunsClick?: () => void;
}) => {
    const { theme } = useUnistyles();
    const [showPermDropdown, setShowPermDropdown] = React.useState(false);
    const [showSandboxDropdown, setShowSandboxDropdown] = React.useState(false);
    const permissionModes = React.useMemo(
        () => permissionModesForVendor(vendor),
        [vendor],
    );

    return (
        <View style={{
            flexDirection: 'row',
            alignItems: 'center',
            paddingHorizontal: 24,
            paddingBottom: 4,
            gap: 12,
            maxWidth: layout.maxWidth,
            alignSelf: 'center',
            width: '100%',
        }}>
            {/* Permission mode */}
            {permissionMode && onPermissionModeChange && (
                <View style={{ position: 'relative' }}>
                    <Pressable
                        onPress={() => {
                            hapticsLight();
                            setShowPermDropdown(prev => !prev);
                        }}
                        hitSlop={{ top: 5, bottom: 5, left: 5, right: 5 }}
                        style={(p) => ({
                            flexDirection: 'row',
                            alignItems: 'center',
                            opacity: p.pressed ? 0.6 : 1,
                        })}
                    >
                        <Ionicons
                            name={permissionModeIcon(permissionMode, vendor) as any}
                            size={12}
                            color={theme.colors.textSecondary}
                            style={{ marginRight: 2 }}
                        />
                        <Text style={{ fontSize: 12, color: theme.colors.textSecondary }}>
                            {permissionModeLabel(permissionMode, vendor)}
                        </Text>
                        <Ionicons
                            name="chevron-up"
                            size={10}
                            color={theme.colors.textSecondary}
                            style={{ marginLeft: 2 }}
                        />
                    </Pressable>

                    {showPermDropdown && (
                        <>
                            <TouchableWithoutFeedback onPress={() => setShowPermDropdown(false)}>
                                <View style={{
                                    position: 'absolute',
                                    top: -1000, left: -1000, right: -1000, bottom: -1000,
                                    zIndex: 999,
                                }} />
                            </TouchableWithoutFeedback>
                            <View style={{
                                position: 'absolute',
                                bottom: '100%',
                                left: 0,
                                marginBottom: 8,
                                zIndex: 1000,
                            }}>
                                <FloatingOverlay maxHeight={240} keyboardShouldPersistTaps="always">
                                    <View style={{ paddingVertical: 8 }}>
                                        {permissionModes.map((mode) => {
                                            const isSelected = permissionMode === mode;
                                            return (
                                                <Pressable
                                                    key={mode}
                                                    onPress={() => {
                                                        hapticsLight();
                                                        onPermissionModeChange(mode);
                                                        setShowPermDropdown(false);
                                                    }}
                                                    style={({ pressed }) => ({
                                                        flexDirection: 'row',
                                                        alignItems: 'center',
                                                        paddingHorizontal: 16,
                                                        paddingVertical: 8,
                                                        backgroundColor: pressed ? theme.colors.surfacePressed : 'transparent',
                                                    })}
                                                >
                                                    <View style={{
                                                        width: 16, height: 16, borderRadius: 8,
                                                        borderWidth: 2,
                                                        borderColor: isSelected ? theme.colors.radio.active : theme.colors.radio.inactive,
                                                        alignItems: 'center', justifyContent: 'center',
                                                        marginRight: 12,
                                                    }}>
                                                        {isSelected && (
                                                            <View style={{
                                                                width: 6, height: 6, borderRadius: 3,
                                                                backgroundColor: theme.colors.radio.dot,
                                                            }} />
                                                        )}
                                                    </View>
                                                    <Ionicons
                                                        name={permissionModeIcon(mode, vendor) as any}
                                                        size={14}
                                                        color={isSelected ? theme.colors.radio.active : theme.colors.text}
                                                        style={{ marginRight: 8 }}
                                                    />
                                                    <Text style={{
                                                        fontSize: 14,
                                                        color: isSelected ? theme.colors.radio.active : theme.colors.text,
                                                    }}>
                                                        {permissionModeLabel(mode, vendor)}
                                                    </Text>
                                                </Pressable>
                                            );
                                        })}
                                    </View>
                                </FloatingOverlay>
                            </View>
                        </>
                    )}
                </View>
            )}

            {/* Sandbox policy */}
            {sandboxPolicy && onSandboxPolicyChange && (
                <View style={{ position: 'relative' }}>
                    <Pressable
                        onPress={() => {
                            hapticsLight();
                            setShowSandboxDropdown(prev => !prev);
                        }}
                        hitSlop={{ top: 5, bottom: 5, left: 5, right: 5 }}
                        style={(p) => ({
                            flexDirection: 'row',
                            alignItems: 'center',
                            opacity: p.pressed ? 0.6 : 1,
                        })}
                    >
                        <Ionicons
                            name={sandboxPolicy === 'unrestricted' ? 'globe-outline' : 'folder-outline'}
                            size={12}
                            color={sandboxPolicy === 'unrestricted' ? theme.colors.permission.bypass : theme.colors.textSecondary}
                            style={{ marginRight: 2 }}
                        />
                        <Text style={{
                            fontSize: 12,
                            color: sandboxPolicy === 'unrestricted' ? theme.colors.permission.bypass : theme.colors.textSecondary,
                        }}>
                            {sandboxLabels[sandboxPolicy] || sandboxPolicy}
                        </Text>
                        <Ionicons
                            name="chevron-up"
                            size={10}
                            color={theme.colors.textSecondary}
                            style={{ marginLeft: 2 }}
                        />
                    </Pressable>

                    {showSandboxDropdown && (
                        <>
                            <TouchableWithoutFeedback onPress={() => setShowSandboxDropdown(false)}>
                                <View style={{
                                    position: 'absolute',
                                    top: -1000, left: -1000, right: -1000, bottom: -1000,
                                    zIndex: 999,
                                }} />
                            </TouchableWithoutFeedback>
                            <View style={{
                                position: 'absolute',
                                bottom: '100%',
                                left: 0,
                                marginBottom: 8,
                                zIndex: 1000,
                            }}>
                                <FloatingOverlay maxHeight={160} keyboardShouldPersistTaps="always">
                                    <View style={{ paddingVertical: 8, minWidth: 180 }}>
                                        {(['workspace_write', 'unrestricted'] as const).map((policy) => {
                                            const isSelected = sandboxPolicy === policy;
                                            const iconName = policy === 'unrestricted' ? 'globe-outline' : 'folder-outline';
                                            return (
                                                <Pressable
                                                    key={policy}
                                                    onPress={() => {
                                                        hapticsLight();
                                                        onSandboxPolicyChange(policy);
                                                        setShowSandboxDropdown(false);
                                                    }}
                                                    style={({ pressed }) => ({
                                                        flexDirection: 'row',
                                                        alignItems: 'center',
                                                        paddingHorizontal: 16,
                                                        paddingVertical: 8,
                                                        backgroundColor: pressed ? theme.colors.surfacePressed : 'transparent',
                                                    })}
                                                >
                                                    <View style={{
                                                        width: 16, height: 16, borderRadius: 8,
                                                        borderWidth: 2,
                                                        borderColor: isSelected ? theme.colors.radio.active : theme.colors.radio.inactive,
                                                        alignItems: 'center', justifyContent: 'center',
                                                        marginRight: 12,
                                                    }}>
                                                        {isSelected && (
                                                            <View style={{
                                                                width: 6, height: 6, borderRadius: 3,
                                                                backgroundColor: theme.colors.radio.dot,
                                                            }} />
                                                        )}
                                                    </View>
                                                    <Ionicons
                                                        name={iconName}
                                                        size={14}
                                                        color={isSelected ? theme.colors.radio.active : theme.colors.text}
                                                        style={{ marginRight: 8 }}
                                                    />
                                                    <Text style={{
                                                        fontSize: 14,
                                                        color: isSelected ? theme.colors.radio.active : theme.colors.text,
                                                    }}>
                                                        {sandboxLabels[policy] || policy}
                                                    </Text>
                                                </Pressable>
                                            );
                                        })}
                                    </View>
                                </FloatingOverlay>
                            </View>
                        </>
                    )}
                </View>
            )}

            {onMcpClick && (
                <Pressable
                    onPress={() => {
                        hapticsLight();
                        onMcpClick();
                    }}
                    hitSlop={{ top: 5, bottom: 5, left: 5, right: 5 }}
                    style={(p) => ({
                        flexDirection: 'row',
                        alignItems: 'center',
                        opacity: p.pressed ? 0.6 : 1,
                    })}
                >
                    <Ionicons
                        name="server-outline"
                        size={13}
                        color={(activeMcpCount ?? 0) > 0 ? theme.colors.text : theme.colors.textSecondary}
                        style={{ marginRight: 3 }}
                    />
                    <Text style={{
                        fontSize: 12,
                        color: (activeMcpCount ?? 0) > 0 ? theme.colors.text : theme.colors.textSecondary,
                        ...Typography.default('semiBold'),
                    }}>
                        MCP ({activeMcpCount ?? 0})
                    </Text>
                </Pressable>
            )}

            {/* Background runs — hidden when count is 0 */}
            {onRunsClick && (activeRunCount ?? 0) > 0 && (
                <Pressable
                    onPress={() => {
                        hapticsLight();
                        onRunsClick();
                    }}
                    hitSlop={{ top: 5, bottom: 5, left: 5, right: 5 }}
                    style={(p) => ({
                        flexDirection: 'row',
                        alignItems: 'center',
                        opacity: p.pressed ? 0.6 : 1,
                    })}
                >
                    <Ionicons
                        name={(activeRunCount ?? 0) > 0 ? 'play-circle' : 'play-circle-outline'}
                        size={13}
                        color={(activeRunCount ?? 0) > 0 ? '#34C759' : theme.colors.textSecondary}
                        style={{ marginRight: 3 }}
                    />
                    <Text style={{
                        fontSize: 12,
                        color: (activeRunCount ?? 0) > 0 ? '#34C759' : theme.colors.textSecondary,
                        ...Typography.default('semiBold'),
                    }}>
                        后台任务 ({activeRunCount ?? 0})
                    </Text>
                </Pressable>
            )}

        </View>
    );
});

/**
 * Persona chat page — a dedicated chat UI for persona conversations.
 */
export default function PersonaChatPage() {
    const { id: personaId } = useLocalSearchParams<{ id: string }>();
    const { theme } = useUnistyles();
    const router = useRouter();
    const machines = useAllMachines();
    const sessions = useAllSessions();

    const machineId = useMemo(() => {
        const machineIds = new Set<string>();
        for (const session of sessions) {
            const mid = session.metadata?.machineId;
            if (mid) machineIds.add(mid);
        }
        for (const mid of machineIds) {
            if (machines.some(m => m.id === mid && isMachineOnline(m))) {
                return mid;
            }
        }
        const online = machines.find(m => isMachineOnline(m));
        return online?.id || (machines.length > 0 ? machines[0].id : undefined);
    }, [machines, sessions]);

    const { personas, loading } = usePersonas({ machineId });
    const { workspaces, refresh: refreshWorkspaces } = useAgentWorkspaces({
        machineId,
        pollingInterval: 5000,
    });

    const persona = useMemo(() => {
        return personas.find(p => p.id === personaId);
    }, [personas, personaId]);

    const workspace = useMemo(() => {
        return workspaces.find((item) => item.persona.id === personaId) || null;
    }, [workspaces, personaId]);

    // Only show loading spinner on initial load (no personas yet).
    // Once persona is found, never unmount the chat — background refreshes
    // should not disrupt the conversation.
    if (!persona) {
        return (
            <View style={{
                flex: 1,
                backgroundColor: theme.colors.surface,
                alignItems: 'center',
                justifyContent: 'center',
            }}>
                <ActivityIndicator size="large" color={theme.colors.textSecondary} />
                <Text style={{
                    marginTop: 16,
                    fontSize: 14,
                    color: theme.colors.textSecondary,
                    ...Typography.default(),
                }}>
                    {loading ? t('persona.loading') : t('persona.notFound')}
                </Text>
            </View>
        );
    }

    return (
        <PersonaChatLoaded
            persona={persona}
            machineId={machineId}
            onBack={() => router.back()}
            workspace={workspace}
            onWorkspaceRefresh={refreshWorkspaces}
        />
    );
}

interface PersonaChatLoadedProps {
    persona: {
        id: string;
        name: string;
        description: string;
        avatarId: string;
        chatSessionId: string;
        continuousBrowsing: boolean;
        modelId: string | null;
        workdir: string;
        agent?: VendorName;
    };
    machineId?: string;
    onBack: () => void;
    /** Override the default ChatHeaderView. Receives session for connection status. */
    renderHeader?: (session: import('@/sync/storageTypes').Session | null) => React.ReactNode;
    /** Override the default clear behavior (which creates a new persona). */
    onClear?: () => Promise<void> | void;
    /** Called when the user switches the model. Use this to persist the
     *  change for non-persona entities (e.g. hypothesis agents). */
    onProfileChange?: (modelId: string) => void;
    /** Hide the model picker button (e.g. for hypothesis agents with fixed model). */
    hideProfilePicker?: boolean;
    /** Hide header action icons (memory, notifications, continuous browsing). */
    hideHeaderActions?: boolean;
    workspace?: WorkspaceSummary | null;
    onWorkspaceRefresh?: () => Promise<void> | void;
}

// Fallback session object used when the real session hasn't loaded yet.
// This keeps useSessionStatus callable unconditionally (React hooks rules).
const FALLBACK_SESSION: import('@/sync/storageTypes').Session = {
    id: '',
    seq: 0,
    createdAt: 0,
    updatedAt: 0,
    active: false,
    activeAt: 0,
    metadata: null,
    metadataVersion: 0,
    agentState: null,
    agentStateVersion: 0,
    thinking: false,
    thinkingAt: 0,
    presence: 0 as any,
};

export { PersonaChatLoaded };
export type { PersonaChatLoadedProps };

function PersonaChatLoaded({ persona, machineId, onBack, renderHeader, onClear: onClearProp, onProfileChange, hideProfilePicker, hideHeaderActions, workspace, onWorkspaceRefresh }: PersonaChatLoadedProps) {
    const { theme } = useUnistyles();
    const router = useRouter();
    const safeArea = useSafeAreaInsets();
    const headerHeight = useHeaderHeight();
    const [isDeletingWorkspace, setIsDeletingWorkspace] = useState(false);
    const personaAgent: VendorName = persona.agent ?? 'cteno';
    const runtimeControls = useSessionRuntimeControls(persona.chatSessionId);
    const runtimeVendor = runtimeControls.vendor ?? personaAgent;
    const supportedPermissionModes = React.useMemo(
        () => permissionModesForVendor(runtimeVendor),
        [runtimeVendor],
    );
    const showPermissionModeSelector = runtimeControls.permissionMode.outcome !== 'unsupported';
    const showModelPicker = !hideProfilePicker && runtimeControls.model.outcome !== 'unsupported';
    const modelChangeNeedsRestart = runtimeControls.model.outcome === 'restart_required';

    // Handle pending session: when chatSessionId starts with "pending-", the
    // server session is being created in the background. Listen for the
    // persona_session_ready push event and refresh personas to get the real ID.
    const isPendingSession = persona.chatSessionId.startsWith('pending-');

    const handleDeleteWorkspace = useCallback(async () => {
        if (!workspace || !machineId || isDeletingWorkspace) return;
        setIsDeletingWorkspace(true);
        try {
            const result = await machineDeleteAgentWorkspace(machineId, workspace.persona.id);
            if (!result.success) {
                throw new Error(result.error || 'Failed to delete workspace');
            }
            await onWorkspaceRefresh?.();
            router.replace('/persona' as any);
        } catch (error) {
            console.error('Failed to delete workspace:', error);
        } finally {
            setIsDeletingWorkspace(false);
        }
    }, [isDeletingWorkspace, machineId, onWorkspaceRefresh, router, workspace]);

    const effectiveSessionId = isPendingSession ? '' : persona.chatSessionId;
    const session = useSession(effectiveSessionId);
    const { messages, isLoaded } = useSessionMessages(effectiveSessionId);
    const runtimeEffort: RuntimeEffort = session?.runtimeEffort || 'default';
    const [message, setMessage] = React.useState('');
    const [pendingImages, setPendingImages] = React.useState<PendingImage[]>([]);
    const [workspaceRouteStatus, setWorkspaceRouteStatus] = React.useState<string | null>(null);
    const [workspaceRuntimeOverride, setWorkspaceRuntimeOverride] = React.useState<WorkspaceRuntimeWithTemplateState | null>(null);
    const [isGatedTaskControlPending, setIsGatedTaskControlPending] = React.useState(false);
    const [showMemory, setShowMemory] = React.useState(false);
    const [showWorkspaceBrowser, setShowWorkspaceBrowser] = React.useState(false);
    const realtimeStatus = useRealtimeStatus();

    React.useEffect(() => {
        setWorkspaceRuntimeOverride((previousRuntime) =>
            mergeWorkspaceRuntimeWithTemplateState(workspace?.runtime, previousRuntime)
        );
    }, [persona.id, workspace?.runtime]);

    const effectiveWorkspace = React.useMemo(() => {
        if (!workspace) return workspace;
        return {
            ...workspace,
            runtime: workspaceRuntimeOverride ?? workspace.runtime ?? null,
        };
    }, [workspace, workspaceRuntimeOverride]);
    const gatedTasksTemplateState = React.useMemo(
        () => getGatedTasksTemplateState(effectiveWorkspace),
        [effectiveWorkspace],
    );
    const autoresearchTemplateState = React.useMemo(
        () => getAutoresearchTemplateState(effectiveWorkspace),
        [effectiveWorkspace],
    );

    // Fetch models to determine vision support
    const [models, setModels] = React.useState<ModelOptionDisplay[]>([]);
    const [defaultModelId, setDefaultModelId] = React.useState<string>(() => {
        if (!machineId) return 'default';
        return loadCachedVendorDefaultModelId(machineId, runtimeVendor) || 'default';
    });
    // Local override for model selection (survives updatePersona failures, e.g. hypothesis agents)
    const [selectedModelId, setSelectedModelId] = React.useState<string | null>(null);
    const reloadPersonaModels = React.useCallback(() => {
        if (!machineId) return Promise.resolve();
        return machineListModels(machineId, runtimeVendor).then(result => {
            setModels(result.models || []);
            setDefaultModelId(result.defaultModelId || 'default');
            frontendLog(`[PersonaModelSource] ${JSON.stringify({
                machineId,
                runtimeVendor,
                personaId: persona.id,
                chatSessionId: persona.chatSessionId,
                count: (result.models || []).length,
                defaultModelId: result.defaultModelId || 'default',
                ids: (result.models || []).slice(0, 20).map((model) => ({
                    id: model.id,
                    chatModel: model.chat?.model,
                    isProxy: model.isProxy === true,
                    sourceType: model.sourceType ?? null,
                    vendor: model.vendor ?? null,
                    supportedReasoningEfforts: model.supportedReasoningEfforts || [],
                })),
            })}`);
        }).catch(() => {});
    }, [machineId, persona.chatSessionId, persona.id, runtimeVendor]);

    React.useEffect(() => {
        reloadPersonaModels();
    }, [reloadPersonaModels, persona.id, persona.chatSessionId]);

    const currentModelId = useMemo(
        () => selectedModelId || session?.metadata?.modelId || defaultModelId || persona.modelId || 'default',
        [selectedModelId, session?.metadata?.modelId, defaultModelId, persona.modelId],
    );
    const currentModel = useMemo(
        () => models.find((model) => model.id === currentModelId) || null,
        [currentModelId, models],
    );
    const currentModelEffortLevels = useMemo(
        () => currentModel?.supportedReasoningEfforts?.length
            ? currentModel.supportedReasoningEfforts
            : undefined,
        [currentModel],
    );

    const supportsVision = useMemo(() => {
        return currentModel?.supportsVision ?? false;
    }, [currentModel]);

    // Current model name for display
    const currentModelName = useMemo(() => {
        return currentModel?.name || currentModelId || 'default';
    }, [currentModel, currentModelId]);

    // Continuous browsing toggle
    const { createPersona, updatePersona, refresh: refreshPersonas } = usePersonas({ machineId });

    // When session is pending, listen for push event + poll fast to get real session ID
    React.useEffect(() => {
        if (!isPendingSession) return;
        const unsub = onHypothesisPush((pushId, event) => {
            if (pushId === persona.id && event === 'persona_session_ready') {
                refreshPersonas();
            }
        });
        const timer = setInterval(refreshPersonas, 2000);
        return () => { unsub(); clearInterval(timer); };
    }, [isPendingSession, persona.id, refreshPersonas]);

    const [continuousBrowsing, setContinuousBrowsing] = React.useState(persona.continuousBrowsing);
    const doToggleContinuousBrowsing = React.useCallback(async (next: boolean) => {
        setContinuousBrowsing(next);
        try {
            await updatePersona({ id: persona.id, continuousBrowsing: next });
        } catch {
            setContinuousBrowsing(!next); // revert on error
        }
    }, [persona.id, updatePersona]);

    const handleToggleContinuousBrowsing = React.useCallback(async () => {
        const next = !continuousBrowsing;
        // Turning off never needs confirmation
        if (!next) {
            doToggleContinuousBrowsing(next);
            return;
        }
        // Skip confirmation if user chose "don't ask again"
        const skip = storage.getState().localSettings.skipContinuousBrowsingConfirm;
        if (skip) {
            doToggleContinuousBrowsing(next);
            return;
        }
        Modal.show({
            component: ContinuousBrowsingConfirmModal,
            props: {
                onConfirm: (dontRemind: boolean) => {
                    if (dontRemind) {
                        storage.getState().applyLocalSettings({ skipContinuousBrowsingConfirm: true });
                    }
                    doToggleContinuousBrowsing(next);
                },
            },
        });
    }, [continuousBrowsing, doToggleContinuousBrowsing]);

    // Model switching
    const profileModalIdRef = React.useRef<string | null>(null);
    const handleEffortPress = React.useCallback(() => {
        if (!currentModelId || runtimeControls.model.outcome === 'unsupported') return;
        let effortModalId: string | null = null;

        effortModalId = Modal.show({
            component: EffortPickerModal,
            props: {
                value: runtimeEffort,
                availableLevels: currentModelEffortLevels,
                onSelect: async (effort: RuntimeEffort) => {
                    if (effortModalId) {
                        Modal.hide(effortModalId);
                        effortModalId = null;
                    }
                    try {
                        await sessionApplyRuntimeModelChange(
                            persona.chatSessionId,
                            machineId,
                            currentModelId,
                            effort,
                            runtimeControls.model,
                        );
                    } catch (error) {
                        console.error('Failed to switch reasoning effort:', error);
                    }
                },
                onClose: () => {
                    if (effortModalId) {
                        Modal.hide(effortModalId);
                        effortModalId = null;
                    }
                },
            },
        });
    }, [currentModelEffortLevels, currentModelId, machineId, persona.chatSessionId, runtimeControls.model, runtimeEffort]);

    const handleModelPress = React.useCallback(() => {
        const showPicker = (nextModels: ModelOptionDisplay[]) => {
            if (nextModels.length === 0) return;
            frontendLog(`[PersonaModelPress.showPicker] ${JSON.stringify({
                machineId: machineId ?? null,
                runtimeVendor,
                personaId: persona.id,
                chatSessionId: persona.chatSessionId,
                currentModelId,
                count: nextModels.length,
                ids: nextModels.slice(0, 20).map((model) => ({
                    id: model.id,
                    chatModel: model.chat?.model,
                    isProxy: model.isProxy === true,
                    sourceType: model.sourceType ?? null,
                    vendor: model.vendor ?? null,
                })),
            })}`);

            profileModalIdRef.current = Modal.show({
                component: ProfilePickerModal,
                props: {
                    models: nextModels,
                    currentModelId,
                    onSelect: async (modelId: string) => {
                        if (profileModalIdRef.current) {
                            Modal.hide(profileModalIdRef.current);
                            profileModalIdRef.current = null;
                        }
                        try {
                            if (machineId && runtimeControls.model.outcome !== 'unsupported') {
                                await sessionApplyRuntimeModelChange(
                                    persona.chatSessionId,
                                    machineId,
                                    modelId,
                                    runtimeEffort,
                                    runtimeControls.model,
                                );
                            }
                            setSelectedModelId(modelId);
                            if (onProfileChange) {
                                onProfileChange(modelId);
                            } else {
                                updatePersona({ id: persona.id, modelId }).then(() => refreshPersonas()).catch(() => {});
                            }
                        } catch (err) {
                            console.error('Failed to switch model:', err);
                        }
                    },
                    description: modelChangeNeedsRestart ? CODER_MODEL_RESTART_DESCRIPTION : undefined,
                    onClose: () => {
                        if (profileModalIdRef.current) {
                            Modal.hide(profileModalIdRef.current);
                            profileModalIdRef.current = null;
                        }
                    },
                },
            });
        };

        if (!machineId) {
            showPicker(models);
            return;
        }

        machineListModels(machineId, runtimeVendor)
            .then((result) => {
                const nextModels = result.models || [];
                setModels(nextModels);
                setDefaultModelId(result.defaultModelId || 'default');
                frontendLog(`[PersonaModelPress.reload] ${JSON.stringify({
                    machineId,
                    runtimeVendor,
                    personaId: persona.id,
                    chatSessionId: persona.chatSessionId,
                    count: nextModels.length,
                    defaultModelId: result.defaultModelId || 'default',
                    ids: nextModels.slice(0, 20).map((model) => ({
                        id: model.id,
                        chatModel: model.chat?.model,
                        isProxy: model.isProxy === true,
                        sourceType: model.sourceType ?? null,
                        vendor: model.vendor ?? null,
                    })),
                })}`);
                showPicker(nextModels);
            })
            .catch(() => {
                showPicker(models);
            });
    }, [models, currentModelId, persona.chatSessionId, persona.id, machineId, updatePersona, refreshPersonas, onProfileChange, runtimeControls.model, modelChangeNeedsRestart, runtimeEffort, runtimeVendor]);

    const resolvedSessionPermissionMode = React.useMemo(() => {
        const sessionPermissionMode = session?.permissionMode || session?.metadata?.permissionMode;
        return supportedPermissionModes.includes(sessionPermissionMode as PermissionMode)
            ? (sessionPermissionMode as PermissionMode)
            : null;
    }, [session?.permissionMode, session?.metadata?.permissionMode, supportedPermissionModes]);

    const [permissionMode, setPermissionMode] = React.useState<PermissionMode>(() => {
        if (resolvedSessionPermissionMode) return resolvedSessionPermissionMode;
        return 'default';
    });

    React.useEffect(() => {
        if (resolvedSessionPermissionMode) {
            setPermissionMode(resolvedSessionPermissionMode);
            return;
        }
        setPermissionMode('default');
    }, [resolvedSessionPermissionMode]);

    const handlePermissionModeChange = React.useCallback(async (mode: PermissionMode) => {
        if (!showPermissionModeSelector) return;
        const result = await sessionApplyPermissionModeChange(
            persona.chatSessionId,
            mode,
            runtimeControls.permissionMode,
        );
        if (result.outcome === 'applied') {
            setPermissionMode(mode);
            return;
        }
        Modal.alert('Permission Mode', result.message);
    }, [showPermissionModeSelector, persona.chatSessionId, runtimeControls.permissionMode]);

    // Sandbox policy — default to workspace_write
    const [sandboxPolicy, setSandboxPolicy] = React.useState<'workspace_write' | 'unrestricted'>(
        (session?.sandboxPolicy as any) || 'workspace_write'
    );

    const handleSandboxPolicyChange = React.useCallback((policy: 'workspace_write' | 'unrestricted') => {
        setSandboxPolicy(policy);
        storage.getState().updateSessionSandboxPolicy(persona.chatSessionId, policy);
        sessionSetSandboxPolicy(persona.chatSessionId, policy);
    }, [persona.chatSessionId]);

    // Trigger session sync on mount + mark as read
    React.useLayoutEffect(() => {
        sync.onSessionVisible(persona.chatSessionId);
        storage.getState().markPersonaRead(persona.chatSessionId);
    }, [persona.chatSessionId]);

    // Keep marking as read while viewing (handles new messages arriving)
    React.useEffect(() => {
        if (messages.length > 0) {
            storage.getState().markPersonaRead(persona.chatSessionId);
        }
    }, [messages.length, persona.chatSessionId]);

    // Always call hook (React rules); use fallback when session is null
    const sessionStatus = useSessionStatus(session ?? FALLBACK_SESSION);
    const hasSession = session != null;

    // Memoize connectionStatus to avoid creating new object references on every render
    const connectionStatus = React.useMemo(() => {
        if (!hasSession) return undefined;
        return {
            text: sessionStatus.statusText,
            color: sessionStatus.statusColor,
            dotColor: sessionStatus.statusDotColor,
            isPulsing: sessionStatus.isPulsing,
            compressionInfo: sessionStatus.compressionInfo ? {
                text: sessionStatus.compressionInfo.text,
                color: sessionStatus.compressionInfo.color,
                percentage: sessionStatus.compressionInfo.percentage,
            } : undefined,
        };
    }, [hasSession, sessionStatus.statusText, sessionStatus.statusColor, sessionStatus.statusDotColor, sessionStatus.isPulsing, sessionStatus.compressionInfo?.text, sessionStatus.compressionInfo?.color, sessionStatus.compressionInfo?.percentage]);

    const [mcpServers, setMcpServers] = React.useState<MCPServerItem[]>([]);
    const [activeMcpIds, setActiveMcpIds] = React.useState<string[]>([]);

    React.useEffect(() => {
        if (!machineId) return;
        sessionGetMCPServers(persona.chatSessionId, machineId).then((result) => {
            setMcpServers(result.allServers);
            setActiveMcpIds(result.activeServerIds);
            if (result.activeServerIds.length > 0) {
                sessionSetMCPServers(persona.chatSessionId, result.activeServerIds);
            }
        }).catch((err) => console.warn('Failed to load persona session MCP servers:', err));
    }, [persona.chatSessionId, machineId]);

    const mcpModalIdRef = React.useRef<string | null>(null);
    const handleMcpClick = React.useCallback(() => {
        if (mcpServers.length === 0) {
            Modal.alert(t('mcp.title'), t('mcp.noServers'));
            return;
        }
        const handleClose = () => {
            if (mcpModalIdRef.current) {
                Modal.hide(mcpModalIdRef.current);
                mcpModalIdRef.current = null;
            }
        };
        mcpModalIdRef.current = Modal.show({
            component: MCPSelectorModal,
            props: {
                servers: mcpServers,
                activeServerIds: activeMcpIds,
                onSelectionChange: (newIds: string[]) => {
                    setActiveMcpIds(newIds);
                    sessionSetMCPServers(persona.chatSessionId, newIds);
                },
                onClose: handleClose,
            },
        });
    }, [persona.chatSessionId, mcpServers, activeMcpIds]);

    // ── Background tasks infrastructure (runs + scheduled tasks + dispatch task sessions) ──

    // Background runs (shell commands etc.) — poll every 5s
    const [runs, setRuns] = React.useState<RunRecord[]>([]);
    React.useEffect(() => {
        if (!machineId) return;
        let mounted = true;
        const loadRuns = async () => {
            try {
                const result = await machineListRuns(machineId, persona.chatSessionId);
                if (mounted) setRuns(result);
            } catch (err) { /* ignore */ }
        };
        loadRuns();
        const interval = setInterval(loadRuns, 5000);
        return () => { mounted = false; clearInterval(interval); };
    }, [machineId, persona.chatSessionId]);

    // Scheduled tasks — filter to this persona only
    const { tasks: allScheduledTasks, toggleTask: toggleScheduledTask, deleteTask: deleteScheduledTask, updateTask: updateScheduledTask } = useScheduledTasks({ machineId });
    const scheduledTasks = React.useMemo(() => allScheduledTasks.filter(t => t.persona_id === persona.id), [allScheduledTasks, persona.id]);

    // Dispatch task sessions — poll every 10s
    const sessions = useAllSessions();
    const [taskSummaries, setTaskSummaries] = React.useState<TaskSessionItem[]>([]);
    React.useEffect(() => {
        if (!machineId) return;
        let mounted = true;
        const fetchTasks = async () => {
            try {
                const summaries = await machineGetPersonaTasks(machineId, persona.id);
                if (mounted) {
                    setTaskSummaries(summaries.map(ts => {
                        const sess = sessions.find(s => s.id === ts.sessionId);
                        return { ...ts, session: sess ? { thinking: sess.thinking, presence: sess.presence as string, metadata: sess.metadata } : undefined };
                    }));
                }
            } catch (err) { /* ignore */ }
        };
        fetchTasks();
        const interval = setInterval(fetchTasks, 10_000);
        return () => { mounted = false; clearInterval(interval); };
    }, [machineId, persona.id, sessions]);

    const taskLifecycle = useSessionTaskLifecycle(effectiveSessionId);
    const { tasks: canonicalAgentTasks } = useBackgroundTasks(
        effectiveSessionId ? (machineId || '') : '',
        effectiveSessionId || undefined,
    );
    const agentTasks = React.useMemo(
        () => deriveAgentBackgroundTasks(messages, effectiveSessionId, session?.metadata, taskLifecycle, canonicalAgentTasks),
        [canonicalAgentTasks, messages, effectiveSessionId, session?.metadata, taskLifecycle],
    );
    const activeAgentBackgroundTaskCount = React.useMemo(
        () => canonicalAgentTasks.length > 0
            ? canonicalAgentTasks.filter((task) => task.status === 'running').length
            : countActiveAgentBackgroundTasks(agentTasks),
        [agentTasks, canonicalAgentTasks],
    );

    const backgroundTaskCount = runs.length + scheduledTasks.length + taskSummaries.length + activeAgentBackgroundTaskCount;

    // Background runs modal handler
    const runsModalIdRef = React.useRef<string | null>(null);
    const closeRunsModal = React.useCallback(() => {
        if (runsModalIdRef.current) {
            Modal.hide(runsModalIdRef.current);
            runsModalIdRef.current = null;
        }
    }, []);

    const handleRunsClick = React.useCallback(async () => {
        if (!machineId) return;

        // Refresh runs before showing modal
        try {
            const latestRuns = await machineListRuns(machineId, persona.chatSessionId);
            setRuns(latestRuns);
        } catch (err) { /* ignore */ }

        closeRunsModal();
        runsModalIdRef.current = Modal.show({
            component: BackgroundRunsModal,
            props: {
                machineId,
                sessionId: persona.chatSessionId,
                runs,
                scheduledTasks,
                taskSessions: taskSummaries,
                agentTasks,
                onToggleScheduledTask: toggleScheduledTask,
                onViewScheduledTaskDetail: (task: ScheduledTask) => {
                    closeRunsModal();
                    setSelectedScheduledTask(task);
                },
                onViewTaskSession: (sid: string) => {
                    closeRunsModal();
                    router.push(`/session/${sid}`);
                },
                onViewAgentTask: (task: AgentBackgroundTaskItem) => {
                    closeRunsModal();
                    router.push(`/session/${task.sessionId}/message/${task.messageId}`);
                },
                onRefresh: async () => {
                    if (!machineId) return;
                    try {
                        const latestRuns = await machineListRuns(machineId, persona.chatSessionId);
                        setRuns(latestRuns);
                    } catch (err) { /* ignore */ }
                },
                onClose: closeRunsModal,
            },
        });
    }, [agentTasks, machineId, persona.chatSessionId, persona.id, runs, scheduledTasks, taskSummaries, toggleScheduledTask, closeRunsModal, router]);

    // Scheduled task detail
    const [selectedScheduledTask, setSelectedScheduledTask] = React.useState<ScheduledTask | null>(null);

    // Agent list modal
    const [availableAgents, setAvailableAgents] = React.useState<AgentConfig[]>([]);
    React.useEffect(() => {
        if (!machineId) return;
        let cancelled = false;
        console.log('[AgentList] Fetching agents for machine:', machineId, 'workdir:', persona.workdir);
        machineListAgents(machineId, persona.workdir).then((agents) => {
            console.log('[AgentList] Received', agents.length, 'agents:', agents.map(a => a.id));
            if (!cancelled) setAvailableAgents(agents);
        }).catch((err) => {
            console.warn('[AgentList] Failed to fetch agents:', err);
        });
        return () => { cancelled = true; };
    }, [machineId, persona.workdir]);

    // Skill list + selection
    const [availableSkills, setAvailableSkills] = React.useState<SkillListItem[]>([]);
    const [selectedSkills, setSelectedSkills] = React.useState<SkillListItem[]>([]);
    React.useEffect(() => {
        if (!machineId) return;
        let cancelled = false;
        machineListSkills(machineId).then((result) => {
            if (!cancelled) setAvailableSkills(result.skills || []);
        }).catch(() => {});
        return () => { cancelled = true; };
    }, [machineId]);

    const handleSkillSelect = React.useCallback((skill: SkillListItem) => {
        setSelectedSkills(prev => prev.some(s => s.id === skill.id) ? prev : [...prev, skill]);
    }, []);

    const handleRemoveSkill = React.useCallback((id: string) => {
        setSelectedSkills(prev => prev.filter(s => s.id !== id));
    }, []);

    const skillModalIdRef = React.useRef<string | null>(null);
    const handleSkillClick = React.useCallback(() => {
        const handleClose = () => {
            if (skillModalIdRef.current) {
                Modal.hide(skillModalIdRef.current);
                skillModalIdRef.current = null;
            }
        };
        skillModalIdRef.current = Modal.show({
            component: SkillListModal,
            props: {
                skills: availableSkills,
                onSelect: handleSkillSelect,
                onClose: handleClose,
            },
        });
        // Refresh in background
        if (machineId) {
            machineListSkills(machineId).then((result) => {
                if (result.skills?.length) setAvailableSkills(result.skills);
            }).catch(() => {});
        }
    }, [machineId, availableSkills, handleSkillSelect]);

    const agentModalIdRef = React.useRef<string | null>(null);
    const handleAgentClick = React.useCallback(() => {
        const handleClose = () => {
            if (agentModalIdRef.current) {
                Modal.hide(agentModalIdRef.current);
                agentModalIdRef.current = null;
            }
        };
        agentModalIdRef.current = Modal.show({
            component: AgentListModal,
            props: {
                agents: availableAgents,
                onClose: handleClose,
            },
        });
        // Refresh in background
        if (machineId) {
            machineListAgents(machineId, persona.workdir).then((agents) => {
                if (agents.length) setAvailableAgents(agents);
            }).catch(() => {});
        }
    }, [machineId, persona.workdir, availableAgents]);

    const handleAddImage = useCallback((img: PickedImage) => {
        setPendingImages(prev => [...prev, img]);
    }, []);

    const handleRemoveImage = useCallback((idx: number) => {
        setPendingImages(prev => prev.filter((_, i) => i !== idx));
    }, []);

    // Drag-and-drop support (web only)
    const [isDragOver, setIsDragOver] = React.useState(false);
    const handleAddImageRef = React.useRef(handleAddImage);
    const messageRef = React.useRef(message);
    handleAddImageRef.current = handleAddImage;
    messageRef.current = message;

    React.useEffect(() => {
        if (Platform.OS !== 'web') return;

        const IMAGE_EXTS = /\.(jpe?g|png|gif|webp|bmp)$/i;
        const EXT_TO_MIME: Record<string, string> = {
            '.jpg': 'image/jpeg', '.jpeg': 'image/jpeg', '.png': 'image/png',
            '.gif': 'image/gif', '.webp': 'image/webp', '.bmp': 'image/bmp',
        };
        const MAX_BASE64_SIZE = 5 * 1024 * 1024;
        let dragCounter = 0;

        // Prevent browser default (opening file)
        const onDragOver = (e: DragEvent) => {
            e.preventDefault();
            if (e.dataTransfer) e.dataTransfer.dropEffect = 'copy';
        };
        const onDragEnter = (e: DragEvent) => {
            e.preventDefault();
            dragCounter++;
            if (dragCounter === 1) setIsDragOver(true);
        };
        const onDragLeave = () => {
            dragCounter--;
            if (dragCounter <= 0) {
                dragCounter = 0;
                setIsDragOver(false);
            }
        };
        const onDrop = (e: DragEvent) => {
            e.preventDefault();
            e.stopPropagation();
            dragCounter = 0;
            setIsDragOver(false);
            // File processing handled by Tauri event below;
            // HTML5 fallback for non-Tauri web (images only via FileReader)
        };

        document.addEventListener('dragover', onDragOver);
        document.addEventListener('dragenter', onDragEnter);
        document.addEventListener('dragleave', onDragLeave);
        document.addEventListener('drop', onDrop);

        // Tauri: use native drag events for overlay + file processing
        let tauriUnlisteners: (() => void)[] = [];
        (async () => {
            try {
                const { listen } = await import('@tauri-apps/api/event');
                const { invoke } = await import('@tauri-apps/api/core');

                const u1 = await listen('tauri://drag-enter', () => setIsDragOver(true));
                const u2 = await listen('tauri://drag-leave', () => setIsDragOver(false));
                const u3 = await listen<{ paths: string[] }>('tauri://drag-drop', async (event) => {
                    setIsDragOver(false);
                    const paths = event.payload.paths;
                    const nonImagePaths: string[] = [];

                    for (const filePath of paths) {
                        const ext = filePath.substring(filePath.lastIndexOf('.')).toLowerCase();
                        if (IMAGE_EXTS.test(filePath)) {
                            try {
                                const base64: string = await invoke('read_file_base64', { path: filePath });
                                if (base64.length > MAX_BASE64_SIZE) continue;
                                const mediaType = EXT_TO_MIME[ext] || 'image/jpeg';
                                const uri = `data:${mediaType};base64,${base64}`;
                                handleAddImageRef.current({ uri, media_type: mediaType, data: base64 });
                            } catch (err) {
                                console.error('Failed to read image:', filePath, err);
                            }
                        } else {
                            nonImagePaths.push(filePath);
                        }
                    }

                    if (nonImagePaths.length > 0) {
                        const pathsText = nonImagePaths.join(' ');
                        const current = messageRef.current;
                        setMessage(current + (current ? ' ' : '') + pathsText);
                    }
                });
                tauriUnlisteners = [u1, u2, u3];
            } catch {
                // Not in Tauri — fallback: use HTML5 drop for images
                document.removeEventListener('drop', onDrop);
                const onDropFallback = (e: DragEvent) => {
                    e.preventDefault();
                    e.stopPropagation();
                    dragCounter = 0;
                    setIsDragOver(false);
                    const files = e.dataTransfer?.files;
                    if (!files) return;
                    for (const file of Array.from(files)) {
                        if (!file.type.startsWith('image/') || file.size > 20 * 1024 * 1024) continue;
                        const reader = new FileReader();
                        reader.onload = () => {
                            const dataUrl = reader.result as string;
                            const base64 = dataUrl.split(',')[1];
                            if (!base64 || base64.length > MAX_BASE64_SIZE) return;
                            const uri = URL.createObjectURL(file);
                            handleAddImageRef.current({ uri, media_type: file.type, data: base64 });
                        };
                        reader.readAsDataURL(file);
                    }
                };
                document.addEventListener('drop', onDropFallback);
                // Store for cleanup
                (onDrop as any).__fallback = onDropFallback;
            }
        })();

        return () => {
            document.removeEventListener('dragover', onDragOver);
            document.removeEventListener('dragenter', onDragEnter);
            document.removeEventListener('dragleave', onDragLeave);
            document.removeEventListener('drop', onDrop);
            if ((onDrop as any).__fallback) {
                document.removeEventListener('drop', (onDrop as any).__fallback);
            }
            tauriUnlisteners.forEach(u => u());
        };
    }, []);

    const applyWorkspaceTurnRuntime = React.useCallback((
        result: {
            state?: WorkspaceRuntimeSummary['state'];
            events?: WorkspaceEvent[];
            templateState?: unknown;
        },
    ) => {
        setWorkspaceRuntimeOverride((previousRuntime) => {
            const templateState =
                normalizeWorkspaceTemplateState(result.templateState) ?? previousRuntime?.templateState;
            if (!result.state) {
                if (templateState === previousRuntime?.templateState) return previousRuntime ?? null;
                return previousRuntime ? { ...previousRuntime, templateState } : null;
            }

            return {
                state: result.state,
                recentActivities: result.state.activities.slice(-24),
                recentEvents: result.events || [],
                templateState,
            };
        });
    }, []);

    const handleGatedTaskControl = React.useCallback(async (command: 'pause' | 'skip') => {
        if (!machineId || !effectiveWorkspace || isGatedTaskControlPending) return;
        setIsGatedTaskControlPending(true);
        setWorkspaceRouteStatus(command === 'pause' ? 'Pausing gated task…' : 'Skipping gated task…');
        try {
            const result = await machineWorkspaceSendMessage(machineId, {
                personaId: persona.id,
                message: command,
            });
            if (!result.success) {
                throw new Error(result.error || `Failed to ${command} gated task`);
            }
            applyWorkspaceTurnRuntime({
                state: result.state,
                events: result.events,
                templateState: (result as { templateState?: unknown }).templateState,
            });
            setWorkspaceRouteStatus(command === 'pause' ? 'Gated task paused.' : 'Gated task skipped.');
            await onWorkspaceRefresh?.();
        } catch (error) {
            console.error(`Failed to ${command} gated task:`, error);
            setWorkspaceRouteStatus(command === 'pause' ? 'Pause failed.' : 'Skip failed.');
        } finally {
            setIsGatedTaskControlPending(false);
        }
    }, [applyWorkspaceTurnRuntime, effectiveWorkspace, isGatedTaskControlPending, machineId, onWorkspaceRefresh, persona.id]);

    const handleReviewerAction = React.useCallback((decision: 'approve' | 'reject') => {
        if (!gatedTasksTemplateState) return;
        const reviewerRoleId = gatedTasksTemplateState.reviewerRoleId || 'reviewer';
        const reviewerSessionId = effectiveWorkspace?.members.find((member) => member.roleId === reviewerRoleId)?.sessionId;
        setWorkspaceRouteStatus(
            decision === 'approve'
                ? `Open @${reviewerRoleId} and reply with APPROVED: ... to move into commit.`
                : `Open @${reviewerRoleId} and reply with REJECTED: ... to send feedback back to coding.`
        );
        if (reviewerSessionId) {
            router.push(`/session/${reviewerSessionId}`);
        }
    }, [effectiveWorkspace?.members, gatedTasksTemplateState, router]);

    const handleSend = useCallback(async () => {
        const hasText = message.trim().length > 0;
        const hasImages = pendingImages.length > 0;
        const hasSkills = selectedSkills.length > 0;
        if (hasText || hasImages || hasSkills) {
            const userText = message;
            const images = hasImages ? pendingImages.map(img => ({ media_type: img.media_type, data: img.data })) : undefined;

            const roleDispatch = !hasImages && !hasSkills
                ? parseWorkspaceRoleMention(userText, effectiveWorkspace)
                : null;
            const workspacePlainTextTurn = !!effectiveWorkspace && !hasImages && !hasSkills && !roleDispatch && userText.trim().length > 0;

            if ((roleDispatch || workspacePlainTextTurn) && machineId) {
                setMessage('');
                setWorkspaceRouteStatus(
                    roleDispatch
                        ? `已派发给 @${roleDispatch.roleId}`
                        : '已广播到工作间，等待成员认领…'
                );
                try {
                    const result = await machineWorkspaceSendMessage(machineId, {
                        personaId: persona.id,
                        roleId: roleDispatch?.roleId,
                        message: roleDispatch ? roleDispatch.instruction : userText.trim(),
                    });
                    if (!result.success) {
                        throw new Error(result.error || 'Failed to route workspace message');
                    }
                    applyWorkspaceTurnRuntime({
                        state: result.state,
                        events: result.events,
                        templateState: (result as { templateState?: unknown }).templateState,
                    });
                    const routeStatus = summarizeWorkspaceTurnResult(result);
                    if (routeStatus) {
                        setWorkspaceRouteStatus(routeStatus);
                    } else if (workspacePlainTextTurn) {
                        setWorkspaceRouteStatus('工作间已收到消息');
                    }
                    await onWorkspaceRefresh?.();
                } catch (error) {
                    console.error('Failed to route role message:', error);
                    setMessage(userText);
                    setWorkspaceRouteStatus(
                        roleDispatch
                            ? `派发 @${roleDispatch.roleId} 失败`
                            : '发送到工作间失败'
                    );
                }
                return;
            }

            setWorkspaceRouteStatus(null);
            setMessage('');
            setPendingImages([]);
            setSelectedSkills([]);

            // Build text with skill injection
            let text: string;
            let displayText: string | undefined;
            if (hasSkills) {
                const skillBlocks = selectedSkills.map(s =>
                    `<activated_skill id="${s.id}" name="${s.name}">\n  <description>\n    ${s.description}\n  </description>\n\n  <instructions>\n${s.instructions || s.description}\n  </instructions>\n</activated_skill>`
                ).join('\n\n');
                text = skillBlocks + (userText ? '\n\n' + userText : '');
                const tags = selectedSkills.map(s => `@${s.name}`).join(' ');
                displayText = (tags + (userText ? ' ' + userText : '')).trim();
            } else {
                text = userText || ' ';
            }

            let targetSessionId = persona.chatSessionId;
            const isPendingSession = persona.chatSessionId.startsWith('pending-');

            // Only block on reconnect for pending sessions that need real session creation.
            // For normal sessions, sync.sendMessage already performs reconnect internally,
            // and doing it here delays optimistic message rendering.
            if (machineId && isPendingSession) {
                try {
                    const result = await machineReconnectSession(machineId, persona.chatSessionId);
                    if (result.status === 'session_replaced' && result.newSessionId) {
                        targetSessionId = result.newSessionId;
                        console.log(`[Persona] Session replaced: ${persona.chatSessionId} → ${targetSessionId}`);
                        refreshPersonas();
                    }
                } catch (e) {
                    console.warn('reconnect-session failed, sending anyway:', e);
                }
            }
            sync.sendMessage(targetSessionId, text, displayText, images);
        }
    }, [message, pendingImages, selectedSkills, effectiveWorkspace, persona.id, persona.chatSessionId, machineId, onWorkspaceRefresh, refreshPersonas]);

    const handleWorkspaceRolePress = React.useCallback((roleId: string) => {
        setWorkspaceRouteStatus(null);
        setMessage((prev) => {
            const trimmed = prev.trim();
            if (trimmed.startsWith(`@${roleId} `) || trimmed === `@${roleId}`) {
                return prev;
            }
            return `@${roleId} ${prev}`.trimEnd();
        });
    }, []);

    const handleWorkspaceMemberOpen = React.useCallback((sessionId: string) => {
        router.push(`/session/${sessionId}`);
    }, [router]);

    const highlightedWorkspaceRoleId = React.useMemo(
        () => extractFocusedWorkspaceRole(message, effectiveWorkspace),
        [message, effectiveWorkspace],
    );

    const handleAbort = useCallback(() => {
        sessionAbort(persona.chatSessionId);
    }, [persona.chatSessionId]);

    // Speech-to-text (kept aligned with the session-mode chat flow)
    const accumulatedTextRef = React.useRef('');
    const handleMicrophonePress = useCallback(async () => {
        if (realtimeStatus === 'connecting') return;
        if (realtimeStatus === 'disconnected' || realtimeStatus === 'error') {
            try {
                accumulatedTextRef.current = message;
                await startSpeechToText((text: string, isFinal: boolean) => {
                    if (isFinal) {
                        accumulatedTextRef.current = accumulatedTextRef.current
                            ? accumulatedTextRef.current + text
                            : text;
                        setMessage(accumulatedTextRef.current);
                    } else {
                        const preview = accumulatedTextRef.current
                            ? accumulatedTextRef.current + text
                            : text;
                        setMessage(preview);
                    }
                });
            } catch (error) {
                console.error('Failed to start speech-to-text:', error);
                Modal.alert(t('common.error'), t('errors.voiceSessionFailed'));
            }
        } else if (realtimeStatus === 'connected') {
            await stopSpeechToText();
        }
    }, [realtimeStatus, message]);

    const micButtonState = useMemo(() => ({
        onMicPress: handleMicrophonePress,
        isMicActive: realtimeStatus === 'connected' || realtimeStatus === 'connecting',
    }), [handleMicrophonePress, realtimeStatus]);

    // Clear — create a new persona with the same workdir, old one stays offline
    // Can be overridden via onClearProp (e.g. for hypothesis design sessions)
    const [isClearLoading, setIsClearLoading] = React.useState(false);
    const handleClear = useCallback(async () => {
        if (!machineId || isClearLoading) return;
        setIsClearLoading(true);
        try {
            if (onClearProp) {
                await onClearProp();
            } else {
                const newPersona = await createPersona({ workdir: persona.workdir });
                if (newPersona?.id) {
                    router.replace(`/persona/${newPersona.id}` as any);
                }
            }
        } catch (err) {
            console.error('Failed to clear:', err);
        } finally {
            setIsClearLoading(false);
        }
    }, [machineId, persona.workdir, isClearLoading, createPersona, router, onClearProp]);

    // Header
    const header = renderHeader ? renderHeader(session ?? null) : (
        <View style={{
            position: 'absolute',
            top: 0,
            left: 0,
            right: 0,
            zIndex: 1000,
        }}>
            <ChatHeaderView
                title={persona.name}
                subtitle={persona.description}
                avatarId={persona.avatarId === 'default' && persona.agent ? getVendorAvatarId(persona.agent) : persona.avatarId}
                flavor={session ? inferSessionVendor(session) : null}
                onBackPress={onBack}
                onHomePress={() => router.replace('/')}
                onAvatarPress={() => router.push(`/session/${persona.chatSessionId}/info`)}
                onContinuousBrowsingToggle={hideHeaderActions ? undefined : handleToggleContinuousBrowsing}
                continuousBrowsing={hideHeaderActions ? undefined : continuousBrowsing}
                onMemoryPress={hideHeaderActions ? undefined : (() => setShowMemory(true))}
                onWorkspacePress={hideHeaderActions ? undefined : (() => setShowWorkspaceBrowser(true))}
                isConnected={session?.presence === 'online'}
            />
        </View>
    );

    const hasCoordinatorConversation = !effectiveWorkspace && session && messages.length > 0
        ? true
        : !!(effectiveWorkspace && session && messages.length > 0);

    // Content: workspace keeps a single shared chat feed as the main stage.
    const content = (
        <View style={{ flex: 1 }}>
            {effectiveWorkspace ? (
                <WorkspaceChatFeed workspace={effectiveWorkspace} />
            ) : hasCoordinatorConversation ? (
                <View style={{ flex: 1 }}>
                    <Deferred>
                        {session && messages.length > 0 && (
                            <ChatList session={session} />
                        )}
                    </Deferred>
                </View>
            ) : null}
        </View>
    );

    const placeholder = (!session || messages.length === 0) ? (
        <>
            {isPendingSession ? (
                <View style={{ alignItems: 'center', gap: 8 }}>
                    <ActivityIndicator size="small" color={theme.colors.textSecondary} />
                    <Text style={{ fontSize: 13, color: theme.colors.textSecondary, ...Typography.default() }}>
                        {t('persona.connecting')}
                    </Text>
                </View>
            ) : effectiveWorkspace ? null : isLoaded || !session ? (
                <PersonaEmptyState
                    name={persona.name}
                    description={persona.description}
                    avatarId={persona.avatarId === 'default' && persona.agent ? getVendorAvatarId(persona.agent) : persona.avatarId}
                />
            ) : (
                <ActivityIndicator size="small" color={theme.colors.textSecondary} />
            )}
        </>
    ) : null;

    const modelButtonLabel = showModelPicker && modelChangeNeedsRestart
        ? `${currentModelName} · ${CODER_MODEL_RESTART_HINT}`
        : currentModelName;

    // Input
    const input = (
        <View>
            <PersonaChatInput
                placeholder={effectiveWorkspace ? '输入 @pm、@prd、@coder ... 然后描述任务' : t('persona.inputPlaceholder', { name: persona.name })}
                value={message}
                onChangeText={setMessage}
                onSend={handleSend}
                autocompleteOptions={effectiveWorkspace?.members
                    .map((member) => member.roleId)
                    .filter((roleId): roleId is string => !!roleId)
                    .map((roleId) => ({
                        id: roleId,
                        description: `派发给 ${roleId}`,
                    }))}
                onMicPress={micButtonState.onMicPress}
                isMicActive={micButtonState.isMicActive}
                onAbort={handleAbort}
                showAbortButton={hasSession && sessionStatus.state === 'thinking'}
                connectionStatus={connectionStatus}
                agentCount={availableAgents.length}
                onAgentClick={hideHeaderActions ? undefined : handleAgentClick}
                activeSkillCount={availableSkills.length}
                onSkillClick={hideHeaderActions ? undefined : handleSkillClick}
                selectedSkills={selectedSkills}
                onRemoveSkill={handleRemoveSkill}
                onClear={handleClear}
                isClearLoading={isClearLoading}
                supportsVision={supportsVision}
                pendingImages={pendingImages}
                onAddImage={handleAddImage}
                onRemoveImage={handleRemoveImage}
                modelName={showModelPicker ? modelButtonLabel : undefined}
                onModelPress={showModelPicker ? handleModelPress : undefined}
                effortName={showModelPicker ? runtimeEffortLabel(runtimeEffort) : undefined}
                onEffortPress={showModelPicker ? handleEffortPress : undefined}
                usageVendor={runtimeVendor === 'claude' || runtimeVendor === 'codex' || runtimeVendor === 'gemini' ? runtimeVendor : null}
                usageMachineId={machineId}
            />
            {/* Toolbar below input: permission mode + background runs */}
            <PersonaToolbar
                vendor={runtimeVendor}
                permissionMode={showPermissionModeSelector ? permissionMode : undefined}
                onPermissionModeChange={showPermissionModeSelector ? handlePermissionModeChange : undefined}
                sandboxPolicy={sandboxPolicy}
                onSandboxPolicyChange={handleSandboxPolicyChange}
                activeMcpCount={mcpServers.length}
                onMcpClick={handleMcpClick}
                activeRunCount={backgroundTaskCount}
                onRunsClick={handleRunsClick}
            />
        </View>
    );

    return (
        <>
            {header}

            <View style={{
                flex: 1,
                paddingTop: safeArea.top + headerHeight,
                paddingBottom: safeArea.bottom + ((isRunningOnMac() || Platform.OS === 'web') ? 32 : 0),
            }}>
                {/* {effectiveWorkspace && (
                    <WorkspaceBanner
                        workspace={effectiveWorkspace}
                        onDelete={handleDeleteWorkspace}
                        deleting={isDeletingWorkspace}
                    />
                )} */}
                {workspaceRouteStatus && (
                    <View style={{ paddingHorizontal: 20, paddingBottom: 6 }}>
                        <Text style={{ fontSize: 12, color: theme.colors.textSecondary, ...Typography.default() }}>
                            {workspaceRouteStatus}
                        </Text>
                    </View>
                )}
                {effectiveWorkspace && isGatedTasksTemplate(effectiveWorkspace.binding.templateId) && (
                    <GatedTasksPanel
                        templateState={gatedTasksTemplateState ?? { type: 'gated_tasks' }}
                        onApprove={() => handleReviewerAction('approve')}
                        onReject={() => handleReviewerAction('reject')}
                        onPause={() => handleGatedTaskControl('pause')}
                        onSkip={() => handleGatedTaskControl('skip')}
                        controlsDisabled={isGatedTaskControlPending}
                    />
                )}
                {effectiveWorkspace && autoresearchTemplateState && (
                    <AutoresearchPanel templateState={autoresearchTemplateState} />
                )}
                {effectiveWorkspace && (
                    <WorkspaceActivityStrip
                        workspace={effectiveWorkspace}
                        onRolePress={handleWorkspaceRolePress}
                        onOpenSession={handleWorkspaceMemberOpen}
                        highlightedRoleId={highlightedWorkspaceRoleId}
                    />
                )}
                <AgentContentView
                    content={content}
                    input={input}
                    placeholder={placeholder}
                />
                {/* Drag-and-drop overlay */}
                {isDragOver && (
                    <View style={{
                        position: 'absolute',
                        top: 0, left: 0, right: 0, bottom: 0,
                        backgroundColor: 'rgba(0,0,0,0.35)',
                        zIndex: 100,
                        alignItems: 'center',
                        justifyContent: 'center',
                    }}>
                        <View style={{
                            width: 72,
                            height: 72,
                            borderRadius: 36,
                            backgroundColor: theme.colors.button.primary.background,
                            alignItems: 'center',
                            justifyContent: 'center',
                        }}>
                            <Ionicons name="add" size={40} color="#fff" />
                        </View>
                        <Text style={{ color: '#fff', fontSize: 15, marginTop: 12 }}>
                            Drop files here
                        </Text>
                    </View>
                )}
            </View>

            {machineId && (
                <MemoryEditorModal
                    visible={showMemory}
                    onClose={() => setShowMemory(false)}
                    machineId={machineId}
                    ownerId={persona.id}
                />
            )}

            {machineId && (
                <WorkspaceBrowserModal
                    visible={showWorkspaceBrowser}
                    onClose={() => setShowWorkspaceBrowser(false)}
                    machineId={machineId}
                    workspaceRoot={persona.workdir}
                />
            )}

            <ScheduledTaskDetailModal
                task={selectedScheduledTask}
                visible={!!selectedScheduledTask}
                onClose={() => setSelectedScheduledTask(null)}
                onToggle={async (id, enabled) => {
                    await toggleScheduledTask(id, enabled);
                }}
                onDelete={async (id) => {
                    await deleteScheduledTask(id);
                    setSelectedScheduledTask(null);
                }}
                onUpdate={async (id, updates) => {
                    await updateScheduledTask(id, updates);
                }}
            />
        </>
    );
}

export const SessionPersonaPage = React.memo(({ id }: { id: string }) => {
    const sessionId = id;
    const router = useRouter();
    const session = useSession(sessionId);
    const isDataReady = useIsDataReady();
    const { theme } = useUnistyles();
    const safeArea = useSafeAreaInsets();
    const isLandscape = useIsLandscape();
    const deviceType = useDeviceType();
    const headerHeight = useHeaderHeight();
    const realtimeStatus = useRealtimeStatus();
    const isTablet = useIsTablet();
    const [showMemory, setShowMemory] = React.useState(false);
    const [showWorkspaceBrowser, setShowWorkspaceBrowser] = React.useState(false);
    const machineId = session?.metadata?.machineId;

    const headerProps = React.useMemo(() => {
        if (!isDataReady) {
            return {
                title: '',
                subtitle: undefined,
                avatarId: undefined,
                onAvatarPress: undefined,
                isConnected: false,
                flavor: null,
            };
        }

        if (!session) {
            return {
                title: t('errors.sessionDeleted'),
                subtitle: undefined,
                avatarId: undefined,
                onAvatarPress: undefined,
                isConnected: false,
                flavor: null,
            };
        }

        const isConnected = session.presence === 'online';
        return {
            title: getSessionName(session),
            subtitle: session.metadata?.path
                ? formatPathRelativeToHome(session.metadata.path, session.metadata?.homeDir)
                : undefined,
            avatarId: getSessionAvatarId(session),
            onAvatarPress: () => router.push(`/session/${sessionId}/info`),
            isConnected,
            flavor: inferSessionVendor(session),
            tintColor: isConnected ? '#000' : '#8E8E93',
        };
    }, [isDataReady, router, session, sessionId]);

    return (
        <>
            {isLandscape && deviceType === 'phone' && (
                <View style={{
                    position: 'absolute',
                    top: 0,
                    left: 0,
                    right: 0,
                    height: safeArea.top,
                    backgroundColor: theme.colors.surface,
                    zIndex: 1000,
                    shadowColor: theme.colors.shadow.color,
                    shadowOffset: { width: 0, height: 2 },
                    shadowOpacity: theme.colors.shadow.opacity,
                    shadowRadius: 3,
                    elevation: 5,
                }} />
            )}

            {!(isLandscape && deviceType === 'phone' && Platform.OS !== 'web') && (
                <View style={{
                    position: 'absolute',
                    top: 0,
                    left: 0,
                    right: 0,
                    zIndex: 1000,
                }}>
                    <ChatHeaderView
                        {...headerProps}
                        onBackPress={() => router.back()}
                        onMemoryPress={machineId ? (() => setShowMemory(true)) : undefined}
                        onWorkspacePress={machineId ? (() => setShowWorkspaceBrowser(true)) : undefined}
                    />
                    {!isTablet && realtimeStatus !== 'disconnected' && (
                        <VoiceAssistantStatusBar variant="full" />
                    )}
                </View>
            )}

            <View style={{
                flex: 1,
                paddingTop: !(isLandscape && deviceType === 'phone' && Platform.OS !== 'web')
                    ? safeArea.top + headerHeight + (!isTablet && realtimeStatus !== 'disconnected' ? 48 : 0)
                    : 0,
            }}>
                {!isDataReady ? (
                    <View style={{ flex: 1, justifyContent: 'center', alignItems: 'center' }}>
                        <ActivityIndicator size="small" color={theme.colors.textSecondary} />
                    </View>
                ) : !session ? (
                    <View style={{ flex: 1, justifyContent: 'center', alignItems: 'center' }}>
                        <Ionicons name="trash-outline" size={48} color={theme.colors.textSecondary} />
                        <Text style={{ color: theme.colors.text, fontSize: 20, marginTop: 16, fontWeight: '600' }}>
                            {t('errors.sessionDeleted')}
                        </Text>
                        <Text style={{ color: theme.colors.textSecondary, fontSize: 15, marginTop: 8, textAlign: 'center', paddingHorizontal: 32 }}>
                            {t('errors.sessionDeletedDescription')}
                        </Text>
                    </View>
                ) : (
                    <SessionPersonaLoaded key={sessionId} sessionId={sessionId} session={session} />
                )}
            </View>

            {machineId && (
                <MemoryEditorModal
                    visible={showMemory}
                    onClose={() => setShowMemory(false)}
                    machineId={machineId}
                />
            )}

            {machineId && (
                <WorkspaceBrowserModal
                    visible={showWorkspaceBrowser}
                    onClose={() => setShowWorkspaceBrowser(false)}
                    machineId={machineId}
                    workspaceRoot={session?.metadata?.path}
                />
            )}
        </>
    );
});

function SessionPersonaLoaded({ sessionId, session }: { sessionId: string, session: Session }) {
    const { theme } = useUnistyles();
    const router = useRouter();
    const safeArea = useSafeAreaInsets();
    const isLandscape = useIsLandscape();
    const deviceType = useDeviceType();
    const [message, setMessage] = React.useState('');
    const realtimeStatus = useRealtimeStatus();
    const { messages, isLoaded, relayError } = useSessionMessages(sessionId);
    const machines = useAllMachines();
    const acknowledgedCliVersions = useLocalSetting('acknowledgedCliVersions');
    const cliVersion = session.metadata?.version;
    const machineId = session.metadata?.machineId;
    const isCliOutdated = cliVersion && !isVersionSupported(cliVersion, MINIMUM_CLI_VERSION);
    const isAcknowledged = machineId && acknowledgedCliVersions[machineId] === cliVersion;
    const shouldShowCliWarning = isCliOutdated && !isAcknowledged;
    const permissionMode = (session.permissionMode || session.metadata?.permissionMode || 'default') as PermissionMode;
    const sandboxPolicy = session.sandboxPolicy || 'workspace_write';
    const sessionStatus = useSessionStatus(session);
    const sessionUsage = useSessionUsage(sessionId);
    const alwaysShowContextSize = useSetting('alwaysShowContextSize');
    const experiments = useSetting('experiments');
    const runtimeControls = useSessionRuntimeControls(sessionId);
    const ownerMachineName = React.useMemo(() => {
        if (!machineId) {
            return 'Machine';
        }
        const machine = machines.find((item) => item.id === machineId);
        return machine?.metadata?.displayName || machine?.metadata?.host || machineId.slice(0, 8);
    }, [machineId, machines]);

    const isPersonaSession = session.metadata?.flavor === 'persona';
    const cachedPersonas = useCachedPersonas();
    const personaId = React.useMemo(() => {
        if (!isPersonaSession) return undefined;
        const persona = cachedPersonas.find((item) => item.chatSessionId === sessionId);
        return persona?.id;
    }, [cachedPersonas, isPersonaSession, sessionId]);

    const { flow: orchestrationFlow } = useOrchestrationFlow({
        personaId,
        machineId,
    });
    const [showOrchFlow, setShowOrchFlow] = React.useState(false);

    const { clearDraft } = useDraft(sessionId, message, setMessage);
    const handlePromptSuggestionPress = React.useCallback((suggestion: string) => {
        setMessage(suggestion);
        storage.getState().updateSessionDraft(sessionId, suggestion);
    }, [sessionId]);

    const [mcpServers, setMcpServers] = React.useState<MCPServerItem[]>([]);
    const [activeMcpIds, setActiveMcpIds] = React.useState<string[]>([]);
    const [llmModels, setLlmModels] = React.useState<ModelOptionDisplay[]>([]);
    const [llmDefaultModelId, setLlmDefaultModelId] = React.useState<string>('default');
    const sessionModelId = session.metadata?.modelId || 'default';
    const runtimeEffort = session.runtimeEffort || 'default';

    const reloadRuntimeModels = React.useCallback(() => {
        if (!machineId) return Promise.resolve();
        const runtimeVendor = (session.metadata?.vendor as VendorName | undefined) ?? 'cteno';
        return machineListModels(machineId, runtimeVendor).then(result => {
            setLlmModels(result.models || []);
            setLlmDefaultModelId(result.defaultModelId || 'default');
            frontendLog(`[SessionRuntimeModelSource] ${JSON.stringify({
                machineId,
                runtimeVendor,
                sessionId,
                count: (result.models || []).length,
                defaultModelId: result.defaultModelId || 'default',
                ids: (result.models || []).slice(0, 20).map((model) => ({
                    id: model.id,
                    chatModel: model.chat?.model,
                    isProxy: model.isProxy === true,
                    sourceType: model.sourceType ?? null,
                    vendor: model.vendor ?? null,
                })),
            })}`);
        }).catch(() => {});
    }, [machineId, session.metadata?.vendor]);

    React.useEffect(() => {
        reloadRuntimeModels();
    }, [reloadRuntimeModels, sessionId]);

    const [runs, setRuns] = React.useState<RunRecord[]>([]);
    const { subagents, stopSubAgent: handleStopSubAgent } = useSubAgents({
        sessionId,
        machineId: machineId || '',
        activeOnly: false,
    });
    const displayableSubAgents = getDisplayableSubAgents(subagents);
    const [selectedSubAgent, setSelectedSubAgent] = React.useState<typeof subagents[0] | null>(null);
    const [showSubAgentDetail, setShowSubAgentDetail] = React.useState(false);

    const {
        tasks: scheduledTasks,
        toggleTask: toggleScheduledTask,
        deleteTask: deleteScheduledTask,
        updateTask: updateScheduledTask,
    } = useScheduledTasks({ machineId });
    const taskLifecycle = useSessionTaskLifecycle(sessionId);
    const { tasks: canonicalAgentTasks } = useBackgroundTasks(machineId || '', sessionId);
    const agentTasks = React.useMemo(
        () => deriveAgentBackgroundTasks(messages, sessionId, session.metadata, taskLifecycle, canonicalAgentTasks),
        [canonicalAgentTasks, messages, sessionId, session.metadata, taskLifecycle],
    );
    const activeAgentBackgroundTaskCount = React.useMemo(
        () => canonicalAgentTasks.length > 0
            ? canonicalAgentTasks.filter((task) => task.status === 'running').length
            : countActiveAgentBackgroundTasks(agentTasks),
        [agentTasks, canonicalAgentTasks],
    );
    const backgroundTaskCount = runs.length + displayableSubAgents.length + scheduledTasks.length + activeAgentBackgroundTaskCount;
    const [selectedScheduledTask, setSelectedScheduledTask] = React.useState<ScheduledTask | null>(null);
    const [showScheduledTaskDetail, setShowScheduledTaskDetail] = React.useState(false);

    React.useEffect(() => {
        if (!selectedScheduledTask) return;
        const updated = scheduledTasks.find((task) => task.id === selectedScheduledTask.id);
        if (updated) {
            setSelectedScheduledTask(updated);
            return;
        }
        setShowScheduledTaskDetail(false);
        setSelectedScheduledTask(null);
    }, [scheduledTasks, selectedScheduledTask]);

    React.useEffect(() => {
        if (!machineId) return;
        sessionGetMCPServers(sessionId, machineId).then(result => {
            setMcpServers(result.allServers);
            setActiveMcpIds(result.activeServerIds);
            if (result.activeServerIds.length > 0) {
                sessionSetMCPServers(sessionId, result.activeServerIds);
            }
        }).catch(err => console.warn('Failed to load session MCP servers:', err));
    }, [machineId, sessionId]);

    React.useEffect(() => {
        if (!machineId) return;

        const loadRuns = async () => {
            try {
                const result = await machineListRuns(machineId, sessionId);
                setRuns(result);
            } catch (err) {
                console.warn('Failed to load background runs:', err);
            }
        };

        loadRuns();
        const interval = setInterval(loadRuns, 5000);
        return () => clearInterval(interval);
    }, [machineId, sessionId]);

    const mcpModalIdRef = React.useRef<string | null>(null);
    const handleMcpClick = React.useCallback(() => {
        if (mcpServers.length === 0) {
            Modal.alert(t('mcp.title'), t('mcp.noServers'));
            return;
        }

        const handleSelectionChange = (newIds: string[]) => {
            setActiveMcpIds(newIds);
            sessionSetMCPServers(sessionId, newIds);
        };

        const handleClose = () => {
            if (mcpModalIdRef.current) {
                Modal.hide(mcpModalIdRef.current);
                mcpModalIdRef.current = null;
            }
        };

        mcpModalIdRef.current = Modal.show({
            component: MCPSelectorModal,
            props: {
                servers: mcpServers,
                activeServerIds: activeMcpIds,
                onSelectionChange: handleSelectionChange,
                onClose: handleClose,
            },
        });
    }, [activeMcpIds, mcpServers, sessionId]);

    const runsModalIdRef = React.useRef<string | null>(null);
    const closeRunsModal = React.useCallback(() => {
        if (runsModalIdRef.current) {
            Modal.hide(runsModalIdRef.current);
            runsModalIdRef.current = null;
        }
    }, []);

    const showRunsModal = React.useCallback((modalRuns: RunRecord[], modalScheduledTasks: ScheduledTask[]) => {
        if (!machineId) return;

        closeRunsModal();
        runsModalIdRef.current = Modal.show({
            component: BackgroundRunsModal,
            props: {
                machineId,
                sessionId,
                runs: modalRuns,
                subagents: displayableSubAgents,
                scheduledTasks: modalScheduledTasks,
                agentTasks,
                orchestrationFlow,
                onStopSubAgent: handleStopSubAgent,
                onViewSubAgentDetail: (subagent: typeof subagents[0]) => {
                    setSelectedSubAgent(subagent);
                    setShowSubAgentDetail(true);
                },
                onViewScheduledTaskDetail: (task: ScheduledTask) => {
                    setSelectedScheduledTask(task);
                    setShowScheduledTaskDetail(true);
                },
                onViewAgentTask: (task: AgentBackgroundTaskItem) => {
                    closeRunsModal();
                    router.push(`/session/${task.sessionId}/message/${task.messageId}`);
                },
                onToggleScheduledTask: toggleScheduledTask,
                onRefresh: async () => {
                    if (!machineId) return;
                    try {
                        const latestRuns = await machineListRuns(machineId, sessionId);
                        setRuns(latestRuns);
                    } catch (err) {
                        console.warn('Failed to refresh runs:', err);
                    }
                },
                onClose: closeRunsModal,
            },
        });
    }, [agentTasks, closeRunsModal, displayableSubAgents, handleStopSubAgent, machineId, orchestrationFlow, router, sessionId, subagents, toggleScheduledTask]);

    const handleRunsClick = React.useCallback(async () => {
        if (!machineId) return;

        try {
            const latestRuns = await machineListRuns(machineId, sessionId);
            setRuns(latestRuns);
            showRunsModal(latestRuns, scheduledTasks);
        } catch (err) {
            console.warn('Failed to refresh runs:', err);
            showRunsModal(runs, scheduledTasks);
        }
    }, [machineId, runs, scheduledTasks, sessionId, showRunsModal]);

    const handleDismissCliWarning = React.useCallback(() => {
        if (machineId && cliVersion) {
            storage.getState().applyLocalSettings({
                acknowledgedCliVersions: {
                    ...acknowledgedCliVersions,
                    [machineId]: cliVersion,
                },
            });
        }
    }, [acknowledgedCliVersions, cliVersion, machineId]);

    const showRuntimeControlFeedback = React.useCallback((
        label: 'Model' | 'Permission Mode',
        messageText: string,
        outcome: 'applied' | 'restart_required' | 'unsupported' | 'failed',
    ) => {
        const title = outcome === 'applied'
            ? `${label} Applied`
            : outcome === 'restart_required'
                ? `${label} Restart Required`
                : outcome === 'failed'
                    ? `${label} Update Failed`
                    : `${label} Unsupported`;
        Modal.alert(title, messageText);
    }, []);

    const updatePermissionMode = React.useCallback(async (
        mode: 'default' | 'auto' | 'acceptEdits' | 'plan' | 'dontAsk' | 'bypassPermissions' | 'read-only' | 'safe-yolo' | 'yolo',
    ) => {
        const result = await sessionApplyPermissionModeChange(sessionId, mode, runtimeControls.permissionMode);
        showRuntimeControlFeedback('Permission Mode', result.message, result.outcome);
    }, [runtimeControls.permissionMode, sessionId, showRuntimeControlFeedback]);

    const updateSandboxPolicy = React.useCallback((policy: 'workspace_write' | 'unrestricted') => {
        storage.getState().updateSessionSandboxPolicy(sessionId, policy);
        sessionSetSandboxPolicy(sessionId, policy);
    }, [sessionId]);

    const updateRuntimeModel = React.useCallback(async (
        modelId: string,
        nextEffort: import('@/components/EffortSelector').RuntimeEffort = runtimeEffort,
    ) => {
        if (modelId === sessionModelId && nextEffort === runtimeEffort) return;
        const result = await sessionApplyRuntimeModelChange(
            sessionId,
            machineId,
            modelId,
            nextEffort,
            runtimeControls.model,
        );
        showRuntimeControlFeedback('Model', result.message, result.outcome);
    }, [machineId, runtimeControls.model, runtimeEffort, sessionId, sessionModelId, showRuntimeControlFeedback]);

    const accumulatedTextRef = React.useRef('');
    const handleMicrophonePress = React.useCallback(async () => {
        if (realtimeStatus === 'connecting') return;
        if (realtimeStatus === 'disconnected' || realtimeStatus === 'error') {
            try {
                accumulatedTextRef.current = message;
                await startSpeechToText((text: string, isFinal: boolean) => {
                    if (isFinal) {
                        accumulatedTextRef.current = accumulatedTextRef.current
                            ? accumulatedTextRef.current + text
                            : text;
                        setMessage(accumulatedTextRef.current);
                    } else {
                        const preview = accumulatedTextRef.current
                            ? accumulatedTextRef.current + text
                            : text;
                        setMessage(preview);
                    }
                });
                tracking?.capture('voice_session_started', { sessionId });
            } catch (error) {
                console.error('Failed to start speech-to-text:', error);
                Modal.alert(t('common.error'), t('errors.voiceSessionFailed'));
                tracking?.capture('voice_session_error', {
                    error: error instanceof Error ? error.message : 'Unknown error',
                });
            }
        } else if (realtimeStatus === 'connected') {
            await stopSpeechToText();
            tracking?.capture('voice_session_stopped');
        }
    }, [message, realtimeStatus, sessionId]);

    const micButtonState = useMemo(() => ({
        onMicPress: handleMicrophonePress,
        isMicActive: realtimeStatus === 'connected' || realtimeStatus === 'connecting',
    }), [handleMicrophonePress, realtimeStatus]);

    React.useLayoutEffect(() => {
        sync.onSessionVisible(sessionId);
        gitStatusSync.getSync(sessionId);
    }, [realtimeStatus, sessionId]);

    const orchFlowPanel = orchestrationFlow ? (
        <View style={{ borderBottomWidth: 1, borderBottomColor: theme.colors.divider }}>
            <Pressable
                onPress={() => setShowOrchFlow(prev => !prev)}
                style={{
                    flexDirection: 'row',
                    alignItems: 'center',
                    paddingHorizontal: 14,
                    paddingVertical: 8,
                    gap: 8,
                }}
            >
                <Ionicons name="git-network-outline" size={16} color="#007AFF" />
                <Text style={{ fontSize: 13, fontWeight: '600', color: theme.colors.text, flex: 1 }}>
                    {orchestrationFlow.title}
                </Text>
                <Text style={{ fontSize: 12, color: theme.colors.textSecondary }}>
                    {orchestrationFlow.nodes.filter(node => node.status === 'completed').length}/{orchestrationFlow.nodes.length}
                </Text>
                <Ionicons
                    name={showOrchFlow ? 'chevron-up' : 'chevron-down'}
                    size={16}
                    color={theme.colors.textSecondary}
                />
            </Pressable>
            {showOrchFlow && <OrchestrationFlowView flow={orchestrationFlow} />}
        </View>
    ) : null;

    const relayOfflineBanner = relayError === 'machine_offline' ? (
        <View
            style={{
                flexDirection: 'row',
                alignItems: 'center',
                gap: 8,
                marginHorizontal: 12,
                marginTop: 12,
                marginBottom: 4,
                paddingHorizontal: 12,
                paddingVertical: 10,
                borderRadius: 12,
                backgroundColor: '#FEF3C7',
                borderWidth: 1,
                borderColor: '#F59E0B',
            }}
        >
            <Ionicons name="cloud-offline-outline" size={16} color="#B45309" />
            <Text style={{ flex: 1, color: '#92400E', fontSize: 13, fontWeight: '600' }}>
                {ownerMachineName} 离线中，显示本地缓存
            </Text>
        </View>
    ) : null;

    const content = (
        <>
            {relayOfflineBanner}
            {orchFlowPanel}
            <Deferred>
                {messages.length > 0 && <ChatList session={session} />}
            </Deferred>
        </>
    );

    const placeholder = messages.length === 0 ? (
        <>
            {isLoaded ? (
                <EmptyMessages session={session} />
            ) : (
                <ActivityIndicator size="small" color={theme.colors.textSecondary} />
            )}
        </>
    ) : null;

    const input = (
        <>
            {!!session.promptSuggestions?.length && (
                <View style={{ paddingHorizontal: 16, paddingTop: 8, paddingBottom: 4, flexDirection: 'row', flexWrap: 'wrap', gap: 8 }}>
                    {session.promptSuggestions.map((suggestion, index) => (
                        <Pressable
                            key={`${suggestion}-${index}`}
                            onPress={() => handlePromptSuggestionPress(suggestion)}
                            style={({ pressed }) => ({
                                paddingHorizontal: 12,
                                paddingVertical: 8,
                                borderRadius: 999,
                                backgroundColor: pressed ? theme.colors.surfacePressed : theme.colors.surface,
                                borderWidth: 1,
                                borderColor: theme.colors.divider,
                            })}
                        >
                            <Text numberOfLines={1} style={{ color: theme.colors.text, fontSize: 13 }}>
                                {suggestion}
                            </Text>
                        </Pressable>
                    ))}
                </View>
            )}
            <AgentInput
                placeholder={t('session.inputPlaceholder')}
                value={message}
                onChangeText={setMessage}
                sessionId={sessionId}
                permissionMode={runtimeControls.permissionMode.outcome !== 'unsupported' ? permissionMode : undefined}
                onPermissionModeChange={runtimeControls.permissionMode.outcome !== 'unsupported' ? updatePermissionMode : undefined}
                sandboxPolicy={sandboxPolicy}
                onSandboxPolicyChange={runtimeControls.sandboxPolicySupported ? updateSandboxPolicy : undefined}
                metadata={session.metadata}
                connectionStatus={{
                    text: sessionStatus.statusText,
                    color: sessionStatus.statusColor,
                    dotColor: sessionStatus.statusDotColor,
                    isPulsing: sessionStatus.isPulsing,
                    compressionInfo: sessionStatus.compressionInfo,
                }}
                onSend={() => {
                    if (!message.trim()) return;
                    setMessage('');
                    clearDraft();
                    storage.getState().applySessions([{ ...session, promptSuggestions: [] }]);
                    sync.sendMessage(sessionId, message);
                    trackMessageSent();
                }}
                onMicPress={micButtonState.onMicPress}
                isMicActive={micButtonState.isMicActive}
                onAbort={() => sessionAbort(sessionId)}
                showAbortButton={sessionStatus.state === 'thinking'}
                onFileViewerPress={experiments ? (() => router.push(`/session/${sessionId}/files`)) : undefined}
                autocompletePrefixes={['@', '/']}
                autocompleteSuggestions={(query) => getSuggestions(sessionId, query)}
                usageData={sessionUsage ? {
                    inputTokens: sessionUsage.inputTokens,
                    outputTokens: sessionUsage.outputTokens,
                    cacheCreation: sessionUsage.cacheCreation,
                    cacheRead: sessionUsage.cacheRead,
                    contextSize: sessionUsage.contextSize,
                } : session.latestUsage ? {
                    inputTokens: session.latestUsage.inputTokens,
                    outputTokens: session.latestUsage.outputTokens,
                    cacheCreation: session.latestUsage.cacheCreation,
                    cacheRead: session.latestUsage.cacheRead,
                    contextSize: session.latestUsage.contextSize,
                } : undefined}
                alwaysShowContextSize={alwaysShowContextSize}
                llmProfiles={runtimeControls.model.outcome !== 'unsupported' ? llmModels : []}
                selectedLlmProfileId={sessionModelId}
                llmDefaultProfileId={llmDefaultModelId}
                onLlmProfileChange={runtimeControls.model.outcome !== 'unsupported' ? updateRuntimeModel : undefined}
                runtimeEffort={runtimeEffort}
                onRuntimeEffortChange={runtimeControls.model.outcome !== 'unsupported'
                    ? ((effort) => updateRuntimeModel(sessionModelId, effort))
                    : undefined}
                activeMcpCount={mcpServers.length}
                onMcpClick={handleMcpClick}
            />
        </>
    );

    return (
        <>
            {shouldShowCliWarning && !(isLandscape && deviceType === 'phone') && (
                <Pressable
                    onPress={handleDismissCliWarning}
                    style={{
                        position: 'absolute',
                        top: 8,
                        alignSelf: 'center',
                        backgroundColor: '#FFF3CD',
                        borderRadius: 100,
                        paddingHorizontal: 14,
                        paddingVertical: 7,
                        flexDirection: 'row',
                        alignItems: 'center',
                        zIndex: 998,
                        shadowColor: '#000',
                        shadowOffset: { width: 0, height: 2 },
                        shadowOpacity: 0.15,
                        shadowRadius: 4,
                        elevation: 4,
                    }}
                >
                    <Ionicons name="warning-outline" size={14} color="#FF9500" style={{ marginRight: 6 }} />
                    <Text style={{ fontSize: 12, color: '#856404', fontWeight: '600' }}>
                        {t('sessionInfo.cliVersionOutdated')}
                    </Text>
                    <Ionicons name="close" size={14} color="#856404" style={{ marginLeft: 8 }} />
                </Pressable>
            )}

            <View style={{ flexBasis: 0, flexGrow: 1, paddingBottom: safeArea.bottom + ((isRunningOnMac() || Platform.OS === 'web') ? 32 : 0) }}>
                <AgentContentView
                    content={content}
                    input={input}
                    placeholder={placeholder}
                />
            </View>

            {isLandscape && deviceType === 'phone' && (
                <Pressable
                    onPress={() => router.back()}
                    style={{
                        position: 'absolute',
                        top: safeArea.top + 8,
                        left: 16,
                        width: 44,
                        height: 44,
                        borderRadius: 22,
                        backgroundColor: `rgba(${theme.dark ? '28, 23, 28' : '255, 255, 255'}, 0.9)`,
                        alignItems: 'center',
                        justifyContent: 'center',
                        ...Platform.select({
                            ios: {
                                shadowColor: '#000',
                                shadowOffset: { width: 0, height: 2 },
                                shadowOpacity: 0.1,
                                shadowRadius: 4,
                            },
                            android: {
                                elevation: 2,
                            },
                        }),
                    }}
                    hitSlop={15}
                >
                    <Ionicons
                        name={Platform.OS === 'ios' ? 'chevron-back' : 'arrow-back'}
                        size={Platform.select({ ios: 28, default: 24 })}
                        color="#000"
                    />
                </Pressable>
            )}

            <SubAgentDetailModal
                subagent={selectedSubAgent}
                visible={showSubAgentDetail}
                onClose={() => {
                    setShowSubAgentDetail(false);
                    setSelectedSubAgent(null);
                }}
                onStop={async (id) => {
                    try {
                        await handleStopSubAgent(id);
                        setShowSubAgentDetail(false);
                        setSelectedSubAgent(null);
                    } catch (err) {
                        console.error('Failed to stop SubAgent:', err);
                    }
                }}
            />

            <ScheduledTaskDetailModal
                task={selectedScheduledTask}
                visible={showScheduledTaskDetail}
                onClose={() => {
                    setShowScheduledTaskDetail(false);
                    setSelectedScheduledTask(null);
                }}
                onToggle={async (id, enabled) => {
                    await toggleScheduledTask(id, enabled);
                }}
                onDelete={async (id) => {
                    await deleteScheduledTask(id);
                    setShowScheduledTaskDetail(false);
                    setSelectedScheduledTask(null);
                    if (runsModalIdRef.current && machineId) {
                        try {
                            const [latestRuns, latestScheduledTasks] = await Promise.all([
                                machineListRuns(machineId, sessionId),
                                machineListScheduledTasks(machineId),
                            ]);
                            setRuns(latestRuns);
                            showRunsModal(latestRuns, latestScheduledTasks);
                        } catch (err) {
                            console.warn('Failed to refresh background modal after deleting scheduled task:', err);
                            closeRunsModal();
                        }
                    }
                }}
                onUpdate={async (id, updates) => {
                    await updateScheduledTask(id, updates);
                }}
            />
        </>
    );
}

// -- Continuous Browsing confirmation modal with checkbox --

function ContinuousBrowsingConfirmModal({ onConfirm, onClose }: {
    onConfirm: (dontRemind: boolean) => void;
    onClose: () => void;
}) {
    const { theme } = useUnistyles();
    const [dontRemind, setDontRemind] = React.useState(false);

    const styles = StyleSheet.create({
        container: {
            backgroundColor: theme.colors.surface,
            borderRadius: 14,
            width: 300,
            overflow: 'hidden',
            shadowColor: theme.colors.shadow.color,
            shadowOffset: { width: 0, height: 2 },
            shadowOpacity: 0.25,
            shadowRadius: 4,
            elevation: 5,
        },
        content: {
            paddingHorizontal: 16,
            paddingTop: 20,
            paddingBottom: 12,
            alignItems: 'center',
        },
        title: {
            fontSize: 17,
            fontWeight: '600',
            textAlign: 'center',
            color: theme.colors.text,
            marginBottom: 8,
        },
        message: {
            fontSize: 13,
            textAlign: 'center',
            color: theme.colors.text,
            lineHeight: 18,
        },
        checkboxRow: {
            flexDirection: 'row',
            alignItems: 'center',
            paddingHorizontal: 16,
            paddingBottom: 12,
            gap: 8,
        },
        checkboxOuter: {
            width: 20,
            height: 20,
            borderRadius: 4,
            borderWidth: 2,
            borderColor: theme.colors.textSecondary,
            alignItems: 'center',
            justifyContent: 'center',
        },
        checkboxOuterSelected: {
            borderColor: theme.colors.radio.active,
            backgroundColor: theme.colors.radio.active,
        },
        checkboxLabel: {
            fontSize: 13,
            color: theme.colors.textSecondary,
        },
        buttonContainer: {
            borderTopWidth: 1,
            borderTopColor: theme.colors.divider,
            flexDirection: 'row',
        },
        button: {
            flex: 1,
            paddingVertical: 11,
            alignItems: 'center',
            justifyContent: 'center',
        },
        buttonPressed: {
            backgroundColor: theme.colors.divider,
        },
        buttonSeparator: {
            width: 1,
            backgroundColor: theme.colors.divider,
        },
        buttonText: {
            fontSize: 17,
            color: theme.colors.textLink,
        },
        cancelText: {
            fontWeight: '400',
        },
    });

    return (
        <View style={styles.container}>
            <View style={styles.content}>
                <Text style={styles.title}>{t('persona.continuousBrowsingTitle')}</Text>
                <Text style={styles.message}>{t('persona.continuousBrowsingMessage')}</Text>
            </View>

            <Pressable style={styles.checkboxRow} onPress={() => setDontRemind(v => !v)}>
                <View style={[styles.checkboxOuter, dontRemind && styles.checkboxOuterSelected]}>
                    {dontRemind && <Ionicons name="checkmark" size={14} color="#fff" />}
                </View>
                <Text style={styles.checkboxLabel}>{t('persona.continuousBrowsingDontRemind')}</Text>
            </Pressable>

            <View style={styles.buttonContainer}>
                <Pressable
                    style={({ pressed }) => [styles.button, pressed && styles.buttonPressed]}
                    onPress={onClose}
                >
                    <Text style={[styles.buttonText, styles.cancelText]}>{t('common.cancel')}</Text>
                </Pressable>
                <View style={styles.buttonSeparator} />
                <Pressable
                    style={({ pressed }) => [styles.button, pressed && styles.buttonPressed]}
                    onPress={() => { onConfirm(dontRemind); onClose(); }}
                >
                    <Text style={[styles.buttonText, { fontWeight: '600' }]}>{t('persona.continuousBrowsingEnable')}</Text>
                </Pressable>
            </View>
        </View>
    );
}

// -- Profile picker modal for switching models --

function ProfilePickerModal({ models, currentModelId, onSelect, onClose, description }: {
    models: ModelOptionDisplay[];
    currentModelId: string;
    onSelect: (modelId: string) => void;
    onClose: () => void;
    description?: string;
}) {
    const { theme } = useUnistyles();

    React.useEffect(() => {
        frontendLog(`[ProfilePickerModal] ${JSON.stringify({
            currentModelId,
            description: description ?? null,
            count: models.length,
            ids: models.slice(0, 20).map((model) => ({
                id: model.id,
                chatModel: model.chat?.model,
                isProxy: model.isProxy === true,
            })),
        })}`);
    }, [currentModelId, description, models]);

    return (
        <View style={{
            backgroundColor: theme.colors.surface,
            borderRadius: 14,
            width: 320,
            maxHeight: 440,
            overflow: 'hidden',
            shadowColor: theme.colors.shadow.color,
            shadowOffset: { width: 0, height: 2 },
            shadowOpacity: 0.25,
            shadowRadius: 4,
            elevation: 5,
        }}>
            <View style={{
                paddingHorizontal: 16,
                paddingTop: 16,
                paddingBottom: 12,
                borderBottomWidth: 1,
                borderBottomColor: theme.colors.divider,
                flexDirection: 'row',
                alignItems: 'center',
                justifyContent: 'space-between',
            }}>
                <Text style={{
                    fontSize: 17,
                    fontWeight: '600',
                    color: theme.colors.text,
                }}>
                    {t('persona.switchModel')}
                </Text>
                <Pressable onPress={onClose} hitSlop={8}>
                    <Ionicons name="close" size={20} color={theme.colors.textSecondary} />
                </Pressable>
            </View>
            {description ? (
                <View style={{
                    paddingHorizontal: 16,
                    paddingTop: 10,
                    paddingBottom: 8,
                    borderBottomWidth: 1,
                    borderBottomColor: theme.colors.divider,
                }}>
                    <Text style={{
                        fontSize: 12,
                        color: theme.colors.textSecondary,
                        ...Typography.default(),
                    }}>
                        {description}
                    </Text>
                </View>
            ) : null}
            <ScrollView style={{ maxHeight: 380 }}>
                <LlmProfileList
                    models={models}
                    selectedModelId={currentModelId}
                    onModelChange={onSelect}
                    variant="modal"
                />
            </ScrollView>
        </View>
    );
}

function EffortPickerModal({ value, availableLevels, onSelect, onClose }: {
    value: RuntimeEffort;
    availableLevels?: RuntimeEffort[];
    onSelect: (effort: RuntimeEffort) => void;
    onClose: () => void;
}) {
    const { theme } = useUnistyles();

    return (
        <View style={{
            backgroundColor: theme.colors.surface,
            borderRadius: 14,
            width: 320,
            overflow: 'hidden',
            shadowColor: theme.colors.shadow.color,
            shadowOffset: { width: 0, height: 2 },
            shadowOpacity: 0.25,
            shadowRadius: 4,
            elevation: 5,
        }}>
            <View style={{
                paddingHorizontal: 16,
                paddingTop: 16,
                paddingBottom: 12,
                borderBottomWidth: 1,
                borderBottomColor: theme.colors.divider,
                flexDirection: 'row',
                alignItems: 'center',
                justifyContent: 'space-between',
            }}>
                <Text style={{
                    fontSize: 17,
                    fontWeight: '600',
                    color: theme.colors.text,
                }}>
                    推理强度
                </Text>
                <Pressable onPress={onClose} hitSlop={8}>
                    <Ionicons name="close" size={20} color={theme.colors.textSecondary} />
                </Pressable>
            </View>
            <EffortSelector
                value={value}
                availableLevels={availableLevels}
                onChange={onSelect}
                title=""
            />
        </View>
    );
}
