//! Resident schedules, memory and projects.

use super::*;
use crate::protocol::ResidentProject;
use axum::routing::patch;

pub(super) fn routes() -> Router<App> {
    Router::new()
        .route("/v1/residents", get(resident_list))
        .route("/v1/residents/{name}/projects", get(projects).post(project))
        .route("/v1/residents/{name}/projects/{id}", patch(project))
        .route("/v1/residents/{name}/{change}", post(resident))
        .route("/v1/residents/{name}/run/{schedule}", post(run))
        .route("/v1/residents/{name}/memory", get(memory))
        .route("/v1/residents/{name}/memory/{id}", delete(forget))
}

async fn resident_list(State(app): State<App>) -> Json<Vec<crate::residents::Listed>> {
    Json(app.residents.list().await)
}

async fn projects(State(app): State<App>, Path(at): Path<At>) -> Reply<Vec<ResidentProject>> {
    Ok(Json(app.residents.projects(&at.name).await?))
}

/// A projects route's resident, and on a rename, its project.
#[derive(Deserialize)]
struct At {
    name: SessionId,
    id: Option<String>,
}

#[derive(Deserialize)]
struct Naming {
    name: String,
    request: Option<String>,
}

/// A new project, 201, a repeated request id answering the project it made; or with the route's
/// id, that project renamed, its folder and so its id unchanged.
async fn project(
    State(app): State<App>,
    Path(at): Path<At>,
    Json(n): Json<Naming>,
) -> Result<(StatusCode, Json<ResidentProject>), Refusal> {
    request(n.request.as_deref())?;
    let (id, made) = (at.id.as_deref(), n.request.as_deref());
    let named = app.residents.name_project(&at.name, id, &n.name, made);
    let status = id.map_or(StatusCode::CREATED, |_| StatusCode::OK);
    Ok((status, Json(named.await?)))
}

/// Run now: the schedule's entry in the resident's home session.
async fn run(State(app): State<App>, Path((name, schedule)): Path<(SessionId, String)>) -> Reply {
    let entry = app.residents.run(&name, &schedule).await?;
    Ok(Json(json!({ "entry": entry })))
}

/// What a resident remembers, as memory lists it.
async fn memory(State(app): State<App>, Path(name): Path<SessionId>) -> Reply {
    Ok(Json(app.residents.memory(&name).await?))
}

/// The person deletes one of a resident's memories, words and all.
async fn forget(State(app): State<App>, Path((name, id)): Path<(SessionId, String)>) -> Reply {
    app.residents.forget(&name, &id).await?;
    Ok(Json(json!({})))
}

async fn resident(
    State(app): State<App>,
    Path((name, change)): Path<(SessionId, String)>,
) -> Reply {
    match change.as_str() {
        "start" => app.residents.start(&name).await?,
        "stop" => app.residents.stop(&name).await?,
        _ => return Err(bad(format!("A resident can start or stop, not {change}."))),
    }
    Ok(Json(json!({})))
}
