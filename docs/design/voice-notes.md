# Voice notes: a spoken input, transcribed before it is admitted

Design page, 2026-11-01, for voice notes from the phone and the terminal (Trey approved the feature on 2026-11-01; the terminal's recording is built against this same surface).

Three rules carry it:

- **A voice note is a typed input whose words a program wrote.** It is admitted through the same route and the same `Controller::admit`, after its transcript exists; from admission on, nothing tells it apart but one record field.
- **Transcription is a configured program, as `speech` is.** Razor hands it the audio on standard input and reads the transcript from standard output. It knows no vendor, model or key.
- **The audio is an upload, and the record names it.** The existing files route stores and serves it, ranges included; the input carries a path, never bytes.

## Surface

```rust
// ---- src/protocol/mod.rs -----------------------------------------------------------------------

/// A spoken input's audio and how a client draws it; the input's `text` is its transcript. One
/// type on the wire and in the record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Voice {
    /// The audio, relative to the session's folder, as `POST /v1/sessions/{id}/files/{name}`
    /// returned it (AAC in `.m4a` from the phone; Razor does not read the format).
    pub file: PathBuf,
    /// Its length as the recording client measured it; clients show it, and it sets the
    /// transcription's time limit.
    pub duration_ms: u64,
    /// The note's loudness from start to end in equal steps, 0 silent to 255 loudest, at most 64
    /// values (the phone sends 48); kept and returned, never read. A client drawing another
    /// client's note resamples to its own width; empty draws as a flat line.
    pub levels: Vec<u8>,
}

pub struct Input {
    // ... as today, plus:
    /// Set only on a voice note; replay, catch-up, pending and taken lines carry it like any field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<Voice>,
}

// ---- src/protocol/api.rs -----------------------------------------------------------------------

pub struct ClientInput {
    // ... as today, plus:
    /// A voice note: the input route answers only once its transcript is made and the input
    /// admitted. `text` should be empty; whatever it holds, a handed skill's block included,
    /// comes before the transcript.
    #[serde(default)]
    pub voice: Option<Voice>,
}

pub struct Server {
    // ... as today, plus:
    /// This home transcribes voice notes: its `transcribe` is set. Filled by the API on the way
    /// out, like the rest of `Server`, so a paired server's own value reaches every client.
    pub transcribe: bool,
}

// ---- src/config/mod.rs -------------------------------------------------------------------------

pub struct Config {
    // ... as today, plus, a machine key (refused in a project file, like `speech`):
    /// A program and its arguments that read one voice note's audio on standard input and write
    /// its transcript to standard output; a failure exits nonzero with a sentence as standard
    /// error's last line. Empty: voice notes are refused.
    pub transcribe: Vec<String>,
}

// ---- src/api/speech.rs (a reply read aloud, and now a voice note heard) ------------------------

/// Admits `input`, a voice note, once the `transcribe` program has written its words, which are
/// appended to its text. The work runs in a task of its own that owns the file's claim through
/// admission and that the request awaits, so a client that leaves does not stop it. Two things
/// keep a note from being transcribed or admitted twice, and only these: the claim, which
/// refuses the same file while its program runs; and the request id, which a repeat check reads
/// before the claim and again under it, answering the first entry while that id is among the
/// session's newest 64. A note with no request id has no protection once admitted, and a server
/// restart during transcription admits nothing, so its resend transcribes again.
/// Each refusal before admission names one state, in one sentence, and admitted nothing:
/// 409 "This voice note is still being transcribed." (that note's program is running);
/// 429 "Four voice notes and replies are being processed; send it again in a moment." (nothing
/// started); 503 "This server has no transcriber; set `transcribe`.";
/// 500 "Couldn't transcribe it: <the program's last line of standard error>." or "…: it took
/// longer than <n> seconds." or "…: the program exited with <status>." or "…: no words were
/// heard." or "…: its words were not UTF-8." or "…: it wrote more than 1 MiB.". The upload is
/// still there to resend. An error from admission itself is uncertain, since admission records
/// the input before its last step can fail: the client resends with the same request id.
pub(super) async fn admit(app: &App, id: &SessionId, input: Input) -> Result<EntryId, Refusal>;

/// Runs a configured program in a process group of its own with `stdin` (and `text` written to
/// it), draining standard output and standard error together with the writer under one time
/// limit: at most `cap` bytes of output, refused with a sentence past it rather than cut, and
/// the last 4 KiB of standard error. The whole process group is killed on every way out but a
/// clean exit (`shell::Group`). Ok only on exit 0; an error is one sentence. Shared by speech
/// and transcription.
async fn run(command: &[String], stdin: Stdio, text: Option<String>, cap: usize, limit: Duration)
    -> io::Result<Vec<u8>>;

/// One program running for one file of one session, a reply's audio by its path and a voice
/// note by its device and inode, so no other spelling of a path is a second note: at most four
/// at once across speech and transcription, each file once. Made with no await between the check and the guard, and
/// dropped by its owner after admission (or after keeping speech's audio) frees the place.
struct Claim(SessionId, String);

// ---- src/children/message.rs -------------------------------------------------------------------

/// As today, taking the input and moving its text out (both callers own it and keep only its
/// images): a voice note's words are led by one line from `children/text/mail.toml`, "(Voice
/// note, transcribed by machine; some words may be misheard.)", for both model paths and the
/// rebuilt context. The record never holds the lead.
pub fn heard(me: &Summary, dir: &Path, input: &mut Input) -> String;
```

**Routes.** No new route.

| request | change |
| --- | --- |
| `POST /v1/sessions/{id}/files/{name}` | none: the client uploads the audio's raw bytes, e.g. as `voice.m4a`, and gets `{"path": "files/<16 hex>/voice.m4a"}` |
| `POST /v1/sessions/{id}/input` | `ClientInput.voice`; answers `{entry}` once transcribed and admitted; 409, 429, 500 and 503 as above; more than 64 levels is 400 |
| `GET /v1/sessions/{id}/files/{voice.file}` | playback, with byte ranges as today; a name ending `.m4a` is served as `audio/mp4` instead of `application/octet-stream` |
| `GET /v1/status`, every `agents` line | `server.transcribe`, and each paired server's in `servers` |

A route `GET /v1/sessions/{id}/voice/{entry}` was the obvious alternative. The files route already serves the same file with the same range code, and every client holds the path from the record, so a second route would only look the path up again. The one change it needs is the audio type, without which AVPlayer may refuse the file.

## Data flow

1. The client uploads the audio through the files route: raw bytes, one fresh folder per upload, at most the 32 MiB body bound (about two hours at 32 kbit/s).
2. It sends the input with `voice` and a request id. The route reads the repeat check, claims `(session, voice.file)`, then reads the repeat check again under the claim: a resend during transcription gets 409, one after admission gets the first entry, even one that lands the moment the claim drops, and an admitted note never sees 429. A claim refused because four programs are running is 429: nothing started.
3. The route builds the `Input` exactly as for typed words (origin, model, effort, skill, request id), with `voice` set.
4. `speech::admit` opens the file through `Controller::file`, which already refuses a path outside the session's folder or through a link, and hands that open file to the program as its standard input: no audio bytes pass through Razor's memory.
5. A task of its own, owning the claim, runs the program with a limit of five minutes plus the note's length (its length counted up to three hours), takes at most 1 MiB of UTF-8 transcript (more, or other bytes, is a failure, not a cut), trims it, and appends it to the input's text.
6. The same task calls `Controller::admit`, unchanged: the size cap, the plugins' input event (which sees the transcript), the record, the pending queue, delivery at the next tool boundary, Take Back and the draft all apply as for typed words. The claim drops after admission.
7. The request returns `{entry}`; a client that left meanwhile finds the entry through its retry.
8. Rendering for a model, `children::heard` puts the lead before the words of an input with `voice`, on both model paths and in a rebuild's verbatim asks.
9. The record keeps the transcript as `text` and `{file, duration_ms, levels}` as `voice`; clients draw the waveform from `levels` and play `file` through the files route.
10. The terminal marks such an input's row "voice note"; that mark belongs to the terminal's own design.

## What a client can show, and how it knows

Every state a client draws comes from an answer it holds, never a guess, and the same answers serve the phone and the terminal:

| state | how the sending client knows | what it may do |
| --- | --- | --- |
| uploading | the files request is in flight | on failure, upload again; nothing reached the session |
| transcribing | the input request is in flight; after a lost connection, resending the same request gets 409 | wait; resend later to learn the outcome |
| busy | 429 | resend in a moment; nothing started |
| failed | 500 with its sentence (503 when the server has no transcriber) | show the sentence; resend the same file, with no new upload |
| admitted | `{entry}`, or the same entry from a resend | from here, exactly a typed input: pending while the agent works, taken, taken back |
| unknown | a dropped connection, a 502 from a paired server, or any other error | resend with the same request id: 409, the first entry, or a fresh attempt |

Other clients attached to the session see the note from its admission on, as a typed input appears when it is admitted; they do not see it while it is transcribed.

## Where a typed input's path is reused, and where it forks

Reused unchanged: the upload route; request-id validation and the repeat check; origin, paired-server origin, model, effort and skill handling; `Controller::admit` and everything after it (plugins' input event, size cap, record append, pending, tool-boundary delivery, Take Back, taken, the queue while the agent works, draft clearing); the stream and entry pages, replay and catch-up; forwarding to a paired server (the session routes' layer); recaps (they read the transcript). The 20 pictures-and-files count does not include the audio, and admission's inside-the-folder check does not need to, since the route opened the file through `Controller::file`.

Forked, in one place: the input route, for an input with `voice`, claims the file, then admits through `speech::admit` (transcribe, then the same `Controller::admit`) instead of calling `Controller::admit` directly. Model rendering adds one lead line. The terminal's mark belongs to the terminal's own design.

What keeps a note single, stated exactly: the claim excludes a second run of the same file while one runs; the request id deduplicates admission while it is among the session's newest 64 ids (`Summary::repeat`). An input with no request id has no protection after admission, as a typed one has none; and a server restart during transcription admits nothing, so the resend transcribes again.

## The hard decision: inline base64, or upload then reference

| | Inline base64 in `ClientInput` | Upload, then reference (chosen) |
| --- | --- | --- |
| Retry idempotence | A retry resends the whole audio (9.3 MB of base64 for half an hour) before the repeat check can run, since the JSON body is read whole first. The in-flight claim can only key on the request id, because each attempt stores a fresh copy; with no request id, every retry transcribes again. | The request that waits on transcription is about 1 KB, so retrying it over a bad connection costs nothing. The claim keys on the uploaded file itself, so a retry of the same note is caught during transcription even without a request id, and after admission by the request id. A dropped upload is retried alone and may leave an unused folder, as uploads do today. |
| Request size | Base64 adds a third: the 32 MiB body holds about 24 MB of audio, about 1 h 40 min at 32 kbit/s, and needs its own cap and sentence beside `IMAGE_BASE64`. | Raw bytes under the same 32 MiB bound: about 2 h 20 min. No new cap. |
| Memory held | The body, the parsed string and the decoded audio overlap: about 19 to 26 MB for a half-hour note, per request, held until the audio is written. | The upload holds its 7 MB body while writing, as the files route already does. During transcription Razor holds no audio: the program reads the open file. |
| Lines | A second wire type with base64 audio beside the record's path (one concept, two definitions: RULES 5), its decode, cap and store: about +18. | The record's `Voice` is the wire type: protocol +2 for the field, API 0 for storage. |
| Cost to the client | One request. | Two requests, the same two the phone already makes to send a file. |

Upload-then-reference wins on every row but the round trip, and the round trip is one the phone already makes for files.

**`heard` takes `&mut Input` and moves the text out,** rather than gaining a `voice` argument. The extra argument would push the controller's call past rustfmt's line width and add about five lines to a full row; both callers own the input and keep only its images afterwards, so nothing is copied.

## Flow

One new flow, with `transcribe` set to a stand-in shell program that writes a fixed transcript and counts its runs: a voice note is admitted with the transcript as its text and `voice` in the record and on replay; the stand-in model's request has the lead and the record does not; a retry during transcription gets 409 and after it the first entry, with one run counted; a failing program's last line reaches a 500; an unset `transcribe` refuses with 503 and `server.transcribe` is false; playback through the files route answers a byte range as `audio/mp4`.

## Notes for the contract and the docs

- **Glossary line:** **voice note**: an input the person spoke: its audio kept in the session's folder, its text the transcript a configured program wrote.
- **SPEC:** speech is not in SPEC.md either. If these belong in the contract, one sentence in section 3's API paragraph: "A person may speak an input: the client uploads the audio, a configured program transcribes it before admission, and the record keeps the transcript as the input's text beside the audio."
- **Through a paired server**, forwarding gives up after 30 seconds with 502, while transcription on the owning server runs on; the client's retry with the same request id then gets 409 or the entry. Speech has the same limit.
- **The four-program cap is shared** with speech: a burst of read-alouds can refuse a voice note with 429 (nothing started; resend). One cap is one rule; separate caps are a second set.
- **The capability refreshes with the next `agents` line** after `transcribe` changes, as `brand` does; the route itself reads the key at every request.

## As built

Departures from the page:
- **The time limit is five minutes plus the note's length**, the speech program's own limit, not one minute: a transcriber may spend 90 seconds on its first engine before falling back to its second.
- **`admit` takes the input with its voice inside**, as the input route builds it, and admits a typed input directly, so the route has one call; `Input.voice` is boxed, since the record's entry enum would otherwise grow for every entry.
- **The sentence without a transcriber** is "This server has no transcriber; set `transcribe`."
- **The program's standard error** reaches the server's log as its last line, for speech and transcription alike; speech's error was inherited whole before.
- **The claim keys a voice note on its opened file's device and inode**, not its path, after `files//…` was seen running a second time beside `files/…` (and case aliases would on macOS). The route opens the file before claiming it.
- **Typed words before a transcript stay on their own line**: a non-empty `text` is joined to the words with a newline.
- **Read-aloud's failures are 500 as well**: a speech program that fails, is missing, or makes no audio is 500 with its sentence; 503 only when `speech` is not set (Trey's ruling for program failures).
- **The terminal's mark** belongs to the terminal's own design, not this page's; none is built here.

Flow 50 (`flows/50-voice.py`) passes, and fails, each in turn, with the claim's in-flight refusal removed, with both repeat checks removed, with standard error's first line taken for its last, with the lead removed, with the claim keyed on the path (a `//` alias runs), with no line between typed words and the transcript, and with a claim never released (a fresh note after four claimed runs gets 429). A resend landing between the route's repeat check and the claim, the race the second check closes, cannot be timed from a flow, and is not shown red.

## One automatic playback destination

Trey approved one Razor speech mechanism on 2026-11-02: “plays on the Mac when at the terminal, phone otherwise.” The existing shared voice-settings owner also chooses the player, once before synthesis. Local terminals with `play` configured report input through `PUT /v1/voice/activity` with their process token; a false `active` removes it at close. The owner retains at most 64 tokens, expiring after sixty seconds, and selects the most recent report. A Mac's existing fifteen-second login-input sample covers input outside Razor; terminal key, mouse and paste events report on every platform, at most once per fifteen seconds.

A delivery carries the selected `terminal` token beside its existing `sound`; only that terminal auto-plays, including notes from other sessions and paired servers. All terminals still share the receipt after successful playback. Quiet hours select silence. With both phone and terminal present, the terminal wins; with no active terminal, the existing phone delivery remains, unread if no phone is present. Closing the selected terminal removes its report so another recent terminal may win the next note. A crash expires within sixty seconds. Routing stays fixed while speech is made and has no retry or claim queue.

The default permission becomes Always; Away remains an explicit choice that permits unsolicited speech only away from the computer. An existing install keeps its saved permission. The speech program keeps its own voice and credit reserve; no second synthesis or speech command is needed.

Extending per-session attachment would still miss notes from another agent and duplicate playback across terminals. A playback claim would add a request and a queue for every note. Selecting one terminal at the existing owner adds only bounded presence and one delivery field, with no new coordinator or persisted presence. Flow 57 proves terminal selection, cross-session and paired-server routing, phone fallback, and one player across simultaneous terminals through the real API and a speech/player stand-in. Mac login-input sampling is checked separately on macOS.
