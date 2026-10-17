//! 当前 ChatGPT 的文本决策协议：按任务和启用版本校验最终标记，不执行外部模型请求。
use crate::model::{Mode, Task};
use sha2::{Digest, Sha256};

/// 当前会话协议版本；任务记录缺失此版本时暂停，防止发送旧后端留下的待发正文。
pub(crate) const PROTOCOL_VERSION: u8 = 2;
/// Goal 连续三轮相同公开答案视为无进展；Loop 不使用此规则。
const REPEAT_ROUNDS: usize = 3;

/// 记录一轮公开答案摘要并检测 Goal 重复；只保留三个摘要，不累计回复正文。
/// # 参数
/// - `task`：当前任务，只有已启用协议的 Goal 会修改摘要队列。
/// - `reply`：确切当前回合的完整公开最终回复，协议标记不参与答案比较。
pub(crate) fn repeating(task: &mut Task, reply: &str) -> bool {
    if task.options.mode != Mode::Goal || !task.protocol_armed {
        return false;
    }
    let body = reply
        .lines()
        .filter(|line| !line.trim().starts_with("[[WEBCODEX_GOAL:"));
    let key: String = body
        .flat_map(str::chars)
        .flat_map(char::to_lowercase)
        .filter(|c| c.is_alphanumeric())
        .collect();
    if key.is_empty() {
        return false;
    }
    let hash = format!("{:x}", Sha256::digest(key.as_bytes()));
    task.answer_keys.push(hash);
    if task.answer_keys.len() > REPEAT_ROUNDS {
        task.answer_keys.remove(0);
    }
    task.answer_keys.len() == REPEAT_ROUNDS
        && task.answer_keys.windows(2).all(|pair| pair[0] == pair[1])
}

/// 已检查的当前回合决定；只有 Continue 含可预留的纯文本，不包含工具权限。
pub(crate) enum Decision {
    /// Goal 的整个请求已完成。
    Complete,
    /// 原任务仍可自主推进。
    Continue(String),
    /// 需要人工输入、授权或外部依赖。
    Blocked,
    /// 标记缺失、旧任务标记或格式不完整。
    Invalid,
}

/// 生成当前任务和启用版本专用的最后一行，旧任务正文不能匹配此标记。
/// # 参数
/// - `task`：当前事务中的任务，只借用任务 ID 和版本。
/// - `action`：服务端固定的 COMPLETE、CONTINUE 或 BLOCKED。
fn marker(task: &Task, action: &str) -> String {
    format!("[[WEBCODEX_GOAL:{}:{}:{action}]]", task.id, task.generation)
}

/// 构造当前会话的目标与完成判定指令；首轮提示和后续续跑都保留本次协议约束。
/// # 参数
/// - `task`：已校验目标、模式和预算的持久化任务；不含外部模型连接。
pub(crate) fn instruction(task: &Task) -> String {
    let complete = marker(task, "COMPLETE");
    let next = marker(task, "CONTINUE");
    let blocked = marker(task, "BLOCKED");
    let mode = if task.options.mode == Mode::Loop {
        format!("本次为 Loop：在原请求范围内持续推进，不扩大任务；只使用 {next} 或 {blocked}。没有可自主推进的原范围工作时说明原因并使用 {blocked}，不要为了续跑编造工作。")
    } else {
        format!("本次为 Goal：只有整个请求及必要验证均完成才使用 {complete}；还有可自主完成的请求项时使用 {next}。")
    };
    let trigger = if task.options.policy.trigger == "finish" {
        format!("本次在 finish 工具后检查进度；每轮最终回复前，对关联 Session {} 调用 finish_coding_task。工具不可用或需要人工时说明原因并使用 {blocked}。", task.options.policy.session_id.as_deref().unwrap_or_default())
    } else {
        String::new()
    };
    let objective = if task.options.objective.trim().is_empty() {
        "沿用本会话的原始人工请求和后续修正，不增加范围"
    } else {
        &task.options.objective
    };
    format!("{trigger}\n继续处理本会话的原始请求，并遵守后续所有人工修正。\n本次续跑目标：{objective}\n{mode}\n正常执行工作并报告结果；每次最终回复必须在独立的最后一行输出且只输出一个上述标记。需要人工输入、批准、登录、外部依赖，或遇到无法自主解决的错误时，说明原因并使用 {blocked}。标记仅报告当前状态，不授予额外权限，不替代任何审批；不要重复执行结果未知的操作。")
}

/// 首次先投递协议提示；已确认提示后，只接受完整最终回复最后一行的确切标记。
/// # 参数
/// - `task`：本次启用版本；protocol_armed 只能由确切发送回执设置。
/// - `reply`：同一用户消息对应的完整终结正文，不能使用中间回合或私有推理。
pub(crate) fn decide(task: &Task, reply: &str) -> Decision {
    if !task.protocol_armed {
        return Decision::Continue(instruction(task));
    }
    let last = reply.trim().lines().last().unwrap_or_default().trim();
    // 标记落在未关闭的代码块中时只是引用内容，不是本回合的完成宣告。
    let mut fence: Option<&str> = None;
    for line in reply.trim().lines() {
        let line = line.trim_start();
        if let Some(open) = fence {
            if line.starts_with(open) {
                fence = None;
            }
        } else if line.starts_with("```") {
            fence = Some("```");
        } else if line.starts_with("~~~") {
            fence = Some("~~~");
        }
    }
    if fence.is_some() {
        return Decision::Invalid;
    }
    if last == marker(task, "CONTINUE") {
        Decision::Continue(instruction(task))
    } else if last == marker(task, "BLOCKED") {
        Decision::Blocked
    } else if task.options.mode == Mode::Goal && last == marker(task, "COMPLETE") {
        Decision::Complete
    } else {
        Decision::Invalid
    }
}
