//! 浏览器投递的准备、发送前授权和确切未发送恢复；调用方必须在同一账本事务内保存状态。
use crate::model::{Delivery, Dispatch, Observation, Task, PREPARED_MS};
use crate::{recovery, store::user_hash, Error};

/// 每条消息仅允许一次有明确未发送证明的自动重试，未知投递不适用。
const MAX_NOT_SENT_RETRIES: u32 = 1;

/// 校验当前页面仍属于待发消息的原回合，且没有人工输入、停止或后台工作。
/// # 参数
/// - `task`：已通过账户、版本和浏览器绑定校验的任务。
/// - `page`：刚取得的完整页面状态，不得包含截断正文。
fn current(task: &Task, page: &Observation) -> bool {
    !page.busy
        && !page.draft
        && !page.tools_busy
        && !page.approval
        && !page.blocked
        && !page.activity.stop_requested
        && !page.activity.rate_limited
        && page.user_id == task.expected_user
        && user_hash(&page.user_text) == task.expected_user_hash
        && if task.recovery.pending {
            recovery::fingerprint(page) == task.recovery.fingerprint
                && !page.reply.as_ref().is_some_and(|reply| reply.terminal)
        } else {
            page.reply.as_ref().is_some_and(|reply| {
                reply.terminal
                    && Some(&reply.id) == task.last_reply.as_ref()
                    && reply.user_id == task.expected_user
                    && task.messages.iter().any(|message| {
                        message.role == "assistant"
                            && message.id == reply.id
                            && message.content == reply.text
                    })
            })
        }
}

/// 读取已准备的正文；只提供内容，不授予点击权，授权前重读同一内容是安全的。
/// # 参数
/// - `task`：待发消息仍未获授权的任务。
/// - `id`：客户端读到的待发消息 ID，必须完全匹配。
/// - `page`：当前完整页面观测。
/// - `now`：Unix 毫秒时间，不得超过授权或准备期限。
/// # Errors
/// 状态、预算、页面或 ID 变化返回 Conflict，原任务不变。
pub(crate) fn prepare(
    task: &mut Task,
    id: &str,
    page: &Observation,
    now: i64,
) -> Result<Option<Dispatch>, Error> {
    if !matches!(task.state.as_str(), "ready" | "prepared")
        || now < task.next_at
        || now >= task.expires_at
        || task.rounds >= task.options.limit
        || task.delivery.authorized_at.is_some()
    {
        return Err(Error::Conflict);
    }
    if !current(task, page) {
        if page.busy || page.draft || page.tools_busy || page.approval || page.activity.rate_limited
        {
            return Err(Error::Conflict);
        }
        task.pause("page_changed", now);
        return Ok(None);
    }
    crate::native::seal(task);
    let pending = task.pending.as_ref().ok_or(Error::Conflict)?;
    if pending.id != id {
        return Err(Error::Conflict);
    }
    let dispatch = Dispatch {
        id: pending.id.clone(),
        text: pending.wire_text(),
    };
    // 重读不延长准备期限，防止陈旧客户端持续占住待发消息。
    if task.state == "ready" {
        task.delivery.prepared_at = now;
        task.changed_at = now;
        task.state = "prepared".into();
    }
    Ok(Some(dispatch))
}

/// 将一次发送授权原子提交；之后即使响应丢失，也只允许确切回执对账。
/// # 参数
/// - `task`：当前事务内已准备的任务副本。
/// - `id`：本次准备的消息 ID。
/// - `page`：尚未插入发送正文时取得的最新页面状态。
/// - `now`：Unix 毫秒时间；准备超过三十秒即拒绝。
/// # Errors
/// 准备、页面、预算或授权状态变化返回 Conflict，不返还可重用发送权。
pub(crate) fn authorize(
    task: &mut Task,
    id: &str,
    page: &Observation,
    now: i64,
) -> Result<(), Error> {
    if task.state != "prepared"
        || task.delivery.authorized_at.is_some()
        || now - task.delivery.prepared_at >= PREPARED_MS
        || now >= task.expires_at
        || task.rounds >= task.options.limit
        || task.pending.as_ref().is_none_or(|pending| pending.id != id)
        || !current(task, page)
    {
        return Err(Error::Conflict);
    }
    task.delivery.authorized_at = Some(now);
    task.delivery.generation = task.generation;
    task.state = "dispatching".into();
    task.changed_at = now;
    task.rounds += 1;
    Ok(())
}

/// 接收页面未点击且已清除自有草稿的明确反馈；无法取得页面或点击结果未知时不得调用。
/// # 参数
/// - `task`：本次已获授权、尚无回执的任务副本。
/// - `id`：被证明未发送的确切消息 ID。
/// - `page`：清理后的完整页面，原用户和最终回复必须仍一致。
/// - `now`：Unix 毫秒时间；重试仍受原期限及发送间隔约束。
/// # Errors
/// 消息或授权不符返回 Conflict；页面变化/重试耗尽则保留暂停记录，不再投递。
pub(crate) fn not_sent(
    task: &mut Task,
    id: &str,
    page: &Observation,
    now: i64,
) -> Result<(), Error> {
    if !matches!(task.state.as_str(), "dispatching" | "uncertain" | "paused")
        || task.delivery.authorized_at.is_none()
        || task.pending.as_ref().is_none_or(|pending| pending.id != id)
    {
        return Err(Error::Conflict);
    }
    let unchanged = current(task, page);
    if unchanged {
        if let Some(id) = task
            .pending
            .as_ref()
            .and_then(|pending| pending.queue_id.as_ref())
        {
            if let Some(entry) = task
                .queue
                .iter_mut()
                .find(|entry| &entry.id == id && entry.delivery == "uncertain")
            {
                // 仅这条确切未发送证明可以恢复旧队列项；其他未知项保持未知。
                entry.delivery = "queued".into();
            }
        }
    }
    if task.state == "paused" && unchanged {
        // 人工暂停后晚到的确切未点击证明只清理债务，不能重新启用任务。
        task.pending = None;
        task.delivery = Delivery::default();
    } else if !unchanged || now >= task.expires_at {
        task.pause("page_changed", now);
    } else if task.delivery.generation != task.generation {
        // 目标更新已撤销旧正文；明确未点击后清除旧债务，下一次观察按新目标重新决策。
        task.pending = None;
        task.delivery = Delivery::default();
        task.last_reply = None;
        task.state = "running".into();
        task.reason.clear();
    } else if task.delivery.not_sent >= MAX_NOT_SENT_RETRIES {
        task.pause("send_failed", now);
    } else {
        task.delivery = Delivery {
            not_sent: task.delivery.not_sent + 1,
            ..Delivery::default()
        };
        if let Some(pending) = task.pending.as_mut() {
            // 已取消的页面发送 ID 不得复用；旧证明或旧页面不能影响新重试。
            pending.id = uuid::Uuid::new_v4().to_string();
            pending.wire = None;
        }
        task.state = "ready".into();
        task.reason = "not_sent_retry".into();
        task.next_at = now + i64::from(task.options.delay) * 1000;
        task.changed_at = now;
    }
    Ok(())
}
