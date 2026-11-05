//! Rebuilds read only their asks, continuation note and settled calls through the index.

use super::{Error, Record, io, text};
use crate::protocol::{Body, BridgeInput, Entry, EntryId, Input, Trigger, now};
use std::collections::{BTreeMap, HashMap};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, process::Command};

impl Record {
    /// Carry the latest four person/parent inputs at or before cut verbatim, earlier ids,
    /// calls since the latest carried ask/rebuild, every other unanswered input at or before
    /// cut, live jobs, the parent's recorded child facts, meetings not yet closed, disk changes
    /// and the continuation note.
    /// A handover's own note, when it has one; otherwise the last recorded one, with its age.
    /// Inputs `held`, not yet sent to the model, are left out wherever they sit. Beside the note
    /// and the context, each input at or before cut whose video is still unanswered or kept for
    /// life (a watcher's own), wherever it sits, so the rebuilt request can show it: with its
    /// words, unless the context already carries them verbatim.
    pub(crate) async fn rebuild(
        &self,
        trigger: Trigger,
        note: Option<String>,
        cut: EntryId,
        held: &[EntryId],
    ) -> Result<(String, String, Vec<BridgeInput>), Error> {
        let (note, shown) = self.note(note).await?;
        let mut asks: Vec<_> = self
            .summary
            .asks
            .iter()
            .chain(&self.summary.parent_inputs)
            .copied()
            .filter(|id| *id <= cut && !held.contains(id))
            .collect();
        asks.sort_unstable();
        let split = asks.len().saturating_sub(4);
        let mut lines = Vec::new();
        if let Some((id, _)) = self.summary.last_rebuild {
            lines.push(text::earlier_rebuild(id));
        }
        if split > 0 {
            lines.push(text::earlier(&asks[..split]));
        }
        let (me, dir) = (&self.summary, &self.dir);
        for id in &asks[split..] {
            if let Body::Input(input) = self.entry(*id).await?.body {
                let input = text::heard(me, dir, *id, input);
                lines.push(text::ask(*id, &input.text));
                lines.extend(input.images.iter().map(|image| text::image(&image.path)));
            }
        }
        let mut videos = Vec::new();
        for (id, _) in me
            .videos
            .iter()
            .filter(|(id, _)| *id <= cut && !held.contains(id))
        {
            if let Body::Input(input) = self.entry(*id).await?.body {
                let mut input = text::heard(me, dir, *id, input);
                if asks[split..].contains(id) {
                    (input.text, input.images) = (String::new(), Vec::new());
                }
                videos.push(input);
            }
        }
        let from = asks
            .last()
            .copied()
            .unwrap_or(EntryId(1))
            .max(self.summary.last_rebuild.map_or(EntryId(1), |(id, _)| id));
        let calls = self.calls_since(from, cut, held).await?;
        Ok((
            note,
            text::context(
                trigger,
                &shown,
                changed_files(&self.summary.cwd).await,
                lines,
                calls,
                me,
            ),
            videos,
        ))
    }

    /// The continuation note: the handover's, else the last rebuild's.
    async fn note(&self, note: Option<String>) -> Result<(String, String), Error> {
        if let Some(note) = note.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
            return Ok((note.into(), note.into()));
        }
        if let Some((id, at)) = self.summary.last_rebuild
            && let Body::Rebuild { note, .. } = self.entry(id).await?.body
            && !note.is_empty()
        {
            let shown = text::old_note(&note, now().saturating_sub(at) / 60_000);
            return Ok((note, shown));
        }
        Ok((String::new(), String::new()))
    }

    async fn calls_since(
        &self,
        from: EntryId,
        cut: EntryId,
        held: &[EntryId],
    ) -> Result<Vec<String>, Error> {
        let mut replay = self.replay(EntryId(from.0 - 1)).await?;
        // Only the calls and admitted inputs since the latest ask/rebuild, not the full log.
        let mut calls = HashMap::new();
        let mut lines = BTreeMap::new();
        while let Some(line) = replay.next_line().await {
            let entry: Entry = serde_json::from_str(&line?)
                .map_err(|e| self.damaged(EntryId(0), e.to_string()))?;
            match entry.body {
                Body::Call { call, name, input } => {
                    calls.insert(
                        call,
                        (
                            entry.id,
                            name,
                            input.to_string().chars().take(120).collect::<String>(),
                        ),
                    );
                }
                Body::Result { call, outcome, .. } => {
                    if let Some((id, name, input)) = calls.remove(&call) {
                        lines.insert(id, text::call(id, &name, &input, Some((outcome, entry.id))));
                    }
                }
                Body::Input(input)
                    if entry.id <= cut
                        && !held.contains(&entry.id)
                        && !input.from.is_person()
                        && input.trigger.is_none()
                        && !self.summary.parent_inputs.contains(&entry.id) =>
                {
                    lines.insert(entry.id, rendered(entry.id, input)?);
                }
                Body::Withdrawn { inputs } => {
                    for id in inputs {
                        lines.remove(&id);
                    }
                }
                _ => {}
            }
        }
        for (id, name, input) in calls.into_values() {
            lines.insert(id, text::call(id, &name, &input, None));
        }
        // The rebuilt turn answers every unanswered input at or before the cut, so one from
        // before the latest ask is shown too, read through the index.
        let s = &self.summary;
        let asks = |id: &&EntryId| s.asks.contains(id) || s.parent_inputs.contains(id);
        let older = s
            .unanswered
            .iter()
            .filter(|id| **id < from && !held.contains(id));
        for &id in older.filter(|id| !asks(id)) {
            if let Body::Input(input) = self.entry(id).await?.body
                && !input.from.is_person()
                && input.trigger.is_none()
            {
                lines.insert(id, rendered(id, input)?);
            }
        }
        Ok(lines.into_values().collect())
    }
}

async fn changed_files(cwd: &Path) -> String {
    let status = async {
        let mut child = Command::new("git")
            .args([
                "--no-optional-locks",
                "-c",
                "core.quotePath=true",
                "status",
                "--porcelain",
            ])
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .ok()?;
        let mut stdout = child.stdout.take()?;
        let mut buffer = [0; 8192];
        let mut changes = Vec::new();
        let mut entries = 0usize;
        loop {
            let n = stdout.read(&mut buffer).await.ok()?;
            if n == 0 {
                break;
            }
            for &byte in &buffer[..n] {
                if entries < 50 {
                    // Bound retained paths independently of the repository's size.
                    if changes.len() == 64 * 1024 {
                        return None;
                    }
                    changes.push(byte);
                }
                if byte == b'\n' {
                    entries += 1;
                }
            }
        }
        if !child.wait().await.ok()?.success() || entries == 0 {
            return None;
        }
        Some(text::files(
            &String::from_utf8(changes).ok()?,
            entries.saturating_sub(50),
        ))
    };
    tokio::time::timeout(Duration::from_secs(1), status)
        .await
        .ok()
        .flatten()
        .unwrap_or_default()
}

/// An input's line in the rebuilt context: its id, its origin and its text on one line.
fn rendered(id: EntryId, input: Input) -> Result<String, Error> {
    let from = serde_json::to_string(&input.from)
        .map_err(|e| io("rendering an input's origin")(std::io::Error::other(e)))?;
    let text = input.text.split_whitespace().collect::<Vec<_>>().join(" ");
    Ok(text::input(id, &from, &text))
}
