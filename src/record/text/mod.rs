//! Model-facing rebuild text, checked by the compiler rather than a runtime template parser.

mod input;
pub(crate) use input::{call_line, calls_left_out, heard, length};

use super::{ChildSummary, Summary};
use crate::protocol::{Body, End, EntryId, JobEvent, Millis, Outcome, Trigger, Usage};

/// Requests a continuation note with the reason the session is handing over.
pub(crate) fn handover(trigger: Trigger) -> &'static str {
    match trigger {
        Trigger::Idle => include_str!("idle.md"),
        Trigger::Change => include_str!("change.md"),
        _ => include_str!("threshold.md"),
    }
}

/// Ends an expired restart without answering any input or closing the session's jobs.
pub(crate) fn interrupted(gap: Millis) -> Body {
    let hours = gap / 3_600_000;
    let unit = if hours == 1 { "hour" } else { "hours" };
    Body::TurnEnd {
        inputs: Vec::new(),
        end: End::Error(format!(
            "Interrupted by a restart {hours} {unit} after its last activity; send any input to continue."
        )),
        usage: Usage::default(),
        context: None,
        window: None,
        threshold: None,
        closed: true,
        close_reason: None,
        report: None,
    }
}

pub(super) fn context(
    trigger: Trigger,
    note: &str,
    files: String,
    asks: Vec<String>,
    calls: Vec<String>,
    summary: &Summary,
) -> String {
    let trigger = match trigger {
        Trigger::Threshold => "the context reached its threshold",
        Trigger::Overflow => "the host rejected a request that exceeded its context window",
        Trigger::Handover => "you called handover",
        Trigger::Idle => "the session reached its configured idle handover time",
        Trigger::Restart => "the earlier conversation could not be continued",
        Trigger::Change => "the model or profile changed",
        Trigger::Night => "the resident's night began",
    };
    let note = if note.is_empty() {
        "There is no continuation note."
    } else {
        note
    };
    let jobs = summary.jobs.iter().filter_map(|e| match e {
        JobEvent::Started { job, pid, .. } => Some(job_line(*job, *pid)),
        _ => None,
    });
    let children = summary.children.iter().map(child);
    let meetings = summary.meetings.iter().filter_map(input::meeting);
    format!(
        include_str!("rebuild.md"),
        trigger = trigger,
        note = note,
        files = files,
        asks = lines(asks),
        calls = lines(calls),
        jobs = lines(jobs.collect()),
        children = lines(children.collect()),
        meetings = lines(meetings.collect())
    )
}

pub(super) fn files(changes: &str, more: usize) -> String {
    let more = if more == 0 {
        String::new()
    } else {
        format!("and {more} more\n")
    };
    format!(include_str!("files.md"), changes = changes, more = more)
}

fn lines(lines: Vec<String>) -> String {
    if lines.is_empty() {
        "None.".into()
    } else {
        lines.join("\n")
    }
}

pub(super) fn old_note(note: &str, minutes: u64) -> String {
    let age = match minutes {
        m if m < 120 => format!("{m} minutes"),
        m if m < 48 * 60 => format!("{} hours", m / 60),
        m => format!("{} days", m / 1440),
    };
    format!(
        "{note}\n\n(This note came from the previous rebuild, {age} ago; no newer note was written.)"
    )
}

pub(super) fn ask(id: EntryId, text: &str) -> String {
    format!("#{}: {text}", id.0)
}
pub(super) fn image(path: &std::path::Path) -> String {
    format!("Image: {}", path.display())
}
pub(super) fn earlier(ids: &[EntryId]) -> String {
    let ids = ids
        .iter()
        .map(|id| format!("#{}", id.0))
        .collect::<Vec<_>>()
        .join(", ");
    format!("Earlier asks, by entry id: {ids}.")
}
pub(super) fn earlier_rebuild(id: EntryId) -> String {
    format!(
        "Earlier asks are in rebuild entry #{}; recall it for their ids.",
        id.0
    )
}
pub(super) fn input(id: EntryId, from: &str, text: &str) -> String {
    format!("- #{} {from}: {text}", id.0)
}
fn job_line(job: crate::protocol::JobId, pid: u32) -> String {
    format!("- job-{}, pid {pid}", job.0)
}
fn child(child: &ChildSummary) -> String {
    let report = child.report.as_ref().map_or_else(
        || "none".into(),
        |r| format!("{:?}", r.status).to_lowercase(),
    );
    let settled = child
        .settled
        .map_or_else(|| "none".into(), |d| format!("{d:?}").to_lowercase());
    format!(
        "- {} (session {}): started; report: {report}; stopped: {}; settled: {settled}.",
        child.child.name, child.session, child.stopped
    )
}
pub(super) fn call(
    id: EntryId,
    name: &str,
    input: &str,
    result: Option<(Outcome, EntryId)>,
) -> String {
    let result = match result {
        Some((outcome, id)) => format!(
            "{}, result #{}",
            format!("{outcome:?}").to_lowercase(),
            id.0
        ),
        None => "no result yet".into(),
    };
    format!("- #{} {name} {input}: {result}", id.0)
}
