//! `provider/*` 协议镜像（Provider 管理面）。
//!
//! 真源：`packages/shared/src/mode-protocol/index.ts` 的
//! `modeProviderTestModelConnectivityParamsSchema` / `...ResultSchema`（strict）。

use serde::{Deserialize, Serialize};

use crate::frame::ExtraFields;
use crate::session::{ModelSelection, WorkspaceRef};

/// `provider/testModelConnectivity`。
pub const PROVIDER_TEST_MODEL_CONNECTIVITY_METHOD: &str = "provider/testModelConnectivity";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderTestModelConnectivityParams {
    pub workspace: WorkspaceRef,
    pub selection: ModelSelection,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

/// 结果 schema 是 `{ success: literal true }`：false 不是合法结果，失败走错误帧。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderTestModelConnectivityResult {
    pub success: bool,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

impl ProviderTestModelConnectivityResult {
    pub fn validate(&self) -> Result<(), String> {
        if self.success {
            Ok(())
        } else {
            Err("invalid provider/testModelConnectivity result: success must be true".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_rejects_success_false() {
        let result = ProviderTestModelConnectivityResult {
            success: false,
            extra: Default::default(),
        };
        assert!(result.validate().is_err());
        let ok = ProviderTestModelConnectivityResult {
            success: true,
            extra: Default::default(),
        };
        assert!(ok.validate().is_ok());
    }

    #[test]
    fn params_serialize_camel_case() {
        let params = ProviderTestModelConnectivityParams {
            workspace: WorkspaceRef {
                workspace_path: "D:/ws".into(),
                workspace_key: "D:/ws".into(),
                workspace_identity: None,
                remote_session_id: None,
                extra: Default::default(),
            },
            selection: ModelSelection {
                provider_id: "deepseek".into(),
                model_id: "deepseek-flash".into(),
                options: None,
                extra: Default::default(),
            },
            extra: Default::default(),
        };
        let text = serde_json::to_string(&params).unwrap();
        assert!(text.contains("\"workspacePath\":\"D:/ws\""), "{text}");
        assert!(text.contains("\"providerId\":\"deepseek\""), "{text}");
    }
}
