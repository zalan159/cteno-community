import { access, appendFile, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';

import type { WorkspaceEvent } from './events.js';
import { resolveRoleModel, resolveRoleProvider } from './providerResolution.js';
import type {
  RoleSpec,
  WorkspaceProvider,
  WorkspaceSpec,
  WorkspaceState,
} from './types.js';

const RUNTIME_DIR = '.multi-agent-runtime';

export interface PersistedProviderBinding {
  roleId: string;
  providerConversationId: string;
  kind: 'session' | 'thread';
  updatedAt: string;
}

export interface PersistedProviderState {
  workspaceId: string;
  provider: WorkspaceProvider;
  rootConversationId?: string;
  memberBindings: Record<string, PersistedProviderBinding>;
  metadata?: Record<string, unknown>;
  updatedAt: string;
}

export class LocalWorkspacePersistence {
  readonly root: string;

  constructor(root: string) {
    this.root = root;
  }

  static fromSpec(spec: WorkspaceSpec): LocalWorkspacePersistence | undefined {
    if (!spec.cwd) {
      return undefined;
    }

    return new LocalWorkspacePersistence(
      join(spec.cwd, RUNTIME_DIR, spec.id),
    );
  }

  static fromWorkspace(workspaceCwd: string, workspaceId: string): LocalWorkspacePersistence {
    return new LocalWorkspacePersistence(join(workspaceCwd, RUNTIME_DIR, workspaceId));
  }

  async initializeWorkspace(spec: WorkspaceSpec): Promise<void> {
    await mkdir(this.rolesDir(), { recursive: true });
    await this.writeJson(this.workspaceSpecPath(), spec);
    await this.writeJson(this.statePath(), {
      workspaceId: spec.id,
      note: 'Workspace initialized. Runtime state will be updated after the first event batch.',
    });
    await this.writeJson(this.providerStatePath(), {
      workspaceId: spec.id,
      provider: spec.provider,
      memberBindings: {},
      updatedAt: new Date().toISOString(),
    } satisfies PersistedProviderState);

    for (const role of spec.roles) {
      const roleDir = join(this.rolesDir(), role.id);
      await mkdir(roleDir, { recursive: true });
      await writeFile(join(roleDir, 'AGENT.md'), renderAgentMarkdown(spec, role), 'utf8');
    }
  }

  async ensureWorkspaceInitialized(spec: WorkspaceSpec): Promise<void> {
    try {
      await access(this.workspaceSpecPath());
    } catch {
      await this.initializeWorkspace(spec);
    }
  }

  async persistRuntime(options: {
    state: WorkspaceState;
    events: WorkspaceEvent[];
    providerState: PersistedProviderState;
  }): Promise<void> {
    await mkdir(this.root, { recursive: true });
    await this.writeJson(this.statePath(), options.state);
    await this.writeJson(this.providerStatePath(), options.providerState);
    if (options.events.length > 0) {
      const payload =
        options.events.map(event => JSON.stringify(event)).join('\n') + '\n';
      await appendFile(this.eventsPath(), payload, 'utf8');
    }
  }

  async loadWorkspaceSpec(): Promise<WorkspaceSpec> {
    return this.readJson<WorkspaceSpec>(this.workspaceSpecPath());
  }

  async loadWorkspaceState(): Promise<WorkspaceState> {
    return this.readJson<WorkspaceState>(this.statePath());
  }

  async loadProviderState(): Promise<PersistedProviderState> {
    return this.readJson<PersistedProviderState>(this.providerStatePath());
  }

  async loadEvents(): Promise<WorkspaceEvent[]> {
    let content = '';
    try {
      content = await readFile(this.eventsPath(), 'utf8');
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'ENOENT') {
        return [];
      }
      throw error;
    }
    return content
      .split('\n')
      .map(line => line.trim())
      .filter(Boolean)
      .map(line => JSON.parse(line) as WorkspaceEvent);
  }

  async deleteWorkspace(): Promise<void> {
    await rm(this.root, { recursive: true, force: true });
  }

  workspaceSpecPath(): string {
    return join(this.root, 'workspace.json');
  }

  statePath(): string {
    return join(this.root, 'state.json');
  }

  providerStatePath(): string {
    return join(this.root, 'provider-state.json');
  }

  eventsPath(): string {
    return join(this.root, 'events.jsonl');
  }

  rolesDir(): string {
    return join(this.root, 'roles');
  }

  private async writeJson(path: string, value: unknown): Promise<void> {
    await mkdir(dirname(path), { recursive: true });
    await writeFile(path, JSON.stringify(value, null, 2), 'utf8');
  }

  private async readJson<T>(path: string): Promise<T> {
    return JSON.parse(await readFile(path, 'utf8')) as T;
  }
}

function renderAgentMarkdown(spec: WorkspaceSpec, role: RoleSpec): string {
  const lines: string[] = [];
  lines.push(`# ${role.name}`);
  lines.push('');
  lines.push(role.description ?? role.agent.description);
  lines.push('');
  lines.push('## Workspace');
  lines.push(`- Workspace: ${spec.name}`);
  lines.push(`- Role ID: ${role.id}`);
  if (role.outputRoot) {
    lines.push(`- Output root: ${role.outputRoot}`);
  }
  lines.push('');
  lines.push('## Instructions');
  lines.push(role.agent.prompt);
  lines.push('');

  if (role.agent.tools?.length) {
    lines.push('## Tools');
    for (const tool of role.agent.tools) {
      lines.push(`- ${tool}`);
    }
    lines.push('');
  }

  if (role.agent.disallowedTools?.length) {
    lines.push('## Disallowed Tools');
    for (const tool of role.agent.disallowedTools) {
      lines.push(`- ${tool}`);
    }
    lines.push('');
  }

  if (role.agent.skills?.length) {
    lines.push('## Skills');
    for (const skill of role.agent.skills) {
      lines.push(`- ${skill}`);
    }
    lines.push('');
  }

  if (role.agent.model || spec.defaultModel || spec.model || spec.provider !== 'hybrid') {
    lines.push('## Model');
    lines.push(resolveRoleModel(spec, role));
    lines.push('');
  }

  if (role.agent.provider || spec.defaultProvider || spec.provider !== 'hybrid') {
    lines.push('## Provider');
    lines.push(resolveRoleProvider(spec, role));
    lines.push('');
  }

  if (role.agent.permissionMode) {
    lines.push('## Permission Mode');
    lines.push(role.agent.permissionMode);
    lines.push('');
  }

  return `${lines.join('\n').trimEnd()}\n`;
}
