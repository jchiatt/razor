//! The four settings files: the pack's, the person's, a project's and its private one. Each keeps
//! its last good table and is read again only when a stat shows it changed.

use super::{Config, Error};
use crate::protocol::McpScope;
use serde::{Serialize, ser::SerializeStruct};
use std::{
    collections::BTreeMap,
    io::{ErrorKind, Read},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};
use toml::{Table, Value};

/// The starter pack's settings, used when the person's file names no pack.
const STARTER: &str = "packs/starter/config.toml";

/// Keys only the person's file and the pack may set.
const MACHINE: [&str; 22] = [
    "models",
    "call_model",
    "call_effort",
    "server",
    "claude",
    "skills",
    "startup",
    "residents",
    "pack",
    "brand",
    "ui",
    "phone",
    "speech",
    "transcribe",
    "live",
    "capture",
    "play",
    "guards",
    "plugins",
    "ask",
    "notify",
    "push",
];

/// Merged values and the file each came from. Clones are independent: the controller holds the
/// machine files, and each session its own copy with its project's two.
#[derive(Clone)]
pub(crate) struct Settings {
    pub(crate) config: Config,
    pub(crate) project: Option<PathBuf>,
    pub(super) sources: BTreeMap<String, PathBuf>,
    pub(crate) errors: Vec<String>,
    pack: Option<File>,
    person: File,
    /// The project's `config.toml` and `config.local.toml`, when there is a project.
    local: Vec<File>,
    values: Table,
    /// Why the merged values do not make a Config, when each file alone does.
    invalid: Option<String>,
}

#[derive(Clone)]
struct File {
    path: PathBuf,
    kind: Kind,
    /// The variant files `good` read, whose edits are this file's.
    variants: Vec<PathBuf>,
    /// The stats `good` was read at, the file's then its variants'; `None` reads the file at
    /// the next pass.
    seen: Option<Vec<Stamp>>,
    good: Table,
    error: Option<String>,
}

/// The pack's file must exist; a project's may not set machine keys.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Pack,
    Person,
    Project,
}

/// Device, inode, size and change time, or `None` for a missing file.
type Stamp = Option<(u64, u64, u64, i64, i64)>;

impl File {
    fn new(path: PathBuf, kind: Kind) -> Self {
        let (seen, good, error) = (None, Table::new(), None);
        Self {
            path,
            kind,
            variants: Vec::new(),
            seen,
            good,
            error,
        }
    }

    /// Reads the file if its stat or a variant file's changed and returns the table it replaced.
    /// A failed read keeps the last good table, holds its error, and is tried again next pass.
    fn read(&mut self) -> Option<Table> {
        let stamps: Vec<Stamp> = std::iter::once(&self.path)
            .chain(&self.variants)
            .map(|path| std::fs::metadata(path).ok())
            .map(|stat| stat.map(|m| (m.dev(), m.ino(), m.len(), m.ctime(), m.ctime_nsec())))
            .collect();
        if self.seen.as_ref() == Some(&stamps) {
            return None;
        }
        let read = text(&self.path, self.kind != Kind::Pack)
            .and_then(|text| parse(&text, &self.path, self.kind == Kind::Project));
        let table = match read {
            Ok((table, variants)) => {
                // Stamped against the old list, so a changed list is read once more next pass.
                self.variants = variants;
                table
            }
            Err(error) => {
                (self.seen, self.error) = (None, Some(error.to_string()));
                return None;
            }
        };
        // An edit within the clock's granularity of this read could leave a stamp unchanged,
        // so a file changed in the last 100 ms is read again at the next pass.
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH);
        let at = stamps
            .iter()
            .flatten()
            .map(|s| Duration::new(s.3 as u64, s.4 as u32));
        let at = at.max().unwrap_or_default();
        let settled = now.is_ok_and(|now| now.saturating_sub(at) >= Duration::from_millis(100));
        (self.seen, self.error) = (settled.then_some(stamps), None);
        (table != self.good).then(|| std::mem::replace(&mut self.good, table))
    }
}

impl Settings {
    /// The machine files; an invalid one refuses startup, and a missing person's file is empty.
    pub(crate) fn open(home: PathBuf) -> Result<Self, Error> {
        let home = std::path::absolute(home).map_err(io)?;
        let mut settings = Self {
            config: Config::default(),
            project: None,
            sources: BTreeMap::new(),
            errors: Vec::new(),
            pack: super::shipped(STARTER).map(|path| File::new(path, Kind::Pack)),
            person: File::new(home.join("config.toml"), Kind::Person),
            local: Vec::new(),
            values: Table::new(),
            invalid: None,
        };
        settings.read();
        match settings.errors.first() {
            Some(error) => Err(Error::Invalid(error.clone())),
            None => Ok(settings),
        }
    }

    /// The same machine files with a project's, which a linked worktree reads at its main
    /// checkout; `project` stays the local root, where commands and stdio servers run. A folder
    /// whose `.razor` is Razor's home, such as the person's home folder, has no project files:
    /// its `config.toml` is the person's own, already read.
    pub(crate) fn project(&self, cwd: &Path) -> Result<Self, Error> {
        let (root, files) = roots(cwd)?;
        let mut settings = self.clone();
        settings.project = Some(root);
        let dot = files.join(".razor");
        let id = |path: &Path| std::fs::metadata(path).ok().map(|m| (m.dev(), m.ino()));
        let home = self.person.path.parent().and_then(id);
        if home.is_none() || id(&dot) != home {
            settings.local = ["config.toml", "config.local.toml"]
                .map(|name| File::new(dot.join(name), Kind::Project))
                .into();
        }
        settings.read();
        Ok(settings)
    }

    /// Reads each changed file and merges again if a table changed; an invalid edit keeps that
    /// file's last good table and reports its error. True when a table changed.
    pub(crate) fn read(&mut self) -> bool {
        let mut changed = false;
        if let Some(old) = self.person.read() {
            changed = true;
            let named = self.person.good.get("pack").and_then(Value::as_str);
            let named = named.map(|dir| Path::new(dir).join("config.toml"));
            let named = named.or_else(|| super::shipped(STARTER));
            if named.as_ref() != self.pack.as_ref().map(|pack| &pack.path) {
                let mut pack = named.map(|path| File::new(path, Kind::Pack));
                if let Some(pack) = &mut pack {
                    pack.read();
                }
                match pack.as_mut().and_then(|pack| pack.error.take()) {
                    // A person's file naming a pack that does not read keeps its whole last
                    // good table, and the old pack.
                    Some(error) => {
                        (self.person.good, self.person.seen) = (old, None);
                        self.person.error = Some(error);
                    }
                    None => self.pack = pack,
                }
            }
        }
        for file in self.pack.iter_mut().chain(&mut self.local) {
            changed |= file.read().is_some();
        }
        if changed {
            self.merge();
        }
        let files = self.pack.iter().chain([&self.person]).chain(&self.local);
        let errors = files.filter_map(|file| file.error.as_ref());
        self.errors = errors.chain(&self.invalid).cloned().collect();
        changed
    }

    /// Later files win: a value by its key, and a table's entries one by one.
    fn merge(&mut self) {
        let (mut values, mut sources) = (Table::new(), BTreeMap::new());
        for file in self.pack.iter().chain([&self.person]).chain(&self.local) {
            for (key, value) in &file.good {
                if let (Some(Value::Table(under)), Value::Table(over)) =
                    (values.get_mut(key), value)
                {
                    under.extend(over.clone());
                } else {
                    let prefix = format!("{key}.");
                    sources.retain(|name: &String, _| name != key && !name.starts_with(&prefix));
                    values.insert(key.clone(), value.clone());
                }
                let Value::Table(table) = value else {
                    sources.insert(key.clone(), file.path.clone());
                    continue;
                };
                let names = table
                    .keys()
                    .map(|name| (format!("{key}.{name}"), file.path.clone()));
                sources.extend(names);
            }
        }
        self.invalid = match Config::parse(values.clone()) {
            Ok(mut config) => {
                config.pack = self
                    .pack
                    .as_ref()
                    .and_then(|pack| pack.path.parent().map(Path::to_path_buf));
                (self.config, self.values, self.sources) = (config, values, sources);
                None
            }
            Err(error) => Some(error.to_string()),
        };
    }

    /// The file a scope writes, and whether it is a project's: with no project files (see
    /// `project`), every scope is the person's file.
    pub(super) fn destination(&self, scope: McpScope) -> Result<(PathBuf, bool), Error> {
        let file = self.local.get(usize::from(scope == McpScope::Private));
        let Some(path) = file
            .filter(|_| scope != McpScope::Global)
            .map(|f| f.path.clone())
        else {
            return Ok((self.person.path.clone(), false));
        };
        // The folder holding `.razor`, never recreated to hold settings once moved or deleted.
        match path.ancestors().nth(2).filter(|folder| !folder.is_dir()) {
            Some(folder) => Err(Error::Invalid(format!(
                "{} no longer exists; restore it before changing its project settings.",
                folder.display()
            ))),
            None => Ok((path, true)),
        }
    }
}

/// The normalized file values rather than every nested Config type's own serialization;
/// defaults have no source and are absent.
impl Serialize for Settings {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("Settings", 4)?;
        state.serialize_field("config", &self.values)?;
        state.serialize_field("project", &self.project)?;
        state.serialize_field("sources", &self.sources)?;
        state.serialize_field("errors", &self.errors)?;
        state.end()
    }
}

/// A settings, import or git pointer file of at most 1 MiB; a missing optional one is empty.
pub(super) fn text(path: &Path, optional: bool) -> Result<String, Error> {
    let shown = path.display();
    let mut bytes = Vec::new();
    match std::fs::File::open(path) {
        Err(error) if optional && error.kind() == ErrorKind::NotFound => {}
        file => {
            let read = file.and_then(|file| file.take((1 << 20) + 1).read_to_end(&mut bytes));
            read.map_err(|error| {
                Error::Invalid(format!(
                    "Cannot read {shown}: {error}; check the file and its permissions."
                ))
            })?;
        }
    }
    if bytes.len() > 1 << 20 {
        return Err(Error::Invalid(format!(
            "{shown} is larger than 1 MiB; shorten it."
        )));
    }
    String::from_utf8(bytes)
        .map_err(|_| Error::Invalid(format!("{shown} is not UTF-8 text; save it as UTF-8.")))
}

/// One file's table: machine keys refused in a project's, paths made absolute (`~/` against the
/// home folder, others against its directory), and the whole checked as a Config so an error
/// names its file; with the variant files it read. A startup list's `razor`, `project` and
/// `about` are words, not paths.
pub(super) fn parse(
    text: &str,
    path: &Path,
    project: bool,
) -> Result<(Table, Vec<PathBuf>), Error> {
    let shown = path.display();
    let mut table: Table = text.parse().map_err(|e: toml::de::Error| {
        Error::Invalid(format!("{shown} does not parse: {}.", e.message()))
    })?;
    // Only the person names where a project's tasks go, so a repository cannot send them away.
    if table.contains_key("tasks") && !(project && path.ends_with("config.local.toml")) {
        let only = "only a project's .razor/config.local.toml names where its tasks go";
        return Err(Error::Invalid(format!("{shown} cannot set tasks; {only}.")));
    }
    if project {
        let nested = |name: &str, key| table.get(name).and_then(Value::as_table)?.get(key);
        let mut keys: Vec<_> = MACHINE
            .into_iter()
            .filter(|key| table.contains_key(*key))
            .collect();
        keys.extend(nested("responses", "base_url").map(|_| "responses.base_url"));
        keys.extend(table.get("sessions").map(|_| "sessions"));
        keys.extend(nested("children", "startup").map(|_| "children.startup"));
        if !keys.is_empty() {
            let keys = keys.join(", ");
            let why = format!("{shown} cannot set {keys}; move these keys to the person's file.");
            return Err(Error::Invalid(why));
        }
    }
    let (dir, home) = (
        path.parent().unwrap_or(Path::new("/")),
        std::env::var_os("HOME"),
    );
    let absolute = |value: Option<&mut Value>| {
        if let Some(Value::String(path)) = value {
            let full = match (path.strip_prefix("~/"), &home) {
                (Some(rest), Some(home)) => Path::new(home).join(rest),
                _ => dir.join(&*path),
            };
            *path = full.to_string_lossy().into_owned();
        }
    };
    absolute(table.get_mut("pack"));
    // A program named by a path, not by a bare name found on PATH, counts from its file too.
    for key in ["ask", "notify", "speech", "transcribe", "capture", "play"] {
        let program = list(table.get_mut(key)).next();
        absolute(program.filter(|p| p.as_str().is_some_and(|p| p.contains('/'))));
    }
    list(table.get_mut("skills")).for_each(|dir| absolute(Some(dir)));
    let file = |e: &&mut Value| !matches!(e.as_str(), Some("razor" | "project" | "about"));
    list(table.get_mut("startup"))
        .filter(file)
        .for_each(|entry| absolute(Some(entry)));
    if let Some(children) = table.get_mut("children").and_then(Value::as_table_mut) {
        list(children.get_mut("startup"))
            .filter(file)
            .for_each(|entry| absolute(Some(entry)));
        absolute(children.get_mut("reviewer"));
        absolute(children.get_mut("researcher"));
    }
    if let Some(claude) = table.get_mut("claude").and_then(Value::as_table_mut) {
        absolute(claude.get_mut("usage_file"));
    }
    for key in ["development", "production"] {
        let push = table.get_mut("push").and_then(Value::as_table_mut);
        let key = push
            .and_then(|p| p.get_mut(key))
            .and_then(Value::as_table_mut);
        absolute(key.and_then(|k| k.get_mut("key")));
    }
    // A variant file is read with the file naming it, so a malformed one refuses this file.
    let mut variants = Vec::new();
    for chat in tables(table.get_mut("models")) {
        absolute(chat.get_mut("variant"));
        if let Some(Value::String(path)) = chat.get("variant") {
            let path = PathBuf::from(path);
            let json: serde_json::Value = serde_json::from_str(&self::text(&path, false)?)
                .map_err(|e| Error::Invalid(format!("{} is not JSON: {e}.", path.display())))?;
            let variant = Value::try_from(json).map_err(|e| {
                Error::Invalid(format!("{} cannot be a variant: {e}.", path.display()))
            })?;
            chat.insert("variant".into(), variant);
            variants.push(path);
        }
    }
    for resident in tables(table.get_mut("residents")) {
        absolute(resident.get_mut("cwd"));
        let entries = list(resident.get_mut("startup")).filter(file);
        entries.for_each(|entry| absolute(Some(entry)));
    }
    for server in tables(table.get_mut("mcp")) {
        for header in tables(server.get_mut("headers")) {
            absolute(header.get_mut("file"));
        }
        let oauth = server.get_mut("oauth").and_then(Value::as_table_mut);
        let secret = oauth.and_then(|oauth| oauth.get_mut("client_secret")?.as_table_mut());
        absolute(secret.and_then(|secret| secret.get_mut("file")));
    }
    Config::parse(table.clone()).map_err(|e| Error::Invalid(format!("{shown}: {e}")))?;
    Ok((table, variants))
}

/// An array's values, such as `skills`'s folders.
fn list(value: Option<&mut Value>) -> impl Iterator<Item = &mut Value> {
    value.and_then(Value::as_array_mut).into_iter().flatten()
}

/// The tables among a table's values, such as each `[mcp.<name>]`.
fn tables(value: Option<&mut Value>) -> impl Iterator<Item = &mut Table> {
    let values = value
        .and_then(Value::as_table_mut)
        .into_iter()
        .flat_map(|t| t.iter_mut());
    values.filter_map(|(_, value)| value.as_table_mut())
}

/// The local root, and the checkout whose `.razor` files apply: a linked worktree's main one.
pub(crate) fn roots(cwd: &Path) -> Result<(PathBuf, PathBuf), Error> {
    let cwd = match std::fs::canonicalize(cwd) {
        // A folder moved or deleted since keeps its path, whose ancestors still find the project.
        Err(e) if e.kind() == ErrorKind::NotFound => std::path::absolute(cwd).map_err(io)?,
        cwd => cwd.map_err(io)?,
    };
    for root in cwd.ancestors() {
        let dot = root.join(".git");
        match std::fs::metadata(&dot) {
            Ok(meta) if meta.is_dir() => return Ok((root.into(), root.into())),
            Ok(_) => return Ok((root.into(), main(root, &dot)?)),
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(io(error)),
        }
    }
    Ok((cwd.clone(), cwd))
}

/// A linked worktree's main checkout, from its `.git` pointer and git's common directory.
fn main(root: &Path, dot: &Path) -> Result<PathBuf, Error> {
    let pointer = text(dot, false)?;
    let Some(gitdir) = pointer.trim().strip_prefix("gitdir:") else {
        let dot = dot.display();
        return Err(Error::Invalid(format!(
            "{dot} needs a gitdir pointer; repair the worktree."
        )));
    };
    let gitdir = root.join(gitdir.trim());
    let common = text(&gitdir.join("commondir"), true)?;
    if common.trim().is_empty() {
        return Ok(root.into());
    }
    let common = std::fs::canonicalize(gitdir.join(common.trim())).map_err(io)?;
    Ok(common.parent().unwrap_or(&common).into())
}

fn io(source: std::io::Error) -> Error {
    Error::Other {
        source,
        context: "finding settings files",
    }
}
