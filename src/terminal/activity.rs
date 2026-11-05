//! Local terminal input, plus Mac login input, selects one player at the shared voice owner.
use super::{App, audio};
use reqwest::Method;
use tokio::time::*;

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventSourceSecondsSinceLastEventType(state: i32, event: u32) -> f64;
}

/// Reports local input at most once per fifteen seconds, only when this machine can play.
/// One token identifies this process across all agents and directly paired servers.
pub fn report(app: &mut App) {
    if !app.audio.play || std::env::var_os("SSH_CONNECTION").is_some() {
        return;
    }
    if app
        .audio
        .reported
        .is_some_and(|at| at.elapsed() < Duration::from_secs(15))
    {
        return;
    }
    let body = serde_json::json!({"terminal": *audio::RUN, "active": true});
    if app.spawn(|api| async move {
        let _ = api.json(Method::PUT, "v1/voice/activity", Some(body)).await;
        |_: &mut App| {}
    }) {
        app.audio.reported = Some(Instant::now());
    }
}

/// Every fifteen seconds, reports Mac login input since the previous sample, even outside
/// Razor. Sampling and the request run beside the frame loop; dropping the channel stops it.
#[cfg(target_os = "macos")]
pub fn start(tx: tokio::sync::mpsc::Sender<super::Done>) {
    if std::env::var_os("SSH_CONNECTION").is_some() {
        return;
    }
    tokio::spawn(async move {
        let mut last = Instant::now();
        let mut timer = interval_at(last + Duration::from_secs(15), Duration::from_secs(15));
        timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            tokio::select! { _ = tx.closed() => break, _ = timer.tick() => {} }
            let idle = tokio::task::spawn_blocking(|| {
                std::env::var("RAZOR_IDLE_SECONDS")
                    .ok()
                    .and_then(|s| s.parse::<f64>().ok())
                    // Combined login session (0), any keyboard/mouse/tablet event (~0).
                    .unwrap_or_else(|| unsafe {
                        CGEventSourceSecondsSinceLastEventType(0, u32::MAX)
                    })
            })
            .await;
            let elapsed = last.elapsed().as_secs_f64();
            last = Instant::now();
            if idle.is_ok_and(|idle| idle >= 0.0 && idle < elapsed)
                && tx.send(Box::new(report)).await.is_err()
            {
                break;
            }
        }
    });
}
