//! 人工输入、完整工作流和当前 ChatGPT 规划的持久化队列；只在账本事务内修改，不调用模型 API。
use crate::model::{Delivery, Mode, Observation, Pending, QueuedInput, Task, MAX_OBJECTIVE};
use crate::{decision, Error};
use serde::Deserialize;
use uuid::Uuid;

/// 每任务最多三十二条排队输入，防止无界积压。
const MAX_ENTRIES: usize = 32;
/// 每任务队列正文合计最多 64 KiB；完整工作流会在第一阶段包含全部阶段。
const MAX_QUEUE_BYTES: usize = 64 * 1024;
/// 一份规划最多十二阶段、合计 24 KiB；至少两阶段，首阶段完整实现，后续为验证检查点。
const MAX_STAGES: usize = 12;
/// 规划阶段的总字节上限，不含首次投递时的固定控制指令。
const MAX_PLAN_BYTES: usize = 24 * 1024;

/// 当前 ChatGPT 返回的规划载荷；只有匹配任务及版本的公开回复可解析，不授予额外执行权限。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    /// 二至十二个非空阶段，首阶段包含原请求全部要求，不允许隐去实现需求。
    stages: Vec<String>,
}

/// 验证排队条目总量后插入；人工输入及规划请求先于阶段检查点，组内保持 FIFO。
/// # 参数
/// - `task`：事务内任务，失败时外层事务不得保存局部修改。
/// - `entries`：本次已校验的新条目，所有权交给队列。
/// # Errors
/// 总条目或字节数超限返回 Capacity，不修改原队列。
fn insert(task: &mut Task, entries: Vec<QueuedInput>) -> Result<(), Error> {
    let bytes: usize = task
        .queue
        .iter()
        .chain(entries.iter())
        .map(|entry| entry.text.len())
        .sum();
    if task.queue.len() + entries.len() > MAX_ENTRIES || bytes > MAX_QUEUE_BYTES {
        return Err(Error::Capacity);
    }
    for entry in entries {
        let at = if entry.kind == "stage" {
            task.queue.len()
        } else {
            task.queue
                .iter()
                .position(|queued| queued.kind == "stage")
                .unwrap_or(task.queue.len())
        };
        task.queue.insert(at, entry);
    }
    Ok(())
}

/// 生成工作流条目；第一次投递带完整工作流，后续仅作为原任务验证/改进检查点。
/// # 参数
/// - `stages`：二至十二个完整阶段，单项最多八千字节，总量最多 24 KiB。
/// - `trigger`：已校验的 after_turn 或 finish。
/// # Errors
/// 阶段为空、数量或长度超限返回 Invalid，不修改任务。
fn workflow(stages: &[String], trigger: &str) -> Result<Vec<QueuedInput>, Error> {
    if !(2..=MAX_STAGES).contains(&stages.len())
        || stages
            .iter()
            .any(|stage| stage.trim().is_empty() || stage.len() > MAX_OBJECTIVE)
        || stages.iter().map(String::len).sum::<usize>() > MAX_PLAN_BYTES
    {
        return Err(Error::Invalid);
    }
    let all = stages
        .iter()
        .enumerate()
        .map(|(index, stage)| format!("{}. {stage}", index + 1))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(stages.iter().enumerate().map(|(index, stage)| QueuedInput {
        id: Uuid::new_v4().to_string(), kind: "stage".into(), trigger: trigger.into(), delivery: "queued".into(),
        text: if index == 0 {
            format!("完整工作流：\n{all}\n第一阶段必须完成原始请求的完整实现及全部约束，不能将实现要求推迟到后续检查点。\n当前阶段：{stage}")
        } else {
            format!("原请求的后续检查点 {}：{stage}\n保留完整原始要求和人工修正；发现问题就修复并复验，不新增无关工作。", index + 1)
        },
    }).collect())
}

/// 添加人工消息、工作流或规划请求；规划由当前聊天完成，不开辅助聊天或访问决策 API。
/// # 参数
/// - `task`：账户/版本/绑定已校验的事务内任务。
/// - `kind`：message、workflow 或 plan。
/// - `text`：message 的非空正文，最多八千字节；其他类型必须为空。
/// - `stages`：workflow 的完整阶段列表；其他类型必须为空。
/// - `trigger`：after_turn 或 finish；finish 要求关联 Session。
/// # Errors
/// 类型、内容、触发器不合法返回 Invalid，容量耗尽返回 Capacity。
pub(crate) fn add(
    task: &mut Task,
    kind: &str,
    text: &str,
    stages: &[String],
    trigger: &str,
) -> Result<(), Error> {
    if !["after_turn", "finish"].contains(&trigger) {
        return Err(Error::Invalid);
    }
    let entries = match kind {
        "message"
            if !text.trim().is_empty() && text.len() <= MAX_OBJECTIVE && stages.is_empty() =>
        {
            vec![QueuedInput {
                id: Uuid::new_v4().to_string(),
                kind: kind.into(),
                text: text.into(),
                trigger: trigger.into(),
                delivery: "queued".into(),
            }]
        }
        "workflow" if text.is_empty() => workflow(stages, trigger)?,
        "plan" if text.is_empty() && stages.is_empty() => vec![QueuedInput {
            id: Uuid::new_v4().to_string(),
            kind: kind.into(),
            text: "规划当前原始请求的完整实现与验证工作流".into(),
            trigger: trigger.into(),
            delivery: "queued".into(),
        }],
        _ => return Err(Error::Invalid),
    };
    insert(task, entries)
}

/// 队列变更撤销尚未发送的旧内容；已授权消息保持身份和版本，仅允许回执对账。
/// # 参数
/// - `task`：队列已更新的事务内任务。
/// - `now`：Unix 毫秒时间，不重置轮数或授权期限。
pub(crate) fn invalidate_preparation(task: &mut Task, now: i64) {
    if task.delivery.authorized_at.is_some() || task.native.held {
        return;
    }
    task.generation += 1;
    task.protocol_armed = false;
    task.pending = None;
    task.delivery = Delivery::default();
    if matches!(task.state.as_str(), "ready" | "prepared") {
        task.state = "running".into();
    }
    task.changed_at = now;
}

/// 只在原回合最终完成且满足触发器时预留队首；人工队列优先于 Goal 自动推进。
/// # 参数
/// - `task`：当前任务，不能已授权、暂停、完成、过期或耗尽轮数。
/// - `page`：同一用户消息的完整观测；工具、审批、草稿和停止均阻止预留。
/// - `now`：Unix 毫秒时间；保留原有最短发送间隔。
/// # 返回
/// 已预留或因上下文冲突暂停时返回 true；false 不消费队列，任何返回值都不授予发送权。
pub(crate) fn reserve(task: &mut Task, page: &Observation, now: i64) -> bool {
    let Some(entry) = task.queue.iter().find(|entry| entry.delivery == "queued") else {
        return false;
    };
    if task.native.held
        || task.queue.iter().any(|entry| entry.delivery == "uncertain")
        || !matches!(task.state.as_str(), "running" | "watching" | "ready")
        || task
            .pending
            .as_ref()
            .is_some_and(|pending| pending.queue_id.is_some())
        || task.delivery.authorized_at.is_some()
        || now >= task.expires_at
        || task.rounds >= task.options.limit
        || page.busy
        || page.draft
        || page.tools_busy
        || page.approval
        || page.blocked
        || page.activity.stop_requested
        || page.activity.rate_limited
        || page.user_id != task.expected_user
        || !page
            .reply
            .as_ref()
            .is_some_and(|reply| reply.terminal && reply.user_id == task.expected_user)
    {
        return false;
    }
    if entry.trigger == "finish" && !task.recovery.finish_seen {
        if task.native.delivered {
            return false;
        }
        if let Some(reply) = &page.reply {
            if crate::store::record(task, &reply.id, "assistant", &reply.text).is_err() {
                task.pause("context_limit", now);
                return true;
            }
        }
        // Continue 没有 Goal 的首轮提示；finish 队列先单独关联当前聊天，不提前消费队列项。
        task.pending = Some(Pending { id: Uuid::new_v4().to_string(), queue_id: None, wire: None,
            text: "先关联当前主聊天与原任务。关联后用原生 finish 报告本轮实际进度并领取已排队工作，不另开聊天，不自行发起新用户消息。".into() });
        task.delivery = Delivery::default();
        task.last_reply = page.reply.as_ref().map(|reply| reply.id.clone());
        task.state = "ready".into();
        task.reason = "native_finish_required".into();
        task.changed_at = now;
        return true;
    }
    let entry = entry.clone();
    if let Some(reply) = &page.reply {
        if let Err(error) = crate::store::record(task, &reply.id, "assistant", &reply.text) {
            task.pause(
                if matches!(error, Error::ContextLimit) {
                    "context_limit"
                } else {
                    "page_changed"
                },
                now,
            );
            return true;
        }
    }
    let text = if entry.kind == "plan" {
        plan_instruction(task)
    } else {
        entry.text.clone()
    };
    let text = if task.options.mode == Mode::Continue {
        text
    } else {
        format!("{text}\n\n{}", decision::instruction(task))
    };
    task.pending = Some(Pending {
        id: Uuid::new_v4().to_string(),
        text,
        queue_id: Some(entry.id.clone()),
        wire: None,
    });
    task.delivery = Delivery::default();
    task.last_reply = page.reply.as_ref().map(|reply| reply.id.clone());
    task.state = "ready".into();
    task.reason = "queued_input".into();
    task.changed_at = now;
    true
}

/// 生成仅供当前聊天的完整规划协议，浏览器投递与原生工具结果共用。
/// # 参数
/// - `task`：当前任务与版本；规划只产生原请求的阶段，不授予其他执行权。
pub(crate) fn plan_instruction(task: &Task) -> String {
    format!("依据本聊天原始请求和全部人工修正规划 2～12 个阶段。第一阶段包含完整实现、全部要求及执行方法；后续只做验证和改进检查点。不得遗漏或缩小原请求，不调用决策 API，不创建辅助聊天。输出以下精确边界及 JSON，JSON 总长不超过 24 KiB；原生 finish 将此完整规划放入 summary：\n[[WEBCODEX_PLAN:{}:{}]]\n{{\"stages\":[\"完整实现工作流\",\"验证工作流\"]}}\n[[/WEBCODEX_PLAN]]", task.id, task.generation)
}

/// 确切投递回执才移除对应条目；人工原文单独返回给账本，自动控制提示不能变成人工约束。
/// # 参数
/// - `task`：已由调用方校验确切用户消息回执的任务，pending 仍须存在。
/// # 返回
/// 人工 message 返回稳定条目 ID 和原文；阶段/规划或非队列投递返回 None。
pub(crate) fn acknowledge(task: &mut Task) -> Option<(String, String)> {
    let id = task.pending.as_ref()?.queue_id.as_ref()?;
    let at = task.queue.iter().position(|entry| &entry.id == id)?;
    let entry = task.queue.remove(at);
    task.planning = entry.kind == "plan";
    task.planning_trigger = task.planning.then_some(entry.trigger);
    (entry.kind == "message").then_some((format!("queue:{}", entry.id), entry.text))
}

/// 只解析当前任务/版本规划边界中的 JSON，成功后把整个工作流原子排队。
/// # 参数
/// - `task`：已确认收到规划请求、尚未消费其终结回复的任务。
/// - `reply`：当前聊天的完整公开最终回复，不读取私有推理。
/// # Errors
/// 边界、JSON、阶段数或容量不合法时返回固定错误，由调用方暂停供用户检查。
pub(crate) fn accept_plan(task: &mut Task, reply: &str) -> Result<(), Error> {
    let start = format!("[[WEBCODEX_PLAN:{}:{}]]", task.id, task.generation);
    let (_, body) = reply.split_once(&start).ok_or(Error::Invalid)?;
    let (body, _) = body
        .split_once("[[/WEBCODEX_PLAN]]")
        .ok_or(Error::Invalid)?;
    if reply.matches(&start).count() != 1 || body.len() > MAX_PLAN_BYTES {
        return Err(Error::Invalid);
    }
    let plan: Plan = serde_json::from_str(body.trim()).map_err(|_| Error::Invalid)?;
    let trigger = task
        .planning_trigger
        .as_deref()
        .unwrap_or(&task.options.policy.trigger);
    let entries = workflow(&plan.stages, trigger)?;
    insert(task, entries)?;
    task.planning = false;
    task.planning_trigger = None;
    Ok(())
}
