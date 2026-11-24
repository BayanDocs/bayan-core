//! The restart allowance ([engine protocol specification][spec] §12.1 step 1, §14.2): at most three restarts in any ten minutes, counted once, for both shells, by the engine's bindings.
//!
//! When the engine discards its session after a panic, it sends `engine.error` with `recoverable: false`. Its binding, the native engine thread ([`crate::thread`], which `bayan-ffi` drives) or the web worker host (through `bayan-wasm`), then applies [`RestartAllowance::failure`] at the monotonic time of the job that failed, and adds the result to that `engine.error` as `restart` with [`add_restart`]: `{ number, limit }` while the allowance lasts, and the shell starts a new session; `null` once it is used up, and the engine stops and answers nothing more.
//!
//! The allowance is a pure function of the times the bindings pass: the engine reads no clock (ADR-0012 decision 5). It lives in the binding, outside the engine's session, so it is not part of a recording's `initial_state`, and the binding adds `restart` only after the engine has digested what it sent, so no recording holds it and a recording that spans a restart replays with every digest equal (spec §14.2 item 4).
//!
//! [spec]: https://github.com/BayanDocs/docs/blob/HEAD/specs/engine-protocol.md

use serde::{Deserialize, Serialize};

use crate::protocol::{EngineError, Restart};

/// How many restarts the allowance grants in any ten minutes.
pub const RESTART_LIMIT: u32 = 3;

/// How long a restart counts, in milliseconds: it counts while it is less than 600,000 ms (ten minutes) old, and no longer from exactly 600,000 ms.
pub const RESTART_WINDOW_MS: u64 = 600_000;

/// Which restart a failure at `now_ms` is, given the times of the earlier restarts, or `None` if the allowance is used up and the engine stops. A restart at `t` counts while `now_ms − t` is less than [`RESTART_WINDOW_MS`]; a time after `now_ms` (which a monotonic clock never gives) counts too.
#[must_use]
pub fn decide(earlier: &[u64], now_ms: u64) -> Option<Restart> {
    let counted = earlier
        .iter()
        .filter(|&&time| now_ms.saturating_sub(time) < RESTART_WINDOW_MS)
        .count();
    let counted = u32::try_from(counted).unwrap_or(u32::MAX);
    (counted < RESTART_LIMIT).then_some(Restart {
        number: counted + 1,
        limit: RESTART_LIMIT,
    })
}

/// The restarts that a binding has granted: their monotonic times, in milliseconds on the binding's clock.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RestartAllowance {
    /// The times of the restarts that may still count, oldest first; at most [`RESTART_LIMIT`].
    times: Vec<u64>,
}

impl RestartAllowance {
    /// An allowance with no restarts yet.
    #[must_use]
    pub const fn new() -> Self {
        Self { times: Vec::new() }
    }

    /// An allowance with the times of earlier restarts, as a binding that keeps them outside the engine hands them over: the web worker host, whose engine instance a panic replaces with all its state. Only the latest [`RESTART_LIMIT`] times are kept; earlier ones can never count again.
    #[must_use]
    pub fn with_times(times: &[u64]) -> Self {
        let mut times = times.to_vec();
        times.sort_unstable();
        let limit = usize::try_from(RESTART_LIMIT).unwrap_or(usize::MAX);
        let excess = times.len().saturating_sub(limit);
        times.drain(..excess);
        Self { times }
    }

    /// The times of the restarts that may still count, oldest first.
    #[must_use]
    pub fn times(&self) -> &[u64] {
        &self.times
    }

    /// The engine discarded its session at `now_ms`: the restart that follows, which is then counted, or `None` if the allowance is used up and the engine stops.
    pub fn failure(&mut self, now_ms: u64) -> Option<Restart> {
        let restart = decide(&self.times, now_ms)?;
        self.times
            .retain(|&time| now_ms.saturating_sub(time) < RESTART_WINDOW_MS);
        self.times.push(now_ms);
        Some(restart)
    }
}

/// An event as the engine writes it, for [`add_restart`].
#[derive(Serialize, Deserialize)]
struct Event {
    v: u32,
    #[serde(rename = "type")]
    kind: String,
    seq: u64,
    payload: EngineError,
}

/// The longest `engine.error` that reports a discarded session: it holds only a code, a message identifier and no arguments, so anything longer is another message.
const MAX_DISCARD_EVENT_BYTES: usize = 1024;

/// Adds `restart` to the `engine.error` with `recoverable: false` that ends `messages`, the messages the engine sent for the call in which it discarded its session ([`crate::Engine::take_session_discarded`]): that event is always the last of them. Returns whether it found the event.
pub fn add_restart(messages: &mut [String], restart: Option<Restart>) -> bool {
    let Some(last) = messages.last_mut() else {
        return false;
    };
    if last.len() > MAX_DISCARD_EVENT_BYTES {
        return false;
    }
    let Ok(mut event) = serde_json::from_str::<Event>(last) else {
        return false;
    };
    if event.kind != "engine.error" || event.payload.recoverable {
        return false;
    }
    event.payload.restart = Some(restart);
    match serde_json::to_string(&event) {
        Ok(json) => {
            *last = json;
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbers(allowance: &mut RestartAllowance, times: &[u64]) -> Vec<Option<u32>> {
        times
            .iter()
            .map(|&time| allowance.failure(time).map(|restart| restart.number))
            .collect()
    }

    #[test]
    fn three_restarts_in_ten_minutes_then_the_engine_stops() {
        let mut allowance = RestartAllowance::new();
        assert_eq!(
            numbers(&mut allowance, &[0, 1_000, 2_000, 3_000]),
            [Some(1), Some(2), Some(3), None]
        );
        assert_eq!(
            decide(&[], 5),
            Some(Restart {
                number: 1,
                limit: 3
            })
        );
    }

    #[test]
    fn a_restart_counts_until_it_is_exactly_ten_minutes_old() {
        // 599,999 ms old: still counted, so the fourth failure stops the engine.
        let mut allowance = RestartAllowance::new();
        assert_eq!(
            numbers(&mut allowance, &[1_000, 2_000, 3_000, 600_999]),
            [Some(1), Some(2), Some(3), None]
        );
        // 600,000 ms old: no longer counted, so the fourth failure is restart 3 of 3.
        let mut allowance = RestartAllowance::new();
        assert_eq!(
            numbers(&mut allowance, &[1_000, 2_000, 3_000, 601_000]),
            [Some(1), Some(2), Some(3), Some(3)]
        );
        assert_eq!(allowance.times(), [2_000, 3_000, 601_000]);
    }

    #[test]
    fn the_allowance_refills_as_restarts_leave_the_window() {
        let mut allowance = RestartAllowance::new();
        assert_eq!(
            numbers(&mut allowance, &[0, 300_000, 900_000, 1_000_000, 1_200_000]),
            [Some(1), Some(2), Some(1), Some(2), Some(3)]
        );
    }

    #[test]
    fn handed_over_times_keep_counting() {
        let mut allowance = RestartAllowance::with_times(&[50, 10, 30, 20]);
        assert_eq!(allowance.times(), [20, 30, 50]);
        assert_eq!(allowance.failure(60), None);
        // A clock that ran backwards counts the earlier restarts as new.
        assert_eq!(
            decide(&[1_000], 0),
            Some(Restart {
                number: 2,
                limit: 3
            })
        );
    }

    #[test]
    fn restart_is_added_to_the_last_engine_error_only() {
        let event = r#"{"v":1,"type":"engine.error","seq":4,"payload":{"code":"panic","message_id":"engine-error-panic","recoverable":false,"args":{}}}"#;
        let mut messages = vec![
            r#"{"v":1,"re":3,"ok":false,"error":{"code":"panic","message_id":"engine-error-panic","args":{}}}"#.to_owned(),
            event.to_owned(),
        ];
        assert!(add_restart(
            &mut messages,
            Some(Restart {
                number: 2,
                limit: 3
            })
        ));
        assert_eq!(
            messages[1],
            r#"{"v":1,"type":"engine.error","seq":4,"payload":{"code":"panic","message_id":"engine-error-panic","recoverable":false,"args":{},"restart":{"number":2,"limit":3}}}"#
        );
        let mut stopped = vec![event.to_owned()];
        assert!(add_restart(&mut stopped, None));
        assert!(
            stopped[0].ends_with(r#""args":{},"restart":null}}"#),
            "{}",
            stopped[0]
        );
        // Anything else is left alone.
        let recoverable = event.replace("false", "true");
        let mut others = vec![recoverable.clone()];
        assert!(!add_restart(&mut others, None));
        assert_eq!(others, [recoverable]);
        assert!(!add_restart(&mut [], None));
    }
}
