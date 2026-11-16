use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, NSObject};
use objc2::{MainThreadMarker, MainThreadOnly, extern_class, extern_methods, sel};
use objc2_app_kit::{NSMenu, NSMenuItem};
use objc2_foundation::{NSBundle, NSString};

// SAFETY: Sparkle's controller inherits NSObject and must be used on the main thread.
extern_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SPUStandardUpdaterController"]
    struct UpdaterController;
);

impl UpdaterController {
    extern_methods!(
        #[unsafe(method(initWithStartingUpdater:updaterDelegate:userDriverDelegate:))]
        fn init(
            this: Allocated<Self>,
            starting_updater: bool,
            updater_delegate: Option<&AnyObject>,
            user_driver_delegate: Option<&AnyObject>,
        ) -> Retained<Self>;
    );
}

pub(super) struct Updater {
    controller: Retained<UpdaterController>,
    _framework: Retained<NSBundle>,
}

impl Updater {
    pub(super) fn load(mtm: MainThreadMarker) -> Option<Self> {
        let app = NSBundle::mainBundle();
        app.objectForInfoDictionaryKey(&NSString::from_str("SUFeedURL"))?;
        let framework = app
            .privateFrameworksURL()
            .and_then(|url| {
                url.URLByAppendingPathComponent(&NSString::from_str("Sparkle.framework"))
            })
            .and_then(|url| NSBundle::bundleWithURL(&url));
        let Some(framework) = framework else {
            tracing::error!("The packaged Sparkle framework is missing");
            return None;
        };
        // SAFETY: Load the Sparkle framework embedded in this app's signed bundle on the
        // main thread. Keep the bundle retained while the updater uses its classes.
        if let Err(error) = unsafe { framework.loadAndReturnError() } {
            tracing::error!(%error, "Could not load Sparkle");
            return None;
        }
        if AnyClass::get(c"SPUStandardUpdaterController").is_none() {
            tracing::error!("The packaged Sparkle framework has no updater controller");
            return None;
        }
        let controller = UpdaterController::init(mtm.alloc(), true, None, None);
        Some(Self {
            controller,
            _framework: framework,
        })
    }

    pub(super) fn add_menu_item(&self, mtm: MainThreadMarker, menu: &NSMenu) {
        // SAFETY: Sparkle implements this one-argument action. StatusMenu clears its menu
        // before releasing the updater controller, which AppKit holds as a weak target.
        unsafe {
            let item = NSMenuItem::initWithTitle_action_keyEquivalent(
                mtm.alloc(),
                &NSString::from_str("Check for Updates…"),
                Some(sel!(checkForUpdates:)),
                &NSString::new(),
            );
            item.setTarget(Some(&self.controller));
            menu.addItem(&item);
        }
    }
}
