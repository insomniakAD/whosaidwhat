# Task 2 — Process Detection, Notification, Auto-Stop

The code lives in `src-tauri/src/detect/` (signals + state machine),
`src-tauri/src/notify.rs` (prompt), and `src-tauri/src/capture/` (recording +
auto-stop). The pure logic is compiled and unit-tested (30 tests, `rustc --test`
harness, D-006); the macOS FFI layer is written from the evidence below and
type-checks on first macOS build.

Evidence tiers: **[fetched]** = verbatim code fragments retrieved via the GitHub
code-search API; **[search-verified]** = search-excerpt evidence, URL cited, page not
directly fetchable from this sandbox; **[inference]** = design reasoning.

## 1. Detection signals, and why these exact ones

### Zoom — precise, process-based
The `CptHost` helper (bundle id `us.zoom.CptHost`) exists **only during an active
meeting** and exits the instant you leave; Zoom's main app (`us.zoom.xos`) keeps
running after the call. Watching the helper gives precise start *and* end edges.
**[fetched]** — verbatim from shipped detectors:
- https://github.com/moshebe/transcribeer/blob/main/gui/Sources/TranscribeerApp/Services/Meeting/MeetingTypes.swift — "the meeting-only helper (`us.zoom.CptHost`) exits the instant you leave"
- https://github.com/brunerd/macAdminTools — `pgrep "CptHost" && return 0` in `inMeeting_Zoom.sh`

**Pitfall (load-bearing):** Zoom also ships `caphost`, which runs whenever Zoom is
open, meeting or not. Substring matching produces permanent false positives; the code
matches exact executable names only. **[fetched]**
https://github.com/pixelsmasher13/platypus/blob/main/src-tauri/src/engine/meeting_detector.rs —
"`caphost` is a DIFFERENT process that runs whenever Zoom is open … do NOT match it".

### Microphone — the universal corroborator
CoreAudio's `kAudioDevicePropertyDeviceIsRunningSomewhere` (fourcc `'gone'` =
`0x676f6e65`) on the default input device reads 1 when **any** process does mic I/O —
one signal that covers Zoom, Teams, Meet, FaceTime, Webex with zero per-app work,
and it can be event-driven via `AudioObjectAddPropertyListener`. **[fetched]**
- https://github.com/madsmtm/objc2-generated/blob/main/CoreAudio/AudioHardware.rs — the selector constant
- https://github.com/djgould/transcriber/blob/main/src-tauri/src/device_listener.rs — working listener in a Tauri app (coreaudio-sys)

**Pitfall (load-bearing):** the moment whosaidwhat records, *it* holds the mic, the
device-level flag saturates at 1, and meeting-end becomes invisible. Shipped fix: the
macOS 14+ per-process objects — enumerate `kAudioHardwarePropertyProcessObjectList`,
skip your own PID, check `kAudioProcessPropertyIsRunningInput`. **[fetched]**
https://github.com/piro0919/chappie/blob/main/src-tauri/src/mic_activity.rs — "the
simpler device-level property can't work here because Chappie holds the input device
open continuously"; regression-tested for exactly this bug in
desduvauchelle/echo-scribe. `MacSignalSource::mic_in_use()` switches between the two
APIs on the `self_recording` flag. coreaudio-sys 0.2 predates Sonoma, so the three
per-process selectors are defined from the SDK fourccs in `detect/macos.rs` —
flagged for verification on first macOS build.

### Teams 2.x — the hard one
New Teams on macOS exposes no meeting window titles and no meeting-only child
process; its mic use isn't even visible to AVCaptureDevice. Working OSS treats
"Teams process tree running + audio evidence" as start, and reads Microsoft's
`audiomxd` daemon state from the unified log for end. **[fetched]**
https://github.com/ewilderj/meeting-notes-processor/blob/main/transcriber/SETUP.md —
the two-tier strategy verbatim. The legacy `logs.txt` trick (`a::1`/`a::3` events) is
dead in new Teams. **[search-verified]** https://github.com/RobertD502/TeamsStatusMacOS

**Decision (inference, logged D-009):** whosaidwhat uses *MSTeams running + mic
in use* as the Teams marker (`require_mic_for = true` in the detector) and accepts
that Teams meeting-end resolves via the mic going quiet (5-tick debounce) rather
than an authoritative signal. Parsing another vendor's daemon logs via `log show` is
brittle across Teams updates; it's noted as an optional refinement, not shipped.

### Google Meet — browser tabs, mic-gated
Meet has no process at all. Shipped approaches: browser window-title/tab scanning, or
calendar integration (Granola prompts from calendar events). Tab enumeration via
AppleScript is the programmatic route: Chromium browsers expose tab `title`+`URL`,
Safari `name`+`URL`. **[fetched]**
https://github.com/silverstein/minutes/blob/main/tauri/src-tauri/src/call_detect.rs —
`query_browser_tabs` with exactly this dialect split.

**Pitfall (load-bearing):** the Meet landing page titles itself "Meet - Google Meet"
and false-positives naive matchers. **[fetched]** dralquinta/busy-light removed those
patterns after hitting it. whosaidwhat matches only `meet.google.com/xxx-xxxx-xxx`
meeting codes and `lookup/` paths (`detect/meet_url.rs`, unit-tested), and only
probes tabs while a browser runs *and* the mic is live — so the osascript probe
costs nothing at idle. First probe per browser triggers the macOS Automation consent
dialog (one-time, per browser) — an accepted UX cost, noted in the app's onboarding.

### App lifecycle — event-driven poll gating
`NSWorkspace` posts `NSWorkspaceDidLaunchApplicationNotification` /
`...DidTerminate...` on its **own** notification center (not the default one);
objc2-app-kit exports the statics behind the `NSWorkspace` feature; observer tokens
must stay alive. **[fetched]**
- https://github.com/madsmtm/objc2-generated/blob/main/AppKit/NSWorkspace.rs
- https://github.com/vdavid/cmdr/blob/main/apps/desktop/src-tauri/src/file_system/open_with.rs — `addObserverForName_object_queue_usingBlock` pattern

whosaidwhat uses these only to switch the sysinfo poll between 15 s (idle) and 2 s
(active) cadences — the poll remains the source of truth, so a missed notification
can never wedge detection (inference; `Detector::next_poll_interval`).

## 2. The state machine (what makes the signals trustworthy)

`detect/state.rs` — pure, exhaustively unit-tested:

- **Enter debounce** (2 ticks ≈ 4 s): transient helpers at app launch don't trigger.
- **Exit debounce** (5 ticks ≈ 10 s): marker flapping (e.g. helper restarts between
  breakout rooms) doesn't end the meeting — the same "leave grace" idea platypus
  ships (`LEAVE_GRACE_POLLS`). **[fetched]** (platypus, above)
- **Mic-corroborated mode** (`require_mic_for`) for Teams/Meet: marker without mic is
  a lobby/tab, not a meeting; during a meeting, marker *or* mic keeps it alive
  (Meet tab stays open while someone mutes → no premature end).
- App crash mid-meeting emits `MeetingEnded` + `AppQuit` in order — the recorder
  auto-stops and finalizes the file even when Zoom dies.

## 3. Notification prompt

**Constraint discovered in research (load-bearing):** tauri-plugin-notification's
desktop backend is notify-rust, and `register_action_types` exists only in
`mobile.rs` — so a notification with a real "Start recording" **button** cannot be
delivered through the plugin on macOS. **[fetched]**
https://github.com/tauri-apps/plugins-workspace/blob/v2/plugins/notification/src/desktop.rs
(the desktop path even fakes the bundle id in dev: `set_application("com.apple.Terminal")`).

Shipped work-arounds. **Both paths are implemented** behind
`notify::PromptPresenter`: path 1 as `notify::WindowPrompt` (wired to the
shell's `prompt` window), path 2 as `notify::un_center::UnCenterPrompt` —
category `WSW_MEETING` with `WSW_START`/`WSW_DISMISS` actions, a
`define_class!` delegate, `CustomDismissAction` so swipe-dismiss clears the
pending callback, gated on `un_center::available()` (NSBundle
bundle-identifier check; UN APIs raise `NSInternalInconsistencyException`
in unbundled binaries — Apple forums 679326/649583). The shell picks
un_center when bundled, the window otherwise (shell.rs `PromptSurface`).
API surface written against the generated bindings
(github.com/madsmtm/objc2-generated, UserNotifications/*.rs) and wezterm's
shipped delegate (wezterm-toast-notification/src/macos.rs); not compilable
in this sandbox — flagged in-file per D-006.
1. **Custom always-on-top window** styled as a notification — works under `tauri
   dev`, no permissions, fully clickable. **[fetched]**
   https://github.com/pixelsmasher13/platypus/blob/main/src-tauri/src/engine/meeting_popup.rs
   (built "intentionally … rather than a real UNUserNotificationCenter notification").
   This doubles as the recording pill (docs/01, §2.1).
2. **UNUserNotificationCenter via objc2** with a category + delegate — real buttons,
   but requires a bundled signed .app; gate on `!tauri::is_dev()`. **[fetched]**
   https://github.com/indigoai-us/hq-desktop-app/blob/main/apps/sync/src-tauri/src/commands/un_notify.rs;
   https://github.com/ariso-ai/oats — `if !tauri::is_dev() { macos_un::show(...) }`.

**Consent honesty:** the shipped *headless* daemon (`main.rs`) cannot show either
surface — it has no UI/run-loop — so under `RecordPolicy::Prompt` it logs and does
NOT record; only `RecordPolicy::Auto` records headlessly. The prompt surfaces belong
to the Tauri shell. This is deliberate: silently auto-recording under a "prompt"
policy would violate consent.

## 4. Auto-stop and save

`capture/session.rs` (pure, tested) + `capture/macos.rs`:

- `MeetingEnded` while recording → `stop_and_save()`: stop cpal stream and SCK
  stream *first*, then finalize both WAV headers (hound), then emit
  `RecordingSaved { path, duration_ms }` → pipeline. Writer flushes per-buffer so a
  crash mid-meeting still leaves playable audio.
- `MeetingEnded` while the prompt is still up → `DismissPrompt` (no stale prompts).
- Overlapping second meeting while recording → ignored by design; one recording at a
  time, and only the *recorded* app's end stops it (tested:
  `overlapping_meeting_is_ignored_while_recording`).
- Meeting-end sources, per app: Zoom `CptHost` exit (precise), Teams/Meet mic-quiet +
  marker-gone with 10 s debounce, any app's process death (immediate).

## 5. Decision log additions

**D-009 (Teams end-detection)** — see §1 Teams: mic-quiet debounce over `audiomxd`
log parsing; brittleness beats a dependency on Microsoft's private log format.

**D-010 (poll vs pure events)** — polling with event-driven *gating* instead of a
fully event-driven detector: NSWorkspace tells us when apps launch/quit, but meeting
markers (CptHost, tabs) have no notifications; a 2 s poll of a process table is
microseconds of CPU. Simplicity wins; the state machine stays testable.
