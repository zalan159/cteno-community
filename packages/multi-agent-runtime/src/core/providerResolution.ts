import type {
  MultiAgentProvider,
  RoleSpec,
  RoleTaskRequest,
  WorkflowNodeSpec,
  WorkspaceSpec,
} from './types.js';

export const DEFAULT_PROVIDER_MODELS: Record<MultiAgentProvider, string> = {
  'claude-agent-sdk': 'claude-sonnet-4-5',
  'codex-sdk': 'gpt-5.4',
};

export interface ResolvedDispatchTarget {
  provider: MultiAgentProvider;
  model: string;
}

export function resolveWorkspaceDefaultProvider(spec: WorkspaceSpec): MultiAgentProvider | undefined {
  if (spec.defaultProvider) {
    return spec.defaultProvider;
  }

  return spec.provider === 'hybrid' ? undefined : spec.provider;
}

export function resolveWorkspaceDefaultModel(
  spec: WorkspaceSpec,
  provider: MultiAgentProvider,
): string {
  const defaultProvider = resolveWorkspaceDefaultProvider(spec);
  if (spec.defaultModel && (!defaultProvider || defaultProvider === provider)) {
    return spec.defaultModel;
  }

  if (spec.model && (!defaultProvider || defaultProvider === provider)) {
    return spec.model;
  }

  return DEFAULT_PROVIDER_MODELS[provider];
}

export function resolveRoleProvider(
  spec: WorkspaceSpec,
  role: RoleSpec,
): MultiAgentProvider {
  return role.agent.provider ?? resolveWorkspaceProviderFallback(spec);
}

export function resolveRoleModel(
  spec: WorkspaceSpec,
  role: RoleSpec,
  provider = resolveRoleProvider(spec, role),
): string {
  if (role.agent.model && (!role.agent.provider || role.agent.provider === provider)) {
    return role.agent.model;
  }

  return resolveWorkspaceDefaultModel(spec, provider);
}

export function resolveWorkflowNodeProvider(
  spec: WorkspaceSpec,
  role: RoleSpec,
  node: WorkflowNodeSpec,
): MultiAgentProvider {
  return node.provider ?? resolveRoleProvider(spec, role);
}

export function resolveWorkflowNodeModel(
  spec: WorkspaceSpec,
  role: RoleSpec,
  node: WorkflowNodeSpec,
  provider = resolveWorkflowNodeProvider(spec, role, node),
): string {
  if (node.model) {
    return node.model;
  }

  const roleProvider = resolveRoleProvider(spec, role);
  if (provider === roleProvider) {
    return resolveRoleModel(spec, role, provider);
  }

  return resolveWorkspaceDefaultModel(spec, provider);
}

export function resolveDispatchTarget(
  spec: WorkspaceSpec,
  role: RoleSpec,
  request: Pick<RoleTaskRequest, 'provider' | 'model' | 'workflowNodeId'> = {},
): ResolvedDispatchTarget {
  const workflowNode =
    request.workflowNodeId && spec.workflow
      ? spec.workflow.nodes.find(node => node.id === request.workflowNodeId)
      : undefined;
  const provider =
    request.provider ??
    (workflowNode ? resolveWorkflowNodeProvider(spec, role, workflowNode) : undefined) ??
    resolveRoleProvider(spec, role);
  const model =
    request.model ??
    (workflowNode ? resolveWorkflowNodeModel(spec, role, workflowNode, provider) : undefined) ??
    resolveRoleModel(spec, role, provider);

  return {
    provider,
    model,
  };
}

export function collectExplicitProviders(spec: Pick<WorkspaceSpec, 'roles' | 'workflow'>): Set<MultiAgentProvider> {
  const providers = new Set<MultiAgentProvider>();

  for (const role of spec.roles) {
    if (role.agent.provider) {
      providers.add(role.agent.provider);
    }
  }

  for (const node of spec.workflow?.nodes ?? []) {
    if (node.provider) {
      providers.add(node.provider);
    }
  }

  return providers;
}

function resolveWorkspaceProviderFallback(spec: WorkspaceSpec): MultiAgentProvider {
  const defaultProvider = resolveWorkspaceDefaultProvider(spec);
  if (defaultProvider) {
    return defaultProvider;
  }

  const explicitProviders = collectExplicitProviders(spec);
  if (explicitProviders.size === 1) {
    return explicitProviders.values().next().value as MultiAgentProvider;
  }

  throw new Error(
    `Workspace "${spec.name}" cannot resolve a default provider. Set template/profile default provider or an explicit role provider.`,
  );
}
