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
//!   This is the only presenter that implements [`PromptPresenter`] today.
//! - The `un_center` module (bundled builds): a skeleton for the
//!   `UNUserNotificationCenter` path via objc2 — a notification category
//!   carrying Start/Dismiss actions plus a delegate. It is intentionally
//!   comment-only until the app has a signed .app bundle (calling UN APIs from
//!   an unbundled binary aborts the process); finish it gated on
//!   `!tauri::is_dev()`.
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
/// Kept separate from [`WindowPrompt`] so the dev loop never touches
/// UNUserNotificationCenter — calling it from an unbundled binary raises
/// `NSInternalInconsistencyException: bundleProxyForCurrentProcess is nil`
/// and aborts the process (Apple forums threads 679326 / 649583; the exact
/// message is quoted in invertase/notifee#260). Callers MUST check
/// [`un_center::available`] first.
#[cfg(target_os = "macos")]
pub mod un_center {
    //! Actionable "Meeting detected — start recording?" notification via
    //! `objc2-user-notifications` (the `notify-rust`/`mac-notification-sys`
    //! stack tauri-plugin-notification uses wraps the deprecated
    //! NSUserNotification API and cannot deliver action buttons on macOS —
    //! notify-rust#145; the plugin's Actions API is mobile-only per
    //! v2.tauri.app/plugin/notification).
    //!
    //! API surface written verbatim against the generated bindings in
    //! github.com/madsmtm/objc2-generated (UserNotifications/*.rs) and the
    //! shipped usage in wezterm's `wezterm-toast-notification/src/macos.rs`
    //! (define_class! delegate, RcBlock completion handlers, category +
    //! action registration) — evidence tiers in docs/02 §Notifications.
    //! NOT COMPILED IN THIS SANDBOX (crates.io blocked, macOS target): the
    //! first `cargo build` on a Mac is the type-check, per BUILD_LOG D-006.
    //!
    //! Integration contract (delegate-before-launch): construct
    //! [`UnCenterPrompt`] during app startup — for the Tauri shell, inside
    //! `.setup()` — so responses that arrive while the app was closed are
    //! not missed (Apple: assign the delegate before the app finishes
    //! launching).

    use std::sync::{Arc, Mutex};

    use block2::{Block, RcBlock};
    use objc2::rc::Retained;
    use objc2::runtime::{Bool, ProtocolObject};
    use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
    use objc2_foundation::{
        ns_string, NSArray, NSBundle, NSError, NSObject, NSObjectProtocol, NSSet, NSString,
    };
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNMutableNotificationContent, UNNotificationAction,
        UNNotificationActionOptions, UNNotificationCategory, UNNotificationCategoryOptions,
        UNNotificationRequest, UNNotificationResponse, UNUserNotificationCenter,
        UNUserNotificationCenterDelegate,
    };

    use super::{prompt_copy, PromptPresenter, PromptResponse};
    use crate::detect::state::MeetingApp;

    // Note: identifiers appear as literals inside `ns_string!` below (the
    // macro takes a string literal, not a const); these consts exist for the
    // Rust-side comparisons and must stay in sync with those literals.
    const ACTION_START: &str = "WSW_START";
    const REQUEST_ID: &str = "wsw-meeting-prompt";

    /// Bundle gate: UNUserNotificationCenter requires a real .app bundle with
    /// a bundle identifier; anything else (plain `cargo run`, `tauri dev`
    /// without a bundle) must stay on [`super::WindowPrompt`]. This is the
    /// defensive check pattern used by shipped objc2 consumers
    /// (takemo101/agent-bench src/notification/center.rs).
    pub fn available() -> bool {
        unsafe { NSBundle::mainBundle().bundleIdentifier().is_some() }
    }

    type PendingResponse = Arc<Mutex<Option<Box<dyn FnOnce(PromptResponse) + Send>>>>;

    pub struct DelegateIvars {
        pending: PendingResponse,
    }

    define_class!(
        // SAFETY: NSObject subclass with no lifecycle overrides; ivars are a
        // plain Rust struct managed by objc2's DefinedClass machinery.
        #[unsafe(super = NSObject)]
        #[name = "WSWNotificationDelegate"]
        #[ivars = DelegateIvars]
        pub struct NotifDelegate;

        unsafe impl NSObjectProtocol for NotifDelegate {}

        unsafe impl UNUserNotificationCenterDelegate for NotifDelegate {
            /// The user tapped the notification or one of its action buttons.
            /// `actionIdentifier` is our action id, or Apple's default
            /// ("com.apple.UNNotificationDefaultActionIdentifier", a body
            /// click) / dismiss ("com.apple.UNNotificationDismissActionIdentifier",
            /// delivered because the category opts into CustomDismissAction).
            #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
            unsafe fn did_receive(
                &self,
                _center: &UNUserNotificationCenter,
                response: &UNNotificationResponse,
                completion_handler: &Block<dyn Fn()>,
            ) {
                let action = unsafe { response.actionIdentifier() }.to_string();
                // Only the explicit button records; a body click or dismissal
                // is not consent (docs/02 §3: one honest question).
                let chosen = if action == ACTION_START {
                    PromptResponse::StartRecording
                } else {
                    PromptResponse::Dismiss
                };
                if let Ok(mut pending) = self.ivars().pending.lock() {
                    if let Some(cb) = pending.take() {
                        cb(chosen);
                    }
                }
                completion_handler.call(());
            }
        }
    );

    impl NotifDelegate {
        fn new(pending: PendingResponse) -> Retained<Self> {
            let this = Self::alloc().set_ivars(DelegateIvars { pending });
            unsafe { msg_send![super(this), init] }
        }
    }

    /// [`PromptPresenter`] over UNUserNotificationCenter. Construct once at
    /// startup (see module docs); [`available`] must be true.
    pub struct UnCenterPrompt {
        center: Retained<UNUserNotificationCenter>,
        // The center holds its delegate WEAKLY — this field is what keeps it
        // alive (wezterm re-reads CENTER.delegate() after setting it to prove
        // exactly this point).
        _delegate: Retained<NotifDelegate>,
        pending: PendingResponse,
    }

    // SAFETY: Apple documents UNUserNotificationCenter as usable from any
    // thread, and wezterm shares its center through a `LazyLock` static (both
    // cited in docs/02 §Notifications). The delegate is only ever invoked by
    // the ObjC runtime; `pending` is the one piece of shared Rust state and
    // it is behind a Mutex.
    unsafe impl Send for UnCenterPrompt {}

    impl UnCenterPrompt {
        pub fn new() -> Self {
            assert!(available(), "UNUserNotificationCenter requires a bundled .app");
            let pending: PendingResponse = Arc::new(Mutex::new(None));
            let delegate = NotifDelegate::new(pending.clone());
            let center = unsafe { UNUserNotificationCenter::currentNotificationCenter() };

            unsafe {
                let proto = ProtocolObject::from_retained(delegate.clone());
                center.setDelegate(Some(&proto));

                // Nothing shows until the user grants this (first call shows
                // the system permission dialog; later calls are no-ops).
                let on_auth = RcBlock::new(|granted: Bool, _err: *mut NSError| {
                    if !granted.as_bool() {
                        tracing::warn!("notification permission denied; prompts will not appear");
                    }
                });
                center.requestAuthorizationWithOptions_completionHandler(
                    UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
                    &on_auth,
                );

                // Category with the two actions. Start is a background action
                // (recording starts in-process; yanking the meeting out of
                // focus to foreground our app would be hostile). Dismiss is
                // Destructive so it renders in the warning style, and the
                // category opts into CustomDismissAction so swipe-dismiss
                // reaches the delegate and clears the pending callback.
                let start = UNNotificationAction::actionWithIdentifier_title_options(
                    ns_string!("WSW_START"),
                    ns_string!("Start recording"),
                    UNNotificationActionOptions::empty(),
                );
                let dismiss = UNNotificationAction::actionWithIdentifier_title_options(
                    ns_string!("WSW_DISMISS"),
                    ns_string!("Ignore"),
                    UNNotificationActionOptions::Destructive,
                );
                let no_intents: Retained<NSArray<NSString>> = NSArray::new();
                let category = UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
                    ns_string!("WSW_MEETING"),
                    &NSArray::from_retained_slice(&[start, dismiss]),
                    &no_intents,
                    UNNotificationCategoryOptions::CustomDismissAction,
                );
                center.setNotificationCategories(&NSSet::from_retained_slice(&[category]));
            }

            UnCenterPrompt { center, _delegate: delegate, pending }
        }

        fn withdraw(&self) {
            unsafe {
                let ids = NSArray::from_retained_slice(&[NSString::from_str(REQUEST_ID)]);
                self.center.removePendingNotificationRequestsWithIdentifiers(&ids);
                self.center.removeDeliveredNotificationsWithIdentifiers(&ids);
            }
        }
    }

    impl PromptPresenter for UnCenterPrompt {
        fn show(&mut self, app: &MeetingApp, on_response: Box<dyn FnOnce(PromptResponse) + Send>) {
            let (title, body) = prompt_copy(app);
            if let Ok(mut pending) = self.pending.lock() {
                *pending = Some(on_response);
            }
            unsafe {
                let content = UNMutableNotificationContent::new();
                content.setTitle(&NSString::from_str(&title));
                content.setBody(&NSString::from_str(&body));
                content.setCategoryIdentifier(ns_string!("WSW_MEETING"));
                // trigger: None ⇒ deliver immediately (generated signature:
                // requestWithIdentifier:content:trigger: with Option trigger).
                let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
                    ns_string!("wsw-meeting-prompt"),
                    &content,
                    None,
                );
                self.center.addNotificationRequest_withCompletionHandler(&request, None);
            }
        }

        fn dismiss(&mut self) {
            if let Ok(mut pending) = self.pending.lock() {
                *pending = None;
            }
            self.withdraw();
        }
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
