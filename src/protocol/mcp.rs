//! The `[mcp.<name>]` table: one MCP server's definition, as settings files and the API carry it.

use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    num::NonZeroU64,
    path::PathBuf,
};

/// A whole named server: exactly one command or HTTP URL, which only a disabled entry may omit.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Definition")]
pub(crate) struct Mcp {
    pub(crate) enabled: bool,
    pub(crate) search: Option<bool>,
    pub(crate) timeout: Option<NonZeroU64>,
    pub(crate) tools: Option<BTreeSet<String>>,
    #[serde(flatten)]
    pub(crate) connection: Option<Connection>,
}

#[derive(Deserialize)]
struct Definition {
    #[serde(default = "enabled")]
    enabled: bool,
    search: Option<bool>,
    timeout: Option<NonZeroU64>,
    tools: Option<BTreeSet<String>>,
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    url: Option<String>,
    #[serde(default)]
    headers: BTreeMap<String, Reference>,
    oauth: Option<OAuth>,
}

fn enabled() -> bool {
    true
}

impl TryFrom<Definition> for Mcp {
    type Error = &'static str;
    fn try_from(d: Definition) -> Result<Self, Self::Error> {
        let web = |url: &str| {
            let url = reqwest::Url::parse(url);
            url.is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some())
        };
        let connection = match (d.command, d.url) {
            (Some(command), None) if !command.trim().is_empty() => {
                let (args, env) = (d.args, d.env);
                Some(Connection::Stdio { command, args, env })
            }
            (None, Some(url)) if web(&url) => {
                let (headers, oauth) = (d.headers, d.oauth);
                Some(Connection::Http {
                    url,
                    headers,
                    oauth,
                })
            }
            (None, None) if !d.enabled => None,
            _ => {
                return Err("An MCP server needs exactly one command or HTTP URL unless disabled.");
            }
        };
        if let Some(Connection::Http { oauth: Some(o), .. }) = &connection
            && o.client_secret.is_some()
            && (o.client_id.is_none() || o.issuer.is_none())
        {
            return Err("An MCP client secret needs its client ID and expected issuer.");
        }
        let (enabled, search, tools) = (d.enabled, d.search, d.tools);
        Ok(Self {
            enabled,
            search,
            timeout: d.timeout,
            tools,
            connection,
        })
    }
}

/// Stdio keeps literal environment values; HTTP credentials are references resolved at use.
#[derive(Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub(crate) enum Connection {
    Stdio {
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
    },
    Http {
        url: String,
        headers: BTreeMap<String, Reference>,
        oauth: Option<OAuth>,
    },
}

/// A credential's location, never its resolved bytes in configuration or records.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Reference {
    Env(String),
    File(PathBuf),
}

/// Expected authorization server and an optional preregistered client.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct OAuth {
    pub(crate) issuer: Option<String>,
    pub(crate) client_id: Option<String>,
    pub(crate) client_secret: Option<Reference>,
}
