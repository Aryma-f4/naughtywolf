use serde::{Deserialize, Serialize};

use crate::sliver::connection::SliverConnection;
use crate::sliver::proto::{commonpb, sliverpb};

/// A Sliver extension/module exposed for execution on agents.
#[derive(Debug, Clone, Serialize)]
pub struct ModuleInfo {
    pub name: String,
    /// Whether the extension is loaded server-side (server store) or only on agents.
    pub server_store: bool,
}

/// List extensions currently registered in the Sliver server.
pub async fn list_modules(conn: &mut SliverConnection) -> Result<Vec<ModuleInfo>, String> {
    let resp = conn
        .client
        .list_extensions(tonic::Request::new(sliverpb::ListExtensionsReq {
            request: Some(commonpb::Request {
                r#async: false,
                timeout: 30,
                beacon_id: String::new(),
                session_id: String::new(),
            }),
        }))
        .await
        .map_err(|e| format!("ListExtensions RPC failed: {e}"))?
        .into_inner();

    Ok(resp
        .names
        .into_iter()
        .map(|name| ModuleInfo {
            name,
            server_store: false,
        })
        .collect())
}

/// Arguments for executing a module on an agent session.
#[derive(Debug, Deserialize)]
pub struct ExecModuleRequest {
    /// Optional arguments passed as bytes to the extension.
    pub args: Option<String>,
    /// If set, run server-side and relay result back to session.
    pub server_store: Option<bool>,
    /// Optional export function to call within the extension.
    pub export: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ExecModuleResponse {
    pub success: bool,
    pub module: String,
    /// Raw output bytes from the extension, encoded as a UTF-8 string for JSON.
    pub output: String,
    pub message: String,
}

/// Invoke a registered Sliver extension on a session.
pub async fn exec_module(
    conn: &mut SliverConnection,
    session_id: &str,
    module_name: &str,
    req: ExecModuleRequest,
) -> Result<ExecModuleResponse, String> {
    let args_bytes = req
        .args
        .as_ref()
        .map(|a| a.as_bytes().to_vec())
        .unwrap_or_default();

    let call_req = sliverpb::CallExtensionReq {
        name: module_name.to_string(),
        server_store: req.server_store.unwrap_or(false),
        args: args_bytes,
        export: req.export.unwrap_or_default(),
        request: Some(commonpb::Request {
            r#async: false,
            timeout: 60,
            beacon_id: String::new(),
            session_id: session_id.to_string(),
        }),
    };

    let resp = conn
        .client
        .call_extension(tonic::Request::new(call_req))
        .await
        .map_err(|e| format!("CallExtension RPC failed: {e}"))?
        .into_inner();

    let output = String::from_utf8_lossy(&resp.output).to_string();

    Ok(ExecModuleResponse {
        success: true,
        module: module_name.to_string(),
        output,
        message: format!("Module '{}' executed on session", module_name),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_info_struct() {
        let m = ModuleInfo {
            name: "mimikatz".to_string(),
            server_store: true,
        };
        assert_eq!(m.name, "mimikatz");
        assert!(m.server_store);
    }
}
