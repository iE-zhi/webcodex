//! 有界回合进展账本；保存来源、计数和状态，不保存推理、工具载荷或控制令牌。
use crate::model::{Observation, Request, Task};
use serde::{Deserialize, Serialize};

/// 每任务最多六十四个变化事件；重复心跳仅更新 observed_at，不扩张账本。
const MAX_EVENTS: usize = 64;
/// 列表只带最近八条，避免三十二个任务的每次调度传输完整历史。
pub(crate) const VIEW_EVENTS: usize = 8;

/// 可持久化的公开回合变化；按事务提交顺序排列，时间均为服务端 Unix 毫秒。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProgressEvent {
    /// 服务端观察或控制操作提交时间；不采用网页时钟。
    pub at: i64,
    /// 固定事件类型，不允许网页提供任意日志正文。
    pub kind: String,
    /// 操作结束后的任务控制版本。
    pub generation: u64,
    /// 同一原生回合内已交出的工作序号；浏览器轮次不冒充原生序号。
    pub sequence: u32,
    /// 提供方 source turn；缺失时为空，不能据此推断别的回合。
    pub source_turn: String,
    /// 同一文档内的活动变化计数，文档变化后可重新从零开始。
    pub revision: u64,
    /// 确切浏览器主文档 ID；仅用于关联观察，不授予发送权限。
    pub document_id: String,
    /// 当前用户消息身份，不含正文；人工新请求会更换控制版本。
    pub user_id: String,
    /// 生成中或有后端工具工作，不含工具名称、参数及私有输出。
    pub busy: bool,
    /// 仅表示当前在等用户，不包含审批内容。
    pub approval: bool,
    /// 固定任务状态及原因，反映该操作提交后的结果。
    pub state: String,
    /// 固定安全原因码，空串表示没有异常或等待原因。
    pub reason: String,
}

/// 从公开观测复制固定元数据；不保留原 Observation 或其中正文的借用。
/// # 参数
/// - `page`：已校验账户绑定的公开状态，由调用方在保存前再次验证。
/// - `kind`：当前服务端操作名，只接受模块内固定调用方。
fn sample(page: &Observation, kind: &str) -> ProgressEvent {
    ProgressEvent {
        kind: kind.into(),
        source_turn: page.activity.source_turn.clone(),
        revision: page.activity.revision,
        document_id: page.document_id.clone(),
        user_id: page.user_id.clone(),
        busy: page.busy || page.tools_busy,
        approval: page.approval,
        ..ProgressEvent::default()
    }
}

/// 只抽取本次请求的少量进展字段，避免为记录事件复制完整公开回复。
/// # 参数
/// - `request`：尚未移动的已反序列化操作；权限及字段校验仍由原操作执行。
pub(crate) fn capture(request: &Request) -> ProgressEvent {
    match request {
        Request::Start { page, .. } => sample(page, "start"),
        Request::Update { page, .. } => sample(page, "update"),
        Request::Observe { page, .. } => sample(page, "observe"),
        Request::Prepare { page, .. } => sample(page, "prepare"),
        Request::Authorize { page, .. } => sample(page, "authorize"),
        Request::NotSent { page, .. } => sample(page, "not_sent"),
        Request::Rebind { page, .. } => sample(page, "rebind"),
        Request::Repair {
            page: Some(page), ..
        } => sample(page, "repair"),
        Request::Repair { .. } => ProgressEvent {
            kind: "repair".into(),
            ..ProgressEvent::default()
        },
        Request::Pause { .. } => ProgressEvent {
            kind: "pause".into(),
            ..ProgressEvent::default()
        },
        Request::Queue { .. } | Request::Dequeue { .. } => ProgressEvent {
            kind: "queue".into(),
            ..ProgressEvent::default()
        },
        _ => ProgressEvent::default(),
    }
}

/// 在请求被移动前复制小型来源回执，其他文本和页面对象仍只由原操作持有。
/// # 参数
/// - `request`：当前请求；只有完整页面操作可附带公开来源回执。
pub(crate) fn source_proof(request: &Request) -> Option<crate::model::SourceProof> {
    match request {
        Request::Start { page, .. }
        | Request::Update { page, .. }
        | Request::Observe { page, .. }
        | Request::Prepare { page, .. }
        | Request::Authorize { page, .. }
        | Request::NotSent { page, .. }
        | Request::Rebind { page, .. }
        | Request::Claim { page, .. }
        | Request::Repair {
            page: Some(page), ..
        } => page.source_proof.clone(),
        _ => None,
    }
}

/// 将变化与任务状态原子保存；无页面的控制操作继承最近来源，重复心跳不插入新事件。
/// # 参数
/// - `task`：当前事务内任务，原子提交责任在调用方。
/// - `event`：受控元数据；无页面身份时继承最近已验证的回合来源。
/// - `now`：服务端 Unix 毫秒时间，不影响任务预算或授权。
pub(crate) fn append(task: &mut Task, mut event: ProgressEvent, now: i64) {
    if event.kind.is_empty() {
        return;
    }
    if event.user_id.is_empty() {
        if let Some(prior) = task.journal.last() {
            event.source_turn.clone_from(&prior.source_turn);
            event.document_id.clone_from(&prior.document_id);
            event.user_id.clone_from(&prior.user_id);
            event.revision = prior.revision;
            event.busy = prior.busy;
            event.approval = prior.approval;
        }
    }
    event.generation = task.generation;
    event.sequence = task.native.sequence;
    event.state.clone_from(&task.state);
    event.reason.clone_from(&task.reason);
    if task.journal.last().is_some_and(|last| {
        let mut comparable = event.clone();
        comparable.at = last.at;
        *last == comparable
    }) {
        return;
    }
    event.at = now;
    if task.journal.len() >= MAX_EVENTS {
        task.journal.remove(0);
    }
    task.journal.push(event);
}
