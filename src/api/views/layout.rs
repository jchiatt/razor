//! Resolves bounded snapshots without changing saved definitions.
use crate::protocol::{AgentView, CloseReason, Server, SessionId, State, views::*};
use std::collections::{BTreeMap, BTreeSet};

fn native(id: &SessionId) -> &str {
    id.as_str().split('@').next().unwrap_or_default()
}
fn rank(a: &AgentView) -> u8 {
    match a.state {
        _ if !a.asks.is_empty() || a.interrupted => 0,
        State::Stuck => 1,
        State::Working if !a.closing => 2,
        State::Done if a.close_reason != Some(CloseReason::Idle) => 4,
        // A closing turn and an idle close are invisible: the session keeps an idle one's place.
        _ => 3,
    }
}
/// Whether a resident's session stays under its home: all but a thread the person closed, which
/// leaves for Earlier as a closed root does (Trey, 2026-11-04: threads "don't go away when closed").
fn kept(a: &AgentView) -> bool {
    rank(a) < 4 || a.person_input_at.is_none()
}
fn sort(rows: &mut [&AgentView], by: Sort, order: &[SessionId]) {
    rows.sort_by_key(|a| (native(&a.session), &a.session));
    match by {
        Sort::Attention => rows.sort_by_key(|a| rank(a)),
        Sort::Recent => rows.sort_by_key(|a| std::cmp::Reverse(a.last_activity)),
        Sort::Created => {}
        Sort::Name => rows.sort_by_cached_key(|a| a.name.to_lowercase()),
        Sort::Mine => rows.sort_by_cached_key(|a| {
            order
                .iter()
                .position(|id| native(id) == native(&a.session))
                .unwrap_or(usize::MAX)
        }),
    }
}
fn section(id: &str, kind: SectionKind, name: &str) -> Section {
    Section {
        id: id.into(),
        kind,
        name: name.into(),
        ..Default::default()
    }
}
fn fill(
    section: &mut Section,
    mut rows: Vec<&AgentView>,
    order: (Sort, &[SessionId]),
    children: &BTreeMap<&SessionId, Vec<&AgentView>>,
    live: &BTreeSet<SessionId>,
) {
    sort(&mut rows, order.0, order.1);
    // A resident's helpers show while live, its open threads once the person speaks in them.
    let residents = section.kind == SectionKind::Residents;
    let shown = |a: &AgentView| {
        !residents
            || a.resident.is_some() && a.person_input_at.is_some()
            || a.resident.is_none() && live.contains(&a.session)
    };
    // At most one entry per row in the bounded snapshot; no call-stack limit on child depth.
    let mut pending: Vec<_> = rows.into_iter().rev().map(|a| (a, 0, false)).collect();
    while let Some((a, depth, folded)) = pending.pop() {
        let visible = depth == 0 || shown(a);
        if visible {
            section.sessions.push(ViewRow {
                session: a.session.clone(),
                depth,
                folded,
            });
        }
        if !visible && a.resident.is_some() {
            continue;
        }
        if section.kind != SectionKind::NeedsYou
            && let Some(rows) = children.get(&a.session)
        {
            // Finished children past the newest three fold, after their siblings.
            let mut kids: Vec<_> = (rows.iter().rev())
                .scan(0, |done, c| {
                    *done += usize::from(rank(c) == 4 && shown(c));
                    Some((*c, depth + u16::from(visible), rank(c) == 4 && *done > 3))
                })
                .collect();
            // Popped last first: a resident's most recently active kids first.
            kids.sort_by_key(|k| (!k.2, residents.then_some(k.0.last_activity)));
            pending.extend(kids);
        }
    }
}
/// Roots are matched by specificity; descendants follow them and have their own attention shortcuts.
pub(super) fn resolve(
    saved: &Views,
    agents: &[AgentView],
    servers: &[Server],
    unread: &BTreeSet<SessionId>,
    now: u64,
) -> Vec<Layout> {
    // Homes and threads share the same display tree; recorded parentage stays unchanged.
    let rows: BTreeMap<_, _> = (agents.iter())
        .filter(|a| {
            a.settled.is_none()
                && !(a.parent.is_some() && a.state == State::Done && a.report.is_none())
                && !a.report.as_ref().is_some_and(|r| {
                    r.status == crate::protocol::ReportStatus::Finished && r.no_changes
                })
        })
        .map(|a| (&a.session, a))
        .collect();
    let homes: BTreeSet<_> = (servers.iter())
        .flat_map(|s| s.residents.iter().map(|r| r.session.on(&s.name)))
        .collect();
    let live: BTreeSet<_> = rows
        .values()
        .filter(|a| {
            rank(a) < 3
                || rank(a) == 3
                    && (unread.contains(&a.session)
                        || a.terminals > 0
                        || a.looked_at.is_some_and(|at| a.spoke_at > Some(at))
                        || a.person_input_at
                            .is_some_and(|at| now.saturating_sub(at) < 43_200_000))
        })
        .map(|a| a.session.clone())
        .collect();
    let mut children = BTreeMap::<_, Vec<_>>::new();
    for a in rows.values() {
        let parent = a.parent.clone().or_else(|| {
            (a.resident.as_ref().filter(|_| kept(a)))
                .map(|r| r.on(a.session.address().map_or("", |(_, server)| server)))
        });
        if let Some(p) = parent.and_then(|p| rows.get_key_value(&p).map(|(id, _)| *id)) {
            children.entry(p).or_default().push(*a);
        }
    }
    let fallback = View {
        name: "Default".into(),
        everything_else: Arrangement {
            by: By::State,
            ..Default::default()
        },
        ..Default::default()
    };
    let views = if saved.views.is_empty() {
        std::slice::from_ref(&fallback)
    } else {
        &saved.views
    };
    (views.iter())
        .map(|view| layout(view, saved, &rows, &children, servers, &homes, &live))
        .collect()
}
fn matched(groups: &[(&Group, u16)], a: &AgentView) -> Option<usize> {
    let mut best = None;
    for (i, (g, _)) in groups.iter().enumerate() {
        for m in &g.matches {
            let score = match m {
                Match::Session(session) if native(session) == native(&a.session) => 3,
                Match::Folder(folder) if folder == &a.project => 2,
                Match::Repo(repo) if Some(repo) == a.repo.as_ref() => 1,
                _ => 0,
            };
            if score > best.map_or(0, |(_, score)| score) {
                best = Some((i, score));
            }
        }
    }
    best.map(|(i, _)| i)
}
fn layout(
    view: &View,
    saved: &Views,
    rows: &BTreeMap<&SessionId, &AgentView>,
    children: &BTreeMap<&SessionId, Vec<&AgentView>>,
    servers: &[Server],
    homes: &BTreeSet<SessionId>,
    live: &BTreeSet<SessionId>,
) -> Layout {
    let mut groups = Vec::new();
    let mut pending: Vec<_> = view.groups.iter().rev().map(|g| (g, 0)).collect();
    while let Some((g, depth)) = pending.pop() {
        groups.push((g, depth));
        pending.extend(g.groups.iter().rev().map(|g| (g, depth + 1)));
    }
    // Built-ins precede groups; unmatched sections follow them, with Earlier last.
    let mut placed = BTreeMap::<_, Vec<_>>::new();
    for a in rows.values() {
        if homes.contains(&a.session) {
            placed.entry((0, 0, String::new())).or_default().push(*a);
            continue;
        }
        let name = a.session.address().map_or("", |(_, name)| name);
        let host = servers.iter().find(|s| s.name == name);
        let state = ["asking", "stuck", "working", "needs_you", "done"][rank(a) as usize];
        if rank(a) > 1 && view.hidden.iter().any(|s| s == state)
            || host.is_some_and(|s| view.hidden.contains(&s.id))
        {
            continue;
        }
        if rank(a) < 2 {
            placed.entry((1, 0, String::new())).or_default().push(*a);
        }
        if a.resident.is_some() && kept(a)
            || a.parent.as_ref().is_some_and(|p| rows.contains_key(p))
        {
            continue;
        }
        let index = if !live.contains(&a.session) {
            groups.len() + 4
        } else if (saved.pinned.iter()).any(|pin| native(pin) == native(&a.session)) {
            2
        } else {
            matched(&groups, a).map_or(groups.len() + 3, |i| i + 3)
        };
        let key = (index == groups.len() + 3).then(|| other(a, view.everything_else.by, host));
        let (order, key) = key.unwrap_or_default();
        placed.entry((index, order, key)).or_default().push(*a);
    }
    // A group shows while it or one of its subgroups holds an agent (Trey, 2026-11-02).
    let mut ancestors = Vec::new();
    for (i, (_, depth)) in (3..).zip(&groups) {
        ancestors.truncate(*depth as usize);
        if placed.contains_key(&(i, 0, String::new())) {
            for parent in &ancestors {
                placed.entry((*parent, 0, String::new())).or_default();
            }
        }
        ancestors.push(i);
    }
    Layout {
        name: view.name.clone(),
        sections: sections(placed, &groups, saved, view, children, live),
    }
}
fn sections(
    placed: BTreeMap<(usize, u8, String), Vec<&AgentView>>,
    groups: &[(&Group, u16)],
    saved: &Views,
    view: &View,
    children: &BTreeMap<&SessionId, Vec<&AgentView>>,
    live: &BTreeSet<SessionId>,
) -> Vec<Section> {
    placed
        .into_iter()
        .map(|((i, _, key), rows)| {
            let mut s = match i {
                0 => section("residents", SectionKind::Residents, "Residents"),
                1 => section("needs_you", SectionKind::NeedsYou, "NEEDS YOU"),
                2 => section("pinned", SectionKind::Pinned, "PINNED"),
                i if i < groups.len() + 3 => {
                    let (g, depth) = groups[i - 3];
                    let mut s = section(&format!("group/{}", g.id), SectionKind::Group, &g.name);
                    s.colour = Some(g.colour);
                    s.why = g.why.clone();
                    s.depth = depth;
                    s
                }
                i if i == groups.len() + 3 => section(
                    &format!("everything_else/{:?}/{key}", view.everything_else.by),
                    SectionKind::EverythingElse,
                    &key,
                ),
                _ => section("earlier", SectionKind::Earlier, "Earlier"),
            };
            let (by, order) = match s.kind {
                SectionKind::Residents => (Sort::Name, &[][..]),
                SectionKind::NeedsYou => (Sort::Attention, &[][..]),
                SectionKind::Pinned => (Sort::Mine, &saved.pinned[..]),
                SectionKind::Group => (groups[i - 3].0.sort, &groups[i - 3].0.order[..]),
                SectionKind::EverythingElse => {
                    (view.everything_else.sort, &view.everything_else.order[..])
                }
                SectionKind::Earlier => (Sort::Created, &[][..]),
            };
            fill(&mut s, rows, (by, order), children, live);
            s
        })
        .collect()
}
fn other(a: &AgentView, by: By, host: Option<&Server>) -> (u8, String) {
    let key = match by {
        By::None => "Everything else".into(),
        By::Repository => a.repo.clone().unwrap_or_else(|| "No repository".into()),
        By::Project => a.project.clone(),
        By::Machine => host
            .map_or("Unknown machine", |s| s.machine.as_str())
            .into(),
        By::Realm => host.map_or("Unknown realm", |s| s.realm.as_str()).into(),
        By::State => ["ASKING", "STUCK", "WORKING", "IDLE", "DONE"][rank(a) as usize].into(),
    };
    (u8::from(by == By::State) * rank(a), key)
}
