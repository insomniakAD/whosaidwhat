//! Platform-independent meeting-detection state machine.
//!
//! The macOS layer (`detect::macos`) produces one [`SignalSnapshot`] per poll tick
//! per conferencing app; this module turns those noisy snapshots into clean
//! `MeetingStarted` / `MeetingEnded` events with debouncing.
//!
//! This module deliberately has **zero dependencies** (not even `std::time` for
//! logic — ticks are abstract) so it can be unit-tested on any host and reasoned
//! about in isolation. Debounce thresholds are tick counts; the poller runs at a
//! fixed interval (default 2s), so `enter_ticks = 2` means "signal held for ~4s".

/// The conferencing apps we watch. `Other` carries a stable key for future apps
/// configured at runtime (e.g. Webex) without a code change.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MeetingApp {
    Zoom,
    Teams,
    /// Google Meet has no desktop process; it is detected via browser + mic signals.
    Meet,
    Other(String),
}

impl MeetingApp {
    pub fn display_name(&self) -> &str {
        match self {
            MeetingApp::Zoom => "Zoom",
            MeetingApp::Teams => "Microsoft Teams",
            MeetingApp::Meet => "Google Meet",
            MeetingApp::Other(name) => name,
        }
    }
}

/// One poll tick's worth of evidence about a single app.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SignalSnapshot {
    /// The app's main process is running (or for Meet: a browser is running).
    pub app_running: bool,
    /// A meeting-specific marker is present (Zoom: `CptHost` child process;
    /// Teams: call-window/child-process marker; Meet: a browser tab/window title
    /// matching "Meet" — see detect::macos for exactly what feeds this).
    pub meeting_marker: bool,
    /// The default input device is in use system-wide (CoreAudio
    /// `kAudioDevicePropertyDeviceIsRunningSomewhere`). Shared across apps;
    /// used to corroborate weak markers and to time meeting end.
    pub mic_in_use: bool,
}

/// Lifecycle of a single app's meeting, as inferred from signals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeetingPhase {
    /// App not running (or no evidence of it).
    Idle,
    /// App running, no meeting detected yet.
    AppOpen,
    /// Meeting confirmed in progress.
    InMeeting,
}

/// Events emitted on phase transitions. The app layer maps these to
/// notifications ("Start recording?") and to the recorder's auto-stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetectorEvent {
    AppLaunched(MeetingApp),
    MeetingStarted(MeetingApp),
    MeetingEnded(MeetingApp),
    AppQuit(MeetingApp),
}

/// Debounce configuration, in poll ticks.
#[derive(Debug, Clone, Copy)]
pub struct Debounce {
    /// Consecutive ticks the meeting marker must be present to enter `InMeeting`.
    /// Guards against transient helper processes at app launch.
    pub enter_ticks: u32,
    /// Consecutive ticks the marker must be absent to leave `InMeeting`.
    /// Guards against marker flapping (e.g. Zoom's helper restarting between
    /// breakout rooms) so the recorder does not stop/save mid-meeting.
    pub exit_ticks: u32,
    /// For marker-weak apps (Meet), also require the mic to have been in use;
    /// meeting end additionally waits for the mic to go quiet.
    pub require_mic_for: bool,
}

impl Default for Debounce {
    fn default() -> Self {
        // At the default 2s poll interval: start after ~4s of evidence,
        // end after ~10s of absence. Ending too eagerly truncates recordings;
        // starting a touch late costs only seconds of preamble.
        Debounce { enter_ticks: 2, exit_ticks: 5, require_mic_for: false }
    }
}

/// Per-app detector: feed it one `SignalSnapshot` per tick, collect events.
#[derive(Debug)]
pub struct AppDetector {
    app: MeetingApp,
    debounce: Debounce,
    phase: MeetingPhase,
    marker_streak: u32,
    absence_streak: u32,
    mic_seen_during_meeting: bool,
}

impl AppDetector {
    pub fn new(app: MeetingApp, debounce: Debounce) -> Self {
        AppDetector {
            app,
            debounce,
            phase: MeetingPhase::Idle,
            marker_streak: 0,
            absence_streak: 0,
            mic_seen_during_meeting: false,
        }
    }

    pub fn phase(&self) -> MeetingPhase {
        self.phase
    }

    pub fn app(&self) -> &MeetingApp {
        &self.app
    }

    /// Advance one tick. Returns the events this tick produced (0..=2:
    /// a meeting end and an app quit can coincide).
    pub fn tick(&mut self, s: SignalSnapshot) -> Vec<DetectorEvent> {
        let mut events = Vec::new();

        // App-level transitions first.
        match (self.phase, s.app_running) {
            (MeetingPhase::Idle, true) => {
                self.phase = MeetingPhase::AppOpen;
                events.push(DetectorEvent::AppLaunched(self.app.clone()));
            }
            (MeetingPhase::AppOpen, false) => {
                self.phase = MeetingPhase::Idle;
                self.reset_streaks();
                events.push(DetectorEvent::AppQuit(self.app.clone()));
                return events;
            }
            (MeetingPhase::InMeeting, false) => {
                // App died mid-meeting (crash or quit): the meeting is over now,
                // regardless of debounce — the audio source is gone.
                self.phase = MeetingPhase::Idle;
                self.reset_streaks();
                events.push(DetectorEvent::MeetingEnded(self.app.clone()));
                events.push(DetectorEvent::AppQuit(self.app.clone()));
                return events;
            }
            _ => {}
        }

        // Meeting-level transitions.
        match self.phase {
            MeetingPhase::AppOpen => {
                let evidence = if self.debounce.require_mic_for {
                    s.meeting_marker && s.mic_in_use
                } else {
                    s.meeting_marker
                };
                if evidence {
                    self.marker_streak += 1;
                    if self.marker_streak >= self.debounce.enter_ticks {
                        self.phase = MeetingPhase::InMeeting;
                        self.absence_streak = 0;
                        self.mic_seen_during_meeting = s.mic_in_use;
                        events.push(DetectorEvent::MeetingStarted(self.app.clone()));
                    }
                } else {
                    self.marker_streak = 0;
                }
            }
            MeetingPhase::InMeeting => {
                self.mic_seen_during_meeting |= s.mic_in_use;
                let still_going = if self.debounce.require_mic_for {
                    // For marker-weak apps, either signal keeps the meeting alive.
                    s.meeting_marker || s.mic_in_use
                } else {
                    s.meeting_marker
                };
                if still_going {
                    self.absence_streak = 0;
                } else {
                    self.absence_streak += 1;
                    if self.absence_streak >= self.debounce.exit_ticks {
                        self.phase = MeetingPhase::AppOpen;
                        self.reset_streaks();
                        events.push(DetectorEvent::MeetingEnded(self.app.clone()));
                    }
                }
            }
            MeetingPhase::Idle => {}
        }

        events
    }

    fn reset_streaks(&mut self) {
        self.marker_streak = 0;
        self.absence_streak = 0;
        self.mic_seen_during_meeting = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(app: bool, marker: bool, mic: bool) -> SignalSnapshot {
        SignalSnapshot { app_running: app, meeting_marker: marker, mic_in_use: mic }
    }

    fn zoom() -> AppDetector {
        AppDetector::new(MeetingApp::Zoom, Debounce::default())
    }

    #[test]
    fn launch_then_meeting_with_debounce() {
        let mut d = zoom();
        let ev = d.tick(snap(true, false, false));
        assert_eq!(ev, vec![DetectorEvent::AppLaunched(MeetingApp::Zoom)]);
        // Marker must persist for enter_ticks (2) before MeetingStarted.
        assert!(d.tick(snap(true, true, true)).is_empty());
        let ev = d.tick(snap(true, true, true));
        assert_eq!(ev, vec![DetectorEvent::MeetingStarted(MeetingApp::Zoom)]);
        assert_eq!(d.phase(), MeetingPhase::InMeeting);
    }

    #[test]
    fn transient_marker_does_not_start_meeting() {
        let mut d = zoom();
        d.tick(snap(true, false, false));
        d.tick(snap(true, true, false)); // one-tick blip
        d.tick(snap(true, false, false)); // streak resets
        let ev = d.tick(snap(true, true, false));
        assert!(ev.is_empty(), "single tick after reset must not trigger");
        assert_eq!(d.phase(), MeetingPhase::AppOpen);
    }

    #[test]
    fn marker_flap_does_not_end_meeting() {
        let mut d = zoom();
        d.tick(snap(true, false, false));
        d.tick(snap(true, true, true));
        d.tick(snap(true, true, true)); // started
        // Marker drops for fewer than exit_ticks (5), then returns.
        for _ in 0..4 {
            assert!(d.tick(snap(true, false, true)).is_empty());
        }
        assert!(d.tick(snap(true, true, true)).is_empty());
        assert_eq!(d.phase(), MeetingPhase::InMeeting);
    }

    #[test]
    fn sustained_absence_ends_meeting() {
        let mut d = zoom();
        d.tick(snap(true, false, false));
        d.tick(snap(true, true, true));
        d.tick(snap(true, true, true));
        let mut ended = false;
        for _ in 0..5 {
            for e in d.tick(snap(true, false, false)) {
                if e == DetectorEvent::MeetingEnded(MeetingApp::Zoom) {
                    ended = true;
                }
            }
        }
        assert!(ended);
        assert_eq!(d.phase(), MeetingPhase::AppOpen);
    }

    #[test]
    fn app_quit_mid_meeting_ends_then_quits() {
        let mut d = zoom();
        d.tick(snap(true, false, false));
        d.tick(snap(true, true, true));
        d.tick(snap(true, true, true));
        let ev = d.tick(snap(false, false, false));
        assert_eq!(
            ev,
            vec![
                DetectorEvent::MeetingEnded(MeetingApp::Zoom),
                DetectorEvent::AppQuit(MeetingApp::Zoom),
            ]
        );
        assert_eq!(d.phase(), MeetingPhase::Idle);
    }

    #[test]
    fn meet_requires_mic_corroboration() {
        let mut d = AppDetector::new(
            MeetingApp::Meet,
            Debounce { require_mic_for: true, ..Debounce::default() },
        );
        d.tick(snap(true, false, false)); // browser running
        // Marker (Meet tab) without mic: not a joined meeting (could be lobby/tab open).
        assert!(d.tick(snap(true, true, false)).is_empty());
        assert!(d.tick(snap(true, true, false)).is_empty());
        assert_eq!(d.phase(), MeetingPhase::AppOpen);
        // Marker + mic for enter_ticks: meeting.
        d.tick(snap(true, true, true));
        let ev = d.tick(snap(true, true, true));
        assert_eq!(ev, vec![DetectorEvent::MeetingStarted(MeetingApp::Meet)]);
        // Tab still open but mic quiet counts toward absence only when both gone:
        // marker OR mic keeps it alive for require_mic_for apps.
        for _ in 0..10 {
            d.tick(snap(true, true, false));
        }
        assert_eq!(d.phase(), MeetingPhase::InMeeting, "open tab keeps meeting alive");
        // Tab closed + mic quiet → ends after exit_ticks.
        let mut ended = false;
        for _ in 0..5 {
            for e in d.tick(snap(true, false, false)) {
                if e == DetectorEvent::MeetingEnded(MeetingApp::Meet) {
                    ended = true;
                }
            }
        }
        assert!(ended);
    }

    #[test]
    fn relaunch_after_quit_detects_again() {
        let mut d = zoom();
        d.tick(snap(true, false, false));
        d.tick(snap(true, true, false));
        d.tick(snap(true, true, false));
        d.tick(snap(false, false, false)); // quit mid-meeting
        let ev = d.tick(snap(true, false, false));
        assert_eq!(ev, vec![DetectorEvent::AppLaunched(MeetingApp::Zoom)]);
        d.tick(snap(true, true, false));
        let ev = d.tick(snap(true, true, false));
        assert_eq!(ev, vec![DetectorEvent::MeetingStarted(MeetingApp::Zoom)]);
    }
}
