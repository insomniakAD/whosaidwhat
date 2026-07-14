//! Platform-independent recording-session lifecycle.
//!
//! Coordinates the user prompt, the recorder backend, and the detector's
//! auto-stop, without touching any OS API — the actual audio I/O lives behind
//! [`RecorderBackend`] (implemented by `capture::macos` with ScreenCaptureKit
//! for system audio + cpal for the mic, written as two separate tracks). Zero
//! external dependencies so it unit-tests anywhere.

use crate::detect::state::MeetingApp;

/// Outcome of asking the backend to finalize a recording. The two-track design
/// (see capture::macos) yields two WAVs; both paths are carried explicitly so
/// the pipeline never has to reconstruct one from the other by string surgery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedRecording {
    /// Absolute path of the system-audio track (remote participants), 48 kHz.
    pub system_path: String,
    /// Absolute path of the mic track (local user), if a mic track was written.
    pub mic_path: Option<String>,
    /// Duration in milliseconds as reported by the backend.
    pub duration_ms: u64,
}

/// The audio backend contract. `start` must be idempotent-safe (error if busy),
/// `stop` must flush and finalize the container so a crash right after still
/// leaves a playable file.
pub trait RecorderBackend {
    type Error: std::fmt::Display;

    fn start(&mut self, app: &MeetingApp, out_dir: &str, file_stem: &str) -> Result<(), Self::Error>;
    fn stop(&mut self) -> Result<SavedRecording, Self::Error>;
    fn is_recording(&self) -> bool;
}

/// What the session manager wants the UI/notification layer to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEffect {
    /// Show the "Meeting detected — start recording?" notification.
    PromptUser { app: MeetingApp },
    /// Remove a stale prompt (meeting ended before the user answered).
    DismissPrompt { app: MeetingApp },
    /// Recording finished and the file is safe on disk; hand off to the pipeline.
    RecordingSaved { app: MeetingApp, saved: SavedRecording },
    /// Surface a backend failure to the user (and log it).
    Error { message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Idle,
    /// Notification shown, waiting for the user (or for auto-record policy).
    Prompted,
    Recording,
}

/// Recording policy the user picks in settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordPolicy {
    /// Always ask via notification (default: consent-first).
    Prompt,
    /// Start recording as soon as a meeting is confirmed.
    Auto,
    /// Never record automatically; user starts from the app UI only.
    Manual,
}

pub struct SessionManager<B: RecorderBackend> {
    backend: B,
    policy: RecordPolicy,
    out_dir: String,
    state: SessionState,
    /// The app whose meeting the current prompt/recording belongs to.
    active_app: Option<MeetingApp>,
}

impl<B: RecorderBackend> SessionManager<B> {
    pub fn new(backend: B, policy: RecordPolicy, out_dir: String) -> Self {
        SessionManager { backend, policy, out_dir, state: SessionState::Idle, active_app: None }
    }

    pub fn state(&self) -> SessionState {
        self.state
    }

    /// Detector says a meeting started.
    pub fn on_meeting_started(&mut self, app: MeetingApp, file_stem: &str) -> Vec<SessionEffect> {
        match (self.state, self.policy) {
            (SessionState::Idle, RecordPolicy::Prompt) => {
                self.state = SessionState::Prompted;
                self.active_app = Some(app.clone());
                vec![SessionEffect::PromptUser { app }]
            }
            (SessionState::Idle, RecordPolicy::Auto) => self.start_recording(app, file_stem),
            (SessionState::Idle, RecordPolicy::Manual) => vec![],
            // Already prompted/recording for another meeting: one recording at a
            // time. The single system-audio capture picks up ALL concurrent
            // remote audio, so a second overlapping meeting is still captured
            // acoustically; but if it OUTLIVES the current recording it must be
            // re-offered. The detector's AppDetector for that app is already
            // InMeeting and will not re-emit MeetingStarted, so the run loop
            // re-checks Detector::active_meetings() whenever the session returns
            // to Idle (see main.rs / Detector::active_meetings).
            _ => vec![],
        }
    }

    /// User accepted the notification prompt (or pressed record in the UI).
    pub fn on_user_accept(&mut self, file_stem: &str) -> Vec<SessionEffect> {
        match self.state {
            SessionState::Prompted => {
                let app = self.active_app.clone().expect("prompted implies active_app");
                self.start_recording(app, file_stem)
            }
            // Manual start from the UI with no prompt outstanding.
            SessionState::Idle => {
                let app = MeetingApp::Other("manual".to_string());
                self.start_recording(app, file_stem)
            }
            SessionState::Recording => vec![],
        }
    }

    /// User dismissed the prompt.
    pub fn on_user_decline(&mut self) -> Vec<SessionEffect> {
        if self.state == SessionState::Prompted {
            self.state = SessionState::Idle;
            self.active_app = None;
        }
        vec![]
    }

    /// Detector says the meeting ended → auto-stop & save, or clear the prompt.
    pub fn on_meeting_ended(&mut self, app: &MeetingApp) -> Vec<SessionEffect> {
        match self.state {
            SessionState::Recording if self.active_app.as_ref() == Some(app) => self.stop_and_save(),
            SessionState::Prompted if self.active_app.as_ref() == Some(app) => {
                self.state = SessionState::Idle;
                self.active_app = None;
                vec![SessionEffect::DismissPrompt { app: app.clone() }]
            }
            _ => vec![],
        }
    }

    /// User pressed stop in the UI.
    pub fn on_user_stop(&mut self) -> Vec<SessionEffect> {
        if self.state == SessionState::Recording {
            self.stop_and_save()
        } else {
            vec![]
        }
    }

    fn start_recording(&mut self, app: MeetingApp, file_stem: &str) -> Vec<SessionEffect> {
        match self.backend.start(&app, &self.out_dir, file_stem) {
            Ok(()) => {
                self.state = SessionState::Recording;
                self.active_app = Some(app);
                vec![]
            }
            Err(e) => {
                self.state = SessionState::Idle;
                self.active_app = None;
                vec![SessionEffect::Error { message: format!("failed to start recording: {e}") }]
            }
        }
    }

    fn stop_and_save(&mut self) -> Vec<SessionEffect> {
        let app = self.active_app.take().expect("recording implies active_app");
        self.state = SessionState::Idle;
        match self.backend.stop() {
            Ok(saved) => vec![SessionEffect::RecordingSaved { app, saved }],
            Err(e) => vec![SessionEffect::Error { message: format!("failed to save recording: {e}") }],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeBackend {
        recording: bool,
        started: u32,
        fail_start: bool,
    }

    impl RecorderBackend for FakeBackend {
        type Error = String;

        fn start(&mut self, _app: &MeetingApp, _dir: &str, _stem: &str) -> Result<(), String> {
            if self.fail_start {
                return Err("device busy".into());
            }
            assert!(!self.recording, "start while recording");
            self.recording = true;
            self.started += 1;
            Ok(())
        }

        fn stop(&mut self) -> Result<SavedRecording, String> {
            assert!(self.recording, "stop while idle");
            self.recording = false;
            Ok(SavedRecording {
                system_path: "/tmp/x.system.wav".into(),
                mic_path: Some("/tmp/x.mic.wav".into()),
                duration_ms: 1234,
            })
        }

        fn is_recording(&self) -> bool {
            self.recording
        }
    }

    fn mgr(policy: RecordPolicy) -> SessionManager<FakeBackend> {
        SessionManager::new(FakeBackend::default(), policy, "/tmp".into())
    }

    #[test]
    fn prompt_accept_end_saves() {
        let mut m = mgr(RecordPolicy::Prompt);
        let fx = m.on_meeting_started(MeetingApp::Zoom, "zoom-2026-07-14");
        assert_eq!(fx, vec![SessionEffect::PromptUser { app: MeetingApp::Zoom }]);
        assert!(m.on_user_accept("zoom-2026-07-14").is_empty());
        assert_eq!(m.state(), SessionState::Recording);
        let fx = m.on_meeting_ended(&MeetingApp::Zoom);
        match &fx[0] {
            SessionEffect::RecordingSaved { app, saved } => {
                assert_eq!(*app, MeetingApp::Zoom);
                assert_eq!(saved.duration_ms, 1234);
            }
            other => panic!("expected RecordingSaved, got {other:?}"),
        }
        assert_eq!(m.state(), SessionState::Idle);
    }

    #[test]
    fn auto_policy_records_without_prompt() {
        let mut m = mgr(RecordPolicy::Auto);
        assert!(m.on_meeting_started(MeetingApp::Teams, "t").is_empty());
        assert_eq!(m.state(), SessionState::Recording);
    }

    #[test]
    fn meeting_end_before_answer_dismisses_prompt() {
        let mut m = mgr(RecordPolicy::Prompt);
        m.on_meeting_started(MeetingApp::Meet, "m");
        let fx = m.on_meeting_ended(&MeetingApp::Meet);
        assert_eq!(fx, vec![SessionEffect::DismissPrompt { app: MeetingApp::Meet }]);
        assert_eq!(m.state(), SessionState::Idle);
    }

    #[test]
    fn decline_then_end_is_quiet() {
        let mut m = mgr(RecordPolicy::Prompt);
        m.on_meeting_started(MeetingApp::Zoom, "z");
        m.on_user_decline();
        assert!(m.on_meeting_ended(&MeetingApp::Zoom).is_empty());
    }

    #[test]
    fn overlapping_meeting_is_ignored_while_recording() {
        let mut m = mgr(RecordPolicy::Auto);
        m.on_meeting_started(MeetingApp::Zoom, "z");
        assert!(m.on_meeting_started(MeetingApp::Teams, "t").is_empty());
        // Ending the *other* app's meeting must not stop our recording.
        assert!(m.on_meeting_ended(&MeetingApp::Teams).is_empty());
        assert_eq!(m.state(), SessionState::Recording);
        // Our meeting ending does stop it.
        let fx = m.on_meeting_ended(&MeetingApp::Zoom);
        assert!(matches!(fx[0], SessionEffect::RecordingSaved { .. }));
    }

    #[test]
    fn backend_failure_surfaces_error_and_resets() {
        let mut m = SessionManager::new(
            FakeBackend { fail_start: true, ..Default::default() },
            RecordPolicy::Auto,
            "/tmp".into(),
        );
        let fx = m.on_meeting_started(MeetingApp::Zoom, "z");
        assert!(matches!(fx[0], SessionEffect::Error { .. }));
        assert_eq!(m.state(), SessionState::Idle);
    }

    #[test]
    fn manual_start_and_stop() {
        let mut m = mgr(RecordPolicy::Manual);
        assert!(m.on_meeting_started(MeetingApp::Zoom, "z").is_empty());
        m.on_user_accept("manual-rec");
        assert_eq!(m.state(), SessionState::Recording);
        let fx = m.on_user_stop();
        assert!(matches!(fx[0], SessionEffect::RecordingSaved { .. }));
    }
}
