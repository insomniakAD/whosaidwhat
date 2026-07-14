//! macOS signal providers for meeting detection.
//!
//! Three layers, cheapest first (all findings below verified against shipped
//! open-source detectors via GitHub code search — see docs/02-process-detection.md
//! for sources):
//!
//! 1. **App lifecycle (event-driven, free):** `NSWorkspace` posts
//!    `NSWorkspaceDidLaunchApplicationNotification` / `...DidTerminate...` on its
//!    own notification center. We subscribe via `objc2-app-kit` and use these to
//!    switch the poller between a slow idle cadence and the active cadence —
//!    no busy work while no conferencing app is running.
//!
//! 2. **Meeting markers (polled, cheap):**
//!    - Zoom: the `CptHost` helper (bundle id `us.zoom.CptHost`) exists *only*
//!      during an active meeting and exits the instant it ends. Match the exact
//!      process name; never substring-match — Zoom also ships a `caphost`
//!      helper that runs whenever Zoom is open and would false-positive.
//!    - Teams 2.x: exposes no meeting window title and no meeting-only child
//!      process on macOS; we treat "MSTeams running + mic in use" as the marker.
//!    - Google Meet: browser-based, no process at all; we enumerate browser tabs
//!      via AppleScript (Chromium exposes tab `title` + `URL`, Safari `name` +
//!      `URL`) and match `meet.google.com/xxx-xxxx-xxx` meeting URLs, excluding
//!      the `Meet - Google Meet` landing page. The probe only runs while a
//!      browser is frontmost-running *and* the mic is active, so idle cost is nil.
//!
//! 3. **Microphone ground truth (event-driven):** CoreAudio's
//!    `kAudioDevicePropertyDeviceIsRunningSomewhere` on the default input device
//!    flips to 1 whenever *any* process does mic I/O. Universal corroborator.
//!    Pitfall (verified in shipped code): once whosaidwhat itself records, the
//!    device-level flag saturates at 1, so meeting-*end* detection while
//!    recording uses the macOS 14+ per-process objects
//!    (`kAudioHardwarePropertyProcessObjectList` → `kAudioProcessPropertyIsRunningInput`),
//!    skipping our own PID. On macOS < 14 we fall back to the meeting markers
//!    alone (Zoom stays precise; Teams end degrades to app-quit).
//!
//! FFI-verification note: this module compiles only on macOS. The objc2 /
//! coreaudio-sys call shapes follow the crates' generated signatures as verified
//! from shipped open-source Rust apps (djgould/transcriber, vdavid/cmdr,
//! piro0919/chappie); the sandbox that authored this file could not cross-compile
//! to aarch64-apple-darwin, so treat the first macOS build as the type-check.

#![cfg(target_os = "macos")]
#![allow(non_upper_case_globals)]

use std::collections::HashMap;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use sysinfo::{ProcessRefreshKind, RefreshKind, System};

use super::meet_url::is_meet_meeting_url;
use super::state::{MeetingApp, SignalSnapshot};

/// Exact (not substring) process names that exist only during an active meeting.
/// `CptHost` is Zoom's in-meeting caption-host helper.
const ZOOM_MEETING_PROCESS: &str = "CptHost";
/// Zoom main client process name on macOS.
const ZOOM_APP_PROCESS: &str = "zoom.us";
/// Teams 2.x main process name on macOS.
const TEAMS_APP_PROCESS: &str = "MSTeams";
/// Browsers probed for Google Meet tabs, with their AppleScript dialect.
const MEET_BROWSERS: &[(&str, BrowserKind)] = &[
    ("Google Chrome", BrowserKind::Chromium),
    ("Arc", BrowserKind::Chromium),
    ("Microsoft Edge", BrowserKind::Chromium),
    ("Brave Browser", BrowserKind::Chromium),
    ("Safari", BrowserKind::Safari),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserKind {
    Chromium,
    Safari,
}

/// One poll of every signal, for all watched apps at once.
pub struct MacSignalSource {
    system: System,
    mic: MicMonitor,
    /// Set true while whosaidwhat itself records (saturates the device-level
    /// mic flag; switches end-detection to the per-process API).
    self_recording: Arc<AtomicBool>,
}

impl MacSignalSource {
    pub fn new(self_recording: Arc<AtomicBool>) -> Self {
        MacSignalSource {
            system: System::new_with_specifics(
                RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing()),
            ),
            mic: MicMonitor::new(),
            self_recording,
        }
    }

    /// Poll all signals. Returns one snapshot per watched app.
    pub fn poll(&mut self) -> HashMap<MeetingApp, SignalSnapshot> {
        self.system.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

        let mut zoom_running = false;
        let mut zoom_meeting = false;
        let mut teams_running = false;
        let mut browser_running = false;

        for process in self.system.processes().values() {
            // Exact-match on the executable name (not the full command line):
            // substring matching is how the `caphost` false positive happens.
            let name = process.name().to_string_lossy();
            match name.as_ref() {
                ZOOM_APP_PROCESS => zoom_running = true,
                ZOOM_MEETING_PROCESS => zoom_meeting = true,
                TEAMS_APP_PROCESS => teams_running = true,
                _ => {
                    if MEET_BROWSERS.iter().any(|(b, _)| name.as_ref() == *b) {
                        browser_running = true;
                    }
                }
            }
        }

        let mic = self.mic_in_use();

        // Meet's tab probe is comparatively expensive (spawns osascript), so it
        // is gated: only while a browser runs AND the mic is live.
        let meet_tab_open = if browser_running && mic {
            detect_meet_tab()
        } else {
            false
        };

        HashMap::from([
            (
                MeetingApp::Zoom,
                SignalSnapshot {
                    app_running: zoom_running || zoom_meeting,
                    meeting_marker: zoom_meeting,
                    mic_in_use: mic,
                },
            ),
            (
                MeetingApp::Teams,
                SignalSnapshot {
                    app_running: teams_running,
                    // Teams 2.x has no meeting-only child process on macOS;
                    // mic-in-use while Teams runs is the best available marker.
                    // AppDetector runs Teams with `require_mic_for = true`.
                    meeting_marker: teams_running && mic,
                    mic_in_use: mic,
                },
            ),
            (
                MeetingApp::Meet,
                SignalSnapshot {
                    app_running: browser_running,
                    meeting_marker: meet_tab_open,
                    mic_in_use: mic,
                },
            ),
        ])
    }

    /// True when some process other than ourselves is using the microphone.
    fn mic_in_use(&mut self) -> bool {
        if self.self_recording.load(Ordering::Relaxed) {
            // Our own capture keeps the device-level flag pinned at 1;
            // ask per-process instead (macOS 14+), or degrade gracefully.
            self.mic.any_other_process_running_input().unwrap_or(false)
        } else {
            self.mic.device_running_somewhere().unwrap_or(false)
        }
    }
}

/// Detect an open Google Meet meeting tab in any running browser.
///
/// AppleScript per browser dialect; requires the Automation TCC grant the
/// first time it targets each browser (the OS shows the consent dialog).
/// Meeting URLs look like meet.google.com/abc-defg-hij; the bare landing
/// page (meet.google.com, title "Meet - Google Meet") must not match.
pub fn detect_meet_tab() -> bool {
    for (browser, kind) in MEET_BROWSERS {
        let script = match kind {
            BrowserKind::Chromium => format!(
                r#"tell application "System Events" to set isRunning to (name of processes) contains "{browser}"
if isRunning then
  tell application "{browser}"
    set out to ""
    repeat with w in windows
      repeat with t in tabs of w
        set out to out & (URL of t) & "\n"
      end repeat
    end repeat
    return out
  end tell
end if
return """#
            ),
            BrowserKind::Safari => format!(
                r#"tell application "System Events" to set isRunning to (name of processes) contains "{browser}"
if isRunning then
  tell application "{browser}"
    set out to ""
    repeat with w in windows
      repeat with t in tabs of w
        set out to out & (URL of t) & "\n"
      end repeat
    end repeat
    return out
  end tell
end if
return """#
            ),
        };

        let output = Command::new("osascript").arg("-e").arg(&script).output();
        if let Ok(out) = output {
            let urls = String::from_utf8_lossy(&out.stdout);
            if urls.lines().any(is_meet_meeting_url) {
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// CoreAudio microphone monitor
// ---------------------------------------------------------------------------

use coreaudio_sys::{
    kAudioDevicePropertyDeviceIsRunningSomewhere, kAudioHardwarePropertyDefaultInputDevice,
    kAudioObjectPropertyElementMaster, kAudioObjectPropertyScopeGlobal, kAudioObjectSystemObject,
    AudioDeviceID, AudioObjectGetPropertyData, AudioObjectPropertyAddress,
};

/// Selectors for the macOS 14+ per-process audio objects. Not yet exported by
/// coreaudio-sys 0.2 (its headers predate Sonoma), so defined here from the
/// macOS 14.4 SDK AudioHardware.h fourcc values:
///   kAudioHardwarePropertyProcessObjectList = 'prs#'
///   kAudioProcessPropertyPID                = 'ppid'
///   kAudioProcessPropertyIsRunningInput     = 'piri'
const kAudioHardwarePropertyProcessObjectList: u32 = u32::from_be_bytes(*b"prs#");
const kAudioProcessPropertyPID: u32 = u32::from_be_bytes(*b"ppid");
const kAudioProcessPropertyIsRunningInput: u32 = u32::from_be_bytes(*b"piri");

pub struct MicMonitor;

impl MicMonitor {
    pub fn new() -> Self {
        MicMonitor
    }

    fn global_address(selector: u32) -> AudioObjectPropertyAddress {
        AudioObjectPropertyAddress {
            mSelector: selector,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMaster,
        }
    }

    fn default_input_device(&self) -> Option<AudioDeviceID> {
        let mut device: AudioDeviceID = 0;
        let mut size = std::mem::size_of::<AudioDeviceID>() as u32;
        let addr = Self::global_address(kAudioHardwarePropertyDefaultInputDevice);
        let status = unsafe {
            AudioObjectGetPropertyData(
                kAudioObjectSystemObject,
                &addr,
                0,
                std::ptr::null(),
                &mut size,
                &mut device as *mut _ as *mut _,
            )
        };
        (status == 0 && device != 0).then_some(device)
    }

    /// Device-level: is ANY process running I/O on the default input device?
    pub fn device_running_somewhere(&self) -> Option<bool> {
        let device = self.default_input_device()?;
        let mut running: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;
        let addr = Self::global_address(kAudioDevicePropertyDeviceIsRunningSomewhere);
        let status = unsafe {
            AudioObjectGetPropertyData(
                device,
                &addr,
                0,
                std::ptr::null(),
                &mut size,
                &mut running as *mut _ as *mut _,
            )
        };
        (status == 0).then_some(running != 0)
    }

    /// Per-process (macOS 14+): is any process besides ours capturing input?
    /// Returns None when the API is unavailable (pre-Sonoma) or errors.
    pub fn any_other_process_running_input(&self) -> Option<bool> {
        let own_pid = std::process::id() as i32;

        // 1. Enumerate process objects.
        let list_addr = Self::global_address(kAudioHardwarePropertyProcessObjectList);
        let mut size: u32 = 0;
        let status = unsafe {
            coreaudio_sys::AudioObjectGetPropertyDataSize(
                kAudioObjectSystemObject,
                &list_addr,
                0,
                std::ptr::null(),
                &mut size,
            )
        };
        if status != 0 || size == 0 {
            return None;
        }
        let count = size as usize / std::mem::size_of::<u32>();
        let mut objects: Vec<u32> = vec![0; count];
        let status = unsafe {
            AudioObjectGetPropertyData(
                kAudioObjectSystemObject,
                &list_addr,
                0,
                std::ptr::null(),
                &mut size,
                objects.as_mut_ptr() as *mut _,
            )
        };
        if status != 0 {
            return None;
        }

        // 2. For each process object: skip our own PID, check input activity.
        for obj in objects {
            let mut pid: i32 = -1;
            let mut pid_size = std::mem::size_of::<i32>() as u32;
            let pid_addr = Self::global_address(kAudioProcessPropertyPID);
            let ok = unsafe {
                AudioObjectGetPropertyData(
                    obj,
                    &pid_addr,
                    0,
                    std::ptr::null(),
                    &mut pid_size,
                    &mut pid as *mut _ as *mut _,
                )
            };
            if ok != 0 || pid == own_pid {
                continue;
            }

            let mut running: u32 = 0;
            let mut run_size = std::mem::size_of::<u32>() as u32;
            let run_addr = Self::global_address(kAudioProcessPropertyIsRunningInput);
            let ok = unsafe {
                AudioObjectGetPropertyData(
                    obj,
                    &run_addr,
                    0,
                    std::ptr::null(),
                    &mut run_size,
                    &mut running as *mut _ as *mut _,
                )
            };
            if ok == 0 && running != 0 {
                return Some(true);
            }
        }
        Some(false)
    }
}

// ---------------------------------------------------------------------------
// NSWorkspace app-lifecycle observer (event-driven poll gating)
// ---------------------------------------------------------------------------

pub mod workspace {
    //! Subscribes to app launch/termination on NSWorkspace's own notification
    //! center. Used only to gate the poll cadence (idle vs active); the poll
    //! loop itself remains the source of truth so a missed notification can
    //! never wedge detection.

    use std::sync::mpsc::Sender;

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_app_kit::{
        NSWorkspace, NSWorkspaceDidLaunchApplicationNotification,
        NSWorkspaceDidTerminateApplicationNotification,
    };
    use objc2_foundation::NSNotification;

    /// Which lifecycle edge fired.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum AppLifecycle {
        Launched,
        Terminated,
    }

    /// Observer tokens; keep this alive for the subscription lifetime —
    /// dropping it removes the observers.
    pub struct WorkspaceWatcher {
        tokens: Vec<Retained<AnyObject>>,
    }

    impl WorkspaceWatcher {
        /// `tx` receives an edge whenever any application launches/terminates.
        /// Must be called on a thread with a running main run loop (Tauri's
        /// main thread qualifies).
        pub fn subscribe(tx: Sender<AppLifecycle>) -> Self {
            let workspace = unsafe { NSWorkspace::sharedWorkspace() };
            let center = unsafe { workspace.notificationCenter() };

            let mut tokens = Vec::with_capacity(2);
            for (name, edge) in [
                (unsafe { NSWorkspaceDidLaunchApplicationNotification }, AppLifecycle::Launched),
                (
                    unsafe { NSWorkspaceDidTerminateApplicationNotification },
                    AppLifecycle::Terminated,
                ),
            ] {
                let tx = tx.clone();
                let block = RcBlock::new(move |_notif: std::ptr::NonNull<NSNotification>| {
                    let _ = tx.send(edge);
                });
                let token = unsafe {
                    center.addObserverForName_object_queue_usingBlock(
                        Some(name),
                        None,
                        None,
                        &block,
                    )
                };
                tokens.push(token);
            }
            WorkspaceWatcher { tokens }
        }
    }

    impl Drop for WorkspaceWatcher {
        fn drop(&mut self) {
            let workspace = unsafe { NSWorkspace::sharedWorkspace() };
            let center = unsafe { workspace.notificationCenter() };
            for token in self.tokens.drain(..) {
                unsafe { center.removeObserver(&token) };
            }
        }
    }
}
