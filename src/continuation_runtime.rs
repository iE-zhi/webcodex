//! 将续跑观测与本账户 Job、Workflow Session 和 finish 事件协调；不执行工具或调用模型。
use crate::auth::AuthContext;
use crate::tool_runtime::ToolRuntime;
use std::sync::Arc;
use webcodex_continuation::{Controller, Request};

/// 已认证浏览器的后端守卫失败时，给原任务保存明确暂停原因；旧版本冲突不覆盖新状态。
/// # 参数
/// - `request`：原始账户控制请求，只取确切任务、版本和浏览器绑定。
/// - `controller`：同一个持久化账本。
/// - `owner`：已经验证的稳定账户 ID。
/// - `code`：本模块固定错误；只有不能安全继续的权限、依赖及状态缺口触发暂停。
pub(crate) async fn pause_after_guard_failure(
    request: &Request,
    controller: &Controller,
    owner: &str,
    code: &str,
) {
    if ![
        "session_access_denied",
        "session_unavailable",
        "runtime_unavailable",
        "runner_access_denied",
    ]
    .contains(&code)
    {
        return;
    }
    let target = match request {
        Request::Update {
            task_id,
            generation,
            binding,
            ..
        }
        | Request::Observe {
            task_id,
            generation,
            binding,
            ..
        }
        | Request::Claim {
            task_id,
            generation,
            binding,
            ..
        }
        | Request::Prepare {
            task_id,
            generation,
            binding,
            ..
        }
        | Request::Authorize {
            task_id,
            generation,
            binding,
            ..
        }
        | Request::NotSent {
            task_id,
            generation,
            binding,
            ..
        }
        | Request::Rebind {
            task_id,
            generation,
            binding,
            ..
        }
        | Request::Repair {
            task_id,
            generation,
            binding,
            ..
        } => Some((task_id, generation, binding)),
        _ => None,
    };
    let Some((id, generation, binding)) = target else {
        return;
    };
    let result = controller
        .handle(
            owner.to_owned(),
            Request::Pause {
                task_id: id.clone(),
                generation: *generation,
                binding: binding.clone(),
                reason: Some(code.into()),
            },
            chrono::Utc::now().timestamp_millis(),
        )
        .await;
    match result {
        Ok(_)
        | Err(webcodex_continuation::Error::Conflict | webcodex_continuation::Error::NotFound) => {}
        Err(error) => tracing::error!(
            code = error.code(),
            "continuation guard pause could not be persisted"
        ),
    }
}

/// 聚合已授权的主 Session 与其他工作 Session；只检查明确关联项，不等待账户内无关 Job。
/// # 参数
/// - `runtime`：持有当前调用身份、人工等待和后台 Job 的可信运行时。
/// - `auth`：当前认证账户，每个关联 Session 都重新检查拥有者及项目权限。
/// - `ids`：最多十六个去重 Session，空列表表示尚未完成来源绑定。
/// # 返回
/// 返回当前在途工作及人工等待；任何不完整账本或权限错误均拒绝推进。
pub(crate) async fn session_facts(
    runtime: &ToolRuntime,
    auth: &AuthContext,
    ids: &[String],
) -> Result<(bool, bool), &'static str> {
    if ids.len() > 16 {
        return Err("invalid_request");
    }
    let mut held = false;
    let mut approval = false;
    for id in ids {
        runtime
            .authorize_session_target(id, "continuation", Some(auth))
            .await
            .map_err(|_| "session_access_denied")?;
        let summary = runtime
            .sessions
            .summary(id, Some(1))
            .ok_or("session_unavailable")?;
        let activity = runtime
            .sessions
            .continuation_activity(id)
            .ok_or("session_unavailable")?;
        if !activity.complete {
            return Err("runtime_unavailable");
        }
        held |= activity.busy;
        approval |= summary.messages.open_questions > 0 || summary.messages.pending_guidance > 0;
    }
    if !ids.is_empty() {
        // 一次读取已过滤权限的 Job 后匹配关联集合，不为每个子任务重复扫描整个账户。
        let jobs = runtime
            .runner_registry
            .list_jobs_for_auth_filtered(
                crate::runner_http::runner_access_from_auth(Some(auth)).as_ref(),
                None,
                None,
                None,
            )
            .await;
        held |= jobs.iter().any(|job| {
            job.session_id.as_ref().is_some_and(|id| ids.contains(id))
                && !webcodex_core::runner_job_lifecycle::RunnerJobLifecycle::from_wire(&job.status)
                    .is_ok_and(|state| state.is_terminal())
        });
    }
    Ok((held, approval))
}

/// 覆盖浏览器不能证明的后端事实；读取失败即拒绝本次发送，原任务保留。
/// # 参数
/// - `request`：认证后的操作；只修改页面的工具等待和 finish 事实。
/// - `controller`：续跑账本，读取已有任务的策略，不接受客户端替换策略。
/// - `runtime`：Server 当前工具运行时，提供有权限过滤的 Job 和 Session。
/// - `auth`：当前 PAT/OAuth 认证主体；Session 创建者与项目权限均须匹配。
/// - `owner`：当前账户主键，不能由请求 JSON 指定。
/// # Errors
/// 元数据、Session 权限或后端状态读取失败返回固定错误码；不泄漏其他账户状态。
pub(crate) async fn enrich(
    request: &mut Request,
    controller: &Arc<Controller>,
    runtime: &Arc<ToolRuntime>,
    auth: &AuthContext,
    owner: &str,
) -> Result<(), &'static str> {
    let (id, binding, initial) = match &*request {
        Request::Start {
            binding, options, ..
        }
        | Request::Update {
            binding, options, ..
        } => (None, binding, Some(options.policy.clone())),
        Request::Observe {
            task_id, binding, ..
        }
        | Request::Claim {
            task_id, binding, ..
        }
        | Request::Prepare {
            task_id, binding, ..
        }
        | Request::Authorize {
            task_id, binding, ..
        }
        | Request::NotSent {
            task_id, binding, ..
        }
        | Request::Rebind {
            task_id, binding, ..
        }
        | Request::Repair {
            task_id, binding, ..
        } => (Some(task_id.as_str()), binding, None),
        _ => return Ok(()),
    };
    let (policy, native_link, mut sessions, primary_session) = if let Some(policy) = initial {
        (policy, None, Vec::new(), None)
    } else {
        let task = controller
            .handle(
                owner.to_owned(),
                Request::List {
                    client: binding.client.clone(),
                },
                chrono::Utc::now().timestamp_millis(),
            )
            .await
            .map_err(|_| "continuation_unavailable")?
            .tasks
            .into_iter()
            .find(|task| Some(task.id.as_str()) == id)
            .ok_or("task_not_found")?;
        (
            task.options.policy,
            task.native_window.zip(task.native_principal),
            task.native_sessions,
            task.native_primary_session,
        )
    };
    if let Some(session) = policy.session_id.as_ref() {
        sessions.push(session.clone());
    }
    if let Some(session) = primary_session {
        sessions.push(session);
    }
    sessions.sort();
    sessions.dedup();
    let (mut held, approval) = session_facts(runtime, auth, &sessions).await?;
    if let Some(runner_id) = policy.runner_id.as_deref() {
        let access = crate::runner_http::runner_access_from_auth(Some(auth));
        let runner = runtime
            .runner_registry
            .get_runner_view_for_auth(runner_id, access.as_ref())
            .await
            .ok_or("runner_access_denied")?;
        // 即便管理员可查看其他 Runner，也不能把本账户续跑租约交给其他账户的 agent。
        if auth.username.is_none() || runner.owner != auth.username {
            return Err("runner_access_denied");
        }
    }
    if let Some((window, principal)) = native_link {
        let registry = runtime.window_activity_registry();
        let visible = Some((principal.0.as_str(), principal.1.as_str()));
        held |= registry.coverage_partial_for(visible)
            || registry
                .list_for_window(&window, visible)
                .iter()
                .any(|request| {
                    request.is_meaningful()
                        && request.tool_name.as_deref() != Some("update_task_continuation")
                });
    }
    let page = match request {
        Request::Start { page, .. }
        | Request::Observe { page, .. }
        | Request::Claim { page, .. }
        | Request::Prepare { page, .. }
        | Request::Update { page, .. }
        | Request::Authorize { page, .. }
        | Request::NotSent { page, .. }
        | Request::Rebind { page, .. } => Some(page),
        Request::Repair { page, .. } => page.as_mut(),
        _ => None,
    };
    if let Some(page) = page {
        page.tools_busy |= held;
        page.approval |= approval;
        page.finish_id = None;
    } else if held || approval {
        return Err("runtime_busy");
    }
    Ok(())
}
