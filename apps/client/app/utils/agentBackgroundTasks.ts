import type { Metadata } from '@/sync/storageTypes';
import type { SessionTaskLifecycleEntry } from '@/sync/storageTypes';
import type { Message, ToolCall } from '@/sync/typesMessage';
import type { VendorName } from '@/sync/ops';
import { isHostOwnedTool } from '@/components/tools/hostTool';

export type AgentBackgroundTaskVendor = VendorName | 'persona' | 'unknown';

export interface AgentBackgroundTaskItem {
    key: string;
    messageId: string;
    sessionId: string;
    callId: string | null;
    taskId: string | null;
    taskType: string | null;
    title: string;
    subtitle: string | null;
    vendor: AgentBackgroundTaskVendor;
    state: 'running' | 'completed' | 'error';
    createdAt: number;
    startedAt: number | null;
    completedAt: number | null;
    childToolCount: number;
    runningChildCount: number;
    errorChildCount: number;
}

function fallbackLifecycleTaskTitle(
    vendor: AgentBackgroundTaskVendor,
    lifecycleEntry: SessionTaskLifecycleEntry,
): string {
    return firstNonEmptyString(
        lifecycleEntry.description,
        lifecycleEntry.summary,
        lifecycleEntry.taskType ? `${lifecycleEntry.taskType} 后台任务` : null,
    ) ?? (vendor === 'claude' ? 'Claude 后台任务' : '后台任务');
}

function inferTaskVendor(metadata: Metadata | null | undefined): AgentBackgroundTaskVendor {
    const vendor = metadata?.vendor?.trim().toLowerCase();
    if (vendor === 'cteno' || vendor === 'claude' || vendor === 'codex' || vendor === 'gemini') {
        return vendor;
    }

    const flavor = metadata?.flavor?.trim().toLowerCase() ?? '';
    if (flavor.includes('claude')) return 'claude';
    if (flavor.includes('codex') || flavor.includes('openai') || flavor.includes('gpt')) return 'codex';
    if (flavor.includes('gemini')) return 'gemini';
    if (flavor.includes('cteno')) return 'cteno';
    if (flavor.includes('persona')) return 'persona';
    return 'unknown';
}

function isAgentTaskTool(tool: Pick<ToolCall, 'name'>): boolean {
    return tool.name === 'Task' || tool.name === 'Agent';
}

function isLeafBackgroundTool(tool: ToolCall): boolean {
    if (isAgentTaskTool(tool)) {
        return false;
    }
    if (isHostOwnedTool(tool)) {
        return false;
    }
    return tool.state === 'running';
}

function firstNonEmptyString(...values: Array<unknown>): string | null {
    for (const value of values) {
        if (typeof value === 'string' && value.trim().length > 0) {
            return value.trim();
        }
    }
    return null;
}

function getBackgroundTaskTitle(tool: ToolCall): string {
    if (!isAgentTaskTool(tool)) {
        return (
            firstNonEmptyString(
                tool.description,
                tool.input?.description,
                tool.input?.command,
                tool.input?.cmd,
                tool.input?.prompt,
                tool.input?.query,
                tool.input?.path,
                tool.input?.file_path,
                tool.input?.url,
            ) ?? tool.name
        );
    }
    return (
        firstNonEmptyString(
            tool.input?.description,
            tool.description,
            tool.input?.prompt,
            tool.input?.task,
            tool.input?.title,
        ) ?? '后台任务'
    );
}

function getBackgroundTaskSubtitle(tool: ToolCall, childToolCount: number): string | null {
    const taskType = firstNonEmptyString(tool.input?.taskType, tool.input?.task_type);
    if (!isAgentTaskTool(tool)) {
        return firstNonEmptyString(
            tool.input?.summary,
            tool.input?.lastToolName,
            taskType,
            tool.input?.subagent_type,
            tool.input?.agent_type,
        );
    }
    const detail = firstNonEmptyString(
        taskType,
        tool.input?.subagent_type,
        tool.input?.agent_type,
        tool.input?.agentType,
        tool.input?.vendor,
    );

    if (detail && childToolCount > 0) {
        return `${detail} · ${childToolCount} 个子工具`;
    }
    if (detail) {
        return detail;
    }
    if (childToolCount > 0) {
        return `${childToolCount} 个子工具`;
    }
    return null;
}

function summarizeToolChildren(messages: Message[]): {
    childToolCount: number;
    runningChildCount: number;
    errorChildCount: number;
} {
    let childToolCount = 0;
    let runningChildCount = 0;
    let errorChildCount = 0;

    for (const message of messages) {
        if (message.kind !== 'tool-call') continue;

        childToolCount += 1;
        if (message.tool.state === 'running') {
            runningChildCount += 1;
        } else if (message.tool.state === 'error') {
            errorChildCount += 1;
        }

        const nested = summarizeToolChildren(message.children);
        childToolCount += nested.childToolCount;
        runningChildCount += nested.runningChildCount;
        errorChildCount += nested.errorChildCount;
    }

    return { childToolCount, runningChildCount, errorChildCount };
}

const BACKGROUND_TASK_STARTED_PATTERNS = [
    /后台任务已启动[（(]ID[:：]?\s*([a-z0-9_-]+)[)）][，,]?\s*(.*)/i,
    /background task started[（(]id[:：]?\s*([a-z0-9_-]+)[)）][,，]?\s*(.*)/i,
];

const BACKGROUND_TASK_FINISHED_PATTERNS = [
    /后台任务(?:已)?(?:完成|结束|失败|取消)[（(]ID[:：]?\s*([a-z0-9_-]+)[)）]/i,
    /background task (?:completed|finished|failed|cancelled)[（(]id[:：]?\s*([a-z0-9_-]+)[)）]/i,
];

function flattenMessages(messages: Message[]): Message[] {
    const flattened: Message[] = [];

    const visit = (message: Message) => {
        flattened.push(message);
        if (message.kind !== 'tool-call') {
            return;
        }
        for (const child of message.children) {
            visit(child);
        }
    };

    for (const message of messages) {
        visit(message);
    }

    return flattened;
}

function parseBackgroundTaskStartedText(text: string): { id: string; detail: string | null } | null {
    for (const pattern of BACKGROUND_TASK_STARTED_PATTERNS) {
        const match = text.match(pattern);
        if (!match) continue;

        return {
            id: match[1],
            detail: firstNonEmptyString(match[2]),
        };
    }
    return null;
}

function parseBackgroundTaskFinishedText(text: string): string | null {
    for (const pattern of BACKGROUND_TASK_FINISHED_PATTERNS) {
        const match = text.match(pattern);
        if (match?.[1]) {
            return match[1];
        }
    }
    return null;
}

function lifecycleStateForTask(
    taskId: string | null,
    lifecycle: Record<string, SessionTaskLifecycleEntry>,
): SessionTaskLifecycleEntry | null {
    if (!taskId) {
        return null;
    }
    return lifecycle[taskId] ?? null;
}

function resolveTaskState(
    taskId: string | null,
    baseState: AgentBackgroundTaskItem['state'],
    lifecycle: Record<string, SessionTaskLifecycleEntry>,
): AgentBackgroundTaskItem['state'] {
    const entry = lifecycleStateForTask(taskId, lifecycle);
    return entry?.state ?? baseState;
}

/**
 * Compat fallback only for bg-12.
 * Canonical background task data should come from useBackgroundTasks() first.
 * Keep this legacy session-message reconstruction path only for sessions that
 * do not have canonical task records yet.
 */
function deriveLegacyAgentBackgroundTasks(
    messages: Message[],
    sessionId: string,
    metadata?: Metadata | null,
    lifecycle: Record<string, SessionTaskLifecycleEntry> = {},
): AgentBackgroundTaskItem[] {
    const vendor = inferTaskVendor(metadata);
    const tasks: AgentBackgroundTaskItem[] = [];
    const flattenedMessages = flattenMessages(messages);
    const completedAnnouncementIds = new Set<string>();
    const completedAnnouncementTimes = new Map<string, number>();
    const structuredTaskIds = new Set<string>();
    const renderedTaskIds = new Set<string>();

    for (const message of flattenedMessages) {
        if (message.kind !== 'agent-text') {
            continue;
        }
        const completedId = parseBackgroundTaskFinishedText(message.text);
        if (completedId) {
            completedAnnouncementIds.add(completedId);
            completedAnnouncementTimes.set(completedId, message.createdAt);
        }
    }

    const visit = (message: Message, insideAgentTask: boolean) => {
        if (message.kind !== 'tool-call') {
            return;
        }

        const taskLike = isAgentTaskTool(message.tool);
        if (taskLike) {
            const { childToolCount, runningChildCount, errorChildCount } = summarizeToolChildren(message.children);
            const taskId = firstNonEmptyString(message.tool.input?.taskId, message.tool.input?.task_id);
            if (taskId) {
                structuredTaskIds.add(taskId);
            }
            const state = resolveTaskState(taskId, message.tool.state, lifecycle);
            const isVisible = state !== 'running' || message.tool.state === 'running' || runningChildCount > 0;
            if (isVisible) {
                const lifecycleEntry = lifecycleStateForTask(taskId, lifecycle);
                if (taskId) {
                    renderedTaskIds.add(taskId);
                }
                tasks.push({
                    key: `${sessionId}:${message.id}`,
                    messageId: message.id,
                    sessionId,
                    callId: message.tool.callId ?? null,
                    taskId,
                    taskType: firstNonEmptyString(message.tool.input?.taskType, message.tool.input?.task_type),
                    title: getBackgroundTaskTitle(message.tool),
                    subtitle: getBackgroundTaskSubtitle(message.tool, childToolCount),
                    vendor,
                    state,
                    createdAt: message.createdAt,
                    startedAt: lifecycleEntry?.startedAt ?? message.tool.startedAt,
                    completedAt: lifecycleEntry?.completedAt ?? message.tool.completedAt,
                    childToolCount,
                    runningChildCount,
                    errorChildCount,
                });
            }
        } else if (!insideAgentTask && isLeafBackgroundTool(message.tool)) {
            const taskId = firstNonEmptyString(message.tool.input?.taskId, message.tool.input?.task_id);
            const lifecycleEntry = lifecycleStateForTask(taskId, lifecycle);
            if (taskId) {
                renderedTaskIds.add(taskId);
            }
            tasks.push({
                key: `${sessionId}:${message.id}`,
                messageId: message.id,
                sessionId,
                callId: message.tool.callId ?? null,
                taskId,
                taskType: firstNonEmptyString(message.tool.input?.taskType, message.tool.input?.task_type),
                title: getBackgroundTaskTitle(message.tool),
                subtitle: getBackgroundTaskSubtitle(message.tool, 0),
                vendor,
                state: resolveTaskState(taskId, message.tool.state, lifecycle),
                createdAt: message.createdAt,
                startedAt: lifecycleEntry?.startedAt ?? message.tool.startedAt,
                completedAt: lifecycleEntry?.completedAt ?? message.tool.completedAt,
                childToolCount: 0,
                runningChildCount: 0,
                errorChildCount: 0,
            });
        }

        for (const child of message.children) {
            visit(child, insideAgentTask || taskLike);
        }
    };

    for (const message of messages) {
        visit(message, false);
    }

    for (const message of flattenedMessages) {
        if (message.kind !== 'agent-text') {
            continue;
        }

        const started = parseBackgroundTaskStartedText(message.text);
        if (!started || structuredTaskIds.has(started.id)) {
            continue;
        }
        const lifecycleEntry = lifecycleStateForTask(started.id, lifecycle);
        const state = lifecycleEntry?.state ?? (completedAnnouncementIds.has(started.id) ? 'completed' : 'running');

        tasks.push({
            key: `${sessionId}:announcement:${started.id}`,
            messageId: message.id,
            sessionId,
            callId: null,
            taskId: started.id,
            taskType: null,
            title: vendor === 'claude' ? 'Claude 后台任务' : '后台任务',
            subtitle: firstNonEmptyString(`ID: ${started.id}`, started.detail)
                ? [ `ID: ${started.id}`, started.detail ].filter(Boolean).join(' · ')
                : null,
            vendor,
            state,
            createdAt: message.createdAt,
            startedAt: lifecycleEntry?.startedAt ?? message.createdAt,
            completedAt: lifecycleEntry?.completedAt ?? completedAnnouncementTimes.get(started.id) ?? null,
            childToolCount: 0,
            runningChildCount: 0,
            errorChildCount: 0,
        });
        renderedTaskIds.add(started.id);
    }

    for (const lifecycleEntry of Object.values(lifecycle)) {
        if (!lifecycleEntry.description && !lifecycleEntry.taskType) {
            continue;
        }
        if (renderedTaskIds.has(lifecycleEntry.taskId)) {
            continue;
        }

        const title = fallbackLifecycleTaskTitle(vendor, lifecycleEntry);
        const subtitle = firstNonEmptyString(
            lifecycleEntry.summary,
            lifecycleEntry.description && title !== lifecycleEntry.description ? lifecycleEntry.description : null,
            lifecycleEntry.taskType,
            `ID: ${lifecycleEntry.taskId}`,
        );

        tasks.push({
            key: `${sessionId}:lifecycle:${lifecycleEntry.taskId}`,
            messageId: `${sessionId}:lifecycle:${lifecycleEntry.taskId}`,
            sessionId,
            callId: null,
            taskId: lifecycleEntry.taskId,
            taskType: lifecycleEntry.taskType ?? null,
            title,
            subtitle,
            vendor,
            state: lifecycleEntry.state,
            createdAt: lifecycleEntry.startedAt ?? lifecycleEntry.updatedAt,
            startedAt: lifecycleEntry.startedAt ?? null,
            completedAt: lifecycleEntry.completedAt ?? null,
            childToolCount: 0,
            runningChildCount: 0,
            errorChildCount: lifecycleEntry.state === 'error' ? 1 : 0,
        });
    }

    return tasks.sort((a, b) => {
        const aTime = a.startedAt ?? a.createdAt;
        const bTime = b.startedAt ?? b.createdAt;
        return bTime - aTime;
    });
}

export function deriveAgentBackgroundTasks(
    messages: Message[],
    sessionId: string,
    metadata?: Metadata | null,
    lifecycle: Record<string, SessionTaskLifecycleEntry> = {},
    canonicalTasks: ReadonlyArray<unknown> = [],
): AgentBackgroundTaskItem[] {
    if (canonicalTasks.length > 0) {
        return [];
    }

    return deriveLegacyAgentBackgroundTasks(messages, sessionId, metadata, lifecycle);
}

export function countActiveAgentBackgroundTasks(tasks: AgentBackgroundTaskItem[]): number {
    return tasks.filter((task) => task.state === 'running').length;
}
