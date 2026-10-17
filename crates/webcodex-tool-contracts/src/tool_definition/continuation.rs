//! 当前 ChatGPT 原生续跑的工具声明；只改变已授权任务控制状态，不执行工程工具。
use super::{adaptive_runtime_direct, def, model_spec, ToolDefinition, ToolDirectReason};
use crate::metadata::{ToolPathHint, ToolRisk, SESSION_COLLABORATE, TOOL_PROVIDER_CONTROL};

/// 原生续跑在普通工作流后的模型发现顺序，不扩大权限。
const DIRECT_RANK: u16 = 11;

/// 唯一注册组；审计只记录身份和状态，不记录令牌及进度正文。
pub(super) const DEFINITIONS: &[ToolDefinition] = &[
    adaptive_runtime_direct(model_spec(def(
        "update_task_continuation",
        super::ToolAuditPolicy::typed_fields(&[
            super::ToolAuditResultField::value("task_id"),
            super::ToolAuditResultField::value("sequence"),
            super::ToolAuditResultField::value("released"),
            super::ToolAuditResultField::value("waiting"),
            super::ToolAuditResultField::value("error_kind"),
        ]),
        super::ToolVisibility::ModelVisible, "session", None, TOOL_PROVIDER_CONTROL,
        super::ToolSemanticContract {
            effect: super::ToolEffect::Mutate, risk: ToolRisk::SessionCollaborate,
            approval: super::ToolApprovalPolicy::None, idempotency: super::ToolIdempotency::FencedReplay,
        },
        Some(SESSION_COLLABORATE), true, ToolPathHint::None, false, false, super::ToolSessionEvidencePolicy::NONE,
    ).with_activity(super::ToolActivityPresentation::Transport, super::ToolActivityInteraction::NonMeaningful), "Control an explicitly enabled continuation using only the exact task_id, generation and control_token issued in this current main chat. Never guess, forward or reuse another chat's token. Reuse this task's authorized Workflow Session; if none exists, create one with normal start_session first. First bind returns a source challenge: emit exactly that single line as public commentary in this main chat, then bind again after the browser confirms it. A worker or tool result cannot confirm the main chat. Declare all other dependent Session IDs in related_session_ids (maximum 15); they stay associated and block completion while work or approval remains. At each real checkpoint perform the original requested validation, then finish with CURRENT ChatGPT's own continue/complete/blocked decision and public summary. No decision API or helper chat. Follow returned instructions in this same turn until released=true; never post another user message. waiting=true permits waiting for existing work or human input only, with no busy polling or new stage. Keep request_id unchanged when retrying one logical check. A repeated sequence is the same work: reconcile committed or unknown actions before continuing. This grants no Project, shell or approval authority."),
        DIRECT_RANK, ToolDirectReason::CoreWorkflow),
];
