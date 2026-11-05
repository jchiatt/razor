//! MCP management: one dispatch for every client.

use super::{Error, Settings};
use crate::protocol::{McpOperation, McpRow, McpScope};
use crate::tools::{Auth, Tools, idle};
use serde_json::{Value, json};

/// Lists, signs in or out, reconnects, edits or previews an import; never resolves a credential.
pub(crate) async fn run(
    settings: &Settings,
    auth: &Auth,
    tools: Option<&Tools>,
    operation: &McpOperation,
) -> Result<Value, Error> {
    use McpOperation::*;
    let refused = |e: crate::tools::Error| Error::Invalid(e.to_string());
    let (name, config) = match operation {
        List => {
            let mut rows = Vec::new();
            for (name, config) in &settings.config.mcp {
                let (state, tools, error) = tools.map_or_else(
                    || (idle(config.enabled).into(), Vec::new(), None),
                    |tools| tools.mcp_state(name),
                );
                let file = settings.sources.get(&format!("mcp.{name}")).cloned();
                let scope = [McpScope::Global, McpScope::Project, McpScope::Private]
                    .into_iter()
                    .find(|s| file.is_some() && settings.destination(*s).ok().map(|d| d.0) == file);
                let signed_in = auth.signed_in(name, config).await;
                rows.push(McpRow {
                    name: name.clone(),
                    file,
                    scope,
                    config: config.clone(),
                    state,
                    tools,
                    signed_in,
                    error,
                });
            }
            return Ok(json!(rows));
        }
        Import { source, scope } => return super::mcp::preview(settings, source, *scope),
        SignIn { name } | SignOut { name } | Reconnect { name } => {
            (name, settings.config.mcp.get(name))
        }
        _ => return super::mcp::edit(settings, operation),
    };
    let config = config
        .ok_or_else(|| Error::Invalid(format!("There is no MCP server `{name}`; add it first.")))?;
    match operation {
        SignIn { .. } => {
            let https = settings.config.server.https_url.as_deref();
            let url = auth.sign_in(name, config, https).await.map_err(refused)?;
            return Ok(json!({"url": url}));
        }
        SignOut { .. } => auth.sign_out(name, config).await.map_err(refused)?,
        _ => {}
    }
    auth.reconnect(Some(name));
    Ok(json!({}))
}
