//! Meeting detection: platform signals in, clean events out.
//!
//! `state` is the pure, unit-tested state machine; `meet_url` classifies
//! Google Meet tab URLs; `macos` produces real signals. `Detector` wires one
//! `AppDetector` per conferencing app to a signal source and runs the poll loop.

pub mod meet_url;
pub mod state;

#[cfg(target_os = "macos")]
pub mod macos;

use std::collections::HashMap;
use std::time::Duration;

use state::{AppDetector, Debounce, DetectorEvent, MeetingApp, SignalSnapshot};

/// Poll cadence while at least one conferencing app is running.
pub const ACTIVE_POLL: Duration = Duration::from_secs(2);
/// Poll cadence while nothing we watch is running (NSWorkspace launch
/// notifications snap us back to the active cadence immediately).
pub const IDLE_POLL: Duration = Duration::from_secs(15);

/// Anything that can produce per-app signal snapshots (macOS in production,
/// scripted fakes in tests).
pub trait SignalSource {
    fn poll(&mut self) -> HashMap<MeetingApp, SignalSnapshot>;
}

#[cfg(target_os = "macos")]
impl SignalSource for macos::MacSignalSource {
    fn poll(&mut self) -> HashMap<MeetingApp, SignalSnapshot> {
        macos::MacSignalSource::poll(self)
    }
}

/// Multiplexes signal snapshots into per-app detectors and aggregates events.
pub struct Detector<S: SignalSource> {
    source: S,
    apps: Vec<AppDetector>,
}

impl<S: SignalSource> Detector<S> {
    pub fn new(source: S) -> Self {
        Detector {
            source,
            apps: vec![
                AppDetector::new(MeetingApp::Zoom, Debounce::default()),
                // Teams and Meet have no meeting-only process marker, so their
                // markers are mic-corroborated (see detect::macos) and their
                // detectors require the mic signal.
                AppDetector::new(
                    MeetingApp::Teams,
                    Debounce { require_mic_for: true, ..Debounce::default() },
                ),
                AppDetector::new(
                    MeetingApp::Meet,
                    Debounce { require_mic_for: true, ..Debounce::default() },
                ),
            ],
        }
    }

    /// Whether any watched app is currently running (drives poll cadence).
    pub fn any_app_running(&self) -> bool {
        self.apps.iter().any(|a| a.phase() != state::MeetingPhase::Idle)
    }

    /// One tick: poll signals, advance every app detector, return all events.
    pub fn tick(&mut self) -> Vec<DetectorEvent> {
        let snapshots = self.source.poll();
        let mut events = Vec::new();
        for det in &mut self.apps {
            let snap = snapshots.get(det.app()).copied().unwrap_or_default();
            events.extend(det.tick(snap));
        }
        events
    }

    /// The cadence the caller should sleep before the next tick.
    pub fn next_poll_interval(&self) -> Duration {
        if self.any_app_running() {
            ACTIVE_POLL
        } else {
            IDLE_POLL
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ScriptedSource {
        frames: Vec<HashMap<MeetingApp, SignalSnapshot>>,
        at: usize,
    }

    impl SignalSource for ScriptedSource {
        fn poll(&mut self) -> HashMap<MeetingApp, SignalSnapshot> {
            let frame = self.frames[self.at.min(self.frames.len() - 1)].clone();
            self.at += 1;
            frame
        }
    }

    fn frame(zoom: (bool, bool), mic: bool) -> HashMap<MeetingApp, SignalSnapshot> {
        HashMap::from([
            (
                MeetingApp::Zoom,
                SignalSnapshot { app_running: zoom.0, meeting_marker: zoom.1, mic_in_use: mic },
            ),
            (
                MeetingApp::Teams,
                SignalSnapshot { app_running: false, meeting_marker: false, mic_in_use: mic },
            ),
            (
                MeetingApp::Meet,
                SignalSnapshot { app_running: false, meeting_marker: false, mic_in_use: mic },
            ),
        ])
    }

    #[test]
    fn full_zoom_meeting_lifecycle_through_detector() {
        let frames = vec![
            frame((false, false), false), // idle
            frame((true, false), false),  // zoom launched
            frame((true, true), true),    // meeting marker tick 1
            frame((true, true), true),    // meeting marker tick 2 → started
            frame((true, true), true),
            frame((true, false), false), // marker gone x5 → ended
            frame((true, false), false),
            frame((true, false), false),
            frame((true, false), false),
            frame((true, false), false),
            frame((false, false), false), // zoom quit
        ];
        let mut d = Detector::new(ScriptedSource { frames, at: 0 });

        let mut all = Vec::new();
        for _ in 0..11 {
            all.extend(d.tick());
        }
        let zoom_events: Vec<_> = all
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    DetectorEvent::AppLaunched(MeetingApp::Zoom)
                        | DetectorEvent::MeetingStarted(MeetingApp::Zoom)
                        | DetectorEvent::MeetingEnded(MeetingApp::Zoom)
                        | DetectorEvent::AppQuit(MeetingApp::Zoom)
                )
            })
            .collect();
        assert_eq!(
            zoom_events,
            vec![
                &DetectorEvent::AppLaunched(MeetingApp::Zoom),
                &DetectorEvent::MeetingStarted(MeetingApp::Zoom),
                &DetectorEvent::MeetingEnded(MeetingApp::Zoom),
                &DetectorEvent::AppQuit(MeetingApp::Zoom),
            ]
        );
    }

    #[test]
    fn poll_interval_switches_with_app_presence() {
        let frames = vec![frame((false, false), false), frame((true, false), false)];
        let mut d = Detector::new(ScriptedSource { frames, at: 0 });
        d.tick();
        assert_eq!(d.next_poll_interval(), IDLE_POLL);
        d.tick();
        assert_eq!(d.next_poll_interval(), ACTIVE_POLL);
    }
}
