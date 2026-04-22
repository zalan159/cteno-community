import type { WorkspaceTemplate } from '../core/templates.js';

export function createCodingStudioTemplate(): WorkspaceTemplate {
  return {
    templateId: 'coding-studio',
    templateName: 'Coding Studio',
    description: 'A software delivery workspace with fixed specialist roles.',
    provider: 'claude-agent-sdk',
    defaultRoleId: 'pm',
    coordinatorRoleId: 'pm',
    orchestratorPrompt:
      'You are the orchestrator for a software delivery workspace. Keep the team aligned, route work to the correct role agent, and summarize progress crisply.',
    claimPolicy: {
      mode: 'claim',
      claimTimeoutMs: 30000,
      maxAssignees: 1,
      allowSupportingClaims: true,
      fallbackRoleId: 'pm',
    },
    activityPolicy: {
      publishUserMessages: true,
      publishCoordinatorMessages: true,
      publishDispatchLifecycle: true,
      publishMemberMessages: true,
      defaultVisibility: 'public',
    },
    workflow: {
      mode: 'review_loop',
      entryNodeId: 'claim_scope',
      stages: [
        {
          id: 'scope',
          name: 'Scope',
          description: 'Claim the request, draft the PRD, and get it accepted.',
          entryNodeId: 'claim_scope',
          exitNodeIds: ['review_prd'],
        },
        {
          id: 'delivery',
          name: 'Delivery',
          description: 'Design, implement, test, and review the change.',
          entryNodeId: 'architecture',
          exitNodeIds: ['release_review', 'complete'],
        },
      ],
      nodes: [
        {
          id: 'claim_scope',
          type: 'claim',
          title: 'Broadcast request and collect claim',
          candidateRoleIds: ['pm', 'prd'],
          stageId: 'scope',
        },
        {
          id: 'draft_prd',
          type: 'assign',
          title: 'Draft PRD',
          roleId: 'prd',
          producesArtifacts: ['prd_doc'],
          stageId: 'scope',
        },
        {
          id: 'review_prd',
          type: 'review',
          title: 'Review PRD',
          reviewerRoleId: 'reviewer',
          requiresArtifacts: ['prd_doc'],
          stageId: 'scope',
        },
        {
          id: 'architecture',
          type: 'assign',
          title: 'Create architecture plan',
          roleId: 'architect',
          requiresArtifacts: ['prd_doc'],
          producesArtifacts: ['arch_doc'],
          stageId: 'delivery',
        },
        {
          id: 'implement',
          type: 'assign',
          title: 'Implement change',
          roleId: 'coder',
          requiresArtifacts: ['prd_doc', 'arch_doc'],
          producesArtifacts: ['code_change'],
          stageId: 'delivery',
        },
        {
          id: 'test',
          type: 'assign',
          title: 'Run validation',
          roleId: 'tester',
          requiresArtifacts: ['code_change'],
          producesArtifacts: ['test_report'],
          stageId: 'delivery',
        },
        {
          id: 'release_review',
          type: 'review',
          title: 'Final release review',
          reviewerRoleId: 'reviewer',
          requiresArtifacts: ['prd_doc', 'arch_doc', 'test_report'],
          stageId: 'delivery',
        },
        {
          id: 'complete',
          type: 'complete',
          title: 'Finish delivery',
          stageId: 'delivery',
        },
      ],
      edges: [
        { from: 'claim_scope', to: 'draft_prd', when: 'success' },
        { from: 'draft_prd', to: 'review_prd', when: 'success' },
        { from: 'review_prd', to: 'architecture', when: 'approved' },
        { from: 'review_prd', to: 'draft_prd', when: 'rejected' },
        { from: 'architecture', to: 'implement', when: 'success' },
        { from: 'implement', to: 'test', when: 'success' },
        { from: 'test', to: 'release_review', when: 'pass' },
        { from: 'test', to: 'implement', when: 'fail' },
        { from: 'release_review', to: 'complete', when: 'approved' },
        { from: 'release_review', to: 'implement', when: 'rejected' },
      ],
    },
    artifacts: [
      {
        id: 'prd_doc',
        kind: 'doc',
        path: '10-prd/',
        ownerRoleId: 'prd',
        required: true,
        description: 'Implementation-ready PRD markdown.',
      },
      {
        id: 'arch_doc',
        kind: 'doc',
        path: '30-arch/',
        ownerRoleId: 'architect',
        required: true,
        description: 'Architecture and interface notes.',
      },
      {
        id: 'code_change',
        kind: 'code',
        path: '40-code/',
        ownerRoleId: 'coder',
        required: true,
        description: 'Code changes required to satisfy the request.',
      },
      {
        id: 'test_report',
        kind: 'report',
        path: '50-test/',
        ownerRoleId: 'tester',
        required: true,
        description: 'Verification evidence and residual risks.',
      },
    ],
    completionPolicy: {
      successNodeIds: ['complete'],
      failureNodeIds: [],
      maxIterations: 8,
      defaultStatus: 'stuck',
    },
    roles: [
      {
        id: 'pm',
        name: 'PM',
        outputRoot: '00-management/',
        agent: {
          description: 'Plans scope, sequencing, and acceptance criteria.',
          prompt:
            'You are a product/project manager. Clarify scope, break work into milestones, and keep handoffs explicit. Prefer concise plans with acceptance criteria.',
          capabilities: ['read', 'glob', 'grep'],
        },
      },
      {
        id: 'prd',
        name: 'PRD',
        outputRoot: '10-prd/',
        agent: {
          description: 'Writes product requirement docs and task definitions.',
          prompt:
            'You write implementation-ready PRDs. Always produce a concrete markdown deliverable instead of notes. Include explicit sections for Goal, User Story, Scope, Non-Goals, and Acceptance Criteria, and make the content specific enough for downstream implementation.',
          capabilities: ['read', 'write', 'edit', 'glob', 'grep'],
          initialPrompt:
            'Default PRD contract: write the deliverable under `10-prd/` unless the task gives another file path. Use these exact markdown section headings: `## Goal`, `## User Story`, `## Scope`, `## Non-Goals`, `## Acceptance Criteria`. Do not stop at an overview.',
        },
      },
      {
        id: 'architect',
        name: 'Architect',
        outputRoot: '30-arch/',
        agent: {
          description: 'Designs implementation plans and system changes.',
          prompt:
            'You are a software architect. Produce pragmatic design notes, data flow decisions, interfaces, and risks before coding starts.',
          capabilities: ['read', 'write', 'edit', 'glob', 'grep'],
        },
      },
      {
        id: 'coder',
        name: 'Coder',
        outputRoot: '40-code/',
        agent: {
          description: 'Implements code changes and keeps diffs focused.',
          prompt:
            'You are an implementation specialist. Make the requested change with minimal churn, explain assumptions briefly, and keep code consistent with the repository style.',
          capabilities: ['read', 'write', 'edit', 'glob', 'grep', 'shell'],
          requiresEditAccess: true,
        },
      },
      {
        id: 'tester',
        name: 'Tester',
        outputRoot: '50-test/',
        agent: {
          description: 'Runs tests, validates behavior, and reports regressions.',
          prompt:
            'You are a verification specialist. Run the narrowest useful checks first, surface failures clearly, and report residual risks if full coverage is not possible.',
          capabilities: ['read', 'write', 'edit', 'glob', 'grep', 'shell'],
        },
      },
      {
        id: 'reviewer',
        name: 'Reviewer',
        outputRoot: '60-review/',
        agent: {
          description: 'Reviews changes for bugs, regressions, and missing tests.',
          prompt:
            'You perform code review with a bug-finding mindset. Prioritize correctness, regressions, and missing validation over style commentary.',
          capabilities: ['read', 'glob', 'grep'],
        },
      },
    ],
  };
}
