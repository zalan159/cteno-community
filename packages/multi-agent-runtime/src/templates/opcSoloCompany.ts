import type { WorkspaceTemplate } from '../core/templates.js';

export function createOpcSoloCompanyTemplate(): WorkspaceTemplate {
  return {
    templateId: 'opc-solo-company',
    templateName: 'OPC Solo Company',
    description: 'A one-person company staffed by specialist digital operators.',
    provider: 'claude-agent-sdk',
    defaultRoleId: 'ceo',
    coordinatorRoleId: 'ceo',
    orchestratorPrompt:
      'You orchestrate a one-person company staffed by specialist digital operators. Route work to the best role, keep recommendations practical, and prefer concrete operating documents over abstract advice.',
    claimPolicy: {
      mode: 'coordinator_only',
      maxAssignees: 1,
      fallbackRoleId: 'ceo',
    },
    activityPolicy: {
      publishUserMessages: true,
      publishCoordinatorMessages: true,
      publishDispatchLifecycle: true,
      publishMemberMessages: true,
      defaultVisibility: 'public',
    },
    workflow: {
      mode: 'pipeline',
      entryNodeId: 'intake',
      stages: [
        {
          id: 'intake',
          name: 'Intake',
          description: 'CEO frames the request and routes it to the right operator.',
          entryNodeId: 'intake',
          exitNodeIds: ['route_specialist'],
        },
        {
          id: 'operations',
          name: 'Operations',
          description: 'Specialist operators prepare concrete operating artifacts.',
          entryNodeId: 'route_specialist',
          exitNodeIds: ['ceo_review', 'complete'],
        },
      ],
      nodes: [
        {
          id: 'intake',
          type: 'assign',
          title: 'Frame request and operating goal',
          roleId: 'ceo',
          stageId: 'intake',
        },
        {
          id: 'route_specialist',
          type: 'claim',
          title: 'Route to specialist',
          candidateRoleIds: ['finance', 'tax', 'admin', 'recruiter'],
          stageId: 'operations',
        },
        {
          id: 'finance_work',
          type: 'assign',
          title: 'Prepare finance deliverable',
          roleId: 'finance',
          producesArtifacts: ['finance_doc'],
          stageId: 'operations',
        },
        {
          id: 'tax_work',
          type: 'assign',
          title: 'Prepare tax deliverable',
          roleId: 'tax',
          producesArtifacts: ['tax_doc'],
          stageId: 'operations',
        },
        {
          id: 'admin_work',
          type: 'assign',
          title: 'Prepare admin deliverable',
          roleId: 'admin',
          producesArtifacts: ['admin_doc'],
          stageId: 'operations',
        },
        {
          id: 'recruit_work',
          type: 'assign',
          title: 'Prepare recruiting deliverable',
          roleId: 'recruiter',
          producesArtifacts: ['recruit_doc'],
          stageId: 'operations',
        },
        {
          id: 'ceo_review',
          type: 'review',
          title: 'CEO review and approve',
          reviewerRoleId: 'ceo',
          stageId: 'operations',
        },
        {
          id: 'complete',
          type: 'complete',
          title: 'Finish operating workflow',
          stageId: 'operations',
        },
      ],
      edges: [
        { from: 'intake', to: 'route_specialist', when: 'success' },
        { from: 'route_specialist', to: 'finance_work', when: 'success' },
        { from: 'route_specialist', to: 'tax_work', when: 'success' },
        { from: 'route_specialist', to: 'admin_work', when: 'success' },
        { from: 'route_specialist', to: 'recruit_work', when: 'success' },
        { from: 'finance_work', to: 'ceo_review', when: 'success' },
        { from: 'tax_work', to: 'ceo_review', when: 'success' },
        { from: 'admin_work', to: 'ceo_review', when: 'success' },
        { from: 'recruit_work', to: 'ceo_review', when: 'success' },
        { from: 'ceo_review', to: 'complete', when: 'approved' },
        { from: 'ceo_review', to: 'route_specialist', when: 'rejected' },
      ],
    },
    artifacts: [
      {
        id: 'finance_doc',
        kind: 'report',
        path: 'company/10-finance/',
        ownerRoleId: 'finance',
        required: true,
        description: 'Finance checklist, budget, or operating summary.',
      },
      {
        id: 'tax_doc',
        kind: 'report',
        path: 'company/20-tax/',
        ownerRoleId: 'tax',
        required: true,
        description: 'Tax filing checklist or compliance note.',
      },
      {
        id: 'admin_doc',
        kind: 'doc',
        path: 'company/30-admin/',
        ownerRoleId: 'admin',
        required: true,
        description: 'Administrative SOP or operator checklist.',
      },
      {
        id: 'recruit_doc',
        kind: 'doc',
        path: 'company/40-recruiting/',
        ownerRoleId: 'recruiter',
        required: true,
        description: 'Hiring brief, scorecard, or interview plan.',
      },
    ],
    completionPolicy: {
      successNodeIds: ['complete'],
      failureNodeIds: [],
      maxIterations: 4,
      defaultStatus: 'stuck',
    },
    roles: [
      {
        id: 'ceo',
        name: 'CEO',
        outputRoot: 'company/00-ceo/',
        agent: {
          description: 'Owns priorities, approvals, and operating decisions for the solo company.',
          prompt:
            'You are the CEO of a one-person software company. Frame decisions clearly, make tradeoffs explicit, and turn fuzzy requests into concrete next actions.',
          capabilities: ['read', 'write', 'edit', 'glob', 'grep'],
        },
      },
      {
        id: 'finance',
        name: 'Finance',
        outputRoot: 'company/10-finance/',
        agent: {
          description: 'Prepares cash, revenue, budget, and monthly operating documents.',
          prompt:
            'You are a finance operator for a lean software business. Produce concise, audit-friendly checklists, budgets, and summaries with concrete assumptions and numbers where possible.',
          capabilities: ['read', 'write', 'edit', 'glob', 'grep'],
        },
      },
      {
        id: 'tax',
        name: 'Tax',
        outputRoot: 'company/20-tax/',
        agent: {
          description: 'Prepares filing checklists, tax calendars, and compliance notes.',
          prompt:
            'You are a tax operations specialist for a solo company. Focus on deadlines, supporting documents, risks, and what needs accountant review.',
          capabilities: ['read', 'write', 'edit', 'glob', 'grep'],
        },
      },
      {
        id: 'admin',
        name: 'Admin',
        outputRoot: 'company/30-admin/',
        agent: {
          description: 'Handles administrative SOPs, vendor coordination, and internal operations.',
          prompt:
            'You are an operations administrator. Turn messy business tasks into checklists, SOPs, and lightweight systems that a solo founder can actually maintain.',
          capabilities: ['read', 'write', 'edit', 'glob', 'grep'],
        },
      },
      {
        id: 'recruiter',
        name: 'Recruiter',
        outputRoot: 'company/40-recruiting/',
        agent: {
          description: 'Drafts hiring briefs, scorecards, and interview plans when the company needs help.',
          prompt:
            'You are a pragmatic recruiting partner for a small company. Keep hiring materials specific, lightweight, and aligned with the business stage.',
          capabilities: ['read', 'write', 'edit', 'glob', 'grep'],
        },
      },
    ],
  };
}
