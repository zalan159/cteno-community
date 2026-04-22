import {
  applyTaskControlToolCall,
  supportsTaskControlTool,
} from './taskControl.js';
import type { TaskControlToolCall, TaskDispatch } from './types.js';

const TASK_CONTROL_BLOCK_PATTERN = /```task-control\s*([\s\S]*?)```/gi;

export interface ParsedTaskControlProtocol {
  calls: TaskControlToolCall[];
  cleanedText: string;
}

export function buildTaskControlPromptInjection(dispatch: TaskDispatch): string | undefined {
  if (!dispatch.workItemId) {
    return undefined;
  }

  return [
    'This dispatch is part of a structured workflow work item.',
    'At the end of your final answer, include a `task-control` fenced code block so the runtime can update machine-readable task state.',
    'Inside the block, write one JSON object per line or a JSON array.',
    'Use these commands when relevant:',
    '- `task.set_status` for progress or blocked/completed state',
    '- `task.write_handoff` when handing work to the next role',
    '- `task.submit_review` when reviewing/evaluating a task',
    '- `task.record_evidence` for tests, logs, diffs, or reports',
    'Do not wrap the JSON objects in prose inside the code block.',
    'Example:',
    '```task-control',
    '{"toolName":"task.set_status","input":{"status":"completed","summary":"Implementation complete."}}',
    '{"toolName":"task.write_handoff","input":{"summary":"Ready for review.","details":"Updated the code path and ran focused checks.","toRoleId":"reviewer"}}',
    '```',
  ].join('\n');
}

export function applyTaskControlProtocolFromText(
  dispatch: TaskDispatch,
  text: string,
): ParsedTaskControlProtocol & { dispatch: TaskDispatch } {
  const parsed = parseTaskControlProtocol(text);
  let nextDispatch = dispatch;
  for (const call of parsed.calls) {
    nextDispatch = applyTaskControlToolCall(nextDispatch, call, {
      actorRoleId: dispatch.roleId,
    });
  }

  return {
    dispatch: nextDispatch,
    calls: parsed.calls,
    cleanedText: parsed.cleanedText,
  };
}

export function parseTaskControlProtocol(text: string): ParsedTaskControlProtocol {
  const calls: TaskControlToolCall[] = [];
  const cleanedText = text.replaceAll(TASK_CONTROL_BLOCK_PATTERN, '').trim();

  for (const match of text.matchAll(TASK_CONTROL_BLOCK_PATTERN)) {
    const body = match[1]?.trim();
    if (!body) {
      continue;
    }
    calls.push(...parseTaskControlBlock(body));
  }

  return {
    calls,
    cleanedText,
  };
}

function parseTaskControlBlock(body: string): TaskControlToolCall[] {
  const trimmed = body.trim();
  const candidates: unknown[] = [];

  if (trimmed.startsWith('[')) {
    const parsed = JSON.parse(trimmed);
    if (Array.isArray(parsed)) {
      candidates.push(...parsed);
    }
  } else if (trimmed.startsWith('{') && isSingleJsonObject(trimmed)) {
    candidates.push(JSON.parse(trimmed));
  } else {
    for (const line of trimmed.split(/\r?\n/)) {
      const next = line.trim();
      if (!next) {
        continue;
      }
      candidates.push(JSON.parse(next));
    }
  }

  return candidates.flatMap(candidate => normalizeTaskControlCall(candidate));
}

function normalizeTaskControlCall(candidate: unknown): TaskControlToolCall[] {
  if (!candidate || typeof candidate !== 'object') {
    return [];
  }

  const toolName = (candidate as { toolName?: unknown }).toolName;
  const input = (candidate as { input?: unknown }).input;
  if (typeof toolName !== 'string' || !supportsTaskControlTool(toolName)) {
    return [];
  }
  if (!input || typeof input !== 'object' || Array.isArray(input)) {
    return [];
  }

  switch (toolName) {
    case 'task.set_status':
      return [{ toolName, input: input as Extract<TaskControlToolCall, { toolName: 'task.set_status' }>['input'] }];
    case 'task.write_handoff':
      return [{ toolName, input: input as Extract<TaskControlToolCall, { toolName: 'task.write_handoff' }>['input'] }];
    case 'task.submit_review':
      return [{ toolName, input: input as Extract<TaskControlToolCall, { toolName: 'task.submit_review' }>['input'] }];
    case 'task.record_evidence':
      return [{ toolName, input: input as Extract<TaskControlToolCall, { toolName: 'task.record_evidence' }>['input'] }];
    default:
      return [];
  }
}

function isSingleJsonObject(value: string): boolean {
  try {
    const parsed = JSON.parse(value);
    return typeof parsed === 'object' && parsed !== null && !Array.isArray(parsed);
  } catch {
    return false;
  }
}
