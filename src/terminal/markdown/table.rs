//! Table measurement and viewport rows share the outer parser’s link references.
use super::*;
use pulldown_cmark::BrokenLink;

pub(super) struct Table {
    pub(super) widths: Vec<usize>,
    head: bool,
    pub(super) hidden_height: usize,
}
impl Table {
    // Measures, then streams rows through the outer parser: no whole-table row tree, and the
    // outer parser's link references, never a copied index or a rescan of the whole reply.
    pub(super) fn new(
        source: &str,
        refs: &RefDefs<'_>,
        count: usize,
        columns: usize,
        out: &mut Lines,
    ) -> Self {
        let mut widths = vec![1; count.min(columns)];
        if count.saturating_mul(2).saturating_add(1) <= columns {
            let (mut column, mut used) = (0, 0);
            let known = |link: BrokenLink<'_>| {
                let found = refs.get(&link.reference)?;
                Some((
                    found.dest.clone(),
                    found.title.clone().unwrap_or_else(|| "".into()),
                ))
            };
            for event in Parser::new_with_broken_link_callback(source, OPTIONS, Some(known)) {
                match event {
                    Event::Text(text)
                    | Event::Code(text)
                    | Event::Html(text)
                    | Event::InlineHtml(text) => used += width(&text),
                    Event::End(TagEnd::TableCell) => {
                        if let Some(w) = widths.get_mut(column) {
                            *w = (*w).max(used).min(columns);
                        }
                        (column, used) = (column + 1, 0);
                    }
                    Event::End(TagEnd::TableHead | TagEnd::TableRow) => column = 0,
                    _ => {}
                }
            }
            let room = columns.saturating_sub(widths.len() + 1);
            fit(&mut widths, room);
            // The table spans the reply: spare columns are shared, the first taking the rest.
            let (n, spare) = (
                widths.len().max(1),
                room.saturating_sub(widths.iter().sum()),
            );
            for (i, w) in widths.iter_mut().enumerate() {
                *w += spare / n + usize::from(i < spare % n);
            }
        }
        let table = Self {
            widths,
            head: true,
            hidden_height: 0,
        };
        table.border("┌", "┬", "┐", out);
        table
    }
    pub(super) fn border(&self, left: &str, mid: &str, right: &str, out: &mut Lines) {
        if !self.widths.is_empty() {
            let rules: Vec<_> = self.widths.iter().map(|w| "─".repeat(*w)).collect();
            let rule = format!("{left}{}{right}", rules.join(mid));
            out.rendered(Rendered {
                words: Line::styled(rule, Style::default().fg(GRAY)),
                kind: Kind::Frame,
                ..Rendered::default()
            });
        }
    }
    // A rule between every two rows; the words touch the borders.
    pub(super) fn row(&mut self, row: &[Cell<'_>], out: &mut Lines) {
        if !std::mem::take(&mut self.head) {
            self.border("├", "┼", "┤", out);
        }
        let range =
            out.range.start.saturating_sub(out.total)..out.range.end.saturating_sub(out.total);
        let cells: Vec<_> = (0..self.widths.len())
            .map(|j| {
                let mut cell = Lines {
                    range: range.clone(),
                    ..Lines::default()
                };
                if let Some((spans, links)) = row.get(j) {
                    cell.linked(spans, self.widths[j], false, links);
                }
                cell
            })
            .collect();
        let height = cells.iter().map(|cell| cell.total).max().unwrap_or(0);
        for y in 0..height.max(std::mem::take(&mut self.hidden_height)) {
            if !out.wants() {
                out.total += 1;
                out.last_nonempty = true;
                continue;
            }
            let border = Style::default().fg(GRAY);
            let mut spans = vec![Span::styled("│", border)];
            let (mut links, mut column, mut bars) = (Vec::new(), 1, vec![0]);
            for (j, cell) in cells.iter().enumerate() {
                let line = y
                    .checked_sub(range.start)
                    .and_then(|at| cell.lines.get(at))
                    .cloned()
                    .unwrap_or_default();
                let used: usize = line.words.spans.iter().map(|s| width(&s.content)).sum();
                links.extend(line.links.into_iter().map(|link| Link {
                    columns: link.columns.start + column..link.columns.end + column,
                    url: link.url,
                }));
                column += self.widths[j].max(used) + 1;
                bars.push(column as u16 - 1);
                spans.extend(line.words.spans);
                let gap = " ".repeat(self.widths[j].saturating_sub(used));
                spans.push(Span::styled(format!("{gap}│"), border));
            }
            out.rendered(Rendered {
                words: Line::from(spans),
                links,
                kind: Kind::Cells(bars),
            });
        }
    }
}

// Columns too wide for `room` shrink in proportion, OpenTUI's fitter as the cockpit ported it:
// each keeps a cell and grows toward its natural width by the square root of what it lacks,
// water-filled to the room; the cells rounding leaves go to the columns that grew least for their
// need, and any rounding past the room comes off those that grew most. Columns that fit stay.
fn fit(widths: &mut [usize], room: usize) {
    let need: Vec<usize> = widths.iter().map(|w| w - 1).collect();
    let room = room.saturating_sub(widths.len()).min(need.iter().sum());
    let root = |i: usize| (need[i] as f64).sqrt();
    let mut order: Vec<usize> = (0..need.len()).filter(|&i| need[i] > 0).collect();
    order.sort_by_key(|&i| need[i]);
    let (mut left, mut weight) = (room as f64, order.iter().map(|&i| root(i)).sum::<f64>());
    let mut grow = vec![0; need.len()];
    for &i in &order {
        if left / weight <= root(i) {
            break;
        }
        (grow[i], left, weight) = (need[i], left - need[i] as f64, weight - root(i));
    }
    for &i in &order {
        if grow[i] < need[i] {
            grow[i] = need[i].min((left / weight * root(i)) as usize);
        }
    }
    // Which of two columns grew more for its need, `a` by `ga` cells and `b` by `gb`.
    let more =
        |a: usize, ga: usize, b: usize, gb: usize| (ga * ga * need[b]).cmp(&(gb * gb * need[a]));
    while grow.iter().sum::<usize>() > room {
        let most = (0..grow.len()).filter(|&i| grow[i] > 0);
        let Some(i) = most.max_by(|&a, &b| more(a, grow[a], b, grow[b])) else {
            break;
        };
        grow[i] -= 1;
    }
    while grow.iter().sum::<usize>() < room {
        let least = (0..grow.len()).filter(|&i| grow[i] < need[i]);
        let Some(i) = least.min_by(|&a, &b| more(a, grow[a] + 1, b, grow[b] + 1)) else {
            break;
        };
        grow[i] += 1;
    }
    for (w, g) in widths.iter_mut().zip(grow) {
        *w = 1 + g;
    }
}
