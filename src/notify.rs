//! macOS notifications and the Dock badge for agents that need you while
//! Wezzy is in the background.
//!
//! Notifications go through `UNUserNotificationCenter`, which only works in a
//! bundled app with a bundle id (`scripts/bundle.sh`): touching it from a bare
//! `cargo run` binary raises an Objective-C exception. Unbundled builds skip
//! notifications (the chime and toast still happen); the badge works in both.
//!
//! - Permission is requested on the first notification, not at launch.
//! - Each notification's identifier is `pane-<server pane id>`, so a newer one
//!   for the same pane replaces the older, and a click maps back to a pane
//!   across app restarts.
//! - The delegate runs on whatever thread UserNotifications picks; it only
//!   forwards the click with `send_event(AppEvent::NotificationClicked)`.
//!
//! Main thread only, except where noted.

use block2::{Block, RcBlock};
use mux::pane::PaneId;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, AnyProtocol, Bool, ClassBuilder, Sel};
use objc2::{class, msg_send, sel};
use parking_lot::Mutex;
use std::ffi::CString;
use std::sync::OnceLock;

#[link(name = "UserNotifications", kind = "framework")]
extern "C" {}

/// UNAuthorizationOptionBadge | UNAuthorizationOptionAlert (sound: the chime does that).
const AUTH_OPTIONS: usize = (1 << 0) | (1 << 2);
const ID_PREFIX: &str = "pane-";

fn nsstring(s: &str) -> Retained<AnyObject> {
    let c = CString::new(s.replace('\0', "")).unwrap_or_default();
    unsafe { msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()] }
}

fn to_string(s: &AnyObject) -> String {
    unsafe {
        let p: *const std::os::raw::c_char = msg_send![s, UTF8String];
        if p.is_null() {
            return String::new();
        }
        std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}

/// Running from Wezzy.app (has a bundle id), so UserNotifications is usable.
fn bundled() -> bool {
    static BUNDLED: OnceLock<bool> = OnceLock::new();
    *BUNDLED.get_or_init(|| unsafe {
        let bundle: Option<Retained<AnyObject>> = msg_send![class!(NSBundle), mainBundle];
        let id: Option<Retained<AnyObject>> = bundle.and_then(|b| msg_send![&b, bundleIdentifier]);
        id.is_some() && AnyClass::get(c"UNUserNotificationCenter").is_some()
    })
}

fn center() -> Option<Retained<AnyObject>> {
    if !bundled() {
        return None;
    }
    unsafe { msg_send![class!(UNUserNotificationCenter), currentNotificationCenter] }
}

fn identifier(pane_id: PaneId) -> Option<String> {
    crate::session::remote_pane_id(pane_id).map(|r| format!("{ID_PREFIX}{r}"))
}

/// Install the click delegate and clear what an earlier run left in
/// Notification Center. Doesn't ask for permission.
pub fn start() {
    let Some(center) = center() else {
        return;
    };
    static DELEGATE: OnceLock<usize> = OnceLock::new();
    let delegate = *DELEGATE.get_or_init(|| unsafe {
        let obj: *mut AnyObject = msg_send![delegate_class(), new];
        obj as usize // lives for the whole run; the center only holds it weakly
    });
    unsafe {
        let _: () = msg_send![&center, setDelegate: delegate as *mut AnyObject];
        let _: () = msg_send![&center, removeAllDeliveredNotifications];
    }
}

fn delegate_class() -> &'static AnyClass {
    // userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:
    unsafe extern "C-unwind" fn did_receive(
        _this: &AnyObject,
        _sel: Sel,
        _center: *mut AnyObject,
        response: *mut AnyObject,
        done: *mut Block<dyn Fn()>,
    ) {
        if let Some(response) = response.as_ref() {
            let request: Option<Retained<AnyObject>> = msg_send![response, notification];
            let request: Option<Retained<AnyObject>> = request.and_then(|n| msg_send![&n, request]);
            let id: Option<Retained<AnyObject>> = request.and_then(|r| msg_send![&r, identifier]);
            let remote = id
                .map(|s| to_string(&s))
                .and_then(|s| s.strip_prefix(ID_PREFIX)?.parse::<PaneId>().ok());
            if let Some(remote) = remote {
                crate::app::send_event(crate::app::AppEvent::NotificationClicked(remote));
            }
        }
        if let Some(done) = done.as_ref() {
            done.call(());
        }
    }

    let mut b = ClassBuilder::new(c"WezzyNotificationDelegate", class!(NSObject))
        .expect("notification delegate class");
    if let Some(p) = AnyProtocol::get(c"UNUserNotificationCenterDelegate") {
        b.add_protocol(p);
    }
    unsafe {
        b.add_method(
            sel!(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:),
            did_receive as unsafe extern "C-unwind" fn(_, _, _, _, _),
        );
    }
    b.register()
}

/// Post (or replace) the notification for an agent's pane. Asks for
/// permission the first time; after that the stored answer is reused.
pub fn post(pane_id: PaneId, title: String, body: String) {
    if !crate::settings::get().notifications {
        return;
    }
    let (Some(center), Some(id)) = (center(), identifier(pane_id)) else {
        return;
    };
    // called on a UserNotifications queue; its API is thread-safe
    let then = RcBlock::new(move |granted: Bool, _err: *mut AnyObject| {
        if !granted.as_bool() {
            return;
        }
        let Some(center) = self::center() else {
            return;
        };
        unsafe {
            let content: Retained<AnyObject> = msg_send![class!(UNMutableNotificationContent), new];
            let _: () = msg_send![&content, setTitle: &*nsstring(&title)];
            if !body.is_empty() {
                let _: () = msg_send![&content, setBody: &*nsstring(&body)];
            }
            let _: () = msg_send![&content, setThreadIdentifier: &*nsstring(&id)];
            let request: Retained<AnyObject> = msg_send![
                class!(UNNotificationRequest),
                requestWithIdentifier: &*nsstring(&id),
                content: &*content,
                trigger: std::ptr::null_mut::<AnyObject>()
            ];
            let _: () = msg_send![
                &center,
                addNotificationRequest: &*request,
                withCompletionHandler: std::ptr::null_mut::<Block<dyn Fn(*mut AnyObject)>>()
            ];
        }
    });
    unsafe {
        let _: () = msg_send![
            &center,
            requestAuthorizationWithOptions: AUTH_OPTIONS,
            completionHandler: &*then
        ];
    }
}

/// Take a pane's notification out of Notification Center (you went to it).
pub fn withdraw(pane_id: PaneId) {
    let (Some(center), Some(id)) = (center(), identifier(pane_id)) else {
        return;
    };
    unsafe {
        let ids: Retained<AnyObject> = msg_send![class!(NSArray), arrayWithObject: &*nsstring(&id)];
        let _: () = msg_send![&center, removeDeliveredNotificationsWithIdentifiers: &*ids];
    }
}

/// Show how many agents are waiting on you on the Dock icon (none: no badge).
pub fn sync_badge() {
    static SHOWN: Mutex<usize> = Mutex::new(0);
    let n = if crate::settings::get().notifications {
        crate::agents::waiting_count()
    } else {
        0
    };
    let mut shown = SHOWN.lock();
    if *shown == n {
        return;
    }
    *shown = n;
    unsafe {
        let app: Option<Retained<AnyObject>> = msg_send![class!(NSApplication), sharedApplication];
        let tile: Option<Retained<AnyObject>> = app.and_then(|a| msg_send![&a, dockTile]);
        let Some(tile) = tile else {
            return;
        };
        let label = (n > 0).then(|| nsstring(&n.to_string()));
        let _: () = msg_send![&tile, setBadgeLabel: label.as_deref()];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::raw::c_char;

    type Obj = *mut AnyObject;

    /// Every message notify.rs sends matches the real method's signature, so
    /// dev builds (which check this at the call) can't panic. Posts nothing.
    #[test]
    fn signatures_match() {
        let center = AnyClass::get(c"UNUserNotificationCenter").unwrap();
        center.metaclass().verify_sel::<(), Obj>(sel!(currentNotificationCenter)).unwrap();
        center.verify_sel::<(Obj,), ()>(sel!(setDelegate:)).unwrap();
        center.verify_sel::<(), ()>(sel!(removeAllDeliveredNotifications)).unwrap();
        center
            .verify_sel::<(usize, &Block<dyn Fn(Bool, Obj)>), ()>(sel!(
                requestAuthorizationWithOptions:completionHandler:
            ))
            .unwrap();
        center
            .verify_sel::<(&AnyObject, *mut Block<dyn Fn(Obj)>), ()>(sel!(
                addNotificationRequest:withCompletionHandler:
            ))
            .unwrap();
        center
            .verify_sel::<(&AnyObject,), ()>(sel!(removeDeliveredNotificationsWithIdentifiers:))
            .unwrap();

        let content = AnyClass::get(c"UNMutableNotificationContent").unwrap();
        for s in [sel!(setTitle:), sel!(setBody:), sel!(setThreadIdentifier:)] {
            content.verify_sel::<(&AnyObject,), ()>(s).unwrap();
        }
        let request = AnyClass::get(c"UNNotificationRequest").unwrap();
        request
            .metaclass()
            .verify_sel::<(&AnyObject, &AnyObject, Obj), Obj>(sel!(requestWithIdentifier:content:trigger:))
            .unwrap();
        request.verify_sel::<(), Obj>(sel!(identifier)).unwrap();
        AnyClass::get(c"UNNotificationResponse")
            .unwrap()
            .verify_sel::<(), Obj>(sel!(notification))
            .unwrap();
        AnyClass::get(c"UNNotification").unwrap().verify_sel::<(), Obj>(sel!(request)).unwrap();
        AnyClass::get(c"NSDockTile")
            .unwrap()
            .verify_sel::<(Option<&AnyObject>,), ()>(sel!(setBadgeLabel:))
            .unwrap();
        class!(NSString)
            .metaclass()
            .verify_sel::<(*const c_char,), Obj>(sel!(stringWithUTF8String:))
            .unwrap();
        class!(NSArray).metaclass().verify_sel::<(&AnyObject,), Obj>(sel!(arrayWithObject:)).unwrap();
        // the delegate class registers and answers the click selector
        assert!(delegate_class()
            .instance_method(sel!(
                userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:
            ))
            .is_some());
    }

    #[test]
    fn strings_round_trip_and_tests_are_unbundled() {
        assert_eq!(to_string(&nsstring("Claude · wezzy")), "Claude · wezzy");
        assert!(!bundled());
    }
}
