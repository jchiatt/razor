//! A child's turn end, decided from recorded facts rather than its prose.

use super::{Error, TEXT, other, report};
use crate::{
    config::Settings,
    profiles,
    protocol::{Body, End, EntryId, Input, Origin, ReportStatus, SessionId},
    record::{Record, TEXT_BREAK, closing_text},
};
use std::{path::PathBuf, time::Instant};

/// Constant-space turn facts; closing text stays only in the record during the grace.
#[derive(Clone, Copy, Default)]
pub(crate) struct Turn {
    sent: bool,
    grace: Option<Instant>,
    nudged: bool,
    text: Option<(EntryId, EntryId)>,
}

impl Turn {
    /// A new turn clears its facts, but only a new task clears the admitted nudge.
    pub(crate) fn start(&mut self) {
        self.sent = false;
        self.grace = None;
        self.text = None;
    }

    /// Marks Razor's nudge only after its input has passed admission.
    pub(crate) fn nudge(&mut self) {
        self.nudged = true;
    }

    /// Remembers only committed mail and the first and last closing Text ids.
    pub(crate) fn recorded(&mut self, body: &Body, at: EntryId) {
        match body {
            Body::MailSent { .. } => self.sent = true,
            Body::Text { .. } => self.text = Some((self.text.map_or(at, |(first, _)| first), at)),
            Body::Call { .. } => self.text = None,
            _ => {}
        }
    }

    /// At a child's turn end, `held` (pending work, or a child already closed), answered person
    /// inputs and a stop forbid a report. Otherwise an error prepares a `blocked` report at once:
    /// its sentence, any closing text, then the line saying a continue retries, cut to 64 KiB
    /// with that line kept. A clean end prepares the same report as finish, or starts the grace.
    pub(crate) async fn end(
        &mut self,
        record: &Record,
        inputs: &[EntryId],
        end: &End,
        held: bool,
    ) -> Result<Option<(SessionId, Input, Option<Vec<PathBuf>>)>, Error> {
        let summary = record.summary();
        let Some(child) = &summary.child else {
            return Ok(None);
        };
        if held || *end == End::Stopped {
            return Ok(None);
        }
        for id in inputs {
            if let Body::Input(i) = record
                .entry(*id)
                .await
                .map_err(other("reading turn inputs"))?
                .body
                && i.from.is_person()
            {
                return Ok(None);
            }
        }
        let (status, text) = match end {
            End::Error(why) => {
                let line = profiles::CHILD_ERROR.replace("{name}", &child.name);
                let room = TEXT.saturating_sub(line.len() + TEXT_BREAK.len());
                let why = head(why, room);
                let closing =
                    self.closing(record, room.saturating_sub(why.len() + TEXT_BREAK.len()));
                let mut parts = vec![why.to_owned(), closing.await?, line];
                parts.retain(|part| !part.is_empty());
                (ReportStatus::Blocked, closing_text(&parts))
            }
            _ if self.sent && !self.nudged => {
                self.grace = Some(Instant::now());
                return Ok(None);
            }
            _ => (ReportStatus::Finished, self.closing(record, TEXT).await?),
        };
        report(summary, status, text).await.map(Some)
    }

    /// The closing Text blocks, joined, in at most `room` bytes: a cut one ends with where the
    /// whole text is, and is empty when even that does not fit.
    async fn closing(&self, record: &Record, room: usize) -> Result<String, Error> {
        let Some((first, last)) = self.text else {
            return Ok(String::new());
        };
        // At most `room` bytes including separators: even empty blocks consume separator bytes.
        let (mut text, mut truncated) = (String::new(), false);
        for id in first.0..=last.0 {
            if let Body::Text { text: part } = record
                .entry(EntryId(id))
                .await
                .map_err(other("reading closing text"))?
                .body
            {
                if id != first.0 {
                    text.push_str(TEXT_BREAK);
                }
                let kept = head(&part, room.saturating_sub(text.len()));
                truncated = kept.len() < part.len() || text.len() > room;
                text.push_str(kept);
                if truncated {
                    break;
                }
            }
        }
        if truncated {
            let line = if first == last {
                profiles::CHILD_REPORT
            } else {
                profiles::CHILD_REPORT_RANGE
            };
            let line = line
                .replace("{session}", record.summary().session.as_str())
                .replace("{first}", &first.0.to_string())
                .replace("{last}", &last.0.to_string());
            text = match room.checked_sub(line.len()) {
                Some(keep) => head(&text, keep).to_owned() + &line,
                None => String::new(),
            };
        }
        Ok(text)
    }

    /// Holds an open child's idle task until its grace or admitted nudge has been resolved.
    pub(crate) fn waiting(&self) -> bool {
        self.grace.is_some() || self.nudged
    }

    /// Offers the nudge against the live setting without marking an unadmitted input.
    pub(crate) fn tick(&self, settings: &mut Settings) -> Option<Input> {
        let since = self.grace?;
        settings.read();
        (since.elapsed() >= settings.config.children.grace)
            .then(|| Input::new(Origin::Razor, profiles::CHILD_NUDGE.into()))
    }
}

/// The longest start of `text` within `max` bytes that ends on a character boundary.
fn head(text: &str, max: usize) -> &str {
    &text[..text.floor_char_boundary(max)]
}
