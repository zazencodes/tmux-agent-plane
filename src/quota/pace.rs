//! Pace and runway, ported from quota-axi's pace.ts: how fast a window is being used against
//! its reset clock, and whether it lasts until the reset.

use super::{Role, Time, Window};
use serde::Serialize;

/// Reserve within this many points of zero is on pace.
const ON_PACE_DEADBAND: f64 = 1.0;

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum PaceStatus {
    /// Using faster than the clock: less left than time left.
    Ahead,
    OnPace,
    Behind,
}

#[derive(Serialize, Clone, PartialEq, Debug)]
pub struct Pace {
    pub status: PaceStatus,
    /// Share of the cycle still to run: where linear use would leave the bar.
    pub time_remaining_percent: f64,
    pub elapsed_percent: f64,
    /// Remaining percent minus time remaining percent.
    pub reserve_percent_points: f64,
    /// Used percent over elapsed percent.
    pub burn_multiple: Option<f64>,
    /// When the window empties if use continues at the cycle's average rate.
    pub projected_exhausted_at: Option<Time>,
    pub cycle_seconds: f64,
}

/// The current cycle: `resets_at − starts_at` when the start is known, else `window_seconds`
/// ending at `resets_at`. Unknown otherwise, or once reset, or before the cycle opens.
fn cycle(window: &Window, now: Time) -> Option<(f64, Time, Time)> {
    let resets = window.resets_at.filter(|r| *r > now)?;
    let starts = match window.starts_at {
        Some(starts) => starts,
        None => resets - chrono::Duration::seconds(i64::try_from(window.window_seconds?).ok()?),
    };
    if starts >= resets || starts > now {
        return None;
    }
    Some((millis(resets - starts) / 1000.0, starts, resets))
}

fn millis(duration: chrono::TimeDelta) -> f64 {
    duration.num_milliseconds() as f64
}

/// A window's pace at `now`. None while stale, without usage, or without a known cycle.
pub fn pace(window: &Window, now: Time, stale: bool) -> Option<Pace> {
    if stale {
        return None;
    }
    let used = window.percent_used?;
    let remaining = 100.0 - used;
    let (cycle_seconds, starts, resets) = cycle(window, now)?;
    let cycle_ms = cycle_seconds * 1000.0;
    let elapsed_ms = millis(now - starts);
    let time_remaining = 100.0 * millis(resets - now) / cycle_ms;
    let elapsed = 100.0 * elapsed_ms / cycle_ms;
    let reserve = remaining - time_remaining;
    let status = if reserve.abs() <= ON_PACE_DEADBAND {
        PaceStatus::OnPace
    } else if reserve < 0.0 {
        PaceStatus::Ahead
    } else {
        PaceStatus::Behind
    };
    let projected_exhausted_at = (used > 0.0 && elapsed_ms > 0.0)
        .then(|| {
            let ms_to_empty = remaining / (used / elapsed_ms);
            chrono::TimeDelta::try_milliseconds(ms_to_empty as i64)
                .and_then(|d| now.checked_add_signed(d))
        })
        .flatten();
    Some(Pace {
        status,
        time_remaining_percent: time_remaining,
        elapsed_percent: elapsed,
        reserve_percent_points: reserve,
        burn_multiple: (elapsed > 0.0).then(|| used / elapsed),
        projected_exhausted_at,
        cycle_seconds,
    })
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Runway {
    /// A bounding window is at zero.
    ExhaustedNow,
    /// No bounding window has a reset, or one cannot be measured.
    Unknown,
    /// The earliest projected exhaustion comes before its own reset.
    EmptyAt(Time),
    /// Every bounding window lasts to its reset.
    OnPace,
}

impl Runway {
    pub fn as_str(self) -> &'static str {
        match self {
            Runway::ExhaustedNow => "exhausted_now",
            Runway::Unknown => "unknown",
            Runway::EmptyAt(_) => "empty_in",
            Runway::OnPace => "on_pace",
        }
    }
}

fn zero_use(window: &Window) -> bool {
    window.percent_used == Some(0.0)
}

/// The verdict over a group's bounding windows, in order: any at zero, no resets at all, any
/// unmeasurable, the earliest exhaustion before its reset, else on pace.
pub fn runway(bounds: &[&Window], now: Time, stale: bool) -> Runway {
    if bounds.iter().any(|w| w.remaining() == Some(0.0)) {
        return Runway::ExhaustedNow;
    }
    if bounds.iter().all(|w| w.resets_at.is_none()) {
        return Runway::Unknown;
    }
    let mut earliest: Option<Time> = None;
    for window in bounds {
        let Some(resets) = window.resets_at else {
            // A cycle that has not started yet (no reset, nothing used) does not block.
            if zero_use(window) {
                continue;
            }
            return Runway::Unknown;
        };
        let Some(pace) = pace(window, now, stale) else {
            return Runway::Unknown;
        };
        if zero_use(window) {
            continue;
        }
        let Some(empty) = pace.projected_exhausted_at.filter(|at| *at > now) else {
            return Runway::Unknown;
        };
        if empty < resets {
            earliest = Some(earliest.map_or(empty, |e| e.min(empty)));
        }
    }
    earliest.map_or(Runway::OnPace, Runway::EmptyAt)
}

/// The headline of a group: the least left over its bounding windows, the window that sets it,
/// and the runway.
pub struct Headline<'a> {
    pub remaining: Option<f64>,
    pub limiting: Option<&'a Window>,
    pub runway: Runway,
}

/// Unknown when the group has no bounding window, one without a percent, or a window we do not
/// understand (it might bound everything).
pub fn headline<'a>(windows: &[&'a Window], now: Time, stale: bool) -> Headline<'a> {
    let unknown = Headline {
        remaining: None,
        limiting: None,
        runway: Runway::Unknown,
    };
    if windows.iter().any(|w| w.role == Role::Unknown) {
        return unknown;
    }
    let bounds: Vec<&Window> = windows
        .iter()
        .copied()
        .filter(|w| w.role == Role::Bound)
        .collect();
    if bounds.is_empty() {
        return unknown;
    }
    let runway = runway(&bounds, now, stale);
    if bounds.iter().any(|w| w.percent_used.is_none()) {
        return Headline { runway, ..unknown };
    }
    let limiting = bounds
        .iter()
        .copied()
        .min_by(|a, b| a.remaining().unwrap().total_cmp(&b.remaining().unwrap()))
        .unwrap();
    Headline {
        remaining: limiting.remaining(),
        limiting: Some(limiting),
        runway,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quota::tests::{at, window};

    const HOUR: i64 = 3600;

    #[test]
    fn pace_from_window_seconds_ending_at_reset() {
        // A 5h window, 1h in, 40% used: 80% of the time left but only 60% of the allowance.
        let w = window("five_hour", 40.0, Some(5 * HOUR), Some(18_000));
        let p = pace(&w, at(HOUR), false).unwrap();
        assert!((p.time_remaining_percent - 80.0).abs() < 1e-9);
        assert!((p.elapsed_percent - 20.0).abs() < 1e-9);
        assert!((p.reserve_percent_points - -20.0).abs() < 1e-9);
        assert!((p.burn_multiple.unwrap() - 2.0).abs() < 1e-9);
        assert_eq!(p.status, PaceStatus::Ahead);
        // 40% per hour: the remaining 60% lasts 1.5h.
        assert_eq!(p.projected_exhausted_at, Some(at(HOUR + 5400)));
    }

    #[test]
    fn pace_prefers_the_known_start_and_rejects_bad_cycles() {
        let mut w = window("credits", 50.0, Some(10 * HOUR), None);
        w.starts_at = Some(at(0));
        let p = pace(&w, at(5 * HOUR), false).unwrap();
        assert_eq!(p.cycle_seconds, 36_000.0);
        assert_eq!(p.status, PaceStatus::OnPace);
        assert!(pace(&w, at(10 * HOUR), false).is_none(), "reset passed");
        assert!(pace(&w, at(HOUR), true).is_none(), "stale");
        w.starts_at = Some(at(2 * HOUR));
        assert!(pace(&w, at(HOUR), false).is_none(), "cycle not open yet");
        let w = window("no_cycle", 50.0, Some(10 * HOUR), None);
        assert!(pace(&w, at(HOUR), false).is_none(), "duration unknown");
    }

    #[test]
    fn runway_verdicts_in_order() {
        let now = at(HOUR);
        let slow = window("week", 10.0, Some(5 * HOUR), Some(18_000));
        let fast = window("session", 40.0, Some(5 * HOUR), Some(18_000));
        let empty = window("gone", 100.0, Some(5 * HOUR), Some(18_000));
        let resetless = window("resetless", 30.0, None, Some(18_000));
        let untouched = window("untouched", 0.0, None, Some(18_000));
        let unmeasurable = window("unmeasurable", 30.0, Some(5 * HOUR), None);

        assert_eq!(
            runway(&[&slow, &empty, &resetless], now, false),
            Runway::ExhaustedNow
        );
        assert_eq!(runway(&[&resetless], now, false), Runway::Unknown);
        assert_eq!(runway(&[&slow, &unmeasurable], now, false), Runway::Unknown);
        assert_eq!(runway(&[&slow, &resetless], now, false), Runway::Unknown);
        assert_eq!(runway(&[&slow, &untouched], now, false), Runway::OnPace);
        assert_eq!(
            runway(&[&slow, &fast], now, false),
            Runway::EmptyAt(at(HOUR + 5400))
        );
        assert_eq!(
            runway(&[&slow], now, true),
            Runway::Unknown,
            "stale pace is unknown"
        );
    }

    #[test]
    fn headline_is_the_least_left_over_bounding_windows() {
        let now = at(HOUR);
        let session = window("session", 40.0, Some(5 * HOUR), Some(18_000));
        let week = window("week", 10.0, Some(100 * HOUR), Some(604_800));
        let mut model = window("model:opus", 90.0, Some(100 * HOUR), Some(604_800));
        model.role = Role::Model;
        let h = headline(&[&session, &week, &model], now, false);
        assert_eq!(h.remaining, Some(60.0));
        assert_eq!(h.limiting.unwrap().id, "session");
        assert!(matches!(h.runway, Runway::EmptyAt(_)));

        let mut odd = window("odd", 5.0, Some(5 * HOUR), None);
        odd.role = Role::Unknown;
        let h = headline(&[&session, &odd], now, false);
        assert_eq!(h.remaining, None);
        assert_eq!(h.runway, Runway::Unknown);
    }
}
