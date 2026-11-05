//! A video uploaded to the Files API beside a Gemini chat endpoint (docs/design/video.md).

use super::{Error, Responses, bounded, other, rejected};
use crate::{
    profiles::Endpoint,
    protocol::{Millis, video_type},
};
use serde_json::Value;
use std::{io::SeekFrom, path::Path, time::Duration};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

/// How long an upload and Google's processing of it may take together.
const LIMIT: Duration = Duration::from_secs(300);

/// A video's file `uri` and when Google deletes it.
pub(super) type Upload = Result<(String, Millis), Error>;

impl Responses {
    /// Uploads the video at `path` to the Files API beside `endpoint` in one raw request, its
    /// body streamed from the file and the key as `x-goog-api-key`, the only way that API takes
    /// it; then reads the file at its `uri` each second until it is ACTIVE, all within five
    /// minutes. Gives that `uri` and when Google deletes the file; no error holds the key, Google's
    /// own words included.
    pub(super) async fn upload(&self, endpoint: &Endpoint, path: &Path) -> Upload {
        let name = endpoint.key.as_deref().unwrap_or_default();
        let value = std::env::var(name).unwrap_or_default();
        let (key, url) = (value.as_str(), endpoint.url.trim_end_matches('/'));
        let read = |response: reqwest::Response| async move {
            match response.status().as_u16() {
                401 | 403 => Err(Error::Key(name.into())),
                200 => serde_json::from_slice::<Value>(&bounded(response, 1 << 20).await?)
                    .map_err(other("reading Google's answer about the video")),
                _ => Err(rejected(response.status(), response).await.without(key)),
            }
        };
        let io = |source| Error::Other {
            source: Box::new(source),
            context: "reading the video",
        };
        let uploaded = async {
            let (mut file, mut head) = (tokio::fs::File::open(path).await.map_err(io)?, [0; 12]);
            file.read_exact(&mut head).await.map_err(io)?;
            let kind =
                video_type(&head).ok_or(Error::Refused("it is not MP4 or QuickTime".into()))?;
            let length = file.seek(SeekFrom::End(0)).await.map_err(io)?;
            file.rewind().await.map_err(io)?;
            let upload = url.trim_end_matches("/v1beta/openai").to_owned() + "/upload/v1beta/files";
            let sent = (self.client.post(upload))
                .header("x-goog-api-key", key)
                .header("x-goog-upload-protocol", "raw")
                .header("content-type", kind)
                .header("content-length", length)
                .body(reqwest::Body::wrap_stream(ReaderStream::new(file)));
            let sent = sent.send().await.map_err(other("uploading the video"))?;
            let mut file = read(sent).await?["file"].take();
            while let (Some("PROCESSING"), Some(uri)) =
                (file["state"].as_str(), file["uri"].as_str())
            {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let state = self
                    .client
                    .get(uri)
                    .header("x-goog-api-key", key)
                    .send()
                    .await;
                file = read(state.map_err(other("reading the video's state"))?).await?;
            }
            let expires = file["expirationTime"]
                .as_str()
                .map(str::parse::<jiff::Timestamp>);
            match (file["state"].as_str(), file["uri"].as_str(), expires) {
                (Some("ACTIVE"), Some(uri), Some(Ok(at))) => {
                    Ok((uri.to_owned(), at.as_millisecond().max(0) as Millis))
                }
                _ => Err(Error::Refused(match file["error"]["message"].as_str() {
                    Some(why) => format!("Google could not process it: {why}"),
                    None => "Google's answer named no file ready to use".into(),
                })),
            }
        };
        let late = || Error::Refused("its upload took longer than five minutes".into());
        let done = tokio::time::timeout(LIMIT, uploaded).await;
        done.unwrap_or_else(|_| Err(late()))
            .map_err(|e| e.without(key))
    }
}
