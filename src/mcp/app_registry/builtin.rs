//! 保留 Computer 截图资源及无需人工交互的 Host 自动续跑载体。
//! 它们承载协议能力，不提供常规项目管理或结果展示界面。
use super::{AppListing, BundledMcpApp};

pub(in crate::mcp) const MCP_COMPUTER_UI_RESOURCE_URI: &str = "ui://webcodex/computer/v12";
pub(in crate::mcp) const MCP_COMPUTER_UI_RESOURCE_TTL_MS: u64 = 0;
pub(in crate::mcp) const MCP_AGENT_CONTINUATION_UI_RESOURCE_URI: &str =
    "ui://webcodex/agent-continuation/v21";
pub(in crate::mcp) const MCP_JOB_TERMINAL_CONTINUATION_UI_RESOURCE_URI: &str =
    "ui://webcodex/job-terminal-continuation/v2";

pub(in crate::mcp) const MCP_COMPUTER_APP_HTML: &str = include_str!("../../mcp_computer_app.html");
pub(in crate::mcp) const MCP_AGENT_CONTINUATION_APP_HTML: &str =
    include_str!("../../mcp_agent_continuation_app.html");
pub(in crate::mcp) const MCP_JOB_TERMINAL_CONTINUATION_APP_HTML: &str =
    include_str!("../../mcp_job_terminal_continuation_app.html");

/// 仅公布 Computer 原生截图及 Host 续跑协议需要的资源。
pub(super) static BUILTIN_MCP_APPS: &[BundledMcpApp] = &[
    BundledMcpApp {
        listing: Some(AppListing {
            name: "WebCodex Computer",
            description:
                "Read-only Computer screenshot carrier for native observe_computer results.",
        }),
        cache_ttl_ms: Some(MCP_COMPUTER_UI_RESOURCE_TTL_MS),
        ..BundledMcpApp::template(MCP_COMPUTER_UI_RESOURCE_URI, MCP_COMPUTER_APP_HTML)
    },
    BundledMcpApp {
        listing: Some(AppListing {
            name: "WebCodex Agent Continuation",
            description: "Host-side continuation transport for an authorized durable Agent wake.",
        }),
        tools: &["present_agent_continuation", "wait_for_agent_events"],
        ..BundledMcpApp::template(
            MCP_AGENT_CONTINUATION_UI_RESOURCE_URI,
            MCP_AGENT_CONTINUATION_APP_HTML,
        )
    },
    BundledMcpApp {
        listing: Some(AppListing {
            name: "WebCodex Job Continuation",
            description: "Host-side continuation transport for an authorized terminal Job wait.",
        }),
        tools: &["present_job_terminal_continuation"],
        ..BundledMcpApp::template(
            MCP_JOB_TERMINAL_CONTINUATION_UI_RESOURCE_URI,
            MCP_JOB_TERMINAL_CONTINUATION_APP_HTML,
        )
    },
];
