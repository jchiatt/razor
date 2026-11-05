//! Codex's `imagegen` on the ChatGPT sign-in (docs/design/responses.md): one generation or edit
//! per call, as Codex makes it at the pin, its PNG saved under the session's `images/`.

use super::{Error, Responses, bounded, other, rejected};
use crate::protocol::{IMAGE_BYTES, SessionId, image_type};
use base64::{Engine, engine::general_purpose::STANDARD};
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;
use tokio::io::AsyncReadExt;

/// The most bytes an image response may hold: Codex's 32 MiB image, as base64, with its JSON.
const RESPONSE: usize = 48 << 20;
/// Codex's own client name, which it sends on every request.
const ORIGINATOR: &str = "codex_cli_rs";

/// `imagegen`'s arguments, typed as Codex decodes them at the pin, so a field of the wrong type is
/// refused before any request; `images` is what the Responses task added for
/// `num_last_images_to_include`, the conversation's latest images as data URLs.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    prompt: String,
    #[serde(default)]
    transparent_background: bool,
    referenced_image_paths: Option<Vec<String>>,
    num_last_images_to_include: Option<usize>,
    #[serde(default)]
    images: Vec<String>,
}

impl Responses {
    /// One `imagegen` call: an edit of the files `referenced_image_paths` names, or of the
    /// conversation's latest images (`images`, data URLs the Responses task put in for
    /// `num_last_images_to_include`), else a generation, POSTed to `{base}/images/edits` or
    /// `/images/generations` with the sign-in's bearer and account id, refreshing the sign-in
    /// once on 401. Reads at most 48 MiB, writes the first image's PNG bytes to `saved` and
    /// returns their length. Arguments Codex refuses, a field of the wrong type among them, are
    /// `Refused` before any request; a refusal from the backend is `Rejected` with its message,
    /// the bearer taken out. `turn` is Codex's image turn id.
    pub(crate) async fn image(
        &self,
        base: &str,
        session: &SessionId,
        turn: &str,
        input: Value,
        saved: &Path,
    ) -> Result<u64, Error> {
        let refuse = |text: &str| Err(Error::Refused(text.into()));
        let args: Args = serde_json::from_value(input)
            .map_err(|e| Error::Refused(format!("The input does not fit this tool: {e}.")))?;
        let mut images = Vec::new();
        match (args.referenced_image_paths, args.num_last_images_to_include) {
            (Some(_), Some(_)) => {
                return refuse(
                    "Provide only one of referenced_image_paths or num_last_images_to_include.",
                );
            }
            (Some(paths), None) if paths.len() > 5 => {
                return refuse("referenced_image_paths must contain at most 5 paths.");
            }
            (Some(paths), None) => {
                for path in paths {
                    images.push(json!({"image_url": data_url(&path).await?}));
                }
            }
            (None, Some(count)) => {
                if !(1..=5).contains(&count) || args.images.len() != count {
                    return Err(Error::Refused(format!(
                        "num_last_images_to_include must be 1 to 5, and the conversation has {} images.",
                        args.images.len()
                    )));
                }
                images.extend(args.images.into_iter().map(|url| json!({"image_url": url})));
            }
            (None, None) => {}
        }
        let background = match args.transparent_background {
            true => "transparent",
            false => "opaque",
        };
        let mut body = json!({"prompt": args.prompt, "background": background,
            "model": "gpt-image-2", "quality": "auto", "size": "auto"});
        let route = match images.is_empty() {
            true => "images/generations",
            false => {
                body["images"] = images.into();
                "images/edits"
            }
        };
        let url = format!("{}/{route}", base.trim_end_matches('/'));
        let mut tokens = self.credentials(false, None).await?;
        for retry in [true, false] {
            let mut post = (self.client.post(&url).bearer_auth(&tokens.access_token))
                .header("session_id", session.as_str())
                .header("originator", ORIGINATOR)
                .header("x-codex-image-turn-id", turn)
                .json(&body);
            if !tokens.account_id.is_empty() {
                post = post.header("chatgpt-account-id", &tokens.account_id);
            }
            let response = post
                .send()
                .await
                .map_err(other("sending an image request"))?;
            let status = response.status();
            if status == StatusCode::UNAUTHORIZED && retry {
                tokens = self.credentials(false, Some(&tokens.access_token)).await?;
                continue;
            }
            if !status.is_success() {
                return Err(rejected(status, response)
                    .await
                    .without(&tokens.access_token));
            }
            let reply: Value = serde_json::from_slice(&bounded(response, RESPONSE).await?)
                .map_err(other("reading an image response"))?;
            let encoded = reply["data"][0]["b64_json"].as_str();
            let encoded = encoded.ok_or(Error::Refused(
                "The image request returned no image.".into(),
            ))?;
            let bytes =
                (STANDARD.decode(encoded.trim())).map_err(other("decoding a generated image"))?;
            let written = async {
                tokio::fs::create_dir_all(saved.parent().unwrap_or(saved)).await?;
                tokio::fs::write(saved, &bytes).await
            };
            written.await.map_err(other("saving a generated image"))?;
            return Ok(bytes.len() as u64);
        }
        Err(Error::Auth)
    }
}

/// A referenced image as a data URL: an absolute path to a PNG, JPEG or WebP file of at most
/// `IMAGE_BYTES`, read with that bound.
async fn data_url(path: &str) -> Result<String, Error> {
    let unreadable = || {
        Error::Refused(format!(
            "Unable to read the referenced image at `{path}`; give an absolute path to a PNG, JPEG or WebP image of at most 7.5 MB."
        ))
    };
    if !Path::new(path).is_absolute() {
        return Err(unreadable());
    }
    let mut bytes = Vec::new();
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|_| unreadable())?;
    (file.take(IMAGE_BYTES as u64 + 1).read_to_end(&mut bytes))
        .await
        .map_err(|_| unreadable())?;
    match image_type(&bytes) {
        Some((media_type, _)) if bytes.len() <= IMAGE_BYTES => Ok(format!(
            "data:{media_type};base64,{}",
            STANDARD.encode(&bytes)
        )),
        _ => Err(unreadable()),
    }
}
