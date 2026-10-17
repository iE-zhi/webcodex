//! 长任务 HTTP 适配及启动配置；复用 Server bearer 认证，只接受有稳定账户的会话协作权限。
use crate::action_audit::{ActionAudit, ActionAuditRecord};
use crate::auth::{AuthContext, AuthKind, SCOPE_SESSION_COLLABORATE};
use salvo::prelude::*;
use std::path::Path;
use std::sync::Arc;
use webcodex_continuation::{Controller, Error, Request as ContinuationRequest};

/// 单个续跑请求最多 2 MiB，覆盖两条 128 KiB 消息的最坏 JSON 转义；超限不解析。
const MAX_REQUEST_BYTES: usize = 2 * 1024 * 1024;
/// 后端调度维护每五秒检查预算与未确认操作，不由扩展决定超时状态。
const MAINTENANCE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// 打开本机续跑账本；Goal/Loop 由当前 ChatGPT 的公开回复标记驱动，不读取模型密钥。
/// # 参数
/// - `data_dir`：Server 已创建并持有的私有状态目录。
/// # Errors
/// 目录权限或数据库检查失败时阻止续跑控制器启动，原有数据不被覆盖。
pub(crate) fn open(data_dir: &Path) -> Result<Arc<Controller>, String> {
    #[cfg(windows)]
    webcodex_environment::runtime_entry::validate_windows_env_acl(data_dir)
        .map_err(|_| "continuation data directory must have owner-private ACL".to_string())?;
    Controller::open(
        &data_dir.join("continuations.sqlite"),
        chrono::Utc::now().timestamp_millis(),
    )
    .map(Arc::new)
    .map_err(|error| error.code().to_owned())
}

/// 将现有 Server 生命周期与维护循环放在同一个 Future 中；退出时没有脱离所有者的后台任务。
/// # 参数
/// - `controller`：当前 Server 独占的续跑控制器。
/// - `server`：已有 HTTP/Tunnel 生命周期 Future，结束时停止维护。
pub(crate) async fn supervise<F, T>(controller: Arc<Controller>, server: F) -> T
where
    F: std::future::Future<Output = T>,
{
    tokio::pin!(server);
    let mut interval = tokio::time::interval(MAINTENANCE_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            result = &mut server => return result,
            _ = interval.tick() => {
                if let Err(error) = controller.maintain(chrono::Utc::now().timestamp_millis()).await {
                    tracing::error!(code = error.code(), "continuation maintenance failed");
                }
            }
        }
    }
}

/// 处理本账户任务请求；没有认证、账户或 session:collaborate 权限均拒绝，正文不进入日志。
/// # 参数
/// - `req`：已经过 AuthMiddleware 的 POST 请求，body 读取前检查大小。
/// - `depot`：认证账户及 Server 控制器的只读注入。
/// - `res`：仅写固定错误码、无正文任务投影或一次性发送指令。
#[handler]
pub(crate) async fn control(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let owner = match depot.obtain::<AuthContext>() {
        Ok(auth)
            if matches!(auth.kind, AuthKind::ApiToken | AuthKind::OAuth2Token)
                && !auth.is_bootstrap
                && auth.has_scope(SCOPE_SESSION_COLLABORATE) =>
        {
            auth.user_id.clone()
        }
        _ => None,
    };
    let Some(owner) = owner else {
        res.status_code(StatusCode::FORBIDDEN);
        res.render(Json(
            serde_json::json!({"error":"continuation_account_required"}),
        ));
        return;
    };
    let Ok(controller) = depot.obtain::<Arc<Controller>>().cloned() else {
        res.status_code(StatusCode::SERVICE_UNAVAILABLE);
        res.render(Json(
            serde_json::json!({"error":"continuation_unavailable"}),
        ));
        return;
    };
    if req
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > MAX_REQUEST_BYTES)
    {
        res.status_code(StatusCode::PAYLOAD_TOO_LARGE);
        return;
    }
    let body = match req.payload_with_max_size(MAX_REQUEST_BYTES).await {
        Ok(body) => body,
        Err(_) => {
            res.status_code(StatusCode::PAYLOAD_TOO_LARGE);
            return;
        }
    };
    let mut request: ContinuationRequest = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(_) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(serde_json::json!({"error":"invalid_request"})));
            return;
        }
    };
    let (Ok(auth), Ok(runtime)) = (
        depot.obtain::<AuthContext>(),
        depot.obtain::<Arc<crate::tool_runtime::ToolRuntime>>(),
    ) else {
        res.status_code(StatusCode::SERVICE_UNAVAILABLE);
        res.render(Json(serde_json::json!({"error":"runtime_unavailable"})));
        return;
    };
    if let Err(error) =
        crate::continuation_runtime::enrich(&mut request, &controller, runtime, auth, &owner).await
    {
        crate::continuation_runtime::pause_after_guard_failure(
            &request,
            &controller,
            &owner,
            error,
        )
        .await;
        tracing::warn!(
            code = error,
            "continuation runtime guard rejected operation"
        );
        ActionAudit::start(req, depot, "/api/continuations", "continuation").record(
            ActionAuditRecord::new("continuation_runtime_guard", false, StatusCode::CONFLICT)
                .error(Some(error.into())),
        );
        res.status_code(StatusCode::CONFLICT);
        res.render(Json(serde_json::json!({"error":error})));
        return;
    }
    // 审计只记录操作与安全状态，不记录公开消息、待发正文、PAT。
    let operation = match &request {
        ContinuationRequest::Start { .. } => "continuation_start",
        ContinuationRequest::Update { .. } => "continuation_update",
        ContinuationRequest::List { .. } => "continuation_list",
        ContinuationRequest::Inspect { .. } => "continuation_inspect",
        ContinuationRequest::Observe { .. } => "continuation_observe",
        ContinuationRequest::Claim { .. } => "continuation_claim",
        ContinuationRequest::Prepare { .. } => "continuation_prepare",
        ContinuationRequest::Authorize { .. } => "continuation_authorize",
        ContinuationRequest::NotSent { .. } => "continuation_not_sent",
        ContinuationRequest::Queue { .. } => "continuation_queue",
        ContinuationRequest::Dequeue { .. } => "continuation_dequeue",
        ContinuationRequest::Pause { .. } => "continuation_pause",
        ContinuationRequest::Delete { .. } => "continuation_delete",
        ContinuationRequest::Rebind { .. } => "continuation_rebind",
        ContinuationRequest::Repair { .. } => "continuation_repair",
    };
    let audit = ActionAudit::start(req, depot, "/api/continuations", "continuation");
    match controller
        .handle(owner, request, chrono::Utc::now().timestamp_millis())
        .await
    {
        Ok(response) => {
            if operation != "continuation_list"
                && (operation != "continuation_observe"
                    || response
                        .tasks
                        .iter()
                        .any(|task| matches!(task.state.as_str(), "ready" | "done" | "paused")))
            {
                let states: Vec<_> = response.tasks.iter().map(|task| serde_json::json!({"task_id":task.id,"generation":task.generation,"state":task.state,"reason":task.reason,"rounds":task.rounds})).collect();
                audit.record(
                    ActionAuditRecord::new(operation, true, StatusCode::OK)
                        .summary(serde_json::json!({"tasks":states})),
                );
            }
            res.render(Json(response));
        }
        Err(error) => {
            let code = match error {
                Error::Invalid => StatusCode::BAD_REQUEST,
                Error::NotFound => StatusCode::NOT_FOUND,
                Error::Conflict => StatusCode::CONFLICT,
                Error::Capacity | Error::Busy => StatusCode::TOO_MANY_REQUESTS,
                Error::ContextLimit => StatusCode::PAYLOAD_TOO_LARGE,
                Error::Storage => StatusCode::INTERNAL_SERVER_ERROR,
            };
            tracing::warn!(code = error.code(), "continuation operation rejected");
            audit.record(
                ActionAuditRecord::new(operation, false, code).error(Some(error.code().into())),
            );
            res.status_code(code);
            res.render(Json(serde_json::json!({"error":error.code()})));
        }
    }
}
