import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';

export async function createScratchDir(prefix) {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), `${prefix}-`));
  return root;
}

export function attachConsoleLogger(workspace, label) {
  return workspace.onEvent(event => {
    if (event.type === 'dispatch.queued' || event.type === 'dispatch.started' || event.type === 'dispatch.progress' || event.type === 'dispatch.completed' || event.type === 'dispatch.failed' || event.type === 'dispatch.stopped' || event.type === 'dispatch.result') {
      const dispatchLabel = `${event.dispatch.roleId}:${event.dispatch.dispatchId.slice(0, 8)}`;
      const detail = event.type === 'dispatch.result'
        ? event.resultText.slice(0, 140)
        : event.type === 'dispatch.progress'
          ? (event.summary ?? event.description)
          : event.type === 'dispatch.completed' || event.type === 'dispatch.failed' || event.type === 'dispatch.stopped'
            ? event.summary
            : event.type === 'dispatch.started'
              ? event.description
              : (event.dispatch.summary ?? event.dispatch.instruction.slice(0, 100));
      console.log(`[${label}] ${event.type} ${dispatchLabel} :: ${detail}`);
      return;
    }

    if (event.type === 'workspace.initialized') {
      console.log(`[${label}] workspace.initialized session=${event.sessionId ?? 'n/a'} agents=${event.availableAgents.join(', ')}`);
    }
  });
}

export async function printFileIfExists(filePath) {
  try {
    const text = await fs.readFile(filePath, 'utf8');
    console.log(`\nFILE ${filePath}\n${text}`);
  } catch {
    console.log(`\nFILE ${filePath} was not created.`);
  }
}
