use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly, extern_class, extern_methods};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSApplication, NSControlStateValueOff,
    NSControlStateValueOn, NSMenuItem,
};
use objc2_foundation::{NSBundle, NSError, NSObject, NSString, NSUserDefaults};

const PROMPT_SEEN: &str = "StartAtLoginPromptSeen";
pub(super) const MENU_TITLE: &str = "Start at Login";

// SMAppServiceStatus values from ServiceManagement/SMAppService.h.
const ENABLED: isize = 1;
const REQUIRES_APPROVAL: isize = 2;

#[link(name = "ServiceManagement", kind = "framework")]
unsafe extern "C" {}

// SAFETY: SMAppService is an NSObject subclass available on the supported macOS versions.
extern_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SMAppService"]
    pub(super) struct LoginItem;
);

impl LoginItem {
    extern_methods!(
        #[unsafe(method(mainAppService))]
        fn main_app_service(mtm: MainThreadMarker) -> Retained<Self>;

        #[unsafe(method(status))]
        fn status(&self) -> isize;

        #[unsafe(method(registerAndReturnError:_))]
        fn register(&self) -> Result<(), Retained<NSError>>;

        #[unsafe(method(openSystemSettingsLoginItems))]
        fn open_system_settings(mtm: MainThreadMarker);
    );

    pub(super) fn for_main_bundle(mtm: MainThreadMarker) -> Option<Retained<Self>> {
        let bundle = NSBundle::mainBundle();
        if bundle.bundleIdentifier().is_none()
            || !bundle
                .bundleURL()
                .pathExtension()
                .is_some_and(|extension| extension.to_string().eq_ignore_ascii_case("app"))
        {
            return None;
        }
        Some(Self::main_app_service(mtm))
    }

    pub(super) fn offer(&self) {
        let defaults = NSUserDefaults::standardUserDefaults();
        let key = NSString::from_str(PROMPT_SEEN);
        if defaults.boolForKey(&key) {
            return;
        }
        defaults.setBool_forKey(true, &key);
        if matches!(self.status(), ENABLED | REQUIRES_APPROVAL) {
            return;
        }

        let mtm = self.mtm();
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str("Start Holod at login?"));
        alert.setInformativeText(&NSString::from_str(
            "Holod can open automatically when you sign in to your Mac. You can also enable this from the Holod menu.",
        ));
        alert.addButtonWithTitle(&NSString::from_str(MENU_TITLE));
        let skip = alert.addButtonWithTitle(&NSString::from_str("Not Now"));
        skip.setKeyEquivalent(&NSString::from_str("\u{1b}"));
        NSApplication::sharedApplication(mtm).activate();
        if alert.runModal() == NSAlertFirstButtonReturn {
            self.enable();
        }
    }

    pub(super) fn update_menu_item(&self, item: &NSMenuItem) -> bool {
        let status = self.status();
        let title = if status == REQUIRES_APPROVAL {
            "Approve Start at Login…"
        } else {
            MENU_TITLE
        };
        item.setTitle(&NSString::from_str(title));
        item.setState(if status == ENABLED {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        status != ENABLED
    }

    pub(super) fn enable(&self) {
        let mtm = self.mtm();
        match self.status() {
            ENABLED => return,
            REQUIRES_APPROVAL => return Self::open_system_settings(mtm),
            _ => {}
        }

        let result = self.register();
        match self.status() {
            ENABLED => {}
            REQUIRES_APPROVAL => Self::open_system_settings(mtm),
            _ => {
                if let Err(error) = result {
                    let alert = NSAlert::new(mtm);
                    alert.setMessageText(&NSString::from_str("Could not start Holod at login"));
                    alert.setInformativeText(&error.localizedDescription());
                    alert.addButtonWithTitle(&NSString::from_str("OK"));
                    NSApplication::sharedApplication(mtm).activate();
                    alert.runModal();
                }
            }
        }
    }
}
