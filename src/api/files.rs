use super::{App, Refusal, Reply, bad, other};
use crate::protocol::SessionId;
use axum::{
    body::{Body, Bytes},
    extract::{Json, Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use http_range::HttpRange;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

// The controller checks and opens once; every read, metadata query and seek uses that handle.
pub(super) async fn file(
    State(app): State<App>,
    Path((id, name)): Path<(SessionId, String)>,
    headers: HeaderMap,
) -> Result<Response, Refusal> {
    let file = app.controller.file(&id, &name).await?;
    let kind = match name.rsplit('.').next() {
        Some("m4a") => "audio/mp4",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("mp4") => "video/mp4",
        Some("mov") => "video/quicktime",
        _ => "application/octet-stream",
    };
    serve(file, &headers, kind).await
}

/// An opened file as `kind`, whole or one byte range of it.
pub(super) async fn serve(
    mut file: tokio::fs::File,
    headers: &HeaderMap,
    kind: &'static str,
) -> Result<Response, Refusal> {
    let size = file
        .metadata()
        .await
        .map_err(other("Reading file size"))?
        .len();
    let mut status = StatusCode::OK;
    let mut start = 0;
    let mut length = size;
    // No validator is published: an If-Range cannot match, so send the whole representation.
    if !headers.contains_key(header::IF_RANGE)
        && let Some(value) = headers.get(header::RANGE)
        // Unknown range units must be ignored (RFC 9110); the crate parses only bytes.
        && value.as_bytes().get(..6).is_some_and(|unit| unit.eq_ignore_ascii_case(b"bytes="))
        && headers.get_all(header::RANGE).iter().count() == 1
    {
        match HttpRange::parse_bytes(value.as_bytes(), size) {
            Ok(ranges) if ranges.len() == 1 && ranges[0].length > 0 => {
                start = ranges[0].start;
                length = ranges[0].length;
                status = StatusCode::PARTIAL_CONTENT;
            }
            // RFC 9110 permits ignoring a Range; multipart responses are not needed here.
            Ok(ranges) if ranges.len() > 1 => {}
            _ => {
                return Ok((
                    [
                        (header::CONTENT_RANGE, format!("bytes */{size}")),
                        (header::ACCEPT_RANGES, "bytes".into()),
                    ],
                    Refusal(
                        StatusCode::RANGE_NOT_SATISFIABLE,
                        "The byte range cannot be read; request a range within the file.".into(),
                    ),
                )
                    .into_response());
            }
        }
    }
    file.seek(std::io::SeekFrom::Start(start))
        .await
        .map_err(other("Seeking file output"))?;
    let mut response = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, kind)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, length);
    if status == StatusCode::PARTIAL_CONTENT {
        response = response.header(
            header::CONTENT_RANGE,
            format!("bytes {start}-{}/{size}", start + length - 1),
        );
    }
    Ok(response
        .body(Body::from_stream(ReaderStream::new(file.take(length))))
        .map_err(other("Encoding file response"))?)
}

/// A person's file, at most the 32 MiB body bound, kept under its own name in a new folder of
/// its own; an input names the path this returns.
pub(super) async fn upload(
    State(app): State<App>,
    Path((id, name)): Path<(SessionId, String)>,
    bytes: Bytes,
) -> Reply {
    if name.contains('/') || name.trim_matches('.').is_empty() {
        return Err(bad("Name the file with one name, such as scan.pdf."));
    }
    let path = format!("files/{}/{name}", crate::devices::fresh(8)?);
    app.controller.keep(&id, &path, bytes.into()).await?;
    Ok(Json(json!({ "path": path })))
}
