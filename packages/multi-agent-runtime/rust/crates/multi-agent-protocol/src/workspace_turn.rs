use std::collections::BTreeSet;

use crate::{
    ClaimDecision, CoordinatorDecisionKind, CoordinatorWorkflowDecision, RoleSpec,
    WorkflowNodeSpec, WorkflowNodeType, WorkflowVoteDecision, WorkspaceClaimResponse,
    WorkspaceSpec, WorkspaceTurnAssignment, WorkspaceTurnPlan, WorkspaceTurnRequest,
    WorkspaceVisibility, WorkspaceWorkflowVoteResponse,
};
use serde_json::Value;

pub fn plan_workspace_turn(
    spec: &WorkspaceSpec,
    request: &WorkspaceTurnRequest,
) -> WorkspaceTurnPlan {
    let coordinator_role_id = resolve_coordinator_role_id(spec);
    let fallback_role_id = spec
        .claim_policy
        .as_ref()
        .and_then(|policy| policy.fallback_role_id.clone())
        .or_else(|| spec.default_role_id.clone())
        .unwrap_or_else(|| coordinator_role_id.clone());
    let max_assignments = usize::from(
        request
            .max_assignments
            .or_else(|| {
                spec.claim_policy
                    .as_ref()
                    .and_then(|policy| policy.max_assignees)
            })
            .unwrap_or(1)
            .max(1),
    );

    let assignments = if let Some(preferred_role_id) = request.prefer_role_id.as_ref() {
        vec![build_assignment(
            preferred_role_id,
            &request.message,
            spec,
            None,
        )]
    } else {
        build_heuristic_assignments(spec, &request.message, &fallback_role_id, max_assignments)
    };

    WorkspaceTurnPlan {
        coordinator_role_id,
        response_text: build_response_text(&assignments),
        assignments,
        rationale: Some(
            "Planned from workspace roles, claim policy, and message-role affinity.".to_string(),
        ),
    }
}

pub fn direct_workspace_turn_plan(
    spec: &WorkspaceSpec,
    request: &WorkspaceTurnRequest,
    role_id: &str,
) -> WorkspaceTurnPlan {
    WorkspaceTurnPlan {
        coordinator_role_id: resolve_coordinator_role_id(spec),
        response_text: format!("@{} will take this next.", role_id),
        assignments: vec![build_assignment(role_id, &request.message, spec, None)],
        rationale: Some("Direct role targeting bypassed coordinator routing.".to_string()),
    }
}

pub fn build_coordinator_decision_prompt(
    spec: &WorkspaceSpec,
    request: &WorkspaceTurnRequest,
    claim_responses: Option<&[WorkspaceClaimResponse]>,
) -> String {
    let coordinator_role_id = resolve_coordinator_role_id(spec);
    let max_assignments = std::cmp::max(
        1,
        request
            .max_assignments
            .or_else(|| {
                spec.claim_policy
                    .as_ref()
                    .and_then(|policy| policy.max_assignees)
            })
            .unwrap_or(1),
    );
    let fallback_role_id = spec
        .claim_policy
        .as_ref()
        .and_then(|policy| policy.fallback_role_id.clone())
        .or_else(|| spec.default_role_id.clone())
        .unwrap_or_else(|| coordinator_role_id.clone());
    let role_lines = spec
        .roles
        .iter()
        .map(describe_role)
        .collect::<Vec<_>>()
        .join("\n");
    let preferred_role_line = request.prefer_role_id.as_ref().map_or_else(
        || {
            "Choose the best role or small set of roles based on the actual work required."
                .to_string()
        },
        |role_id| {
            format!(
                "Bias toward @{} if it is a strong fit, but do not force it if another role is clearly better.",
                role_id
            )
        },
    );
    let claim_context = claim_responses
        .filter(|responses| !responses.is_empty())
        .map(|responses| {
            let lines = responses
                .iter()
                .map(|response| {
                    format!(
                        "- @{} -> {} (confidence {:.2}): {}{}",
                        response.role_id,
                        claim_decision_label(response.decision),
                        response.confidence,
                        response.rationale,
                        response
                            .public_response
                            .as_ref()
                            .map(|value| format!(" | public: {}", value))
                            .unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "Team claim responses already collected. Use them when deciding whether to respond, delegate, or propose workflow.\n{}",
                lines
            )
        })
        .unwrap_or_default();

    vec![
        format!(
            "You are @{}, the coordinator for the workspace \"{}\".",
            coordinator_role_id, spec.name
        ),
        "A user just sent one workspace-level message. Treat it as visible to the whole team."
            .to_string(),
        "Your job is to choose exactly one of three paths:".to_string(),
        "1. `respond`: you answer directly without delegating and without starting workflow."
            .to_string(),
        "2. `delegate`: you direct one specialist role to take this next, still staying in group-chat mode."
            .to_string(),
        "3. `propose_workflow`: you think this needs programmatic workflow execution, so you ask the team to vote on entering workflow mode."
            .to_string(),
        preferred_role_line,
        format!("You may assign at most {} role(s).", max_assignments),
        format!(
            "Fallback role if no specialist clearly fits: @{}.",
            fallback_role_id
        ),
        "Return strict JSON only. Do not wrap it in markdown fences. Do not add prose before or after the JSON."
            .to_string(),
        "JSON schema:".to_string(),
        serde_json::json!({
            "kind": "respond | delegate | propose_workflow",
            "responseText": "A short public coordinator response.",
            "targetRoleId": "one valid workspace role id when kind=delegate, otherwise empty string",
            "workflowVoteReason": "short reason for entering workflow mode when kind=propose_workflow, otherwise empty string",
            "rationale": "One short sentence explaining the routing decision."
        })
        .to_string(),
        "Available roles:".to_string(),
        role_lines,
        "User workspace message:".to_string(),
        request.message.clone(),
        if claim_context.is_empty() {
            String::new()
        } else {
            claim_context
        },
        "Rules:".to_string(),
        "- Use only valid roleId values from the available roles. Return bare ids like `prd`, not `@prd`."
            .to_string(),
        "- Prefer `respond` or `delegate` by default.".to_string(),
        "- Use `propose_workflow` only when the task clearly needs staged programmatic flow, loops, gates, or formal deliverable progression."
            .to_string(),
        "- If kind=`delegate`, choose one specialist role instead of opening workflow vote."
            .to_string(),
        "- Keep responseText concise and public-facing.".to_string(),
    ]
    .into_iter()
    .filter(|line| !line.is_empty())
    .collect::<Vec<_>>()
    .join("\n\n")
}

pub fn parse_coordinator_decision(
    raw_text: &str,
    spec: &WorkspaceSpec,
    request: &WorkspaceTurnRequest,
) -> CoordinatorWorkflowDecision {
    let coordinator_role_id = resolve_coordinator_role_id(spec);
    let parsed = extract_json_object(raw_text);
    let kind = parsed
        .as_ref()
        .and_then(|value| value.get("kind"))
        .and_then(Value::as_str)
        .map(|value| value.trim().to_lowercase())
        .unwrap_or_else(|| "delegate".to_string());
    let target_role_id = parsed
        .as_ref()
        .and_then(|value| value.get("targetRoleId"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|role_id| spec.roles.iter().any(|role| role.id == *role_id))
        .map(ToString::to_string);
    let workflow_vote_reason = parsed
        .as_ref()
        .and_then(|value| value.get("workflowVoteReason"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    let response_text = parsed
        .as_ref()
        .and_then(|value| value.get("responseText"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| match kind.as_str() {
            "respond" => format!("@{} will handle this directly.", coordinator_role_id),
            "propose_workflow" => format!(
                "@{} proposes entering workflow mode for this request.",
                coordinator_role_id
            ),
            _ => format!(
                "@{} will take this next.",
                target_role_id
                    .clone()
                    .unwrap_or_else(|| coordinator_role_id.clone())
            ),
        });
    let normalized_kind = if kind == "propose_workflow" && spec.workflow.is_some() {
        CoordinatorDecisionKind::ProposeWorkflow
    } else if should_propose_workflow_heuristically(spec, &request.message) {
        CoordinatorDecisionKind::ProposeWorkflow
    } else if kind == "delegate" && target_role_id.is_some() {
        CoordinatorDecisionKind::Delegate
    } else {
        CoordinatorDecisionKind::Respond
    };

    CoordinatorWorkflowDecision {
        kind: normalized_kind,
        response_text,
        target_role_id,
        workflow_vote_reason: if workflow_vote_reason.is_some()
            || normalized_kind == CoordinatorDecisionKind::ProposeWorkflow
        {
            Some(workflow_vote_reason.unwrap_or_else(|| {
                "This request appears to need staged workflow execution with formal flow control."
                    .to_string()
            }))
        } else {
            None
        },
        rationale: parsed
            .as_ref()
            .and_then(|value| value.get("rationale"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string),
    }
}

pub fn build_workspace_claim_prompt(
    spec: &WorkspaceSpec,
    role: &RoleSpec,
    request: &WorkspaceTurnRequest,
) -> String {
    vec![
        format!(
            "You are @{} ({}) in the workspace \"{}\".",
            role.id, role.name, spec.name
        ),
        role.description
            .as_ref()
            .map(|value| format!("Role description: {}", value))
            .unwrap_or_default(),
        format!("Agent description: {}", role.agent.description),
        role.output_root
            .as_ref()
            .map(|value| format!("Preferred output root: {}", value))
            .unwrap_or_default(),
        "A new workspace message is visible to the whole team.".to_string(),
        "Your job right now is only to decide whether you should claim this task, support another owner, or decline."
            .to_string(),
        "Do not perform the work yet. Do not use tools. Do not write files. Do not answer with prose outside JSON."
            .to_string(),
        "Return strict JSON with this schema:".to_string(),
        serde_json::json!({
            "decision": "claim | support | decline",
            "confidence": 0.0,
            "rationale": "one short sentence explaining the decision",
            "publicResponse": "short public update that can appear in the workspace timeline, or an empty string if none",
            "proposedInstruction": "concrete next step you would execute if chosen as owner or supporter, or an empty string if none"
        })
        .to_string(),
        "Decision rules:".to_string(),
        "- Use `claim` when you should be a primary owner for this request.".to_string(),
        "- Use `support` when you can contribute meaningfully but should not be the main owner."
            .to_string(),
        "- Use `decline` when another role is a better fit or the task is outside your lane."
            .to_string(),
        "- Set confidence between 0 and 1.".to_string(),
        "- Keep rationale and publicResponse concise.".to_string(),
        "- Always include every JSON field. Use an empty string for publicResponse or proposedInstruction when you have nothing useful to add."
            .to_string(),
        "Workspace message:".to_string(),
        request.message.clone(),
    ]
    .into_iter()
    .filter(|line| !line.is_empty())
    .collect::<Vec<_>>()
    .join("\n\n")
}

pub fn parse_workspace_claim_response(
    raw_input: &str,
    role: &RoleSpec,
    request: &WorkspaceTurnRequest,
) -> WorkspaceClaimResponse {
    let parsed = extract_json_object(raw_input);
    let decision = normalize_claim_decision(
        parsed
            .as_ref()
            .and_then(|value| value.get("decision"))
            .and_then(Value::as_str),
        role,
        &request.message,
    );
    let confidence = normalize_confidence(
        parsed
            .as_ref()
            .and_then(|value| value.get("confidence"))
            .and_then(Value::as_f64),
        decision,
    );
    let rationale = parsed
        .as_ref()
        .and_then(|value| value.get("rationale"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| fallback_claim_rationale(role, decision, &request.message));
    let public_response = parsed
        .as_ref()
        .and_then(|value| value.get("publicResponse"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    let proposed_instruction = parsed
        .as_ref()
        .and_then(|value| value.get("proposedInstruction"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);

    WorkspaceClaimResponse {
        role_id: role.id.clone(),
        decision,
        confidence,
        rationale,
        public_response,
        proposed_instruction,
    }
}

pub fn build_workflow_vote_prompt(
    spec: &WorkspaceSpec,
    role: &RoleSpec,
    request: &WorkspaceTurnRequest,
    coordinator_decision: &CoordinatorWorkflowDecision,
) -> String {
    vec![
        format!(
            "You are @{} ({}) in the workspace \"{}\".",
            role.id, role.name, spec.name
        ),
        role.description
            .as_ref()
            .map(|value| format!("Role description: {}", value))
            .unwrap_or_default(),
        format!("Agent description: {}", role.agent.description),
        "The coordinator is considering switching this request from normal group-chat handling into workflow mode."
            .to_string(),
        format!(
            "Coordinator public message: {}",
            coordinator_decision.response_text
        ),
        coordinator_decision
            .workflow_vote_reason
            .as_ref()
            .map(|value| format!("Workflow proposal reason: {}", value))
            .unwrap_or_default(),
        "Your job is only to vote on whether this request should enter workflow mode now."
            .to_string(),
        "Return strict JSON only. Do not use tools. Do not do the work yet.".to_string(),
        "Return strict JSON with this schema:".to_string(),
        serde_json::json!({
            "decision": "approve | reject | abstain",
            "confidence": 0.0,
            "rationale": "one short sentence explaining your vote",
            "publicResponse": "short public update for the workspace timeline, or an empty string"
        })
        .to_string(),
        "Decision rules:".to_string(),
        "- Use `approve` when formal workflow execution is the best next step.".to_string(),
        "- Use `reject` when normal group-chat delegation is enough.".to_string(),
        "- Use `abstain` only when you genuinely cannot judge whether workflow mode is warranted."
            .to_string(),
        "- If the request clearly needs staged execution, loops, gates, review, or keep/discard control, do not abstain."
            .to_string(),
        if role.id == coordinator_role_id(spec) && coordinator_decision.kind == CoordinatorDecisionKind::ProposeWorkflow {
            "- You are the coordinator who proposed workflow mode. Do not abstain unless you explicitly changed your mind."
                .to_string()
        } else {
            String::new()
        },
        "- Keep rationale and publicResponse concise.".to_string(),
        "Workspace message:".to_string(),
        request.message.clone(),
    ]
    .into_iter()
    .filter(|line| !line.is_empty())
    .collect::<Vec<_>>()
    .join("\n\n")
}

pub fn parse_workflow_vote_response(
    raw_input: &str,
    role: &RoleSpec,
    spec: &WorkspaceSpec,
    request: &WorkspaceTurnRequest,
    coordinator_decision: &CoordinatorWorkflowDecision,
) -> WorkspaceWorkflowVoteResponse {
    let parsed = extract_json_object(raw_input);
    let raw_decision = parsed
        .as_ref()
        .and_then(|value| value.get("decision"))
        .and_then(Value::as_str)
        .map(|value| value.trim().to_lowercase())
        .unwrap_or_else(|| "abstain".to_string());
    let parsed_decision = match raw_decision.as_str() {
        "approve" => WorkflowVoteDecision::Approve,
        "reject" => WorkflowVoteDecision::Reject,
        _ => WorkflowVoteDecision::Abstain,
    };
    let decision = normalize_workflow_vote_decision(
        parsed_decision,
        spec,
        request,
        coordinator_decision,
        role,
    );
    let confidence = normalize_confidence(
        parsed
            .as_ref()
            .and_then(|value| value.get("confidence"))
            .and_then(Value::as_f64),
        match decision {
            WorkflowVoteDecision::Approve => ClaimDecision::Claim,
            _ => ClaimDecision::Decline,
        },
    );
    let rationale = parsed
        .as_ref()
        .and_then(|value| value.get("rationale"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| {
            format!(
                "@{} voted {:?} on entering workflow mode.",
                role.id, decision
            )
            .to_lowercase()
        });
    let public_response = parsed
        .as_ref()
        .and_then(|value| value.get("publicResponse"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .or_else(|| match decision {
            WorkflowVoteDecision::Approve => {
                Some(format!("@{} approves entering workflow mode.", role.id))
            }
            WorkflowVoteDecision::Reject => {
                Some(format!("@{} prefers to stay in group-chat mode.", role.id))
            }
            WorkflowVoteDecision::Abstain => None,
        });

    WorkspaceWorkflowVoteResponse {
        role_id: role.id.clone(),
        decision,
        confidence,
        rationale,
        public_response,
    }
}

pub fn decide_coordinator_action(
    spec: &WorkspaceSpec,
    request: &WorkspaceTurnRequest,
) -> CoordinatorWorkflowDecision {
    let coordinator_role_id = resolve_coordinator_role_id(spec);
    let fallback_role_id = spec
        .claim_policy
        .as_ref()
        .and_then(|policy| policy.fallback_role_id.clone())
        .or_else(|| spec.default_role_id.clone())
        .unwrap_or_else(|| coordinator_role_id.clone());

    if should_propose_workflow_heuristically(spec, &request.message) {
        return CoordinatorWorkflowDecision {
            kind: CoordinatorDecisionKind::ProposeWorkflow,
            response_text: format!(
                "@{} proposes entering workflow mode for this request.",
                coordinator_role_id
            ),
            target_role_id: None,
            workflow_vote_reason: Some(
                "This request appears to need staged workflow execution with formal flow control."
                    .to_string(),
            ),
            rationale: Some(
                "Workflow candidates indicate loops, gates, or staged deliverables.".to_string(),
            ),
        };
    }

    if let Some(target_role_id) = request.prefer_role_id.clone().or_else(|| {
        build_heuristic_assignments(spec, &request.message, &fallback_role_id, 1)
            .into_iter()
            .next()
            .map(|assignment| assignment.role_id)
    }) {
        return CoordinatorWorkflowDecision {
            kind: CoordinatorDecisionKind::Delegate,
            response_text: format!("@{} will take this next.", target_role_id),
            target_role_id: Some(target_role_id),
            workflow_vote_reason: None,
            rationale: Some("Role/message affinity favored a direct delegation.".to_string()),
        };
    }

    CoordinatorWorkflowDecision {
        kind: CoordinatorDecisionKind::Respond,
        response_text: format!("@{} will handle this directly.", coordinator_role_id),
        target_role_id: None,
        workflow_vote_reason: None,
        rationale: Some("No stronger specialist routing signal was found.".to_string()),
    }
}

pub fn resolve_workflow_vote_candidate_role_ids(spec: &WorkspaceSpec) -> Vec<String> {
    if let Some(configured) = spec
        .workflow_vote_policy
        .as_ref()
        .and_then(|policy| policy.candidate_role_ids.clone())
    {
        let valid = configured
            .into_iter()
            .filter(|role_id| spec.roles.iter().any(|role| role.id == *role_id))
            .collect::<Vec<_>>();
        if !valid.is_empty() {
            return valid;
        }
    }

    spec.roles.iter().map(|role| role.id.clone()).collect()
}

pub fn synthesize_workflow_vote_response(
    spec: &WorkspaceSpec,
    request: &WorkspaceTurnRequest,
    coordinator_decision: &CoordinatorWorkflowDecision,
    role: &RoleSpec,
) -> WorkspaceWorkflowVoteResponse {
    let decision = normalize_workflow_vote_decision(
        WorkflowVoteDecision::Abstain,
        spec,
        request,
        coordinator_decision,
        role,
    );
    WorkspaceWorkflowVoteResponse {
        role_id: role.id.clone(),
        decision,
        confidence: if decision == WorkflowVoteDecision::Approve {
            0.9
        } else if decision == WorkflowVoteDecision::Reject {
            0.75
        } else {
            0.2
        },
        rationale: format!("@{} voted {:?} on workflow mode.", role.id, decision).to_lowercase(),
        public_response: Some(match decision {
            WorkflowVoteDecision::Approve => {
                format!("@{} approves entering workflow mode.", role.id)
            }
            WorkflowVoteDecision::Reject => {
                format!("@{} prefers to stay in group chat mode.", role.id)
            }
            WorkflowVoteDecision::Abstain => format!("@{} abstained.", role.id),
        }),
    }
}

pub fn should_approve_workflow_vote(
    spec: &WorkspaceSpec,
    responses: &[WorkspaceWorkflowVoteResponse],
) -> bool {
    let approvals = responses
        .iter()
        .filter(|response| response.decision == WorkflowVoteDecision::Approve)
        .count();
    let rejections = responses
        .iter()
        .filter(|response| response.decision == WorkflowVoteDecision::Reject)
        .count();
    let decisive = approvals + rejections;
    let minimum_approvals = spec
        .workflow_vote_policy
        .as_ref()
        .and_then(|policy| policy.minimum_approvals)
        .unwrap_or(1)
        .max(1) as usize;
    let required_ratio = spec
        .workflow_vote_policy
        .as_ref()
        .and_then(|policy| policy.required_approval_ratio)
        .unwrap_or(1)
        .max(1) as usize;

    if approvals < minimum_approvals || decisive == 0 {
        return false;
    }

    approvals * required_ratio >= decisive
}

pub fn build_workflow_entry_plan(
    spec: &WorkspaceSpec,
    request: &WorkspaceTurnRequest,
) -> WorkspaceTurnPlan {
    let coordinator_role_id = resolve_coordinator_role_id(spec);
    let Some(workflow) = spec.workflow.as_ref() else {
        return plan_workspace_turn(spec, request);
    };
    let Some(entry_node) = workflow
        .nodes
        .iter()
        .find(|node| node.id == workflow.entry_node_id)
    else {
        return plan_workspace_turn(spec, request);
    };

    let assignment = build_assignment_from_workflow_node(spec, request, entry_node);
    if let Some(assignment) = assignment {
        WorkspaceTurnPlan {
            coordinator_role_id,
            response_text: format!(
                "Workflow mode approved. Starting at \"{}\" with @{}.",
                entry_node
                    .title
                    .clone()
                    .unwrap_or_else(|| entry_node.id.clone()),
                assignment.role_id
            ),
            assignments: vec![assignment],
            rationale: Some(format!("Workflow mode entered at node {}.", entry_node.id)),
        }
    } else {
        WorkspaceTurnPlan {
            coordinator_role_id,
            response_text: "Workflow mode approved, but the entry node has no direct assignee yet."
                .to_string(),
            assignments: Vec::new(),
            rationale: Some(format!("Workflow mode entered at node {}.", entry_node.id)),
        }
    }
}

pub fn resolve_claim_candidate_role_ids(
    spec: &WorkspaceSpec,
    request: &WorkspaceTurnRequest,
) -> Vec<String> {
    let all_role_ids = spec
        .roles
        .iter()
        .map(|role| role.id.clone())
        .collect::<Vec<_>>();
    let candidates = resolve_workflow_candidates(spec, &request.message);
    let mut ordered_role_ids = Vec::new();

    if let Some(preferred_role_id) = request
        .prefer_role_id
        .as_ref()
        .filter(|role_id| all_role_ids.iter().any(|candidate| candidate == *role_id))
    {
        ordered_role_ids.push(preferred_role_id.clone());
    }

    for candidate in candidates {
        if let Some(role_id) =
            choose_best_candidate_role(spec, &request.message, &candidate.role_ids)
        {
            if !ordered_role_ids.contains(&role_id) {
                ordered_role_ids.push(role_id);
            }
        }
    }

    for role_id in all_role_ids {
        if !ordered_role_ids.contains(&role_id) {
            ordered_role_ids.push(role_id);
        }
    }

    ordered_role_ids
}

pub fn build_plan_from_claim_responses(
    spec: &WorkspaceSpec,
    request: &WorkspaceTurnRequest,
    responses: &[WorkspaceClaimResponse],
) -> WorkspaceTurnPlan {
    let coordinator_role_id = resolve_coordinator_role_id(spec);
    let fallback_role_id = spec
        .claim_policy
        .as_ref()
        .and_then(|policy| policy.fallback_role_id.clone())
        .or_else(|| spec.default_role_id.clone())
        .unwrap_or_else(|| coordinator_role_id.clone());
    let max_assignments = usize::from(
        request
            .max_assignments
            .or_else(|| {
                spec.claim_policy
                    .as_ref()
                    .and_then(|policy| policy.max_assignees)
            })
            .unwrap_or(1)
            .max(1),
    );
    let lowered_message = request.message.to_lowercase();

    let mut claims = responses
        .iter()
        .filter(|response| response.decision == ClaimDecision::Claim)
        .cloned()
        .collect::<Vec<_>>();
    claims.sort_by(|left, right| {
        compare_claim_candidates(left, right, spec, &coordinator_role_id, &lowered_message)
    });

    let mut supports = responses
        .iter()
        .filter(|response| response.decision == ClaimDecision::Support)
        .cloned()
        .collect::<Vec<_>>();
    supports.sort_by(|left, right| {
        compare_claim_candidates(left, right, spec, &coordinator_role_id, &lowered_message)
    });

    let mut assignments = claims
        .into_iter()
        .take(max_assignments)
        .map(|response| build_assignment_from_claim_response(spec, request, response))
        .collect::<Vec<_>>();

    if assignments.len() < max_assignments
        && spec
            .claim_policy
            .as_ref()
            .and_then(|policy| policy.allow_supporting_claims)
            .unwrap_or(false)
    {
        for response in supports {
            if assignments.len() >= max_assignments {
                break;
            }
            assignments.push(build_assignment_from_claim_response(
                spec, request, response,
            ));
        }
    }

    if assignments.is_empty() {
        assignments.push(build_assignment(
            &guess_fallback_role_id(spec, &request.message, &fallback_role_id),
            &request.message,
            spec,
            None,
        ));
    }

    assignments = unique_assignments(assignments);
    let response_text = build_claim_response_text(&assignments, responses);

    WorkspaceTurnPlan {
        coordinator_role_id,
        response_text,
        assignments,
        rationale: Some(
            if responses
                .iter()
                .any(|response| response.decision != ClaimDecision::Decline)
            {
                "Assignments were resolved from member claim/support responses.".to_string()
            } else {
                "No member claimed the task, so runtime fell back to heuristic routing.".to_string()
            },
        ),
    }
}

pub fn resolve_coordinator_role_id(spec: &WorkspaceSpec) -> String {
    spec.coordinator_role_id
        .clone()
        .or_else(|| spec.default_role_id.clone())
        .or_else(|| spec.roles.first().map(|role| role.id.clone()))
        .unwrap_or_else(|| "coordinator".to_string())
}

fn build_assignment(
    role_id: &str,
    message: &str,
    spec: &WorkspaceSpec,
    workflow_candidate: Option<&WorkflowCandidate>,
) -> WorkspaceTurnAssignment {
    let role_name = spec
        .roles
        .iter()
        .find(|role| role.id == role_id)
        .map(|role| role.name.clone())
        .unwrap_or_else(|| role_id.to_string());

    let requested_output_path = extract_requested_output_path(message);
    let instruction = enforce_output_contract(
        message.trim().to_string(),
        requested_output_path.as_deref(),
        &[],
    );

    WorkspaceTurnAssignment {
        role_id: role_id.to_string(),
        instruction,
        summary: Some(format!("Handle workspace request as {}", role_name)),
        visibility: request_visibility_default(spec),
        workflow_node_id: workflow_candidate.map(|candidate| candidate.node_id.clone()),
        stage_id: workflow_candidate.and_then(|candidate| candidate.stage_id.clone()),
    }
}

fn build_assignment_from_claim_response(
    spec: &WorkspaceSpec,
    request: &WorkspaceTurnRequest,
    response: WorkspaceClaimResponse,
) -> WorkspaceTurnAssignment {
    let requested_output_path = extract_requested_output_path(&request.message);
    let proposed_instruction = response.proposed_instruction.clone();
    let instruction = match (proposed_instruction, requested_output_path.as_deref()) {
        (Some(proposed), Some(path)) if proposed.contains(path) => proposed,
        (Some(proposed), None) => proposed,
        _ => request.message.trim().to_string(),
    };
    let instruction = enforce_output_contract(instruction, requested_output_path.as_deref(), &[]);

    WorkspaceTurnAssignment {
        role_id: response.role_id.clone(),
        instruction,
        summary: response
            .public_response
            .clone()
            .or_else(|| Some(format!("Handle workspace request as @{}", response.role_id))),
        visibility: request
            .visibility
            .or_else(|| request_visibility_default(spec))
            .or(Some(WorkspaceVisibility::Public)),
        workflow_node_id: find_best_workflow_candidate_for_role(
            spec,
            &request.message,
            &response.role_id,
        )
        .map(|candidate| candidate.node_id),
        stage_id: find_best_workflow_candidate_for_role(spec, &request.message, &response.role_id)
            .and_then(|candidate| candidate.stage_id),
    }
}

pub fn build_assignment_from_workflow_node(
    spec: &WorkspaceSpec,
    request: &WorkspaceTurnRequest,
    node: &WorkflowNodeSpec,
) -> Option<WorkspaceTurnAssignment> {
    let role_id = resolve_workflow_node_role_id(spec, &request.message, node)?;
    let artifact_hints = node
        .produces_artifacts
        .clone()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|artifact_id| {
            spec.artifacts
                .as_ref()
                .and_then(|artifacts| artifacts.iter().find(|artifact| artifact.id == artifact_id))
                .map(|artifact| format!("{} -> {}", artifact.id, artifact.path))
        })
        .collect::<Vec<_>>();

    let mut instruction_parts = vec![format!(
        "You are executing workflow node \"{}\" ({}).",
        node.title.clone().unwrap_or_else(|| node.id.clone()),
        node_type_name(node.node_type)
    )];
    if let Some(stage_id) = node.stage_id.as_ref() {
        instruction_parts.push(format!("Current stage: {}.", stage_id));
    }
    if let Some(prompt) = node.prompt.as_ref() {
        instruction_parts.push(format!("Node-specific prompt: {}", prompt));
    }
    if let Some(command) = node.command.as_ref() {
        instruction_parts.push(format!(
            "Node command to prepare for or execute: {}",
            command
        ));
    }
    if !artifact_hints.is_empty() {
        instruction_parts.push(format!(
            "Artifacts to produce or update: {}.",
            artifact_hints.join(", ")
        ));
    }
    instruction_parts.push(format!("Original user request: {}", request.message));
    instruction_parts.push(
        "Focus only on this workflow step. Do not try to finish the entire workflow in one turn."
            .to_string(),
    );

    let requested_output_path = extract_requested_output_path(&request.message);
    let instruction = enforce_output_contract(
        instruction_parts.join("\n"),
        requested_output_path.as_deref(),
        &artifact_hints,
    );

    Some(WorkspaceTurnAssignment {
        role_id,
        instruction,
        summary: Some(
            node.title
                .clone()
                .map(|title| format!("{} ({})", title, node_type_name(node.node_type)))
                .unwrap_or_else(|| format!("Run workflow node {}", node.id)),
        ),
        visibility: node
            .visibility
            .or(request.visibility)
            .or(request_visibility_default(spec))
            .or(Some(WorkspaceVisibility::Public)),
        workflow_node_id: Some(node.id.clone()),
        stage_id: node.stage_id.clone(),
    })
}

fn request_visibility_default(spec: &WorkspaceSpec) -> Option<crate::WorkspaceVisibility> {
    spec.activity_policy
        .as_ref()
        .and_then(|policy| policy.default_visibility)
}

fn build_response_text(assignments: &[WorkspaceTurnAssignment]) -> String {
    let names = assignments
        .iter()
        .map(|assignment| format!("@{}", assignment.role_id))
        .collect::<Vec<_>>();

    match names.as_slice() {
        [] => "The coordinator will take this next.".to_string(),
        [only] => format!("{} will take this next.", only),
        _ => format!("{} will split this work.", names.join(" and ")),
    }
}

fn build_claim_response_text(
    assignments: &[WorkspaceTurnAssignment],
    responses: &[WorkspaceClaimResponse],
) -> String {
    let owners = responses
        .iter()
        .filter(|response| {
            response.decision == ClaimDecision::Claim
                && assignments
                    .iter()
                    .any(|assignment| assignment.role_id == response.role_id)
        })
        .collect::<Vec<_>>();
    let supports = responses
        .iter()
        .filter(|response| {
            response.decision == ClaimDecision::Support
                && assignments
                    .iter()
                    .any(|assignment| assignment.role_id == response.role_id)
        })
        .collect::<Vec<_>>();

    if owners.is_empty() && supports.is_empty() {
        return build_response_text(assignments);
    }

    let primary = if owners.is_empty() {
        &supports
    } else {
        &owners
    };
    let owner_labels = primary
        .iter()
        .map(|response| format!("@{}", response.role_id))
        .collect::<Vec<_>>();
    let support_labels = supports
        .iter()
        .filter(|response| {
            !primary
                .iter()
                .any(|owner| owner.role_id == response.role_id)
        })
        .map(|response| format!("@{}", response.role_id))
        .collect::<Vec<_>>();

    if !support_labels.is_empty() {
        return format!(
            "{} will take this next, with support from {}.",
            owner_labels.join(" and "),
            support_labels.join(" and ")
        );
    }

    if owner_labels.len() == 1 {
        return format!("{} will take this next.", owner_labels[0]);
    }

    format!("{} will split this work.", owner_labels.join(" and "))
}

fn compare_claim_responses(
    left: &WorkspaceClaimResponse,
    right: &WorkspaceClaimResponse,
) -> std::cmp::Ordering {
    right
        .confidence
        .partial_cmp(&left.confidence)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| left.role_id.cmp(&right.role_id))
}

fn compare_claim_candidates(
    left: &WorkspaceClaimResponse,
    right: &WorkspaceClaimResponse,
    spec: &WorkspaceSpec,
    coordinator_role_id: &str,
    lowered_message: &str,
) -> std::cmp::Ordering {
    score_claim_candidate(left, spec, coordinator_role_id, lowered_message)
        .partial_cmp(&score_claim_candidate(
            right,
            spec,
            coordinator_role_id,
            lowered_message,
        ))
        .unwrap_or(std::cmp::Ordering::Equal)
        .reverse()
        .then_with(|| compare_claim_responses(left, right))
}

fn score_claim_candidate(
    response: &WorkspaceClaimResponse,
    spec: &WorkspaceSpec,
    coordinator_role_id: &str,
    lowered_message: &str,
) -> f32 {
    let affinity = spec
        .roles
        .iter()
        .find(|role| role.id == response.role_id)
        .map(|role| score_role_for_message(role, lowered_message))
        .unwrap_or(0) as f32;
    let coordinator_penalty = if response.role_id == coordinator_role_id && affinity > 0.0 {
        3.0
    } else {
        0.0
    };
    let specialist_bonus = if response.role_id != coordinator_role_id && affinity > 0.0 {
        1.0
    } else {
        0.0
    };

    response.confidence * 100.0 + affinity * 10.0 + specialist_bonus - coordinator_penalty
}

fn unique_assignments(assignments: Vec<WorkspaceTurnAssignment>) -> Vec<WorkspaceTurnAssignment> {
    let mut seen = BTreeSet::new();
    assignments
        .into_iter()
        .filter(|assignment| seen.insert(assignment.role_id.clone()))
        .collect()
}

fn unique_role_ids(values: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    values
        .into_iter()
        .filter(|value| seen.insert(value.clone()))
        .collect()
}

#[derive(Debug, Clone)]
struct WorkflowCandidate {
    node_id: String,
    node_type: WorkflowNodeType,
    stage_id: Option<String>,
    role_ids: Vec<String>,
    score: usize,
}

fn build_heuristic_assignments(
    spec: &WorkspaceSpec,
    message: &str,
    fallback_role_id: &str,
    max_assignments: usize,
) -> Vec<WorkspaceTurnAssignment> {
    let workflow_candidates = resolve_workflow_candidates(spec, message);
    if !workflow_candidates.is_empty() {
        let mut assignments = Vec::new();
        for candidate in workflow_candidates.iter() {
            if let Some(role_id) = choose_best_candidate_role(spec, message, &candidate.role_ids) {
                if assignments
                    .iter()
                    .any(|assignment: &WorkspaceTurnAssignment| assignment.role_id == role_id)
                {
                    continue;
                }
                assignments.push(build_assignment(&role_id, message, spec, Some(candidate)));
                if assignments.len() >= max_assignments {
                    return assignments;
                }
            }
        }
        if !assignments.is_empty() {
            return assignments;
        }
    }

    let mut scored = spec
        .roles
        .iter()
        .map(|role| {
            (
                role.id.clone(),
                score_role_for_message(role, &message.to_lowercase()),
            )
        })
        .collect::<Vec<_>>();
    scored.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));

    let mut selected = scored
        .into_iter()
        .filter(|(_, score)| *score > 0)
        .map(|(role_id, _)| role_id)
        .take(max_assignments)
        .collect::<Vec<_>>();

    if selected.is_empty() {
        selected.push(guess_fallback_role_id(spec, message, fallback_role_id));
    }

    unique_role_ids(selected)
        .into_iter()
        .map(|role_id| build_assignment(&role_id, message, spec, None))
        .collect()
}

fn resolve_workflow_candidates(spec: &WorkspaceSpec, message: &str) -> Vec<WorkflowCandidate> {
    let Some(workflow) = spec.workflow.as_ref() else {
        return Vec::new();
    };
    let lowered = message.to_lowercase();

    let stage_lookup = workflow
        .stages
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|stage| (stage.id.clone(), stage))
        .collect::<std::collections::BTreeMap<_, _>>();
    let artifact_lookup = spec
        .artifacts
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|artifact| (artifact.id.clone(), artifact))
        .collect::<std::collections::BTreeMap<_, _>>();

    let mut candidates = workflow
        .nodes
        .iter()
        .filter_map(|node| {
            let role_ids = unique_role_ids(
                [node.role_id.clone(), node.reviewer_role_id.clone()]
                    .into_iter()
                    .flatten()
                    .chain(node.candidate_role_ids.clone().unwrap_or_default())
                    .collect(),
            );
            if role_ids.is_empty() {
                return None;
            }

            let stage = node
                .stage_id
                .as_ref()
                .and_then(|stage_id| stage_lookup.get(stage_id));
            let role_corpus = role_ids
                .iter()
                .filter_map(|role_id| spec.roles.iter().find(|role| role.id == *role_id))
                .map(|role| {
                    [
                        Some(role.id.clone()),
                        Some(role.name.clone()),
                        role.description.clone(),
                        Some(role.agent.description.clone()),
                        role.output_root.clone(),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" ")
                })
                .collect::<Vec<_>>();
            let artifact_corpus = node
                .requires_artifacts
                .clone()
                .unwrap_or_default()
                .into_iter()
                .chain(node.produces_artifacts.clone().unwrap_or_default())
                .filter_map(|artifact_id| artifact_lookup.get(&artifact_id).cloned())
                .map(|artifact| {
                    [Some(artifact.id), Some(artifact.path), artifact.description]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .collect::<Vec<_>>();

            let corpus = [
                Some(node.id.clone()),
                node.title.clone(),
                stage.map(|value| value.name.clone()),
                stage.and_then(|value| value.description.clone()),
            ]
            .into_iter()
            .flatten()
            .chain(role_corpus)
            .chain(artifact_corpus)
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();

            let token_score = build_search_tokens(&corpus)
                .into_iter()
                .filter(|token| token.len() >= 3 && lowered.contains(token))
                .map(|token| if token.len() > 8 { 3 } else { 1 })
                .sum::<usize>();
            let role_score = role_ids
                .iter()
                .filter_map(|role_id| spec.roles.iter().find(|role| role.id == *role_id))
                .map(|role| score_role_for_message(role, &lowered))
                .sum::<usize>();
            let score = token_score + role_score + workflow_node_priority(node.node_type);

            (score > 0).then_some(WorkflowCandidate {
                node_id: node.id.clone(),
                node_type: node.node_type,
                stage_id: node.stage_id.clone(),
                role_ids,
                score,
            })
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.node_id.cmp(&right.node_id))
    });
    candidates
}

fn workflow_node_priority(node_type: crate::WorkflowNodeType) -> usize {
    match node_type {
        crate::WorkflowNodeType::Assign => 6,
        crate::WorkflowNodeType::Review => 5,
        crate::WorkflowNodeType::Shell => 4,
        crate::WorkflowNodeType::Evaluate => 3,
        crate::WorkflowNodeType::Claim => 1,
        _ => 0,
    }
}

pub fn should_propose_workflow_heuristically(spec: &WorkspaceSpec, message: &str) -> bool {
    let Some(workflow) = spec.workflow.as_ref() else {
        return false;
    };

    let candidates = resolve_workflow_candidates(spec, message);
    if candidates.iter().any(|candidate| {
        matches!(
            candidate.node_type,
            WorkflowNodeType::Shell
                | WorkflowNodeType::Evaluate
                | WorkflowNodeType::Loop
                | WorkflowNodeType::Commit
                | WorkflowNodeType::Revert
                | WorkflowNodeType::Merge
        )
    }) {
        return true;
    }

    if workflow.mode == crate::WorkflowMode::Loop {
        let lowered = message.to_lowercase();
        return [
            "research",
            "experiment",
            "iteration",
            "loop",
            "hypothesis",
            "benchmark",
            "evaluate",
        ]
        .iter()
        .any(|token| lowered.contains(token));
    }

    false
}

fn normalize_workflow_vote_decision(
    decision: WorkflowVoteDecision,
    spec: &WorkspaceSpec,
    request: &WorkspaceTurnRequest,
    coordinator_decision: &CoordinatorWorkflowDecision,
    role: &RoleSpec,
) -> WorkflowVoteDecision {
    if matches!(
        decision,
        WorkflowVoteDecision::Approve | WorkflowVoteDecision::Reject
    ) {
        return decision;
    }

    let coordinator_role_id = spec
        .coordinator_role_id
        .as_ref()
        .or(spec.default_role_id.as_ref());
    if coordinator_decision.kind == CoordinatorDecisionKind::ProposeWorkflow
        && coordinator_role_id.is_some_and(|coordinator| coordinator == &role.id)
    {
        return WorkflowVoteDecision::Approve;
    }

    if coordinator_decision.kind == CoordinatorDecisionKind::ProposeWorkflow
        && role_participates_in_workflow(spec, &role.id)
    {
        return WorkflowVoteDecision::Approve;
    }

    if !should_propose_workflow_heuristically(spec, &request.message) {
        return WorkflowVoteDecision::Abstain;
    }

    let has_lane = resolve_workflow_candidates(spec, &request.message)
        .into_iter()
        .any(|candidate| candidate.role_ids.into_iter().any(|value| value == role.id));
    if has_lane {
        WorkflowVoteDecision::Approve
    } else {
        WorkflowVoteDecision::Abstain
    }
}

fn role_participates_in_workflow(spec: &WorkspaceSpec, role_id: &str) -> bool {
    let Some(workflow) = spec.workflow.as_ref() else {
        return false;
    };
    workflow.nodes.iter().any(|node| {
        node.role_id.as_deref() == Some(role_id)
            || node.reviewer_role_id.as_deref() == Some(role_id)
            || node
                .candidate_role_ids
                .as_ref()
                .is_some_and(|ids| ids.iter().any(|value| value == role_id))
    })
}

fn resolve_workflow_node_role_id(
    spec: &WorkspaceSpec,
    message: &str,
    node: &WorkflowNodeSpec,
) -> Option<String> {
    if let Some(role_id) = node.role_id.as_ref() {
        if spec.roles.iter().any(|role| role.id == *role_id) {
            return Some(role_id.clone());
        }
    }
    if let Some(role_id) = node.reviewer_role_id.as_ref() {
        if spec.roles.iter().any(|role| role.id == *role_id) {
            return Some(role_id.clone());
        }
    }
    if let Some(candidate_role_ids) = node.candidate_role_ids.as_ref() {
        return choose_best_candidate_role(spec, message, candidate_role_ids)
            .or_else(|| candidate_role_ids.first().cloned());
    }
    None
}

fn node_type_name(node_type: WorkflowNodeType) -> &'static str {
    match node_type {
        WorkflowNodeType::Announce => "announce",
        WorkflowNodeType::Assign => "assign",
        WorkflowNodeType::Claim => "claim",
        WorkflowNodeType::Shell => "shell",
        WorkflowNodeType::Evaluate => "evaluate",
        WorkflowNodeType::Review => "review",
        WorkflowNodeType::Branch => "branch",
        WorkflowNodeType::Loop => "loop",
        WorkflowNodeType::Artifact => "artifact",
        WorkflowNodeType::Commit => "commit",
        WorkflowNodeType::Revert => "revert",
        WorkflowNodeType::Merge => "merge",
        WorkflowNodeType::Complete => "complete",
    }
}

fn choose_best_candidate_role(
    spec: &WorkspaceSpec,
    message: &str,
    role_ids: &[String],
) -> Option<String> {
    let lowered = message.to_lowercase();
    let mut scored = role_ids
        .iter()
        .map(|role_id| {
            let score = spec
                .roles
                .iter()
                .find(|role| role.id == *role_id)
                .map(|role| score_role_for_message(role, &lowered))
                .unwrap_or(0);
            (role_id.clone(), score)
        })
        .collect::<Vec<_>>();
    scored.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    scored.into_iter().next().map(|entry| entry.0)
}

fn find_best_workflow_candidate_for_role(
    spec: &WorkspaceSpec,
    message: &str,
    role_id: &str,
) -> Option<WorkflowCandidate> {
    resolve_workflow_candidates(spec, message)
        .into_iter()
        .find(|candidate| candidate.role_ids.iter().any(|value| value == role_id))
}

fn guess_fallback_role_id(spec: &WorkspaceSpec, message: &str, fallback_role_id: &str) -> String {
    let lowered = message.to_lowercase();
    let mut best_role_id = fallback_role_id.to_string();
    let mut best_score = 0;

    for role in &spec.roles {
        let score = score_role_for_message(role, &lowered);
        if score > best_score {
            best_role_id = role.id.clone();
            best_score = score;
        }
    }

    best_role_id
}

fn score_role_for_message(role: &RoleSpec, lowered_message: &str) -> usize {
    let mut corpus_parts = vec![role.id.to_lowercase(), role.name.to_lowercase()];
    if let Some(description) = role.description.as_ref() {
        corpus_parts.push(description.to_lowercase());
    }
    corpus_parts.push(role.agent.description.to_lowercase());
    if let Some(output_root) = role.output_root.as_ref() {
        corpus_parts.push(output_root.to_lowercase());
    }
    if let Some(hints) = role_hints(&role.id) {
        corpus_parts.extend(hints.iter().map(|hint| hint.to_string()));
    }

    build_search_tokens(&corpus_parts.join(" "))
        .into_iter()
        .filter(|token| token.len() >= 3 && lowered_message.contains(token))
        .map(|token| if token.len() > 8 { 3 } else { 1 })
        .sum()
}

fn build_search_tokens(text: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    text.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '@' && ch != '-')
        .map(|value| value.trim().to_lowercase())
        .filter(|value| !value.is_empty())
        .filter(|value| seen.insert(value.clone()))
        .collect()
}

fn describe_role(role: &RoleSpec) -> String {
    let mut parts = vec![format!("@{} ({})", role.id, role.name)];
    if let Some(description) = role.description.as_ref() {
        parts.push(description.clone());
    }
    parts.push(role.agent.description.clone());
    if let Some(output_root) = role.output_root.as_ref() {
        parts.push(format!("output root: {}", output_root));
    }
    parts.join(" — ")
}

fn claim_decision_label(decision: ClaimDecision) -> &'static str {
    match decision {
        ClaimDecision::Claim => "claim",
        ClaimDecision::Support => "support",
        ClaimDecision::Decline => "decline",
    }
}

fn coordinator_role_id(spec: &WorkspaceSpec) -> String {
    resolve_coordinator_role_id(spec)
}

fn extract_json_object(raw_text: &str) -> Option<Value> {
    if let Ok(value) = serde_json::from_str::<Value>(raw_text.trim()) {
        if value.is_object() {
            return Some(value);
        }
    }

    let start = raw_text.find('{')?;
    let end = raw_text.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str::<Value>(&raw_text[start..=end]).ok()
}

fn normalize_claim_decision(
    raw_decision: Option<&str>,
    role: &RoleSpec,
    message: &str,
) -> ClaimDecision {
    match raw_decision
        .map(|value| value.trim().to_lowercase())
        .unwrap_or_default()
        .as_str()
    {
        "claim" => ClaimDecision::Claim,
        "support" => ClaimDecision::Support,
        "decline" => ClaimDecision::Decline,
        _ => {
            let lowered = message.to_lowercase();
            let score = score_role_for_message(role, &lowered);
            if score >= 3 {
                ClaimDecision::Claim
            } else if score > 0 {
                ClaimDecision::Support
            } else {
                ClaimDecision::Decline
            }
        }
    }
}

fn normalize_confidence(raw_confidence: Option<f64>, decision: ClaimDecision) -> f32 {
    raw_confidence
        .map(|value| value.clamp(0.0, 1.0) as f32)
        .unwrap_or(match decision {
            ClaimDecision::Claim => 0.9,
            ClaimDecision::Support => 0.6,
            ClaimDecision::Decline => 0.25,
        })
}

fn fallback_claim_rationale(role: &RoleSpec, decision: ClaimDecision, message: &str) -> String {
    match decision {
        ClaimDecision::Claim => format!(
            "@{} is a strong fit for this request based on the current message.",
            role.id
        ),
        ClaimDecision::Support => format!(
            "@{} can contribute, but another role may be the primary owner.",
            role.id
        ),
        ClaimDecision::Decline => format!(
            "@{} is not the clearest owner for \"{}\".",
            role.id, message
        ),
    }
}

fn extract_requested_output_path(message: &str) -> Option<String> {
    let normalized = message.replace("\r\n", "\n");
    let matcher = regex::Regex::new(
        r#"(?:^|[\s`"'“”‘’(（])([A-Za-z0-9._/-]+/[A-Za-z0-9._-]+\.(?:md|txt|json|ya?ml|csv|ts|tsx|js|jsx|rs|py|sh))(?:$|[\s`"'“”‘’)\]）。，、,:;!?])"#,
    )
    .expect("valid output path regex");
    if let Some(captures) = matcher.captures(&normalized) {
        return captures.get(1).map(|value| value.as_str().to_string());
    }

    let to_matcher = regex::Regex::new(
        r#"(?:\bto\b|保存到|写入到|写到|输出到|保存至)\s*([A-Za-z0-9._/-]+/[A-Za-z0-9._-]+\.(?:md|txt|json|ya?ml|csv|ts|tsx|js|jsx|rs|py|sh))(?:$|[\s`"'“”‘’)\]）。，、,:;!?])"#,
    )
    .expect("valid output path regex");
    to_matcher
        .captures(&normalized)
        .and_then(|captures| captures.get(1).map(|value| value.as_str().to_string()))
}

fn enforce_output_contract(
    mut instruction: String,
    requested_output_path: Option<&str>,
    artifact_hints: &[String],
) -> String {
    let mut rules = Vec::new();
    if let Some(path) = requested_output_path {
        rules.push(format!(
            "You must actually create or update `{}` in the workspace using the available file-editing tools.",
            path
        ));
        rules.push(format!(
            "Do not stop at a plan, pseudo-code, or shell snippet. Only report completion after `{}` exists with the requested content.",
            path
        ));
    }
    if !artifact_hints.is_empty() {
        rules.push(format!(
            "Treat these artifact targets as required deliverables for this turn: {}.",
            artifact_hints.join(", ")
        ));
    }
    if !rules.is_empty() {
        instruction.push_str("\n\nExecution requirements:\n");
        for rule in rules {
            instruction.push_str("- ");
            instruction.push_str(&rule);
            instruction.push('\n');
        }
    }
    instruction
}

fn role_hints(role_id: &str) -> Option<&'static [&'static str]> {
    match role_id {
        "pm" => Some(&["plan", "milestone", "scope", "coordination"]),
        "prd" => Some(&[
            "prd",
            "requirement",
            "requirements",
            "spec",
            "user story",
            "acceptance criteria",
        ]),
        "architect" => Some(&[
            "architecture",
            "design",
            "interface",
            "data model",
            "technical plan",
        ]),
        "coder" => Some(&["implement", "implementation", "code", "patch", "bug fix"]),
        "tester" => Some(&["test", "qa", "regression", "verification"]),
        "reviewer" => Some(&["review", "audit", "bug finding"]),
        "ceo" => Some(&["priority", "decision", "approval", "strategy"]),
        "finance" => Some(&[
            "finance",
            "monthly close",
            "cash",
            "invoice",
            "invoices",
            "subscription",
            "revenue",
            "kpi",
            "burn",
            "runway",
            "budget",
        ]),
        "tax" => Some(&["tax", "filing", "sales tax", "vat", "estimated tax"]),
        "admin" => Some(&["admin", "vendor", "operations", "sop", "checklist"]),
        "recruiter" => Some(&["recruit", "candidate", "hiring", "interview"]),
        "lead" => Some(&["research lead", "hypothesis", "question framing"]),
        "scout" => Some(&["research", "sources", "web", "compare", "brief"]),
        "experimenter" => Some(&["experiment", "metric", "measure", "test design"]),
        "critic" => Some(&["critique", "risk", "skeptic", "assumption"]),
        "shangshu" => Some(&["coordination", "governance", "routing"]),
        "zhongshu" => Some(&["brief", "mandate", "task order", "draft"]),
        "menxia" => Some(&["review", "challenge", "risk", "red team"]),
        "gongbu" => Some(&["implementation", "build", "execution", "tooling"]),
        "hubu" => Some(&["budget", "resources", "capacity", "allocation"]),
        "libu" => Some(&["documentation", "communication", "release notes"]),
        "xingbu" => Some(&["compliance", "safety", "policy"]),
        "bingbu" => Some(&["release", "incident", "operations", "rollout"]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        ClaimDecision, WorkspaceClaimResponse, WorkspaceInstanceParams,
        create_autoresearch_template, create_claude_workspace_profile,
        create_coding_studio_template, create_opc_solo_company_template, instantiate_workspace,
    };

    use super::*;

    #[test]
    fn coding_turn_prefers_prd_for_prd_request() {
        let template = create_coding_studio_template();
        let profile = create_claude_workspace_profile(None);
        let instance = WorkspaceInstanceParams {
            id: "workspace-1".to_string(),
            name: "Workspace".to_string(),
            cwd: None,
        };
        let spec = instantiate_workspace(&template, &instance, &profile);
        let plan = plan_workspace_turn(
            &spec,
            &crate::WorkspaceTurnRequest {
                message: "Create a PRD for group mentions with acceptance criteria".to_string(),
                visibility: None,
                max_assignments: None,
                prefer_role_id: None,
            },
        );

        assert_eq!(plan.assignments[0].role_id, "prd");
    }

    #[test]
    fn opc_turn_prefers_finance_for_monthly_close() {
        let template = create_opc_solo_company_template();
        let profile = create_claude_workspace_profile(None);
        let instance = WorkspaceInstanceParams {
            id: "workspace-2".to_string(),
            name: "Workspace".to_string(),
            cwd: None,
        };
        let spec = instantiate_workspace(&template, &instance, &profile);
        let plan = plan_workspace_turn(
            &spec,
            &crate::WorkspaceTurnRequest {
                message: "Prepare a monthly close checklist and review subscriptions".to_string(),
                visibility: None,
                max_assignments: None,
                prefer_role_id: None,
            },
        );

        assert_eq!(plan.assignments[0].role_id, "finance");
    }

    #[test]
    fn claim_responses_choose_owner_and_supporter() {
        let template = create_coding_studio_template();
        let profile = create_claude_workspace_profile(None);
        let instance = WorkspaceInstanceParams {
            id: "workspace-3".to_string(),
            name: "Workspace".to_string(),
            cwd: None,
        };
        let spec = instantiate_workspace(&template, &instance, &profile);
        let plan = build_plan_from_claim_responses(
            &spec,
            &crate::WorkspaceTurnRequest {
                message: "Create a PRD and outline implementation follow-up".to_string(),
                visibility: None,
                max_assignments: Some(2),
                prefer_role_id: None,
            },
            &[
                WorkspaceClaimResponse {
                    role_id: "prd".to_string(),
                    decision: ClaimDecision::Claim,
                    confidence: 0.9,
                    rationale: "PRD should own this first.".to_string(),
                    public_response: Some("@prd can take the brief.".to_string()),
                    proposed_instruction: Some(
                        "Write the PRD first, then call out dependencies for implementation."
                            .to_string(),
                    ),
                },
                WorkspaceClaimResponse {
                    role_id: "architect".to_string(),
                    decision: ClaimDecision::Support,
                    confidence: 0.7,
                    rationale: "I can support with technical framing.".to_string(),
                    public_response: Some(
                        "@architect can support the technical framing.".to_string(),
                    ),
                    proposed_instruction: Some(
                        "Prepare a short technical follow-up once the PRD is drafted.".to_string(),
                    ),
                },
            ],
        );

        assert_eq!(plan.assignments.len(), 2);
        assert_eq!(plan.assignments[0].role_id, "prd");
        assert_eq!(plan.assignments[1].role_id, "architect");
    }

    #[test]
    fn workflow_candidates_prefer_prd_lane_for_prd_request() {
        let template = create_coding_studio_template();
        let profile = create_claude_workspace_profile(None);
        let instance = WorkspaceInstanceParams {
            id: "workspace-4".to_string(),
            name: "Workspace".to_string(),
            cwd: None,
        };
        let spec = instantiate_workspace(&template, &instance, &profile);

        let role_ids = resolve_claim_candidate_role_ids(
            &spec,
            &crate::WorkspaceTurnRequest {
                message: "Write a PRD for group mentions and put it in 10-prd/group-mentions.md"
                    .to_string(),
                visibility: None,
                max_assignments: None,
                prefer_role_id: None,
            },
        );

        assert_eq!(role_ids.first().map(String::as_str), Some("prd"));
        assert_eq!(role_ids.len(), spec.roles.len());
    }

    #[test]
    fn workflow_candidates_prefer_scout_lane_for_research_brief() {
        let template = create_autoresearch_template();
        let profile = create_claude_workspace_profile(None);
        let instance = WorkspaceInstanceParams {
            id: "workspace-5".to_string(),
            name: "Workspace".to_string(),
            cwd: None,
        };
        let spec = instantiate_workspace(&template, &instance, &profile);

        let role_ids = resolve_claim_candidate_role_ids(
            &spec,
            &crate::WorkspaceTurnRequest {
                message: "Research how teams talk about group mentions and write a sourced brief"
                    .to_string(),
                visibility: None,
                max_assignments: None,
                prefer_role_id: None,
            },
        );

        assert_eq!(role_ids.first().map(String::as_str), Some("scout"));
        assert_eq!(role_ids.len(), spec.roles.len());
    }

    #[test]
    fn autoresearch_coordinator_proposes_workflow_for_loop_request() {
        let template = create_autoresearch_template();
        let profile = create_claude_workspace_profile(None);
        let instance = WorkspaceInstanceParams {
            id: "workspace-6".to_string(),
            name: "Workspace".to_string(),
            cwd: None,
        };
        let spec = instantiate_workspace(&template, &instance, &profile);

        let decision = decide_coordinator_action(
            &spec,
            &crate::WorkspaceTurnRequest {
                message: "Run an iterative autoresearch loop with experiment evaluation and keep/discard decisions.".to_string(),
                visibility: None,
                max_assignments: None,
                prefer_role_id: None,
            },
        );

        assert_eq!(
            decision.kind,
            crate::CoordinatorDecisionKind::ProposeWorkflow
        );
    }

    #[test]
    fn workflow_entry_plan_starts_autoresearch_at_lead() {
        let template = create_autoresearch_template();
        let profile = create_claude_workspace_profile(None);
        let instance = WorkspaceInstanceParams {
            id: "workspace-7".to_string(),
            name: "Workspace".to_string(),
            cwd: None,
        };
        let spec = instantiate_workspace(&template, &instance, &profile);

        let plan = build_workflow_entry_plan(
            &spec,
            &crate::WorkspaceTurnRequest {
                message: "Start the autoresearch workflow for mention semantics.".to_string(),
                visibility: None,
                max_assignments: None,
                prefer_role_id: None,
            },
        );

        assert_eq!(plan.assignments.len(), 1);
        assert_eq!(plan.assignments[0].role_id, "lead");
        assert_eq!(
            plan.assignments[0].workflow_node_id.as_deref(),
            Some("frame_hypothesis")
        );
    }

    #[test]
    fn workflow_vote_synthesizes_approval_for_workflow_participants() {
        let template = create_autoresearch_template();
        let profile = create_claude_workspace_profile(None);
        let instance = WorkspaceInstanceParams {
            id: "workspace-8".to_string(),
            name: "Workspace".to_string(),
            cwd: None,
        };
        let spec = instantiate_workspace(&template, &instance, &profile);
        let request = crate::WorkspaceTurnRequest {
            message: "Run an iterative autoresearch loop with keep/discard control.".to_string(),
            visibility: None,
            max_assignments: None,
            prefer_role_id: None,
        };
        let coordinator_decision = decide_coordinator_action(&spec, &request);
        let role = spec
            .roles
            .iter()
            .find(|role| role.id == "scout")
            .expect("scout role should exist");

        let response =
            synthesize_workflow_vote_response(&spec, &request, &coordinator_decision, role);

        assert_eq!(response.decision, WorkflowVoteDecision::Approve);
    }
}
