//! 用临时 SQLite 和当前会话的受控公开回复验证续跑；不启动模型服务或真实浏览器。
use serde_json::json;
use webcodex_continuation::*;

/// 固定测试时钟，单位 Unix 毫秒；所有到期检查由测试显式推进。
const NOW: i64 = 1_000_000;

/// 创建账户私有的临时目录，不依赖构建机的默认 umask；测试结束由 TempDir 清理。
fn private_directory() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    directory
}

/// 创建合法的单浏览器会话绑定，没有真实账户或浏览器数据。
fn binding() -> Binding {
    Binding {
        client: "11111111-1111-4111-8111-111111111111".into(),
        browser: "22222222-2222-4222-8222-222222222222".into(),
        conversation: "33333333-3333-4333-8333-333333333333".into(),
        origin: "https://chatgpt.com".into(),
        prefix: String::new(),
        tab_id: 7,
    }
}

/// 构造成功终结的公开回合，用户与回复 ID 相互对应。
fn page() -> Observation {
    Observation {
        source_proof: None,
        url: format!("https://chatgpt.com/c/{}", binding().conversation),
        interrupted: false,
        tools_busy: false,
        approval: false,
        finish_id: None,
        document_id: "doc-before".into(),
        activity: Activity::default(),
        supported: true,
        busy: false,
        draft: false,
        blocked: false,
        user_id: "u1".into(),
        user_text: "实现完整功能并验证".into(),
        reply: Some(Reply {
            id: "a1".into(),
            user_id: "u1".into(),
            text: "还剩验证尚未完成".into(),
            terminal: true,
        }),
    }
}

/// 启用一个最多两轮、间隔三十秒的新任务。
/// # 参数
/// - `controller`：本测试拥有的独立账本控制器。
/// - `mode`：要覆盖的 Continue、Goal 或 Loop 分支。
async fn start(controller: &Controller, mode: Mode) -> TaskView {
    controller
        .handle(
            "owner".into(),
            Request::Start {
                request_id: uuid::Uuid::new_v4().to_string(),
                task_id: None,
                generation: None,
                binding: binding(),
                options: Options {
                    objective: "实现完整功能并验证".into(),
                    mode,
                    limit: 2,
                    delay: 30,
                    policy: Policy {
                        continuous: false,
                        ..Policy::default()
                    },
                },
                page: page(),
            },
            NOW,
        )
        .await
        .unwrap()
        .tasks
        .remove(0)
}

/// 用原版本和绑定提交指定公开页面，验证状态机而非直接修改数据库。
/// # 参数
/// - `controller`：当前测试控制器。
/// - `task`：确切任务与控制版本。
/// - `page`：受控公开回合，所有权交给请求。
/// - `now`：测试时钟，单位 Unix 毫秒。
async fn observe(
    controller: &Controller,
    task: &TaskView,
    page: Observation,
    now: i64,
) -> TaskView {
    controller
        .handle(
            "owner".into(),
            Request::Observe {
                task_id: task.id.clone(),
                generation: task.generation,
                binding: task.binding.clone(),
                page,
            },
            now,
        )
        .await
        .unwrap()
        .tasks
        .remove(0)
}

/// 领取指定 ready 消息；page 为当前回合，重复领取必须失败。
/// # 参数
/// - `controller`：本测试控制器。
/// - `task`：已准备好的任务，含确切投递 ID。
/// - `page`：领取前重新读取的受控状态。
async fn claim(controller: &Controller, task: &TaskView, page: Observation) -> Response {
    dispatch_at(controller, task, page, NOW).await
}

/// 准备并单独授权一次合成投递；错误或页面变化保留准备阶段结果，不伪造发送权。
/// # 参数
/// - `controller`：本测试的真实账本。
/// - `task`：待发任务投影，含确切版本和消息 ID。
/// - `page`：两个阶段共同使用的受控公开页面。
/// - `now`：测试显式推进的 Unix 毫秒时间。
async fn dispatch_at(
    controller: &Controller,
    task: &TaskView,
    page: Observation,
    now: i64,
) -> Response {
    let prepared = controller
        .handle(
            "owner".into(),
            Request::Prepare {
                task_id: task.id.clone(),
                generation: task.generation,
                binding: task.binding.clone(),
                delivery_id: task.delivery_id.clone().unwrap(),
                page: page.clone(),
            },
            now,
        )
        .await
        .unwrap();
    let Some(dispatch) = prepared.dispatch else {
        return prepared;
    };
    let authorized = controller
        .handle(
            "owner".into(),
            Request::Authorize {
                task_id: task.id.clone(),
                generation: task.generation,
                binding: task.binding.clone(),
                delivery_id: dispatch.id.clone(),
                page,
            },
            now,
        )
        .await
        .unwrap();
    Response {
        tasks: authorized.tasks,
        dispatch: Some(dispatch),
        repair: None,
    }
}

/// Goal 的原子预留、接收回执、完成标记及重启状态均由真实账本验证。
#[tokio::test]
async fn goal_claims_once_confirms_receipt_and_stops_on_completion_marker() {
    let dir = private_directory();
    let path = dir.path().join("tasks.sqlite");
    let controller = Controller::open(&path, NOW).unwrap();
    let task = start(&controller, Mode::Goal).await;
    let task = observe(&controller, &task, page(), NOW).await;
    assert_eq!(task.state, "ready");
    let response = claim(&controller, &task, page()).await;
    let dispatch = response.dispatch.unwrap();
    let task = &response.tasks[0];
    assert_eq!(task.state, "dispatching");
    assert_eq!(task.rounds, 1);
    let repeat = controller
        .handle(
            "owner".into(),
            Request::Prepare {
                task_id: task.id.clone(),
                generation: task.generation,
                binding: binding(),
                delivery_id: dispatch.id.clone(),
                page: page(),
            },
            NOW,
        )
        .await;
    assert!(matches!(repeat, Err(Error::Conflict)));
    let mut received = page();
    received.user_id = "u2".into();
    received.user_text = dispatch.text;
    received.reply = Some(Reply {
        id: "a2".into(),
        user_id: "u2".into(),
        text: format!("完成验证\n{}", marker(task, "COMPLETE")),
        terminal: true,
    });
    let task = observe(&controller, task, received.clone(), NOW + 1).await;
    assert_eq!(task.state, "running");
    assert_eq!(
        observe(&controller, &task, received, NOW + 2).await.state,
        "done"
    );
    drop(controller);
    assert_eq!(
        Controller::open(&path, NOW + 3)
            .unwrap()
            .handle(
                "owner".into(),
                Request::List {
                    client: binding().client
                },
                NOW
            )
            .await
            .unwrap()
            .tasks[0]
            .state,
        "done"
    );
}

/// 领取响应丢失后重启不能重新发送；其他账户不能读取或变更原任务。
#[tokio::test]
async fn restart_retains_unknown_delivery_for_receipt_and_rejects_foreign_owner() {
    let dir = private_directory();
    let path = dir.path().join("tasks.sqlite");
    let controller = Controller::open(&path, NOW).unwrap();
    let task = start(&controller, Mode::Goal).await;
    let ready = observe(&controller, &task, page(), NOW).await;
    claim(&controller, &ready, page()).await;
    assert!(matches!(
        controller
            .handle(
                "other".into(),
                Request::Pause {
                    task_id: task.id,
                    generation: 1,
                    binding: binding(),
                    reason: None
                },
                NOW
            )
            .await,
        Err(Error::NotFound)
    ));
    drop(controller);
    let controller = Controller::open(&path, NOW + 1).unwrap();
    let task = controller
        .handle(
            "owner".into(),
            Request::List {
                client: binding().client,
            },
            NOW,
        )
        .await
        .unwrap()
        .tasks
        .remove(0);
    assert_eq!(task.state, "uncertain");
    assert_eq!(task.reason, "awaiting_receipt");
    assert!(task.delivery_id.is_some());
}

/// 草稿、非终结消息、错误回合及未知来源都不得领取消息。
#[tokio::test]
async fn draft_and_interim_reply_never_create_or_claim_delivery() {
    let dir = private_directory();
    let controller = Controller::open(&dir.path().join("tasks.sqlite"), NOW).unwrap();
    let task = start(&controller, Mode::Goal).await;
    let mut interim = page();
    interim.reply.as_mut().unwrap().terminal = false;
    assert_eq!(
        observe(&controller, &task, interim, NOW).await.state,
        "running"
    );
    let ready = observe(&controller, &task, page(), NOW).await;
    let mut draft = page();
    draft.draft = true;
    let result = controller
        .handle(
            "owner".into(),
            Request::Prepare {
                task_id: ready.id.clone(),
                generation: ready.generation,
                binding: ready.binding.clone(),
                delivery_id: ready.delivery_id.clone().unwrap(),
                page: draft,
            },
            NOW,
        )
        .await;
    assert!(matches!(result, Err(Error::Conflict)));
    assert_eq!(
        observe(&controller, &ready, page(), NOW).await.state,
        "ready"
    );
}

/// 人工修改带投递标记的正文仍须暂停，不能仅靠标记认作自动回执。
#[tokio::test]
async fn changed_delivery_receipt_is_recorded_as_human_change() {
    let dir = private_directory();
    let controller = Controller::open(&dir.path().join("tasks.sqlite"), NOW).unwrap();
    let task = start(&controller, Mode::Goal).await;
    let ready = observe(&controller, &task, page(), NOW).await;
    let claimed = claim(&controller, &ready, page()).await;
    let mut edited = page();
    edited.user_id = "edited-receipt".into();
    edited.user_text = format!("{}\n仅检查，不执行。", claimed.dispatch.unwrap().text);
    edited.reply = None;
    let paused = observe(&controller, &claimed.tasks[0], edited, NOW + 1).await;
    assert_eq!(paused.reason, "user_changed");
    assert_eq!(paused.state, "paused");
}

/// 相同回复 ID 的公开正文变化也会撤销投递，避免发送基于旧回复的指令。
#[tokio::test]
async fn changed_assistant_body_before_claim_prevents_delivery() {
    let dir = private_directory();
    let controller = Controller::open(&dir.path().join("tasks.sqlite"), NOW).unwrap();
    let task = start(&controller, Mode::Goal).await;
    let ready = observe(&controller, &task, page(), NOW).await;
    let mut changed = page();
    changed.reply.as_mut().unwrap().text = "现在需要用户批准".into();
    let response = claim(&controller, &ready, changed).await;
    assert!(response.dispatch.is_none());
    assert_eq!(response.tasks[0].reason, "page_changed");
}

/// 相同启用键只创建一个任务；不同内容不能复用同一幂等键。
#[tokio::test]
async fn start_is_idempotent_and_rejects_reused_key_with_different_objective() {
    let dir = private_directory();
    let controller = Controller::open(&dir.path().join("tasks.sqlite"), NOW).unwrap();
    let key = uuid::Uuid::new_v4().to_string();
    // objective 是本测试唯一变化的字段；同一 key 的参数变化必须被拒绝。
    let request = |objective: &str| Request::Start {
        request_id: key.clone(),
        task_id: None,
        generation: None,
        binding: binding(),
        options: Options {
            objective: objective.into(),
            mode: Mode::Goal,
            limit: 1,
            delay: 30,
            policy: Policy::default(),
        },
        page: page(),
    };
    let first = controller
        .handle("owner".into(), request("同一目标"), NOW)
        .await
        .unwrap();
    let repeated = controller
        .handle("owner".into(), request("同一目标"), NOW)
        .await
        .unwrap();
    assert_eq!(first.tasks[0].id, repeated.tasks[0].id);
    assert!(matches!(
        controller
            .handle("owner".into(), request("不同目标"), NOW)
            .await,
        Err(Error::Conflict)
    ));
    assert_eq!(
        controller
            .handle(
                "owner".into(),
                Request::List {
                    client: binding().client
                },
                NOW
            )
            .await
            .unwrap()
            .tasks
            .len(),
        1
    );
}

/// 用户原地编辑保持同一消息 ID 时也必须暂停；显式恢复保留更新后的用户约束。
#[tokio::test]
async fn in_place_human_edit_pauses_and_can_be_resumed_explicitly() {
    let dir = private_directory();
    let controller = Controller::open(&dir.path().join("tasks.sqlite"), NOW).unwrap();
    let task = start(&controller, Mode::Goal).await;
    let mut edited = page();
    edited.user_text = "改为只检查，不要写文件。".into();
    let paused = observe(&controller, &task, edited.clone(), NOW).await;
    assert_eq!(paused.reason, "user_changed");
    let resumed = controller
        .handle(
            "owner".into(),
            Request::Start {
                request_id: uuid::Uuid::new_v4().to_string(),
                task_id: Some(paused.id),
                generation: Some(paused.generation),
                binding: binding(),
                options: Options {
                    objective: "只检查，不写文件".into(),
                    mode: Mode::Goal,
                    limit: 1,
                    delay: 30,
                    policy: Policy::default(),
                },
                page: edited,
            },
            NOW,
        )
        .await
        .unwrap();
    assert_eq!(resumed.tasks[0].state, "running");
}

/// 生成受控的当前启用版本标记，只用于构造模拟 ChatGPT 的公开最终回复。
/// # 参数
/// - `task`：本测试启动的任务投影。
/// - `action`：COMPLETE、CONTINUE、BLOCKED 或有意非法的动作文本。
fn marker(task: &TaskView, action: &str) -> String {
    format!("[[WEBCODEX_GOAL:{}:{}:{action}]]", task.id, task.generation)
}

/// 显式启用并领取首轮目标提示；确认它只含 WebCodex 标记和用户目标。
/// # 参数
/// - `controller`：本测试持有的独立账本。
/// - `mode`：Goal 或 Loop；不需要任何外部模型配置。
async fn bootstrap(controller: &Controller, mode: Mode) -> Response {
    let task = start(controller, mode).await;
    let ready = observe(controller, &task, page(), NOW).await;
    let claimed = claim(controller, &ready, page()).await;
    let text = &claimed.dispatch.as_ref().unwrap().text;
    assert!(text.contains("实现完整功能并验证"));
    assert!(text.contains(&marker(&ready, "CONTINUE")));
    assert!(!text.contains("COS"));
    claimed
}

/// 模拟确切自动发送回执及其终结回复，分别上报两次以覆盖回执与回复的状态转换。
/// # 参数
/// - `controller`：本测试控制器。
/// - `claimed`：唯一一次领取结果，正文移入模拟用户消息。
/// - `text`：模拟 ChatGPT 的完整最终回复。
/// - `step`：模拟消息编号，避免复用已消费的提供方消息 ID。
/// - `now`：本次回执 Unix 毫秒时间，下一次领取还须满足三十秒间隔。
async fn answer(
    controller: &Controller,
    claimed: Response,
    text: String,
    step: u32,
    now: i64,
) -> (TaskView, Observation) {
    let mut received = page();
    received.user_id = format!("u{step}");
    received.user_text = claimed.dispatch.unwrap().text;
    received.reply = Some(Reply {
        id: format!("a{step}"),
        user_id: received.user_id.clone(),
        text,
        terminal: true,
    });
    let acknowledged = observe(controller, &claimed.tasks[0], received.clone(), now).await;
    assert_eq!(acknowledged.state, "running");
    let next = observe(controller, &acknowledged, received.clone(), now + 1).await;
    (next, received)
}

/// Goal 无需外部模型；当前会话在目标提示之后返回确切 COMPLETE 才停止，重复上报不再投递。
#[tokio::test]
async fn goal_stops_only_on_current_chat_completion_after_bootstrap_receipt() {
    let dir = private_directory();
    let controller = Controller::open(&dir.path().join("tasks.sqlite"), NOW).unwrap();
    let claimed = bootstrap(&controller, Mode::Goal).await;
    let text = format!(
        "已完成全部工作和验证。\n{}",
        marker(&claimed.tasks[0], "COMPLETE")
    );
    let (done, received) = answer(&controller, claimed, text, 2, NOW + 1).await;
    assert_eq!(done.state, "done");
    assert_eq!(done.reason, "goal_complete");
    assert_eq!(
        observe(&controller, &done, received, NOW + 3).await.state,
        "done"
    );
}

/// 缺失、其他任务、旧版本、代码引用、非最后一行和 Loop 的完成声明都暂停，不能猜测继续。
#[tokio::test]
async fn malformed_stale_or_quoted_markers_pause_without_delivery() {
    for case in 0..6 {
        let dir = private_directory();
        let controller = Controller::open(&dir.path().join("tasks.sqlite"), NOW).unwrap();
        let mode = if case == 5 { Mode::Loop } else { Mode::Goal };
        let claimed = bootstrap(&controller, mode).await;
        let valid = marker(&claimed.tasks[0], "CONTINUE");
        let text = match case {
            0 => "还有工作未完成。".into(),
            1 => format!("[[WEBCODEX_GOAL:{}:1:CONTINUE]]", uuid::Uuid::new_v4()),
            2 => valid.replace(":1:", ":0:"),
            3 => format!("```text\n{valid}"),
            4 => format!("{valid}\n这不是最后一行。"),
            _ => marker(&claimed.tasks[0], "COMPLETE"),
        };
        let (paused, _) = answer(&controller, claimed, text, 2, NOW + 1).await;
        assert_eq!(paused.reason, "goal_marker_missing");
        assert_eq!(paused.state, "paused");
        assert!(paused.delivery_id.is_none());
    }
}

/// Goal 和 Loop 均按当前会话 CONTINUE 续跑；领取预算包含首轮提示，耗尽后不能自动追加消息。
#[tokio::test]
async fn both_modes_continue_within_budget_and_stop_at_round_limit() {
    for mode in [Mode::Goal, Mode::Loop] {
        let dir = private_directory();
        let controller = Controller::open(&dir.path().join("tasks.sqlite"), NOW).unwrap();
        let claimed = bootstrap(&controller, mode).await;
        let text = marker(&claimed.tasks[0], "CONTINUE");
        let (ready, received) = answer(&controller, claimed, text, 2, NOW + 1).await;
        assert_eq!(ready.state, "ready");
        let second = dispatch_at(&controller, &ready, received, NOW + 30_001).await;
        assert_eq!(second.tasks[0].rounds, 2);
        let text = marker(&second.tasks[0], "CONTINUE");
        let (paused, _) = answer(&controller, second, text, 3, NOW + 30_002).await;
        assert_eq!(paused.reason, "round_limit");
        assert_eq!(paused.state, "paused");
    }
}

/// 阻塞由当前会话明确报告；无论是否完成首轮目标，授权到期都由 Server 独立暂停。
#[tokio::test]
async fn current_chat_blocked_and_server_expiry_pause_without_new_messages() {
    let dir = private_directory();
    let controller = Controller::open(&dir.path().join("tasks.sqlite"), NOW).unwrap();
    let claimed = bootstrap(&controller, Mode::Loop).await;
    let text = format!("需要用户批准。\n{}", marker(&claimed.tasks[0], "BLOCKED"));
    let (paused, _) = answer(&controller, claimed, text, 2, NOW + 1).await;
    assert_eq!(paused.reason, "chat_blocked");
    let another = Controller::open(&dir.path().join("other.sqlite"), NOW).unwrap();
    start(&another, Mode::Goal).await;
    another.maintain(NOW + 24 * 60 * 60 * 1000).await.unwrap();
    let listed = another
        .handle(
            "owner".into(),
            Request::List {
                client: binding().client,
            },
            NOW,
        )
        .await
        .unwrap();
    assert_eq!(listed.tasks[0].reason, "expired");
}

/// 旧版本无协议字段的 ready 记录升级后暂停并保留原数据；不能投递其旧待发正文。
#[tokio::test]
async fn upgrade_pauses_legacy_goal_without_dropping_saved_task() {
    let dir = private_directory();
    let path = dir.path().join("tasks.sqlite");
    let controller = Controller::open(&path, NOW).unwrap();
    let task = start(&controller, Mode::Goal).await;
    drop(controller);
    let db = rusqlite::Connection::open(&path).unwrap();
    let body: String = db
        .query_row("SELECT body FROM continuation_tasks", [], |row| row.get(0))
        .unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&body).unwrap();
    value.as_object_mut().unwrap().remove("protocol_version");
    value.as_object_mut().unwrap().remove("protocol_armed");
    value["state"] = json!("ready");
    db.execute(
        "UPDATE continuation_tasks SET state='ready',body=?1",
        [value.to_string()],
    )
    .unwrap();
    drop(db);
    let restored = Controller::open(&path, NOW + 1).unwrap();
    let listed = restored
        .handle(
            "owner".into(),
            Request::List {
                client: binding().client,
            },
            NOW + 1,
        )
        .await
        .unwrap();
    assert_eq!(listed.tasks[0].id, task.id);
    assert_eq!(listed.tasks[0].state, "paused");
    assert_eq!(listed.tasks[0].reason, "protocol_updated");
}

/// 启用带明确策略与初始页面的测试任务，不启动真实浏览器或后台工具。
/// # 参数
/// - `controller`：测试独占的真实 SQLite 控制器。
/// - `mode`：待验证的续跑模式。
/// - `policy`：边界内的恢复和触发策略。
/// - `initial`：合成公开页面，所有权交给请求。
async fn configured(
    controller: &Controller,
    mode: Mode,
    policy: Policy,
    initial: Observation,
) -> TaskView {
    // 本文件历史场景使用一次启用；持续开关另有独立回归，不混淆完成与等待新请求。
    let policy = Policy {
        continuous: false,
        ..policy
    };
    controller
        .handle(
            "owner".into(),
            Request::Start {
                request_id: uuid::Uuid::new_v4().to_string(),
                task_id: None,
                generation: None,
                binding: binding(),
                options: Options {
                    objective: "完整任务".into(),
                    mode,
                    limit: 1000,
                    delay: 30,
                    policy,
                },
                page: initial,
            },
            NOW,
        )
        .await
        .unwrap()
        .tasks
        .remove(0)
}

/// 请求真实后端领取修复预算；返回错误供测试验证拒绝路径，不替换生产状态机。
/// # 参数
/// - `controller`：当前测试控制器。
/// - `task`：当前投影与绑定。
/// - `kind`：open 或 reload，非法值用于拒绝测试。
/// - `page`：reload 的当前完整页面；open 使用 None。
/// - `now`：显式推进的 Unix 毫秒测试时钟。
async fn repair(
    controller: &Controller,
    task: &TaskView,
    kind: &str,
    page: Option<Observation>,
    now: i64,
) -> Result<Response, Error> {
    controller
        .handle(
            "owner".into(),
            Request::Repair {
                task_id: task.id.clone(),
                generation: task.generation,
                binding: task.binding.clone(),
                kind: kind.into(),
                page,
            },
            now,
        )
        .await
}

/// 明确空闲传输中断等待三十秒即预留消息；重启保留计时，授权后的未知投递不能重放。
#[tokio::test]
async fn idle_transport_failure_continues_without_refresh_and_preserves_unknown_delivery() {
    let dir = private_directory();
    let path = dir.path().join("tasks");
    let controller = Controller::open(&path, NOW).unwrap();
    let mut partial = page();
    partial.reply.as_mut().unwrap().terminal = false;
    partial.interrupted = true;
    let task = configured(
        &controller,
        Mode::Continue,
        Policy::default(),
        partial.clone(),
    )
    .await;
    assert_eq!(
        observe(&controller, &task, partial.clone(), NOW + 29_999)
            .await
            .state,
        "running"
    );
    drop(controller);
    let controller = Controller::open(&path, NOW + 30_000).unwrap();
    let ready = observe(&controller, &task, partial.clone(), NOW + 30_000).await;
    assert_eq!(ready.state, "ready");
    assert!(ready.recovery_send);
    assert_eq!(ready.generation, task.generation);
    assert_eq!(ready.rounds, 0);
    // event 是真实 SQLite 投影中的控制事件；直接恢复不能领取任何刷新预算。
    assert!(ready.journal.iter().all(|event| event.kind != "repair"));
    let claimed = dispatch_at(&controller, &ready, partial.clone(), NOW + 30_000).await;
    let delivery = claimed.dispatch.as_ref().unwrap().id.clone();
    assert_eq!(claimed.tasks[0].state, "dispatching");
    assert_eq!(claimed.tasks[0].rounds, 1);
    drop(controller);
    let controller = Controller::open(&path, NOW + 31_000).unwrap();
    let uncertain = observe(&controller, &claimed.tasks[0], partial, NOW + 31_000).await;
    assert_ne!(uncertain.state, "ready");
    assert_eq!(uncertain.delivery_id.as_deref(), Some(delivery.as_str()));
    assert_eq!(uncertain.rounds, 1);
}

/// Goal 和 Loop 的空闲传输中断都由当前 ChatGPT 的控制提示续跑，不调用模型 API。
#[tokio::test]
async fn goal_and_loop_transport_failures_reserve_current_chat_instructions_without_refresh() {
    for mode in [Mode::Goal, Mode::Loop] {
        let dir = private_directory();
        let controller = Controller::open(&dir.path().join("tasks"), NOW).unwrap();
        let mut partial = page();
        partial.reply.as_mut().unwrap().terminal = false;
        partial.interrupted = true;
        let task = configured(&controller, mode, Policy::default(), partial.clone()).await;
        let ready = observe(&controller, &task, partial.clone(), NOW + 30_000).await;
        assert_eq!(ready.state, "ready");
        assert!(ready.recovery_send);
        let claimed = dispatch_at(&controller, &ready, partial, NOW + 30_000).await;
        assert!(claimed
            .dispatch
            .as_ref()
            .unwrap()
            .text
            .contains("WEBCODEX_GOAL"));
    }
}

/// 三次浏览器修复预算耗尽后，明确空闲的传输中断仍可使用消息轮数预算续跑，不领取第四次修复。
#[tokio::test]
async fn final_browser_repair_does_not_block_idle_transport_continuation() {
    let dir = private_directory();
    let controller = Controller::open(&dir.path().join("tasks"), NOW).unwrap();
    let mut partial = page();
    partial.reply.as_mut().unwrap().terminal = false;
    let task = configured(
        &controller,
        Mode::Continue,
        Policy {
            silence_seconds: 120,
            ..Policy::default()
        },
        partial.clone(),
    )
    .await;
    // 三次修复各间隔一百二十秒，只提交预算，不伪造浏览器执行结果。
    for offset in [0, 120_000, 240_000] {
        repair(&controller, &task, "wake", None, NOW + offset)
            .await
            .unwrap();
    }
    partial.interrupted = true;
    observe(&controller, &task, partial.clone(), NOW + 240_001).await;
    let ready = observe(&controller, &task, partial, NOW + 270_001).await;
    assert_eq!(ready.state, "ready");
    assert!(ready.recovery_send);
    assert!(ready.repair_at.is_none());
}

/// 合成阻挡分别覆盖生成、工具、草稿、审批、人工 Stop、额度和推理失败；三十秒后均不能直接续跑。
#[tokio::test]
async fn transport_recovery_does_not_bypass_busy_draft_approval_stop_limits_or_thinking_failure() {
    for guard in [
        "busy", "tools", "draft", "approval", "blocked", "stop", "limit", "thinking",
    ] {
        let dir = private_directory();
        let controller = Controller::open(&dir.path().join("tasks"), NOW).unwrap();
        let mut partial = page();
        partial.reply.as_mut().unwrap().terminal = false;
        partial.interrupted = true;
        let task = configured(
            &controller,
            Mode::Continue,
            Policy::default(),
            partial.clone(),
        )
        .await;
        match guard {
            "busy" => partial.busy = true,
            "tools" => partial.tools_busy = true,
            "draft" => partial.draft = true,
            "approval" => partial.approval = true,
            "blocked" => partial.blocked = true,
            "stop" => partial.activity.stop_requested = true,
            "limit" => partial.activity.rate_limited = true,
            "thinking" => partial.activity.thinking_failed = true,
            _ => unreachable!("固定测试分支"),
        }
        let first = observe(&controller, &task, partial.clone(), NOW + 1).await;
        let waiting = observe(&controller, &first, partial, NOW + 30_001).await;
        assert_ne!(waiting.state, "ready", "{guard}");
        assert!(waiting.delivery_id.is_none(), "{guard}");
    }
}

/// 没有明确传输中断的未完成回合须先静默并确认刷新；恢复消息仍需一次性领取和确切回执。
#[tokio::test]
async fn silent_unfinished_turn_recovers_after_refresh_without_replaying_unknown_operations() {
    let dir = private_directory();
    let controller = Controller::open(&dir.path().join("tasks"), NOW).unwrap();
    let mut partial = page();
    partial.reply.as_mut().unwrap().terminal = false;
    partial.interrupted = false;
    let task = configured(
        &controller,
        Mode::Goal,
        Policy {
            silence_seconds: 120,
            ..Policy::default()
        },
        partial.clone(),
    )
    .await;
    assert!(matches!(
        repair(
            &controller,
            &task,
            "reload",
            Some(partial.clone()),
            NOW + 119_999
        )
        .await,
        Err(Error::Conflict)
    ));
    let refreshed = repair(
        &controller,
        &task,
        "reload",
        Some(partial.clone()),
        NOW + 120_000,
    )
    .await
    .unwrap();
    assert_eq!(refreshed.repair.as_deref(), Some("reload"));
    assert!(matches!(
        repair(
            &controller,
            &task,
            "reload",
            Some(partial.clone()),
            NOW + 120_001
        )
        .await,
        Err(Error::Conflict)
    ));
    // 领取后仍是旧文档时不能续跑；确认新文档已空闲后等待三十秒任务间隔。
    assert_eq!(
        observe(&controller, &task, partial.clone(), NOW + 240_000)
            .await
            .state,
        "running"
    );
    partial.document_id = "doc-after".into();
    observe(&controller, &task, partial.clone(), NOW + 240_001).await;
    assert_eq!(
        observe(&controller, &task, partial.clone(), NOW + 270_000)
            .await
            .state,
        "running"
    );
    let ready = observe(&controller, &task, partial.clone(), NOW + 270_001).await;
    assert_eq!(ready.state, "ready");
    assert!(ready.recovery_send);
    let claimed = dispatch_at(&controller, &ready, partial.clone(), NOW + 270_001).await;
    assert!(claimed
        .dispatch
        .as_ref()
        .unwrap()
        .text
        .contains(&marker(&task, "CONTINUE")));
    let (done, _) = answer(
        &controller,
        claimed,
        format!("全部完成\n{}", marker(&task, "COMPLETE")),
        2,
        NOW + 270_002,
    )
    .await;
    assert_eq!(done.state, "done");
}

/// 自动暂停或明确未发送后恢复保留同回合刷新证明；人工暂停、新用户及未知投递不能沿用。
#[tokio::test]
async fn exhausted_idle_resume_retains_confirmed_repair_only_for_same_automatic_turn() {
    for scenario in ["automatic", "manual", "new_user", "not_sent"] {
        let dir = private_directory();
        let controller = Controller::open(&dir.path().join("tasks"), NOW).unwrap();
        let mut partial = page();
        partial.reply = None;
        partial.busy = true;
        partial.activity.deliberate = true;
        let task = configured(&controller, Mode::Goal, Policy::default(), partial.clone()).await;
        // 三次预算均由协议领取；第三次以后观察新文档证明刷新，并用显式时钟耗尽生成等待窗口。
        for offset in [0, 1_200_000, 2_400_000] {
            repair(&controller, &task, "wake", None, NOW + offset)
                .await
                .unwrap();
        }
        partial.document_id = "confirmed-after-repair".into();
        observe(&controller, &task, partial.clone(), NOW + 2_400_001).await;
        let paused = observe(&controller, &task, partial.clone(), NOW + 3_600_001).await;
        assert_eq!(paused.reason, "recovery_exhausted");
        let paused = if scenario == "manual" {
            controller
                .handle(
                    "owner".into(),
                    Request::Pause {
                        task_id: paused.id.clone(),
                        generation: paused.generation,
                        binding: paused.binding.clone(),
                        reason: None,
                    },
                    NOW + 3_600_002,
                )
                .await
                .unwrap()
                .tasks
                .remove(0)
        } else {
            paused
        };
        partial.busy = false;
        if scenario == "new_user" {
            partial.user_id = "different-human-turn".into();
        }
        let resumed = controller
            .handle(
                "owner".into(),
                Request::Start {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    task_id: Some(paused.id.clone()),
                    generation: Some(paused.generation),
                    binding: paused.binding.clone(),
                    options: paused.options.clone(),
                    page: partial.clone(),
                },
                NOW + 3_600_003,
            )
            .await
            .unwrap()
            .tasks
            .remove(0);
        assert_eq!(resumed.rounds, 0);
        assert_eq!(
            observe(&controller, &resumed, partial.clone(), NOW + 3_630_002)
                .await
                .state,
            "running"
        );
        let next = observe(&controller, &resumed, partial.clone(), NOW + 3_630_003).await;
        if matches!(scenario, "automatic" | "not_sent") {
            assert_eq!(next.state, "ready");
            assert!(next.recovery_send);
            assert!(next.repair_at.is_none());
            let claimed = dispatch_at(&controller, &next, partial.clone(), NOW + 3_630_003).await;
            assert_eq!(claimed.tasks[0].rounds, 1);
            if scenario == "not_sent" {
                let failed = controller
                    .handle(
                        "owner".into(),
                        Request::Pause {
                            task_id: next.id.clone(),
                            generation: next.generation,
                            binding: next.binding.clone(),
                            reason: Some("send_failed".into()),
                        },
                        NOW + 3_630_004,
                    )
                    .await
                    .unwrap()
                    .tasks
                    .remove(0);
                // 页面明确未点击且已清除自有草稿；用授权时版本清债，不重新启用已暂停任务。
                let cleared = controller
                    .handle(
                        "owner".into(),
                        Request::NotSent {
                            task_id: failed.id.clone(),
                            generation: next.generation,
                            binding: failed.binding.clone(),
                            delivery_id: claimed.dispatch.as_ref().unwrap().id.clone(),
                            page: partial.clone(),
                        },
                        NOW + 3_630_005,
                    )
                    .await
                    .unwrap()
                    .tasks
                    .remove(0);
                assert_eq!(cleared.state, "paused");
                assert!(cleared.delivery_id.is_none());
                let resumed = controller
                    .handle(
                        "owner".into(),
                        Request::Start {
                            request_id: uuid::Uuid::new_v4().to_string(),
                            task_id: Some(cleared.id.clone()),
                            generation: Some(cleared.generation),
                            binding: cleared.binding.clone(),
                            options: cleared.options.clone(),
                            page: partial.clone(),
                        },
                        NOW + 3_630_006,
                    )
                    .await
                    .unwrap()
                    .tasks
                    .remove(0);
                assert_eq!(resumed.rounds, 1);
                assert!(!resumed.recovery_send);
                let retry = observe(&controller, &resumed, partial, NOW + 3_660_006).await;
                assert_eq!(retry.state, "ready");
                assert_ne!(retry.delivery_id, next.delivery_id);
                assert!(retry.repair_at.is_none());
            }
        } else {
            assert_eq!(next.state, "running", "{scenario}");
            assert!(!next.recovery_send, "{scenario}");
        }
    }
}

/// 活跃工具、人工草稿及审批禁止刷新；工具结束后仍需完整静默窗口。
#[tokio::test]
async fn tools_drafts_and_approval_block_recovery_and_preserve_pending_delivery() {
    let dir = private_directory();
    let controller = Controller::open(&dir.path().join("tasks"), NOW).unwrap();
    let task = start(&controller, Mode::Goal).await;
    let ready = observe(&controller, &task, page(), NOW).await;
    let mut busy = page();
    busy.tools_busy = true;
    let rejected = controller
        .handle(
            "owner".into(),
            Request::Prepare {
                task_id: ready.id.clone(),
                generation: ready.generation,
                binding: ready.binding.clone(),
                delivery_id: ready.delivery_id.clone().unwrap(),
                page: busy.clone(),
            },
            NOW,
        )
        .await;
    assert!(matches!(rejected, Err(Error::Conflict)));
    assert_eq!(
        observe(&controller, &ready, page(), NOW).await.state,
        "ready"
    );
    assert!(
        repair(&controller, &ready, "reload", Some(busy), NOW + 600_000)
            .await
            .is_err()
    );
    let mut approval = page();
    approval.approval = true;
    assert_eq!(
        observe(&controller, &ready, approval, NOW).await.reason,
        "approval_required"
    );
}

/// 浏览器重新绑定只移动同一安装的确切会话；旧标签请求被拒绝，期限不延长。
#[tokio::test]
async fn browser_rebind_preserves_authorization_and_fences_old_document() {
    let dir = private_directory();
    let controller = Controller::open(&dir.path().join("tasks"), NOW).unwrap();
    let task = start(&controller, Mode::Goal).await;
    let mut replacement = binding();
    replacement.browser = uuid::Uuid::new_v4().to_string();
    replacement.tab_id = 99;
    let rebound = controller
        .handle(
            "owner".into(),
            Request::Rebind {
                task_id: task.id.clone(),
                generation: task.generation,
                binding: task.binding.clone(),
                replacement: replacement.clone(),
                page: page(),
            },
            NOW + 1000,
        )
        .await
        .unwrap()
        .tasks
        .remove(0);
    assert_eq!(rebound.binding, replacement);
    assert_eq!(rebound.generation, task.generation);
    assert!(matches!(
        controller
            .handle(
                "owner".into(),
                Request::Observe {
                    task_id: task.id,
                    generation: task.generation,
                    binding: task.binding,
                    page: page()
                },
                NOW + 1001
            )
            .await,
        Err(Error::Conflict)
    ));
    assert_eq!(
        observe(&controller, &rebound, page(), NOW + 24 * 3_600_000)
            .await
            .reason,
        "expired"
    );
}

/// 超过九十秒或服务重启后仍能识别原投递的晚到回执；同一投递不能再次领取。
#[tokio::test]
async fn delayed_receipt_after_timeout_and_restart_reconciles_without_second_claim() {
    let dir = private_directory();
    let path = dir.path().join("tasks");
    let controller = Controller::open(&path, NOW).unwrap();
    let task = start(&controller, Mode::Goal).await;
    let ready = observe(&controller, &task, page(), NOW).await;
    let sent = claim(&controller, &ready, page()).await;
    controller.maintain(NOW + 90_001).await.unwrap();
    drop(controller);
    let controller = Controller::open(&path, NOW + 240_000).unwrap();
    let (done, _) = answer(
        &controller,
        sent,
        format!("完成\n{}", marker(&ready, "COMPLETE")),
        2,
        NOW + 240_001,
    )
    .await;
    assert_eq!(done.state, "done");
    assert_eq!(done.rounds, 1);
}

/// finish 模式先发送协议，之后只消费新的可信 finish；BLOCKED 随时可暂停。
#[tokio::test]
async fn finish_trigger_requires_new_event_and_never_blocks_human_intervention() {
    let dir = private_directory();
    let controller = Controller::open(&dir.path().join("tasks"), NOW).unwrap();
    let task = configured(
        &controller,
        Mode::Goal,
        Policy {
            trigger: "finish".into(),
            session_id: Some("wc_session_test".into()),
            ..Policy::default()
        },
        page(),
    )
    .await;
    let ready = observe(&controller, &task, page(), NOW).await;
    assert_eq!(ready.state, "ready");
    let claimed = claim(&controller, &ready, page()).await;
    let (waiting, mut final_page) =
        answer(&controller, claimed, marker(&task, "COMPLETE"), 2, NOW + 1).await;
    assert_eq!(waiting.state, "running");
    final_page.finish_id = Some("finish-current".into());
    assert_eq!(
        observe(&controller, &waiting, final_page, NOW + 2)
            .await
            .state,
        "done"
    );
}

/// 刷新和重开共享三次持久化预算；进程重启不会补发预算或无限创建标签。
#[tokio::test]
async fn recovery_attempts_are_bounded_across_restart() {
    let dir = private_directory();
    let path = dir.path().join("tasks");
    let controller = Controller::open(&path, NOW).unwrap();
    let task = start(&controller, Mode::Goal).await;
    for step in 0..3 {
        assert_eq!(
            repair(&controller, &task, "open", None, NOW + step * 600_000)
                .await
                .unwrap()
                .repair
                .as_deref(),
            Some("open")
        );
    }
    drop(controller);
    let controller = Controller::open(&path, NOW + 1_800_000).unwrap();
    let exhausted = repair(&controller, &task, "open", None, NOW + 1_800_000)
        .await
        .unwrap();
    assert_eq!(exhausted.tasks[0].reason, "recovery_exhausted");
    assert!(exhausted.repair.is_none());
}

/// 自动回合不累计旧回复正文；大量公开输出不能耗尽只用于当前回执的存储预算。
#[tokio::test]
async fn long_goal_discards_obsolete_receipts_without_trimming_human_requirements() {
    let dir = private_directory();
    let controller = Controller::open(&dir.path().join("tasks"), NOW).unwrap();
    let task = configured(
        &controller,
        Mode::Goal,
        Policy {
            continuous: false,
            ..Policy::default()
        },
        page(),
    )
    .await;
    let mut current_page = page();
    let mut ready = observe(&controller, &task, current_page.clone(), NOW).await;
    for step in 2..12 {
        let sent = dispatch_at(
            &controller,
            &ready,
            current_page.clone(),
            NOW + i64::from(step) * 30_001,
        )
        .await;
        let text = format!(
            "{} progress {step}\n{}",
            "x".repeat(25_000),
            marker(&task, if step == 11 { "COMPLETE" } else { "CONTINUE" })
        );
        (ready, current_page) = answer(
            &controller,
            sent,
            text,
            step,
            NOW + i64::from(step) * 30_001,
        )
        .await;
    }
    assert_eq!(ready.state, "done");
    assert_eq!(ready.rounds, 10);
}

/// Continue 的正常结束不追发；仅未完成回合在静默刷新后获得恢复消息。
#[tokio::test]
async fn continue_watches_unfinished_turn_and_stops_on_normal_final() {
    let dir = private_directory();
    let controller = Controller::open(&dir.path().join("tasks"), NOW).unwrap();
    let task = start(&controller, Mode::Continue).await;
    let done = observe(&controller, &task, page(), NOW).await;
    assert_eq!(done.state, "done");
    assert_eq!(done.rounds, 0);
    assert!(done.delivery_id.is_none());
    let mut partial = page();
    partial.reply.as_mut().unwrap().terminal = false;
    let watching = configured(
        &controller,
        Mode::Continue,
        Policy {
            silence_seconds: 120,
            ..Policy::default()
        },
        partial.clone(),
    )
    .await;
    repair(
        &controller,
        &watching,
        "reload",
        Some(partial.clone()),
        NOW + 120_000,
    )
    .await
    .unwrap();
    partial.document_id = "doc-after".into();
    observe(&controller, &watching, partial.clone(), NOW + 120_001).await;
    let ready = observe(&controller, &watching, partial, NOW + 240_001).await;
    assert_eq!(ready.state, "ready");
    assert!(ready.recovery_send);
}

/// finish 先于投递回执到达时仍属于新回合，不能因基线更新丢失事件。
#[tokio::test]
async fn finish_before_delivery_receipt_is_not_lost() {
    let dir = private_directory();
    let controller = Controller::open(&dir.path().join("tasks"), NOW).unwrap();
    let task = configured(
        &controller,
        Mode::Goal,
        Policy {
            trigger: "finish".into(),
            session_id: Some("wc_session_test".into()),
            ..Policy::default()
        },
        page(),
    )
    .await;
    let ready = observe(&controller, &task, page(), NOW).await;
    let claimed = claim(&controller, &ready, page()).await;
    let mut receipt = page();
    receipt.user_id = "u2".into();
    receipt.user_text = claimed.dispatch.unwrap().text;
    receipt.finish_id = Some("finish-new-turn".into());
    receipt.reply = Some(Reply {
        id: "a2".into(),
        user_id: "u2".into(),
        text: marker(&task, "COMPLETE"),
        terminal: true,
    });
    let running = observe(&controller, &claimed.tasks[0], receipt.clone(), NOW + 1).await;
    assert_eq!(
        observe(&controller, &running, receipt, NOW + 2).await.state,
        "done"
    );
}
