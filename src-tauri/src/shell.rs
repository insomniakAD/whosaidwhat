//! Tauri v2 desktop shell: dashboard + consent prompt + recording pill.
//!
//! This mounts the same headless wiring main.rs runs (detector → session →
//! pipeline worker) behind Tauri commands/events, exactly as the main.rs doc
//! comment promised. Three windows (docs/01 §2.1):
//!
//! - `main`   — the dashboard (sidebar, notes with citation chips, transcript,
//!              who-said-what rail, action items), from tauri.conf.json.
//! - `prompt` — the always-on-top "Meeting detected — start recording?"
//!              surface, created hidden at startup, shown on demand. This is
//!              what makes `RecordPolicy::Prompt` actually record (the
//!              headless daemon logs-and-declines; see main.rs D-015).
//! - `pill`   — the recording pill (elapsed time, stop), user-movable per
//!              docs/01 (the PillFloat evidence).
//!
//! Evidence tiers for the Tauri API surface (BUILD_LOG D-008): the config,
//! capability, build.rs, and Builder/entrypoint shapes are [fetched] from a
//! stock create-tauri-app scaffold and a multi-window production app; window
//! builder methods, Emitter/Listener traits, and tray APIs are
//! [search-verified] against v2.tauri.app + docs.rs listings. None of it can
//! compile in this sandbox (crates.io blocked — D-006): the first
//! `cargo build --features shell` on a networked Mac is the type-check.
//!
//! DB concurrency: three rusqlite connections share the SQLite file (UI
//! reads/small writes here, detection-thread meeting-row writes, pipeline
//! worker bulk writes). WAL + the 5 s busy_timeout set in db::Store::open
//! keep that safe; writers are rare and short except the worker's.
//!
//! Consent note: the prompt window is the consent surface. Auto policy
//! records without asking (user's explicit config choice); Manual only ever
//! records via the dashboard button.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::Mutex;

use tauri::Manager;

use crate::config::Config;
use crate::db::{
    ActionItemRow, MeetingRow, SearchHit, SegmentRow, SpeakerStat, Store, SummaryRow,
};

/// User actions flowing from the webviews into the detection thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlMsg {
    /// Prompt window: "Record".
    Accept,
    /// Prompt window: "Ignore".
    Decline,
    /// Dashboard: manual recording start (no meeting detection involved).
    StartManual,
    /// Dashboard or pill: stop the active recording.
    Stop,
}

pub struct AppState {
    store: Mutex<Store>,
    control: Mutex<Sender<ControlMsg>>,
    recording: AtomicBool,
    policy: &'static str,
}

// ---- event payloads (emitted from the macOS detection thread only) ----

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Clone, serde::Serialize)]
struct MeetingDetectedPayload {
    app: String,
    title: String,
    body: String,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Clone, serde::Serialize)]
struct RecordingStatePayload {
    recording: bool,
    app: Option<String>,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Clone, serde::Serialize)]
struct ProgressPayload {
    meeting_id: String,
    stage: String,
    done: usize,
    total: usize,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Clone, serde::Serialize)]
struct MeetingIdPayload {
    meeting_id: String,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Clone, serde::Serialize)]
struct ErrorPayload {
    message: String,
}

// ---- commands (UI → Rust). All DB reads/writes lock the UI connection. ----

type CmdResult<T> = Result<T, String>;

fn locked<'a>(state: &'a tauri::State<'_, AppState>) -> CmdResult<std::sync::MutexGuard<'a, Store>> {
    state.store.lock().map_err(|e| e.to_string())
}

#[tauri::command]
fn list_meetings(state: tauri::State<'_, AppState>, limit: Option<u32>) -> CmdResult<Vec<MeetingRow>> {
    locked(&state)?.list_meetings(limit.unwrap_or(200)).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_segments(state: tauri::State<'_, AppState>, meeting_id: String) -> CmdResult<Vec<SegmentRow>> {
    locked(&state)?.segments_for_meeting(&meeting_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_summary(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    kind: String,
) -> CmdResult<Option<SummaryRow>> {
    locked(&state)?.latest_summary(&meeting_id, &kind).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_action_items(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> CmdResult<Vec<ActionItemRow>> {
    locked(&state)?.action_items_for_meeting(&meeting_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_action_item_done(state: tauri::State<'_, AppState>, id: i64, done: bool) -> CmdResult<()> {
    locked(&state)?.set_action_item_done(id, done).map_err(|e| e.to_string())
}

#[tauri::command]
fn search_transcripts(state: tauri::State<'_, AppState>, query: String) -> CmdResult<Vec<SearchHit>> {
    locked(&state)?.search(&query, 50).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_speaker_stats(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> CmdResult<Vec<SpeakerStat>> {
    locked(&state)?.speaker_stats(&meeting_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn rename_speaker(state: tauri::State<'_, AppState>, speaker_id: i64, name: String) -> CmdResult<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err("name must not be empty".into());
    }
    locked(&state)?.rename_speaker(speaker_id, name).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_status(state: tauri::State<'_, AppState>) -> CmdResult<serde_json::Value> {
    Ok(serde_json::json!({
        "recording": state.recording.load(Ordering::SeqCst),
        "policy": state.policy,
    }))
}

fn send_control(state: &tauri::State<'_, AppState>, msg: ControlMsg) -> CmdResult<()> {
    state
        .control
        .lock()
        .map_err(|e| e.to_string())?
        .send(msg)
        .map_err(|_| "detection thread not running".to_string())
}

/// The prompt window's two buttons. Hides the window immediately (feedback
/// must not wait on the detection thread's next poll tick).
#[tauri::command]
fn prompt_response(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    accept: bool,
) -> CmdResult<()> {
    if let Some(w) = app.get_webview_window("prompt") {
        let _ = w.hide();
    }
    send_control(&state, if accept { ControlMsg::Accept } else { ControlMsg::Decline })
}

#[tauri::command]
fn start_manual_recording(state: tauri::State<'_, AppState>) -> CmdResult<()> {
    send_control(&state, ControlMsg::StartManual)
}

#[tauri::command]
fn stop_recording(state: tauri::State<'_, AppState>) -> CmdResult<()> {
    send_control(&state, ControlMsg::Stop)
}

// ---- entrypoint ----

pub fn run() {
    let config_path = Config::default().data_dir.join("config.json");
    let config = Config::load_or_default(&config_path);
    let _ = config.save(&config_path);

    let ui_store = Store::open(&config.db_path()).expect("open database (ui)");
    let (control_tx, control_rx) = channel::<ControlMsg>();

    let policy: &'static str = match config.record_policy {
        crate::config::RecordPolicyConfig::Prompt => "prompt",
        crate::config::RecordPolicyConfig::Auto => "auto",
        crate::config::RecordPolicyConfig::Manual => "manual",
    };

    let state = AppState {
        store: Mutex::new(ui_store),
        control: Mutex::new(control_tx),
        recording: AtomicBool::new(false),
        policy,
    };

    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            list_meetings,
            get_segments,
            get_summary,
            get_action_items,
            set_action_item_done,
            search_transcripts,
            get_speaker_stats,
            rename_speaker,
            get_status,
            prompt_response,
            start_manual_recording,
            stop_recording,
        ])
        .setup(move |app| {
            build_tray(app.handle())?;
            build_hidden_windows(app.handle())?;

            // Detection + capture are macOS-only; on other hosts the shell is
            // a read-only dashboard over an existing DB (useful in dev).
            #[cfg(target_os = "macos")]
            {
                let handle = app.handle().clone();
                let cfg = config.clone();
                let tx = {
                    let state = app.state::<AppState>();
                    let guard = state.control.lock().expect("control sender");
                    guard.clone()
                };
                std::thread::Builder::new()
                    .name("wsw-detect".into())
                    .spawn(move || macos_detect::detection_loop(handle, cfg, control_rx, tx))?;
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = &control_rx; // no detection thread to hand it to
                tracing::warn!("detection/capture are macOS-only; dashboard-only mode");
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running whosaidwhat");
}

/// Tray: open-dashboard + quit. The app stays useful with the main window
/// closed (docs/01: the Granola/superwhisper pattern).
fn build_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::TrayIconBuilder;

    let open = MenuItem::with_id(app, "open", "Open whosaidwhat", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;

    TrayIconBuilder::new()
        .icon(app.default_window_icon().expect("bundled window icon").clone())
        .menu(&menu)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(())
}

/// The prompt + pill are created hidden at startup (on the main thread, where
/// window creation is unconditionally safe) and toggled with show/hide.
fn build_hidden_windows(app: &tauri::AppHandle) -> tauri::Result<()> {
    let prompt = tauri::WebviewWindowBuilder::new(
        app,
        "prompt",
        tauri::WebviewUrl::App("prompt.html".into()),
    )
    .title("Meeting detected")
    .inner_size(380.0, 132.0)
    .resizable(false)
    .decorations(false)
    .always_on_top(true)
    .visible(false)
    .focused(false) // never steal keyboard focus from the meeting
    .build()?;
    position_top_right(&prompt, 380.0, 24.0, 48.0);

    let pill = tauri::WebviewWindowBuilder::new(
        app,
        "pill",
        tauri::WebviewUrl::App("pill.html".into()),
    )
    .title("Recording")
    .inner_size(260.0, 44.0)
    .resizable(false)
    .decorations(false)
    .always_on_top(true)
    .visible(false)
    .focused(false)
    .build()?;
    // Bottom-center-ish default, but user-movable (drag region in pill.html;
    // PillFloat exists because a locked pill frustrates — docs/01 §2.1).
    position_bottom_center(&pill, 260.0, 44.0, 96.0);
    Ok(())
}

/// Best-effort placement; a window that stays at the OS default position is
/// an annoyance, not an error, so failures are swallowed.
fn position_top_right(w: &tauri::WebviewWindow, width: f64, margin: f64, top: f64) {
    if let Ok(Some(monitor)) = w.current_monitor() {
        let size = monitor.size();
        let scale = monitor.scale_factor();
        let x = size.width as f64 / scale - width - margin;
        let _ = w.set_position(tauri::LogicalPosition::new(x, top));
    }
}

fn position_bottom_center(w: &tauri::WebviewWindow, width: f64, height: f64, bottom: f64) {
    if let Ok(Some(monitor)) = w.current_monitor() {
        let size = monitor.size();
        let scale = monitor.scale_factor();
        let x = (size.width as f64 / scale - width) / 2.0;
        let y = size.height as f64 / scale - height - bottom;
        let _ = w.set_position(tauri::LogicalPosition::new(x, y));
    }
}

// ---- the detection thread (macOS) ----

#[cfg(target_os = "macos")]
mod macos_detect {
    use super::*;
    use std::sync::mpsc::Receiver;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tauri::Emitter;

    use crate::capture::macos::{MacRecorder, SckSystemAudio};
    use crate::capture::session::{
        RecordPolicy, SessionEffect, SessionManager, SessionState,
    };
    use crate::detect::state::{DetectorEvent, MeetingApp};
    use crate::detect::{macos::MacSignalSource, Detector};
    use crate::llm::router::InferenceRouter;
    use crate::notify::prompt_copy;
    use crate::pipeline::worker::{Job, PipelineWorker};

    /// Consent surfaces, most-native-available first: a bundled .app uses
    /// UNUserNotificationCenter action buttons (notify::un_center); anything
    /// unbundled uses the always-on-top prompt window (UN APIs abort in
    /// unbundled binaries — see un_center docs). Both deliver through the
    /// same ControlMsg channel the webview buttons use.
    struct PromptSurface {
        app: tauri::AppHandle,
        tx: Sender<ControlMsg>,
        un_center: Option<crate::notify::un_center::UnCenterPrompt>,
    }

    impl PromptSurface {
        fn new(app: tauri::AppHandle, tx: Sender<ControlMsg>) -> Self {
            let un_center = if crate::notify::un_center::available() {
                Some(crate::notify::un_center::UnCenterPrompt::new())
            } else {
                tracing::info!("unbundled build: consent prompt uses the window surface");
                None
            };
            PromptSurface { app, tx, un_center }
        }

        fn show(&mut self, meeting_app: &MeetingApp) {
            let (title, body) = prompt_copy(meeting_app);
            // The dashboard mirrors the prompt state either way.
            let _ = self.app.emit(
                "meeting-detected",
                MeetingDetectedPayload {
                    app: meeting_app.display_name().to_string(),
                    title,
                    body,
                },
            );
            match &mut self.un_center {
                Some(presenter) => {
                    use crate::notify::{PromptPresenter, PromptResponse};
                    let tx = self.tx.clone();
                    presenter.show(
                        meeting_app,
                        Box::new(move |resp| {
                            let _ = tx.send(match resp {
                                PromptResponse::StartRecording => ControlMsg::Accept,
                                PromptResponse::Dismiss => ControlMsg::Decline,
                            });
                        }),
                    );
                }
                None => set_window_visible(&self.app, "prompt", true),
            }
        }

        fn dismiss(&mut self) {
            if let Some(presenter) = &mut self.un_center {
                use crate::notify::PromptPresenter;
                presenter.dismiss();
            }
            set_window_visible(&self.app, "prompt", false);
            let _ = self.app.emit("prompt-closed", ());
        }
    }

    /// Same loop as main.rs, with the Prompt policy actually answered by the
    /// prompt surface instead of auto-declined (main.rs D-015), and every
    /// state change mirrored to the webviews as events.
    pub fn detection_loop(
        app: tauri::AppHandle,
        config: Config,
        rx: Receiver<ControlMsg>,
        tx: Sender<ControlMsg>,
    ) {
        let mut store = Store::open(&config.db_path()).expect("open database (detect)");
        let mut surface = PromptSurface::new(app.clone(), tx);

        let worker_db_path = config.db_path();
        let worker_inference = config.inference.clone();
        let worker_app = app.clone();
        let worker = PipelineWorker::spawn(
            move || {
                let store = Store::open(&worker_db_path).expect("open database (worker)");
                let router = InferenceRouter::new(worker_inference);
                let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
                (store, router, runtime)
            },
            move |(store, router, runtime), job: Job| {
                let progress_app = worker_app.clone();
                let progress_meeting = job.meeting_id.clone();
                let mut progress = move |stage: &str, done: usize, total: usize| {
                    let _ = progress_app.emit(
                        "pipeline-progress",
                        ProgressPayload {
                            meeting_id: progress_meeting.clone(),
                            stage: stage.to_string(),
                            done,
                            total,
                        },
                    );
                };
                let result = runtime.block_on(crate::pipeline::run_with_default_engines(
                    store,
                    router,
                    &job.meeting_id,
                    &job.system_wav,
                    job.mic_wav.as_deref(),
                    &mut progress,
                ));
                match result {
                    Ok(_) => {
                        let _ = worker_app.emit(
                            "summary-ready",
                            MeetingIdPayload { meeting_id: job.meeting_id.clone() },
                        );
                    }
                    Err(e) => {
                        tracing::error!("pipeline failed for {}: {e}", job.meeting_id);
                        let _ = store.set_meeting_status(&job.meeting_id, "failed:pipeline");
                        let _ = worker_app.emit(
                            "pipeline-failed",
                            ErrorPayload { message: format!("{}: {e}", job.meeting_id) },
                        );
                    }
                }
            },
        )
        .expect("spawn pipeline worker");

        let self_recording = std::sync::Arc::new(AtomicBool::new(false));
        let source = MacSignalSource::new(self_recording.clone());
        let recorder = MacRecorder::new(SckSystemAudio::new(), self_recording);
        let mut detector = Detector::new(source);
        let mut session = SessionManager::new(
            recorder,
            RecordPolicy::from(config.record_policy),
            config.recordings_dir().display().to_string(),
        );

        // The meeting the visible prompt belongs to (for the accept stem).
        let mut prompted: Option<MeetingApp> = None;
        let mut was_recording = false;

        loop {
            // Sleep on the control channel instead of thread::sleep so user
            // clicks are handled within milliseconds, not at the next poll.
            match rx.recv_timeout(detector.next_poll_interval()) {
                Ok(msg) => {
                    let effects = match msg {
                        ControlMsg::Accept => match prompted.take() {
                            Some(meeting_app) => {
                                session.on_user_accept(&stem_for(&meeting_app))
                            }
                            None => vec![], // prompt raced meeting-end; nothing to accept
                        },
                        ControlMsg::Decline => {
                            prompted = None;
                            session.on_user_decline()
                        }
                        ControlMsg::StartManual => {
                            session.on_user_accept(&stem_for(&MeetingApp::Other("manual".into())))
                        }
                        ControlMsg::Stop => session.on_user_stop(),
                    };
                    handle_effects(&mut surface, &mut store, &worker, &mut prompted, effects);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return, // app gone
            }

            for event in detector.tick() {
                match event {
                    DetectorEvent::MeetingStarted(meeting_app) => {
                        let effects =
                            session.on_meeting_started(meeting_app.clone(), &stem_for(&meeting_app));
                        handle_effects(&mut surface, &mut store, &worker, &mut prompted, effects);
                    }
                    DetectorEvent::MeetingEnded(meeting_app) => {
                        let effects = session.on_meeting_ended(&meeting_app);
                        handle_effects(&mut surface, &mut store, &worker, &mut prompted, effects);
                        // Re-offer any concurrent meeting (see main.rs).
                        if session.state() == SessionState::Idle {
                            for other in detector.active_meetings() {
                                let effects =
                                    session.on_meeting_started(other.clone(), &stem_for(&other));
                                handle_effects(&mut surface, &mut store, &worker, &mut prompted, effects);
                                if session.state() != SessionState::Idle {
                                    break;
                                }
                            }
                        }
                    }
                    DetectorEvent::AppLaunched(a) => tracing::debug!("{} launched", a.display_name()),
                    DetectorEvent::AppQuit(a) => tracing::debug!("{} quit", a.display_name()),
                }
            }

            // Mirror recording-state transitions to the UI (start has no
            // SessionEffect; it is observable only as a state change).
            let recording = session.state() == SessionState::Recording;
            if recording != was_recording {
                was_recording = recording;
                if let Some(state) = app.try_state::<AppState>() {
                    state.recording.store(recording, Ordering::SeqCst);
                }
                set_window_visible(&app, "pill", recording);
                let _ = app.emit(
                    "recording-state",
                    RecordingStatePayload { recording, app: None },
                );
            }
        }
    }

    fn stem_for(meeting_app: &MeetingApp) -> String {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        format!(
            "{}-{}",
            meeting_app.display_name().to_lowercase().replace(' ', "-"),
            now
        )
    }

    fn set_window_visible(app: &tauri::AppHandle, label: &str, visible: bool) {
        if let Some(w) = app.get_webview_window(label) {
            let _ = if visible { w.show() } else { w.hide() };
        }
    }

    fn handle_effects(
        surface: &mut PromptSurface,
        store: &mut Store,
        worker: &PipelineWorker,
        prompted: &mut Option<MeetingApp>,
        effects: Vec<SessionEffect>,
    ) {
        let app = &surface.app.clone();
        for effect in effects {
            match effect {
                SessionEffect::PromptUser { app: meeting_app } => {
                    *prompted = Some(meeting_app.clone());
                    surface.show(&meeting_app);
                }
                SessionEffect::DismissPrompt { .. } => {
                    *prompted = None;
                    surface.dismiss();
                }
                SessionEffect::RecordingSaved { app: meeting_app, saved } => {
                    tracing::info!(
                        "recording saved: {} ({} ms)",
                        saved.system_path,
                        saved.duration_ms
                    );
                    let end = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
                    let started_at = (end.as_secs() as i64) - (saved.duration_ms / 1000) as i64;
                    let meeting_id = format!("mtg-{}", end.as_nanos());
                    let app_key = meeting_app.display_name().to_lowercase().replace(' ', "-");
                    if let Err(e) = store.create_meeting(
                        &meeting_id,
                        &format!("{} meeting", meeting_app.display_name()),
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
                    let queued = worker.submit(Job {
                        meeting_id: meeting_id.clone(),
                        system_wav: std::path::PathBuf::from(&saved.system_path),
                        mic_wav: saved.mic_path.as_ref().map(std::path::PathBuf::from),
                    });
                    if !queued {
                        tracing::error!("pipeline worker gone; {meeting_id} left unprocessed");
                        let _ = store.set_meeting_status(&meeting_id, "failed:worker");
                    }
                    let _ = app.emit("meeting-saved", MeetingIdPayload { meeting_id });
                }
                SessionEffect::Error { message } => {
                    tracing::error!("{message}");
                    let _ = app.emit("error", ErrorPayload { message });
                }
            }
        }
    }
}
