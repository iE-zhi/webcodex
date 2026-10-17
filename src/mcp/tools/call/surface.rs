//! Headless MCP 工具入口准入：只保留核心工具与 Host 自动续跑。
use super::*;

pub(super) struct RuntimeSurface {
    pub(super) capabilities: ToolProtocolCapabilities,
    pub(super) discard_recorder: bool,
    pub(super) echo_app_content: bool,
    pub(super) host_continuation: bool,
}

impl RuntimeSurface {
    /// 按协议版本、服务能力及工具权限校验直接工具调用。
    ///
    /// # 参数
    /// - `context`：已认证的 MCP 请求上下文。
    /// - `diagnostics`：记录准入拒绝及请求生命周期。
    /// - `params`：经过解析的工具调用。
    /// - `via_adaptive_runtime_gateway`：调用是否来自受准入的统一运行时入口。
    /// - `_result_presentation`：保留现有调用接口，Headless 不再使用展示投影。
    pub(super) fn admit(
        context: &CallContext<'_>,
        diagnostics: &mut CallDiagnostics<'_>,
        params: &mut McpToolCallParams,
        via_adaptive_runtime_gateway: bool,
        _result_presentation: McpToolResultPresentation,
    ) -> Result<Self, McpOutcome> {
        let stateless_2026 = context.stateless_2026;
        let host_apps = context.server_mcp_apps_enabled && stateless_2026;
        let auth = context.auth;
        let id = context.id.clone();
        let lifecycle = &mut diagnostics.lifecycle;

        let agent_continuation = host_apps && is_agent_continuation_app_tool_name(&params.name);
        let job_continuation =
            host_apps && is_job_terminal_continuation_app_tool_name(&params.name);
        let operator_extension =
            stateless_2026 && runtime_tool_operator_extension_family(&params.name).is_some();

        let direct_denied = crate::model_surface::is_retired_display_tool(&params.name)
            || (!agent_continuation
                && !job_continuation
                && !operator_extension
                && !via_adaptive_runtime_gateway
                && !is_adaptive_runtime_direct_tool(&params.name));

        if direct_denied {
            if let Some(lc) = lifecycle.as_deref() {
                lc.dispatch_failed("direct_route_denied");
                lc.dispatch_finished(false, Some(false), "direct_route_denied");
            }
            let message = if mcp_adaptive_runtime_gateway_target_route(&params.name, stateless_2026)
                != crate::model_surface::AdaptiveRuntimeGatewayTargetRoute::Gateway
            {
                format!(
                    "tool '{}' is unavailable on the current MCP surface; refresh tools/list and select a current callable",
                    params.name
                )
            } else if !crate::tool_runtime::kernel::runtime_tool_scope_allows_discovery(
                auth,
                &params.name,
            ) {
                format!(
                    "tool '{}' is unavailable under the current authorization; check granted scopes and current tools/list",
                    params.name
                )
            } else {
                format!(
                    "tool '{}' is not directly callable on Adaptive Runtime; use call_runtime_tool with its current canonical arguments and normal authorization",
                    params.name
                )
            };
            return Err(McpOutcome::BadRequest(rpc_error(id, -32602, message)));
        }

        Ok(Self {
            capabilities: ToolProtocolCapabilities {
                control_sidecars: stateless_2026,
                context_sidecar: stateless_2026,
                skill_runtime: stateless_2026,
                skill_management: stateless_2026,
                memory_surface: stateless_2026,
                trace_diagnostics: stateless_2026,
                goal_plan_app: false,
                work_result_app: false,
                artifact_app: false,
                agent_continuation_app: host_apps,
            },
            discard_recorder: false,
            echo_app_content: agent_continuation || job_continuation,
            host_continuation: agent_continuation || job_continuation,
        })
    }
}
