//! Device authorization, enrollment routes and phone notification settings.
use super::{App, Refusal, Reply, bad};
use crate::protocol::{Device, Follow, Notifications, Push, SessionId};
use crate::{config, devices::Devices};
use axum::{
    extract::{Extension, Json, Path, Request, State},
    http::{StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;
use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
};
const FOLLOWS: usize = 8;
const MUTED: usize = 256;

/// The check every request passes: its bearer key is the local key or an enrolled device's. The
/// request then carries that device's channel, which its streams watch.
pub(super) async fn authorize(
    State(devices): State<Arc<Devices>>,
    mut request: Request,
    next: Next,
) -> Response {
    if request.method() == axum::http::Method::GET && request.uri().path() == "/v1/mcp/callback" {
        return next.run(request).await;
    }
    let given = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default()
        .as_bytes();
    let Some((gone, name)) = devices.authorize(given).await else {
        let sentence = "This request needs a device key that works: Authorization: Bearer <key>.";
        return Refusal(StatusCode::UNAUTHORIZED, sentence.into()).into_response();
    };
    request.extensions_mut().insert(gone);
    request.extensions_mut().insert(Enrolled(name));
    next.run(request).await
}

/// The enrolled device whose key a request carried, by name; None for the local key.
#[derive(Clone)]
pub(super) struct Enrolled(pub(super) Option<String>);

/// The enrolled devices, without their keys and tokens.
pub(super) async fn list(State(app): State<App>) -> Json<Vec<Device>> {
    Json(app.devices.list().await)
}

/// A device to enroll: another server has an address, and its pairing's key when the other
/// side made it.
#[derive(Deserialize)]
pub(super) struct Name {
    name: String,
    address: Option<String>,
    key: Option<String>,
}

/// Answers the device with its key and, for one with no address, `pairing`: the line the phone
/// app pairs from, when `server.address` is a machine name.
pub(super) async fn enroll(
    State(app): State<App>,
    Json(Name { name, address, key }): Json<Name>,
) -> Reply {
    let device = app.devices.enroll(name, address, key).await?;
    let server = app.controller.config().await.server;
    let line = device
        .address
        .is_none()
        .then(|| pairing(&app.id, &server, device.key.as_deref().unwrap_or_default()))
        .flatten();
    let mut answer = json!(device);
    answer["pairing"] = json!(line);
    Ok(Json(answer))
}

/// `razor://pair?id=…&label=…&address=…&key=…`, its query form-encoded; None when
/// `server.address` is empty or an IP address, which the phone does not accept.
fn pairing(id: &str, server: &config::Server, key: &str) -> Option<String> {
    let address = server.address.as_deref().filter(|a| {
        !a.is_empty() && a.parse::<SocketAddr>().is_err() && a.parse::<IpAddr>().is_err()
    })?;
    let mut line = reqwest::Url::parse("razor://pair").ok()?;
    let label = super::label(server);
    let fields = [
        ("id", id),
        ("label", &label),
        ("address", address),
        ("key", key),
    ];
    line.query_pairs_mut().extend_pairs(fields);
    Some(line.into())
}

pub(super) async fn revoke(State(app): State<App>, Path(name): Path<String>) -> Reply {
    app.devices.revoke(&name).await?;
    Ok(Json(json!({})))
}

/// `PUT /devices/self/push`: the calling device's notification token. Answers `{}`, or
/// `{"unsent": "…"}` when this server cannot send to its environment yet; it is kept either way.
pub(super) async fn register(
    State(app): State<App>,
    Extension(device): Extension<Enrolled>,
    Json(push): Json<Push>,
) -> Reply {
    let (name, environment) = (phone(device, &push)?, push.environment);
    app.devices.change(&name, |d| d.push = Some(push)).await?;
    Ok(Json(super::push::unsent(&app, environment).await))
}

pub(super) async fn unregister(
    State(app): State<App>,
    Extension(Enrolled(name)): Extension<Enrolled>,
) -> Reply {
    // The local key names no device, so it changes nothing.
    let name = name.unwrap_or_default();
    app.devices.change(&name, |d| d.push = None).await?;
    Ok(Json(json!({})))
}

/// `GET /devices/self/notifications`: what the calling device's notifications bring.
pub(super) async fn notifications(
    State(app): State<App>,
    Extension(device): Extension<Enrolled>,
) -> Reply {
    let name = enrolled(device)?;
    Ok(Json(json!(app.devices.notifications(&name).await)))
}

/// `PUT /devices/self/notifications`: replaces them, answered with what is kept.
pub(super) async fn set_notifications(
    State(app): State<App>,
    Extension(device): Extension<Enrolled>,
    Json(set): Json<Notifications>,
) -> Reply {
    let name = enrolled(device)?;
    if set.muted.len() > MUTED {
        let sentence = format!("A device mutes at most {MUTED} sessions; unmute some first.");
        return Err(bad(sentence));
    }
    let kept = json!(set);
    app.devices
        .change(&name, |d| d.notifications = Some(set))
        .await?;
    Ok(Json(kept))
}

/// `PUT /sessions/{id}/follow`: a Live Activity's token for a top-level session that is not a
/// resident, answered as `register` is.
pub(super) async fn follow(
    State(app): State<App>,
    Extension(device): Extension<Enrolled>,
    Path(session): Path<SessionId>,
    Json(push): Json<Push>,
) -> Reply {
    let (name, environment) = (phone(device, &push)?, push.environment);
    let summary = app.controller.summary(&session).await?;
    if summary.child.is_some() || app.residents.timed(&summary).await {
        let sentence = "Only a top-level session that is not a resident is followed.";
        return Err(bad(sentence));
    }
    app.devices
        .change(&name, |d| {
            d.follows.retain(|f| f.session != session);
            d.follows.push(Follow { session, push });
            d.follows.drain(..d.follows.len().saturating_sub(FOLLOWS));
        })
        .await?;
    Ok(Json(super::push::unsent(&app, environment).await))
}

pub(super) async fn unfollow(
    State(app): State<App>,
    Extension(Enrolled(name)): Extension<Enrolled>,
    Path(session): Path<SessionId>,
) -> Reply {
    // The local key names no device, so it changes nothing.
    let name = name.unwrap_or_default();
    app.devices
        .change(&name, |d| d.follows.retain(|f| f.session != session))
        .await?;
    Ok(Json(json!({})))
}

/// The enrolled device a registration came from, once its token is one Apple could have made.
fn phone(device: Enrolled, push: &Push) -> Result<String, Refusal> {
    let name = enrolled(device)?;
    let hex = push.token.bytes().all(|b| b.is_ascii_hexdigit());
    // Apple's tokens are opaque and vary in length: a simulator's Live Activity token is 256.
    if push.token.is_empty() || push.token.len() > 1024 || !hex {
        return Err(bad("A push token is at most 1,024 hex digits."));
    }
    Ok(name)
}

/// The enrolled device a request came from; the local key names none.
fn enrolled(Enrolled(name): Enrolled) -> Result<String, Refusal> {
    let sentence =
        "Only an enrolled device registers for notifications; enroll it with razor devices add.";
    name.ok_or_else(|| bad(sentence))
}
