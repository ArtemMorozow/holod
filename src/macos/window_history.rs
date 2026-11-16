use std::ptr::{self, NonNull};

use objc2_application_services::{AXError, AXObserver, AXObserverCallback, AXUIElement};
use objc2_core_foundation::{
    CFRetained, CFRunLoop, CFRunLoopSource, CFString, kCFRunLoopCommonModes,
};

use crate::core::recency::Recency;
use crate::macos::accessibility as ax;

#[derive(Default)]
pub struct WindowHistory {
    pid: Option<i32>,
    current: Option<CFRetained<AXUIElement>>,
    recent: Recency<CFRetained<AXUIElement>>,
    observer: Option<FocusObserver>,
}

impl WindowHistory {
    pub fn observe(&mut self, pid: i32, callback: AXObserverCallback) {
        if self.pid != Some(pid) {
            self.pid = Some(pid);
            self.current = None;
            self.observer = None;
            match FocusObserver::install(pid, callback) {
                Ok(observer) => self.observer = Some(observer),
                Err(error) => {
                    tracing::debug!(pid, code = error.0, "Could not observe window focus")
                }
            }
        }

        let app = ax::application(pid);
        match ax::read(&app, "AXFocusedWindow") {
            Ok(value) => {
                if let Ok(window) = value.downcast::<AXUIElement>() {
                    self.record(window);
                }
            }
            Err(AXError::NoValue | AXError::InvalidUIElement | AXError::AttributeUnsupported) => {
                self.current = None;
            }
            Err(error) => tracing::debug!(pid, code = error.0, "Could not read focused window"),
        }
    }

    pub fn record(&mut self, window: CFRetained<AXUIElement>) {
        self.recent.visit(window.clone());
        self.current = Some(window);
    }

    pub fn sort_key(&self, window: &CFRetained<AXUIElement>) -> (bool, usize) {
        self.recent.sort_key(window, self.current.as_ref())
    }
}

struct FocusObserver {
    run_loop: CFRetained<CFRunLoop>,
    source: CFRetained<CFRunLoopSource>,
    _observer: CFRetained<AXObserver>,
}

impl FocusObserver {
    fn install(pid: i32, callback: AXObserverCallback) -> Result<Self, AXError> {
        let mut observer = ptr::null_mut();
        // SAFETY: The observer has a valid target PID, callback and writable out pointer.
        let result = unsafe { AXObserver::create(pid, callback, NonNull::from(&mut observer)) };
        if result != AXError::Success {
            return Err(result);
        }
        let observer = NonNull::new(observer).ok_or(AXError::Failure)?;
        // SAFETY: AXObserverCreate returned an owned observer on success.
        let observer = unsafe { CFRetained::from_raw(observer) };
        let app = ax::application(pid);
        let mut registration = AXError::NotificationUnsupported;
        for name in ["AXFocusedWindowChanged", "AXMainWindowChanged"] {
            let notification = CFString::from_static_str(name);
            // SAFETY: Both AX objects are live and the callback uses no per-observer context.
            registration =
                unsafe { observer.add_notification(&app, &notification, ptr::null_mut()) };
            if registration == AXError::Success {
                break;
            }
        }
        if registration != AXError::Success {
            return Err(registration);
        }
        // SAFETY: The observer is live and owns the source while this call retains it.
        let source = unsafe { observer.run_loop_source() };
        let run_loop = CFRunLoop::main().ok_or(AXError::Failure)?;
        // SAFETY: This immutable Core Foundation constant is available on macOS.
        let mode = unsafe { kCFRunLoopCommonModes };
        // The main run loop only signals the worker. AX requests stay on the worker thread.
        run_loop.add_source(Some(&source), mode);
        Ok(Self {
            run_loop,
            source,
            _observer: observer,
        })
    }
}

impl Drop for FocusObserver {
    fn drop(&mut self) {
        // SAFETY: This immutable mode is the same one used to install the source.
        let mode = unsafe { kCFRunLoopCommonModes };
        self.run_loop.remove_source(Some(&self.source), mode);
    }
}
