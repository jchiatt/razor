//! Directly paired homes: one bounded transient snapshot each, and one-hop session forwarding.
use super::*;
use crate::protocol::{AgentView, Device, now};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::sync::{RwLock, broadcast};
use tokio::task::JoinSet;
use tokio_util::io::StreamReader;

const BYTES: usize = 8 << 20;
pub(super) use crate::devices::HOP;

pub(super) struct Peers {
    rows: RwLock<BTreeMap<String, Peer>>,
    changed: broadcast::Sender<Live>,
}
struct Peer {
    server: Server,
    agents: BTreeMap<SessionId, AgentView>,
    deliveries: Vec<Delivery>,
    seen: u64,
    bytes: usize,
}
impl Peer {
    fn recount(&mut self) {
        // Reserve room for timestamp growth and collection separators between snapshots.
        self.bytes = bytes(&(&self.server, &self.agents, &self.deliveries)) + 128;
        self.bound();
    }
    fn bound(&mut self) {
        if self.bytes <= BYTES {
            return;
        }
        self.agents.clear();
        self.deliveries.clear();
        if bytes(&self.server) + 128 > BYTES {
            self.server = Server {
                name: std::mem::take(&mut self.server.name),
                ..Default::default()
            };
        }
        self.server.silent = Some(self.seen);
        self.bytes = bytes(&self.server) + 128;
    }
}

fn bytes(value: &impl Serialize) -> usize {
    serde_json::to_vec(value).map_or(BYTES + 1, |b| b.len())
}

impl Default for Peers {
    fn default() -> Self {
        Self {
            rows: RwLock::default(),
            changed: broadcast::channel(64).0,
        }
    }
}
impl Peers {
    pub(super) fn subscribe(&self) -> broadcast::Receiver<Live> {
        self.changed.subscribe()
    }

    pub(super) async fn extend(&self, agents: &mut Vec<AgentView>, local: Server) -> Vec<Server> {
        let mut servers = vec![local];
        for peer in self.rows.read().await.values() {
            if !peer.server.id.is_empty() && servers.iter().any(|s| s.id == peer.server.id) {
                continue;
            }
            servers.push(peer.server.clone());
            agents.extend(peer.agents.values().cloned().map(|mut row| {
                qualify(&mut row, &peer.server.name);
                row
            }));
        }
        servers
    }

    pub(super) async fn deliveries(
        &self,
        mut rows: Vec<Delivery>,
        limit: usize,
        words: &[String],
    ) -> Vec<Delivery> {
        for peer in self.rows.read().await.values() {
            rows.extend(peer.deliveries.iter().filter(|d| d.matches(words)).cloned());
        }
        rows.sort_by_key(|d| std::cmp::Reverse((d.at, d.session.clone(), d.entry)));
        rows.truncate(limit);
        rows
    }

    /// Direct enrollment and last reported definition version for the configured shared home.
    pub(super) async fn owner(&self, app: &App, id: &str) -> Option<(Device, Option<u64>)> {
        let rows = self.rows.read().await;
        let (name, peer) = rows.iter().find(|(_, p)| p.server.id == id)?;
        let (device, _) = app
            .devices
            .paired()
            .await
            .into_iter()
            .find(|(d, _)| &d.name == name)?;
        Some((device, peer.server.views_version))
    }

    async fn update(&self, name: &str, line: Option<Live>) {
        let mut rows = self.rows.write().await;
        let Some(peer) = rows.get_mut(name) else {
            return;
        };
        let was_silent = peer.server.silent.is_some();
        peer.server.silent = line.is_none().then_some(peer.seen);
        if line.is_some() {
            peer.seen = now();
        }
        let mut event = match line {
            Some(Live::Agents {
                mut server, agents, ..
            }) => {
                server.name = name.into();
                if server.machine.is_empty() {
                    server.machine = peer.server.machine.clone();
                }
                server.silent = None;
                peer.server = *server;
                peer.agents = agents.into_iter().map(|a| (a.session.clone(), a)).collect();
                peer.recount();
                None
            }
            Some(Live::Server(mut server)) => {
                server.name = name.into();
                if !was_silent && peer.server == *server {
                    return;
                }
                let mut old = peer.server.clone();
                old.load = server.load;
                old.views_version = server.views_version;
                old.residents.clone_from(&server.residents);
                let event = (!was_silent && old == *server).then(|| Live::Server(server.clone()));
                peer.bytes = peer.bytes.saturating_sub(bytes(&peer.server)) + bytes(&server);
                peer.server = *server;
                event
            }
            Some(Live::Agent(mut row)) => {
                let old = peer.agents.insert(row.session.clone(), *row.clone());
                peer.bytes += bytes(&row);
                peer.bytes = peer.bytes.saturating_sub(old.as_ref().map_or(0, bytes));
                if old.is_none() {
                    peer.bytes += bytes(&row.session) + 2;
                }
                qualify(&mut row, name);
                Some(Live::Agent(row))
            }
            Some(Live::Delivery(mut d)) => {
                d.session = d.session.on(name);
                peer.deliveries.retain(|old| {
                    let keep = old.session != d.session || old.entry != d.entry;
                    if !keep {
                        peer.bytes = peer.bytes.saturating_sub(bytes(old) + 1);
                    }
                    keep
                });
                peer.bytes += bytes(&d) + 1;
                peer.deliveries.push((*d).clone());
                peer.deliveries
                    .sort_by_key(|d| std::cmp::Reverse((d.at, d.session.clone(), d.entry)));
                let keep = peer.deliveries.len().min(256);
                let evicted: usize = peer.deliveries.drain(keep..).map(|d| bytes(&d) + 1).sum();
                peer.bytes = peer.bytes.saturating_sub(evicted);
                Some(Live::Delivery(d))
            }
            None if was_silent => return,
            Some(_) if !was_silent => return,
            _ => None,
        };
        peer.bound();
        if peer.server.silent.is_some() {
            event = None;
        }
        let _ = self
            .changed
            .send(event.unwrap_or_else(|| Live::Server(Box::default())));
    }
}

/// Runs beside serving; dropping the server drops the JoinSet and all peer readers.
pub(super) async fn run(app: App) {
    let mut tasks = JoinSet::new();
    let mut refresh = tokio::time::interval(Duration::from_secs(5));
    loop {
        tokio::select! {
            _ = refresh.tick() => {
                for (device,mut gone) in app.devices.paired().await {
                    let name = device.name.clone();
                    if app.peers.rows.read().await.contains_key(&name) { continue; }
                    let host = device.address.as_deref().unwrap_or_default().rsplit_once(':').map_or("",|a| a.0).to_owned();
                    let mut peer = Peer {
                        server: Server { name:name.clone(), machine:host, silent:Some(now()), ..Default::default() },
                        agents:BTreeMap::new(), deliveries:Vec::new(), seen:now(), bytes:0,
                    };
                    peer.recount();
                    app.peers.rows.write().await.insert(name.clone(), peer);
                    let running = app.clone();
                    tasks.spawn(async move {
                        tokio::select! { () = read(running.clone(),device) => {}, _ = gone.changed() => {} }
                        running.peers.rows.write().await.remove(&name);
                        let _ = running.peers.changed.send(Live::Server(Box::default()));
                    });
                    let _ = app.peers.changed.send(Live::Server(Box::default()));
                }
            }
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
        }
    }
}

async fn read(app: App, device: Device) {
    loop {
        let _: Option<()> = async {
            let request =
                app.devices
                    .request(&device, Method::GET, "/v1/status?local=true&heartbeat=true");
            let response = tokio::time::timeout(Duration::from_secs(2), request.send())
                .await
                .ok()?
                .ok()?
                .error_for_status()
                .ok()?;
            // Subscribe before loading the page, closing the delivery race.
            let page = "/v1/deliveries?local=true&limit=256";
            if let Some(mut deliveries) = app
                .devices
                .fetch::<Vec<Delivery>>(&device, page, &(), BYTES)
                .await
                && let Some(peer) = app.peers.rows.write().await.get_mut(&device.name)
            {
                for d in &mut deliveries {
                    d.session = d.session.on(&device.name);
                }
                peer.deliveries = deliveries;
                peer.recount();
                for delivery in &peer.deliveries {
                    let _ = app
                        .peers
                        .changed
                        .send(Live::Delivery(delivery.clone().into()));
                }
            }
            let bytes = response
                .bytes_stream()
                .map(|b| b.map_err(std::io::Error::other));
            let mut reader = BufReader::new(StreamReader::new(bytes));
            let mut line = Vec::new();
            let mut initial = true;
            loop {
                line.clear();
                let size = tokio::time::timeout(
                    Duration::from_secs(45),
                    (&mut reader)
                        .take(BYTES as u64 + 1)
                        .read_until(b'\n', &mut line),
                )
                .await
                .ok()?
                .ok()?;
                if size == 0 || size > BYTES || !line.ends_with(b"\n") {
                    return None;
                }
                // A line a newer peer sends that this server does not know is skipped, as the
                // terminal skips it; leaving would meet the same line on every reconnect.
                let Ok(live) = serde_json::from_slice::<Live>(&line) else {
                    continue;
                };
                if let Live::Agents { server, .. } = &live
                    && server.id == *app.id
                {
                    return None;
                }
                let heartbeat = matches!(live, Live::Server(_));
                let reconnect = std::mem::replace(&mut initial, false);
                app.peers.update(&device.name, Some(live)).await;
                // A reconnect recovers every child there; a heartbeat, those no longer working.
                let ended = match (reconnect, heartbeat) {
                    (true, _) => None,
                    (false, true) => Some(
                        (app.peers.rows.read().await.get(&device.name).into_iter())
                            .flat_map(|p| p.agents.values())
                            .filter(|r| r.state != crate::protocol::State::Working)
                            .map(|r| r.session.on(&device.name))
                            .collect::<Vec<_>>(),
                    ),
                    (false, false) => continue,
                };
                let recovered =
                    crate::children::recover(&app.controller, &device.name, ended.as_deref());
                if let Err(e) = recovered.await {
                    eprintln!("razor: recovering children on {}: {e}", device.name);
                }
            }
        }
        .await;
        app.peers.update(&device.name, None).await;
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

/// Forward the same device-authorized session operations a person's client uses, once: a
/// session addressed `<id>@<server>`, or `?server=<name>` on listing or starting sessions and
/// listing models or repositories.
pub(super) async fn forward(
    State(app): State<App>,
    Extension(caller): Extension<devices::Enrolled>,
    Extension(mut gone): Extension<Gone>,
    request: Request,
    next: Next,
) -> Result<Response, Refusal> {
    let decoded = request.uri().path().replace("%40", "@");
    let query = request.uri().query().unwrap_or_default();
    let address = match decoded.strip_prefix("/v1/sessions/") {
        Some(tail) => tail.split_once('/').and_then(|(address, route)| {
            let (id, name) = address.split_once('@')?;
            Some((
                name.to_owned(),
                format!("/v1/sessions/{id}/{route}?{query}"),
            ))
        }),
        None => {
            let (named, rest): (Vec<_>, Vec<_>) =
                query.split('&').partition(|p| p.starts_with("server="));
            let path = format!("{decoded}?{}", rest.join("&"));
            let named = named.first().filter(|name| name.len() > 7);
            named.map(|name| (name[7..].to_owned(), path))
        }
    };
    let Some((name, path)) = address else {
        return Ok(next.run(request).await);
    };
    let paired = app.devices.paired().await;
    let from_server = paired
        .iter()
        .any(|(d, _)| Some(&d.name) == caller.0.as_ref());
    guard(&decoded, request.headers().contains_key(HOP) || from_server)?;
    let (device, mut peer_gone) =
        paired
            .into_iter()
            .find(|(d, _)| d.name == name)
            .ok_or(Refusal(
                StatusCode::NOT_FOUND,
                "That server is not paired; enroll it before addressing its sessions.".into(),
            ))?;
    let response = relay(&app, &device, request, &path).await?;
    Ok(response.map(|body| {
        Body::from_stream(body.into_data_stream().take_until(async move {
            tokio::select! { _ = gone.changed() => {}, _ = peer_gone.changed() => {} }
        }))
    }))
}

/// Forwards to `path` and its query once, bounding the input size and the wait for response
/// headers; a body, a stream or a small answer such as the views, lasts until it ends, its client
/// leaves or a key is revoked.
pub(super) async fn relay(
    app: &App,
    device: &Device,
    request: Request,
    path: &str,
) -> Result<Response, Refusal> {
    let mut outgoing = app
        .devices
        .request(device, request.method().clone(), path)
        .header(HOP, "1");
    for h in [header::CONTENT_TYPE, header::RANGE, header::IF_RANGE] {
        if let Some(value) = request.headers().get(&h) {
            outgoing = outgoing.header(h, value);
        }
    }
    let body = axum::body::to_bytes(request.into_body(), BODY_BYTES)
        .await
        .map_err(|_| bad("The forwarded input exceeds 32 MiB; shorten it."))?;
    send(outgoing.body(body)).await
}

async fn send(outgoing: reqwest::RequestBuilder) -> Result<Response, Refusal> {
    let Ok(Ok(response)) = tokio::time::timeout(Duration::from_secs(30), outgoing.send()).await
    else {
        return Err(Refusal(
            StatusCode::BAD_GATEWAY,
            "The paired request did not finish; check its transcript before sending again.".into(),
        ));
    };
    let mut output = Response::new(Body::empty());
    *output.status_mut() = response.status();
    for h in [
        header::CONTENT_TYPE,
        header::CONTENT_LENGTH,
        header::CONTENT_RANGE,
        header::ACCEPT_RANGES,
    ] {
        if let Some(value) = response.headers().get(&h) {
            output.headers_mut().insert(h, value.clone());
        }
    }
    *output.body_mut() = Body::from_stream(response.bytes_stream());
    Ok(output)
}

/// Refuses a forwarded path with a dot segment or an encoded separator, and any request that
/// already came through a server: the hop header or a paired server's own key.
fn guard(path: &str, relayed: bool) -> Result<(), Refusal> {
    let path = path.to_ascii_lowercase().replace("%2e", ".");
    if path.split('/').any(|s| matches!(s, "." | ".."))
        || ["%2f", "%5c", "\\"].iter().any(|s| path.contains(s))
    {
        return Err(bad(
            "A forwarded path cannot contain dot segments or encoded separators.",
        ));
    }
    match relayed {
        true => Err(bad(
            "Only a person's client can forward a directly paired session operation once.",
        )),
        false => Ok(()),
    }
}

fn qualify(row: &mut AgentView, name: &str) {
    row.session = row.session.on(name);
    row.parent = row.parent.take().map(|p| p.on(name));
    for ask in &mut row.asks {
        ask.id.0 = ask.id.0.on(name);
    }
}

/// Only an addressed enrollment can perform direct-child operations; the key names its owner.
pub(super) async fn child(
    State(app): State<App>,
    Extension(caller): Extension<devices::Enrolled>,
    Json(request): Json<crate::protocol::ChildRequest>,
) -> Reply {
    let server = match caller.0 {
        Some(name) => app.devices.peer(&name).await,
        None => None,
    }
    .ok_or_else(|| bad("Only a directly paired server can operate its children."))?;
    Ok(Json(
        app.controller.receive_child(&server.name, request).await?,
    ))
}

/// Only reservations, settings reads and terminal reports cross to the configured directly
/// paired owner, once.
pub(super) async fn voice(
    app: &App,
    owner: &str,
    action: &str,
    method: reqwest::Method,
    body: Option<Value>,
) -> Result<Value, Refusal> {
    let Some((device, _)) = app.peers.owner(app, owner).await else {
        return Err(bad("The voice settings owner is unknown; write instead."));
    };
    let request = app
        .devices
        .request(&device, method, &format!("/v1/voice/{action}"))
        .header(HOP, "1")
        .timeout(Duration::from_secs(30))
        .json(&body);
    let response = send(request)
        .await
        .map_err(|_| bad("The voice settings owner is unavailable; write instead."))?;
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 64 << 10)
        .await
        .map_err(other("Reading voice owner answer"))?;
    let answer: Value =
        serde_json::from_slice(&bytes).map_err(other("Reading voice owner answer"))?;
    if !status.is_success() {
        return Err(Refusal(
            status,
            answer["error"]
                .as_str()
                .unwrap_or("The voice owner refused; write instead.")
                .into(),
        ));
    }
    Ok(answer)
}
