//! `razor`, the binary: `razor serve` runs the API server; `--print`, `razor login` and
//! `razor devices` are its clients, through its socket (docs/design/claude-slice.md and
//! docs/design/api.md say exactly what they read and write); `razor plugins` needs no server, and
//! `razor setup` runs the others in order for a first run (docs/design/setup.md).

use razor_harness::api;
use razor_harness::config::{INSTALL_DIR, make_home, setup};
use razor_harness::controller::Controller;
use razor_harness::extensions;
use razor_harness::protocol::{Body, Effort, End, EntryId, Event, SessionId, Speed};
use razor_harness::residents::{self, Residents};
use serde_json::{Value, json};
use std::os::unix::{fs::OpenOptionsExt, process::CommandExt};
use std::path::{Path, PathBuf};
use std::process::{ExitCode, Stdio};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

/// Why `razor` stopped, as one sentence for the person: a usage error exits 2, a failure 1.
enum Error {
    Usage(String),
    Failed(String),
}
use Error::{Failed, Usage};

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Text,
    Json,
    Stream,
}

#[derive(Default)]
struct Args {
    model: Option<String>,
    effort: Option<Effort>,
    speed: Option<Speed>,
    resume: Option<SessionId>,
    continue_newest: bool,
    agents: bool,
    print: Option<Mode>,
    message: Option<String>,
}

fn mode(word: &str) -> Option<Mode> {
    match word {
        "text" => Some(Mode::Text),
        "json" => Some(Mode::Json),
        "stream" => Some(Mode::Stream),
        _ => None,
    }
}

fn parse(mut words: std::vec::IntoIter<String>) -> Result<Args, Error> {
    let mut args = Args::default();
    let printing = words
        .as_slice()
        .iter()
        .any(|word| word == "--print" || word.starts_with("--print="));
    while let Some(word) = words.next() {
        let mut value = |flag: &str| {
            words
                .next()
                .ok_or_else(|| Usage(format!("{flag} needs a value.")))
        };
        match word.as_str() {
            "--model" => args.model = Some(value("--model")?),
            "--speed" => {
                args.speed = Some(
                    serde_json::from_value(json!(value("--speed")?))
                        .map_err(|_| Usage("--speed is standard, fast or ultrafast.".into()))?,
                )
            }
            "--effort" => {
                let effort = value("--effort")?;
                args.effort = Some(
                    serde_json::from_value(Value::String(effort.clone())).map_err(|_| {
                        Usage(format!(
                            "--effort is low, medium, high, xhigh or max, not {effort}."
                        ))
                    })?,
                );
            }
            "--resume" => {
                let id = SessionId::try_from(value("--resume")?);
                args.resume = Some(id.map_err(|e| Usage(e.to_string()))?);
            }
            "--continue" => args.continue_newest = true,
            "--agents" => args.agents = true,
            "--print" => {
                args.print = Some(Mode::Text);
                let mut ahead = words.clone();
                if let Some(m) = ahead.next().as_deref().and_then(mode) {
                    args.print = Some(m);
                    words = ahead;
                }
            }
            w if w.starts_with("--print=") => {
                let m = mode(&w[8..]);
                args.print = Some(m.ok_or_else(|| {
                    Usage(format!("--print is text, json or stream, not {}.", &w[8..]))
                })?)
            }
            w if w.starts_with("--") => return Err(Usage(format!("razor has no flag {w}."))),
            _ if args.message.is_none() && !printing && !word.chars().any(char::is_whitespace) => {
                return Err(Usage(unknown(&word)));
            }
            _ if args.message.is_some() => {
                return Err(Usage("Give the message as one argument, in quotes.".into()));
            }
            _ => args.message = Some(word),
        }
    }
    if args.resume.is_some() && args.continue_newest {
        return Err(Usage(
            "--resume and --continue cannot be used together.".into(),
        ));
    }
    Ok(args)
}

/// What `razor help`, `--help` and `-h` print, before anything is read.
const USAGE: &str = "\
Usage: razor [options] [\"message\"]  open the terminal app, with a first message if given
       razor --print[=text|json|stream] [options] [\"message\"]  answer once; else read stdin
       razor <command>
Options:
  --model <name>             choose a model (for example opus, sonnet or sol)
  --effort <low|medium|high|xhigh|max>  choose reasoning effort
  --speed <standard|fast|ultrafast>    choose service speed
  --resume <id>              reopen a session
  --continue                 reopen this folder's newest session
  --agents                   open the agents list beside the newest session
  --print[=text|json|stream]  answer once; read stdin when no message is given
  --version                  print the installed version
  --help, -h                 show this help
Print modes: text prints the final reply; json prints the session id and final message;
stream prints each event as one JSON line.
Exit codes: 0 success; 1 failed; 2 invalid usage.
Environment: RAZOR_HOME selects the home (default ~/.razor).
Docs: ~/.local/share/razor/current/docs/README.md in a standard install, or https://github.com/treygoff24/razor/tree/main/docs

";

const COMMANDS: &[&str] = &[
    "help                       show this help",
    "setup [--console]          choose providers, offer a service and sign in\n  --console bills Claude through Anthropic Console",
    "keys                       save optional web-search and voice keys privately",
    "doctor                     report prerequisites and commands to fix problems",
    "login                      sign in to ChatGPT",
    "install                    run the server in the background at login",
    "serve                      run the server in this terminal",
    "stop                       stop this home's server",
    "restart                    stop and start this home's server",
    "devices                    list paired devices\ndevices add <name> [--address <host:port>] [--key <key>]  pair a device\ndevices remove <name>      remove a paired device",
    "mcp list                   list MCP servers\nmcp add <name> --scope <global|project|private> --config '<JSON>'\nmcp remove|enable|disable <name> --scope <global|project|private>\nmcp reconnect|sign-in|sign-out <name>  reconnect or change sign-in\nmcp import <file> --scope <global|project|private>  preview definitions; apply with /mcp import in the app\nmcp complete '<redirect address>'  finish a browser sign-in\n  --scope defaults to private",
    "plugins                    list configured plugins",
];

/// Print a command's syntax, or all help; unknown commands return false without side effects.
fn help(command: Option<&str>) -> bool {
    if let Some(command) = command {
        let Some(text) = COMMANDS
            .iter()
            .find(|text| text.split_whitespace().next() == Some(command))
        else {
            return false;
        };
        println!("Usage: razor {text}\nRun razor --help for launch options and all commands.");
    } else {
        println!("{USAGE}Commands:");
        for text in COMMANDS {
            println!("  {}", text.replace('\n', "\n  "))
        }
    }
    true
}

fn unknown(command: &str) -> String {
    let pointer = match command {
        "task" | "memory" | "agent" | "message" => format!(
            "{command} is a tool in your Razor session; there is no razor {command} command. "
        ),
        _ => format!("Razor has no command {command}. "),
    };
    let commands = COMMANDS
        .iter()
        .filter_map(|text| text.split_whitespace().next())
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{pointer}Commands: {commands}. To send a one-word message, open razor and type it, or use razor --print {command}."
    )
}

fn main() -> ExitCode {
    std::sync::LazyLock::force(&INSTALL_DIR);
    let words: Vec<String> = std::env::args().skip(1).collect();
    let first = words.first().map(String::as_str);
    if first == Some("--version") {
        if words.len() != 1 {
            eprintln!("Run razor --version without arguments. Run razor --help.");
            return ExitCode::from(2);
        }
        println!("razor {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    if let Some("help" | "--help" | "-h") = first {
        if words.len() == 1 && help(None)
            || first == Some("help")
                && words.len() == 2
                && help(Some(if matches!(words[1].as_str(), "--help" | "-h") {
                    "help"
                } else {
                    &words[1]
                }))
        {
            return ExitCode::SUCCESS;
        }
        eprintln!(
            "{} Run razor --help.",
            unknown(words.get(1).map_or("help", String::as_str))
        );
        return ExitCode::from(2);
    }
    if words
        .iter()
        .skip(1)
        .any(|w| matches!(w.as_str(), "--help" | "-h"))
    {
        if help(
            first.filter(|word| !word.starts_with('-') && !word.chars().any(char::is_whitespace)),
        ) {
            return ExitCode::SUCCESS;
        }
        eprintln!("{} Run razor --help.", unknown(first.unwrap_or_default()));
        return ExitCode::from(2);
    }
    let home = std::env::var_os("RAZOR_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".razor")));
    // Before any thread starts, serve and doctor fill absent key variables from keys.toml.
    if let (Some("serve" | "doctor"), Some(home)) = (first, &home)
        && let Err(e) = setup::load(home)
    {
        eprintln!("{e} Repair the named keys.toml, then run razor keys.");
        return ExitCode::FAILURE;
    }
    let serving = first == Some("serve");
    let mut runtime = if serving {
        if let Some(dir) = &*INSTALL_DIR {
            // Agents' shells find Razor's own programs first and the ripgrep and jq it ships in
            // tools/ last, so the person's own copies of those win. An empty entry would be the
            // working folder, so none is kept.
            let old = std::env::var_os("PATH").unwrap_or_default();
            let old = std::env::split_paths(&old).filter(|p| !p.as_os_str().is_empty());
            let dirs = [dir.clone()].into_iter().chain(old);
            if let Ok(path) = std::env::join_paths(dirs.chain([dir.join("tools")])) {
                // SAFETY: no other threads exist before the runtime starts.
                unsafe { std::env::set_var("PATH", path) };
            }
        }
        let mut runtime = tokio::runtime::Builder::new_multi_thread();
        runtime.worker_threads(2);
        runtime
    } else {
        tokio::runtime::Builder::new_current_thread()
    };
    match runtime.enable_all().build() {
        Ok(runtime) => {
            let result = runtime.block_on(run(home, words));
            if serving {
                runtime.shutdown_timeout(residents::TEARDOWN_TIMEOUT);
            }
            result
        }
        Err(e) => {
            eprintln!("Razor cannot start its runtime: {e}.");
            ExitCode::FAILURE
        }
    }
}

async fn run(home: Option<PathBuf>, words: Vec<String>) -> ExitCode {
    let ran = match (home, words.first().map(String::as_str)) {
        (None, _) => Err(Usage(
            "Set RAZOR_HOME or HOME, so razor knows where its state lives.".into(),
        )),
        (
            Some(_),
            Some(
                command
                @ ("serve" | "keys" | "install" | "stop" | "restart" | "doctor" | "plugins"),
            ),
        ) if words.len() != 1 => Err(Usage(format!("Run razor {command} without arguments."))),
        (Some(home), Some("serve")) => serve(home).await,
        (Some(home), Some("setup")) if words.len() == 1 || words[1..] == ["--console"] => {
            match setup::run(&home, words.len() == 2).await {
                Ok(true) => terminal(home, Args::default()).await,
                Ok(false) => Ok(ExitCode::SUCCESS),
                Err(e) => Err(Failed(e.to_string())),
            }
        }
        (Some(_), Some("setup")) => Err(Usage("Use razor setup or razor setup --console.".into())),
        (Some(home), Some("keys")) => setup::keys(&home)
            .map(|()| ExitCode::SUCCESS)
            .map_err(|e| Failed(e.to_string())),
        (Some(home), Some(command @ ("stop" | "restart"))) => match residents::stop(&home).await {
            Ok(()) if command == "restart" => connect(&home).await.map(|_| ExitCode::SUCCESS),
            Ok(()) => Ok(ExitCode::SUCCESS),
            Err(e) => Err(Failed(e.to_string())),
        },
        (Some(home), Some("install")) => match residents::install(&home).await {
            Ok(sentence) => {
                println!("{sentence}");
                Ok(ExitCode::SUCCESS)
            }
            Err(e) => Err(Failed(e.to_string())),
        },
        (Some(home), Some("mcp")) => match api::mcp::parse(&home, &words[1..]) {
            Err(e) => Err(Usage(e.to_string())),
            Ok(request) => match connect(&home).await {
                Ok((client, key)) => match api::mcp::run(&client, &key, request).await {
                    Ok(text) => {
                        println!("{text}");
                        Ok(ExitCode::SUCCESS)
                    }
                    Err(e) => Err(Failed(e.to_string())),
                },
                Err(e) => Err(e),
            },
        },
        (Some(home), Some("login" | "devices")) => manage(home, &words).await,
        (Some(home), Some("doctor")) => doctor(home).await,
        (Some(home), Some("plugins")) => match razor_harness::config::Config::load(&home).await {
            Ok(config) => {
                print!("{}", extensions::list(&home, &config.plugins).await);
                Ok(ExitCode::SUCCESS)
            }
            Err(e) => Err(Failed(e.to_string())),
        },
        (Some(home), _) => match parse(words.into_iter()) {
            Ok(args @ Args { print: None, .. }) => terminal(home, args).await,
            Ok(args) => print(home, args).await,
            Err(e) => Err(e),
        },
    };
    match ran {
        Ok(code) => code,
        Err(Usage(sentence)) => {
            eprintln!("{sentence} Run razor --help.");
            ExitCode::from(2)
        }
        Err(Failed(sentence)) => {
            eprintln!("{sentence}");
            ExitCode::FAILURE
        }
    }
}

/// `razor serve`: one server per state directory, holding `server.lock`, until SIGTERM.
/// Raises the soft limit on open files to the hard one, at most 65,536. Every agent holds pipes to
/// its jobs, its MCP servers and its Post watch, and a shell's default (256 on macOS, 1,024 under
/// systemd) runs out near twenty agents, after which records stop opening. Best effort; the
/// limit it ends with goes to server.log, so a server started from any shell can be checked.
fn raise_open_files() {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: getrlimit and setrlimit read and write a struct this function owns.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } == 0 {
        limit.rlim_cur = limit.rlim_cur.max(limit.rlim_max.min(65_536));
        // SAFETY: as above; a refusal leaves the limit as it was.
        unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) };
        // SAFETY: as above.
        unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) };
        eprintln!("razor: open files allowed: {}", limit.rlim_cur);
    }
}

async fn serve(home: PathBuf) -> Result<ExitCode, Error> {
    raise_open_files();
    make_home(&home).map_err(|e| Failed(e.to_string()))?;
    let lock = residents::server_lock(&home).map_err(|e| Failed(e.to_string()))?;
    // The kernel releases it at process exit, after the runtime and any manager-visible work.
    std::mem::forget(lock);
    let failed = |e: &dyn std::fmt::Display| Failed(e.to_string());
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|e| failed(&e))?;
    razor_harness::record::backfill(&home).await;
    let responses = std::sync::Arc::new(
        razor_harness::responses::Responses::new(home.clone()).map_err(|e| failed(&e))?,
    );
    let controller = Controller::start(home.clone(), responses.clone())
        .await
        .map_err(|e| failed(&e))?;
    let address = controller.config().await.server.address;
    let residents = Residents::open(controller.clone());
    let deliveries = controller.deliveries();
    let shutdown = async move {
        terminate.recv().await;
    };
    let served = api::serve(
        home, address, controller, residents, deliveries, responses, shutdown,
    );
    served
        .await
        .map_err(|e| Failed(format!("The API server stopped: {e}")))?;
    Ok(ExitCode::SUCCESS)
}

/// `razor doctor`: one line for each thing Razor runs on, found or missing with what to do, a
/// line for settings that do not load, and never a credential's value. It
/// starts no server; Claude Code may create its own config directory during the sign-in check.
/// It exits 0 when the settings load and Claude or GPT can run.
async fn doctor(home: PathBuf) -> Result<ExitCode, Error> {
    use razor_harness::{api::ready, config::Config, tools::web};
    let config = Config::load(&home).await;
    let sign_in = config.as_ref().map_or(&[][..], |c| &c.claude.sign_in);
    let checks = ready::credentials(&home, Some(sign_in), None).await;
    let credential = config
        .as_ref()
        .map_or("ChatGPT sign-in", ready::gpt_credential);
    let mut lines = checks.clone();
    lines.retain(|name, _| *name == "Claude" || *name == credential);
    let fix = "For optional web search, run razor keys.";
    let keys = web::keys().map(|(name, set)| (name, (!set).then(|| fix.to_string())));
    for (name, why) in lines.into_iter().chain(keys).chain([(
        "ELEVENLABS_API_KEY",
        std::env::var_os("ELEVENLABS_API_KEY")
            .is_none()
            .then(|| "For optional voice, run razor keys.".into()),
    )]) {
        match why {
            None => println!("{name}: found."),
            Some(why) => println!("{name}: missing. {why}"),
        }
    }
    if let Err(e) = &config {
        println!("Settings: invalid. {e} Edit the named file, then run razor doctor.");
    }
    let browser = setup::browser();
    let probe = tokio::process::Command::new(&browser)
        .arg("--version")
        .kill_on_drop(true)
        .output();
    if tokio::time::timeout(Duration::from_secs(2), probe)
        .await
        .is_ok_and(|r| r.is_ok_and(|o| o.status.success()))
    {
        println!("Browser: found (agent-browser executable; Chrome launch untested).");
    } else {
        println!(
            "Browser: missing or unresponsive. Reinstall the agent-browser executable, then run agent-browser install."
        );
    }
    let client = reqwest::Client::builder()
        .unix_socket(home.join(api::SOCKET))
        .timeout(Duration::from_secs(2))
        .build();
    if let Ok(client) = client
        && answering(&home, &client).await.is_some()
    {
        println!("Server: running.");
    } else if config.is_ok() {
        println!("Server: stopped; your next razor command starts it.");
    }
    match residents::check_service(&home).await {
        Ok(true) => println!("Service: installed."),
        Ok(false) => println!("Service: absent (optional). Run razor install."),
        Err(e) => println!("Service: invalid. {e}"),
    }
    if let Some(state) = permissions(&home).await {
        println!("Mac permissions: {state}");
    }
    let fix = "Razor cannot start until its settings load; edit the file the Settings line names, then run razor doctor.";
    let config = config.map_err(|_| Failed(fix.into()))?;
    let none = format!(
        "Razor can run on neither Claude nor GPT yet; fix the Claude or {credential} line above."
    );
    let gpt = ready::unready(&checks, &config, "gpt-6.1-sol").is_none();
    let routes = match (checks.get("Claude").is_some_and(Option::is_none), gpt) {
        (true, true) => "Claude and GPT",
        (true, false) => "Claude",
        (false, true) => "GPT",
        (false, false) => return Err(Failed(none)),
    };
    println!("Razor can run on {routes}.");
    Ok(ExitCode::SUCCESS)
}

/// Doctor's word on the macOS permissions of the server for `home`, asked once and given 2
/// seconds; None off macOS or with no server running.
async fn permissions(home: &Path) -> Option<&'static str> {
    let key = tokio::fs::read_to_string(home.join(api::KEY)).await.ok()?;
    let client = reqwest::Client::builder().unix_socket(home.join(api::SOCKET));
    let client = client.timeout(Duration::from_secs(2)).build().ok()?;
    let ask = client.get(format!("{}/v1/permissions", api::LOCAL));
    let held = match ask.bearer_auth(key.trim()).send().await {
        Err(e) if e.is_timeout() && cfg!(target_os = "macos") => {
            return Some("unknown: the server did not answer within 2 seconds. Run razor restart.");
        }
        answer => answer.ok()?.json::<Option<[bool; 2]>>().await.ok()??,
    };
    Some(if held == [true; 2] {
        "found."
    } else {
        "missing. To let agents see the screen and control this Mac, turn on Razor in System Settings, Privacy & Security, under Accessibility (Device Control and Data Access on macOS 27) and Screen & System Audio Recording; then run razor restart."
    })
}

/// `razor login` and `razor devices [add <name> [--address <host:port>] [--key <key>] |
/// remove <name>]`, through the server.
async fn manage(home: PathBuf, words: &[String]) -> Result<ExitCode, Error> {
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    let (address, given) = match words[..] {
        ["login"] | ["devices"] | ["devices", "remove", _] => (None, None),
        ["devices", "add", _, ref flags @ ..] => {
            let (mut address, mut given) = (None, None);
            for pair in flags.chunks(2) {
                match pair {
                    ["--address", value] => address = Some(*value),
                    ["--key", value] => given = Some(*value),
                    _ => return Err(Usage(api::DEVICES_USE.into())),
                }
            }
            (address, given)
        }
        _ => return Err(Usage(api::DEVICES_USE.into())),
    };
    let (client, key) = connect(&home).await?;
    let text = match words[..] {
        ["login"] => (api::login(&client, &key).await)
            .map(|()| "Razor is signed in to ChatGPT.".into())
            .map_err(|e| api::Error::Refused(format!("{e} Run razor login."))),
        ["devices", "add", name, ..] => {
            // The address this server listens on, as it wrote it when it started.
            let found = tokio::fs::read(home.join("server.json"))
                .await
                .unwrap_or_default();
            let found: Value = serde_json::from_slice(&found).unwrap_or_default();
            api::enroll(
                &client,
                &key,
                name,
                address,
                given,
                found["address"].as_str(),
            )
            .await
        }
        ["devices", "remove", name] => api::revoke(&client, &key, name).await,
        _ => api::devices(&client, &key).await,
    };
    println!("{}", text.map_err(|e| Failed(e.to_string()))?);
    Ok(ExitCode::SUCCESS)
}

/// The local key, if a server for `home` answers to it through its socket.
async fn answering(home: &Path, client: &reqwest::Client) -> Option<String> {
    let key = tokio::fs::read_to_string(home.join(api::KEY)).await.ok()?;
    let key = key.trim().to_string();
    let probe = client.get(format!("{}/v1/devices", api::LOCAL));
    let answer = probe.bearer_auth(&key).send().await.ok()?;
    answer.status().is_success().then_some(key)
}

/// A client and local key for `home`. Start its installed service, or a detached server,
/// when none answers, then wait up to ten seconds for readiness.
async fn connect(home: &Path) -> Result<(reqwest::Client, String), Error> {
    let client = reqwest::Client::builder()
        .unix_socket(home.join(api::SOCKET))
        .build()
        .map_err(|e| Failed(format!("Razor cannot make its client: {e}.")))?;
    if let Some(key) = answering(home, &client).await {
        return Ok((client, key));
    }
    make_home(home).map_err(|e| Failed(e.to_string()))?;
    let service = residents::start(home)
        .await
        .map_err(|e| Failed(e.to_string()))?;
    let log_path = home.join("server.log");
    let child = if service {
        None
    } else {
        Some(spawn_server(&log_path)?)
    };
    let probe = async |mut child: Option<std::process::Child>| {
        let mut delay = Duration::from_millis(1);
        loop {
            if let Some(key) = answering(home, &client).await {
                return Ok::<_, Error>(Ok(key));
            }
            if let Some(child) = &mut child
                && let Some(status) = child
                    .try_wait()
                    .map_err(|e| Failed(format!("Checking razor serve failed: {e}.")))?
            {
                if let Some(key) = answering(home, &client).await {
                    return Ok(Ok(key));
                }
                if !residents::server_running(home).map_err(|e| Failed(e.to_string()))? {
                    return Ok(Err(status));
                }
            }
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(Duration::from_millis(100));
        }
    };
    let ready = async {
        if let Ok(key) = probe(child).await? {
            return Ok(key);
        }
        // A lock holder can exit after our child loses the race; make a server exist once more.
        match probe(Some(spawn_server(&log_path)?)).await? {
            Ok(key) => Ok(key),
            Err(status) => Err(server_exit(&log_path, status).await),
        }
    };
    if let Ok(result) = tokio::time::timeout(Duration::from_secs(10), ready).await {
        return result.map(|key| (client, key));
    }
    let logs = if service && cfg!(target_os = "linux") {
        let unit = residents::installed(home)
            .await
            .map_err(|e| Failed(e.to_string()))?;
        unit.map_or_else(
            || "journalctl --user".to_owned(),
            |(_, label)| format!("journalctl --user -u {label}.service"),
        )
    } else {
        log_path.display().to_string()
    };
    Err(Failed(format!(
        "No Razor server answered within 10 seconds; {logs} says why."
    )))
}

/// Start a detached server with both output streams in its private log.
fn spawn_server(log_path: &Path) -> Result<std::process::Child, Error> {
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(log_path)
        .map_err(|e| Failed(format!("server.log cannot be opened: {e}.")))?;
    let started = (|| {
        let errors = log.try_clone()?;
        std::process::Command::new(std::env::current_exe()?)
            .arg("serve")
            .stdin(Stdio::null())
            .stdout(log)
            .stderr(errors)
            .process_group(0)
            .spawn()
    })();
    started.map_err(|e| Failed(format!("razor serve cannot start: {e}.")))
}

/// Report an exited server's last log line, reading at most 4 KiB even after many starts.
async fn server_exit(log: &Path, status: std::process::ExitStatus) -> Error {
    let tail = async {
        let mut file = tokio::fs::File::open(log).await?;
        let start = file.metadata().await?.len().saturating_sub(4096);
        file.seek(std::io::SeekFrom::Start(start)).await?;
        let mut bytes = Vec::new();
        file.take(4096).read_to_end(&mut bytes).await?;
        Ok::<_, std::io::Error>(bytes)
    };
    let bytes = tail.await.unwrap_or_default();
    let text = String::from_utf8_lossy(&bytes);
    let cause = text.lines().rev().find(|line| !line.trim().is_empty());
    let cause = cause.map_or(String::new(), |line| format!("; last line: {line:?}"));
    Failed(format!(
        "razor serve exited ({status}) before answering; see {}{cause}.",
        log.display()
    ))
}

/// The interactive client stages launch choices until the first input is admitted.
async fn terminal(home: PathBuf, args: Args) -> Result<ExitCode, Error> {
    let (client, key) = connect(&home).await?;
    let session = if args.continue_newest {
        let cwd = std::env::current_dir().map_err(|e| Failed(e.to_string()))?;
        let response = client
            .get(format!("{}/v1/sessions", api::LOCAL))
            .query(&[("cwd", cwd.to_string_lossy().as_ref()), ("limit", "1")])
            .bearer_auth(&key)
            .send()
            .await
            .map_err(|e| Failed(format!("The session list could not be read: {e}.")))?;
        if !response.status().is_success() {
            return Err(Failed(
                "The server refused the session list; check server.log.".into(),
            ));
        }
        let rows: Vec<razor_harness::protocol::Listed> = response
            .json()
            .await
            .map_err(|e| Failed(format!("The session list is invalid: {e}.")))?;
        Some(
            rows.into_iter()
                .next()
                .ok_or_else(|| Failed("There is no session in this directory to continue.".into()))?
                .session,
        )
    } else {
        args.resume
    };
    let initial = razor_harness::protocol::ClientInput {
        text: args.message.unwrap_or_default(),
        model: args.model,
        effort: args.effort,
        speed: args.speed,
        ..Default::default()
    };
    let server = reqwest::Url::parse(api::LOCAL).map_err(|e| Failed(e.to_string()))?;
    razor_harness::terminal::run(client, server, key, session, initial, home, args.agents)
        .await
        .map_err(|e| Failed(e.to_string()))?;
    Ok(ExitCode::SUCCESS)
}

/// `--print`: send the message, then read the stream until the turn that answered it ends.
async fn print(home: PathBuf, args: Args) -> Result<ExitCode, Error> {
    let mode = args.print.unwrap_or(Mode::Text);
    let (session, end, answer) = print_turn(&home, args).await?;
    let sentence = match end {
        End::Done => {
            match mode {
                Mode::Text => println!("{answer}"),
                Mode::Json => println!("{}", json!({ "session": session, "message": answer })),
                Mode::Stream => {}
            }
            return Ok(ExitCode::SUCCESS);
        }
        End::Error(sentence) => sentence,
        End::Stopped => "The turn was stopped.".into(),
    };
    if mode == Mode::Json {
        println!("{}", json!({ "session": session, "error": sentence }));
    }
    Err(Failed(sentence))
}

async fn print_turn(home: &Path, args: Args) -> Result<(SessionId, End, String), Error> {
    let message = match args.message {
        Some(message) => message,
        None => {
            let mut text = String::new();
            tokio::io::stdin()
                .read_to_string(&mut text)
                .await
                .map_err(|e| Usage(format!("Standard input is not UTF-8 text: {e}.")))?;
            text
        }
    };
    if message.trim().is_empty() {
        return Err(Usage("The message is empty.".into()));
    }
    let (client, key) = connect(home).await?;
    let call = |path: &str, body: Value| {
        client
            .post(format!("{}{path}", api::LOCAL))
            .bearer_auth(&key)
            .json(&body)
            .send()
    };
    let answer = |r: reqwest::Response| async move {
        let ok = r.status().is_success();
        let body: Value = r.json().await.map_err(|e| Failed(e.to_string()))?;
        match body["error"].as_str() {
            _ if ok => Ok(body),
            Some(sentence) => Err(Failed(sentence.to_string())),
            None => Err(Failed("The server refused the request.".into())),
        }
    };
    let unreachable = |e: reqwest::Error| Failed(format!("The Razor server did not answer: {e}."));
    let id = |text: &str| SessionId::try_from(text.to_string()).map_err(|e| Failed(e.to_string()));
    let cwd = std::env::current_dir().map_err(|e| Failed(e.to_string()))?;
    let session = match (args.resume, args.continue_newest) {
        (Some(id), _) => id,
        (None, true) => {
            let listed = client
                .get(format!("{}/v1/sessions", api::LOCAL))
                .query(&[("cwd", cwd.to_string_lossy().as_ref()), ("limit", "1")])
                .bearer_auth(&key)
                .send()
                .await
                .map_err(unreachable)?;
            let listed = answer(listed).await?;
            id(listed[0]["session"].as_str().ok_or_else(|| {
                Failed("There is no session in this directory to continue.".into())
            })?)?
        }
        (None, false) => {
            let created = answer(
                call(
                    "/v1/sessions",
                    json!({ "cwd": cwd, "model": args.model, "effort": args.effort, "speed": args.speed }),
                )
                .await
                .map_err(unreachable)?,
            )
            .await?;
            id(created["session"].as_str().unwrap_or_default())?
        }
    };
    let admitted = answer(
        call(
            &format!("/v1/sessions/{session}/input"),
            json!({ "text": message, "model": args.model, "effort": args.effort, "speed": args.speed }),
        )
        .await
        .map_err(unreachable)?,
    )
    .await?;
    let input = EntryId(admitted["entry"].as_u64().unwrap_or(0));
    read_turn(&client, &key, session, input, args.print).await
}

/// Read the session's stream from just before `input` until the `turn_end` naming it. The
/// answer is the turn's text after its last call.
async fn read_turn(
    client: &reqwest::Client,
    key: &str,
    session: SessionId,
    input: EntryId,
    mode: Option<Mode>,
) -> Result<(SessionId, End, String), Error> {
    let url = format!(
        "{}/v1/sessions/{session}/events?after={}",
        api::LOCAL,
        input.0.saturating_sub(1)
    );
    let gone = || Failed("The Razor server went away before the turn ended.".into());
    let mut stream = client
        .get(url)
        .bearer_auth(key)
        .send()
        .await
        .map_err(|e| Failed(format!("The Razor server did not answer: {e}.")))?;
    // `pending` holds at most one stream line, and `texts` the turn's text since its last call.
    let (mut pending, mut texts) = (Vec::new(), Vec::<String>::new());
    let mut stdout = tokio::io::stdout();
    while let Some(chunk) = stream.chunk().await.map_err(|_| gone())? {
        pending.extend_from_slice(&chunk);
        while let Some(newline) = pending.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = pending.drain(..=newline).collect();
            if mode == Some(Mode::Stream) {
                let _ = stdout.write_all(&line).await;
                let _ = stdout.flush().await;
            }
            let Ok(Event::Entry(entry)) = serde_json::from_slice::<Event>(&line) else {
                continue;
            };
            match entry.body {
                Body::Text { text } => texts.push(text),
                Body::Call { .. } => texts.clear(),
                Body::TurnEnd { inputs, end, .. } if inputs.contains(&input) => {
                    return Ok((session, end, razor_harness::record::closing_text(&texts)));
                }
                Body::TurnEnd { .. } => texts.clear(),
                _ => {}
            }
        }
    }
    Err(gone())
}
