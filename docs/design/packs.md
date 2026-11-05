# Deployment packs

Files: `src/config/pack.rs` (new), with small edits to `src/config/mod.rs`, `src/protocol/api.rs`, `src/controller/status.rs` and `mod.rs`, the startup page in `src/controller/session/conversation.rs`, and `src/tools/files.rs` for the skill list.

A pack is a directory that configures one deployment (SPEC section 2). Razor reads two things in it by fixed name: `config.toml`, which has the person's keys and sits beneath the person's file, and `skills/`. Everything else (personas, residents' folders, data) is reached through a key in that file or by the residents themselves. The core never knows which tenant, role or brand it runs.

## Surface

```rust
// src/config/mod.rs
pub struct Config {
    // ...today's keys, plus:
    /// `pack`: the pack's directory. Only the person's file names a pack: the person's value
    /// always wins the layering, so a `pack` inside a pack is never read.
    pub pack: Option<PathBuf>,
    /// `[brand]`: the product's name and colours, passed through to clients.
    pub brand: protocol::Brand,
}
impl Config {
    /// The person's file with its pack beneath it, then today's checks.
    /// - A file that is missing (the pack's), unreadable, over 1 MiB, not UTF-8 or that does
    ///   not parse is refused with a sentence that names it.
    /// - A value that does not fit is refused with the context TOML gives: its key, such as
    ///   `guards.delete`, except inside a resident, whose fields arrive flattened and lose it.
    /// The controller keeps the last good config and shows the sentence, as today.
    pub async fn load(home: &Path) -> Result<Self, Error>;
    /// `skills` is `[<pack>/skills]` with a pack, empty without.
    pub fn tools(&self) -> tools::Config;
}

// src/config/pack.rs, private to the config module
/// The person's `home/config.toml` laid over the pack's `config.toml`. Before layering,
/// every relative path a key names (such as `pack`, `skills`, a `startup` file or a resident's
/// `cwd`) is made absolute against the folder of the file that holds it, and `~/` against the
/// home folder. A top-level table both files
/// have merges key by key; every other value comes whole from one file, the person's if he
/// wrote it.
pub(super) async fn table(home: &Path) -> Result<toml::Table, Error>;

// src/protocol/api.rs
/// A pack's brand. Nothing in the core reads it.
pub struct Brand {
    /// Wherever a client names the product. Default "Razor".
    pub name: String,
    /// Theme colour names (such as `glass` and `violet`) to `#rrggbb`.
    pub colours: BTreeMap<String, String>,
}
pub enum Live {
    /// Every `agents` snapshot carries the brand of the last good config.
    Agents { agents: Vec<AgentView>, config: Option<String>, limit: Option<Limit>, brand: Brand },
    // ...unchanged
}

// src/tools/files.rs
impl Tools {
    /// The startup page's skill list: the first 100 skills of the pack's and the project's
    /// folders, by name with the first 200 characters of each `description`; 4 KiB read from each. None when
    /// there are none.
    pub async fn skills(&self) -> Option<String>;
}
```

The controller's status snapshot starts with the brand of the config `Controller::start` read, and sends `agents` again whenever a read changes the brand, beside the config notice. The skill list goes after the startup page, in the first input of each native conversation, never in the `skill` declaration, so a changed skill never changes the cached prefix.

**For clients.** `{"kind":"agents",…,"brand":{"name":"Acme","colours":{"violet":"#336699"}}}`. A status stream's first line is always `agents`. A client draws each named colour, keeps its own for every name left out, ignores what it cannot use, derives blends from the result (using the theme colours), and shows `name` wherever it names the product.

**When a change takes effect.** Every key is live as today: read once a second and at each admitted turn. A pack's skills, connections and brand follow at once. Residents keep their own timing: `cwd` is fixed when the resident's session is made; its startup list is read when a conversation opens; a removed resident's running turn finishes. Nothing restarts because a pack changed.

## Data flow

1. The person's `$RAZOR_HOME/config.toml` names a pack: `pack = "/path/to/pack"`.
2. Every config read goes through `Config::load`; `pack::table` reads the person's file, then the pack's.
3. In each file, every relative path a key names (such as `pack`, `skills`, a `startup` file or a resident's `cwd`) becomes absolute against that file's folder, and `~/` against the home folder.
4. The person's table is laid over the pack's: each top-level table key by key, each value whole.
5. The layered table becomes `Config`; today's checks run; a failure keeps the last good config and its notice says why.
6. Residents: the residents task reads `config.residents` at each wake; a pack's residents start, change and stop like any other.
7. Skills: at the next admitted turn `Tools::configure` gets the skill folders in lookup order: `<pack>/skills`, the project's `.agents/skills` and `.claude/skills` (Codex's and Claude Code's places), then the `skills` key's folders (2026-10-29, Trey: "i like the config key and project skill ideas"). The first folder holding a `SKILL.md` under a name wins, so a project's skill never replaces the pack's, as in Claude Code (personal over project); a library named in `skills` comes after the project, since a repository's own copy of a skill is the tailored one. A new conversation's first input lists only the pack's and project's skills with descriptions, up to 100. Library skills remain available through `skill` and the full catalog endpoint used by slash-command search. `skills` is a machine key; its relative paths count from its file.
8. Connections (`[mcp.*]`, `[tools.*]`, `[web]`) reach `Tools::configure` as if the person had written them.
9. Brand: the status snapshot starts with the startup config's brand and is sent again when a read changes it.
10. Memory: two tables of at most 1 MiB, for one read; between reads only the controller's `Config`.

## The hard decision: how the pack and the person's config compose

**Chosen: the person's file is laid over the pack's, one level deep.** Each top-level table merges key by key, and the value under each key comes whole from one file. TOML's shape already separates a setting from an entry, so one rule covers both:

| Kind | Examples | Both files name it |
| --- | --- | --- |
| Setting | `guards.delete`, `children.max`, `server.address` | The person's value replaces the pack's; the rest of the pack's table stays. |
| Entry | `mcp.crm`, `residents.helper`, `web.exa`, `brand.colours` | The person's entry replaces the pack's whole; nothing is blended. Entries only one file names are all kept. |
| List or top-level value | `web.fetch`, `startup`, `model` | The person's replaces the pack's; lists are never joined. |

What it costs: to change one field of a pack's entry the person copies the whole entry, a later pack update to that entry no longer reaches him, and his file cannot remove a pack's entry. In version one the only pack is the deployer's own, so none of these bites yet.

The alternatives:

- **The pack is the whole config.** No merging, but every fact about one machine (the server's address, the speech and capture programs, each resident's home) lives in the pack, so a pack cannot be shared; a tenant's person cannot change his model without editing the deployer's pack, and every key in the person's file but `pack` has no reader (RULES 12).
- **Deep merge.** One rule, and one field of a pack's resident can change alone. But an entry both files name becomes a blend neither shows: `[mcp.crm] command = "crm-v2"` keeps the pack's `args` and `env`, meant for the old command.
- **A key in both files is an error.** No precedence question, but no pack default can be adjusted, and a pack update that adds a key the person has stops `razor serve` from starting.

There is no `residents/` folder and no brand file: residents and the brand are tables in the pack's `config.toml`, read, bounded, kept live and layered by the code every other key goes through. A resident folder would be a second resident format with a second reader.

No new crate: `toml` 1.1.8 already gives `Table` and `Table::try_into`. A pack is data.

## Notes

- Packs can link their `skills/` directory and name additional skill folders in `skills`.

Glossary lines proposed: **brand**: the product name and colours a pack gives every client. **pack**, amended: a directory whose `config.toml` sits beneath the person's config and whose `skills/` the `skill` action searches.
