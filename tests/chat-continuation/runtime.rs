//! 用真实运行时与 Session 账本验证续跑协调；不执行命令、不使用真实项目或账户。
use crate::auth::AuthContext;
use crate::auth::AuthKind;
use crate::continuation_runtime::enrich;
use crate::tool_runtime::sessions::{self, SessionCreateOptions, SessionGuards, SessionTransport};
use crate::tool_runtime::ToolRuntime;
use serde_json::json;
use std::sync::Arc;
use webcodex_continuation::{Controller, Request};

/// 构造只属于测试账户的凭据，不含任何有效令牌。
/// # 参数
/// - `name`：测试账户稳定主键，用于验证跨账户拒绝。
fn auth(name: &str) -> AuthContext {
    let mut auth = AuthContext::new(AuthKind::ApiToken);
    auth.user_id = Some(name.into());
    auth.username = Some(name.into());
    auth.api_key_id = Some(format!("key-{name}"));
    auth.token_kind = Some("user".into());
    auth.scopes = vec!["session:collaborate".into()];
    auth
}

/// 使用原生身份与权限创建无项目的测试 Session，避免依赖 Runner 或 Git。
/// # 参数
/// - `runtime`：当前测试独占的运行时。
/// - `auth`：Session 的确切创建者。
fn session(runtime: &ToolRuntime, auth: &AuthContext) -> String {
    let mut options =
        SessionCreateOptions::new(None, None, Default::default(), SessionGuards::default());
    options.owner_authority_fingerprint = Some(
        crate::tool_runtime::session_context::workflow_session_authority_fingerprint(Some(auth))
            .unwrap(),
    );
    runtime
        .sessions
        .start_session_with_options(options)
        .unwrap()
        .session_id
}

/// 生成带伪造 finish 的 Start；可信适配必须覆盖该字段，不能接收网页的完成证明。
/// # 参数
/// - `session`：本测试关联的原生 Session ID。
fn request(session: &str) -> Request {
    serde_json::from_value(json!({"action":"start","request_id":uuid::Uuid::new_v4().to_string(),"task_id":null,"generation":null,
        "binding":{"client":uuid::Uuid::new_v4().to_string(),"browser":uuid::Uuid::new_v4().to_string(),"conversation":"33333333-3333-4333-8333-333333333333",
            "origin":"https://chatgpt.com","prefix":"","tab_id":7},
        "options":{"objective":"原任务","mode":"goal","limit":100,"delay":30,"policy":{"session_id":session}},
        "page":{"url":"https://chatgpt.com/c/33333333-3333-4333-8333-333333333333","supported":true,"busy":false,
            "draft":false,"blocked":false,"user_id":"u1","user_text":"原任务","reply":null,"finish_id":"forged"}})).unwrap()
}

/// 公开观测仅由适配器写回；不读取或伪造控制器内部数据库状态。
/// # 参数
/// - `request`：enrich 处理完成的 Start 请求。
fn page(request: &Request) -> &webcodex_continuation::Observation {
    let Request::Start { page, .. } = request else {
        panic!("test request must be Start")
    };
    page
}

/// 长调用的开始事件滑出两百条展示尾窗后仍必须等待；其他调用全部完成也不能替它确认结束。
#[tokio::test]
async fn native_long_call_remains_busy_after_presentation_tail_rolls_over() {
    /// 插入 201 个短调用，保证长调用开始事件位于展示尾窗之外，但没有淘汰保留账本。
    const EXTRA_CALLS: usize = 201;
    /// 同时淘汰保留账本，验证在途身份不依赖展示尾窗或历史保留容量。
    const RETAINED_EVENTS: usize = 32;
    /// 独立测试仅需一个 Session，保留十六个身份不影响其他测试。
    const MAX_SESSIONS: usize = 16;
    let mut runtime = ToolRuntime::new_for_tests();
    runtime.sessions = sessions::SessionStore::new_in_memory(MAX_SESSIONS, RETAINED_EVENTS);
    let runtime = Arc::new(runtime);
    let auth = auth("long-call-owner");
    let id = session(&runtime, &auth);
    let long = runtime.sessions.record_tool_call_started_with_metadata(
        Some(&id),
        SessionTransport::Mcp,
        "read_file",
        &json!({}),
        None,
        Default::default(),
        sessions::session_tool_contract("read_file"),
    );
    for _ in 0..EXTRA_CALLS {
        let short = runtime.sessions.record_tool_call_started_with_metadata(
            Some(&id),
            SessionTransport::Mcp,
            "runtime_info",
            &json!({}),
            None,
            Default::default(),
            sessions::session_tool_contract("runtime_info"),
        );
        runtime
            .sessions
            .record_tool_call_finished(short, true, &json!({}), None, None)
            .unwrap();
    }
    let shown = runtime.sessions.summary(&id, Some(200)).unwrap();
    assert!(shown
        .events
        .iter()
        .all(|event| event.tool_name != "read_file"));
    assert!(runtime.sessions.continuation_activity(&id).unwrap().busy);
    runtime
        .sessions
        .record_tool_call_finished(long, true, &json!({}), None, None)
        .unwrap();
    assert!(!runtime.sessions.continuation_activity(&id).unwrap().busy);
}

/// 丢弃一个克隆不算取消；最后一个未完成凭据释放后只把所属 Session 标为未知，其他 Session 可继续。
#[tokio::test]
async fn cancelled_call_only_blocks_its_session_after_last_token_drops() {
    let runtime = ToolRuntime::new_for_tests();
    let account = auth("cancel-owner");
    let own = session(&runtime, &account);
    let other = session(&runtime, &account);
    let token = runtime
        .sessions
        .record_tool_call_started_with_metadata(
            Some(&own),
            SessionTransport::Mcp,
            "read_file",
            &json!({}),
            None,
            Default::default(),
            sessions::session_tool_contract("read_file"),
        )
        .unwrap();
    let clone = token.clone();
    drop(token);
    let active = runtime.sessions.continuation_activity(&own).unwrap();
    assert!(active.busy && active.complete);
    drop(clone);
    let unknown = runtime.sessions.continuation_activity(&own).unwrap();
    assert!(unknown.busy && !unknown.complete);
    let unaffected = runtime.sessions.continuation_activity(&other).unwrap();
    assert!(!unaffected.busy && unaffected.complete);
    assert_eq!(
        crate::continuation_runtime::session_facts(&runtime, &account, &[own])
            .await
            .unwrap_err(),
        "runtime_unavailable"
    );
    assert_eq!(
        crate::continuation_runtime::session_facts(&runtime, &account, &[other])
            .await
            .unwrap(),
        (false, false)
    );
}

/// 关联 Session 的真实调用参与完成门槛；未关联 Session 的调用和外账户 Session 不能混入等待集合。
#[tokio::test]
async fn related_sessions_wait_for_exact_dependencies_and_recheck_permissions() {
    let runtime = ToolRuntime::new_for_tests();
    let account = auth("dependency-owner");
    let primary = session(&runtime, &account);
    let child = session(&runtime, &account);
    let unrelated = session(&runtime, &account);
    let foreign = session(&runtime, &auth("foreign-owner"));
    let start = |id: &str| {
        runtime.sessions.record_tool_call_started_with_metadata(
            Some(id),
            SessionTransport::Mcp,
            "read_file",
            &json!({}),
            None,
            Default::default(),
            sessions::session_tool_contract("read_file"),
        )
    };
    let other_call = start(&unrelated);
    let deps = vec![primary, child.clone()];
    assert_eq!(
        crate::continuation_runtime::session_facts(&runtime, &account, &deps)
            .await
            .unwrap(),
        (false, false)
    );
    let child_call = start(&child);
    assert_eq!(
        crate::continuation_runtime::session_facts(&runtime, &account, &deps)
            .await
            .unwrap(),
        (true, false)
    );
    runtime
        .sessions
        .record_tool_call_finished(child_call, true, &json!({}), None, None)
        .unwrap();
    assert_eq!(
        crate::continuation_runtime::session_facts(&runtime, &account, &deps)
            .await
            .unwrap(),
        (false, false)
    );
    assert_eq!(
        crate::continuation_runtime::session_facts(&runtime, &account, &[foreign])
            .await
            .unwrap_err(),
        "session_access_denied"
    );
    runtime
        .sessions
        .record_tool_call_finished(other_call, true, &json!({}), None, None)
        .unwrap();
}

/// 通过真实规范工具分发完成 bind/finish，并验证令牌不进入审计或 Session 历史。
#[tokio::test]
async fn native_mcp_dispatch_binds_exact_source_and_keeps_control_tokens_out_of_audit() {
    let directory = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let now = chrono::Utc::now().timestamp_millis();
    let controller = Arc::new(Controller::open(&directory.path().join("tasks"), now).unwrap());
    let runtime = ToolRuntime::new_for_tests().with_chat_continuations(controller.clone());
    let auth = auth("native-owner");
    let id = session(&runtime, &auth);
    let mut start = request(&id);
    let Request::Start { page, options, .. } = &mut start else {
        panic!("Start fixture");
    };
    options.policy.trigger = "finish".into();
    options.policy.session_id = None;
    page.reply = Some(webcodex_continuation::Reply {
        id: "a1".into(),
        user_id: "u1".into(),
        text: "尚未完成原请求".into(),
        terminal: true,
    });
    let observation = page.clone();
    let task = controller
        .handle("native-owner".into(), start, now)
        .await
        .unwrap()
        .tasks
        .remove(0);
    let ready = controller
        .handle(
            "native-owner".into(),
            Request::Observe {
                task_id: task.id.clone(),
                generation: task.generation,
                binding: task.binding.clone(),
                page: observation.clone(),
            },
            now,
        )
        .await
        .unwrap()
        .tasks
        .remove(0);
    let prepared = controller
        .handle(
            "native-owner".into(),
            Request::Prepare {
                task_id: task.id.clone(),
                generation: task.generation,
                binding: task.binding.clone(),
                delivery_id: ready.delivery_id.unwrap(),
                page: observation.clone(),
            },
            now,
        )
        .await
        .unwrap();
    let dispatch = prepared.dispatch.unwrap();
    let token: String = dispatch
        .text
        .split_once("control_token=")
        .unwrap()
        .1
        .chars()
        .take(64)
        .collect();
    controller
        .handle(
            "native-owner".into(),
            Request::Authorize {
                task_id: task.id.clone(),
                generation: task.generation,
                binding: task.binding.clone(),
                delivery_id: dispatch.id,
                page: observation,
            },
            now,
        )
        .await
        .unwrap();
    let window =
        crate::client_window::ClientWindow::from_opaque("openai-session", "native-main").unwrap();
    let worker =
        crate::client_window::ClientWindow::from_opaque("openai-session", "native-worker").unwrap();
    let build =
        |action: &str, decision: &str| crate::tool_runtime::ToolCall::UpdateTaskContinuation {
            task_id: task.id.clone(),
            generation: task.generation,
            control_token: token.clone(),
            session_id: id.clone(),
            action: action.into(),
            request_id: action.into(),
            decision: decision.into(),
            summary: if action == "finish" {
                "已完成原请求和必要验证"
            } else {
                ""
            }
            .into(),
            related_session_ids: Vec::new(),
        };
    let audit = webcodex_tool_runtime_contracts::tool_audit::session_log_arguments_for_typed_call(
        "update_task_continuation",
        &build("bind", ""),
    );
    assert!(!audit.to_string().contains(&token));
    let shared_transport =
        crate::client_window::ClientWindow::from_opaque("mcp", "shared-transport").unwrap();
    let unidentified = runtime
        .dispatch_with_auth_transport_options_and_metadata_with_window(
            build("bind", ""),
            Some(&auth),
            SessionTransport::Mcp,
            Default::default(),
            Some(&shared_transport),
        )
        .await;
    assert!(
        !unidentified.success,
        "传输连接不能冒充当前 ChatGPT 聊天身份"
    );
    let bound = runtime
        .dispatch_with_auth_transport_options_and_metadata_with_window(
            build("bind", ""),
            Some(&auth),
            SessionTransport::Mcp,
            Default::default(),
            Some(&window),
        )
        .await;
    assert!(bound.success, "{:?}", bound.output);
    assert_eq!(bound.output["released"], false);
    assert_eq!(bound.output["waiting"], true);
    let challenge = bound.output["instructions"]
        .as_str()
        .unwrap()
        .split_once("[[WEBCODEX_SOURCE:")
        .unwrap()
        .1
        .split_once("]]")
        .unwrap()
        .0
        .rsplit(':')
        .next()
        .unwrap()
        .to_string();
    let mut proof = self::page(&request(&id)).clone();
    proof.user_id = "native-public-receipt".into();
    proof.user_text = dispatch.text;
    proof.reply = None;
    proof.busy = true;
    proof.source_proof = Some(webcodex_continuation::SourceProof {
        task_id: task.id.clone(),
        generation: task.generation,
        challenge,
        user_id: proof.user_id.clone(),
        message_id: "native-public-commentary".into(),
    });
    controller
        .handle(
            "native-owner".into(),
            Request::Observe {
                task_id: task.id.clone(),
                generation: task.generation,
                binding: task.binding.clone(),
                page: proof,
            },
            now,
        )
        .await
        .unwrap();
    let confirmed = runtime
        .dispatch_with_auth_transport_options_and_metadata_with_window(
            build("bind", ""),
            Some(&auth),
            SessionTransport::Mcp,
            Default::default(),
            Some(&window),
        )
        .await;
    assert!(confirmed.success);
    assert_eq!(confirmed.output["waiting"], false);
    let linked = controller
        .handle(
            "native-owner".into(),
            Request::Inspect {
                task_id: task.id.clone(),
            },
            now,
        )
        .await
        .unwrap();
    assert_eq!(
        linked.tasks[0].native_primary_session.as_deref(),
        Some(id.as_str()),
        "当前聊天自动关联自己的 Session，无需面板手填"
    );
    assert!(
        linked.tasks[0].options.policy.session_id.is_none(),
        "自动关联不能改写用户的固定 Session 偏好"
    );
    let rejected = runtime
        .dispatch_with_auth_transport_options_and_metadata_with_window(
            build("finish", "complete"),
            Some(&auth),
            SessionTransport::Mcp,
            Default::default(),
            Some(&worker),
        )
        .await;
    assert!(!rejected.success);
    let done = runtime
        .dispatch_with_auth_transport_options_and_metadata_with_window(
            build("finish", "complete"),
            Some(&auth),
            SessionTransport::Mcp,
            Default::default(),
            Some(&window),
        )
        .await;
    assert!(done.success, "{:?}", done.output);
    assert_eq!(done.output["released"], true);
    assert!(done.output["control_token"].is_null());
    let ledger = serde_json::to_string(&runtime.sessions.summary(&id, Some(200)).unwrap()).unwrap();
    assert!(
        !ledger.contains(&token),
        "原生控制令牌不能进入通用 Session 历史"
    );
    assert!(
        !ledger.contains("已完成原请求和必要验证"),
        "公开进度不复制到通用工具参数审计"
    );
}

/// 工具开始与结束使用真实事件账本；伪造 finish 被清除，成功 finish 的确切 ID 可用。
#[tokio::test]
async fn native_tool_wait_and_finish_are_authoritative() {
    let runtime = Arc::new(ToolRuntime::new_for_tests());
    let auth = auth("owner");
    let id = session(&runtime, &auth);
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let controller = Arc::new(Controller::open(&dir.path().join("tasks"), 1_000_000).unwrap());
    let mut first = request(&id);
    enrich(&mut first, &controller, &runtime, &auth, "owner")
        .await
        .unwrap();
    assert!(page(&first).finish_id.is_none());
    assert!(!page(&first).tools_busy);
    let started = runtime.sessions.record_tool_call_started_with_metadata(
        Some(&id),
        SessionTransport::Mcp,
        "finish_coding_task",
        &json!({}),
        None,
        Default::default(),
        sessions::session_tool_contract("finish_coding_task"),
    );
    let mut running = request(&id);
    enrich(&mut running, &controller, &runtime, &auth, "owner")
        .await
        .unwrap();
    assert!(page(&running).tools_busy);
    assert!(page(&running).finish_id.is_none());
    let event = runtime
        .sessions
        .record_tool_call_finished(started, true, &json!({}), None, None)
        .unwrap();
    let mut completed = request(&id);
    enrich(&mut completed, &controller, &runtime, &auth, "owner")
        .await
        .unwrap();
    assert!(!page(&completed).tools_busy);
    assert_eq!(
        runtime
            .sessions
            .continuation_activity(&id)
            .unwrap()
            .finish_id
            .as_deref(),
        Some(event.as_str())
    );
    assert!(
        page(&completed).finish_id.is_none(),
        "Session 历史 finish 不是当前聊天的完成证明"
    );
    let mut foreign = request(&id);
    assert_eq!(
        enrich(
            &mut foreign,
            &controller,
            &runtime,
            &self::auth("other"),
            "other"
        )
        .await
        .unwrap_err(),
        "session_access_denied"
    );
}
