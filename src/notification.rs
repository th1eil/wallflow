//! macOS system notification bridge.
//!
//! Provisional authorization keeps this non-intrusive: no permission prompt is
//! shown and notifications land quietly in Notification Center unless the user
//! promotes the app. Every entry point degrades to a no-op when the center is
//! unavailable (e.g. running the bare binary outside a bundle), so callers
//! never need error handling here — the menu status line remains the primary,
//! always-available feedback channel.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::atomic::AtomicU64;

use block2::RcBlock;
use objc2::runtime::Bool;
use objc2::{msg_send, ClassType};
use objc2::rc::Retained;
use objc2_foundation::{NSBundle, NSError, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotificationRequest,
    UNNotificationSound, UNUserNotificationCenter,
};

/// Whether requestAuthorization granted delivery (provisional counts).
static AUTHORIZED: AtomicBool = AtomicBool::new(false);
/// Unique-per-call identifiers so repeated posts don't collapse into one.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// 裸二进制（`cargo run` 直接跑 target/debug 下的可执行文件，未经 .app 打包）
/// 没有 bundle identifier；此时 `UNUserNotificationCenter` 会在
/// currentNotificationCenter 处抛 NSInternalInconsistencyException 直接崩溃。
/// 确认 bundle id 存在才继续，否则本入口降级为 no-op。
fn notification_available() -> bool {
    NSBundle::mainBundle().bundleIdentifier().is_some()
}

/// Ask for provisional notification delivery once at startup.
/// The completion runs on an arbitrary system thread; results are published
/// through the atomic flag only (thread-safe by design).
pub fn request_authorization() {
    if !notification_available() {
        return;
    }
    let block = RcBlock::new(|granted: Bool, _err: *mut NSError| {
        println!("Notification authorization granted: {}", granted.as_bool());
        AUTHORIZED.store(granted.as_bool(), Ordering::Relaxed);
    });
    let center = UNUserNotificationCenter::currentNotificationCenter();
    let options = UNAuthorizationOptions::Alert
        | UNAuthorizationOptions::Sound
        | UNAuthorizationOptions::Provisional;
    center.requestAuthorizationWithOptions_completionHandler(options, &block);
}

/// Post a title/body notification. No-op until authorized; never panics.
pub fn post(title: &str, body: &str) {
    if !AUTHORIZED.load(Ordering::Relaxed) {
        return;
    }

    let content: Retained<UNMutableNotificationContent> =
        unsafe { msg_send![UNMutableNotificationContent::class(), new] };
    content.setTitle(&NSString::from_str(title));
    content.setBody(&NSString::from_str(body));
    content.setSound(Some(&UNNotificationSound::defaultSound()));

    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let identifier = NSString::from_str(&format!("wallflow-{}", id));
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
        &identifier,
        &content,
        None,
    );

    // Fire-and-forget: a failed post (no bundle id when run bare) must not
    // disturb the caller, matching the silent-degradation contract.
    let center = UNUserNotificationCenter::currentNotificationCenter();
    let done = RcBlock::new(|_err: *mut NSError| {});
    center.addNotificationRequest_withCompletionHandler(&request, Some(&done));
}
