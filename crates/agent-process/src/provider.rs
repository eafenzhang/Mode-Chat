//! `provider/*` 管理面 facade（Provider 连通性测试）。
//!
//! 行为真源：`modeAgentService.testModelConnectivity`——一次正式执行链的连通性探测，
//! Provider 鉴权/headers/reasoning 映射全部由 Agent 负责；Host 只转发选择并校验结果。

use agent_protocol::provider::{
    ProviderTestModelConnectivityParams, ProviderTestModelConnectivityResult,
    PROVIDER_TEST_MODEL_CONNECTIVITY_METHOD,
};
use agent_protocol::session::{ModelSelection, WorkspaceRef};

use crate::control::{AgentControlClient, AgentWorkspaceTarget, ControlError, RuntimePolicy};

impl AgentControlClient {
    /// `provider/testModelConnectivity`：成功必须显式 `{success:true}`。
    pub async fn test_model_connectivity(
        &self,
        target: &AgentWorkspaceTarget,
        selection: ModelSelection,
    ) -> Result<(), ControlError> {
        let process = self
            .client_for(target, RuntimePolicy::StartIfNeeded)
            .await?;
        let params = ProviderTestModelConnectivityParams {
            workspace: WorkspaceRef {
                workspace_path: target.workspace_path.clone(),
                workspace_key: target.workspace_key(),
                workspace_identity: target.workspace_identity.clone(),
                remote_session_id: None,
                extra: Default::default(),
            },
            selection,
            extra: Default::default(),
        };
        let value = serde_json::to_value(&params).map_err(|error| {
            ControlError::InvalidParams(format!(
                "{PROVIDER_TEST_MODEL_CONNECTIVITY_METHOD} params serialization failed: {error}"
            ))
        })?;
        let response = process.client().request(PROVIDER_TEST_MODEL_CONNECTIVITY_METHOD, Some(value)).await?;
        let result: ProviderTestModelConnectivityResult = serde_json::from_value(response).map_err(
            |error| {
                ControlError::InvalidResult(format!(
                    "invalid {PROVIDER_TEST_MODEL_CONNECTIVITY_METHOD} result: {error}"
                ))
            },
        )?;
        result.validate().map_err(ControlError::InvalidResult)?;
        Ok(())
    }
}
