//! Copies through the terminal's OSC 52: text is encoded off the loop, and only the frame loop
//! writes the bytes, so no draw can split the sequence.
use super::{App, Composer, Error, error, fail};
use base64::Engine;
use ratatui::backend::CrosstermBackend;
use std::io::{Stdout, Write};

pub const MAX: usize = 72 << 10;

/// `/copy` the newest reply, `/copy ask` the shown question, `/copy code [n]` a fenced block of
/// the newest reply (the last without `n`).
pub fn command(app: &mut App, args: &str) {
    let mut words = args.split_whitespace();
    let (text, code, noun) = match (words.next(), words.next(), words.next()) {
        (None, ..) => (None, None, ""),
        (Some("ask"), None, None) => match &app.shown_ask {
            Some(ask) => (Some(ask.text.clone()), None, "the question, "),
            None => return app.say("There is no shown question to copy; no text was copied."),
        },
        (Some(word), None, None) if word.eq_ignore_ascii_case("code") => (None, Some(None), ""),
        (Some(word), Some(n), None) if word.eq_ignore_ascii_case("code") => {
            match n.parse::<usize>().ok().and_then(|n| n.checked_sub(1)) {
                Some(n) => (None, Some(Some(n)), ""),
                None => {
                    return app.say(
                        "Code blocks are numbered from 1; use /copy code for the last block.",
                    );
                }
            }
        }
        _ => return app.say("Use /copy, /copy ask or /copy code [n]; no text was copied."),
    };
    if app.copying {
        return app.say("A clipboard write is pending; wait before copying again.");
    }
    app.composer = Composer::default();
    app.edited();
    start(app, text, code, Some(noun));
}

/// Copies `text`, or the newest completed reply when there is none.
pub fn copy(app: &mut App, text: Option<String>) {
    start(app, text, None, Some(""));
}

/// Copies the editor's `text`, the toast counting its lines or characters as the cockpit's did.
pub fn draft(app: &mut App, text: String) {
    start(app, Some(text), None, None);
}

/// Copies the shown question, the toast naming it.
pub fn question(app: &mut App) {
    match app.shown_ask.as_ref().map(|ask| ask.text.clone()) {
        Some(text) => start(app, Some(text), None, Some("the question, ")),
        None => app.say("There is no shown question to copy; no text was copied."),
    }
}

// `noun` names a transcript copy in its toast (`the question, `); none is the editor's.
fn start(
    app: &mut App,
    text: Option<String>,
    code: Option<Option<usize>>,
    noun: Option<&'static str>,
) {
    if app.copying {
        return app.say("A clipboard write is pending; wait before copying again.");
    }
    let newest = || {
        app.transcript()?
            .entries
            .values()
            .rev()
            .find_map(|e| match &e.body {
                crate::protocol::Body::Text { text } | crate::protocol::Body::Thinking { text } => {
                    Some(text.clone())
                }
                _ => None,
            })
    };
    let Some(text) = text.or_else(newest) else {
        return app.say("There is no completed reply to copy yet; wait for its text.");
    };
    // One copy at a time, never cut: terminals cap one OSC 52 (xterm at about 100 KB of base64),
    // so past the cockpit's 72 KiB nothing goes, and an empty or blank copy says so. A phone's
    // toast holds about 38 characters: there a transcript copy says only the count.
    let wide = app.columns >= 72;
    app.copying = app.spawn(|_| async move {
        let encoded = tokio::task::spawn_blocking(move || {
            let text = match code {
                Some(index) => block(&text, index)?,
                None => text,
            };
            if text.trim().is_empty() {
                return Err(error("Nothing to copy."));
            }
            if text.len() > MAX {
                let kib = text.len().div_ceil(1024);
                return Err(error(format!(
                    "That is {kib} KiB; one copy carries at most 72 KiB, so nothing was copied."
                )));
            }
            let chars = unicode_segmentation::UnicodeSegmentation::graphemes(&*text, true).count();
            let lines = text.split('\n').count();
            let said = match noun {
                None if lines > 1 => format!("Copied {lines} lines"),
                None => format!("Copied {chars} characters"),
                Some(noun) if wide => {
                    format!("Copied {noun}{chars} chars to the terminal's clipboard (OSC 52)")
                }
                Some(noun) => format!("Copied {noun}{chars} chars"),
            };
            Ok((base64::engine::general_purpose::STANDARD.encode(text), said))
        });
        let encoded = encoded
            .await
            .map_err(fail(
                "The clipboard text could not be encoded; copy it again.",
            ))
            .and_then(|e| e);
        move |app: &mut App| match encoded {
            Ok(copy) => app.clipboard = Some(copy),
            Err(e) => {
                app.copying = false;
                app.say(&e.to_string());
            }
        }
    });
}

// The `index`th fenced block of `text`, or its last.
fn block(text: &str, index: Option<usize>) -> Result<String, Error> {
    use pulldown_cmark::{Event, Parser, Tag, TagEnd};
    let (mut count, mut inside, mut body) = (0, false, None::<String>);
    for event in Parser::new(text) {
        match event {
            Event::Start(Tag::CodeBlock(_)) => {
                inside = true;
                if index.is_none_or(|n| n == count) {
                    body = Some(String::new());
                }
            }
            Event::Text(text) if inside => body.iter_mut().for_each(|body| body.push_str(&text)),
            Event::End(TagEnd::CodeBlock) if index == Some(count) => break,
            Event::End(TagEnd::CodeBlock) => (inside, count) = (false, count + 1),
            _ => {}
        }
    }
    body.ok_or_else(|| {
        error("The newest reply has no such code block; use /copy for the whole reply.")
    })
}

/// Writes an encoded copy, if one is ready, between frames.
pub fn write(app: &mut App, output: &mut CrosstermBackend<Stdout>) -> Result<(), Error> {
    let Some((encoded, said)) = app.clipboard.take() else {
        return Ok(());
    };
    let written = (|| {
        output.write_all(b"\x1b]52;c;")?;
        output.write_all(encoded.as_bytes())?;
        output.write_all(b"\x07")?;
        output.flush()
    })();
    app.copying = false;
    written.map_err(Error::Terminal)?;
    app.say(&said);
    Ok(())
}
