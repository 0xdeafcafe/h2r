//! Launch Services opens the game's macOS bundle with a Get URL Apple event.
//! Keep winit's application delegate. Install this one event handler during
//! applicationWillFinishLaunching, before AppKit dispatches the launch event.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use objc2::rc::Retained;
use objc2::{ClassType, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSAlert, NSApplication, NSApplicationWillFinishLaunchingNotification};
use objc2_foundation::{
    NSAppleEventDescriptor, NSAppleEventManager, NSNotification, NSNotificationCenter, NSObject,
    NSString,
};

// kInternetEventClass and kAEGetURL share the same four-character code.
const GET_URL: u32 = u32::from_be_bytes(*b"GURL");
const DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");

pub fn resources(executable: &Path) -> Option<PathBuf> {
    let contents = executable.parent()?.parent()?;
    (executable.ends_with("Contents/MacOS/Halo2") && contents.parent()?.extension()? == "app")
        .then(|| contents.join("Resources"))
}

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    struct JoinEvents;

    impl JoinEvents {
        #[unsafe(method(willFinishLaunching:))]
        fn will_finish(&self, _notification: &NSNotification) {
            // SAFETY: This is NSAppleEventManager's registration method with
            // AEEventClass/AEEventID (both UInt32). The selector below takes
            // its event and optional reply descriptors and returns void.
            unsafe {
                let _: () = msg_send![
                    &*NSAppleEventManager::sharedAppleEventManager(),
                    setEventHandler: self,
                    andSelector: sel!(openGameURL:withReply:),
                    forEventClass: GET_URL,
                    andEventID: GET_URL,
                ];
            }
        }

        #[unsafe(method(openGameURL:withReply:))]
        fn open_url(&self, event: &NSAppleEventDescriptor, _reply: Option<&NSAppleEventDescriptor>) {
            let result = event_link(event)
                .and_then(|link| live_hle::join_link::submit(link).map_err(anyhow::Error::msg));
            if let Err(error) = result {
                alert(self.mtm(), "Can't open this game link", &error.to_string());
            }
        }
    }
);

fn event_link(event: &NSAppleEventDescriptor) -> Result<live_proto::JoinLink> {
    // SAFETY: paramDescriptorForKeyword: takes an AEKeyword (UInt32) and
    // returns a nullable NSAppleEventDescriptor.
    let descriptor: Option<Retained<NSAppleEventDescriptor>> =
        unsafe { msg_send![event, paramDescriptorForKeyword: DIRECT_OBJECT] };
    let value = descriptor
        .and_then(|descriptor| descriptor.stringValue())
        .context("The link did not contain a URL.")?;
    Ok(value.to_string().parse()?)
}

fn alert(mtm: MainThreadMarker, title: &str, detail: &str) {
    let _application = NSApplication::sharedApplication(mtm);
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(title));
    alert.setInformativeText(&NSString::from_str(detail));
    alert.runModal();
}

pub fn play(resources: &Path) -> Result<()> {
    let mtm = MainThreadMarker::new().context("Launch the game on the main thread.")?;
    // SAFETY: JoinEvents inherits +new and has no instance variables.
    let handler: Retained<JoinEvents> = unsafe { msg_send![JoinEvents::class(), new] };
    let center = NSNotificationCenter::defaultCenter();
    // SAFETY: The notification is delivered on the main thread. The selector
    // accepts one NSNotification. We retain the observer through app::run.
    unsafe {
        center.addObserver_selector_name_object(
            &handler,
            sel!(willFinishLaunching:),
            Some(NSApplicationWillFinishLaunchingNotification),
            None,
        );
    }
    let result = play_disc(resources);
    // SAFETY: Both registrations belong to our retained handler. The Apple
    // event handler may never have run if the disc failed before launch.
    unsafe {
        center.removeObserver(&handler);
        let _: () = msg_send![
            &*NSAppleEventManager::sharedAppleEventManager(),
            removeEventHandlerForEventClass: GET_URL,
            andEventID: GET_URL,
        ];
    }
    if let Err(error) = &result {
        alert(mtm, "Couldn't start Halo 2", &format!("{error:#}"));
    }
    result
}

fn play_disc(resources: &Path) -> Result<()> {
    let disc = std::fs::read_to_string(resources.join("disc-path"))
        .context("Reinstall the app with just mac-install to select your Halo 2 disc.")?;
    ensure!(
        Path::new(&disc).is_absolute() && Path::new(&disc).is_file(),
        "Can't find the Halo 2 disc at {disc}. Reinstall with just mac-install iso=PATH."
    );
    crate::play_cmd::run(vec![disc, "--live".into()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_url_event_reads_only_a_validated_direct_object() {
        let target = NSAppleEventDescriptor::currentProcessDescriptor();
        // SAFETY: Construct a local, unsent GURL Apple event. Its codes
        // are UInt32, return ID is SInt16, and transaction ID is SInt32.
        let event: Retained<NSAppleEventDescriptor> = unsafe {
            msg_send![
                NSAppleEventDescriptor::class(),
                appleEventWithEventClass: GET_URL,
                eventID: GET_URL,
                targetDescriptor: &*target,
                returnID: -1_i16,
                transactionID: 0_i32,
            ]
        };
        assert!(event_link(&event).is_err());
        let set_url = |text: &str| {
            let descriptor = NSAppleEventDescriptor::descriptorWithString(&NSString::from_str(text));
            // SAFETY: setParamDescriptor:forKeyword: takes a descriptor
            // and a UInt32 keyword, and returns void.
            unsafe {
                let _: () = msg_send![&*event, setParamDescriptor: &*descriptor, forKeyword: DIRECT_OBJECT];
            }
        };
        set_url("h2recomp://other?session=8000000000001234");
        assert!(event_link(&event).is_err());
        set_url("h2recomp://join?server=https%3A%2F%2Flive.example&session=8000000000001234");
        let link = event_link(&event).unwrap();
        assert_eq!(link.session(), "8000000000001234");
        assert!(link.matches_server("https://live.example"));
    }

    #[test]
    fn only_the_bundled_executable_uses_finder_launch_arguments() {
        assert_eq!(
            resources(Path::new("/Applications/My Game.app/Contents/MacOS/Halo2")),
            Some(PathBuf::from("/Applications/My Game.app/Contents/Resources"))
        );
        assert!(resources(Path::new("/usr/local/bin/h2")).is_none());
        assert!(resources(Path::new("/apps/Contents/MacOS/Halo2")).is_none());
    }
}
