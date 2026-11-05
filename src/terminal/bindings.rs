//! Compiled terminal bindings; defaults, descriptions and contexts live in data/keys.toml.
use super::{
    App, Error,
    composer::{completion, shortcodes},
    text::*,
};
use crossterm::event::{KeyCode as K, KeyEvent, KeyModifiers as M};
use ratatui::text::Line;
use std::{
    collections::{BTreeMap, HashMap},
    sync::LazyLock,
};
type Keys = BTreeMap<String, BTreeMap<String, Vec<String>>>;
#[derive(serde::Deserialize)]
struct Catalogue {
    keys: Keys,
    descriptions: BTreeMap<String, String>,
}
static CATALOGUE: LazyLock<Result<Catalogue, toml::de::Error>> =
    LazyLock::new(|| toml::from_str(include_str!("data/keys.toml")));
type Map = HashMap<(&'static str, KeyEvent), (&'static str, KeyEvent)>;
#[derive(Default)]
pub struct Bindings {
    maps: Map,
    keys: BTreeMap<&'static str, Vec<String>>,
    prefix: bool,
}

fn chord(s: &str) -> Option<KeyEvent> {
    let (mut s, mut mods) = (s, M::NONE);
    while let Some((head, tail)) = s.split_once('-') {
        mods |= match head.to_ascii_lowercase().as_str() {
            "ctrl" => M::CONTROL,
            "alt" => M::ALT,
            "shift" => M::SHIFT,
            "super" => M::SUPER,
            _ => break,
        };
        s = tail;
    }
    let code = match s.to_ascii_lowercase().as_str() {
        "enter" => K::Enter,
        "esc" | "escape" => K::Esc,
        "space" => K::Char(' '),
        "tab" => K::Tab,
        "backtab" => K::BackTab,
        "backspace" => K::Backspace,
        "delete" => K::Delete,
        "insert" => K::Insert,
        "home" => K::Home,
        "end" => K::End,
        "up" => K::Up,
        "down" => K::Down,
        "left" => K::Left,
        "right" => K::Right,
        "pageup" => K::PageUp,
        "pagedown" => K::PageDown,
        f if f.starts_with('f') && f.len() > 1 => {
            K::F(f[1..].parse().ok().filter(|n| (1..=24).contains(n))?)
        }
        _ => {
            let mut chars = s.chars();
            let c = chars.next()?;
            if chars.next().is_some() || c.is_control() {
                return None;
            }
            K::Char(if mods.contains(M::SHIFT) {
                c.to_ascii_uppercase()
            } else if !mods.is_empty() {
                c.to_ascii_lowercase()
            } else {
                c
            })
        }
    };
    Some(KeyEvent::new(code, mods))
}

fn normalized(mut k: KeyEvent) -> KeyEvent {
    if let K::Char(c) = k.code {
        if k.modifiers.contains(M::SHIFT) {
            k.code = K::Char(c.to_ascii_uppercase());
        }
        if c != ' ' {
            k.modifiers.remove(M::SHIFT);
        }
    }
    if k.code == K::Tab && k.modifiers.contains(M::SHIFT) {
        k.code = K::BackTab;
    }
    if k.code == K::BackTab {
        k.modifiers.remove(M::SHIFT);
    }
    KeyEvent::new(k.code, k.modifiers)
}

/// Rejects invalid actions, chords and collisions before replacing any bindings.
pub(super) fn compile(overrides: &Keys) -> Result<Bindings, Error> {
    let data = CATALOGUE.as_ref().map_err(super::fail(
        "The default key catalogue is invalid; reinstall Razor.",
    ))?;
    for (context, actions) in overrides {
        if !data
            .keys
            .get(context)
            .is_some_and(|known| actions.keys().all(|name| known.contains_key(name)))
        {
            return Err(super::error(format!(
                "Unknown key action in {context}; use /keys for the available actions."
            )));
        }
    }
    let mut bindings = Bindings::default();
    for (context, actions) in &data.keys {
        for (name, defaults) in actions {
            let full = data
                .descriptions
                .get_key_value(&format!("{context}.{name}"))
                .map(|(name, _)| name.as_str())
                .ok_or_else(|| super::error("A key description is missing; reinstall Razor."))?;
            let keys = overrides
                .get(context)
                .and_then(|a| a.get(name))
                .unwrap_or(defaults);
            if keys.len() > 8 {
                return Err(super::error(format!(
                    "Too many keys for {full}; keep at most eight."
                )));
            }
            let native = defaults
                .first()
                .and_then(|s| chord(s))
                .unwrap_or(K::Null.into());
            for s in keys {
                let key = chord(s).ok_or_else(|| super::error(format!("Invalid key {s} for {full}; use modifiers and a named key or one character.")))?;
                if let Some((other, _)) = bindings
                    .maps
                    .insert((context.as_str(), normalized(key)), (full, native))
                    && other != full
                {
                    return Err(super::error(format!(
                        "Key {s} is assigned twice in {context}; choose distinct keys."
                    )));
                }
            }
            bindings.keys.insert(full, keys.clone());
        }
    }
    Ok(bindings)
}

/// Keeps the last valid table when a live settings update contains an invalid binding.
pub fn reload(app: &mut App) {
    match compile(&app.ui.keys) {
        Ok(b) => app.bindings = b,
        Err(e) => app.say(&e.to_string()),
    }
}

/// All effective chords for one qualified action, including an explicit unbound label.
pub fn hint(app: &App, action: &str) -> String {
    match app.bindings.keys.get(action) {
        Some(keys) if !keys.is_empty() => keys.join("/"),
        _ => "unbound".into(),
    }
}

/// Expands action names in angle brackets in a visible hint.
pub fn text(app: &App, template: &str) -> String {
    template
        .split('<')
        .enumerate()
        .map(|(i, part)| match part.split_once('>') {
            Some((action, tail)) if app.bindings.keys.contains_key(action) => {
                hint(app, action) + tail
            }
            _ if i > 0 => format!("<{part}"),
            _ => part.into(),
        })
        .collect()
}

/// Lists every action, context, description and effective chord.
pub fn rows(app: &App, width: usize) -> Vec<Line<'static>> {
    let Ok(data) = CATALOGUE.as_ref() else {
        return Vec::new();
    };
    app.bindings
        .keys
        .keys()
        .flat_map(|action| {
            let about = &data.descriptions[*action];
            let words = format!("{action}: {} · {about}", hint(app, action));
            wrap(vec![span(words, DATA)], width)
        })
        .collect()
}

fn available(app: &App, context: &str, action: &str, k: KeyEvent) -> bool {
    let modal =
        app.panel.is_some() || app.picker.is_some() || app.start.is_some() || app.grid.is_some();
    let empty = app.composer.text.is_empty();
    match context {
        "early" => action == "global.record" || action == "global.quit" && app.grid.is_none(),
        "voice" => super::audio::captures(app, k),
        "mcp" => super::mcp::captures(app, k),
        "panel" => app.panel.is_some(),
        "picker" => app.picker.is_some() && app.panel.is_none(),
        "new" => action == "global.new" && app.panel.is_none() && app.picker.is_none(),
        "start" => app.start.is_some() && app.panel.is_none() && app.picker.is_none(),
        "open" => action == "list.open" && !modal,
        "list" => action != "list.open" && app.list.open && app.list.keys && !modal,
        "emoji" => {
            app.grid.is_some() && app.panel.is_none() && app.picker.is_none() && app.start.is_none()
        }
        "transcript" if app.picker.is_some() => app.panel.is_none(),
        _ if modal => false,
        "global" | "transcript" | "skill" | "composer" | "editor" => true,
        "attachments" => app.composer.chosen.is_some(),
        "completion" => completion::captures(app),
        "shortcode" => shortcodes::typed(app).is_some(),
        "children" => empty,
        "targets" => {
            if matches!(k.code, K::Up | K::Down) {
                app.selected.is_some()
            } else {
                empty
            }
        }
        "queued_input" => {
            empty
                && app
                    .transcript()
                    .is_some_and(|t| t.queued.difference(&t.taking).next().is_some())
        }
        // A child's split view draws the band with its keys off: they go to the split's own input.
        "question" => {
            empty
                && app.answering.is_none()
                && !app.split_view()
                && app.shown_ask.as_ref().is_some_and(|q| match k.code {
                    K::Char(c) => q.options.len() > c as usize - '1' as usize,
                    _ => true,
                })
        }
        "line_start" => {
            app.composer.cursor() == 0 || app.composer.text[..app.composer.cursor()].ends_with('\n')
        }
        "attached_images" => {
            app.composer.vertical_edge(false) && app.composer.active_images().next().is_some()
        }
        "input_history" => app.composer.vertical_edge(k.code == K::Down),
        _ => false,
    }
}

/// Resolves one physical key once, preserving modal precedence and ordinary printable input.
pub fn translate(app: &mut App, key: KeyEvent) -> Option<(KeyEvent, &'static str, &'static str)> {
    let k = normalized(key);
    if std::mem::take(&mut app.bindings.prefix) {
        match app.bindings.maps.get(&("prefix", k)).copied() {
            Some((_, native)) => super::agents::prefixed(app, native),
            None => app.say("That prefix action is unbound; /keys lists the available actions."),
        }
        return None;
    }
    for context in "early voice mcp panel picker new start open list emoji global transcript skill attachments completion shortcode targets children queued_input question line_start attached_images input_history composer editor".split_whitespace() {
        let scope = match context {
            "early" | "new" => "global",
            "open" => "list",
            _ => context,
        };
        if let Some((action, native)) = app
            .bindings
            .maps
            .get(&(scope, k))
            .copied()
            && available(app, context, action, native)
        {
            if action == "global.prefix" {
                app.bindings.prefix = true;
                return None;
            }
            return Some((native, scope, action));
        }
    }
    let text = matches!(key.code, K::Char(_))
        && !key
            .modifiers
            .intersects(M::CONTROL | M::ALT | M::SUPER | M::META);
    text.then_some((key, "text", ""))
}
