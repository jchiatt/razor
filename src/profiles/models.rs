//! Model names, native routes and configured chat endpoints.

use super::{Endpoint, Error, Replay};
use crate::config::Config;
use crate::protocol::{Effort, Speed, SpeedChoice};

/// The wire a model is reached over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Transport {
    Claude,
    Responses,
    /// OpenAI-compatible `POST {base_url}/chat/completions`, streamed (docs/design/generic.md).
    Chat(&'static Chat),
}

/// A chat model's shipped row; `[models."<model>"]` in config replaces any field but `top`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Chat {
    base_url: &'static str,
    /// The environment variable holding the host's key: read for each request, sent only as
    /// the bearer, never written to the record, a log or an error.
    key: &'static str,
    /// The host's id for the model.
    id: &'static str,
    /// The highest effort the model takes; a higher one is sent as this.
    pub(super) top: Effort,
    /// Which field earlier reasoning goes back under, as the row's probe proved it.
    replay: Replay,
    /// The variant the model gets when config names none: its leader in the evaluation.
    pub(super) variant: &'static str,
}

impl Chat {
    /// Resolves and validates a configured host against this row before it can send a request.
    pub(crate) fn endpoint(
        &self,
        name: &str,
        config: Option<&crate::config::Model>,
    ) -> Result<Endpoint, Error> {
        let defaults = crate::config::Model::default();
        let config = config.unwrap_or(&defaults);
        let url = config.base_url.as_deref().unwrap_or(self.base_url);
        if url.is_empty() {
            let names = CLAUDE
                .iter()
                .copied()
                .chain(SHORT.iter().map(|(name, _)| *name))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(Error::Invalid(format!(
                "{name} is neither a model Razor ships nor a configured chat model; check the name ({names}, or a full id from the model list), or add [models.\"{name}\"] with its base_url"
            )));
        }
        if !reqwest::Url::parse(url).is_ok_and(|u| matches!(u.scheme(), "http" | "https")) {
            return Err(Error::Invalid(format!(
                "[models.\"{name}\"] base_url must be an HTTP or HTTPS URL"
            )));
        }
        Ok(Endpoint {
            url: url.into(),
            id: config
                .id
                .as_deref()
                .unwrap_or(if self.id.is_empty() { name } else { self.id })
                .into(),
            speed_ids: config.speed_ids.clone(),
            key: Some(config.key.as_deref().unwrap_or(self.key).into()),
            replay: config.replay.unwrap_or(self.replay),
            request: config.request.clone().unwrap_or_default(),
        })
    }
}

/// A model's route and context window; the SDK reports Claude's window itself.
pub(crate) struct Model {
    pub(crate) transport: Transport,
    pub(super) window: Option<u64>,
}

impl Model {
    /// Offered efforts in ascending order, capped by the shipped top on chat routes.
    pub(crate) fn efforts(&self) -> &'static [Effort] {
        let top = match self.transport {
            Transport::Chat(chat) => chat.top,
            _ => Effort::Max,
        };
        &Effort::ALL[..Effort::ALL.partition_point(|effort| *effort <= top)]
    }

    /// Refuses invalid limits and connection settings that cannot apply to this route.
    pub(crate) fn validate(&self, name: &str, row: &crate::config::Model) -> Result<(), Error> {
        if row.window == Some(0)
            || row
                .threshold
                .is_some_and(|t| !t.is_finite() || !(0.0 < t && t <= 1.0))
        {
            return Err(Error::Invalid(format!(
                "[models.\"{name}\"] needs a positive window and a threshold greater than zero and at most one"
            )));
        }
        if let Transport::Chat(chat) = self.transport {
            return chat.endpoint(name, Some(row)).map(|_| ());
        }
        if row.base_url.is_some()
            || row.key.is_some()
            || row.id.is_some()
            || row.request.is_some()
            || row.variant.is_some()
            || !row.speed_ids.is_empty()
            || row.replay.is_some()
        {
            return Err(Error::Invalid(format!(
                "[models.\"{name}\"] names a Claude or GPT model; remove base_url, key, id, request, variant, replay and speed_ids"
            )));
        }
        Ok(())
    }
}

/// Resolves Razor's limits from live settings, then the catalog, then the SDK's report;
/// a configured Claude window never changes what its SDK sends.
pub(crate) fn limits(name: &str, config: &Config, reported: Option<u64>) -> (Option<u64>, f64) {
    let row = config.models.get(name);
    let window = row
        .and_then(|r| r.window)
        .or(model(name).window)
        .or(reported.filter(|w| *w > 0));
    let threshold = row
        .and_then(|r| r.threshold)
        .unwrap_or(config.rebuild.threshold);
    (window, threshold)
}

const MODELS: &[(&str, u64)] = &[
    ("gpt-6-astra", 272_000),
    ("gpt-6.1-sol", 272_000),
    ("gpt-6-sol", 272_000),
    ("gpt-6-luna", 272_000),
    ("gpt-5.6-sol", 272_000),
    ("gpt-5.6-terra", 272_000),
    ("gpt-5.6-luna", 272_000),
    ("gpt-daybreak-blue-latest", 272_000),
    ("codex-auto-review", 272_000),
    ("gpt-6.1", 272_000),
    ("gpt-6.1-terra", 500_000),
    ("gpt-daybreak-red-latest", 372_000),
];
const CLAUDE: &[&str] = &["opus", "sonnet", "haiku", "fable"];
/// The short GPT names, each for one id in `MODELS`.
const SHORT: &[(&str, &str)] = &[
    ("sol", "gpt-6.1-sol"),
    ("terra", "gpt-6.1-terra"),
    ("luna", "gpt-6-luna"),
    ("astra", "gpt-6-astra"),
];
const RUNINFRA: &str = "https://api.runinfra.ai/v1";
const CHAT: &[(&str, u64, Chat)] = &[
    (
        "gemini-3.8-flash",
        1_048_576,
        Chat {
            base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
            key: "GEMINI_API_KEY",
            id: "gemini-3.8-flash",
            top: Effort::High,
            replay: Replay::Streamed,
            variant: include_str!("data/variants/gemini-lean.json"),
        },
    ),
    (
        "deepseek-v4.1-flash-1017",
        1_048_576,
        Chat {
            base_url: RUNINFRA,
            key: "RUNINFRA_API_KEY",
            id: "deepseek-v4-1-flash-1017",
            top: Effort::Max,
            replay: Replay::Streamed,
            variant: include_str!("data/variants/bash-coord.json"),
        },
    ),
    (
        "glm-5.3-flash",
        1_048_576,
        Chat {
            base_url: RUNINFRA,
            key: "RUNINFRA_API_KEY",
            id: "glm-5-3-flash",
            top: Effort::Max,
            replay: Replay::Streamed,
            variant: include_str!("data/variants/bash-coord.json"),
        },
    ),
];

/// Its empty address, key and id are filled from `[models.NAME]`: no default host or auth, id = name.
const CONFIGURED: Chat = Chat {
    base_url: "",
    key: "",
    id: "",
    top: Effort::Max,
    replay: Replay::Streamed,
    variant: "{}",
};

/// The id a model name stands for: a short GPT name's full id, any other name unchanged. Each
/// place a name enters Razor calls it once, so records, status and requests hold full ids.
pub(crate) fn id(name: String) -> String {
    SHORT
        .iter()
        .find(|(short, _)| *short == name)
        .map_or(name, |(_, id)| (*id).to_owned())
}

/// Native names use their own wire; other names require a configured chat endpoint.
pub(crate) fn model(name: &str) -> Model {
    if CLAUDE.contains(&name) || name.starts_with("claude-") {
        return Model {
            transport: Transport::Claude,
            window: None,
        };
    }
    if let Some((_, window, chat)) = CHAT.iter().find(|(n, ..)| *n == name) {
        return Model {
            transport: Transport::Chat(chat),
            window: Some(*window),
        };
    }
    let window = MODELS.iter().find(|(n, _)| *n == name).map(|(_, w)| *w);
    Model {
        transport: match window {
            Some(_) => Transport::Responses,
            None => Transport::Chat(&CONFIGURED),
        },
        window,
    }
}

/// The chat endpoint `name` is reached at under `config`, when it takes video; `None` for
/// Claude, GPT and every other chat model.
pub(crate) fn video(name: &str, config: &Config) -> Option<Endpoint> {
    let Transport::Chat(row) = model(name).transport else {
        return None;
    };
    row.endpoint(name, config.models.get(name))
        .ok()
        .filter(Endpoint::video)
}

/// The shipped model names; the server adds configured chat names to this list.
pub(crate) fn models() -> Vec<String> {
    MODELS
        .iter()
        .map(|(name, _)| (*name).to_owned())
        .chain(CHAT.iter().map(|(name, ..)| (*name).to_owned()))
        .chain(CLAUDE.iter().map(|name| (*name).to_owned()))
        .collect()
}

/// The route's choice, with known restrictions and descriptive costs; no account probe.
pub(crate) fn speed_choice(name: &str, speed: Speed, config: &Config) -> SpeedChoice {
    use std::{collections::BTreeMap, sync::LazyLock};
    static TEXT: LazyLock<BTreeMap<String, String>> = LazyLock::new(|| {
        // The shipped speed catalogue is valid TOML matching the pricing table.
        toml::from_str(include_str!("data/speeds.toml")).expect("shipped speed catalogue")
    });
    let configured = config
        .models
        .get(name)
        .is_some_and(|r| r.speed_ids.contains_key(&speed));
    let accepts = |key: &str| TEXT[key].split(',').any(|m| m == name);
    let key = match (speed, model(name).transport) {
        (Speed::Standard, _) => "standard",
        (Speed::Fast, Transport::Responses) => "gpt_fast",
        (Speed::Ultrafast, Transport::Responses) if accepts("ultrafast_models") => "gpt_ultrafast",
        (Speed::Fast, Transport::Claude) if accepts("claude_models") => "claude",
        (Speed::Fast, Transport::Chat(_)) if name.starts_with("gemini-") => "gemini",
        (_, Transport::Chat(_)) if configured => "configured",
        _ => "unavailable",
    };
    SpeedChoice {
        speed,
        reason: (key == "unavailable").then(|| TEXT[key].clone()),
        hint: TEXT[key].clone(),
    }
}
/// Rejects unavailable or conflicting speed choices before any settings are recorded.
pub(crate) fn speed(name: &str, speed: Speed, config: &Config) -> Result<(), Error> {
    if let Some(reason) = speed_choice(name, speed, config).reason {
        return Err(Error::Invalid(reason));
    }
    if name.starts_with("gemini-")
        && speed != Speed::Standard
        && config
            .models
            .get(name)
            .and_then(|r| r.request.as_ref())
            .and_then(|r| r.get("service_tier"))
            .is_some_and(|v| v != "priority")
    {
        return Err(Error::Invalid(
            "request.service_tier conflicts with Fast; remove it or choose Standard.".into(),
        ));
    }
    Ok(())
}
