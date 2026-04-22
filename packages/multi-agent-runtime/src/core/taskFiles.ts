import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';

import type {
  WorkflowTaskListArtifact,
  WorkflowWorkItem,
  WorkflowWorkItemDocumentStatus,
  WorkflowWorkItemStatus,
  WorkflowWorklistRuntimeState,
  WorkspaceSpec,
} from './types.js';

export interface LoadedWorklistDocument {
  document: WorkflowTaskListArtifact;
  sourceKey: 'items' | 'tasks';
}

export interface TaskListMutationContext {
  cwd?: string;
  taskListPath: string;
}

export interface TaskAddInput {
  id: string;
  title: string;
  description: string;
  afterId?: string;
  status?: WorkflowWorkItemDocumentStatus;
  attempts?: number;
  maxAttempts?: number;
  dependsOn?: string[];
  goalsFile?: string;
  referenceFiles?: string[];
  files?: string[];
  acceptanceCriteria?: string[];
  metadata?: Record<string, string | number | boolean | null>;
}

export interface TaskUpdateInput {
  title?: string;
  description?: string;
  status?: WorkflowWorkItemDocumentStatus;
  attempts?: number;
  maxAttempts?: number;
  dependsOn?: string[];
  goalsFile?: string;
  referenceFiles?: string[];
  files?: string[];
  acceptanceCriteria?: string[];
  metadata?: Record<string, string | number | boolean | null>;
}

export interface SharedLessonInput {
  title: string;
  body: string;
  taskId?: string;
  actorRoleId?: string;
}

export function resolveWorkspacePath(cwd: string | undefined, targetPath: string): string {
  if (path.isAbsolute(targetPath)) {
    return targetPath;
  }
  return cwd ? path.join(cwd, targetPath) : targetPath;
}

export async function loadTaskListDocument(
  context: TaskListMutationContext,
): Promise<LoadedWorklistDocument> {
  const artifactPath = resolveWorkspacePath(context.cwd, context.taskListPath);
  try {
    const raw = await readFile(artifactPath, 'utf8');
    return parseWorklistDocument(raw);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (message.includes('ENOENT')) {
      return {
        document: {
          version: 1,
          items: [],
        },
        sourceKey: 'items',
      };
    }
    throw error;
  }
}

export function parseWorklistDocument(raw: string): LoadedWorklistDocument {
  const parsed = JSON.parse(raw) as
    | WorkflowTaskListArtifact
    | { version?: number; mode?: WorkflowTaskListArtifact['mode']; summary?: string; tasks?: WorkflowWorkItem[] };
  const sourceKey = Array.isArray((parsed as { tasks?: unknown[] }).tasks) ? 'tasks' : 'items';
  const items = sourceKey === 'tasks'
    ? (parsed as { tasks: WorkflowWorkItem[] }).tasks
    : (parsed as WorkflowTaskListArtifact).items;

  return {
    document: {
      version: 1,
      ...(parsed.mode ? { mode: parsed.mode } : {}),
      ...(parsed.summary ? { summary: parsed.summary } : {}),
      items: Array.isArray(items) ? items.map(normalizeWorkItem) : [],
    },
    sourceKey,
  };
}

export async function persistTaskListDocument(
  context: TaskListMutationContext,
  loaded: LoadedWorklistDocument,
): Promise<void> {
  const artifactPath = resolveWorkspacePath(context.cwd, context.taskListPath);
  await mkdir(path.dirname(artifactPath), { recursive: true });
  const payload =
    loaded.sourceKey === 'tasks'
      ? {
          version: loaded.document.version,
          ...(loaded.document.mode ? { mode: loaded.document.mode } : {}),
          ...(loaded.document.summary ? { summary: loaded.document.summary } : {}),
          tasks: loaded.document.items,
        }
      : loaded.document;
  await writeFile(artifactPath, `${JSON.stringify(payload, null, 2)}\n`, 'utf8');
}

export function mergeWorklistDocumentWithRuntime(
  current: LoadedWorklistDocument,
  latestOnDisk: LoadedWorklistDocument | undefined,
  worklist: WorkflowWorklistRuntimeState,
): LoadedWorklistDocument {
  const base = latestOnDisk?.document ?? current.document;
  const currentItemsById = new Map(current.document.items.map(item => [item.id, item]));
  const mergedItems: WorkflowWorkItem[] = [];
  const seen = new Set<string>();

  for (const baseItem of base.items) {
    const currentItem = currentItemsById.get(baseItem.id);
    const runtimeItem = worklist.items[baseItem.id];
    mergedItems.push(mergeWorkItem(baseItem, currentItem, runtimeItem));
    seen.add(baseItem.id);
  }

  for (const currentItem of current.document.items) {
    if (seen.has(currentItem.id)) {
      continue;
    }
    const runtimeItem = worklist.items[currentItem.id];
    const status = runtimeItem?.status ?? normalizeWorkItemStatus(currentItem.status);
    if (
      status === 'running' ||
      status === 'completed' ||
      status === 'failed' ||
      status === 'blocked' ||
      status === 'discarded' ||
      status === 'abandoned' ||
      status === 'superseded'
    ) {
      mergedItems.push(mergeWorkItem(currentItem, currentItem, runtimeItem));
    }
  }

  return {
    sourceKey: latestOnDisk?.sourceKey ?? current.sourceKey,
    document: {
      version: 1,
      ...(base.mode ? { mode: base.mode } : current.document.mode ? { mode: current.document.mode } : {}),
      ...(base.summary ? { summary: base.summary } : current.document.summary ? { summary: current.document.summary } : {}),
      items: mergedItems,
    },
  };
}

export async function reloadMergedTaskListDocument(
  context: TaskListMutationContext,
  current: LoadedWorklistDocument,
  worklist: WorkflowWorklistRuntimeState,
): Promise<LoadedWorklistDocument> {
  const latestOnDisk = await loadTaskListDocument(context);
  return mergeWorklistDocumentWithRuntime(current, latestOnDisk, worklist);
}

export async function addTask(
  context: TaskListMutationContext,
  input: TaskAddInput,
): Promise<LoadedWorklistDocument> {
  const loaded = await loadTaskListDocument(context);
  if (loaded.document.items.some(item => item.id === input.id)) {
    throw new Error(`Task "${input.id}" already exists.`);
  }

  const nextItem: WorkflowWorkItem = normalizeWorkItem({
    id: input.id,
    title: input.title,
    description: input.description,
    status: input.status ?? 'pending',
    attempts: input.attempts ?? 0,
    ...(input.maxAttempts !== undefined ? { maxAttempts: input.maxAttempts } : {}),
    ...(input.dependsOn?.length ? { dependsOn: input.dependsOn } : {}),
    ...(input.goalsFile ? { goalsFile: input.goalsFile } : {}),
    ...(input.referenceFiles?.length ? { referenceFiles: input.referenceFiles } : {}),
    ...(input.files?.length ? { files: input.files } : {}),
    ...(input.acceptanceCriteria?.length ? { acceptanceCriteria: input.acceptanceCriteria } : {}),
    ...(input.metadata ? { metadata: input.metadata } : {}),
  });

  if (!input.afterId) {
    loaded.document.items.push(nextItem);
  } else {
    const insertionIndex = loaded.document.items.findIndex(item => item.id === input.afterId);
    if (insertionIndex === -1) {
      throw new Error(`Task "${input.afterId}" does not exist.`);
    }
    loaded.document.items.splice(insertionIndex + 1, 0, nextItem);
  }

  await persistTaskListDocument(context, loaded);
  return loaded;
}

export async function updateTask(
  context: TaskListMutationContext,
  taskId: string,
  input: TaskUpdateInput,
): Promise<LoadedWorklistDocument> {
  const loaded = await loadTaskListDocument(context);
  const task = loaded.document.items.find(item => item.id === taskId);
  if (!task) {
    throw new Error(`Task "${taskId}" does not exist.`);
  }

  const updated = normalizeWorkItem({
    ...task,
    ...(input.title !== undefined ? { title: input.title } : {}),
    ...(input.description !== undefined ? { description: input.description } : {}),
    ...(input.status !== undefined ? { status: input.status } : {}),
    ...(input.attempts !== undefined ? { attempts: input.attempts } : {}),
    ...(input.maxAttempts !== undefined ? { maxAttempts: input.maxAttempts } : {}),
    ...(input.dependsOn !== undefined ? { dependsOn: input.dependsOn } : {}),
    ...(input.goalsFile !== undefined ? { goalsFile: input.goalsFile } : {}),
    ...(input.referenceFiles !== undefined ? { referenceFiles: input.referenceFiles } : {}),
    ...(input.files !== undefined ? { files: input.files } : {}),
    ...(input.acceptanceCriteria !== undefined
      ? { acceptanceCriteria: input.acceptanceCriteria }
      : {}),
    ...(input.metadata !== undefined ? { metadata: input.metadata } : {}),
  });

  const index = loaded.document.items.findIndex(item => item.id === taskId);
  loaded.document.items[index] = updated;
  await persistTaskListDocument(context, loaded);
  return loaded;
}

export async function setTaskStatus(
  context: TaskListMutationContext,
  taskId: string,
  status: WorkflowWorkItemDocumentStatus,
  reason?: string,
): Promise<LoadedWorklistDocument> {
  const loaded = await loadTaskListDocument(context);
  const task = loaded.document.items.find(item => item.id === taskId);
  if (!task) {
    throw new Error(`Task "${taskId}" does not exist.`);
  }

  const metadata = {
    ...(task.metadata ?? {}),
    ...(reason ? { statusReason: reason } : {}),
  };
  const updated = normalizeWorkItem({
    ...task,
    status,
    metadata,
  });

  const index = loaded.document.items.findIndex(item => item.id === taskId);
  loaded.document.items[index] = updated;
  await persistTaskListDocument(context, loaded);
  return loaded;
}

export async function appendSharedLesson(
  cwd: string | undefined,
  lessonsPath: string,
  input: SharedLessonInput,
): Promise<void> {
  const absolutePath = resolveWorkspacePath(cwd, lessonsPath);
  const timestamp = new Date().toISOString();
  const existing = await readOptionalText(absolutePath);
  const header = existing ?? '# Shared Lessons\n\nUse this file to capture reusable pitfalls, environment gotchas, and successful patterns across stateless workflow sessions.\n';
  const entry = [
    `## ${timestamp} - ${input.title}`,
    input.taskId ? `- Task: ${input.taskId}` : null,
    input.actorRoleId ? `- Role: ${input.actorRoleId}` : null,
    '',
    input.body.trim(),
    '',
  ]
    .filter(value => value !== null)
    .join('\n');

  await mkdir(path.dirname(absolutePath), { recursive: true });
  const separator = header.endsWith('\n') ? '' : '\n';
  await writeFile(absolutePath, `${header}${separator}\n${entry}`, 'utf8');
}

async function readOptionalText(targetPath: string): Promise<string | undefined> {
  try {
    return await readFile(targetPath, 'utf8');
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (message.includes('ENOENT')) {
      return undefined;
    }
    throw error;
  }
}

function mergeWorkItem(
  baseItem: WorkflowWorkItem,
  currentItem: WorkflowWorkItem | undefined,
  runtimeItem: WorkflowWorklistRuntimeState['items'][string] | undefined,
): WorkflowWorkItem {
  const merged = {
    ...(currentItem ?? {}),
    ...baseItem,
  };
  const status = runtimeItem?.status ?? merged.status;
  return normalizeWorkItem({
    ...merged,
    ...(status ? { status } : {}),
    attempts: runtimeItem?.attempts ?? Math.max(baseItem.attempts ?? 0, currentItem?.attempts ?? 0),
  });
}

export function normalizeWorkItem(item: WorkflowWorkItem): WorkflowWorkItem {
  const normalizedStatus = normalizeWorkItemStatus(item.status);
  return {
    ...item,
    status: normalizedStatus,
    attempts: Math.max(0, item.attempts ?? 0),
  };
}

export function normalizeWorkItemStatus(
  status: WorkflowWorkItemDocumentStatus | undefined,
): WorkflowWorkItemStatus {
  switch (status) {
    case 'done':
    case 'complete':
      return 'completed';
    case 'running':
    case 'completed':
    case 'failed':
    case 'blocked':
    case 'discarded':
    case 'abandoned':
    case 'superseded':
      return status;
    case 'pending':
    default:
      return 'pending';
  }
}

export function taskListContextFromSpec(
  spec: WorkspaceSpec,
  taskListPath: string,
): TaskListMutationContext {
  return {
    ...(spec.cwd ? { cwd: spec.cwd } : {}),
    taskListPath,
  };
}
