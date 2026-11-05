//! Credential presence, shared by the new-session sheet, model changes and doctor.

use crate::{claude, config::Config, profiles};
use std::{collections::BTreeMap, path::Path, time::Duration};

/// Each check's refusal sentence; absent means Razor found the credentials or prerequisite.
/// Claude is checked with the given sign-in command, empty for Claude Code's own, within any deadline.
pub async fn credentials(
    home: &Path,
    sign_in: Option<&[String]>,
    deadline: Option<Duration>,
) -> BTreeMap<&'static str, Option<String>> {
    let mut checks = BTreeMap::new();
    for (name, file, fix) in [
        (
            "ChatGPT sign-in",
            "auth.json",
            crate::responses::SIGN_IN_MISSING.to_owned(),
        ),
        ("OpenAI API key", "key", crate::responses::key_missing(home)),
    ] {
        let found = tokio::fs::try_exists(home.join(file))
            .await
            .unwrap_or(false);
        checks.insert(name, (!found).then_some(fix));
    }
    if let Some(command) = sign_in {
        let check = async {
            let dir = claude::dir()
                .await
                .map_err(|e| format!("{e} Run razor setup."))?;
            let node = claude::node().await;
            let version = tokio::process::Command::new(&node)
                .arg("--version")
                .kill_on_drop(true)
                .output()
                .await;
            if !version.is_ok_and(|o| o.status.success()) {
                let missing = if node == Path::new("node") {
                    "this install has no private Node; "
                } else {
                    ""
                };
                return Err(format!(
                    "The Claude bridge needs Node 22.18 or later on PATH ({}); {missing}install Node, then run razor doctor.",
                    std::env::var("PATH").unwrap_or_default()
                ));
            }
            if !command.is_empty() {
                let signed = claude::shared::env(command, &dir).await;
                return signed.map(drop).map_err(|e| {
                    format!("{e} Check claude.sign_in in your settings, then run razor doctor.")
                });
            }
            let config = std::env::var_os("CLAUDE_CONFIG_DIR")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| {
                    std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                        .join(".claude")
                });
            claude::sign_in(&dir)
                .await
                .map(drop)
                .map_err(|e| format!("Claude sign-in using {}: {e}", config.display()))
        };
        // A check that outlasts the deadline is no evidence of a missing sign-in: on a busy
        // machine the CLI alone can take longer, so the request goes ahead and its turn reports.
        let result = match deadline {
            Some(deadline) => tokio::time::timeout(deadline, check)
                .await
                .unwrap_or(Ok(())),
            None => check.await,
        };
        checks.insert("Claude", result.err());
    }
    checks
}

/// Why `model` cannot run here, checking only its own route's credentials within two seconds;
/// `None` when Razor found them.
pub(super) async fn model(home: &Path, config: &Config, model: &str) -> Option<String> {
    let claude = profiles::model(model).transport == profiles::Transport::Claude;
    let sign_in = (claude && config.claude.sign_in.is_empty()).then_some(&[][..]);
    let checks = credentials(home, sign_in, Some(Duration::from_secs(2))).await;
    unready(&checks, config, model)
}

/// An absent reason means Razor found the credentials;
/// entitlement, quota and network are untried.
pub fn unready(
    checks: &BTreeMap<&str, Option<String>>,
    config: &Config,
    model: &str,
) -> Option<String> {
    let name = match profiles::model(model).transport {
        profiles::Transport::Claude if !config.claude.sign_in.is_empty() => return None,
        profiles::Transport::Claude => "Claude",
        profiles::Transport::Responses => gpt_credential(config),
        profiles::Transport::Chat(row) => {
            let endpoint = match row.endpoint(model, config.models.get(model)) {
                Ok(endpoint) => endpoint,
                Err(e) => return Some(e.to_string()),
            };
            return endpoint
                .key
                .filter(|key| !key.is_empty() && std::env::var_os(key).is_none())
                .map(|key| format!("{key} is not set in the server's environment."));
        }
    };
    checks.get(name).cloned().flatten()
}

/// The credential the configured GPT route uses, for readiness and doctor.
pub fn gpt_credential(config: &Config) -> &'static str {
    let endpoint = profiles::Endpoint {
        url: config.responses.base_url.clone(),
        ..Default::default()
    };
    if endpoint.public() {
        "OpenAI API key"
    } else {
        "ChatGPT sign-in"
    }
}
