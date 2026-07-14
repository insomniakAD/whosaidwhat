//! The "meeting detected — start recording?" prompt.
//!
//! Research constraint (verified from tauri-plugin-notification's source, see
//! docs/02-process-detection.md): the plugin's desktop backend is notify-rust
//! and `register_action_types` is implemented only for mobile, so a
//! notification with a real "Start recording" **button** cannot be delivered
//! through the plugin on macOS. Shipped Tauri apps solve this two ways, and we
//! support both behind one trait:
//!
//! - [`WindowPrompt`]: a small always-on-top Tauri window styled like a native
//!   notification. Works in `tauri dev`, needs no permissions, fully clickable.
//! - [`UnNotificationPrompt`] (bundled builds): `UNUserNotificationCenter` via
//!   objc2 with a notification category carrying Start/Dismiss actions and a
//!   delegate to receive the response. Requires a signed, bundled .app —
//!   gate on `!tauri::is_dev()`.
//!
//! The prompt is one honest question with two outcomes; everything else
//! (auto-record policy, manual start) bypasses it entirely.

use crate::detect::state::MeetingApp;

/// What the user chose on the prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptResponse {
    StartRecording,
    Dismiss,
}

/// Presents the prompt and reports the response asynchronously via callback.
pub trait PromptPresenter: Send {
    /// Show "Meeting detected in {app} — start recording?".
    /// `on_response` fires at most once, from an arbitrary thread.
    fn show(&mut self, app: &MeetingApp, on_response: Box<dyn FnOnce(PromptResponse) + Send>);

    /// Withdraw a still-visible prompt (meeting ended before the user chose).
    fn dismiss(&mut self);
}

/// Prompt copy shared by both presenters (kept in one place so the wording
/// stays identical across delivery mechanisms).
pub fn prompt_copy(app: &MeetingApp) -> (String, String) {
    (
        format!("{} meeting detected", app.display_name()),
        "Start recording? Audio stays on this Mac.".to_string(),
    )
}

/// Always-on-top Tauri window presenter (dev + unbundled builds).
///
/// The Tauri layer owns actual window creation; this type only carries the
/// wiring contract so the core stays UI-framework-agnostic. In the app:
///
/// ```ignore
/// // main.rs (Tauri v2)
/// let win = tauri::WebviewWindowBuilder::new(&app, "meeting-prompt",
///         tauri::WebviewUrl::App("prompt.html".into()))
///     .always_on_top(true)
///     .decorations(false)
///     .inner_size(360.0, 96.0)
///     .position(x, y) // top-right, under the menu bar
///     .focused(false) // do not steal keyboard focus from the meeting
///     .build()?;
/// // prompt.html buttons invoke `prompt_response` (a #[tauri::command])
/// // which forwards PromptResponse to the SessionManager.
/// ```
pub struct WindowPrompt {
    /// Sender the Tauri command handler uses to deliver the click.
    pub open_window: Box<dyn FnMut(String, String) + Send>,
    pub close_window: Box<dyn FnMut() + Send>,
    pending: Option<Box<dyn FnOnce(PromptResponse) + Send>>,
}

impl WindowPrompt {
    pub fn new(
        open_window: Box<dyn FnMut(String, String) + Send>,
        close_window: Box<dyn FnMut() + Send>,
    ) -> Self {
        WindowPrompt { open_window, close_window, pending: None }
    }

    /// Called by the Tauri `prompt_response` command when a button is clicked.
    pub fn deliver(&mut self, response: PromptResponse) {
        if let Some(cb) = self.pending.take() {
            cb(response);
        }
        (self.close_window)();
    }
}

impl PromptPresenter for WindowPrompt {
    fn show(&mut self, app: &MeetingApp, on_response: Box<dyn FnOnce(PromptResponse) + Send>) {
        let (title, body) = prompt_copy(app);
        self.pending = Some(on_response);
        (self.open_window)(title, body);
    }

    fn dismiss(&mut self) {
        self.pending = None;
        (self.close_window)();
    }
}

/// UNUserNotificationCenter presenter for bundled builds.
///
/// Implementation lives behind cfg(macos) and is deliberately thin: category
/// registration (`WSW_MEETING` with `START_RECORDING` / `DISMISS` actions),
/// a delegate implementing `userNotificationCenter:didReceiveNotificationResponse:`,
/// and permission request on first use. See the objc2 pattern used by shipped
/// Tauri apps referenced in docs/02-process-detection.md §Notifications.
#[cfg(target_os = "macos")]
pub mod un_center {
    //! Skeleton for the bundled-build path. Kept separate from WindowPrompt so
    //! the dev loop never touches UNUserNotificationCenter (which aborts the
    //! process when called from an unbundled binary).
    //!
    //! To finish when the app gets its bundle + signing:
    //! 1. `UNUserNotificationCenter::currentNotificationCenter()`
    //! 2. `requestAuthorizationWithOptions:completionHandler:` (alert + sound)
    //! 3. Register `UNNotificationCategory` "WSW_MEETING" with actions
    //!    `START_RECORDING` (foreground) and `DISMISS` (destructive).
    //! 4. Set a `define_class!` delegate; forward the chosen action to the
    //!    same `PromptResponse` channel WindowPrompt uses.

    /// Bundle-time gate: `UNUserNotificationCenter` requires a real .app bundle.
    pub fn available() -> bool {
        // A bundled app has a non-root bundle path ending in .app; `tauri dev`
        // binaries do not. Checked via NSBundle in the full implementation.
        std::env::var("WSW_FORCE_UN_NOTIFICATIONS").is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::Arc;

    #[test]
    fn window_prompt_delivers_response_once_and_closes() {
        let opened = Arc::new(AtomicU32::new(0));
        let closed = Arc::new(AtomicU32::new(0));
        let (o, c) = (opened.clone(), closed.clone());

        let mut prompt = WindowPrompt::new(
            Box::new(move |_t, _b| {
                o.fetch_add(1, Ordering::SeqCst);
            }),
            Box::new(move || {
                c.fetch_add(1, Ordering::SeqCst);
            }),
        );

        let responded = Arc::new(AtomicBool::new(false));
        let r = responded.clone();
        prompt.show(
            &MeetingApp::Zoom,
            Box::new(move |resp| {
                assert_eq!(resp, PromptResponse::StartRecording);
                r.store(true, Ordering::SeqCst);
            }),
        );
        assert_eq!(opened.load(Ordering::SeqCst), 1);

        prompt.deliver(PromptResponse::StartRecording);
        assert!(responded.load(Ordering::SeqCst));
        assert_eq!(closed.load(Ordering::SeqCst), 1);

        // Second deliver is a no-op on the callback (already consumed).
        prompt.deliver(PromptResponse::Dismiss);
    }

    #[test]
    fn dismiss_drops_pending_callback() {
        let mut prompt = WindowPrompt::new(Box::new(|_, _| {}), Box::new(|| {}));
        let responded = Arc::new(AtomicBool::new(false));
        let r = responded.clone();
        prompt.show(
            &MeetingApp::Meet,
            Box::new(move |_| {
                r.store(true, Ordering::SeqCst);
            }),
        );
        prompt.dismiss();
        prompt.deliver(PromptResponse::StartRecording);
        assert!(!responded.load(Ordering::SeqCst), "callback must not fire after dismiss");
    }

    #[test]
    fn copy_names_the_app() {
        let (title, body) = prompt_copy(&MeetingApp::Teams);
        assert!(title.contains("Microsoft Teams"));
        assert!(body.contains("on this Mac"));
    }
}
