//! The live configuration; parsing is bounded and a bad edit never replaces the last good value.

pub(crate) mod manage;
mod mcp;
mod pack;
mod residents;
pub mod setup;
pub(crate) use pack::{Settings, roots};
pub(crate) use residents::{Day, Deliver, Resident, Schedule, Time};

use crate::{
    protocol::{Brand, Effort, Mcp, Phone, SessionId, Speed, Ui},
    tools,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fmt,
    path::{Path, PathBuf},
    sync::LazyLock,
    time::Duration,
};

/// The binary's canonical folder, resolved before the runtime starts; none if unavailable.
pub static INSTALL_DIR: LazyLock<Option<PathBuf>> = LazyLock::new(|| {
    std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .ok()
        .map(|exe| exe.with_file_name(""))
});

/// The file `name` that Razor ships: beside the binary, else in the source tree it was built
/// from; None when neither has it. It looks on the calling thread, so the async loop calls it
/// through `spawn_blocking`.
pub(crate) fn shipped(name: &str) -> Option<PathBuf> {
    let beside = INSTALL_DIR.iter().map(|dir| dir.join(name));
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join(name);
    beside.chain([source]).find(|path| path.is_file())
}

/// Makes Razor's home and any missing parents owner-only (0700), since its records hold whole
/// conversations; a home that already exists keeps its mode. Serve, the clients that start it, and
/// setup call this before writing there, so whichever runs first makes the home.
pub fn make_home(home: &Path) -> Result<(), Error> {
    use std::os::unix::fs::DirBuilderExt;
    let made = std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(home);
    made.map_err(|source| Error::Other {
        source,
        context: "making Razor's home",
    })
}

/// One struct per TOML table; every field has a reader at the next turn or server start.
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub(crate) model: String,
    pub(crate) effort: Effort,
    /// The model and effort a session started for a call runs on, unless the caller names its
    /// own; a call to an existing session keeps that session's (docs/design/call.md).
    pub(crate) call_model: String,
    pub(crate) call_effort: Effort,
    pub(crate) speed: Speed,
    pub(crate) rebuild: Rebuild,
    pub(crate) sessions: Sessions,
    pub server: Server,
    guards: Guards,
    tools: BTreeMap<String, Script>,
    pub(crate) web: tools::web::Config,
    pub(crate) mcp: BTreeMap<String, Mcp>,
    /// `[[plugins]]`, in the order their handlers run (docs/design/extensions.md).
    pub plugins: Vec<Plugin>,
    pub(crate) children: Children,
    pub(crate) stuck: Stuck,
    profiles: Profiles,
    pub(crate) responses: Responses,
    /// `[models."<model>"]`: limits for every model, connection settings for chat routes.
    pub(crate) models: BTreeMap<String, Model>,
    pub claude: Claude,
    /// `[residents.<name>]`: each resident, by name, which is its session's id.
    pub(crate) residents: BTreeMap<SessionId, Resident>,
    /// The program that carries an ask to the person (for example a notifier that waits for a
    /// reply). The question is its last argument; what it prints when it exits 0 is the answer.
    /// Empty: asks wait for a client.
    pub(crate) ask: Vec<String>,
    /// The program that carries a notification (for example a desktop or phone notifier). The
    /// text is its last argument.
    pub(crate) notify: Vec<String>,
    /// The effective pack's directory: the person's choice, or the starter (docs/design/packs.md).
    pack: Option<PathBuf>,
    /// More skill folders, searched after the pack's and the project's; a conversation's opening
    /// list omits their skills; `skill` still loads them by name.
    pub(crate) skills: Vec<PathBuf>,
    /// The system prompt's entries in order: `razor`, `project`, files and folders, absolute once
    /// parsed (docs/design/profiles.md). Only the person's file and the pack may set it.
    startup: Vec<String>,
    pub(crate) brand: Brand,
    pub(crate) ui: Ui,
    pub(crate) phone: Phone,
    /// A program and its arguments that read a reply's text on standard input and write its
    /// MP3 audio to standard output, for `GET /v1/sessions/{id}/speech/{entry}`; empty, none.
    /// The speaker follows the arguments, a resident's name for its sessions, else the session's
    /// id, so a program can give an agent its own voice.
    pub(crate) speech: Vec<String>,
    /// A program and its arguments that read a voice note's audio on standard input and write its
    /// words to standard output, failing with a sentence as standard error's last line; empty,
    /// voice notes are refused.
    pub(crate) transcribe: Vec<String>,
    /// `[live]`: where live calls go and the names their voice will hear.
    pub(crate) live: LiveSettings,
    /// Local argv writing 16 kHz mono s16le PCM on stdout; empty disables recording.
    pub(crate) capture: Vec<String>,
    /// Local player argv: substitute {rate}, then append the audio path.
    pub(crate) play: Vec<String>,
    /// `[push]`: Apple's push service, a machine key; without it nothing is sent.
    pub(crate) push: Option<Push>,
    /// `[tasks]`: set only in a project's own `.razor/config.local.toml`.
    pub(crate) tasks: Tasks,
}

/// One plugin file; handlers run in configuration order. Relative paths start at Razor's home.
#[derive(Clone, PartialEq, Deserialize)]
pub struct Plugin {
    pub(crate) file: PathBuf,
    /// Command hooks handle these events; JavaScript files register their own.
    #[serde(default)]
    pub(crate) before: Vec<Event>,
    /// Command hooks observe entries of these kinds.
    #[serde(default)]
    pub(crate) after: Vec<String>,
}

/// The only events whose handlers may change or refuse pending work.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Event {
    Prompt,
    Input,
    Call,
    Rebuild,
}

/// `[tools.<name>]`: a script plus a schema. Each call runs `command` with bash, as a job in the
/// session's `cwd`, with the call's input as JSON on its standard input; the script checks that
/// input itself, as an MCP server does. `input`'s root must be `{"type": "object"}`, or tools
/// leaves the tool out.
#[derive(Clone, PartialEq, Deserialize)]
pub(crate) struct Script {
    pub(crate) command: String,
    pub(crate) description: String,
    pub(crate) input: serde_json::Value,
    #[serde(default)]
    pub(crate) read_only: bool,
}

/// How a project's tasks are kept.
#[derive(Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Tasks {
    /// A remote of the project's checkout; its tasks are shared through `refs/razor/tasks` there.
    /// Absent, they are plain files in Razor's home.
    pub(crate) remote: Option<String>,
}

/// A machine key: calls go to OpenAI, or to `url`, which speaks GPT-Live's protocol (a stand-in, or
/// the person's own relay); either gets the home's `key` file when it has one.
#[derive(Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct LiveSettings {
    pub(crate) url: Option<String>,
    pub(crate) names: Vec<String>,
}
#[derive(Clone, Deserialize)]
#[serde(default)]
pub(crate) struct Rebuild {
    pub(crate) threshold: f64,
}
#[derive(Clone, Deserialize)]
#[serde(default)]
pub(crate) struct Sessions {
    pub(crate) idle: usize,
    pub(crate) handover_minutes: u64,
    pub(crate) handover_tokens: u64,
    /// Zero leaves conversations open until the person closes them.
    pub(crate) close_minutes: u64,
}
#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub struct Server {
    pub address: Option<String>,
    pub(crate) https_url: Option<String>,
    /// The machine's shorthand the other servers pair this one under (`desk`, `laptop`): the
    /// first word of every root's name, read at each root's start. Empty: its project's.
    pub(crate) name: String,
    /// How a phone names this home. Empty: `name`, else the machine's name.
    pub(crate) label: String,
    pub(crate) realm: String,
    /// Stable server.id keeping shared settings; empty means this home, never a peer name.
    pub(crate) shared: String,
    /// A command `razor install`'s service runs the server through, the server's own command
    /// as its last arguments; read only by `razor install`.
    pub(crate) wrapper: Vec<String>,
}
/// The Apple developer team and the app's bundle id, and a token signing key for each
/// environment this server sends to; nothing goes to a device whose environment has none.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Push {
    pub(crate) team: String,
    pub(crate) bundle: String,
    pub(crate) development: Option<PushKey>,
    pub(crate) production: Option<PushKey>,
    /// Where pushes go instead of Apple's host: the flows' stand-in.
    pub(crate) endpoint: Option<String>,
}
/// A key's id and the path of its `.p8` file outside any repository; its bytes go nowhere but
/// the signature.
#[derive(Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PushKey {
    pub(crate) key_id: String,
    pub(crate) key: PathBuf,
}
#[derive(Clone, Deserialize)]
#[serde(default)]
struct Guards {
    delete: bool,
}
#[derive(Clone, Deserialize)]
#[serde(default)]
pub(crate) struct Children {
    /// Startup instructions for every child, independent of its parent's list.
    pub(crate) startup: Vec<String>,
    pub(crate) max: usize,
    #[serde(deserialize_with = "duration")]
    pub(crate) grace: Duration,
    /// Files whose text replaces Razor's reviewer or researcher text, read at each start.
    pub(crate) reviewer: Option<PathBuf>,
    pub(crate) researcher: Option<PathBuf>,
    /// The model a video's watcher runs on; one that cannot see video starts none
    /// (docs/design/video.md).
    pub(crate) watcher: String,
}
#[derive(Clone, Deserialize)]
#[serde(default)]
pub(crate) struct Stuck {
    #[serde(deserialize_with = "duration")]
    pub(crate) after: Duration,
}

#[derive(Clone, Default, Deserialize)]
#[serde(default)]
struct Profile {
    pub(crate) code_mode: Option<bool>,
    /// Codex's image generation, which only GPT on the sign-in has.
    image_gen: bool,
}
#[derive(Clone, Default, Deserialize)]
#[serde(default)]
struct Profiles {
    claude: Profile,
    codex: Profile,
}
/// One model's settings, each replacing its shipped row's (docs/design/generic.md); a
/// machine key, refused in a project's file. `key` names a variable and never holds a key.
#[derive(Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Model {
    pub(crate) base_url: Option<String>,
    pub(crate) key: Option<String>,
    pub(crate) id: Option<String>,
    pub(crate) speed_ids: BTreeMap<Speed, String>,
    /// Context tokens; when set, the session asks for a handover at its rebuild threshold.
    pub(crate) window: Option<u64>,
    pub(crate) threshold: Option<f64>,
    pub(crate) replay: Option<Replay>,
    /// Top-level fields added to every request, such as a host's switch for keeping history.
    pub(crate) request: Option<serde_json::Map<String, serde_json::Value>>,
    /// The variant file's declarations, read with the settings file that names it.
    pub(crate) variant: Option<serde_json::Value>,
}
/// How a chat message's reasoning goes back: as it streamed, moved to one of the two fields
/// hosts read, or not at all.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Replay {
    #[default]
    Streamed,
    Reasoning,
    ReasoningContent,
    None,
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub(crate) struct Responses {
    pub(crate) base_url: String,
}
/// `[claude]`, a machine key: a project's file cannot set it.
#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub struct Claude {
    /// A command printing a Claude sign-in token, run for each conversation Razor opens on
    /// Claude; empty: the server's own sign-in (docs/design/claude.md).
    pub sign_in: Vec<String>,
    /// A JSON file another program keeps with each account's plan usage, read for the model
    /// rows of the account `sign_in` names (docs/design/claude.md).
    pub(crate) usage_file: Option<PathBuf>,
}
impl Default for Responses {
    fn default() -> Self {
        Self {
            base_url: "https://chatgpt.com/backend-api/codex".into(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            model: "opus".into(),
            effort: Effort::High,
            call_model: "terra".into(),
            call_effort: Effort::Low,
            speed: Speed::Standard,
            rebuild: Rebuild::default(),
            sessions: Sessions::default(),
            server: Server::default(),
            guards: Guards::default(),
            tools: BTreeMap::new(),
            web: Default::default(),
            mcp: BTreeMap::new(),
            plugins: Vec::new(),
            children: Children::default(),
            stuck: Stuck::default(),
            profiles: Profiles::default(),
            responses: Responses::default(),
            models: BTreeMap::new(),
            claude: Claude::default(),
            residents: BTreeMap::new(),
            ask: Vec::new(),
            notify: Vec::new(),
            pack: None,
            skills: Vec::new(),
            startup: vec!["razor".into(), "project".into()],
            brand: Default::default(),
            ui: Default::default(),
            phone: Default::default(),
            speech: Vec::new(),
            transcribe: Vec::new(),
            live: LiveSettings::default(),
            capture: Vec::new(),
            // Only macOS ships a player, `afplay`; elsewhere none is assumed.
            #[cfg(target_os = "macos")]
            play: vec!["afplay".into(), "-r".into(), "{rate}".into()],
            #[cfg(not(target_os = "macos"))]
            play: Vec::new(),
            push: None,
            tasks: Tasks::default(),
        }
    }
}
impl Default for Rebuild {
    fn default() -> Self {
        Self { threshold: 0.8 }
    }
}
impl Default for Sessions {
    fn default() -> Self {
        Self {
            idle: 4,
            handover_minutes: 50,
            handover_tokens: 50_000,
            close_minutes: 0,
        }
    }
}
impl Default for Guards {
    fn default() -> Self {
        Self { delete: true }
    }
}
impl Default for Children {
    fn default() -> Self {
        Self {
            startup: vec!["project".into()],
            max: 20,
            grace: Duration::from_secs(180),
            reviewer: None,
            researcher: None,
            watcher: "gemini-3.8-flash".into(),
        }
    }
}
impl Default for Stuck {
    fn default() -> Self {
        Self {
            after: Duration::from_secs(900),
        }
    }
}

fn duration<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
    let text = String::deserialize(d)?;
    let text = text.trim();
    let unit = text.trim_start_matches(|c: char| c.is_ascii_digit());
    let n = &text[..text.len() - unit.len()];
    let n: u64 = n.parse().map_err(serde::de::Error::custom)?;
    let factor = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        _ => {
            return Err(serde::de::Error::custom(
                "a duration is a number followed by s, m, h or d",
            ));
        }
    };
    n.checked_mul(factor)
        .map(Duration::from_secs)
        .ok_or_else(|| serde::de::Error::custom("the duration is too large"))
}

impl Config {
    /// Loads machine settings once; live readers retain a Settings handle for per-file fallback.
    pub async fn load(home: &Path) -> Result<Self, Error> {
        Ok(Settings::open(home.to_path_buf())?.config)
    }

    fn parse(table: toml::Table) -> Result<Self, Error> {
        let mut config: Self = table.try_into().map_err(|e: toml::de::Error| {
            let why = e.message().trim_end().replace('\n', " ");
            Error::Invalid(format!("The config is invalid: {why}."))
        })?;
        config.model = crate::profiles::id(std::mem::take(&mut config.model));
        if !config.rebuild.threshold.is_finite()
            || !(0.0 < config.rebuild.threshold && config.rebuild.threshold <= 1.0)
        {
            return Err(Error::Invalid(
                "rebuild.threshold must be greater than zero and at most one.".into(),
            ));
        }
        for list in [&config.web.search, &config.web.fetch, &config.web.answer] {
            if list.iter().enumerate().any(|(i, p)| list[..i].contains(p)) {
                return Err(Error::Invalid(
                    "Each [web] list must name each provider at most once; remove duplicates."
                        .into(),
                ));
            }
        }
        let count = config.models.len();
        config.models = std::mem::take(&mut config.models)
            .into_iter()
            .map(|(name, row)| (crate::profiles::id(name), row))
            .collect();
        if config.models.len() != count {
            return Err(Error::Invalid(
                "Two model rows name the same model; keep one row.".into(),
            ));
        }
        for (name, row) in &config.models {
            crate::profiles::model(name)
                .validate(name, row)
                .map_err(|e| Error::Invalid(e.to_string()))?;
        }
        if config.web.answer.contains(&tools::web::Provider::Firecrawl) {
            return Err(Error::Invalid(
                "Firecrawl has no answer action; use exa or parallel in web.answer.".into(),
            ));
        }
        Ok(config)
    }

    /// The selected family and code-mode setting bind one snapshot of available actions, as a
    /// child's when `child` is set.
    pub(crate) fn profile(
        &self,
        endpoint: &str,
        model: &str,
        actions: std::sync::Arc<[tools::Action]>,
        child: bool,
    ) -> Result<crate::profiles::Profile, crate::profiles::Error> {
        use crate::profiles::{self, Endpoint, Transport};
        let route = profiles::model(model).transport;
        let chat = self.models.get(model);
        let own = || Endpoint {
            url: endpoint.into(),
            id: model.into(),
            ..Default::default()
        };
        let (mode, endpoint) = match route {
            Transport::Claude => (self.profiles.claude.code_mode.unwrap_or(false), own()),
            Transport::Responses => (
                self.profiles.codex.code_mode.unwrap_or(!own().public()),
                own(),
            ),
            Transport::Chat(row) => (false, row.endpoint(model, chat)?),
        };
        let variant = chat.and_then(|c| c.variant.as_ref());
        let image = self.profiles.codex.image_gen;
        profiles::load(endpoint, model, route, mode, variant, actions, child, image)
    }

    /// The startup list `session` opens with: its own when it is a resident that names one.
    pub(crate) fn startup_for(&self, session: &crate::protocol::SessionId) -> &[String] {
        let resident = self.residents.get(session);
        resident
            .and_then(|r| r.startup.as_deref())
            .unwrap_or(&self.startup)
    }
}

impl Settings {
    /// The skill directories, in the `skill` tool's lookup order: the pack's `skills`, the
    /// project's `.agents/skills` and `.claude/skills` (Codex's and Claude Code's), then the
    /// `skills` key's folders, so a project's skill never replaces the pack's.
    pub(crate) fn skills(&self) -> Vec<PathBuf> {
        let pack = self.config.pack.iter().map(|pack| pack.join("skills"));
        let project = (self.project.iter())
            .flat_map(|root| [".agents", ".claude"].map(|dir| root.join(dir).join("skills")));
        let more = self.config.skills.iter().cloned();
        pack.chain(project).chain(more).collect()
    }

    /// Freezes session actions and their local MCP root from this settings snapshot; the caller
    /// adds the plugins' tools.
    pub(crate) fn tools(&self) -> tools::Config {
        let skills = self.skills();
        tools::Config {
            delete_guard: self.config.guards.delete,
            described: skills.len() - self.config.skills.len(),
            skills,
            scripts: self.config.tools.clone(),
            mcp: self.config.mcp.clone(),
            mcp_cwd: self.project.clone(),
            web: self.config.web.clone(),
            plugins: BTreeMap::new(),
        }
    }
}

/// Invalid input is actionable; machine failures keep their source and context.
#[derive(Debug)]
pub enum Error {
    Invalid(String),
    Other {
        source: std::io::Error,
        context: &'static str,
    },
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(text) => f.write_str(text),
            Self::Other { source, context } => write!(f, "Razor failed {context}: {source}."),
        }
    }
}
impl std::error::Error for Error {}
