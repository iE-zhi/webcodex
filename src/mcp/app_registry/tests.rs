//! Headless MCP 仅提供 Computer 截图与自动续跑所需的资源。
use super::*;
use std::collections::HashSet;

/// 验证资源清单不包含普通可视化界面，并保持协议身份唯一。
#[test]
fn headless_app_registry_has_only_functional_carriers() {
    let expected = [
        MCP_COMPUTER_UI_RESOURCE_URI,
        MCP_AGENT_CONTINUATION_UI_RESOURCE_URI,
        MCP_JOB_TERMINAL_CONTINUATION_UI_RESOURCE_URI,
    ];
    let listed = resources_list(None);
    let uris = listed["resources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|resource| resource["uri"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(uris, expected);
    assert_eq!(BUILTIN_MCP_APPS.len(), expected.len());
    let mut identities = HashSet::new();
    for app in BUILTIN_MCP_APPS {
        assert!(identities.insert(app.uri));
        assert!(std::ptr::eq(for_uri(app.uri).unwrap(), app));
        let content = app.read(None);
        assert_eq!(content["contents"][0]["uri"], app.uri);
        assert_eq!(content["contents"][0]["text"], app.html);
    }
    for removed in [
        "ui://webcodex/pdf/v7",
        "ui://webcodex/docx/v2",
        "ui://webcodex/spreadsheet/v3",
        "ui://webcodex/workbench/v5",
        "ui://webcodex/work-result/v31",
        "ui://webcodex/goal-plan/v9",
        "ui://webcodex/changes/v4",
    ] {
        assert!(
            for_uri(removed).is_none(),
            "{removed} must not be registered"
        );
    }
    for removed in [
        "present_pdf",
        "present_docx",
        "present_spreadsheet",
        "present_work_result",
        "present_goal_plan",
        "open_webcodex_workbench",
    ] {
        assert!(for_tool(removed).is_none(), "{removed} must not be bound");
    }
}

/// 验证保留的 Computer 和 Host 续跑资源维持最小权限与固定内容。
#[test]
fn headless_app_carriers_keep_narrow_tool_binding_and_csp() {
    assert!(for_tool("observe_computer").is_none());
    assert_eq!(
        for_tool("present_agent_continuation").unwrap().uri,
        MCP_AGENT_CONTINUATION_UI_RESOURCE_URI
    );
    assert_eq!(
        for_tool("wait_for_agent_events").unwrap().uri,
        MCP_AGENT_CONTINUATION_UI_RESOURCE_URI
    );
    assert_eq!(
        for_tool("present_job_terminal_continuation").unwrap().uri,
        MCP_JOB_TERMINAL_CONTINUATION_UI_RESOURCE_URI
    );

    for domain in [None, Some("https://self-host.example")] {
        let meta = resource_meta(domain);
        assert_eq!(
            meta["ui"]["csp"],
            json!({"connectDomains":[],"resourceDomains":[]})
        );
        assert_eq!(meta["ui"]["prefersBorder"], true);
        assert_eq!(meta["ui"].get("domain"), domain.map(Value::from).as_ref());
        for app in BUILTIN_MCP_APPS {
            let result = app.read(domain);
            assert_eq!(result["contents"][0]["_meta"]["ui"], meta["ui"]);
            assert_eq!(result["contents"][0]["mimeType"], MCP_UI_RESOURCE_MIME_TYPE);
            assert_eq!(
                app.cache_ttl_ms,
                (app.uri == MCP_COMPUTER_UI_RESOURCE_URI).then_some(0)
            );
        }
    }
}
