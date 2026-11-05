//! Tables shipped beside the code, read by this one loader: the emoji, in the order search ties
//! break on, with their names, shortcodes, keywords, tab and toned forms.
use super::{Error, error, fail};
use serde::Deserialize;
use std::{collections::BTreeMap, ops::Range};

/// One emoji: its default form, Unicode's name in lower case, its shortcodes without colons, its
/// keywords, its tab (1 to 8, after Frequently Used) and its toned forms, light to dark.
#[derive(Deserialize)]
pub struct Emoji {
    pub glyph: &'static str,
    pub name: &'static str,
    #[serde(borrow)]
    pub codes: Vec<&'static str>,
    #[serde(borrow)]
    pub keywords: Vec<&'static str>,
    tab: u8,
    #[serde(borrow)]
    pub tones: Option<[&'static str; 5]>,
}

/// Every emoji (1,914 of them), each tab's emoji as one range of them, and each shortcode's
/// first emoji.
#[derive(Default)]
pub struct Table {
    pub emoji: Vec<Emoji>,
    pub tabs: [Range<usize>; 8],
    pub codes: BTreeMap<&'static str, usize>,
}

pub fn load() -> Result<Table, Error> {
    let bad = "The emoji table could not be read; reinstall Razor.";
    let emoji: Vec<Emoji> =
        serde_json::from_str(include_str!("data/emoji.json")).map_err(fail(bad))?;
    let mut table = Table::default();
    for (i, e) in emoji.iter().enumerate() {
        let tab = usize::from(e.tab).checked_sub(1);
        let Some(range) = tab.and_then(|tab| table.tabs.get_mut(tab)) else {
            return Err(error(bad));
        };
        *range = if range.start == range.end {
            i..i + 1
        } else {
            range.start..i + 1
        };
        e.codes
            .iter()
            .for_each(|code| _ = table.codes.entry(code).or_insert(i));
    }
    table.emoji = emoji;
    Ok(table)
}

/// `text` with each known `:shortcode:` outside code replaced by its emoji: fenced blocks and
/// inline code are sent as written (Trey, 2026-10-30), and an unclosed backtick run is prose. A
/// code converts only standing alone, no letter, digit or colon against either colon, so
/// `std::id::x` stays as typed (Trey, 2026-10-30).
pub fn expand(text: &str, table: &Table) -> String {
    let mut out = String::with_capacity(text.len());
    let mut fenced = false;
    for line in text.split_inclusive('\n') {
        let head = line.trim_start();
        let fence = head.starts_with("```") || head.starts_with("~~~");
        if fenced || fence {
            fenced ^= fence;
            out.push_str(line);
            continue;
        }
        let mut rest = line;
        while let Some(i) = rest.find('`') {
            codes(&rest[..i], table, &mut out);
            let run = &rest[i..rest.len() - rest[i..].trim_start_matches('`').len()];
            let after = &rest[i + run.len()..];
            let end = i + run.len() + after.find(run).map_or(0, |n| n + run.len());
            out.push_str(&rest[i..end]);
            rest = &rest[end..];
        }
        codes(rest, table, &mut out);
    }
    out
}

fn codes(text: &str, table: &Table, out: &mut String) {
    let mut rest = text;
    while let Some(i) = rest.find(':') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let alone = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric() && c != ':');
        let end = rest[1..].find(':').map(|n| n + 1);
        let end =
            end.filter(|&e| alone(out.chars().next_back()) && alone(rest[e + 1..].chars().next()));
        match end.and_then(|end| Some((end, table.codes.get(&rest[1..end])?))) {
            Some((end, &n)) => {
                out.push_str(table.emoji[n].glyph);
                rest = &rest[end + 1..];
            }
            None => {
                out.push(':');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
}
