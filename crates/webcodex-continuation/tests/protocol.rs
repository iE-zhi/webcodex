//! 两阶段投递、持续开关和人工等待的真实 SQLite 回归；时钟由测试推进，不发送浏览器消息。
use serde_json::{json, Value};
use webcodex_continuation::{Controller, Error, Request, Response};
use webcodex_continuation::{NativeFacts, NativeRequest};

/// 固定逻辑时钟，单位 Unix 毫秒；不依赖墙上时钟或随机等待。
const NOW: i64 = 1_000_000;

/// 独立私有账本夹具；目录资源随测试结束释放，不访问生产数据。
struct Fixture {
    /// 保持临时数据库父目录存活，Unix 下限定当前用户访问。
    dir: tempfile::TempDir,
    /// 真实生产控制器，所有状态变化均经公开请求完成。
    controller: Controller,
}

impl Fixture {
    /// 使用真实两阶段来源绑定；公开回执来自固定主聊天 fake，不读取私有账本或替 worker 确认。
    /// # 参数
    /// - `prepared`：实际准备响应，保留原始发送正文和服务端控制令牌。
    /// - `authorized`：发送前授权响应，提供确切任务版本和绑定。
    /// - `user`：合成浏览器实际收到该自动消息后的用户消息 ID。
    /// - `now`：固定逻辑时钟；各步骤在同一毫秒串行提交。
    async fn bind_source(
        &self,
        prepared: &Response,
        authorized: &Response,
        user: &str,
        now: i64,
    ) -> webcodex_continuation::NativeResponse {
        let request = native_request(authorized, native_token(prepared), "bind", "", "");
        let pending = self
            .controller
            .native("owner".into(), request.clone(), native_facts(), now)
            .await
            .unwrap();
        assert!(pending.waiting);
        let challenge = pending
            .instructions
            .split_once("[[WEBCODEX_SOURCE:")
            .unwrap()
            .1
            .split_once("]]")
            .unwrap()
            .0
            .rsplit(':')
            .next()
            .unwrap();
        let mut current = page();
        current["user_id"] = json!(user);
        current["user_text"] = json!(prepared.dispatch.as_ref().unwrap().text);
        current["reply"] = Value::Null;
        current["busy"] = json!(true);
        current["source_proof"] = json!({"task_id":authorized.tasks[0].id,"generation":authorized.tasks[0].generation,
            "user_id":user,"message_id":"public-source-proof","challenge":challenge});
        self.call(operation("observe", authorized, current), now)
            .await
            .unwrap();
        let confirmed = self
            .controller
            .native("owner".into(), request, native_facts(), now)
            .await
            .unwrap();
        assert!(!confirmed.waiting);
        confirmed
    }
    /// 创建私有 SQLite 夹具；测试环境错误立即使该测试失败。
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let controller = Controller::open(&dir.path().join("tasks.sqlite"), NOW).unwrap();
        Self { dir, controller }
    }

    /// 用受控 JSON 调用真实请求反序列化和账本，不直接修改数据库。
    /// # 参数
    /// - `body`：本测试的固定协议请求，不含真实账户或凭据。
    /// - `now`：测试控制的 Unix 毫秒时间。
    async fn call(&self, body: Value, now: i64) -> Result<Response, Error> {
        self.controller
            .handle(
                "owner".into(),
                serde_json::from_value::<Request>(body).unwrap(),
                now,
            )
            .await
    }

    /// 启用指定模式并观察一次最终回复，Goal/Loop 会进入 ready，Continue 会等待新人工回合。
    /// # 参数
    /// - `mode`：生产协议的 continue、goal 或 loop。
    async fn start(&self, mode: &str) -> Response {
        let started = self.call(json!({"action":"start", "request_id":uuid::Uuid::new_v4().to_string(),
            "binding":binding(),"options":{"objective":"", "mode":mode,"limit":20,"delay":30},"page":page()}), NOW).await.unwrap();
        self.call(operation("observe", &started, page()), NOW)
            .await
            .unwrap()
    }
}

/// 返回唯一合成聊天绑定；未连接任何真实浏览器。
fn binding() -> Value {
    json!({"client":"11111111-1111-4111-8111-111111111111", "browser":"22222222-2222-4222-8222-222222222222",
        "conversation":"33333333-3333-4333-8333-333333333333", "origin":"https://chatgpt.com","prefix":"","tab_id":7})
}

/// 返回一个完整公开最终回合，后续场景只修改所需的状态字段。
fn page() -> Value {
    json!({"url":"https://chatgpt.com/c/33333333-3333-4333-8333-333333333333","supported":true,"busy":false,
        "draft":false,"blocked":false,"document_id":"before","user_id":"u1","user_text":"原始任务",
        "reply":{"id":"a1","user_id":"u1","text":"尚未完成","terminal":true}})
}

/// 从真实响应取得版本及投递身份，构造下一请求，不推测数据库内部状态。
/// # 参数
/// - `action`：测试要调用的固定操作名。
/// - `response`：上一次真实账本响应。
/// - `page`：本次完整公开页面。
fn operation(action: &str, response: &Response, page: Value) -> Value {
    let task = &response.tasks[0];
    let mut body = json!({"action":action,"task_id":task.id,"generation":task.generation,"binding":task.binding,"page":page});
    if matches!(action, "prepare" | "authorize" | "not_sent") {
        body["delivery_id"] = json!(task.delivery_id);
    }
    body
}

/// 从实际发出的指令读取令牌，不读取私有数据库或由测试伪造令牌身份。
/// # 参数
/// - `response`：真实准备响应，必须包含当前完整投递正文。
fn native_token(response: &Response) -> String {
    response
        .dispatch
        .as_ref()
        .unwrap()
        .text
        .split_once("control_token=")
        .unwrap()
        .1
        .chars()
        .take(64)
        .collect()
}

/// 受控原生事实；固定 Window/Session 无真实凭据，busy 和 approval 由各场景修改。
fn native_facts() -> NativeFacts {
    NativeFacts {
        runtime_unavailable: false,
        related_session_ids: Vec::new(),
        principal: ("user".into(), "owner".into()),
        window: "a".repeat(64),
        session_id: "wc_sess_native_test".into(),
        busy: false,
        approval: false,
    }
}

/// 构造当前聊天原生检查；参数仅是模型的公开声明，账本仍验证版本和令牌。
/// # 参数
/// - `response`：实际任务身份及版本。
/// - `token`：当前指令或上一工具结果发行的令牌。
/// - `action`：bind 或 finish。
/// - `decision`：当前聊天自己声明的状态，bind 为空。
/// - `summary`：公开进度，兼作本测试固定逻辑检查身份。
fn native_request(
    response: &Response,
    token: String,
    action: &str,
    decision: &str,
    summary: &str,
) -> NativeRequest {
    NativeRequest {
        related_session_ids: Vec::new(),
        task_id: response.tasks[0].id.clone(),
        generation: response.tasks[0].generation,
        control_token: token,
        action: action.into(),
        request_id: if action == "bind" {
            "bind"
        } else {
            "checkpoint"
        }
        .into(),
        decision: decision.into(),
        summary: summary.into(),
    }
}

/// worker 先持有继承令牌也不能抢先绑定；只有当前用户回合公开的确切主聊天挑战确认来源。
#[tokio::test]
async fn worker_candidate_cannot_take_over_without_exact_public_source_receipt() {
    let fixture = Fixture::new();
    let ready = fixture.start("goal").await;
    let prepared = fixture
        .call(operation("prepare", &ready, page()), NOW)
        .await
        .unwrap();
    let authorized = fixture
        .call(operation("authorize", &prepared, page()), NOW)
        .await
        .unwrap();
    let bind = native_request(&authorized, native_token(&prepared), "bind", "", "");
    let mut worker = native_facts();
    worker.window = "b".repeat(64);
    worker.session_id = "wc_sess_worker".into();
    let worker_pending = fixture
        .controller
        .native("owner".into(), bind.clone(), worker.clone(), NOW + 1)
        .await
        .unwrap();
    let main_pending = fixture
        .controller
        .native("owner".into(), bind.clone(), native_facts(), NOW + 1)
        .await
        .unwrap();
    assert!(worker_pending.waiting && main_pending.waiting);
    let challenge = main_pending
        .instructions
        .split_once("[[WEBCODEX_SOURCE:")
        .unwrap()
        .1
        .split_once("]]")
        .unwrap()
        .0
        .rsplit(':')
        .next()
        .unwrap();
    let mut current = page();
    current["user_id"] = json!("actual-main-user");
    current["user_text"] = json!(prepared.dispatch.as_ref().unwrap().text);
    current["reply"] = Value::Null;
    current["busy"] = json!(true);
    current["source_proof"] = json!({"task_id":authorized.tasks[0].id,"generation":authorized.tasks[0].generation,"challenge":challenge,"user_id":"wrong-user","message_id":"public-commentary"});
    assert!(
        fixture
            .call(operation("observe", &authorized, current.clone()), NOW + 2)
            .await
            .is_err(),
        "来源声明不能跨用户回合"
    );
    current["source_proof"]["user_id"] = json!("actual-main-user");
    let confirmed = fixture
        .call(operation("observe", &authorized, current), NOW + 3)
        .await
        .unwrap();
    assert_eq!(
        confirmed.tasks[0].native_primary_session.as_deref(),
        Some("wc_sess_native_test")
    );
    assert!(confirmed.tasks[0].options.policy.session_id.is_none());
    assert!(matches!(
        fixture
            .controller
            .native("owner".into(), bind.clone(), worker, NOW + 4)
            .await,
        Err(Error::Conflict)
    ));
    let bound = fixture
        .controller
        .native("owner".into(), bind, native_facts(), NOW + 4)
        .await
        .unwrap();
    assert!(!bound.waiting && !bound.released);
    let mut unavailable = native_facts();
    unavailable.runtime_unavailable = true;
    let stopped = fixture
        .controller
        .native(
            "owner".into(),
            native_request(
                &authorized,
                bound.control_token.unwrap(),
                "finish",
                "complete",
                "无法确认后台状态",
            ),
            unavailable,
            NOW + 5,
        )
        .await
        .unwrap();
    assert!(stopped.released && stopped.control_token.is_none());
    assert!(stopped.instructions.contains("已暂停"));
}

/// 其他依赖持久化后仍参与后续检查；摘要重试中改变依赖集合不得重读旧结果或重复执行。
#[tokio::test]
async fn native_dependencies_persist_and_replay_cannot_change_declared_set() {
    let fixture = Fixture::new();
    let ready = fixture.start("goal").await;
    let prepared = fixture
        .call(operation("prepare", &ready, page()), NOW)
        .await
        .unwrap();
    let authorized = fixture
        .call(operation("authorize", &prepared, page()), NOW)
        .await
        .unwrap();
    let bound = fixture
        .bind_source(&prepared, &authorized, "dependency-source", NOW + 1)
        .await;
    let mut request = native_request(
        &authorized,
        bound.control_token.unwrap(),
        "finish",
        "continue",
        "继续剩余已授权工作",
    );
    request.related_session_ids = vec!["wc_sess_child".into()];
    let mut facts = native_facts();
    facts.related_session_ids = request.related_session_ids.clone();
    let next = fixture
        .controller
        .native("owner".into(), request.clone(), facts.clone(), NOW + 2)
        .await
        .unwrap();
    assert_eq!(next.sequence, 1);
    let inspected = fixture
        .call(
            json!({"action":"inspect","task_id":authorized.tasks[0].id}),
            NOW + 3,
        )
        .await
        .unwrap();
    assert_eq!(inspected.tasks[0].native_sessions, vec!["wc_sess_child"]);
    request.related_session_ids.clear();
    assert!(matches!(
        fixture
            .controller
            .native("owner".into(), request, facts, NOW + 4)
            .await,
        Err(Error::Conflict)
    ));
}

/// 本机租约只给所属账户明确绑定的 Runner；无进程退出证明不打开，未知响应也不重放，修复预算跨重启保留。
#[tokio::test]
async fn local_lease_is_account_runner_scoped_and_wake_budget_is_durable() {
    use webcodex_core::continuation::{LocalLeaseRequest, LocalStatus, LOCAL_LEASE_MS};
    let fixture = Fixture::new();
    let mut current = page();
    current["reply"] = Value::Null;
    let started = fixture.call(json!({"action":"start","request_id":uuid::Uuid::new_v4().to_string(),"binding":binding(),
        "options":{"objective":"原任务","mode":"continue","limit":20,"delay":30,"policy":{"runner_id":"local-runner","keep_awake":true}},"page":current}), NOW).await.unwrap();
    let mut lease_request = LocalLeaseRequest {
        status: LocalStatus::Ready,
        client_id: "local-runner".into(),
        agent_instance_id: uuid::Uuid::new_v4().to_string(),
        request_id: uuid::Uuid::new_v4().to_string(),
        browser_absent: false,
    };
    let empty = fixture
        .controller
        .local_lease("foreign-owner".into(), lease_request.clone(), NOW + 90_000)
        .await
        .unwrap();
    assert!(!empty.keep_awake && empty.wake.is_empty());
    let active = fixture
        .controller
        .local_lease("owner".into(), lease_request.clone(), NOW + 90_000)
        .await
        .unwrap();
    assert!(active.keep_awake && active.wake.is_empty());
    assert_eq!(active.valid_for_ms, LOCAL_LEASE_MS);
    lease_request.browser_absent = true;
    for attempt in 0..3 {
        lease_request.request_id = uuid::Uuid::new_v4().to_string();
        let now = NOW + 90_001 + attempt * 700_000;
        let lease = fixture
            .controller
            .local_lease("owner".into(), lease_request.clone(), now)
            .await
            .unwrap();
        assert_eq!(lease.wake.len(), 1);
        assert_eq!(lease.wake[0].task_id, started.tasks[0].id);
        assert!(fixture
            .controller
            .local_lease("owner".into(), lease_request.clone(), now)
            .await
            .unwrap()
            .wake
            .is_empty());
    }
    let path = fixture.dir.path().join("tasks.sqlite");
    drop(fixture.controller);
    let reopened = Controller::open(&path, NOW + 2_200_000).unwrap();
    let exhausted = reopened
        .local_lease("owner".into(), lease_request, NOW + 2_200_000)
        .await
        .unwrap();
    assert!(!exhausted.keep_awake && exhausted.wake.is_empty());
    let stopped = reopened
        .handle(
            "owner".into(),
            Request::Inspect {
                task_id: started.tasks[0].id.clone(),
            },
            NOW + 2_200_000,
        )
        .await
        .unwrap();
    assert_eq!(stopped.tasks[0].reason, "recovery_exhausted");
}

/// 明确 Retry-After 只延迟原任务；提示消失、重复快照和重启不能提前刷新或延长静态提示期限。
#[tokio::test]
async fn provider_retry_deadline_survives_missing_alert_and_restart_without_spending_budget() {
    let fixture = Fixture::new();
    let mut current = page();
    current["reply"] = Value::Null;
    let started = fixture
        .call(
            json!({"action":"start","request_id":uuid::Uuid::new_v4().to_string(),
        "binding":binding(),"options":{"objective":"","mode":"continue","limit":20,"delay":30,
        "policy":{"silence_seconds":120}},"page":current}),
            NOW,
        )
        .await
        .unwrap();
    current["activity"] = json!({"rate_limited":true,"retry_after":600});
    let held = fixture
        .call(operation("observe", &started, current.clone()), NOW + 1)
        .await
        .unwrap();
    assert_eq!(held.tasks[0].reason, "rate_limited");
    let deadline = held.tasks[0].repair_at.unwrap();
    let repeated = fixture
        .call(operation("observe", &held, current.clone()), NOW + 300_000)
        .await
        .unwrap();
    assert_eq!(repeated.tasks[0].repair_at, Some(deadline));
    current["activity"] = json!({});
    let no_alert = fixture
        .call(operation("observe", &held, current.clone()), NOW + 400_000)
        .await
        .unwrap();
    assert_eq!(no_alert.tasks[0].reason, "rate_limited");
    assert_eq!(no_alert.tasks[0].rounds, 0);
    let request = json!({"action":"repair","task_id":held.tasks[0].id,"generation":held.tasks[0].generation,
        "binding":binding(),"kind":"open","page":null});
    let path = fixture.dir.path().join("tasks.sqlite");
    drop(fixture.controller);
    let reopened = Controller::open(&path, NOW + 400_001).unwrap();
    assert!(matches!(
        reopened
            .handle(
                "owner".into(),
                serde_json::from_value(request.clone()).unwrap(),
                NOW + 599_999
            )
            .await,
        Err(Error::Conflict)
    ));
    let expired = reopened
        .handle(
            "owner".into(),
            serde_json::from_value(operation("observe", &held, current)).unwrap(),
            deadline,
        )
        .await
        .unwrap();
    assert_ne!(expired.tasks[0].reason, "rate_limited");
    assert_eq!(
        reopened
            .handle(
                "owner".into(),
                serde_json::from_value(request).unwrap(),
                deadline
            )
            .await
            .unwrap()
            .repair
            .as_deref(),
        Some("open")
    );
}

/// 精确提供方期限可短于默认五分钟，且静态提示到期后不能自行续期。
#[tokio::test]
async fn explicit_short_provider_wait_is_not_replaced_by_default_access_delay() {
    let fixture = Fixture::new();
    let ready = fixture.start("goal").await;
    let mut current = page();
    current["activity"] = json!({"rate_limited":true,"retry_after":10});
    let held = fixture
        .call(operation("observe", &ready, current.clone()), NOW + 1)
        .await
        .unwrap();
    assert_eq!(held.tasks[0].reason, "rate_limited");
    let released = fixture
        .call(operation("observe", &held, current), NOW + 10_001)
        .await
        .unwrap();
    assert_ne!(released.tasks[0].reason, "rate_limited");
    assert_eq!(released.tasks[0].state, "ready");
}

/// 自适应窗口识别精确 Pro 名称和高推理状态；关闭后全部使用显式秒数，不匹配 profile 子串。
#[tokio::test]
async fn adaptive_recovery_windows_respect_model_failure_and_fixed_timing() {
    for (model, deliberate, failed, adaptive, seconds) in [
        ("gpt-pro", false, false, true, 600),
        ("gpt-pro", true, true, true, 300),
        ("gpt-thinking", true, false, true, 1200),
        ("gpt-profile", false, false, true, 120),
        ("gpt-pro", true, false, false, 120),
    ] {
        let fixture = Fixture::new();
        let mut current = page();
        current["reply"] = Value::Null;
        current["activity"] =
            json!({"model":model,"deliberate":deliberate,"thinking_failed":failed});
        let started = fixture
            .call(
                json!({"action":"start","request_id":uuid::Uuid::new_v4().to_string(),
            "binding":binding(),"options":{"objective":"","mode":"continue","limit":20,"delay":30,
            "policy":{"silence_seconds":120,"adaptive_timing":adaptive}},"page":current}),
                NOW,
            )
            .await
            .unwrap();
        let watched = fixture
            .call(operation("observe", &started, current), NOW)
            .await
            .unwrap();
        assert_eq!(
            watched.tasks[0].repair_at,
            Some(NOW + seconds * 1000),
            "{model}/{adaptive}/{failed}"
        );
    }
}

/// 三次修复均未取得新文档后，空闲非终结状态必须暂停，不能保持 running 直到总授权到期。
#[tokio::test]
async fn exhausted_repairs_without_new_document_pause_idle_incomplete_turn() {
    let fixture = Fixture::new();
    let mut current = page();
    current["reply"] = Value::Null;
    let started = fixture
        .call(
            json!({"action":"start","request_id":uuid::Uuid::new_v4().to_string(),
        "binding":binding(),"options":{"objective":"","mode":"continue","limit":20,"delay":30,
        "policy":{"silence_seconds":120,"adaptive_timing":false}},"page":current}),
            NOW,
        )
        .await
        .unwrap();
    let mut state = started;
    for attempt in 0..3 {
        state = fixture.call(json!({"action":"repair","task_id":state.tasks[0].id,"generation":state.tasks[0].generation,
            "binding":binding(),"kind":"open","page":null}), NOW + attempt * 120_000).await.unwrap();
        assert_eq!(state.repair.as_deref(), Some("open"));
    }
    let first = fixture
        .call(operation("observe", &state, current.clone()), NOW + 360_000)
        .await
        .unwrap();
    let stopped = fixture
        .call(operation("observe", &first, current), NOW + 480_000)
        .await
        .unwrap();
    assert_eq!(stopped.tasks[0].state, "paused");
    assert_eq!(stopped.tasks[0].reason, "recovery_exhausted");
    assert_eq!(stopped.tasks[0].rounds, 0);
}

/// 公开进展只记录来源/计数与提交状态；重复心跳不增长，容量六十四条且跨重启保留。
#[tokio::test]
async fn source_progress_journal_is_bounded_deduplicated_private_and_durable() {
    let fixture = Fixture::new();
    let mut current = page();
    current["reply"] = Value::Null;
    current["busy"] = json!(true);
    current["activity"] = json!({"source_turn":"source-native-turn","revision":1});
    let started = fixture.call(json!({"action":"start","request_id":uuid::Uuid::new_v4().to_string(),
        "binding":binding(),"options":{"objective":"公开目标","mode":"goal","limit":20,"delay":30},"page":current}), NOW).await.unwrap();
    let first = fixture
        .call(operation("observe", &started, current.clone()), NOW + 1)
        .await
        .unwrap();
    let repeated = fixture
        .call(operation("observe", &first, current.clone()), NOW + 2)
        .await
        .unwrap();
    assert_eq!(
        first.tasks[0].journal.len(),
        repeated.tasks[0].journal.len()
    );
    for revision in 2..80 {
        current["activity"]["revision"] = json!(revision);
        fixture
            .call(
                operation("observe", &first, current.clone()),
                NOW + revision,
            )
            .await
            .unwrap();
    }
    let task_id = first.tasks[0].id.clone();
    let inspect = json!({"action":"inspect","task_id":task_id});
    let journal = fixture.call(inspect.clone(), NOW + 80).await.unwrap();
    assert_eq!(journal.tasks[0].journal.len(), 64);
    assert_eq!(journal.tasks[0].journal.last().unwrap().revision, 79);
    assert_eq!(
        journal.tasks[0].journal.last().unwrap().source_turn,
        "source-native-turn"
    );
    let encoded = serde_json::to_string(&journal.tasks[0].journal).unwrap();
    assert!(!encoded.contains("原始任务"));
    assert!(!encoded.contains("control_token"));
    let listed = fixture
        .call(
            json!({"action":"list","client":binding()["client"]}),
            NOW + 81,
        )
        .await
        .unwrap();
    assert_eq!(listed.tasks[0].journal.len(), 8);
    let path = fixture.dir.path().join("tasks.sqlite");
    drop(fixture.controller);
    let reopened = Controller::open(&path, NOW + 82).unwrap();
    let restored = reopened
        .handle(
            "owner".into(),
            serde_json::from_value(inspect).unwrap(),
            NOW + 83,
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_string(&restored.tasks[0].journal).unwrap(),
        encoded
    );
    assert!(matches!(
        reopened
            .handle("other-owner".into(), Request::Inspect { task_id }, NOW + 84)
            .await,
        Err(Error::NotFound)
    ));
}

/// Continue 的 finish 队列先完成原生握手，握手本身不移除或执行排队人工内容。
#[tokio::test]
async fn continue_finish_queue_bootstraps_native_before_handing_queued_work() {
    let fixture = Fixture::new();
    let watching = fixture.start("continue").await;
    let task = &watching.tasks[0];
    let queued = fixture.call(json!({"action":"queue","task_id":task.id,"generation":task.generation,"binding":task.binding,
        "kind":"message","text":"保留已完成工作，验证原请求","trigger":"finish"}), NOW).await.unwrap();
    let ready = fixture
        .call(operation("observe", &queued, page()), NOW)
        .await
        .unwrap();
    assert_eq!(ready.tasks[0].state, "ready");
    let prepared = fixture
        .call(operation("prepare", &ready, page()), NOW)
        .await
        .unwrap();
    assert!(!prepared
        .dispatch
        .as_ref()
        .unwrap()
        .text
        .contains("保留已完成工作，验证原请求"));
    let token = native_token(&prepared);
    let authorized = fixture
        .call(operation("authorize", &prepared, page()), NOW)
        .await
        .unwrap();
    fixture
        .bind_source(&prepared, &authorized, "finish-source", NOW + 1)
        .await;
    let next = fixture
        .controller
        .native(
            "owner".into(),
            native_request(
                &authorized,
                token,
                "finish",
                "complete",
                "原回合已结束，检查排队输入",
            ),
            native_facts(),
            NOW + 2,
        )
        .await
        .unwrap();
    assert!(!next.released);
    assert!(next.instructions.contains("保留已完成工作，验证原请求"));
    assert_eq!(next.sequence, 1);
    let inspected = fixture
        .call(
            json!({"action":"inspect","task_id":authorized.tasks[0].id}),
            NOW + 3,
        )
        .await
        .unwrap();
    assert_eq!(inspected.tasks[0].queue.len(), 1);
    assert_eq!(inspected.tasks[0].queue[0].delivery, "native");
}

/// 已原生绑定却提前给最终回复时，先显示缺少交接；静默窗口届满后暂停而不无限等待。
#[tokio::test]
async fn missing_native_finish_after_final_reply_pauses_after_fixed_window() {
    let fixture = Fixture::new();
    let ready = fixture.start("goal").await;
    let task = &ready.tasks[0];
    let updated = fixture.call(json!({"action":"update","task_id":task.id,"generation":task.generation,"binding":task.binding,
        "options":{"objective":"原请求","mode":"goal","limit":20,"delay":30,"policy":{"silence_seconds":120,"adaptive_timing":false}},"page":page()}), NOW).await.unwrap();
    let ready = fixture
        .call(operation("observe", &updated, page()), NOW)
        .await
        .unwrap();
    let prepared = fixture
        .call(operation("prepare", &ready, page()), NOW)
        .await
        .unwrap();
    let authorized = fixture
        .call(operation("authorize", &prepared, page()), NOW)
        .await
        .unwrap();
    fixture
        .bind_source(&prepared, &authorized, "native-receipt", NOW + 1)
        .await;
    let mut current = page();
    current["user_id"] = json!("native-receipt");
    current["user_text"] = json!(prepared.dispatch.unwrap().text);
    current["reply"] = json!({"id":"premature-final","user_id":"native-receipt","text":"过早结束，未调用finish","terminal":true});
    let receipt = fixture
        .call(operation("observe", &authorized, current.clone()), NOW + 2)
        .await
        .unwrap();
    let waiting = fixture
        .call(operation("observe", &receipt, current.clone()), NOW + 3)
        .await
        .unwrap();
    assert_eq!(waiting.tasks[0].reason, "native_finish_required");
    assert_eq!(waiting.tasks[0].state, "running");
    let stopped = fixture
        .call(operation("observe", &waiting, current), NOW + 120_003)
        .await
        .unwrap();
    assert_eq!(stopped.tasks[0].state, "paused");
    assert_eq!(stopped.tasks[0].reason, "native_finish_required");
}

/// 更新目标后晚到的未发送证明仅撤销旧正文；未知浏览器队列也不能在新人工回合中重放。
#[tokio::test]
async fn updated_objective_discards_old_unsent_body_and_human_change_fences_unknown_queue() {
    let fixture = Fixture::new();
    let ready = fixture.start("goal").await;
    let task = &ready.tasks[0];
    let queued = fixture.call(json!({"action":"queue","task_id":task.id,"generation":task.generation,"binding":task.binding,
        "kind":"message","text":"这条操作不能重发","trigger":"after_turn"}), NOW).await.unwrap();
    let ready = fixture
        .call(operation("observe", &queued, page()), NOW)
        .await
        .unwrap();
    let prepared = fixture
        .call(operation("prepare", &ready, page()), NOW)
        .await
        .unwrap();
    let authorized = fixture
        .call(operation("authorize", &prepared, page()), NOW)
        .await
        .unwrap();
    let task = &authorized.tasks[0];
    let updated = fixture.call(json!({"action":"update","task_id":task.id,"generation":task.generation,"binding":task.binding,
        "options":{"objective":"更新后的目标","mode":"goal","limit":20,"delay":30},"page":page()}), NOW + 1).await.unwrap();
    assert_eq!(updated.tasks[0].queue[0].delivery, "uncertain");
    let cleared = fixture
        .call(operation("not_sent", &authorized, page()), NOW + 2)
        .await
        .unwrap();
    assert_eq!(cleared.tasks[0].state, "running");
    assert!(cleared.tasks[0].delivery_id.is_none());
    assert_eq!(cleared.tasks[0].queue[0].delivery, "queued");
    let ready = fixture
        .call(operation("observe", &cleared, page()), NOW + 30_002)
        .await
        .unwrap();
    let prepared = fixture
        .call(operation("prepare", &ready, page()), NOW + 30_002)
        .await
        .unwrap();
    let authorized = fixture
        .call(operation("authorize", &prepared, page()), NOW + 30_002)
        .await
        .unwrap();
    let mut human = page();
    human["user_id"] = json!("new-human-request");
    human["user_text"] = json!("先检查这条新要求");
    human["reply"] = Value::Null;
    let changed = fixture
        .call(operation("observe", &authorized, human), NOW + 30_003)
        .await
        .unwrap();
    assert_eq!(changed.tasks[0].queue[0].delivery, "uncertain");
    assert!(changed.tasks[0].delivery_id.is_none());
}

/// 原生 finish 只在主聊天来源确认后释放；晚到重复页面观察不能重新投递或恢复旧权限。
#[tokio::test]
async fn native_finish_after_source_confirmation_and_late_observation_releases_once() {
    let fixture = Fixture::new();
    let ready = fixture.start("goal").await;
    let prepared = fixture
        .call(operation("prepare", &ready, page()), NOW)
        .await
        .unwrap();
    let token = native_token(&prepared);
    let authorized = fixture
        .call(operation("authorize", &prepared, page()), NOW)
        .await
        .unwrap();
    let bound = fixture
        .bind_source(&prepared, &authorized, "native-user", NOW + 1)
        .await;
    assert!(!bound.released);
    let finish = native_request(
        &authorized,
        token,
        "finish",
        "complete",
        "原请求及全部验证均已完成",
    );
    let result = fixture
        .controller
        .native("owner".into(), finish.clone(), native_facts(), NOW + 2)
        .await
        .unwrap();
    assert!(result.released);
    assert!(result.control_token.is_none());
    assert_eq!(result.sequence, 0);
    let repeated = fixture
        .controller
        .native("owner".into(), finish, native_facts(), NOW + 3)
        .await
        .unwrap();
    assert_eq!(repeated.instructions, result.instructions);
    let mut receipt = page();
    receipt["user_id"] = json!("native-user");
    receipt["user_text"] = json!(prepared.dispatch.unwrap().text);
    let final_state = fixture
        .call(operation("observe", &authorized, receipt), NOW + 4)
        .await
        .unwrap();
    assert_eq!(final_state.tasks[0].state, "watching");
    assert!(final_state.tasks[0].delivery_id.is_none());
    assert!(final_state.dispatch.is_none());
}

/// 同令牌的重复 finish 仅重读原结果；Window、账户、参数或旧版本变化均不能获得另一轮工作。
#[tokio::test]
async fn native_source_window_token_and_replay_are_fenced() {
    let fixture = Fixture::new();
    let ready = fixture.start("goal").await;
    let prepared = fixture
        .call(operation("prepare", &ready, page()), NOW)
        .await
        .unwrap();
    let token = native_token(&prepared);
    assert!(matches!(
        fixture
            .controller
            .native(
                "owner".into(),
                native_request(&prepared, token.clone(), "bind", "", ""),
                native_facts(),
                NOW
            )
            .await,
        Err(Error::Conflict)
    ));
    let authorized = fixture
        .call(operation("authorize", &prepared, page()), NOW)
        .await
        .unwrap();
    fixture
        .bind_source(&prepared, &authorized, "native-source", NOW)
        .await;
    let request = native_request(
        &authorized,
        token,
        "finish",
        "continue",
        "完整实现已提交，继续验证运行行为",
    );
    let first = fixture
        .controller
        .native("owner".into(), request.clone(), native_facts(), NOW + 1)
        .await
        .unwrap();
    assert!(!first.released);
    assert_eq!(first.sequence, 1);
    let repeat = fixture
        .controller
        .native("owner".into(), request.clone(), native_facts(), NOW + 2)
        .await
        .unwrap();
    assert_eq!(repeat.sequence, first.sequence);
    assert_eq!(repeat.control_token, first.control_token);
    let mut worker = native_facts();
    worker.window = "b".repeat(64);
    assert!(matches!(
        fixture
            .controller
            .native("owner".into(), request.clone(), worker, NOW + 2)
            .await,
        Err(Error::Conflict)
    ));
    assert!(matches!(
        fixture
            .controller
            .native("foreign".into(), request.clone(), native_facts(), NOW + 2)
            .await,
        Err(Error::NotFound)
    ));
    let mut changed = request;
    changed.summary = "相同请求改成其他内容".into();
    assert!(matches!(
        fixture
            .controller
            .native("owner".into(), changed, native_facts(), NOW + 2)
            .await,
        Err(Error::Conflict)
    ));
}

/// 真正工具/审批等待不释放回合或交出下一工作；人工暂停后旧回调失效。
#[tokio::test]
async fn native_waits_for_tools_and_humans_and_obeys_pause() {
    let fixture = Fixture::new();
    let ready = fixture.start("goal").await;
    let prepared = fixture
        .call(operation("prepare", &ready, page()), NOW)
        .await
        .unwrap();
    let token = native_token(&prepared);
    let authorized = fixture
        .call(operation("authorize", &prepared, page()), NOW)
        .await
        .unwrap();
    fixture
        .bind_source(&prepared, &authorized, "native-source", NOW)
        .await;
    let request = native_request(&authorized, token, "finish", "complete", "已有完成依据");
    let mut facts = native_facts();
    facts.busy = true;
    let waiting = fixture
        .controller
        .native("owner".into(), request.clone(), facts, NOW + 1)
        .await
        .unwrap();
    assert!(waiting.waiting);
    assert!(!waiting.released);
    assert_eq!(waiting.sequence, 0);
    let mut facts = native_facts();
    facts.approval = true;
    assert!(
        fixture
            .controller
            .native("owner".into(), request.clone(), facts, NOW + 2)
            .await
            .unwrap()
            .waiting
    );
    let mut pause = operation("pause", &authorized, page());
    pause.as_object_mut().unwrap().remove("page");
    fixture.call(pause, NOW + 3).await.unwrap();
    assert!(matches!(
        fixture
            .controller
            .native("owner".into(), request, native_facts(), NOW + 4)
            .await,
        Err(Error::Conflict)
    ));
}

/// finish 队列通过同一工具结果交接，跨重启重读不出队；下一有效令牌才确认收到。
#[tokio::test]
async fn native_finish_queue_survives_lost_response_and_dequeues_only_on_next_token() {
    let mut fixture = Fixture::new();
    let ready = fixture.start("goal").await;
    let prepared = fixture
        .call(operation("prepare", &ready, page()), NOW)
        .await
        .unwrap();
    let token = native_token(&prepared);
    let authorized = fixture
        .call(operation("authorize", &prepared, page()), NOW)
        .await
        .unwrap();
    fixture
        .bind_source(&prepared, &authorized, "native-source", NOW)
        .await;
    let task = &authorized.tasks[0];
    let queued = fixture.call(json!({"action":"queue","task_id":task.id,"generation":task.generation,"binding":task.binding,
        "kind":"message","text":"检查实际运行结果并修复失败项","trigger":"finish"}), NOW + 1).await.unwrap();
    assert_eq!(queued.tasks[0].generation, task.generation);
    let request = native_request(
        &authorized,
        token,
        "finish",
        "complete",
        "本阶段已经完成，但仍需处理用户排队核验",
    );
    let next = fixture
        .controller
        .native("owner".into(), request.clone(), native_facts(), NOW + 2)
        .await
        .unwrap();
    assert!(!next.released);
    assert!(next.instructions.contains("检查实际运行结果并修复失败项"));
    fixture.controller =
        Controller::open(&fixture.dir.path().join("tasks.sqlite"), NOW + 3).unwrap();
    let repeated = fixture
        .controller
        .native("owner".into(), request, native_facts(), NOW + 4)
        .await
        .unwrap();
    assert_eq!(repeated.instructions, next.instructions);
    assert_eq!(repeated.sequence, next.sequence);
    let observed = fixture
        .call(json!({"action":"inspect","task_id":task.id}), NOW + 4)
        .await
        .unwrap();
    assert_eq!(observed.tasks[0].queue.len(), 1);
    assert_eq!(observed.tasks[0].queue[0].delivery, "native");
    assert!(!serde_json::to_string(&observed)
        .unwrap()
        .contains("native_principal"));
    let token = next.control_token.unwrap();
    fixture
        .controller
        .native(
            "owner".into(),
            native_request(&authorized, token.clone(), "bind", "", ""),
            native_facts(),
            NOW + 5,
        )
        .await
        .unwrap();
    let observed = fixture
        .call(json!({"action":"inspect","task_id":task.id}), NOW + 6)
        .await
        .unwrap();
    assert!(observed.tasks[0].queue.is_empty());
    assert_eq!(observed.tasks[0].rounds, 2);
    let done = fixture
        .controller
        .native(
            "owner".into(),
            native_request(
                &authorized,
                token,
                "finish",
                "complete",
                "用户要求的实际运行核验与失败修复均已完成",
            ),
            native_facts(),
            NOW + 7,
        )
        .await
        .unwrap();
    assert!(done.released);
}

/// after_turn 的人工输入不在原生工具内提前执行；释放当前回合后才由浏览器准备消息。
#[tokio::test]
async fn native_after_turn_queue_yields_before_browser_delivery() {
    let fixture = Fixture::new();
    let ready = fixture.start("goal").await;
    let prepared = fixture
        .call(operation("prepare", &ready, page()), NOW)
        .await
        .unwrap();
    let token = native_token(&prepared);
    let authorized = fixture
        .call(operation("authorize", &prepared, page()), NOW)
        .await
        .unwrap();
    fixture
        .bind_source(&prepared, &authorized, "yield-user", NOW)
        .await;
    let task = &authorized.tasks[0];
    fixture.call(json!({"action":"queue","task_id":task.id,"generation":task.generation,"binding":task.binding,
        "kind":"message","text":"下一回合再核对界面操作","trigger":"after_turn"}), NOW + 1).await.unwrap();
    let yielded = fixture
        .controller
        .native(
            "owner".into(),
            native_request(&authorized, token, "finish", "continue", "当前阶段已结束"),
            native_facts(),
            NOW + 2,
        )
        .await
        .unwrap();
    assert!(yielded.released);
    assert!(!yielded.instructions.contains("下一回合再核对界面操作"));
    let mut current = page();
    current["user_id"] = json!("yield-user");
    current["user_text"] = json!(prepared.dispatch.unwrap().text);
    current["reply"] =
        json!({"id":"yield-final","user_id":"yield-user","text":"当前阶段已结束","terminal":true});
    let received = fixture
        .call(operation("observe", &authorized, current.clone()), NOW + 3)
        .await
        .unwrap();
    let ready = fixture
        .call(operation("observe", &received, current.clone()), NOW + 4)
        .await
        .unwrap();
    let next = fixture
        .call(operation("prepare", &ready, current), NOW + 30_003)
        .await
        .unwrap();
    assert!(next
        .dispatch
        .unwrap()
        .text
        .contains("下一回合再核对界面操作"));
}

/// 准备正文只消耗准备权；暂停事务提交后，晚到的发送前授权必须失败且不能增加轮数。
#[tokio::test]
async fn pause_revokes_preparation_before_send_authorization() {
    let fixture = Fixture::new();
    let ready = fixture.start("goal").await;
    let prepared = fixture
        .call(operation("prepare", &ready, page()), NOW)
        .await
        .unwrap();
    assert_eq!(prepared.tasks[0].state, "prepared");
    assert_eq!(prepared.tasks[0].rounds, 0);
    assert!(!prepared.tasks[0].send_authorized);
    let mut pause = operation("pause", &prepared, page());
    pause.as_object_mut().unwrap().remove("page");
    let paused = fixture.call(pause, NOW + 1).await.unwrap();
    assert_eq!(paused.tasks[0].state, "paused");
    assert!(matches!(
        fixture
            .call(operation("authorize", &prepared, page()), NOW + 2)
            .await,
        Err(Error::Conflict)
    ));
}

/// 准备响应丢失后重启可重新准备；真正授权提交后，重启和重复请求都不得再次获得发送权。
#[tokio::test]
async fn restart_requeues_only_never_authorized_preparation() {
    let mut fixture = Fixture::new();
    let ready = fixture.start("goal").await;
    let prepared = fixture
        .call(operation("prepare", &ready, page()), NOW)
        .await
        .unwrap();
    fixture.controller =
        Controller::open(&fixture.dir.path().join("tasks.sqlite"), NOW + 1).unwrap();
    let again = fixture
        .call(operation("prepare", &prepared, page()), NOW + 2)
        .await
        .unwrap();
    assert_eq!(
        again.dispatch.as_ref().unwrap().text,
        prepared.dispatch.unwrap().text
    );
    let sent = fixture
        .call(operation("authorize", &again, page()), NOW + 3)
        .await
        .unwrap();
    assert_eq!(sent.tasks[0].rounds, 1);
    assert!(sent.tasks[0].send_authorized);
    fixture.controller =
        Controller::open(&fixture.dir.path().join("tasks.sqlite"), NOW + 4).unwrap();
    assert!(matches!(
        fixture
            .call(operation("prepare", &sent, page()), NOW + 5)
            .await,
        Err(Error::Conflict)
    ));
    assert!(matches!(
        fixture
            .call(operation("authorize", &sent, page()), NOW + 5)
            .await,
        Err(Error::Conflict)
    ));
}

/// 仅页面明确没有点击且草稿已清除的反馈允许一次重排；第二次确定未发送后暂停。
#[tokio::test]
async fn definite_not_sent_retries_once_and_unknown_delivery_never_replays() {
    let fixture = Fixture::new();
    let ready = fixture.start("goal").await;
    let prepared = fixture
        .call(operation("prepare", &ready, page()), NOW)
        .await
        .unwrap();
    let sent = fixture
        .call(operation("authorize", &prepared, page()), NOW)
        .await
        .unwrap();
    let retry = fixture
        .call(operation("not_sent", &sent, page()), NOW + 1)
        .await
        .unwrap();
    assert_eq!(retry.tasks[0].state, "ready");
    let again = fixture
        .call(operation("prepare", &retry, page()), NOW + 30_001)
        .await
        .unwrap();
    let authorized = fixture
        .call(operation("authorize", &again, page()), NOW + 30_001)
        .await
        .unwrap();
    let exhausted = fixture
        .call(operation("not_sent", &authorized, page()), NOW + 30_002)
        .await
        .unwrap();
    assert_eq!(exhausted.tasks[0].reason, "send_failed");
    assert_eq!(exhausted.tasks[0].rounds, 2);
}

/// Continue 完成后保持开关，新人工请求撤销旧轮次并进入 running；轮数和授权不会因新消息重置。
#[tokio::test]
async fn continuous_switch_follows_new_human_request_without_restarting_completed_turn() {
    let fixture = Fixture::new();
    let watching = fixture.start("continue").await;
    assert_eq!(watching.tasks[0].state, "watching");
    let unchanged = fixture
        .call(operation("observe", &watching, page()), NOW + 1)
        .await
        .unwrap();
    assert_eq!(unchanged.tasks[0].state, "watching");
    let mut next = page();
    next["user_id"] = json!("new-human");
    next["user_text"] = json!("继续原任务并检查新要求");
    next["reply"] = Value::Null;
    next["busy"] = json!(true);
    let adopted = fixture
        .call(operation("observe", &watching, next), NOW + 2)
        .await
        .unwrap();
    assert_eq!(adopted.tasks[0].state, "running");
    assert_eq!(
        adopted.tasks[0].generation,
        watching.tasks[0].generation + 1
    );
    assert!(adopted.dispatch.is_none());
}

/// 审批是等待状态；用户处理后原消息可继续准备，主动 Stop 则撤销自动恢复。
#[tokio::test]
async fn approval_wait_resumes_but_explicit_stop_pauses() {
    let fixture = Fixture::new();
    let ready = fixture.start("goal").await;
    let mut approval = page();
    approval["approval"] = json!(true);
    let held = fixture
        .call(operation("observe", &ready, approval), NOW + 1)
        .await
        .unwrap();
    assert_eq!(held.tasks[0].state, "ready");
    assert_eq!(held.tasks[0].reason, "approval_required");
    let resumed = fixture
        .call(operation("prepare", &held, page()), NOW + 2)
        .await
        .unwrap();
    assert!(resumed.dispatch.is_some());
    let mut stopped = page();
    stopped["activity"] = json!({"stop_requested":true});
    let paused = fixture
        .call(operation("observe", &resumed, stopped), NOW + 3)
        .await
        .unwrap();
    assert_eq!(paused.tasks[0].reason, "user_stopped");
    assert!(matches!(
        fixture
            .call(operation("authorize", &resumed, page()), NOW + 4)
            .await,
        Err(Error::Conflict)
    ));
}

/// Goal 三次相同答案触发无进展保护；每轮均经过真实准备、授权与确切回执。
#[tokio::test]
async fn goal_repetition_is_bounded_without_using_an_external_decision_model() {
    let fixture = Fixture::new();
    let mut ready = fixture.start("goal").await;
    let mut current = page();
    for round in 0..3 {
        let now = NOW + round * 30_001;
        let prepared = fixture
            .call(operation("prepare", &ready, current.clone()), now)
            .await
            .unwrap();
        let authorized = fixture
            .call(operation("authorize", &prepared, current.clone()), now)
            .await
            .unwrap();
        current["user_id"] = json!(format!("auto-{round}"));
        current["user_text"] = json!(prepared.dispatch.unwrap().text);
        let task = &authorized.tasks[0];
        current["reply"] = json!({"id":format!("reply-{round}"),"user_id":format!("auto-{round}"),"terminal":true,
            "text":format!("仍然需要继续验证。\n[[WEBCODEX_GOAL:{}:{}:CONTINUE]]", task.id, task.generation)});
        let received = fixture
            .call(operation("observe", &authorized, current.clone()), now + 1)
            .await
            .unwrap();
        ready = fixture
            .call(operation("observe", &received, current.clone()), now + 2)
            .await
            .unwrap();
    }
    assert_eq!(ready.tasks[0].state, "paused");
    assert_eq!(ready.tasks[0].reason, "no_progress");
}

/// 工作流首条包含全部阶段；后加入的人工消息优先，准备/授权都不能提前移除队列项。
#[tokio::test]
async fn manual_queue_precedes_workflow_and_acknowledgement_alone_dequeues() {
    let fixture = Fixture::new();
    let ready = fixture.start("goal").await;
    let task = &ready.tasks[0];
    let workflow = fixture.call(json!({"action":"queue","task_id":task.id,"generation":task.generation,"binding":task.binding,
        "kind":"workflow","stages":["完成原请求的全部实现","运行应用并验证全部要求"],"trigger":"after_turn"}), NOW).await.unwrap();
    let task = &workflow.tasks[0];
    let queued = fixture.call(json!({"action":"queue","task_id":task.id,"generation":task.generation,"binding":task.binding,
        "kind":"message","text":"先保留当前配置","trigger":"after_turn"}), NOW).await.unwrap();
    assert_eq!(queued.tasks[0].queue.len(), 3);
    assert_eq!(queued.tasks[0].queue[0].kind, "message");
    let reserved = fixture
        .call(operation("observe", &queued, page()), NOW + 1)
        .await
        .unwrap();
    let prepared = fixture
        .call(operation("prepare", &reserved, page()), NOW + 1)
        .await
        .unwrap();
    assert!(prepared
        .dispatch
        .as_ref()
        .unwrap()
        .text
        .contains("先保留当前配置"));
    assert_eq!(prepared.tasks[0].queue.len(), 3);
    let authorized = fixture
        .call(operation("authorize", &prepared, page()), NOW + 1)
        .await
        .unwrap();
    let mut current = page();
    current["user_id"] = json!("queued-human");
    current["user_text"] = json!(prepared.dispatch.unwrap().text);
    current["reply"] =
        json!({"id":"new-final","user_id":"queued-human","text":"已保留当前配置","terminal":true});
    let received = fixture
        .call(operation("observe", &authorized, current.clone()), NOW + 2)
        .await
        .unwrap();
    assert_eq!(received.tasks[0].queue.len(), 2);
    let next = fixture
        .call(operation("observe", &received, current.clone()), NOW + 3)
        .await
        .unwrap();
    let first = fixture
        .call(operation("prepare", &next, current), NOW + 30_002)
        .await
        .unwrap();
    let text = first.dispatch.unwrap().text;
    assert!(text.contains("完成原请求的全部实现"));
    assert!(text.contains("运行应用并验证全部要求"));
    assert!(text.contains("第一阶段必须完成原始请求的完整实现"));
}

/// 规划请求发送给当前聊天；只有匹配当前任务版本的完整 JSON 才会原子转换为阶段队列。
#[tokio::test]
async fn current_chat_plan_becomes_full_workflow_without_api_or_helper_chat() {
    let fixture = Fixture::new();
    let ready = fixture.start("goal").await;
    let task = &ready.tasks[0];
    let queued = fixture.call(json!({"action":"queue","task_id":task.id,"generation":task.generation,"binding":task.binding,
        "kind":"plan","trigger":"after_turn"}), NOW).await.unwrap();
    let reserved = fixture
        .call(operation("observe", &queued, page()), NOW)
        .await
        .unwrap();
    let prepared = fixture
        .call(operation("prepare", &reserved, page()), NOW)
        .await
        .unwrap();
    assert!(prepared
        .dispatch
        .as_ref()
        .unwrap()
        .text
        .contains("不调用决策 API"));
    let authorized = fixture
        .call(operation("authorize", &prepared, page()), NOW)
        .await
        .unwrap();
    let task = &authorized.tasks[0];
    let mut current = page();
    current["user_id"] = json!("plan-user");
    current["user_text"] = json!(prepared.dispatch.unwrap().text);
    current["reply"] = json!({"id":"plan-reply","user_id":"plan-user","terminal":true,
        "text":format!("[[WEBCODEX_PLAN:{}:{}]]\n{{\"stages\":[\"完整实现原请求\",\"检查运行行为并修复问题\"]}}\n[[/WEBCODEX_PLAN]]", task.id, task.generation)});
    let received = fixture
        .call(operation("observe", &authorized, current.clone()), NOW + 1)
        .await
        .unwrap();
    assert!(received.tasks[0].queue.is_empty());
    let workflow = fixture
        .call(operation("observe", &received, current), NOW + 2)
        .await
        .unwrap();
    assert_eq!(workflow.tasks[0].queue.len(), 2);
    assert_eq!(workflow.tasks[0].state, "ready");
    assert!(workflow.tasks[0]
        .queue
        .iter()
        .all(|entry| entry.kind == "stage"));
}
