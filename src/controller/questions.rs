//! The person's questions MCP asks: a server's, and a sign-in's, recorded in the session.

use super::{Controller, Error};
use crate::protocol::{Ask, Body, EntryId, Event, Input, Origin, SessionId};
use crate::tools::Questions;
use std::sync::Arc;

/// A session's server questions take the same recorded person-question path as sign-in.
pub(super) fn questions(c: Controller, id: SessionId) -> Questions {
    Questions(Arc::new(move |text, options| {
        let (c, id) = (c.clone(), id.clone());
        Box::pin(async move {
            let asked = question(&c, &id, text, options).await;
            asked.map_err(|e| crate::tools::Error::Refused(e.to_string()))
        })
    }))
}

/// Records a question; residents hear of it as of any ask.
pub(super) async fn ask(
    c: &Controller,
    id: &SessionId,
    text: String,
    options: Vec<String>,
    redirect: bool,
) -> Result<EntryId, Error> {
    let body = Body::Question {
        text: text.clone(),
        options: options.clone(),
        redirect,
    };
    let entry = c.append(id, body).await?;
    let ask = Ask {
        id: (id.clone(), entry),
        text,
        options,
        at: crate::protocol::now(),
    };
    crate::residents::ask(c, &c.config().await, &ask);
    Ok(entry)
}

/// Records a server's question and waits on the session's stream for its answer.
async fn question(
    c: &Controller,
    id: &SessionId,
    text: String,
    options: Vec<String>,
) -> Result<String, Error> {
    let entry = ask(c, id, text, options, false).await?;
    let mut lines = c.attach(id, entry, 0).await?;
    while let Some(line) = lines.recv().await {
        if let Ok(Event::Entry(e)) = serde_json::from_str(&line)
            && let Body::Input(Input {
                from: Origin::Answer { ask },
                text,
                ..
            }) = e.body
            && ask == entry
        {
            return Ok(text);
        }
    }
    Err(Error::Invalid(
        "The session closed before this question had an answer.".into(),
    ))
}
