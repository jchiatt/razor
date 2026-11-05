//! A video's watcher: a child Razor starts on a Gemini model when the model reading a video
//! cannot see it (docs/design/video.md).

use super::agent::{Kind, Placement, Start};
use super::{Children, message::words, project};
use crate::controller::{self, Controller};
use crate::protocol::{Body, Entry, EntryId, Input, Origin, SessionId, video_type};
use crate::record::length;
use tokio::io::AsyncReadExt;

/// The watcher's whole first input, filled in by `watch`.
const BRIEF: &str = include_str!("text/watcher.md");
/// The most characters of the person's words, and of the parent's last reply, a brief carries.
const WORDS: usize = 16_000;
const LAST: usize = 4_000;

impl Children {
    /// Admits `input` to `session`. A video must be an MP4 or QuickTime file in the session's
    /// folder, refused before admission otherwise. When the model that will read it (the
    /// input's, else the session's) does not take video and `[children] watcher`'s model has its
    /// key in this server's environment, the input is admitted naming a watcher, the first free
    /// `watcher-{n}` in the tree, which then starts: one lock holds from the name through the
    /// start, so two videos at once take two names, and a repeat of an admitted request id
    /// answers its entry and starts nothing. Errors are admission's own.
    pub(crate) async fn admit(
        &self,
        controller: &Controller,
        session: &SessionId,
        mut input: Input,
    ) -> Result<EntryId, controller::Error> {
        let Some(video) = input.video.as_mut() else {
            return controller.admit(session, input).await;
        };
        video.watcher = None;
        let (mut head, name) = (Vec::new(), video.file.to_string_lossy());
        let file = controller.file(session, &name).await?;
        let _ = file.take(12).read_to_end(&mut head).await;
        if video_type(&head).is_none() {
            let sentence = "Send the video as an MP4 or QuickTime file uploaded to this session.";
            return Err(controller::Error::Invalid(sentence.into()));
        }
        let config = controller.settings(session).await?.config;
        let s = controller.summary(session).await?;
        let model = input.model.as_deref().unwrap_or(&s.model);
        let key = crate::profiles::video(&config.children.watcher, &config).and_then(|e| e.key);
        let keyed =
            key.is_some_and(|k| k.is_empty() || std::env::var_os(k).is_some_and(|v| !v.is_empty()));
        if crate::profiles::video(model, &config).is_some() || !keyed {
            return controller.admit(session, input).await;
        }
        let held = self.watching.lock().await;
        let repeat = controller
            .summary(session)
            .await?
            .repeat(input.request.as_deref());
        if let Some(entry) = repeat {
            return Ok(entry);
        }
        let taken = super::remote::names(controller, session).await;
        let taken = taken.map_err(|e| controller::Error::Invalid(e.to_string()))?;
        let mut names = (1..).map(|n| format!("watcher-{n}"));
        video.watcher = names.find(|n| !taken.contains(n));
        let entry = controller.admit(session, input.clone()).await?;
        self.watch(controller, session, entry, input).await;
        drop(held);
        Ok(entry)
    }

    /// Starts the watcher `input`'s video names, as a child of `parent` launched at `entry`, the
    /// input itself: on `[children] watcher`'s model at the parent's effort, in place in the
    /// video's folder, its first input `text/watcher.md` filled with the person's words, the
    /// parent's name, project and last reply, and the video's path and length, carrying the
    /// video. A start that fails reaches the parent as Razor's input with its sentence.
    async fn watch(
        &self,
        controller: &Controller,
        parent: &SessionId,
        entry: EntryId,
        input: Input,
    ) {
        let Some(mut video) = input.video else {
            return;
        };
        let name = video.watcher.take().unwrap_or_default();
        let started = async {
            let s = controller.summary(parent).await?;
            let mut page = controller.page(parent, Some(entry), 64).await?;
            let mut last = String::new();
            while let Some(Ok(line)) = page.next_line().await {
                if let Ok(Body::Text { text }) =
                    serde_json::from_str::<Entry>(&line).map(|e| e.body)
                {
                    last = text;
                }
            }
            let skip = last.chars().count().saturating_sub(LAST);
            let (last, words) = (last.chars().skip(skip), input.text.chars().take(WORDS));
            video.file = self.dir(parent).join(&video.file);
            let (folder, length) = (video.file.with_file_name(""), length(video.duration_ms));
            let brief = BRIEF
                .replace("{name}", &name)
                .replace("{parent}", &s.name)
                .replace("{project}", &project(&s))
                .replace("{path}", &video.file.to_string_lossy())
                .replace("{duration}", &length)
                .replace("{frames}", &folder.join("frames").to_string_lossy())
                .replace("{last}", &last.collect::<String>())
                .replace("{words}", &words.collect::<String>());
            let config = controller.settings(parent).await?.config;
            let start = Start {
                task: None,
                name: name.clone(),
                server: None,
                brief,
                shared_brief: None,
                kind: Kind::Worker,
                cwd: Some(folder),
                model: Some(config.children.watcher),
                effort: None,
                speed: None,
                placement: Placement::InPlace,
                video: Some(video),
            };
            self.start(controller, parent, entry, start).await
        };
        if let Err(e) = started.await {
            let text = words("watch_failed").replace("{name}", &name);
            let input = Input::new(Origin::Razor, text.replace("{error}", &e.to_string()));
            if let Err(e) = controller.admit(parent, input).await {
                eprintln!("razor: {name} could not start for {parent}, nor say so: {e}");
            }
        }
    }
}
