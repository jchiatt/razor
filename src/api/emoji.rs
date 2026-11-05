//! The person's emoji history: each base emoji's uses and remembered skin tone, kept by the
//! server in `emoji.json` beside `devices.json`, so two devices never race a read-modify-write.
use super::{App, bad, other};
use crate::protocol::{Emoji, EmojiUse, now};
use axum::extract::{Json, State};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{collections::BTreeMap, path::Path, path::PathBuf};
use tokio::{io::AsyncReadExt, sync::Mutex};

/// The base emoji the file keeps, and how many the history answers.
const KEPT: usize = 200;
const RECENT: usize = 32;

pub(super) struct History {
    path: PathBuf,
    kept: Mutex<Kept>,
}

/// Each base emoji's use count and last use, and the tones last chosen.
#[derive(Default, Serialize, Deserialize)]
struct Kept {
    used: BTreeMap<String, (u64, u64)>,
    tones: BTreeMap<String, u8>,
}

impl History {
    /// Reads `emoji.json`, at most 1 MiB; a missing or unreadable file is an empty history.
    pub(super) async fn open(home: &Path) -> Self {
        let (path, mut bytes) = (home.join("emoji.json"), Vec::new());
        if let Ok(file) = tokio::fs::File::open(&path).await {
            let _ = file.take(1 << 20).read_to_end(&mut bytes).await;
        }
        let kept = Mutex::new(serde_json::from_slice(&bytes).unwrap_or_default());
        Self { path, kept }
    }
}

/// `GET /v1/emoji`: the most used, then most recent, with the remembered tones.
pub(super) async fn history(State(app): State<App>) -> Json<Emoji> {
    let kept = app.emoji.kept.lock().await;
    let mut ranked: Vec<_> = kept.used.iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    let recent = ranked.into_iter().take(RECENT);
    Json(Emoji {
        recent: recent.map(|(emoji, _)| emoji.clone()).collect(),
        tones: kept.tones.clone(),
    })
}

/// `POST /v1/emoji`: one use, saved by write-and-rename; past 200 emoji the least used goes.
pub(super) async fn used(State(app): State<App>, Json(used): Json<EmojiUse>) -> super::Reply {
    if used.emoji.is_empty() || used.emoji.len() > 64 || used.tone.is_some_and(|t| t > 5) {
        return Err(bad("A use names one emoji and a tone from 0 to 5."));
    }
    let mut kept = app.emoji.kept.lock().await;
    let (count, _) = kept.used.get(&used.emoji).copied().unwrap_or_default();
    kept.used.insert(used.emoji.clone(), (count + 1, now()));
    match used.tone {
        Some(0) => drop(kept.tones.remove(&used.emoji)),
        Some(tone) => drop(kept.tones.insert(used.emoji, tone)),
        None => {}
    }
    while kept.used.len() > KEPT {
        let least = kept
            .used
            .iter()
            .min_by_key(|(_, u)| **u)
            .map(|(e, _)| e.clone());
        if let Some(least) = least {
            kept.used.remove(&least);
            kept.tones.remove(&least);
        }
    }
    let bytes = serde_json::to_vec(&*kept).map_err(std::io::Error::other);
    let new = app.emoji.path.with_extension("json.new");
    let write = async {
        tokio::fs::write(&new, bytes?).await?;
        tokio::fs::rename(&new, &app.emoji.path).await
    };
    write.await.map_err(other("Writing emoji.json"))?;
    Ok(Json(json!({})))
}
