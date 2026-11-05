//! How a recorded input, and a live call's or a meeting's fact, reads to the model: the words
//! of `input.toml` (RULES.md 16), drawn when a request or a rebuilt context is rendered.

use crate::protocol::{BridgeInput, CallEvent, CallReason, EntryId, Input, MailKind, Origin};
use crate::protocol::{Source, Speaker};
use crate::record::{Meeting, MeetingState, Summary};
use serde_json::json;
use std::{collections::BTreeMap, path::Path, sync::LazyLock};

/// The most characters of a mention the model reads; the channel file holds the rest.
const MENTION: usize = 2_000;

static WORDS: LazyLock<BTreeMap<String, String>> = LazyLock::new(|| {
    // Invariant: the file is compiled into the binary, so it parses in every run of a build or
    // in none, and the first input of any flow shows which.
    toml::from_str(include_str!("input.toml")).expect("src/record/text/input.toml parses")
});

/// One of `input.toml`'s texts.
fn words(key: &str) -> &'static str {
    WORDS.get(key).map_or("", String::as_str)
}

/// The input `entry` as a model path takes it in `me`'s session, whose folder is `dir`. Its
/// words: mail with a sender gets a one-line header naming whose child, parent or channel post
/// it is, and a question's id, and a mention shows its first 2,000 characters; a child's report
/// gets one naming the child and its status; a program's, schedule's, heartbeat's or Razor's
/// input gets a line naming it, not the person; the person's words, and a brief, are their
/// text. A line after them names each attached file by its absolute path, so a rebuilt context
/// keeps it. A voice note's words follow a line saying a machine heard them, and a video's a
/// line with its path and length, naming its watcher when it has one. Its images and video are
/// made absolute; the video goes to the model path only with no watcher, kept for the
/// conversation's life when it came in mail, which only a watcher's first input does.
pub(crate) fn heard(me: &Summary, dir: &Path, entry: EntryId, mut input: Input) -> BridgeInput {
    let mut text = std::mem::take(&mut input.text);
    if input.voice.is_some() {
        text.insert_str(0, words("voice"));
    }
    if input.voice_call.is_some() {
        text.insert_str(0, words("call"));
    }
    let video = input.video.take().and_then(|video| {
        let (path, name) = (dir.join(&video.file), video.watcher.as_deref());
        let line = words(["video", "watching"][usize::from(name.is_some())])
            .replace("{path}", &path.to_string_lossy())
            .replace("{duration}", &length(video.duration_ms))
            .replace("{name}", name.unwrap_or_default());
        text.insert_str(0, &line);
        name.is_none().then_some(path)
    });
    let mut text = header(me, &input.from, text);
    for file in &input.files {
        let path = dir.join(file);
        text += &words("attached").replace("{path}", &path.to_string_lossy());
    }
    for image in &mut input.images {
        image.path = dir.join(&image.path);
    }
    BridgeInput {
        entry,
        text,
        images: input.images,
        video,
        keep: matches!(input.from, Origin::Mail { .. }),
    }
}

/// One fact of a live call or a meeting as the model reads it; a call's start says the call is
/// running only when it is `current`, while a meeting runs until its end fact, whoever restarted.
pub(crate) fn call_line(source: Source, event: &CallEvent, current: bool) -> String {
    let meeting = source == Source::Meeting;
    match event {
        CallEvent::Started { folder, .. } if meeting => {
            meeting_words("meeting_started", folder.as_deref().unwrap_or_default())
        }
        CallEvent::Started { .. } => {
            words(["call_began", "call_started"][usize::from(current)]).into()
        }
        CallEvent::Line {
            name,
            text,
            start_ms,
            ..
        } if meeting => words("meeting_line")
            .replace("{at}", &length(*start_ms))
            .replace("{name}", name.as_deref().unwrap_or_default())
            .replace("{text}", text),
        CallEvent::Line { who, text, .. } => {
            words(["call_voice", "call_person"][usize::from(*who == Speaker::Person)])
                .replace("{text}", text)
        }
        CallEvent::Named { label, name } => words("meeting_named")
            .replace("{label}", label)
            .replace("{name}", name),
        CallEvent::Ended {
            reason: CallReason::Canceled,
        } if meeting => words("meeting_canceled").into(),
        CallEvent::Ended { .. } => {
            words(["call_ended", "meeting_ended"][usize::from(meeting)]).into()
        }
    }
}

/// The line that says how many earlier lines the model is not shown: a meeting's, given the
/// folder meetrec made for it, else a call's.
pub(crate) fn calls_left_out(count: usize, folder: Option<&str>) -> String {
    let text = match folder {
        Some(folder) => meeting_words("meeting_left_out", folder),
        None => words("call_left_out").into(),
    };
    text.replace("{count}", &count.to_string())
}

/// What a rebuilt context says of a meeting not yet closed: a recording one's start, or a
/// stopped one's end and the command that reads its transcript.
pub(super) fn meeting(m: &Meeting) -> Option<String> {
    let text = match m.state {
        MeetingState::Recording => meeting_words("meeting_started", &m.folder),
        MeetingState::Stopped => {
            words("meeting_ended").to_owned() + &meeting_words("meeting_read", &m.folder)
        }
        MeetingState::Closed => return None,
    };
    Some(text.trim_end().into())
}

/// A meeting's words with its full folder path quoted for the shell.
fn meeting_words(key: &str, folder: &str) -> String {
    words(key).replace("{folder}", &format!("'{}'", folder.replace('\'', "'\\''")))
}

/// A video's length as `M:SS`.
pub(crate) fn length(duration_ms: u64) -> String {
    let seconds = duration_ms / 1000;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

fn header(me: &Summary, from: &Origin, text: String) -> String {
    let (id, kind, from) = match from {
        Origin::Mail { id, kind, from } => (id, kind, from),
        Origin::Report { child, status } => {
            let row = me.children.iter().find(|c| c.session == *child);
            let name = row.map_or(child.as_str(), |c| c.child.name.as_str());
            let status = json!(status);
            let header = words("report").replace("{status}", status.as_str().unwrap_or_default());
            return header.replace("{from}", name) + &text;
        }
        Origin::Program { name } => return words("program").replace("{from}", name) + &text,
        Origin::Schedule { name, .. } => return words("schedule").replace("{from}", name) + &text,
        Origin::Heartbeat => return words("heartbeat").to_owned() + &text,
        Origin::Razor => return words("razor").to_owned() + &text,
        Origin::Person | Origin::Answer { .. } => return text,
    };
    if from.is_empty() {
        return text;
    }
    if *kind == MailKind::Mention {
        let shown = match text.char_indices().nth(MENTION) {
            Some((at, _)) => format!("{}{}", &text[..at], words("cut")),
            None => text,
        };
        return words("mention").replace("{from}", from) + &shown;
    }
    let who = if me.child.as_ref().is_some_and(|c| c.parent == id.0) {
        words("parent").replace("{from}", from)
    } else if me.children.iter().any(|c| c.session == id.0) {
        words("child").replace("{from}", from)
    } else {
        from.clone()
    };
    let header = match kind {
        MailKind::Question => words("question").replace("{question}", &json!(id).to_string()),
        MailKind::Answer { question } => {
            words("answer").replace("{entry}", &question.1.0.to_string())
        }
        _ => words("note").to_string(),
    };
    // The sender's words go in last, so nothing in them is read as a field.
    header.replace("{who}", &who) + &text
}
