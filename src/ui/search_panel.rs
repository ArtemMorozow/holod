use std::cell::RefCell;
use std::collections::HashMap;
use std::error::Error;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyClass, ProtocolObject, Sel};
use objc2::{MainThreadMarker, MainThreadOnly, define_class, sel};
use objc2_app_kit::{
    NSAccessibility, NSAlert, NSAlertFirstButtonReturn, NSApplication,
    NSApplicationActivationPolicy, NSControl, NSControlTextEditingDelegate, NSImage, NSMenu,
    NSMenuItem, NSRunningApplication, NSSquareStatusItemLength, NSStatusBar, NSStatusItem,
    NSTableColumn, NSTableView, NSTableViewDataSource, NSTableViewDelegate, NSTextFieldDelegate,
    NSTextView, NSView, NSWorkspace, NSWorkspaceApplicationKey,
    NSWorkspaceDidActivateApplicationNotification,
};
use objc2_application_services::{AXObserver, AXUIElement};
use objc2_core_foundation::CFString;
use objc2_foundation::{
    NSBundle, NSData, NSIndexSet, NSNotification, NSNotificationCenter, NSObject, NSObjectProtocol,
    NSOperationQueue, NSSize, NSString, NSURL,
};

use crate::core::search::matching_rows;
use crate::core::window::{WindowError, WindowInfo};
use crate::macos::{
    accessibility, hotkey, window_backend::MacWindowBackend, window_history::WindowHistory,
};

use super::panel_view::{HELP, PanelView, install_edit_menu, window_row};

thread_local! {
    static PANEL: RefCell<Option<Rc<SearchPanel>>> = const { RefCell::new(None) };
}

pub fn run() -> Result<(), Box<dyn Error>> {
    if AnyClass::get(c"NSGlassEffectView").is_none() {
        return Err("The holod search panel requires macOS 26 or later.".into());
    }
    let mtm = MainThreadMarker::new().ok_or("AppKit must run on the main thread.")?;
    let app = NSApplication::sharedApplication(mtm);
    if app.activationPolicy() != NSApplicationActivationPolicy::Accessory
        && !app.setActivationPolicy(NSApplicationActivationPolicy::Accessory)
    {
        return Err("Could not start the AppKit accessory application.".into());
    }

    if let Err(error) = accessibility::require_access() {
        if NSBundle::mainBundle().bundleIdentifier().is_some() {
            // Start the AppKit event loop before presenting the first-launch modal.
            DispatchQueue::main().exec_async(|| {
                let mtm = MainThreadMarker::new().expect("AppKit runs on the main queue");
                let app = NSApplication::sharedApplication(mtm);
                show_accessibility_alert(mtm, &app);
                app.terminate(None);
            });
            app.run();
        }
        return Err(error.into());
    }

    let monitors = hotkey::register(mtm, || {
        DispatchQueue::main().exec_async(|| with_panel(SearchPanel::toggle));
    })?;
    install_edit_menu(mtm, &app);
    let (requests, rx) = mpsc::channel();
    let workspace_observer = WorkspaceObserver::install(requests.clone());
    let panel = SearchPanel::new(mtm, requests);
    let status_menu = StatusMenu::install(mtm, &app, &panel.controller)?;
    let worker = thread::Builder::new()
        .name("window-accessibility".into())
        .spawn(move || work(rx))?;
    panel.observe_frontmost();
    PANEL.with(|slot| slot.replace(Some(Rc::new(panel))));

    eprintln!("holod is running. Tap Option twice to search.");
    app.run();

    drop(status_menu);
    drop(monitors);
    drop(workspace_observer);
    drop(PANEL.with(|slot| slot.take()));
    if worker.join().is_err() {
        tracing::error!("Window worker exited unexpectedly");
    }
    Ok(())
}

struct StatusMenu {
    bar: Retained<NSStatusBar>,
    item: Retained<NSStatusItem>,
}

impl StatusMenu {
    fn install(
        mtm: MainThreadMarker,
        app: &NSApplication,
        controller: &PanelController,
    ) -> Result<Self, &'static str> {
        let data = NSData::with_bytes(include_bytes!("../../assets/holod-status.png"));
        let icon = NSImage::initWithData(mtm.alloc(), &data)
            .ok_or("Could not load the holod menu bar icon.")?;
        icon.setSize(NSSize::new(20.0, 20.0));
        icon.setTemplate(true);

        let bar = NSStatusBar::systemStatusBar();
        let item = bar.statusItemWithLength(NSSquareStatusItemLength);
        let status_menu = Self { bar, item };
        let button = status_menu
            .item
            .button(mtm)
            .ok_or("Could not create the holod menu bar button.")?;
        let name = NSString::from_str("holod");
        button.setImage(Some(&icon));
        button.setAccessibilityLabel(Some(&name));
        button.setToolTip(Some(&name));

        let menu = NSMenu::initWithTitle(mtm.alloc(), &name);
        let empty = NSString::new();
        // SAFETY: These selectors match the target objects and both targets outlive the menu.
        unsafe {
            let search = NSMenuItem::initWithTitle_action_keyEquivalent(
                mtm.alloc(),
                &NSString::from_str("Search Windows"),
                Some(sel!(showSearchPanel:)),
                &empty,
            );
            search.setTarget(Some(controller));
            menu.addItem(&search);
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            let quit = NSMenuItem::initWithTitle_action_keyEquivalent(
                mtm.alloc(),
                &NSString::from_str("Quit holod"),
                Some(sel!(terminate:)),
                &empty,
            );
            quit.setTarget(Some(app));
            menu.addItem(&quit);
        }
        status_menu.item.setMenu(Some(&menu));
        Ok(status_menu)
    }
}

impl Drop for StatusMenu {
    fn drop(&mut self) {
        self.item.setMenu(None);
        self.bar.removeStatusItem(&self.item);
    }
}

fn show_accessibility_alert(mtm: MainThreadMarker, app: &NSApplication) {
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str("Allow holod in Accessibility"));
    alert.setInformativeText(&NSString::from_str(
        "holod needs Accessibility to switch windows and detect two Option taps. Under Privacy & Security → Accessibility, enable holod. If it’s missing, use + to add it from Applications. Then open holod again.",
    ));
    alert.addButtonWithTitle(&NSString::from_str("Open System Settings"));
    alert.addButtonWithTitle(&NSString::from_str("Quit"));
    app.activate();
    if alert.runModal() == NSAlertFirstButtonReturn {
        let settings = NSString::from_str(
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
        );
        let workspace = NSWorkspace::sharedWorkspace();
        if NSURL::URLWithString(&settings).is_some_and(|url| workspace.openURL(&url)) {
            return;
        }
        let fallback = NSString::from_str("/System/Applications/System Settings.app");
        workspace.openURL(&NSURL::fileURLWithPath(&fallback));
    }
}

fn with_panel<R>(callback: impl FnOnce(&SearchPanel) -> R) -> R
where
    R: Default,
{
    let panel = PANEL.with(|slot| slot.borrow().clone());
    panel.as_deref().map(callback).unwrap_or_default()
}

enum Request {
    Observe(i32),
    List(u64, Option<i32>),
    Focus(u64, String),
}

struct WorkspaceObserver {
    center: Retained<NSNotificationCenter>,
    token: Retained<ProtocolObject<dyn NSObjectProtocol>>,
}

impl WorkspaceObserver {
    fn install(requests: Sender<Request>) -> Self {
        let workspace = NSWorkspace::sharedWorkspace();
        let center = workspace.notificationCenter();
        // SAFETY: These immutable AppKit constants identify the notification and its app payload.
        let (notification_name, app_key) = unsafe {
            (
                NSWorkspaceDidActivateApplicationNotification,
                NSWorkspaceApplicationKey,
            )
        };
        let activate = RcBlock::new(move |notification: NonNull<NSNotification>| {
            // SAFETY: AppKit provides a live notification for this callback.
            let notification = unsafe { notification.as_ref() };
            let app = notification
                .userInfo()
                .and_then(|info| info.objectForKey(app_key))
                .and_then(|app| app.downcast::<NSRunningApplication>().ok());
            if let Some(pid) = app.as_deref().and_then(user_app_pid) {
                let _ = requests.send(Request::Observe(pid));
            }
        });
        // SAFETY: The sendable callback is retained by the notification center and queued on main.
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(notification_name),
                Some(&workspace),
                Some(&NSOperationQueue::mainQueue()),
                &activate,
            )
        };
        Self { center, token }
    }
}

impl Drop for WorkspaceObserver {
    fn drop(&mut self) {
        // SAFETY: This token was registered with the same notification center.
        unsafe { self.center.removeObserver(self.token.as_ref()) };
    }
}

fn user_app_pid(app: &NSRunningApplication) -> Option<i32> {
    let pid = app.processIdentifier();
    (app.activationPolicy() == NSApplicationActivationPolicy::Regular && pid > 0).then_some(pid)
}

fn frontmost_pid() -> Option<i32> {
    NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .as_deref()
        .and_then(user_app_pid)
}

// SAFETY: The observer source runs on the main run loop and this callback ignores its AX arguments.
unsafe extern "C-unwind" fn focused_window_changed(
    _observer: NonNull<AXObserver>,
    _element: NonNull<AXUIElement>,
    _notification: NonNull<CFString>,
    _context: *mut c_void,
) {
    with_panel(SearchPanel::observe_frontmost);
}

fn work(requests: Receiver<Request>) {
    let backend = MacWindowBackend::default();
    let mut history = WindowHistory::default();
    if let Err(error) = accessibility::configure_timeout() {
        tracing::debug!(%error, "Could not set window tracking timeout");
    }
    for request in requests {
        autoreleasepool(|_| match request {
            Request::Observe(pid) => {
                history.observe(pid, Some(focused_window_changed));
            }
            Request::List(generation, pid) => {
                if let Some(pid) = pid {
                    history.observe(pid, Some(focused_window_changed));
                }
                let result = backend.list_for_search(&history);
                DispatchQueue::main().exec_async(move || {
                    with_panel(|panel| panel.receive_windows(generation, result));
                });
            }
            Request::Focus(generation, id) => {
                let result = backend
                    .focus_search_window(&id)
                    .map(|window| history.record(window));
                DispatchQueue::main().exec_async(move || {
                    with_panel(|panel| panel.receive_focus(generation, result));
                });
            }
        });
    }
}

#[derive(Default)]
enum Status {
    #[default]
    Ready,
    Loading,
    Focusing,
    Error(String),
}

#[derive(Default)]
struct State {
    generation: u64,
    windows: Vec<WindowInfo>,
    matches: Vec<usize>,
    status: Status,
}

struct SearchPanel {
    view: PanelView,
    state: RefCell<State>,
    icons: RefCell<HashMap<i32, Option<Retained<NSImage>>>>,
    generic_icon: Option<Retained<NSImage>>,
    requests: Sender<Request>,
    controller: Retained<PanelController>,
}

impl SearchPanel {
    fn new(mtm: MainThreadMarker, requests: Sender<Request>) -> Self {
        let view = PanelView::new(mtm);
        let controller = PanelController::new(mtm);
        // SAFETY: SearchPanel owns the controller and clears the delegates and target on drop.
        // PanelController implements the matching one-argument action.
        unsafe {
            view.search
                .setDelegate(Some(ProtocolObject::from_ref(&*controller)));
            view.table
                .setDataSource(Some(ProtocolObject::from_ref(&*controller)));
            view.table
                .setDelegate(Some(ProtocolObject::from_ref(&*controller)));
            view.table.setTarget(Some(&controller));
            view.table.setDoubleAction(Some(sel!(openSelectedWindow:)));
        }
        let generic_icon = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str("macwindow"),
            None,
        );
        Self {
            view,
            state: RefCell::new(State::default()),
            icons: RefCell::new(HashMap::new()),
            generic_icon,
            requests,
            controller,
        }
    }

    fn icon(&self, pid: i32) -> Option<Retained<NSImage>> {
        self.icons
            .borrow_mut()
            .entry(pid)
            .or_insert_with(|| {
                NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
                    .and_then(|app| app.icon())
            })
            .clone()
            .or_else(|| self.generic_icon.clone())
    }

    fn toggle(&self) {
        if self.view.window.isVisible() {
            self.close();
        } else {
            self.open();
        }
    }

    fn observe_frontmost(&self) {
        if let Some(pid) = frontmost_pid() {
            let _ = self.requests.send(Request::Observe(pid));
        }
    }

    fn open(&self) {
        if self.view.window.isVisible() {
            self.focus_search();
            return;
        }
        let pid = frontmost_pid();
        let generation = {
            let mut state = self.state.borrow_mut();
            state.generation += 1;
            state.windows.clear();
            state.matches.clear();
            state.status = Status::Loading;
            state.generation
        };
        self.icons.borrow_mut().clear();
        self.view.search.setStringValue(&NSString::new());
        self.refresh_rows();
        self.view.window.center();
        self.focus_search();
        self.view.window.invalidateShadow();
        if self.requests.send(Request::List(generation, pid)).is_err() {
            self.worker_unavailable();
        }
    }

    fn focus_search(&self) {
        self.view.window.makeKeyAndOrderFront(None);
        self.view.window.makeFirstResponder(Some(&self.view.search));
    }

    fn close(&self) {
        self.state.borrow_mut().generation += 1;
        self.view.window.orderOut(None);
    }

    fn receive_windows(&self, generation: u64, result: Result<Vec<WindowInfo>, WindowError>) {
        let mut state = self.state.borrow_mut();
        if state.generation != generation {
            return;
        }
        match result {
            Ok(windows) => {
                state.windows = windows;
                state.status = Status::Ready;
            }
            Err(error) => state.status = Status::Error(error.to_string()),
        }
        drop(state);
        self.filter();
    }

    fn filter(&self) {
        let query = self.view.search.currentEditor().map_or_else(
            || self.view.search.stringValue().to_string(),
            |editor| editor.string().to_string(),
        );
        let mut state = self.state.borrow_mut();
        state.matches = matching_rows(&state.windows, &query);
        drop(state);
        self.refresh_rows();
    }

    fn refresh_rows(&self) {
        self.view.table.reloadData();
        if !self.state.borrow().matches.is_empty() {
            self.select(0);
        }
        self.refresh_status();
    }

    fn refresh_status(&self) {
        let state = self.state.borrow();
        let message = match &state.status {
            Status::Loading => Some("Finding windows…"),
            Status::Error(message) if state.matches.is_empty() => Some(message.as_str()),
            Status::Ready if state.matches.is_empty() => Some("No matching windows"),
            _ => None,
        };
        let footer = match &state.status {
            Status::Error(error) if !state.matches.is_empty() => error,
            _ => HELP,
        };
        let count = match &state.status {
            Status::Loading | Status::Error(_) => String::new(),
            Status::Focusing => "Switching…".to_owned(),
            Status::Ready if state.matches.len() == 1 => "1 window".to_owned(),
            Status::Ready => format!("{} windows", state.matches.len()),
        };
        self.view
            .message
            .setStringValue(&NSString::from_str(message.unwrap_or_default()));
        self.view.message.setHidden(message.is_none());
        self.view
            .shortcuts
            .setStringValue(&NSString::from_str(footer));
        self.view.count.setStringValue(&NSString::from_str(&count));
    }

    fn move_selection(&self, offset: isize) {
        let count = self.state.borrow().matches.len() as isize;
        if count > 0 {
            let current = self.view.table.selectedRow();
            let next = if current < 0 {
                0
            } else {
                (current + offset).clamp(0, count - 1)
            };
            self.select(next as usize);
        }
    }

    fn select(&self, row: usize) {
        self.view
            .table
            .selectRowIndexes_byExtendingSelection(&NSIndexSet::indexSetWithIndex(row), false);
        self.view.table.scrollRowToVisible(row as isize);
    }

    fn focus_selected(&self) {
        let Ok(row) = usize::try_from(self.view.table.selectedRow()) else {
            return;
        };
        let mut state = self.state.borrow_mut();
        if matches!(state.status, Status::Focusing) {
            return;
        }
        let Some(&index) = state.matches.get(row) else {
            return;
        };
        let id = state.windows[index].id.clone();
        let generation = state.generation;
        state.status = Status::Focusing;
        drop(state);
        self.refresh_status();
        if self.requests.send(Request::Focus(generation, id)).is_err() {
            self.worker_unavailable();
        }
    }

    fn receive_focus(&self, generation: u64, result: Result<(), WindowError>) {
        if self.state.borrow().generation != generation {
            return;
        }
        match result {
            Ok(()) => self.close(),
            Err(error) => {
                tracing::debug!(%error, "Could not switch windows");
                self.state.borrow_mut().status = Status::Error(error.to_string());
                self.refresh_status();
            }
        }
    }

    fn worker_unavailable(&self) {
        self.state.borrow_mut().status =
            Status::Error("The window worker stopped. Restart holod.".into());
        self.refresh_status();
    }
}

impl Drop for SearchPanel {
    fn drop(&mut self) {
        // SAFETY: The controller is still retained while the AppKit references are cleared.
        unsafe {
            self.view.search.setDelegate(None);
            self.view.table.setDataSource(None);
            self.view.table.setDelegate(None);
            self.view.table.setTarget(None);
            self.view.table.setDoubleAction(None);
        }
        self.view.window.orderOut(None);
    }
}

// SAFETY: This NSObject subclass installs main-thread AppKit delegate callbacks only.
define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    struct PanelController;

    impl PanelController {
        #[unsafe(method(showSearchPanel:))]
        fn show_search_panel(&self, _sender: &NSMenuItem) {
            DispatchQueue::main().exec_async(|| with_panel(SearchPanel::open));
        }

        #[unsafe(method(openSelectedWindow:))]
        fn open_selected_window(&self, sender: &NSTableView) {
            if sender.clickedRow() >= 0 {
                with_panel(SearchPanel::focus_selected);
            }
        }
    }

    // SAFETY: The implementation uses NSObject's default protocol behavior.
    unsafe impl NSObjectProtocol for PanelController {}

    // SAFETY: All callbacks have the AppKit protocol signatures and run on the main thread.
    unsafe impl NSControlTextEditingDelegate for PanelController {
        #[unsafe(method(controlTextDidChange:))]
        fn text_changed(&self, _notification: &NSNotification) {
            with_panel(SearchPanel::filter);
        }

        #[unsafe(method(control:textView:doCommandBySelector:))]
        fn handle_command(&self, _control: &NSControl, _editor: &NSTextView, command: Sel) -> bool {
            with_panel(|panel| {
                if command == sel!(moveUp:) {
                    panel.move_selection(-1);
                } else if command == sel!(moveDown:) {
                    panel.move_selection(1);
                } else if command == sel!(insertNewline:) {
                    panel.focus_selected();
                } else if command == sel!(cancelOperation:) {
                    panel.close();
                } else {
                    return false;
                }
                true
            })
        }
    }

    // SAFETY: NSTextFieldDelegate inherits the implemented text-editing callbacks.
    unsafe impl NSTextFieldDelegate for PanelController {}

    // SAFETY: AppKit calls this data source on the main thread and expects a signed row count.
    unsafe impl NSTableViewDataSource for PanelController {
        #[unsafe(method(numberOfRowsInTableView:))]
        fn number_of_rows(&self, _table: &NSTableView) -> isize {
            with_panel(|panel| panel.state.borrow().matches.len() as isize)
        }
    }

    // SAFETY: AppKit calls the delegate on the main thread and retains each returned view.
    unsafe impl NSTableViewDelegate for PanelController {
        #[unsafe(method_id(tableView:viewForTableColumn:row:))]
        fn row_view(
            &self,
            _table: &NSTableView,
            _column: Option<&NSTableColumn>,
            row: isize,
        ) -> Option<Retained<NSView>> {
            with_panel(|panel| {
                let row = usize::try_from(row).ok()?;
                let state = panel.state.borrow();
                let window = state.windows.get(*state.matches.get(row)?)?;
                let icon = panel.icon(window.pid);
                Some(window_row(self.mtm(), window, icon.as_deref()))
            })
        }
    }
);

impl PanelController {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        // SAFETY: The subclass's Rust ivars are initialized before calling NSObject's initializer.
        unsafe { objc2::msg_send![super(mtm.alloc::<Self>().set_ivars(())), init] }
    }
}
