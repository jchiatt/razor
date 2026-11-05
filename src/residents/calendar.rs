//! Times of day and weekdays in local time, and when a set of them last and next comes due,
//! through libc's `localtime_r` and `mktime`.

use super::{Day, Time};
use crate::protocol::Millis;

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `at` as a fresh run's thread is named for it, in local time: "Nov 1, 05:00".
pub(crate) fn label(at: Millis) -> String {
    let Some(tm) = local(at) else {
        return String::new();
    };
    let month = MONTHS.get(tm.tm_mon as usize).unwrap_or(&"");
    format!("{month} {}, {:02}:{:02}", tm.tm_mday, tm.tm_hour, tm.tm_min)
}

/// The time among `at` on `days` (empty: every day) nearest `now`: the latest at or before it
/// (`back`), or the earliest after it. None when there is none within a week.
pub(crate) fn due(at: &[Time], days: &[Day], now: Millis, back: bool) -> Option<Millis> {
    let today = local(now)?;
    for k in 0..=7 {
        let offset = if back { -k } else { k };
        let found = (at.iter())
            .filter_map(|time| on(&today, offset, *time))
            .filter(|(_, wday)| includes(days, *wday))
            .map(|(ms, _)| ms)
            .filter(|ms| if back { *ms <= now } else { *ms > now });
        let best = if back { found.max() } else { found.min() };
        if best.is_some() {
            return best;
        }
    }
    None
}

/// Each time among `at` on `days` after `now` and at most a day later, soonest first and each
/// once, as `due` reads it, on each of the three dates such a day can reach (a day a clock
/// change shortens included).
pub(crate) fn ahead(at: &[Time], days: &[Day], now: Millis) -> Vec<Millis> {
    let Some(today) = local(now) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for k in 0..=2 {
        let (start, end) = (on(&today, k, Time(0)), on(&today, k + 1, Time(0)));
        let (Some((start, wday)), Some((end, _))) = (start, end) else {
            continue;
        };
        // A date of 24 hours has no clock change: each time is that many seconds past its
        // midnight, on its weekday. On another date a time is read, weekday too, as `due` reads
        // it; one a skipped hour carries past the next midnight is kept only if the scheduler,
        // which looks back from each wake, would fire it.
        let fires = |ms: Millis| ms < end || due(at, days, ms, true) == Some(ms);
        let read = |t: &Time| match end - start == super::DAY {
            true => Some((start + t.0 as Millis * 1000, wday)),
            false => on(&today, k, *t).filter(|(ms, _)| fires(*ms)),
        };
        let found = at
            .iter()
            .filter_map(read)
            .filter(|(ms, wday)| *ms > now && *ms <= now + super::DAY && includes(days, *wday));
        out.extend(found.map(|(ms, _)| ms));
    }
    // Two times a skipped hour carries to the same moment come due once.
    out.sort_unstable();
    out.dedup();
    out
}

/// Whether weekday `wday` (0 is Sunday) is among `days`; empty is every day.
fn includes(days: &[Day], wday: i32) -> bool {
    days.is_empty() || days.iter().any(|d| (*d as i32 + 1) % 7 == wday)
}

/// The local date and time of `now`.
fn local(now: Millis) -> Option<libc::tm> {
    let secs = (now / 1000) as libc::time_t;
    // SAFETY: an all-zero `tm` is a valid value (integers and a null zone pointer), and
    // `localtime_r` writes only into it.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let ok = unsafe { !libc::localtime_r(&secs, &mut tm).is_null() };
    ok.then_some(tm)
}

/// `time` on the day `offset` days from `today`, as milliseconds and its weekday (0 is Sunday).
/// It is read as standard time and as summer time, and the earlier reading that shows that time
/// again wins, so a time that occurs twice is always its first occurrence. A time a clock
/// change skips has no such reading, and comes due as standard time reads it. (`mktime` alone
/// would answer either way for a time that occurs twice, by its own last guess.)
fn on(today: &libc::tm, offset: i32, time: Time) -> Option<(Millis, i32)> {
    let read = |isdst| {
        let mut tm = libc::tm {
            tm_mday: today.tm_mday + offset,
            tm_hour: (time.0 / 3600) as i32,
            tm_min: (time.0 / 60 % 60) as i32,
            tm_sec: (time.0 % 60) as i32,
            tm_isdst: isdst,
            ..*today
        };
        // SAFETY: `mktime` reads `tm` and normalizes it in place; nothing else is touched.
        let secs = unsafe { libc::mktime(&mut tm) };
        let ms = (secs >= 0).then_some(secs as Millis * 1000)?;
        let shown = local(ms)
            .is_some_and(|t| (t.tm_hour * 3600 + t.tm_min * 60 + t.tm_sec) as u32 == time.0);
        Some((ms, tm.tm_wday, shown))
    };
    let (standard, summer) = (read(0)?, read(1));
    let pick = match summer {
        Some(s) if s.2 && (!standard.2 || s.0 < standard.0) => s,
        _ => standard,
    };
    Some((pick.0, pick.1))
}
