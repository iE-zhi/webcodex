//! 原生续跑 MCP 适配；令牌只关联当前主聊天和同一授权 Session，不执行其他工具或调用模型 API。
use super::{ToolCall, ToolResult, ToolRuntime};
use crate::auth::{AuthContext, AuthKind, SCOPE_SESSION_COLLABORATE};
use crate::client_window::ClientWindow;
use serde_json::json;
use webcodex_continuation::{NativeFacts, NativeRequest, Request};

/// 返回固定控制错误；不包含令牌、公开进度、项目路径或其他账户事实。
/// # 参数
/// - `code`：服务端固定错误码。
fn failure(code: &str) -> ToolResult {
    ToolResult::err_with_output(code, json!({"error_kind":code,"instructions":"停止使用旧控制信息；核对当前会话的任务状态，不猜测令牌或其他 Session。"}))
        .with_recovery(super::RecoveryKind::NoAction)
}

impl ToolRuntime {
    /// 当前 ChatGPT 的阶段收尾；先重验认证、来源及后台状态，再交由同一持久账本决定后续指令。
    /// # 参数
    /// - `call`：唯一 UpdateTaskContinuation 类型，包含本回合发行的随机令牌。
    /// - `auth`：MCP 层验证的账户，必须有 session:collaborate，不能为 bootstrap 或匿名。
    /// - `window`：Host/传输提供的当前聊天身份；缺失时拒绝，不退化为账户级关联。
    /// # 返回
    /// 成功只返回当前任务的有界后续工作或停止结果；重复检查不会重复出队。
    pub(super) async fn update_task_continuation_tool(
        &self,
        call: ToolCall,
        auth: Option<&AuthContext>,
        window: Option<&ClientWindow>,
    ) -> ToolResult {
        let ToolCall::UpdateTaskContinuation {
            task_id,
            generation,
            control_token,
            session_id,
            action,
            request_id,
            decision,
            summary,
            related_session_ids,
        } = call
        else {
            return failure("invalid_request");
        };
        let Some(auth) = auth.filter(|auth| {
            matches!(auth.kind, AuthKind::ApiToken | AuthKind::OAuth2Token)
                && !auth.is_bootstrap
                && auth.has_scope(SCOPE_SESSION_COLLABORATE)
        }) else {
            return failure("continuation_account_required");
        };
        let Some(owner) = auth.user_id.as_ref() else {
            return failure("continuation_account_required");
        };
        let Some(window) = window else {
            return failure("continuation_window_required");
        };
        // 传输级 MCP session 可能跨聊天复用；只接受 Host 的聊天身份，不降级到连接或账户身份。
        if !matches!(window.source(), "openai-session" | "openai-conversation") {
            return failure("continuation_window_required");
        }
        let Some(controller) = self.chat_continuations.as_ref() else {
            return failure("continuation_unavailable");
        };
        if self
            .authorize_session_target(&session_id, "update_task_continuation", Some(auth))
            .await
            .is_err()
        {
            return failure("session_access_denied");
        }
        let now = chrono::Utc::now().timestamp_millis();
        let target = match controller
            .handle(
                owner.clone(),
                Request::Inspect {
                    task_id: task_id.clone(),
                },
                now,
            )
            .await
        {
            Ok(result) => result.tasks.into_iter().next(),
            Err(error) => return failure(error.code()),
        };
        let Some(mut target) = target else {
            return failure("task_not_found");
        };
        // 第二次 bind 可等待扩展处理已公开的来源回执；只读查询不持有事务，超过三秒不再等待。
        if action == "bind"
            && target.native_window.is_none()
            && target.reason == "source_confirmation_required"
        {
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
            while target.native_window.is_none() && tokio::time::Instant::now() < deadline {
                let query = controller.handle(
                    owner.clone(),
                    Request::Inspect {
                        task_id: task_id.clone(),
                    },
                    chrono::Utc::now().timestamp_millis(),
                );
                match tokio::time::timeout_at(deadline, query).await {
                    Ok(Ok(result)) => {
                        if let Some(current) = result.tasks.into_iter().next() {
                            target = current;
                        } else {
                            return failure("task_not_found");
                        }
                    }
                    Ok(Err(error)) => return failure(error.code()),
                    Err(_) => break,
                }
                if target.native_window.is_none() {
                    tokio::time::sleep_until(
                        deadline.min(
                            tokio::time::Instant::now() + std::time::Duration::from_millis(250),
                        ),
                    )
                    .await;
                }
            }
        }
        if target
            .options
            .policy
            .session_id
            .as_ref()
            .is_some_and(|id| id != &session_id)
        {
            return failure("session_mismatch");
        }
        if target
            .native_primary_session
            .as_ref()
            .is_some_and(|id| id != &session_id)
        {
            return failure("session_mismatch");
        }
        let mut sessions = target.native_sessions;
        sessions.extend(related_session_ids.iter().cloned());
        sessions.push(session_id.clone());
        sessions.sort();
        sessions.dedup();
        let (session_busy, approval, runtime_unavailable) =
            match crate::continuation_runtime::session_facts(self, auth, &sessions).await {
                Ok((busy, approval)) => (busy, approval, false),
                Err(code) if target.native_window.is_none() => return failure(code),
                Err(_) => (true, false, true),
            };
        sessions.retain(|id| id != &session_id);
        let Ok(principal) = super::session_context::runtime_observation_principal(Some(auth))
        else {
            return failure("continuation_account_required");
        };
        let registry = self.window_activity_registry();
        let visible = Some((principal.0.as_str(), principal.1.as_str()));
        let window_busy = registry.coverage_partial_for(visible)
            || registry
                .list_for_window(window.key(), visible)
                .iter()
                .any(|request| {
                    request.is_meaningful()
                        && request.tool_name.as_deref() != Some("update_task_continuation")
                });
        let facts = NativeFacts {
            runtime_unavailable,
            related_session_ids: sessions,
            principal,
            window: window.key().to_string(),
            session_id,
            busy: session_busy || window_busy,
            approval,
        };
        let request = NativeRequest {
            related_session_ids,
            task_id,
            generation,
            control_token,
            action,
            request_id,
            decision,
            summary,
        };
        match controller
            .native(
                owner.clone(),
                request,
                facts,
                chrono::Utc::now().timestamp_millis(),
            )
            .await
        {
            Ok(result) => match serde_json::to_value(result) {
                Ok(output) => ToolResult::ok(output),
                Err(_) => failure("storage_error"),
            },
            Err(error) => failure(error.code()),
        }
    }
}
