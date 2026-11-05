//! The person's past conversations, here and on each directly paired server: the phone's
//! search and every agent's `recall` beyond its own record.

use super::{Error, other, parse};
use crate::controller::Controller;
use crate::protocol::{BODY_BYTES, Body, Entry, EntryId, Found, Hit, Search};
use crate::record::REASONING;
use serde_json::Value;
use std::cmp::Reverse;

/// `record::search` here and, unless `local` or `resident` (a resident's conversations are on
/// its own server), on each directly paired server within 2 seconds; with `session`, only on
/// that conversation's server. Merged newest first, `limit` (20 unless given, at most 50) in
/// all, a paired server's sessions as `<id>@<server>`, and `partial` when one did not answer.
pub(crate) async fn search(c: &Controller, q: Search) -> Result<Found, Error> {
    let limit = q.limit.unwrap_or(20).clamp(1, 50);
    // With `session`, the name of the one server to ask, empty when it is this one.
    let only = q.session.as_ref().map(|s| s.address().map_or("", |a| a.1));
    let devices = c.devices();
    let servers: Vec<_> = devices
        .paired()
        .await
        .into_iter()
        .filter(|(d, _)| !q.local && q.resident.is_none() && only.is_none_or(|s| d.name == s))
        .collect();
    let mut asked = q.clone();
    (asked.local, asked.limit) = (true, Some(limit));
    asked.session = q.session.as_ref().and_then(|s| s.address()).map(|a| a.0);
    let there = servers
        .iter()
        .map(|(device, _)| devices.fetch(device, "/v1/search", &asked, BODY_BYTES));
    let there = futures_util::future::join_all(there);
    let (here, there) = tokio::join!(crate::record::search(c.home(), &q, limit), there);
    let words = "Search for 1 to 8 words, in at most 200 bytes.";
    let (mut hits, mut partial) = (here.ok_or_else(|| Error::Refused(words.into()))?, false);
    for ((device, _), answer) in servers.iter().zip(there) {
        partial |= answer.is_none();
        for mut hit in answer.into_iter().flat_map(|a: Found| a.hits) {
            hit.session = hit.session.on(&device.name);
            hits.push(hit);
        }
    }
    hits.sort_by_key(|h: &Hit| Reverse(h.at));
    hits.truncate(limit);
    Ok(Found { hits, partial })
}

/// `recall` with `session` or `query`: a search's hits, or with `session` and `id` that entry
/// whole, here or on its paired server, a thinking entry refused as a session's own is.
pub(super) async fn recall(c: &Controller, input: Value) -> Result<Value, Error> {
    let id = input["id"].as_u64().map(EntryId);
    let q: Search = parse(input)?;
    let (Some(session), Some(id), "") = (&q.session, id, q.q.as_str()) else {
        return serde_json::to_value(search(c, q).await?).map_err(other("answering a search"));
    };
    let missing = || Error::Refused(format!("{session} has no entry {} to recall.", id.0));
    let entry: Entry = match session.address() {
        None => serde_json::from_str(&c.get(session, id).await?).map_err(other("reading it"))?,
        Some((native, server)) => {
            let devices = c.devices();
            let server = devices.peer(server).await.ok_or_else(missing)?;
            let page = [("before", id.0 + 1), ("limit", 1)];
            let path = format!("/v1/sessions/{native}/entries?native=true");
            let entry = devices.fetch(&server, &path, &page, BODY_BYTES).await;
            entry.filter(|e: &Entry| e.id == id).ok_or_else(missing)?
        }
    };
    if matches!(entry.body, Body::Thinking { .. }) {
        return Err(Error::Refused(REASONING.replace("{id}", &id.0.to_string())));
    }
    serde_json::to_value(entry).map_err(other("answering a recall"))
}
