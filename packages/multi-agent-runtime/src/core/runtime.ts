import { EventEmitter } from 'node:events';

import type { WorkspaceEvent } from './events.js';
import type { TaskDispatch, WorkspaceState } from './types.js';

const SIGNAL_EXIT_CODES: Partial<Record<NodeJS.Signals, number>> = {
  SIGINT: 130,
  SIGTERM: 143,
};

const processCleanupHandlers = new Map<symbol, (reason: string) => Promise<void>>();
let processCleanupHooksInstalled = false;
let processCleanupInFlight: Promise<void> | undefined;
let processExitTriggered = false;

export function isTerminalDispatchStatus(
  status: TaskDispatch['status'] | WorkspaceState['status'] | undefined,
): boolean {
  return status === 'completed' || status === 'failed' || status === 'stopped';
}

function registerProcessCleanupHandler(
  handler: (reason: string) => Promise<void>,
): symbol {
  ensureProcessCleanupHooksInstalled();
  const token = Symbol('workspace-process-cleanup');
  processCleanupHandlers.set(token, handler);
  return token;
}

function unregisterProcessCleanupHandler(token: symbol | undefined): void {
  if (!token) {
    return;
  }
  processCleanupHandlers.delete(token);
}

function ensureProcessCleanupHooksInstalled(): void {
  if (processCleanupHooksInstalled) {
    return;
  }
  processCleanupHooksInstalled = true;

  process.on('beforeExit', code => {
    if (processCleanupHandlers.size === 0) {
      return;
    }
    void runProcessCleanup(`beforeExit(${code})`);
  });

  for (const signal of ['SIGINT', 'SIGTERM'] as const) {
    process.on(signal, () => {
      if (processExitTriggered) {
        return;
      }
      processExitTriggered = true;
      void (async () => {
        try {
          await runProcessCleanup(`signal:${signal}`);
        } finally {
          process.exitCode = SIGNAL_EXIT_CODES[signal] ?? 1;
          process.exit(process.exitCode);
        }
      })();
    });
  }
}

async function runProcessCleanup(reason: string): Promise<void> {
  if (processCleanupInFlight) {
    return processCleanupInFlight;
  }

  processCleanupInFlight = (async () => {
    const handlers = [...processCleanupHandlers.values()];
    if (handlers.length === 0) {
      return;
    }
    await Promise.allSettled(handlers.map(handler => handler(reason)));
  })();

  try {
    await processCleanupInFlight;
  } finally {
    processCleanupInFlight = undefined;
  }
}

export abstract class WorkspaceRuntime extends EventEmitter {
  private processCleanupToken: symbol | undefined;

  private getDispatchSnapshot(dispatchId: string): TaskDispatch | undefined {
    const runtime = this as WorkspaceRuntime & { getSnapshot?: () => WorkspaceState };
    return runtime.getSnapshot?.().dispatches[dispatchId];
  }

  protected activateProcessExitCleanup(): void {
    if (this.processCleanupToken) {
      return;
    }
    this.processCleanupToken = registerProcessCleanupHandler(reason =>
      this.handleProcessExitCleanup(reason),
    );
  }

  protected deactivateProcessExitCleanup(): void {
    unregisterProcessCleanupHandler(this.processCleanupToken);
    this.processCleanupToken = undefined;
  }

  protected async handleProcessExitCleanup(_reason: string): Promise<void> {}

  protected async handleDispatchTimeout(_dispatchId: string, _error: Error): Promise<void> {}

  protected emitEvent(event: WorkspaceEvent): void {
    this.emit('event', event);
  }

  onEvent(listener: (event: WorkspaceEvent) => void): () => void {
    this.on('event', listener);
    return () => this.off('event', listener);
  }

  waitForEvent<TEvent extends WorkspaceEvent = WorkspaceEvent>(
    predicate: (event: WorkspaceEvent) => event is TEvent,
    options: { timeoutMs?: number } = {},
  ): Promise<TEvent> {
    const timeoutMs = options.timeoutMs ?? 120_000;

    return new Promise<TEvent>((resolve, reject) => {
      let timeout: NodeJS.Timeout | undefined;

      const cleanup = () => {
        this.off('event', onEvent);
        if (timeout) {
          clearTimeout(timeout);
        }
      };

      const onEvent = (event: WorkspaceEvent) => {
        if (!predicate(event)) {
          return;
        }

        cleanup();
        resolve(event);
      };

      this.on('event', onEvent);

      timeout = setTimeout(() => {
        cleanup();
        reject(new Error(`Timed out after ${timeoutMs}ms waiting for workspace event.`));
      }, timeoutMs);
    });
  }

  waitForDispatchTerminal(
    dispatchId: string,
    options: { timeoutMs?: number } = {},
  ): Promise<
    Extract<WorkspaceEvent, { type: 'dispatch.completed' | 'dispatch.failed' | 'dispatch.stopped' }>
  > {
    return this.waitForEvent(
      (
        event,
      ): event is Extract<
        WorkspaceEvent,
        { type: 'dispatch.completed' | 'dispatch.failed' | 'dispatch.stopped' }
      > =>
        (event.type === 'dispatch.completed' ||
          event.type === 'dispatch.failed' ||
          event.type === 'dispatch.stopped') &&
        event.dispatch.dispatchId === dispatchId,
      options,
    );
  }

  waitForDispatchResult(
    dispatchId: string,
    options: { timeoutMs?: number } = {},
  ): Promise<Extract<WorkspaceEvent, { type: 'dispatch.result' }>> {
    return this.waitForEvent(
      (event): event is Extract<WorkspaceEvent, { type: 'dispatch.result' }> =>
        event.type === 'dispatch.result' && event.dispatch.dispatchId === dispatchId,
      options,
    );
  }

  async runDispatch<TDispatch extends TaskDispatch>(
    dispatchPromise: Promise<TDispatch>,
    options: { timeoutMs?: number; resultTimeoutMs?: number } = {},
  ): Promise<TDispatch> {
    const dispatch = await dispatchPromise;
    const current = this.getDispatchSnapshot(dispatch.dispatchId);
    if (current && isTerminalDispatchStatus(current.status)) {
      return { ...current } as TDispatch;
    }
    let terminal;
    try {
      terminal = await this.waitForDispatchTerminal(
        dispatch.dispatchId,
        options.timeoutMs !== undefined ? { timeoutMs: options.timeoutMs } : {},
      );
    } catch (error) {
      const timeoutError = error instanceof Error ? error : new Error(String(error));
      await this.handleDispatchTimeout(dispatch.dispatchId, timeoutError);
      const latest = this.getDispatchSnapshot(dispatch.dispatchId);
      if (latest && isTerminalDispatchStatus(latest.status)) {
        return { ...latest } as TDispatch;
      }
      throw error;
    }

    try {
      const afterTerminal = this.getDispatchSnapshot(dispatch.dispatchId);
      if (afterTerminal?.resultText) {
        return { ...afterTerminal } as TDispatch;
      }
      const result = await this.waitForDispatchResult(dispatch.dispatchId, {
        timeoutMs: options.resultTimeoutMs ?? 10_000,
      });
      return { ...result.dispatch } as TDispatch;
    } catch {
      const latest = this.getDispatchSnapshot(dispatch.dispatchId);
      if (latest) {
        return { ...latest } as TDispatch;
      }
      return { ...terminal.dispatch } as TDispatch;
    }
  }
}
