//! 未完成回合的持久化恢复预算；浏览器操作必须先领取，未知消息只对账而不重放。
use crate::model::{Mode, Observation, Pending, Task};
use crate::Error;
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// 每个用户回合最多三次刷新或重开，重启不清零。
pub(crate) const MAX_REPAIRS: u32 = 3;
/// 自适应模式下 Pro 正常生成等待十分钟，单位秒。
const PRO_WINDOW: u32 = 600;
/// 自适应模式下高推理正常生成等待二十分钟，单位秒。
const DELIBERATE_WINDOW: u32 = 1200;
/// 自适应模式下 Pro 明确推理失败后等待五分钟，单位秒。
const FAILED_WINDOW: u32 = 300;
/// 没有明确 Retry-After 的访问限制至少保持五分钟，单位秒。
const ACCESS_WINDOW: u32 = 300;

/// 按用户策略和公开模型状态选择静默窗口，不读取私有推理。
/// # 参数
/// - `task`：用户授权的恢复策略。
/// - `page`：当前回合的公开模型名称、推理档位和失败类型。
fn window_seconds(task: &Task, page: &Observation) -> u32 {
    let model = page.activity.model.to_ascii_lowercase();
    let pro = model.split(['-', '_', '/', ' ']).any(|part| part == "pro");
    if !task.options.policy.adaptive_timing {
        return task.options.policy.silence_seconds;
    }
    if pro && page.activity.thinking_failed {
        FAILED_WINDOW
    } else if pro {
        PRO_WINDOW
    } else if page.activity.deliberate && !page.activity.thinking_failed {
        DELIBERATE_WINDOW
    } else {
        task.options.policy.silence_seconds
    }
}

/// 保存提供方等待期限；静态提示不反复续期，消失也不能提前解锁刷新或投递。
/// # 参数
/// - `task`：当前事务任务，等待不消耗修复次数或增加轮数。
/// - `page`：当前用户回合的额度/速率提示，不读取私有内容。
/// - `now`：Unix 毫秒时间；提供方期限不能扩展任务授权。
/// # 返回
/// 仍在等待期限内时 true；已重新生成/执行工具或期限已到时 false。
pub(crate) fn provider_wait(task: &mut Task, page: &Observation, now: i64) -> bool {
    if !page.activity.rate_limited && (page.busy || page.tools_busy) {
        task.recovery.provider_until = 0;
        task.recovery.provider_active = false;
        return false;
    }
    if page.activity.rate_limited {
        if task.recovery.provider_until == 0
            || task.recovery.provider_hint != page.activity.retry_after
            || (!task.recovery.provider_active && now >= task.recovery.provider_until)
        {
            let seconds = if page.activity.retry_after == 0 {
                ACCESS_WINDOW
            } else {
                page.activity.retry_after
            };
            task.recovery.provider_until = task
                .recovery
                .provider_until
                .max(now + i64::from(seconds) * 1000);
            task.recovery.provider_hint = page.activity.retry_after;
        }
        task.recovery.provider_active = true;
    } else {
        task.recovery.provider_active = false;
    }
    if now < task.recovery.provider_until {
        task.reason = "rate_limited".into();
        return true;
    }
    if task.reason == "rate_limited" {
        task.reason.clear();
    }
    false
}

/// 生成公开状态摘要；不保存推理、工具参数或网页异常正文。
/// # 参数
/// - `page`：已验证所属会话的公开观测，只在本次调用借用。
pub(crate) fn fingerprint(page: &Observation) -> String {
    let reply = page.reply.as_ref();
    format!(
        "{:x}",
        Sha256::digest(
            format!(
                "{}\0{}\0{}\0{}\0{}\0{}\0{}",
                page.user_id,
                reply.map_or("", |r| r.id.as_str()),
                reply.map_or("", |r| r.text.as_str()),
                page.busy,
                page.interrupted,
                page.activity.source_turn,
                page.activity.revision
            )
            .as_bytes()
        )
    )
}

/// 空闲的传输中断或已确认刷新按任务间隔续跑；其他未完成回合等待静默，不停止生成或批准工具。
/// # 参数
/// - `task`：事务内任务副本，仅修改恢复计时、待发状态和原因。
/// - `page`：同一用户回合的完整公开状态。
/// - `now`：当前 Unix 毫秒时间；不能延长授权截止时间。
pub(crate) fn watch(task: &mut Task, page: &Observation, now: i64) {
    // 只在确认新主文档后开始修复后的静默窗口；领取后崩溃不能冒充已刷新。
    if task.recovery.repaired_at != 0
        && !task.recovery.confirmed
        && !page.document_id.is_empty()
        && page.document_id != task.recovery.document_id
    {
        task.recovery.confirmed = true;
        task.recovery.progress_at = now;
    }
    // 刷新后已空闲但没有最终答复时也可恢复；断线提示消失不能重新进入生成等待窗口。
    let idle_recovery = (page.interrupted
        || (task.recovery.confirmed && task.recovery.repaired_at != 0))
        && !page.busy
        && !page.tools_busy
        && !page.draft
        && !page.blocked
        && !page.approval
        && !page.activity.thinking_failed
        && !page.activity.rate_limited
        && !page.activity.stop_requested;
    task.recovery.window_seconds = if idle_recovery {
        task.options.delay
    } else {
        window_seconds(task, page)
    };
    let hash = fingerprint(page);
    if task.recovery.fingerprint != hash || page.tools_busy || page.draft {
        task.recovery.fingerprint = hash;
        task.recovery.progress_at = now;
    }
    if page.finish_id != task.recovery.finish_id {
        task.recovery.finish_seen |= page.finish_id.is_some();
        task.recovery.finish_id = page.finish_id.clone();
    }
    let window = i64::from(task.recovery.window_seconds) * 1000;
    if task.options.policy.recovery
        && task.recovery.attempts >= MAX_REPAIRS
        && !idle_recovery
        && !page.reply.as_ref().is_some_and(|reply| reply.terminal)
        && !page.tools_busy
        && !page.draft
        && now - task.recovery.progress_at >= window
        && now - task.recovery.repaired_at >= window
    {
        task.pause("recovery_exhausted", now);
        return;
    }
    if !task.options.policy.recovery
        || page.busy
        || page.tools_busy
        || page.draft
        || page.blocked
        || page.approval
        || page.reply.as_ref().is_some_and(|r| r.terminal)
        || now - task.recovery.progress_at < window
        || (!idle_recovery
            && (!task.recovery.confirmed
                || task.recovery.repaired_at == 0
                || now - task.recovery.repaired_at < window))
    {
        return;
    }
    if task.rounds >= task.options.limit {
        task.pause("round_limit", now);
        return;
    }
    let text = if task.options.mode == Mode::Continue {
        "继续当前被中断且尚未完成的任务，保留所有用户要求和修正。先核对已提交操作与后台任务状态，不重复执行结果未知的操作；需要批准或补充信息时停止并说明。".into()
    } else {
        crate::decision::instruction(task)
    };
    task.pending = Some(Pending {
        id: Uuid::new_v4().to_string(),
        text,
        queue_id: None,
        wire: None,
    });
    task.delivery = Default::default();
    task.recovery.pending = true;
    task.state = "ready".into();
    task.reason = "recovering".into();
}

/// 领取一次浏览器修复；先持久化预算再返回动作，失败不修改浏览器。
/// # 参数
/// - `task`：事务中的当前任务，暂停/完成不得修复，未知投递只能重新打开用于对账。
/// - `kind`：只接受 open、reload 或 wake；wake 供确切加载错误的重试。
/// - `page`：reload 的完整当前观测；open 和 wake 必须为 None。
/// - `now`：Unix 毫秒时间；用于冷却和授权检查。
/// # Errors
/// 非法动作返回 Invalid；预算、冷却、用户输入或状态不符返回 Conflict。
pub(crate) fn claim(
    task: &mut Task,
    kind: &str,
    page: Option<&Observation>,
    now: i64,
) -> Result<(), Error> {
    if !["open", "reload", "wake"].contains(&kind) {
        return Err(Error::Invalid);
    }
    if !task.options.policy.recovery
        || (task.state != "running" && kind == "reload")
        || task.state == "watching"
        || matches!(task.state.as_str(), "paused" | "done")
        || now >= task.expires_at
        || now < task.recovery.provider_until
        || task.recovery.attempts >= MAX_REPAIRS
    {
        return Err(Error::Conflict);
    }
    let window = i64::from(task.recovery.window(&task.options.policy)) * 1000;
    if task.recovery.repaired_at != 0 && now - task.recovery.repaired_at < window {
        return Err(Error::Conflict);
    }
    if kind == "reload" {
        let page = page.ok_or(Error::Invalid)?;
        task.binding.validate(page)?;
        if page.document_id.is_empty()
            || page.tools_busy
            || page.approval
            || page.draft
            || page.blocked
            || page.user_id != task.expected_user
            || fingerprint(page) != task.recovery.fingerprint
            || page.reply.as_ref().is_some_and(|r| r.terminal)
            || now - task.recovery.progress_at < window
        {
            return Err(Error::Conflict);
        }
    } else if page.is_some() {
        return Err(Error::Invalid);
    }
    // 无页面的重开也保留最近已验证文档；再次看到旧文档不能当作修复成功。
    task.recovery.document_id = String::new();
    if let Some(page) = page {
        task.recovery.document_id.clone_from(&page.document_id);
    } else {
        for event in task.journal.iter().rev() {
            if !event.document_id.is_empty() {
                task.recovery.document_id.clone_from(&event.document_id);
                break;
            }
        }
    }
    task.recovery.confirmed = false;
    task.recovery.attempts += 1;
    task.recovery.repaired_at = now;
    Ok(())
}

/// 更新静默记录后判断是否等待 finish；人工阻塞标记不受 finish 偏好限制。
/// # 参数
/// - `task`：当前事务任务副本，可转换为待恢复或暂停状态。
/// - `page`：已验证的同一回合状态。
/// - `now`：Unix 毫秒时间，只更新当前回合的进展记录。
pub(crate) fn defer(task: &mut Task, page: &Observation, now: i64) -> bool {
    watch(task, page, now);
    let waiting_native = task.native.held
        || ((task.options.policy.trigger == "finish"
            || task.queue.iter().any(|entry| entry.trigger == "finish"))
            && task.protocol_armed
            && !task.recovery.finish_seen);
    // 公开最终回复不能冒充原生交接；超出静默窗口仍不交接时给出可见停止，不能挂到授权到期。
    if waiting_native
        && task.native.released.is_none()
        && page.reply.as_ref().is_some_and(|reply| reply.terminal)
    {
        task.reason = "native_finish_required".into();
        if now - task.recovery.progress_at
            >= i64::from(task.recovery.window(&task.options.policy)) * 1000
        {
            task.pause("native_finish_required", now);
        }
    }
    task.state != "running"
        || (waiting_native
            && task.native.released.is_none()
            && !page.reply.as_ref().is_some_and(|reply| {
                matches!(
                    crate::decision::decide(task, &reply.text),
                    crate::decision::Decision::Blocked
                )
            }))
}
