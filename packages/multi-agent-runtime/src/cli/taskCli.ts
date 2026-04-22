#!/usr/bin/env node

import {
  addTask,
  appendSharedLesson,
  loadTaskListDocument,
  setTaskStatus,
  updateTask,
} from '../core/taskFiles.js';
import type { WorkflowWorkItemDocumentStatus } from '../core/types.js';

interface ParsedArgs {
  positional: string[];
  flags: Map<string, string[]>;
}

async function main(): Promise<void> {
  const parsed = parseArgs(process.argv.slice(2));
  const [command, ...rest] = parsed.positional;

  switch (command) {
    case 'list':
      await commandList(parsed);
      return;
    case 'get':
      await commandGet(rest[0], parsed);
      return;
    case 'add':
      await commandAdd(parsed);
      return;
    case 'update':
      await commandUpdate(rest[0], parsed);
      return;
    case 'set-status':
      await commandSetStatus(rest[0], rest[1], parsed);
      return;
    case 'lessons:add':
      await commandLessonsAdd(parsed);
      return;
    case 'help':
    case undefined:
      printHelp();
      return;
    default:
      throw new Error(`Unknown command "${command}". Run with "help" for usage.`);
  }
}

async function commandList(parsed: ParsedArgs): Promise<void> {
  const loaded = await loadTaskListDocument(taskContext(parsed));
  process.stdout.write(`${JSON.stringify(loaded.document, null, 2)}\n`);
}

async function commandGet(taskId: string | undefined, parsed: ParsedArgs): Promise<void> {
  if (!taskId) {
    throw new Error('Missing task id for "get".');
  }
  const loaded = await loadTaskListDocument(taskContext(parsed));
  const task = loaded.document.items.find(item => item.id === taskId);
  if (!task) {
    throw new Error(`Task "${taskId}" does not exist.`);
  }
  process.stdout.write(`${JSON.stringify(task, null, 2)}\n`);
}

async function commandAdd(parsed: ParsedArgs): Promise<void> {
  const afterId = flag(parsed, 'after');
  const status = flag(parsed, 'status');
  const dependsOn = flag(parsed, 'depends-on');
  const goalsFile = flag(parsed, 'goals-file');
  const referenceFiles = flag(parsed, 'reference-files');
  const files = flag(parsed, 'files');
  const criteria = flagValues(parsed, 'criteria');
  const loaded = await addTask(
    taskContext(parsed),
    {
      id: requiredFlag(parsed, 'id'),
      title: requiredFlag(parsed, 'title'),
      description: requiredFlag(parsed, 'description'),
      ...(afterId ? { afterId } : {}),
      ...(status ? { status: parseStatus(status) } : {}),
      ...(flag(parsed, 'attempts') ? { attempts: parseIntegerFlag(parsed, 'attempts') } : {}),
      ...(flag(parsed, 'max-attempts')
        ? { maxAttempts: parseIntegerFlag(parsed, 'max-attempts') }
        : {}),
      ...(dependsOn ? { dependsOn: splitCsv(dependsOn) } : {}),
      ...(goalsFile ? { goalsFile } : {}),
      ...(referenceFiles ? { referenceFiles: splitCsv(referenceFiles) } : {}),
      ...(files ? { files: splitCsv(files) } : {}),
      ...(criteria.length ? { acceptanceCriteria: criteria } : {}),
    },
  );
  process.stdout.write(`${JSON.stringify(loaded.document, null, 2)}\n`);
}

async function commandUpdate(taskId: string | undefined, parsed: ParsedArgs): Promise<void> {
  if (!taskId) {
    throw new Error('Missing task id for "update".');
  }
  const title = flag(parsed, 'title');
  const description = flag(parsed, 'description');
  const status = flag(parsed, 'status');
  const dependsOn = flag(parsed, 'depends-on');
  const goalsFile = flag(parsed, 'goals-file');
  const referenceFiles = flag(parsed, 'reference-files');
  const files = flag(parsed, 'files');
  const criteria = flagValues(parsed, 'criteria');
  const loaded = await updateTask(
    taskContext(parsed),
    taskId,
    {
      ...(title !== undefined ? { title } : {}),
      ...(description !== undefined ? { description } : {}),
      ...(status !== undefined ? { status: parseStatus(status) } : {}),
      ...(flag(parsed, 'attempts') !== undefined
        ? { attempts: parseIntegerFlag(parsed, 'attempts') }
        : {}),
      ...(flag(parsed, 'max-attempts') !== undefined
        ? { maxAttempts: parseIntegerFlag(parsed, 'max-attempts') }
        : {}),
      ...(dependsOn !== undefined ? { dependsOn: splitCsv(dependsOn) } : {}),
      ...(goalsFile !== undefined ? { goalsFile } : {}),
      ...(referenceFiles !== undefined ? { referenceFiles: splitCsv(referenceFiles) } : {}),
      ...(files !== undefined ? { files: splitCsv(files) } : {}),
      ...(criteria.length ? { acceptanceCriteria: criteria } : {}),
    },
  );
  process.stdout.write(`${JSON.stringify(loaded.document, null, 2)}\n`);
}

async function commandSetStatus(
  taskId: string | undefined,
  status: string | undefined,
  parsed: ParsedArgs,
): Promise<void> {
  if (!taskId || !status) {
    throw new Error('Usage: set-status <task-id> <status> --tasks <path> [--reason "..."]');
  }
  const loaded = await setTaskStatus(
    taskContext(parsed),
    taskId,
    parseStatus(status),
    flag(parsed, 'reason'),
  );
  process.stdout.write(`${JSON.stringify(loaded.document, null, 2)}\n`);
}

async function commandLessonsAdd(parsed: ParsedArgs): Promise<void> {
  const taskId = flag(parsed, 'task-id');
  const actorRoleId = flag(parsed, 'role');
  await appendSharedLesson(flag(parsed, 'cwd'), requiredFlag(parsed, 'lessons'), {
    title: requiredFlag(parsed, 'title'),
    body: requiredFlag(parsed, 'body'),
    ...(taskId ? { taskId } : {}),
    ...(actorRoleId ? { actorRoleId } : {}),
  });
  process.stdout.write('ok\n');
}

function parseArgs(argv: string[]): ParsedArgs {
  const positional: string[] = [];
  const flags = new Map<string, string[]>();
  for (let index = 0; index < argv.length; index += 1) {
    const current = argv[index];
    if (!current) {
      continue;
    }
    if (!current.startsWith('--')) {
      positional.push(current);
      continue;
    }
    const key = current.slice(2);
    const next = argv[index + 1];
    if (!next || next.startsWith('--')) {
      flags.set(key, [...(flags.get(key) ?? []), 'true']);
      continue;
    }
    flags.set(key, [...(flags.get(key) ?? []), next]);
    index += 1;
  }
  return { positional, flags };
}

function taskContext(parsed: ParsedArgs): { cwd?: string; taskListPath: string } {
  const cwd = flag(parsed, 'cwd');
  const taskListPath = requiredFlag(parsed, 'tasks');
  return {
    ...(cwd ? { cwd } : {}),
    taskListPath,
  };
}

function requiredFlag(parsed: ParsedArgs, name: string): string {
  const value = flag(parsed, name);
  if (!value) {
    throw new Error(`Missing required flag --${name}.`);
  }
  return value;
}

function flag(parsed: ParsedArgs, name: string): string | undefined {
  return parsed.flags.get(name)?.at(-1);
}

function flagValues(parsed: ParsedArgs, name: string): string[] {
  return parsed.flags.get(name) ?? [];
}

function parseIntegerFlag(parsed: ParsedArgs, name: string): number {
  const raw = requiredFlag(parsed, name);
  const parsedValue = Number.parseInt(raw, 10);
  if (!Number.isFinite(parsedValue)) {
    throw new Error(`Flag --${name} must be an integer, received "${raw}".`);
  }
  return parsedValue;
}

function splitCsv(value: string): string[] {
  return value
    .split(',')
    .map(entry => entry.trim())
    .filter(Boolean);
}

function parseStatus(value: string): WorkflowWorkItemDocumentStatus {
  const normalized = value.trim().toLowerCase() as WorkflowWorkItemDocumentStatus;
  const allowed: WorkflowWorkItemDocumentStatus[] = [
    'pending',
    'running',
    'completed',
    'failed',
    'blocked',
    'discarded',
    'abandoned',
    'superseded',
    'done',
    'complete',
  ];
  if (!allowed.includes(normalized)) {
    throw new Error(`Unsupported status "${value}".`);
  }
  return normalized;
}

function printHelp(): void {
  process.stdout.write(
    [
      'Usage:',
      '  node taskCli.js list --tasks 00-management/tasks.json [--cwd /repo/root]',
      '  node taskCli.js get <task-id> --tasks 00-management/tasks.json [--cwd /repo/root]',
      '  node taskCli.js add --tasks 00-management/tasks.json --id task-id --title "Title" --description "..." [--after current-id] [--files a,b] [--criteria "..."]',
      '  node taskCli.js update <task-id> --tasks 00-management/tasks.json [--title "Title"] [--description "..."] [--status pending]',
      '  node taskCli.js set-status <task-id> <status> --tasks 00-management/tasks.json [--reason "..."]',
      '  node taskCli.js lessons:add --lessons 00-management/shared-lessons.md --title "Pitfall" --body "..." [--task-id task-id] [--role coder]',
    ].join('\n') + '\n',
  );
}

main().catch(error => {
  const message = error instanceof Error ? error.message : String(error);
  process.stderr.write(`${message}\n`);
  process.exitCode = 1;
});
