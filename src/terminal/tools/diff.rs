//! Opened diffs keep source-line references, never a rendered copy of the whole diff.
use super::super::text::*;
use super::Call;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    text::{Line, Span},
};

#[derive(Clone, Copy)]
struct Cell<'a> {
    text: &'a str,
    number: Option<u64>,
    sign: char,
}
enum Item<'a> {
    Path(&'a str),
    Lines(Option<Cell<'a>>, Option<Cell<'a>>),
}

// Source references are bounded by the admitted call (64 MiB); the LCS is at most 301² cells.
fn items<'a>(call: &'a Call<'_>) -> Option<Vec<Item<'a>>> {
    let (kind, input) = call.input();
    match kind {
        "edit" => {
            let old: Vec<_> = input["old_string"].as_str()?.lines().collect();
            let new: Vec<_> = input["new_string"].as_str()?.lines().collect();
            let first = serde_json::from_str::<serde_json::Value>(call.raw())
                .ok()
                .and_then(|v| v["line"].as_u64());
            let head = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
            let tail = old[head..]
                .iter()
                .rev()
                .zip(new[head..].iter().rev())
                .take_while(|(a, b)| a == b)
                .count();
            Some(compare(
                &old[head..old.len() - tail],
                &new[head..new.len() - tail],
                first.map(|n| n.saturating_add(head as u64)),
            ))
        }
        "apply_patch" => Some(patch(input.as_str().or_else(|| input["patch"].as_str())?)),
        _ => None,
    }
}

fn compare<'a>(old: &[&'a str], new: &[&'a str], first: Option<u64>) -> Vec<Item<'a>> {
    let (n, m) = (old.len(), new.len());
    let size = if n <= 300 && m <= 300 {
        (n + 1) * (m + 1)
    } else {
        0
    };
    let mut table = vec![0u16; size];
    if !table.is_empty() {
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                table[i * (m + 1) + j] = if old[i] == new[j] {
                    1 + table[(i + 1) * (m + 1) + j + 1]
                } else {
                    table[(i + 1) * (m + 1) + j].max(table[i * (m + 1) + j + 1])
                };
            }
        }
    }
    let (mut i, mut j, mut changes, mut out) = (0, 0, Vec::new(), Vec::new());
    while i < n || j < m {
        if !table.is_empty() && i < n && j < m && old[i] == new[j] {
            out.extend(pair(&changes));
            changes.clear();
            let cell = |text, at| {
                Some(Cell {
                    text,
                    number: first.map(|n| n.saturating_add(at)),
                    sign: ' ',
                })
            };
            out.push(Item::Lines(cell(old[i], i as u64), cell(new[j], j as u64)));
            i += 1;
            j += 1;
            continue;
        }
        let remove = i < n
            && (j == m
                || table.is_empty()
                || table[(i + 1) * (m + 1) + j] >= table[i * (m + 1) + j + 1]);
        let (text, index, sign) = if remove {
            (old[i], i, '-')
        } else {
            (new[j], j, '+')
        };
        changes.push(Cell {
            text,
            number: first.map(|n| n.saturating_add(index as u64)),
            sign,
        });
        if remove {
            i += 1;
        } else {
            j += 1;
        }
    }
    out.extend(pair(&changes));
    out
}

fn pair<'a>(changes: &[Cell<'a>]) -> Vec<Item<'a>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < changes.len() {
        if changes[at].sign == ' ' {
            out.push(Item::Lines(Some(changes[at]), Some(changes[at])));
            at += 1;
            continue;
        }
        let end = changes[at..]
            .iter()
            .position(|c| c.sign == ' ')
            .map_or(changes.len(), |n| at + n);
        let (old, new): (Vec<_>, Vec<_>) = changes[at..end]
            .iter()
            .copied()
            .partition(|c| c.sign == '-');
        out.extend(
            (0..old.len().max(new.len()))
                .map(|n| Item::Lines(old.get(n).copied(), new.get(n).copied())),
        );
        at = end;
    }
    out
}

fn patch(text: &str) -> Vec<Item<'_>> {
    let (mut out, mut changes) = (Vec::new(), Vec::new());
    for line in text.lines() {
        let path = [
            "*** Update File: ",
            "*** Add File: ",
            "*** Delete File: ",
            "*** Move to: ",
        ]
        .iter()
        .find_map(|p| line.strip_prefix(p));
        if path.is_some() || line.starts_with("@@") || line.starts_with("***") {
            out.extend(pair(&changes));
            changes.clear();
            if let Some(path) = path {
                out.push(Item::Path(path));
            } else if line.starts_with("@@") {
                // Codex anchors have no line counts from which to invent an unchanged gap.
                out.push(Item::Path(""));
            }
        } else {
            let sign = line.chars().next().unwrap_or(' ');
            if matches!(sign, '+' | '-' | ' ') {
                changes.push(Cell {
                    text: line.get(1..).unwrap_or(""),
                    number: None,
                    sign,
                });
            }
        }
    }
    out.extend(pair(&changes));
    out
}

/// Counts only changed lines from the call's own edit or patch, added first, removed second.
pub(super) fn counts(call: &Call<'_>) -> Option<(usize, usize)> {
    Some(totals(&items(call)?))
}
fn totals(items: &[Item<'_>]) -> (usize, usize) {
    items.iter().fold((0, 0), |(a, d), row| match row {
        Item::Lines(l, r) => (
            a + usize::from(r.is_some_and(|c| c.sign == '+')),
            d + usize::from(l.is_some_and(|c| c.sign == '-')),
        ),
        _ => (a, d),
    })
}

fn paint(
    b: &mut Buffer,
    r: Rect,
    cell: Option<Cell<'_>>,
    gutter: u16,
    top: usize,
    skip: usize,
) -> usize {
    let bg = match cell.map(|c| c.sign) {
        Some('+') => ADDED_BG,
        Some('-') => rgb(0x2a0907),
        _ => RECESS,
    };
    let mut count = 0;
    let columns = r.width.saturating_sub(gutter) as usize;
    visit_wrap(
        &[Span::raw(cell.map_or("", |c| c.text))],
        columns,
        |text, _, last| {
            if last && text.is_empty() && count > 0 {
                return true;
            }
            let n = count;
            count += 1;
            if top + n < skip || top + n >= skip + r.height as usize {
                return true;
            }
            let y = r.y + (top + n - skip) as u16;
            let cell = cell.filter(|_| n == 0);
            let number = cell
                .and_then(|c| c.number)
                .map_or(String::new(), |n| n.to_string());
            let sign = cell.map_or(' ', |c| c.sign);
            let ink = if sign == '+' { GREEN } else { RED };
            let mut spans = vec![
                Span::styled(
                    format!(" {number:>w$} ", w = gutter.saturating_sub(4) as usize),
                    style(GRAY, bg),
                ),
                Span::styled(format!("{sign} "), style(ink, bg)),
            ];
            spans.extend(to_line(text).spans);
            line(b, r, y, &Line::from(spans), style(DATA, bg));
            true
        },
    );
    count
}

// The first pass counts wrapped rows; the second paints only cells in the viewport.
fn body(b: &mut Buffer, r: Rect, items: &[Item<'_>], skip: usize, split: bool) -> usize {
    let widest = items
        .iter()
        .filter_map(|i| match i {
            Item::Lines(l, r) => l.iter().chain(r.iter()).filter_map(|c| c.number).max(),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    let gutter = (widest.to_string().len() as u16 + 4).max(5);
    let mut at = 0;
    let changed = |item: &Item<'_>| matches!(item, Item::Lines(l,r) if l.is_some_and(|c| c.sign == '-') || r.is_some_and(|c| c.sign == '+'));
    for group in items.chunk_by(|a, b| changed(a) && changed(b)) {
        if !split && changed(&group[0]) {
            for side in 0..2 {
                for item in group {
                    if let Item::Lines(l, right) = item
                        && let Some(cell) = [l, right][side]
                    {
                        at += paint(b, r, Some(*cell), gutter, at, skip);
                    }
                }
            }
            continue;
        }
        for item in group {
            match item {
                Item::Path(path) => {
                    if at >= skip && at < skip + r.height as usize {
                        put(
                            b,
                            r.x,
                            r.y + (at - skip) as u16,
                            path,
                            r.width,
                            bold(WHITE, RECESS),
                        );
                    }
                    at += 1;
                }
                Item::Lines(l, right) if split => {
                    let left_w = r.width.div_ceil(2);
                    let left = Rect::new(r.x, r.y, left_w, r.height);
                    let right_area = Rect::new(r.x + left_w, r.y, r.width - left_w, r.height);
                    let a = paint(b, left, *l, gutter, at, skip);
                    let z = paint(b, right_area, *right, gutter, at, skip);
                    at += a.max(z);
                }
                Item::Lines(l, right) => {
                    at += paint(b, r, l.or(*right), gutter, at, skip);
                }
            }
        }
    }
    at
}

/// Draws an edit or patch in the existing opened viewport; returns false for another call.
pub(super) fn draw(call: &Call<'_>, b: &mut Buffer, r: Rect, scroll: &mut usize) -> bool {
    let Some(items) = items(call) else {
        return false;
    };
    let (a, d) = totals(&items);
    // Beside each other once the card has two readable columns.
    let split = r.width > 108 && a > 0 && d > 0;
    let view = Rect::new(
        r.x + 9,
        r.y + 1,
        r.width.saturating_sub(14),
        r.height.saturating_sub(1),
    );
    let count_area = Rect { height: 0, ..view };
    let total = body(b, count_area, &items, usize::MAX, split) + 6;
    *scroll = (*scroll).min(total.saturating_sub(view.height as usize));
    fill(
        b,
        Rect::new(r.x + 7, r.y + 1, r.width.saturating_sub(11), view.height),
        style(DATA, RECESS),
    );
    let input = call.detail_input(r.width as usize, false);
    call.detail_head(b, r, *scroll, &input[..input.len().min(1)], false);
    if *scroll <= 5 && 5 - *scroll < view.height as usize {
        line(
            b,
            view,
            view.y + (5 - *scroll) as u16,
            &Line::from(vec![
                Span::styled(format!("+{a}"), style(GREEN, RECESS)),
                Span::raw("  "),
                Span::styled(format!("−{d}"), style(RED, RECESS)),
            ]),
            style(DATA, RECESS),
        );
    }
    let offset = 6usize.saturating_sub(*scroll);
    let drawn = Rect::new(
        view.x,
        view.y + offset.min(view.height as usize) as u16,
        view.width,
        view.height.saturating_sub(offset as u16),
    );
    body(b, drawn, &items, scroll.saturating_sub(6), split);
    scrollbar(
        b,
        r.right().saturating_sub(1),
        view.y,
        view.height as usize,
        total,
        *scroll,
    );
    true
}
