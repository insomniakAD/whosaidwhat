//! whosaidwhat daemon entry point (headless core; the Tauri shell mounts the
//! same wiring behind commands/events — see docs/00-architecture.md §Wiring).
//!
//! macOS run loop:
//!   detector poll ──events──► session manager ──effects──► prompt / recorder
//!        ▲                                                        │
//!        └──── self_recording flag (per-process mic checks) ◄─────┘
//!   RecordingSaved ─► pipeline::worker (background thread, own DB connection)
//!                        └─► pipeline::process_recording ─► SQLite + notification
//!
//! The pipeline runs on a dedicated worker thread (pipeline::worker), so the
//! detection loop keeps polling while a recording is transcribed — a meeting
//! that starts mid-processing is detected and recorded on time.

fn main() {
    #[cfg(not(target_os = "macos"))]
    {
        eprintln!(
            "whosaidwhat's capture/detection layers are macOS-only. \
             The pure core (detection state machine, chunker, merge, DB, oMLX \
             client) still compiles and tests on this platform: cargo test"
        );
    }

    #[cfg(target_os = "macos")]
    macos_main::run();
}

#[cfg(target_os = "macos")]
mod macos_main {
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    use whosaidwhat::capture::macos::{MacRecorder, SckSystemAudio};
    use whosaidwhat::capture::session::{RecordPolicy, SessionEffect, SessionManager, SessionState};
    use whosaidwhat::config::Config;
    use whosaidwhat::db::Store;
    use whosaidwhat::detect::state::{DetectorEvent, MeetingApp};
    use whosaidwhat::detect::{macos::MacSignalSource, Detector};
    use whosaidwhat::llm::router::InferenceRouter;
    use whosaidwhat::pipeline::worker::{Job, PipelineWorker};

    pub fn run() {
        let config_path = Config::default().data_dir.join("config.json");
        let config = Config::load_or_default(&config_path);
        let _ = config.save(&config_path); // materialize defaults on first run

        let mut store = Store::open(&config.db_path()).expect("open database");

        // Pipeline worker: its OWN Store connection, router, and runtime, all
        // constructed on the worker thread. The detection loop only creates
        // the meeting row (milliseconds) and queues a Job; WAL + busy_timeout
        // (db.rs) make the two writers safe.
        let worker_db_path = config.db_path();
        let worker_inference = config.inference.clone();
        let worker = PipelineWorker::spawn(
            move || {
                let store = Store::open(&worker_db_path).expect("open database (worker)");
                let router = InferenceRouter::new(worker_inference);
                let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
                (store, router, runtime)
            },
            |(store, router, runtime), job: Job| {
                let mut progress = |stage: &str, done: usize, total: usize| {
                    tracing::info!("pipeline {stage}: {done}/{total}");
                };
                let result = runtime.block_on(whosaidwhat::pipeline::run_with_default_engines(
                    store,
                    router,
                    &job.meeting_id,
                    &job.system_wav,
                    job.mic_wav.as_deref(),
                    &mut progress,
                ));
                match result {
                    Ok(summary_id) => {
                        tracing::info!(
                            "meeting {} summarized (summary {summary_id})",
                            job.meeting_id
                        );
                        notify_done(&job.meeting_id);
                    }
                    Err(e) => {
                        tracing::error!("pipeline failed for {}: {e}", job.meeting_id);
                        let _ = store.set_meeting_status(&job.meeting_id, "failed:pipeline");
                    }
                }
            },
        )
        .expect("spawn pipeline worker");

        let self_recording = Arc::new(AtomicBool::new(false));
        let source = MacSignalSource::new(self_recording.clone());
        let recorder = MacRecorder::new(SckSystemAudio::new(), self_recording);
        let mut detector = Detector::new(source);
        let mut session = SessionManager::new(
            recorder,
            RecordPolicy::from(config.record_policy),
            config.recordings_dir().display().to_string(),
        );

        tracing::info!("whosaidwhat watching for meetings (policy: {:?})", config.record_policy);

        loop {
            for event in detector.tick() {
                match event {
                    DetectorEvent::MeetingStarted(app) => {
                        let effects = start_for(&mut session, &app);
                        handle_effects(&mut store, &worker, effects);
                    }
                    DetectorEvent::MeetingEnded(app) => {
                        let effects = session.on_meeting_ended(&app);
                        handle_effects(&mut store, &worker, effects);
                        // A concurrent meeting may have been ignored while this
                        // one recorded; if the session is now free, re-offer any
                        // app still in a meeting (the detector won't re-emit).
                        if session.state() == SessionState::Idle {
                            for other in detector.active_meetings() {
                                let effects = start_for(&mut session, &other);
                                handle_effects(&mut store, &worker, effects);
                                if session.state() != SessionState::Idle {
                                    break; // one recording at a time
                                }
                            }
                        }
                    }
                    DetectorEvent::AppLaunched(app) => tracing::debug!("{} launched", app.display_name()),
                    DetectorEvent::AppQuit(app) => tracing::debug!("{} quit", app.display_name()),
                }
            }
            std::thread::sleep(detector.next_poll_interval());
        }
    }

    /// Apply the record policy to a detected meeting.
    ///
    /// Consent note: under `RecordPolicy::Prompt` the headless daemon has no UI
    /// to show a notification, so it does NOT record — it logs and waits. Only
    /// `RecordPolicy::Auto` records without a UI. The Tauri shell replaces this
    /// function's Prompt branch with `notify::WindowPrompt`, which shows the
    /// clickable "Start recording?" surface and calls `session.on_user_accept`
    /// on click. Silently auto-recording under Prompt would violate consent.
    fn start_for(
        session: &mut SessionManager<MacRecorder<SckSystemAudio>>,
        app: &MeetingApp,
    ) -> Vec<SessionEffect> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let stem = format!("{}-{}", app.display_name().to_lowercase().replace(' ', "-"), now);
        let effects = session.on_meeting_started(app.clone(), &stem);
        if effects.iter().any(|e| matches!(e, SessionEffect::PromptUser { .. })) {
            tracing::info!(
                "{} meeting detected — Prompt policy needs the app UI to consent; \
                 set record_policy=\"auto\" for headless capture. Not recording.",
                app.display_name()
            );
            // Clear the prompt state we just entered (no UI will answer it).
            session.on_user_decline();
            return vec![];
        }
        effects
    }

    fn handle_effects(store: &mut Store, worker: &PipelineWorker, effects: Vec<SessionEffect>) {
        for effect in effects {
            match effect {
                SessionEffect::RecordingSaved { app, saved } => {
                    tracing::info!(
                        "recording saved: {} ({} ms)",
                        saved.system_path,
                        saved.duration_ms
                    );
                    let end = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
                    // start = end - duration; nanos make the id collision-proof
                    // even for two recordings finishing in the same second.
                    let started_at = (end.as_secs() as i64) - (saved.duration_ms / 1000) as i64;
                    let meeting_id = format!("mtg-{}", end.as_nanos());
                    let app_key = app.display_name().to_lowercase().replace(' ', "-");
                    if let Err(e) = store.create_meeting(
                        &meeting_id,
                        &format!("{} meeting", app.display_name()),
                        Some(&app_key),
                        started_at,
                    ) {
                        tracing::error!("db: {e}");
                        continue;
                    }
                    let _ = store.set_meeting_audio(
                        &meeting_id,
                        Some(&saved.system_path),
                        saved.mic_path.as_deref(),
                        end.as_secs() as i64,
                    );

                    // Hand off to the pipeline worker and keep polling. ASR +
                    // diarization models load lazily per job over there; on
                    // 64 GB the ~2 GB whisper + ~50 MB diarization models
                    // could stay resident, but cold-loading keeps the daemon's
                    // idle footprint near zero and adds only seconds.
                    let queued = worker.submit(Job {
                        meeting_id: meeting_id.clone(),
                        system_wav: std::path::PathBuf::from(&saved.system_path),
                        mic_wav: saved.mic_path.as_ref().map(std::path::PathBuf::from),
                    });
                    if !queued {
                        // Worker thread died; the audio is safe on disk, so
                        // record the failure honestly instead of losing it.
                        tracing::error!("pipeline worker gone; {meeting_id} left unprocessed");
                        let _ = store.set_meeting_status(&meeting_id, "failed:worker");
                    }
                }
                SessionEffect::Error { message } => tracing::error!("{message}"),
                SessionEffect::PromptUser { .. } | SessionEffect::DismissPrompt { .. } => {}
            }
        }
    }

    fn notify_done(meeting_id: &str) {
        // Fire-and-forget completion toast; fine via osascript in the headless
        // build (the Tauri shell uses tauri-plugin-notification instead).
        let _ = std::process::Command::new("osascript")
            .arg("-e")
            .arg(format!(
                "display notification \"Summary ready ({meeting_id})\" with title \
                 \"whosaidwhat\" sound name \"Glass\""
            ))
            .spawn();
    }
}
