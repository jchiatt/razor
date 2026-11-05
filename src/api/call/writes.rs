//! Ordered call facts, hand-offs, choices and charges, off the socket loop.

use super::super::{App, ready, voice};
use crate::profiles;
use crate::protocol::{Body, CallEvent, Input, SessionId, Source, VoiceCallInput};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::sync::mpsc;

/// The call's writes, in order.
pub(super) enum Work {
    /// Record a fact; the person's words are those of a line of theirs, for the next hand-off.
    Fact(CallEvent, Option<String>),
    /// Admit the person's lines since the last hand-off, if any.
    HandOff,
    /// Charge the call's cumulative seconds beyond those already charged.
    Usage(f64),
    /// The phone's `choose`, held for the next hand-off once it is found to run.
    Choose(Value),
}

/// The worker's line for the phone, and whether the day's count has reached its cap.
pub(super) type Report = (Value, bool);

/// Does the call's writes in order, off its loop: records each fact, holds the person's choice,
/// admits each hand-off with the choice held, and charges the most seconds GPT-Live has reported
/// beyond those already paid, reporting each charge and choice back. A failed charge waits for
/// the next report, and the end for two more tries; a charge whose answer was lost may be paid
/// twice, since the count is a cap. Stops once the end is recorded.
pub(super) async fn work(
    app: App,
    id: SessionId,
    call: String,
    mut queue: mpsc::Receiver<Work>,
    reports: mpsc::Sender<Report>,
    device: Option<String>,
) {
    let log = |why: String| eprintln!("razor: live call {call} on {id}: {why}");
    // The person's lines since the last hand-off: the newest 256, about 1 MiB.
    let (mut heard, mut used, mut paid) = (Vec::new(), 0.0_f64, 0.0);
    // The person's choice for the next hand-off, as an input: after it the session's own hold.
    let mut chosen = Input::default();
    while let Some(work) = queue.recv().await {
        let ended = matches!(work, Work::Fact(CallEvent::Ended { .. }, _));
        let handed = matches!(work, Work::HandOff);
        // The end waits for the call's last charge: two more tries, a second apart.
        for _ in 0..2 {
            if !ended || used <= paid {
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
            if let Some(why) = charge(&app, used, &mut paid, &reports).await {
                log(why);
            }
        }
        let failed = match work {
            Work::Fact(event, said) => {
                let body = Body::VoiceCall {
                    call: call.clone(),
                    source: Source::Call,
                    event,
                };
                let appended = app.controller.append(&id, body).await;
                if let (Ok(entry), Some(text)) = (&appended, said) {
                    heard.drain(..heard.len().saturating_sub(255));
                    heard.push((*entry, text));
                }
                appended.err().map(|e| e.to_string())
            }
            Work::HandOff => None,
            Work::Usage(seconds) => {
                used = used.max(seconds);
                charge(&app, used, &mut paid, &reports).await
            }
            // A refusal leaves the choice held before it. As with charges, a reply is dropped only
            // past 16 unread, which only a phone flooding choices brings about.
            Work::Choose(command) => {
                let line = match choose(&app, &id, &command).await {
                    Ok(choice) => {
                        chosen = choice;
                        json!({"type": "chose", "model": chosen.model, "effort": chosen.effort, "handover": chosen.handover})
                    }
                    Err(sentence) => json!({"type": "refused", "sentence": sentence}),
                };
                let _ = reports.try_send((line, false));
                None
            }
        };
        // The words wait for the next hand-off until admitted. A choice the session no longer
        // takes costs them nothing: they go without it, and the phone hears why.
        let admitted = if handed && !heard.is_empty() {
            let lines = heard.iter().map(|(entry, _)| *entry).collect();
            let words: Vec<_> = heard.iter().map(|(_, text)| text.as_str()).collect();
            let voice_call = Some(Box::new(VoiceCallInput {
                id: call.clone(),
                lines,
            }));
            let mut input = Input {
                voice_call,
                device: device.clone(),
                text: words.join(" "),
                ..chosen.clone()
            };
            let mut admitted = app.controller.admit(&id, input.clone()).await;
            if let Err(refusal) = &admitted
                && chosen.model.is_some()
            {
                let refused = json!({"type": "refused", "sentence": refusal.to_string()});
                (input.model, input.effort, input.handover) = (None, None, None);
                admitted = app.controller.admit(&id, input).await;
                if admitted.is_ok() {
                    let _ = reports.try_send((refused, false));
                }
            }
            if admitted.is_ok() {
                (heard, chosen) = (Vec::new(), Input::default());
            }
            admitted.err()
        } else {
            None
        };
        failed
            .into_iter()
            .chain(admitted.map(|e| e.to_string()))
            .for_each(log);
        if ended {
            return;
        }
    }
}

/// The phone's choice of model, effort and handover, each optional, as the hand-off input it shapes,
/// checked as that input would be, the model's credentials included; or why it cannot run.
async fn choose(app: &App, id: &SessionId, command: &Value) -> Result<Input, String> {
    let fields = json!([command["model"], command["effort"], command["handover"]]);
    let mut choice = Input::default();
    (choice.model, choice.effort, choice.handover) = serde_json::from_value(fields)
        .map_err(|_| "Choose a model by name, or an effort of low, medium, high, xhigh or max.")?;
    choice.model = choice.model.map(profiles::id);
    if let Some(model) = &choice.model {
        let config = app.controller.config().await;
        let unready = ready::model(&app.home, &config, model).await;
        unready.map_or(Ok(()), Err)?;
        let checked = app.controller.choosable(id, model.clone()).await;
        checked.map_err(|e| e.to_string())?;
    }
    Ok(choice)
}

/// Charges the seconds `used` beyond those `paid` to the day's count and reports it back; or why
/// the owner refused.
async fn charge(
    app: &App,
    used: f64,
    paid: &mut f64,
    reports: &mpsc::Sender<Report>,
) -> Option<String> {
    if used <= *paid {
        return None;
    }
    let charge = voice::Reserve {
        characters: 0,
        requested: false,
        seconds: used - *paid,
    };
    match voice::ask(app, Some(charge)).await {
        Ok(granted) => {
            *paid = used;
            let (today, day) = (granted.settings.call_used_today, granted.settings);
            let line = json!({"type": "usage", "seconds": used, "today": today});
            let _ = reports.try_send((line, today >= day.call_daily_dollars));
            None
        }
        Err(refusal) => Some(refusal.1),
    }
}
