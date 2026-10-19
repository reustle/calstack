//! Notification Center alerts posted as Calstack itself, so they get their own
//! entry in System Settings › Notifications and can be allowed through Focus,
//! and clicking one joins the meeting. UserNotifications only works inside a
//! signed app bundle: the bare binary, or a bundle macOS refuses, falls back
//! to AppleScript notifications (attributed to Script Editor, not clickable).
use crate::app;
use block2::{Block, RcBlock};
use calstack_core::meeting::recognized_url;
use objc2::{
    declare_class, msg_send_id, mutability,
    rc::Retained,
    runtime::{Bool, ProtocolObject},
    ClassType, DeclaredClass,
};
use objc2_foundation::{NSBundle, NSError, NSObject, NSObjectProtocol, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotification,
    UNNotificationDefaultActionIdentifier, UNNotificationPresentationOptions,
    UNNotificationRequest, UNNotificationResponse, UNNotificationSound, UNUserNotificationCenter,
    UNUserNotificationCenterDelegate,
};
use std::{
    process::Command,
    sync::atomic::{AtomicU64, AtomicU8, Ordering},
};

/// Not running from an app bundle, or macOS rejected the app: use AppleScript.
const SCRIPT: u8 = 0;
const NOT_REQUESTED: u8 = 1;
/// Asked; the user may still be looking at the permission prompt.
const PENDING: u8 = 2;
const GRANTED: u8 = 3;
/// The user turned Calstack's notifications off; respect that.
const DENIED: u8 = 4;

static STATE: AtomicU8 = AtomicU8::new(SCRIPT);
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

declare_class!(
    struct Delegate;

    unsafe impl ClassType for Delegate {
        type Super = NSObject;
        type Mutability = mutability::InteriorMutable;
        const NAME: &'static str = "CalstackNotificationDelegate";
    }

    impl DeclaredClass for Delegate {}

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl UNUserNotificationCenterDelegate for Delegate {
        /// Show banners even while Calstack counts as the active app.
        #[method(userNotificationCenter:willPresentNotification:withCompletionHandler:)]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            handler: &Block<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            handler.call((UNNotificationPresentationOptions::UNNotificationPresentationOptionBanner
                | UNNotificationPresentationOptions::UNNotificationPresentationOptionList
                | UNNotificationPresentationOptions::UNNotificationPresentationOptionSound,));
        }

        /// Clicking the notification opens the meeting link.
        #[method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:)]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            handler: &Block<dyn Fn()>,
        ) {
            let (action, id) = unsafe {
                (
                    response.actionIdentifier(),
                    response.notification().request().identifier().to_string(),
                )
            };
            if *action == *unsafe { UNNotificationDefaultActionIdentifier } {
                if let Some(url) = id.split_once('|').and_then(|(_, url)| recognized_url(url)) {
                    app::spawn_detached(Command::new("open").arg(url.as_str()));
                }
            }
            handler.call(());
        }
    }
);

impl Delegate {
    fn new() -> Retained<Self> {
        unsafe { msg_send_id![super(Self::alloc().set_ivars(())), init] }
    }
}

/// Install the click handler, and ask for permission now (when reminders are
/// on) so the prompt appears at launch rather than right before a meeting.
pub(super) fn init(enabled: bool) {
    if unsafe { NSBundle::mainBundle().bundleIdentifier() }.is_none() {
        return;
    }
    STATE.store(NOT_REQUESTED, Ordering::Relaxed);
    let center = unsafe { UNUserNotificationCenter::currentNotificationCenter() };
    let delegate = Delegate::new();
    unsafe { center.setDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
    // The center holds its delegate weakly; keep it for the process lifetime.
    std::mem::forget(delegate);
    if enabled {
        request_authorization(&center);
    }
}

fn request_authorization(center: &UNUserNotificationCenter) {
    STATE.store(PENDING, Ordering::Relaxed);
    let done = RcBlock::new(|granted: Bool, error: *mut NSError| {
        let state = match unsafe { error.as_ref() } {
            _ if granted.as_bool() => GRANTED,
            None => {
                tracing::info!("notifications are off for Calstack in System Settings");
                DENIED
            }
            Some(error) => {
                tracing::warn!(
                    error = %error.localizedDescription(),
                    "Notification Center unavailable; using AppleScript notifications"
                );
                SCRIPT
            }
        };
        STATE.store(state, Ordering::Relaxed);
    });
    unsafe {
        center.requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::UNAuthorizationOptionAlert
                | UNAuthorizationOptions::UNAuthorizationOptionSound,
            &done,
        )
    };
}

pub(super) fn post(title: &str, body: &str, url: &str) {
    let center = || unsafe { UNUserNotificationCenter::currentNotificationCenter() };
    match STATE.load(Ordering::Relaxed) {
        GRANTED => post_native(&center(), title, body, url),
        DENIED => {}
        // Posting natively while the permission prompt is still open makes
        // macOS answer the request with "not allowed"; use AppleScript until
        // the user has decided.
        NOT_REQUESTED => {
            request_authorization(&center());
            post_script(title, body);
        }
        _ => post_script(title, body),
    }
}

fn post_native(center: &UNUserNotificationCenter, title: &str, body: &str, url: &str) {
    let content = unsafe { UNMutableNotificationContent::new() };
    unsafe {
        content.setTitle(&NSString::from_str(title));
        content.setBody(&NSString::from_str(body));
        content.setSound(Some(&UNNotificationSound::defaultSound()));
    }
    // The identifier carries the link back to the click handler.
    let id = format!("{}|{url}", NEXT_ID.fetch_add(1, Ordering::Relaxed));
    let request = unsafe {
        UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSString::from_str(&id),
            &content,
            None,
        )
    };
    let (title, body) = (title.to_owned(), body.to_owned());
    let done = RcBlock::new(move |error: *mut NSError| {
        if let Some(error) = unsafe { error.as_ref() } {
            tracing::warn!(
                error = %error.localizedDescription(),
                "Notification Center refused the notification; using AppleScript"
            );
            post_script(&title, &body);
        }
    });
    unsafe { center.addNotificationRequest_withCompletionHandler(&request, Some(&done)) };
}

/// Text goes through argv, so it's never parsed as AppleScript.
fn post_script(title: &str, body: &str) {
    app::spawn_detached(
        Command::new("osascript")
            .args([
                "-e",
                "on run argv",
                "-e",
                "display notification (item 2 of argv) with title (item 1 of argv) sound name \"default\"",
                "-e",
                "end run",
            ])
            .args([title, body]),
    );
}
