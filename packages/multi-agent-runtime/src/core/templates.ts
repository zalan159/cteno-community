import type {
  ActivityPolicy,
  ClaimPolicy,
  CompletionPolicy,
  MultiAgentProvider,
  PermissionMode,
  RoleAgentSpec,
  RoleSpec,
  WorkflowArtifactSpec,
  WorkflowSpec,
  WorkflowVotePolicy,
  WorkspaceSpec,
  WorkspaceProvider,
} from './types.js';
import { DEFAULT_PROVIDER_MODELS } from './providerResolution.js';

export type AgentCapability =
  | 'read'
  | 'write'
  | 'edit'
  | 'glob'
  | 'grep'
  | 'shell'
  | 'web_fetch'
  | 'web_search';

export interface TemplateRoleAgentSpec {
  description: string;
  prompt: string;
  provider?: MultiAgentProvider;
  capabilities?: AgentCapability[];
  model?: string;
  skills?: string[];
  mcpServers?: string[];
  initialPrompt?: string;
  maxTurns?: number;
  background?: boolean;
  effort?: RoleAgentSpec['effort'];
  requiresEditAccess?: boolean;
}

export interface TemplateRoleSpec {
  id: string;
  name: string;
  description?: string;
  direct?: boolean;
  outputRoot?: string;
  agent: TemplateRoleAgentSpec;
}

export interface WorkspaceTemplate {
  templateId: string;
  templateName: string;
  description?: string;
  provider?: MultiAgentProvider;
  model?: string;
  defaultRoleId?: string;
  coordinatorRoleId?: string;
  orchestratorPrompt?: string;
  claimPolicy?: ClaimPolicy;
  activityPolicy?: ActivityPolicy;
  workflowVotePolicy?: WorkflowVotePolicy;
  workflow?: WorkflowSpec;
  artifacts?: WorkflowArtifactSpec[];
  completionPolicy?: CompletionPolicy;
  roles: TemplateRoleSpec[];
}

export interface WorkspaceInstanceParams {
  id: string;
  name: string;
  cwd?: string;
}

export interface WorkspaceProfile {
  provider?: MultiAgentProvider;
  model?: string;
  permissionMode?: PermissionMode;
  roleEditPermissionMode?: PermissionMode;
  settingSources?: Array<'user' | 'project' | 'local'>;
  allowedTools?: string[];
  disallowedTools?: string[];
  mapCapabilities?: (capabilities: AgentCapability[]) => string[];
}

const DEFAULT_CAPABILITY_TOOL_MAP: Record<AgentCapability, string> = {
  read: 'Read',
  write: 'Write',
  edit: 'Edit',
  glob: 'Glob',
  grep: 'Grep',
  shell: 'Bash',
  web_fetch: 'WebFetch',
  web_search: 'WebSearch',
};

export function createClaudeWorkspaceProfile(options: {
  model?: string;
  permissionMode?: PermissionMode;
  settingSources?: Array<'user' | 'project' | 'local'>;
  allowedTools?: string[];
  disallowedTools?: string[];
} = {}): WorkspaceProfile {
  return {
    provider: 'claude-agent-sdk',
    model: options.model ?? 'claude-sonnet-4-5',
    permissionMode: options.permissionMode ?? 'acceptEdits',
    roleEditPermissionMode: 'acceptEdits',
    settingSources: options.settingSources ?? ['project'],
    mapCapabilities: capabilities => mapCapabilities(capabilities),
    ...(options.allowedTools ? { allowedTools: options.allowedTools } : {}),
    ...(options.disallowedTools
      ? { disallowedTools: options.disallowedTools }
      : {}),
  };
}

export function createCodexWorkspaceProfile(options: {
  model?: string;
  allowedTools?: string[];
  disallowedTools?: string[];
} = {}): WorkspaceProfile {
  return {
    provider: 'codex-sdk',
    model: options.model ?? 'gpt-5.4',
    mapCapabilities: capabilities => mapCapabilities(capabilities),
    ...(options.allowedTools ? { allowedTools: options.allowedTools } : {}),
    ...(options.disallowedTools
      ? { disallowedTools: options.disallowedTools }
      : {}),
  };
}

export function instantiateWorkspace(
  template: WorkspaceTemplate,
  instance: WorkspaceInstanceParams,
  profile: WorkspaceProfile = {},
): WorkspaceSpec {
  const defaultProvider = profile.provider ?? template.provider;
  const defaultModel =
    profile.model ??
    template.model ??
    (defaultProvider ? DEFAULT_PROVIDER_MODELS[defaultProvider] : undefined);
  const roles = template.roles.map(role =>
    instantiateRole(role, profile),
  );
  const runtimeProvider = inferRuntimeProvider(template, roles, defaultProvider);

  const derivedAllowedTools = uniqueStrings(
    roles.flatMap(role => role.agent.tools ?? []),
  );

  return {
    id: instance.id,
    name: instance.name,
    provider: runtimeProvider,
    ...(defaultProvider ? { defaultProvider } : {}),
    ...(defaultModel ? { defaultModel, model: defaultModel } : {}),
    ...(instance.cwd ? { cwd: instance.cwd } : {}),
    ...(template.orchestratorPrompt
      ? { orchestratorPrompt: template.orchestratorPrompt }
      : {}),
    ...(profile.permissionMode ? { permissionMode: profile.permissionMode } : {}),
    ...(profile.settingSources ? { settingSources: profile.settingSources } : {}),
    allowedTools: profile.allowedTools ?? derivedAllowedTools,
    ...(profile.disallowedTools ? { disallowedTools: profile.disallowedTools } : {}),
    roles,
    ...(template.defaultRoleId ? { defaultRoleId: template.defaultRoleId } : {}),
    ...(template.coordinatorRoleId ? { coordinatorRoleId: template.coordinatorRoleId } : {}),
    ...(template.claimPolicy ? { claimPolicy: template.claimPolicy } : {}),
    ...(template.activityPolicy ? { activityPolicy: template.activityPolicy } : {}),
    ...(template.workflowVotePolicy ? { workflowVotePolicy: template.workflowVotePolicy } : {}),
    ...(template.workflow ? { workflow: template.workflow } : {}),
    ...(template.artifacts ? { artifacts: template.artifacts } : {}),
    ...(template.completionPolicy ? { completionPolicy: template.completionPolicy } : {}),
  };
}

function instantiateRole(
  role: TemplateRoleSpec,
  profile: WorkspaceProfile,
): RoleSpec {
  const mappedTools = profile.mapCapabilities?.(role.agent.capabilities ?? []);

  return {
    id: role.id,
    name: role.name,
    ...(role.description ? { description: role.description } : {}),
    ...(role.direct !== undefined ? { direct: role.direct } : {}),
    ...(role.outputRoot ? { outputRoot: role.outputRoot } : {}),
    agent: {
      description: role.agent.description,
      prompt: role.agent.prompt,
      ...(role.agent.provider ? { provider: role.agent.provider } : {}),
      ...(mappedTools && mappedTools.length > 0 ? { tools: mappedTools } : {}),
      ...(role.agent.model ? { model: role.agent.model } : {}),
      ...(role.agent.skills ? { skills: role.agent.skills } : {}),
      ...(role.agent.mcpServers ? { mcpServers: role.agent.mcpServers } : {}),
      ...(role.agent.initialPrompt
        ? { initialPrompt: role.agent.initialPrompt }
        : {}),
      ...(role.agent.maxTurns ? { maxTurns: role.agent.maxTurns } : {}),
      ...(role.agent.background !== undefined
        ? { background: role.agent.background }
        : {}),
      ...(role.agent.effort !== undefined ? { effort: role.agent.effort } : {}),
      ...(role.agent.requiresEditAccess && profile.roleEditPermissionMode
        ? { permissionMode: profile.roleEditPermissionMode }
        : {}),
    },
  };
}

function mapCapabilities(capabilities: AgentCapability[]): string[] {
  return uniqueStrings(
    capabilities.map(capability => DEFAULT_CAPABILITY_TOOL_MAP[capability]),
  );
}

function uniqueStrings(values: string[]): string[] {
  return Array.from(new Set(values));
}

function inferRuntimeProvider(
  template: WorkspaceTemplate,
  roles: RoleSpec[],
  defaultProvider?: MultiAgentProvider,
): WorkspaceProvider {
  const providers = new Set<MultiAgentProvider>();
  if (defaultProvider) {
    providers.add(defaultProvider);
  }

  for (const role of roles) {
    if (role.agent.provider) {
      providers.add(role.agent.provider);
    }
  }

  for (const node of template.workflow?.nodes ?? []) {
    if (node.provider) {
      providers.add(node.provider);
    }
  }

  if (providers.size === 0) {
    throw new Error(
      `Workspace template "${template.templateName}" does not declare a provider and no profile provider was supplied.`,
    );
  }

  if (providers.size === 1) {
    return providers.values().next().value as MultiAgentProvider;
  }

  return 'hybrid';
}
