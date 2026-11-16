use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::thread;
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2_app_kit::{
    NSApplicationActivationOptions, NSApplicationActivationPolicy, NSRunningApplication,
    NSWorkspace,
};
use objc2_application_services::{AXError, AXUIElement, AXValue};
use objc2_core_foundation::{CFBoolean, CFRetained, CFString};

use crate::core::window::{WindowBackend, WindowError, WindowInfo};
use crate::macos::accessibility as ax;
use crate::macos::window_history::WindowHistory;

const FOCUS_WAIT: Duration = Duration::from_millis(650);
const FOCUS_INTERVAL: Duration = Duration::from_millis(35);

#[derive(Default)]
pub struct MacWindowBackend {
    // Each instance belongs to one thread. The panel keeps its AX objects on the worker.
    snapshot: RefCell<Option<Vec<Window>>>,
}

struct Window {
    info: WindowInfo,
    element: CFRetained<AXUIElement>,
    application: Retained<NSRunningApplication>,
}

impl MacWindowBackend {
    pub fn list_for_search(&self, history: &WindowHistory) -> Result<Vec<WindowInfo>, WindowError> {
        self.refresh(Some(history))
    }

    pub fn focus_search_window(&self, id: &str) -> Result<CFRetained<AXUIElement>, WindowError> {
        ax::require_access()?;
        let snapshot = self.snapshot.borrow();
        let target = snapshot
            .as_ref()
            .and_then(|windows| windows.iter().find(|window| window.info.id == id))
            .ok_or(WindowError::WindowUnavailable)?;
        focus(target)?;
        Ok(target.element.clone())
    }

    fn refresh(&self, history: Option<&WindowHistory>) -> Result<Vec<WindowInfo>, WindowError> {
        self.snapshot.replace(None);
        ax::require_access()?;
        ax::configure_timeout()?;

        let workspace = NSWorkspace::sharedWorkspace();
        let mut windows = Vec::new();
        for app in workspace
            .runningApplications()
            .to_vec()
            .into_iter()
            .filter(|app| is_user_app(app))
        {
            match app_windows(&app) {
                Ok(mut app_windows) => windows.append(&mut app_windows),
                Err(WindowError::AccessibilityDenied) => {
                    return Err(WindowError::AccessibilityDenied);
                }
                Err(error) => {
                    tracing::debug!(pid = app.processIdentifier(), %error, "Skipping application")
                }
            }
        }
        windows.sort_by_cached_key(|window| {
            (
                history.map(|history| history.sort_key(&window.element)),
                window.info.app_name.to_lowercase(),
                window.info.title.to_lowercase(),
                window.info.id.clone(),
            )
        });
        let info = windows.iter().map(|window| window.info.clone()).collect();
        self.snapshot.replace(Some(windows));
        Ok(info)
    }
}

impl WindowBackend for MacWindowBackend {
    fn list_windows(&self) -> Result<Vec<WindowInfo>, WindowError> {
        self.refresh(None)
    }

    fn focus_window(&self, id: &str) -> Result<(), WindowError> {
        if self.snapshot.borrow().is_some() {
            return self.focus_search_window(id).map(|_| ());
        }
        ax::require_access()?;

        let (pid, key) = id.split_once(':').ok_or(WindowError::WindowUnavailable)?;
        let pid = pid
            .parse::<i32>()
            .ok()
            .filter(|pid| *pid > 0)
            .ok_or(WindowError::WindowUnavailable)?;
        if key.contains(':') {
            return Err(WindowError::AmbiguousWindow);
        }

        ax::configure_timeout()?;
        let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
            .filter(|app| is_user_app(app))
            .ok_or(WindowError::WindowUnavailable)?;
        let windows = app_windows(&app)?;
        let target = windows.iter().find(|window| window.info.id == id);
        focus(target.ok_or(WindowError::WindowUnavailable)?)
    }
}

fn is_user_app(app: &NSRunningApplication) -> bool {
    app.activationPolicy() == NSApplicationActivationPolicy::Regular
        && app.processIdentifier() > 0
        && !app.isTerminated()
}

fn app_windows(app: &Retained<NSRunningApplication>) -> Result<Vec<Window>, WindowError> {
    let pid = app.processIdentifier();
    let name = app
        .localizedName()
        .or_else(|| app.bundleIdentifier())
        .map(|name| name.to_string())
        .unwrap_or_else(|| format!("PID {pid}"));
    let launch = app
        .launchDate()
        .map(|date| date.timeIntervalSinceReferenceDate().to_bits());
    let root = ax::application(pid);
    let elements = match ax::windows(&root) {
        Ok(value) => value,
        Err(AXError::AttributeUnsupported | AXError::NoValue | AXError::NotImplemented) => {
            return Ok(Vec::new());
        }
        Err(error) => return ax::check(error, "read AXWindows").map(|()| Vec::new()),
    };
    let attributes = [
        "AXRole",
        "AXSubrole",
        "AXTitle",
        "AXIdentifier",
        "AXDocument",
        "AXPosition",
        "AXSize",
    ]
    .map(CFString::from_static_str);
    let attributes = attributes.each_ref().map(|name| &**name);
    let mut windows = Vec::new();
    let mut identities = HashMap::new();
    for element in elements
        .iter()
        .filter_map(|element| element.downcast::<AXUIElement>().ok())
    {
        let values = match ax::read_many(&element, &attributes) {
            Ok(values) => values,
            Err(AXError::InvalidUIElement | AXError::NoValue) => continue,
            Err(error) => return ax::check(error, "read window attributes").map(|()| Vec::new()),
        };
        let string = |index| {
            values
                .get(index)?
                .downcast::<CFString>()
                .ok()
                .map(|name| name.to_string())
        };
        let role = string(0);
        let subrole = string(1);
        if role.as_deref() != Some("AXWindow")
            || !matches!(subrole.as_deref(), Some("AXStandardWindow" | "AXDialog"))
        {
            continue;
        }
        let title = string(2).unwrap_or_default();
        let identifier = string(3);
        let document = string(4);
        let point = values
            .get(5)
            .and_then(|value| ax::point(value.downcast_ref::<AXValue>()?));
        let size = values
            .get(6)
            .and_then(|value| ax::size(value.downcast_ref::<AXValue>()?));
        let frame = point
            .zip(size)
            .map(|(point, size)| [point.x, point.y, size.width, size.height].map(f64::to_bits));
        let mut hash = DefaultHasher::new();
        (launch, subrole, &title, identifier, document, frame).hash(&mut hash);
        let fingerprint = hash.finish();
        let count = identities.entry(fingerprint).or_insert(0_usize);
        *count += 1;
        let ordinal = *count;
        windows.push((
            (fingerprint, ordinal),
            Window {
                info: WindowInfo {
                    id: String::new(),
                    pid,
                    app_name: name.clone(),
                    title: if title.is_empty() {
                        "(Untitled)".into()
                    } else {
                        title
                    },
                },
                element,
                application: app.clone(),
            },
        ));
    }

    Ok(windows
        .into_iter()
        .map(|((fingerprint, ordinal), mut window)| {
            window.info.id = if identities[&fingerprint] == 1 {
                format!("{pid}:{fingerprint:016x}")
            } else {
                format!("{pid}:{fingerprint:016x}:{ordinal}")
            };
            window
        })
        .collect())
}

fn focus(window: &Window) -> Result<(), WindowError> {
    if window.application.isTerminated() {
        return Err(WindowError::WindowUnavailable);
    }

    let root = ax::application(window.info.pid);
    let minimized = ax::read_bool(&window.element, "AXMinimized");
    match minimized {
        Ok(true) => ax::check(
            ax::set(&window.element, "AXMinimized", CFBoolean::new(false)),
            "restore window",
        )?,
        Ok(false) | Err(AXError::AttributeUnsupported | AXError::NoValue) => {}
        Err(error) => ax::check(error, "check minimized state")?,
    }
    if window.application.isHidden() {
        window.application.unhide();
    }

    optional(
        ax::set(&window.element, "AXMain", CFBoolean::new(true)),
        "make window main",
    )?;
    if ax::set(&root, "AXFrontmost", CFBoolean::new(true)) != AXError::Success {
        window
            .application
            .activateWithOptions(NSApplicationActivationOptions::empty());
    }
    optional(ax::raise(&window.element), "raise window")?;
    optional(
        ax::set(&root, "AXFocusedWindow", &window.element),
        "set focused window",
    )?;

    let deadline = Instant::now() + FOCUS_WAIT;
    let system = ax::system_wide();
    loop {
        let focused = match ax::read(&root, "AXFocusedWindow") {
            Ok(value) => value
                .downcast::<AXUIElement>()
                .is_ok_and(|focused| focused == window.element),
            Err(error @ (AXError::APIDisabled | AXError::InvalidUIElement)) => {
                ax::check(error, "read focused window")?;
                false
            }
            Err(_) => false,
        };
        let active_pid = ax::read(&system, "AXFocusedApplication")
            .and_then(|value| {
                value
                    .downcast::<AXUIElement>()
                    .map_err(|_| AXError::Failure)
            })
            .and_then(|app| ax::pid(&app));
        let application_active = match active_pid {
            Ok(pid) => pid == window.info.pid,
            Err(error) => {
                tracing::debug!(code = error.0, "System-wide AX focus unavailable");
                NSWorkspace::sharedWorkspace()
                    .frontmostApplication()
                    .is_some_and(|app| app.processIdentifier() == window.info.pid)
            }
        };
        if focused && application_active {
            return Ok(());
        }
        if Instant::now() >= deadline {
            tracing::debug!(
                focused,
                application_active,
                "Window focus could not be verified"
            );
            return Err(WindowError::ActivationFailed);
        }
        thread::sleep(FOCUS_INTERVAL);
    }
}

fn optional(result: AXError, operation: &'static str) -> Result<(), WindowError> {
    match result {
        AXError::AttributeUnsupported | AXError::ActionUnsupported | AXError::NotImplemented => {
            tracing::debug!(
                operation,
                code = result.0,
                "Optional AX operation unavailable"
            );
            Ok(())
        }
        _ => ax::check(result, operation),
    }
}
