//! 本机续跑租约的 agent 接口；仅当前注册 Runner 进程可读取本账户明确绑定的任务。
use crate::auth::{AuthContext, AuthKind};
use salvo::prelude::*;
use std::sync::Arc;
use webcodex_continuation::Controller;
use webcodex_core::continuation::LocalLeaseRequest;

/// 本机租约请求只含四个元数据字段，最多四 KiB；不接收目标或可执行程序。
const MAX_BODY_BYTES: usize = 4096;

/// 验证 agent 账户、绑定 Runner 和当前进程实例；不接受用户 PAT、bootstrap 或过期实例。
/// # 参数
/// - `req`：经过 AuthMiddleware 的有界 POST 请求。
/// - `depot`：可信认证、工具运行时及同一个续跑账本。
/// - `res`：只输出固定错误码或无凭据的短租约。
#[handler]
pub(crate) async fn lease(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let auth = match depot.obtain::<AuthContext>() {
        Ok(auth)
            if auth.kind == AuthKind::AgentToken
                && !auth.is_bootstrap
                && auth.user_id.is_some() =>
        {
            auth
        }
        _ => {
            reject(res, StatusCode::FORBIDDEN, "continuation_runner_required");
            return;
        }
    };
    let body = match req.payload_with_max_size(MAX_BODY_BYTES).await {
        Ok(body) => body,
        Err(_) => {
            reject(res, StatusCode::PAYLOAD_TOO_LARGE, "invalid_request");
            return;
        }
    };
    let request: LocalLeaseRequest = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(_) => {
            reject(res, StatusCode::BAD_REQUEST, "invalid_request");
            return;
        }
    };
    if auth.allowed_client_id.as_deref() != Some(&request.client_id) {
        reject(res, StatusCode::FORBIDDEN, "continuation_runner_required");
        return;
    }
    let (Ok(runtime), Ok(controller)) = (
        depot.obtain::<Arc<crate::tool_runtime::ToolRuntime>>(),
        depot.obtain::<Arc<Controller>>(),
    ) else {
        reject(
            res,
            StatusCode::SERVICE_UNAVAILABLE,
            "continuation_unavailable",
        );
        return;
    };
    let access = crate::runner_http::runner_access_from_auth(Some(auth));
    let Some(runner) = runtime
        .runner_registry
        .get_runner_view_for_auth(&request.client_id, access.as_ref())
        .await
    else {
        reject(res, StatusCode::CONFLICT, "runner_unavailable");
        return;
    };
    if !runner.connected
        || runner.runner_instance_id != request.agent_instance_id
        || auth.username.is_none()
        || runner.owner != auth.username
    {
        reject(res, StatusCode::CONFLICT, "runner_instance_changed");
        return;
    }
    let Some(owner) = auth.user_id.clone() else {
        reject(res, StatusCode::FORBIDDEN, "continuation_runner_required");
        return;
    };
    match controller
        .local_lease(owner, request, chrono::Utc::now().timestamp_millis())
        .await
    {
        Ok(result) => {
            res.render(Json(result));
        }
        Err(error) => {
            reject(res, StatusCode::CONFLICT, error.code());
        }
    }
}

/// 返回安全错误码，不回显账户、Token、请求正文或其他 Runner 信息。
/// # 参数
/// - `res`：本次 HTTP 响应，由调用方独占。
/// - `status`：固定失败状态。
/// - `code`：服务端固定错误码，不接受外部异常正文。
fn reject(res: &mut Response, status: StatusCode, code: &str) {
    res.status_code(status);
    res.render(Json(serde_json::json!({"error":code})));
}
