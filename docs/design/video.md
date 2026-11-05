# Video: a Gemini session sees it, any other gets a watcher

Design page, 2026-11-01, for video input (Trey's ruling of 2026-11-01: "Go, build the video Gemini stuff, and yes it's on every server it's fine"). The Gemini facts below were read from Google's documentation and checked by scratch requests to `gemini-3.8-flash` on 2026-11-01.

Four rules carry it:

- **A video is an upload the input names**, as a voice note's audio is: the files route stores it, the input carries `{file, duration_ms}`, and admission is at once. Nothing waits on Gemini.
- **One wire.** The chat route Razor already speaks to Gemini takes a video as an `image_url` part whose URL is the Files API's `uri` for the uploaded file (tested). The upload is the only new request shape.
- **A person's video is seen in the turn it starts.** When that turn ends done, its part in the conversation becomes a line with its path. Only a watcher's video stays, because follow-up questions are its job, and a rebuild carries it.
- **A watcher is an ordinary child.** The server starts it from the input path on `[children] watcher` (Flash by default), launched at the video's input entry, in place in the video's folder; its report, mail, continuation, status row and the phone's circle are a child's.

## Surface

```rust
// ---- src/protocol/mod.rs -----------------------------------------------------------------------

/// A video the person sent: one type on the wire and in the record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Video {
    /// The video, relative to the session's folder as `POST /v1/sessions/{id}/files/{name}`
    /// returned it; MP4 or QuickTime. Absolute only in a watcher's brief, whose video is in its
    /// parent's folder.
    pub file: PathBuf,
    /// Its length as the sending client measured it; shown to the model and to people.
    pub duration_ms: u64,
    /// Set by the server, never a client (a client's value is replaced): the child it started to
    /// watch this video, because the model reading it does not take video. Absent when the model
    /// sees it itself, or no watcher could start (no key for the watcher's model on this server).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watcher: Option<String>,
}

pub struct Input {
    // ... as today, plus:
    /// Set only on an input that carries a video; boxed, as `voice` is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<Box<Video>>,
}

// ---- src/protocol/api.rs -----------------------------------------------------------------------

/// `video/mp4` or `video/quicktime` from a file's first bytes (an ISO media `ftyp` box, `qt  `
/// for QuickTime); `None` for anything else. Beside `image_type`; the input path and the upload
/// both read it.
pub fn video_type(head: &[u8]) -> Option<&'static str>;

pub struct ClientInput {
    // ... as today, plus:
    /// A video uploaded through the files route; answered at once like typed words.
    #[serde(default)]
    pub video: Option<Video>,
}

pub struct Server {
    // ... as today, plus:
    /// This server takes `ClientInput.video`; always true here.
    pub video: bool,
}

// ---- src/protocol/conversation.rs --------------------------------------------------------------

pub struct BridgeInput {
    // ... entry, text, images as today, plus:
    /// The input's video, absolute, for a model path that takes video; the others ignore it.
    /// Set only when no watcher was started for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<PathBuf>,
    /// The video stays in every later request of this native conversation: set on a watcher's
    /// brief. Otherwise the video leaves the requests when its turn ends.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub keep: bool,
}

// ---- src/profiles/ -----------------------------------------------------------------------------

impl Endpoint {
    /// The endpoint is Google's OpenAI-compatibility path (`…/v1beta/openai`), whose host keeps
    /// the Files API beside it at `/upload/v1beta/files`: its model takes video. Any Gemini row,
    /// shipped or configured, that keeps Google's path qualifies; a flow's stand-in at that path
    /// does too.
    pub fn video(&self) -> bool;
}

/// The chat endpoint `model` is reached at under `config`, when it takes video; `None` for
/// Claude, GPT and every other chat model. The input path reads it for the session and the watcher.
pub fn video(model: &str, config: &Config) -> Option<Endpoint>;

impl Conversation {
    // `send` keeps a `razor_video` part as it keeps `razor_image`: {path, keep}. `body` shows it
    // as {"type": "image_url", "image_url": {"url": uri}} with its latest upload's `uri`, or, with
    // none, as the profile's line "The video {path} could not be sent to the model: {error}".
    // Chat requests leave out the `razor_upload` items below, which are record data, not messages.

    /// `shown`, the videos about to be sent, and those still in the requests, less every one
    /// whose latest upload has more than an hour of Google's 48 left.
    pub fn stale(&self, shown: Vec<PathBuf>, now: Millis) -> Vec<PathBuf>;
    /// Keeps `path`'s upload for this request and every later one: a `razor_upload` item
    /// {path, uri, expires} added to the items the next response records, so a renewal is as
    /// durable as a first upload and a reopened conversation reuses it.
    pub fn renew(&mut self, path: &Path, uri: &str, expires: Millis);
    /// Keeps why `path` could not be uploaded, a `razor_upload` item {path, error}, the same
    /// way; the task writes the sentence from the Responses error, and `stale` tries it again.
    pub fn unsent(&mut self, path: &Path, error: &str);
    /// Ends the videos a finished turn saw: every part without `keep` becomes the profile's line
    /// "(The video {path} was in an earlier turn and is no longer shown; ffmpeg can cut its frames
    /// for you to look at again.)". Called when a turn ends done (a stopped or failed turn keeps
    /// its videos for the retry), and by `resume`, since a conversation resumes between turns.
    pub fn spend(&mut self, profile: &Profile);
}

// ---- src/responses/ ----------------------------------------------------------------------------

/// A video's file `uri` and when Google deletes it.
pub(super) type Upload = Result<(String, Millis), Error>;

impl Responses {
    /// Uploads the video at `path` to the Files API beside `endpoint` in one raw request, its
    /// body streamed from the file and the key as `x-goog-api-key`, the only way that API takes
    /// it; then reads the file at its `uri` each second until it is ACTIVE, all within five
    /// minutes. Gives that `uri` and when Google deletes the file; no error holds the key, Google's
    /// own words (a FAILED file's reason) included: `Error::without` now redacts `Refused` too.
    pub(super) async fn upload(&self, endpoint: &Endpoint, path: &Path) -> Upload;
}
// The task: before a request on an endpoint that takes video, every video it will show (the
// waiting inputs' and `Conversation::stale`'s) not yet tried this turn is uploaded in the turn,
// and the request waits for them in the task's select beside the request and the cells, so Escape
// still stops the turn. Each result goes to `renew`, or as its sentence to `unsent`; a turn that
// ends done calls `spend`.

// ---- src/record/ -------------------------------------------------------------------------------

pub struct Summary {
    // ... as today, plus:
    /// Inputs whose video a model may still be shown, each with whether it is kept for the
    /// session's life (a watcher's own, at most one): the kept one and those unanswered, pruned
    /// at each turn's end and withdrawal, apart from the four asks a rebuild keeps verbatim.
    pub videos: Vec<(EntryId, bool)>,
}

impl Record {
    /// As today, and beside the note and the context: each input of `Summary::videos` at or
    /// before the cut and not held, wherever it sits (a watcher's first input after any number
    /// of questions; a batch a model switch or restart rebuild opens is held, and follows),
    /// as a `BridgeInput` with its video, and its words unless the context carries them
    /// verbatim. The controller sends them ahead of the rebuilt context when the new model path
    /// takes video.
    pub async fn rebuild(&self, trigger: Trigger, note: Option<String>, cut: EntryId,
        held: &[EntryId]) -> Result<(String, String, Vec<BridgeInput>), Error>;
}

// ---- src/children/ -----------------------------------------------------------------------------

/// Text a model reads, in each input's own words, now with its video (`text/mail.toml`):
/// `video = "(Video: {path}, {duration} long.)\n"` and `watching = "(Video: {path}, {duration}
/// long. Your helper {name} is watching it on Gemini and will report a machine-made description
/// and timeline of key moments, with frames; message it while it works, or continue it with
/// questions after it reports.)\n"`, ahead of the person's words.
///
/// `heard` now makes the whole `BridgeInput` (the controller's `rendered` is a call to it, and
/// the rebuild uses its text and images): the text as before with the video's line; images and
/// video made absolute; the video passed on only when no watcher was named, and `keep` set when
/// the input is mail, which only a watcher's first input carries a video in.
pub fn heard(me: &Summary, dir: &Path, entry: EntryId, input: Input) -> BridgeInput;

impl Children {
    /// Admits `input` to `session`; the input route's plain admission. A video must be an MP4 or
    /// QuickTime file in the session's folder ("Send the video as an MP4 or QuickTime file
    /// uploaded to this session.", before admission). When the model that will read it (the
    /// input's, else the session's) does not take video and `[children] watcher`'s model has its
    /// key in this server's environment, the input is admitted naming a watcher, the first free
    /// `watcher-{n}` in the tree, which then starts as a child launched at the input's entry: on
    /// the watcher's model at the session's effort, in place in the video's folder, its whole
    /// first input `text/watcher.md` filled with the person's words, the session's name, project
    /// and last reply (its last 4,000 characters), and the video's path, length and `frames/`
    /// folder, carrying the video. One lock holds from the name through the start, so two videos
    /// at once take two names, and a repeat of an admitted request id, read under it, answers its
    /// entry and starts nothing. A start that fails reaches the session as Razor's input: "Your
    /// helper {name} could not start to watch the video: {sentence}". Errors are admission's own.
    pub async fn admit(&self, controller: &Controller, session: &SessionId, input: Input)
        -> Result<EntryId, controller::Error>;
}

// ---- src/controller/mod.rs ---------------------------------------------------------------------

impl Controller {
    /// The child actions; the input route admits a video through them, to start its watcher.
    pub fn children(&self) -> &Children;
}

// ---- src/controller/ ---------------------------------------------------------------------------

impl Controller {
    /// The child actions this controller runs; the input path starts watchers through it.
    pub fn children(&self) -> &Children;
}

// ---- src/config/mod.rs -------------------------------------------------------------------------

pub struct Children {
    // ... max, grace, reviewer, researcher as today, plus:
    /// `[children] watcher`: the model a watcher runs on, `gemini-3.8-flash` by default, read at
    /// each video input. A model that cannot see video, or a Gemini row moved off Google's path
    /// (a proxy in front of Gemini does that), starts no watcher; the config is not refused.
    pub watcher: String,
}
```

**Routes.** No new route.

| request | change |
| --- | --- |
| `POST /v1/sessions/{id}/files/{name}` | none: the phone uploads the raw bytes as `video.mp4` (HEVC from AVAssetExportSession), under the 32 MiB body bound |
| `POST /v1/sessions/{id}/input` | `ClientInput.video`; answers `{entry}` at once; 400 when the file is not MP4 or QuickTime or not in the session's folder; request ids as for any input |
| `GET /v1/sessions/{id}/files/{video.file}` | byte ranges as today; `.mp4` served as `video/mp4`, `.mov` as `video/quicktime`, so AVPlayer takes them |
| `GET /v1/status`, every `agents` line | `server.video`, and each paired server's in `servers` |

## Data flow

1. The phone uploads the video through the files route and sends the input with `video` and the person's words; the route builds the `Input` as for typed words and admits it through `Children::admit` (a voice note keeps its own path).
2. `Children::admit` clears any `watcher` a client sent, opens the file through `Controller::file` (inside the folder, no links) and reads its type from its first bytes.
3. If the model that will read it takes video (`profiles::video`), or no watcher's key is set, it is admitted as it is; with no key, the model reads the path.
4. Otherwise, under one lock, a repeat of an admitted request id answers its entry; else the input is admitted naming the first free `watcher-{n}` in the tree, and the watcher starts, launched at that entry. The route answers `{entry}`, and the phone shows "Watching…" while the row with `parent` = the session and `launched` = that entry is working.
5. `heard` renders the input: the video's line before the words, naming the watcher when there is one; the video itself goes on only when there is none, kept for life only in a watcher's first input.
6. A Claude or GPT path ignores `BridgeInput.video`; the chat path ignores it unless `profile.endpoint.video()`.
7. On a video endpoint, before the request, each video it will show without an upload that has more than an hour left is uploaded (raw, then read until ACTIVE); the request waits in the task's select, so Escape stops the turn and its uploads. Each result is a `razor_upload` item the next response records.
8. The user message holds each input's images, its `razor_video` part shown as the upload's `uri` (or the failure's sentence), then its text. Google asks for the text after the video; this is that order.
9. When the turn ends done, `spend` turns every part without `keep` into its line, so the next turn's requests drop about 95 tokens a second of video (measured: 832 to 162 prompt tokens for an 8-second clip). A stopped or failed turn keeps them for the retry. A resumed conversation starts spent; a kept part's recorded upload is reused while it has more than an hour left.
10. A turn that a rebuild opens (a model switch, a handover, or a restart that cannot resume) carries every unanswered or kept video ahead of the rebuilt context, wherever its input sits, to a model path that takes video, and a watcher keeps its video after any number of questions; a video admitted in the batch that switches the model comes with its input after the context, so it is still seen in its turn.
11. The watcher ends its turn with its report as its last message, and that clean turn end is its report (it is told not to call `agent finish`, which would leave a tool call last and make its next turn rebuild instead of resume). Follow-up questions go by `message` while it works, and by `agent continue` or the person's input after; each resumes its native conversation with the video, uploaded once.

## What the phone sends

The contract:

- The phone sends `.mp4`, HEVC at 1080p, through the files route, and `ClientInput.video = {file, duration_ms}` with the words in `text`, never the path in `files` as well.
- It sends `video` only where `server.video` is true and falls back to `files` elsewhere.
- "Watching…" matches the helper row's `launched` to the video input's entry, as the phone already does for helper rows. `video.watcher` names the same child.
- No poster upload: the phone makes its own still; other clients use the first frame through the files route.
- The 32 MiB body bound stays; the phone lowers resolution and bitrate as length grows (at 1080p HEVC it reaches 32 MiB at about 40 seconds). Gemini samples one frame a second at about 70 tokens a frame, so 720p loses it nothing. Trey kept the bound (2026-11-01: "Sure to both").
- A watcher saves its stills beside the video, `files/<hex>/frames/<MM-SS>.png` in the parent's folder, and names them in its report; any client can fetch them through the files route.

## Hard decision 1: how video reaches Gemini

What Google documents: video goes to `generateContent` (or the Interactions API) as inline data under 100 MB or as a Files API `file_data` reference; the Files API keeps a file 48 hours, up to 2 GB a file and 20 GB a project, and a file is PROCESSING until ACTIVE (or FAILED); a part's `videoMetadata.fps` and `media_resolution` set the sampling. Gemini 3 counts 70 tokens a frame at the default resolution (280 at high) and 25 a second of sound, at one frame a second. The OpenAI-compatibility page documents images and audio and no video part.

What the chat route actually does (scratch requests, 2026-11-01): an `image_url` holding a `data:video/mp4;base64,…` URL or the Files API `uri` is taken as video (an 8-second clip, 752 prompt tokens either way), and works through a tool round trip with thought signatures. Parts typed `file`, `input_video` or `video_url` are refused 400. Nothing on this route sets the frame rate or resolution: `video_metadata` beside or inside the part is ignored (same tokens, and a 0.2-second red flash at 0:04 was missed every time), and `extra_body.google` refuses `video_metadata`, `media_resolution` and `generation_config` as unknown fields. Uploads work in one raw request; the Files API takes the key only as `x-goog-api-key` (a bearer is 401). A chat request naming a file still PROCESSING waited for it but reported 0 prompt tokens; an unknown or expired file is 403, which Razor's chat route would read as a bad key.

| | Added lines | What it buys and costs |
| --- | ---: | --- |
| **Chat route, Files API `uri` as `image_url`** (chosen) | about 130 (upload 45, task 40, conversation 30, endpoint 8, sniff 8) | One wire, as `generic.md` decided. One upload per video per 47 hours, however many requests show it; the record holds the reference, not the bytes. Sound included. Loses frame-rate and resolution control: Gemini sees one frame a second at its default resolution. |
| Inline `data:` URL on the chat route | about 30 | No upload code, but the whole video, a third larger in base64, goes in every request of its turn (each tool call repeats it), against Google's 20 MB guidance for inline requests and the request cap; a kept video would ride in every later request too. |
| Native `generateContent` beside chat for Gemini | about 530, `generic.md`'s count for the adapter, plus the upload | Frame rate, resolution, clipping and agentic processing per part. But a request is one shape or the other, so "the video part only" is the whole second wire: contents and parts, `systemInstruction`, function declarations and responses, `thinkingConfig`, a second stream reader and signatures in native form. Google now leads with the Interactions API and labels `generateContent` legacy. |
| One native `generateContent` call per video, outside the conversation, at a high frame rate | about 60 plus the upload | Dense sampling for the first description only; follow-up questions would go to a conversation that still sees one frame a second, and it is a second path with its own failures. |

The chosen route meets every part of the ruling but one: "For short clips it samples densely (Gemini lets the caller set the frame rate)." Over chat it cannot. Instead the watcher's text tells it how to look closer with its shell, which finds what one frame a second misses: `ffmpeg -vf freezedetect` reports frozen spans to the frame, `mpdecimate` reports repeated (dropped) frames, a scene filter reports flashes, and frames cut at the video's own rate around a moment are images it reads at full resolution (Gemini sees a frame at about 70 tokens, enough to misread small text: it read a frame counter of 914 as 911). Trey agreed to ffmpeg for quick moments, with no native frame-rate call (2026-11-01: "Sure to both"); native frame rate stays the 530-line adapter, built when a measured case needs it.

**Why the record holds the upload.** A watcher's conversation closes at its report and reopens for each follow-up, and every restart opens a fresh task, so an upload kept only in memory would upload the video again for each question. Each upload, first or renewed, is a `razor_upload` item {path, uri, expires} added to the items the next response records, as `razor_tools` declarations are; the part itself stays {path, keep}, so a recorded part is never changed after the fact. `stale` uploads again only with an hour or less of the 48 left. Expiry is decided from Google's own `expirationTime`, never from a failed request, since that failure is the 403 the chat route reports as a key error.

## Hard decision 2: the watcher's shape

| | What it costs | What it gives |
| --- | --- | --- |
| **A Razor child on the configured Gemini model, started by the server from the input path** (chosen) | About 85 lines in children (the decision, the name, the brief, the notice), a `video` field on the start, and a three-line controller accessor, paid for by `rendered` moving into `heard` | Everything a child has: a status row the phone draws as a circle, `launched` as the "Watching…" link, its report as an input, mail while it works, `agent continue` and the person's input after, the shell for ffmpeg and frames, the record, stop, the cap and the tree channel. |
| A lighter helper: one Gemini request, its text admitted to the parent as a program's input | About 60 lines, plus the same upload | No row (the phone would need another "Watching…" signal), no shell (no frames unless Razor runs ffmpeg itself), and no follow-ups: nothing keeps the video and a conversation about it. Each is more code to add back. |
| The parent model starts it, told to by the input's line | No server code | A parent turn spent first, on whether the model does as told; ruled out. |

- **Who owns it:** the parent session; its record holds the Started fact, and the watcher counts toward `children.max` while it works. If the cap is full, the start is refused and the parent is told so in Razor's input.
- **Who settles it:** the parent, as any child, though it works in place and has nothing to merge, so settling only records the decision; a finished child left unsettled holds nothing and counts toward no cap.
- **How it ends:** its report, by its clean turn end with no mail sent (`docs/design/children.md`); its text asks for exactly that and not `agent finish`, so its last response is text and a follow-up resumes the native conversation. A follow-up continues it; the person's input to a finished child continues it too (Trey, 2026-10-31). It keeps the video for its life, every rebuild included.
- **No Gemini key on that server** (every server runs this: Trey's ruling): no watcher starts, and the input is admitted with the video's line alone, so the agent still has the file and can cut frames to look at. The phone sends the same input either way.
- **The upload fails** (network, FAILED, the five-minute limit): the request goes on with the line "The video {path} could not be sent to the model: {why}." in place of the video. A watcher reports what it could do with ffmpeg; a Gemini session tells the person, who can resend. A failed upload is tried again at the next turn, never twice in one.
- **The parent session ends first:** as any child. Escape on the parent does not stop it. Closing the parent closes its descendants with it (`Controller::close`): `/done`'s last turn, `/quit`, and a stop or settle by the parent's own parent close the watcher, and a watcher closed before its report reports nothing; its inputs wait until it is continued.
- **The description is machine-made:** the parent's line says so before the report arrives, and the watcher's text asks it to open the report with "Machine-made description of {path}; it can miss or misread things." The original video stays where it was uploaded; frames go beside it.

The watcher's text (`src/children/text/watcher.md`, data) says: whose video it is and what they were doing (the person's words, the parent's name, project and newest reply); that it sees the video at one frame a second with sound and keeps it while it runs; what to report (a short description, then a timeline of the moments that bear on the person's words, `MM:SS` and one line each, a still saved at each into `frames/` with ffmpeg and named on its line); how to look closer at quick things with ffmpeg; the machine-made opening line; and that its parent may come back with questions about a moment.

## Flows

`flows/54-video.py`, against the stand-in, with a fake MP4 (an `ftyp` box and filler; nothing decodes it):

- **A Gemini session** (`[models."gemini-3.8-flash"] base_url = "{standin}/v1beta/openai"`): the video input is answered at once; the stand-in sees one raw upload with `x-goog-api-key` and the file's bytes, then a state read answered ACTIVE, then a chat request whose user message holds the video as `image_url` with the stand-in's `uri`, before the words and after the line naming the path. A second input: no upload, and the earlier message holds the spent line instead of the video. A third session's upload answered FAILED: its request carries the "could not be sent" line.
- **A GPT session** (Responses stand-in) with the key set: the same request sent twice at once is admitted once and starts one watcher, with no failure to start; the input is recorded with `video.watcher = "watcher-1"`; the parent's request holds the watching line and the path and no video; a child row with `launched` = the input's entry runs on `gemini-3.8-flash` in the video's folder. Its first chat request holds the upload's video and a brief with the person's words, the parent's name and project, and its newest reply. Its report reaches the parent's record. The person's input to the finished watcher continues it: its next request resumes (no rebuild, no call) and still holds the video, with no new upload.
- **Rebuilds:** a GPT session's video input that names `gemini-3.8-flash` rebuilds the context, and the rebuilt request holds the video; a watcher asked four questions, then switched to another Gemini row, still has its video and its first input's words in the rebuilt request.
- **Two videos at once** to one session take `watcher-1` and `watcher-2`, and neither start fails. A FAILED upload whose reason echoes the key leaves the key out of the record and the request.
- **No key** (the Gemini row's `key` set to an unset variable, live): the same input to a GPT session with `GEMINI_API_KEY` unset is admitted with the path line, no watcher and no upload. A text file named `video.mp4` is refused 400 before admission. `server.video` is true, and the files route serves `.mp4` as `video/mp4`.

The stand-in answers a GET from replies of that method (`Reply.method`), for the state read, and the flow counts only Razor's own requests, since other programs on the machine may knock on its port. Existing flows these seams touch: 03-image (images now made absolute in `heard`), 11 to 16 and 40 (children), 25-api, 34-rebuild-admission (the rebuild's use of `heard`), 42-mail-tree, 46-generic (the chat route), 50-voice (the input route).
