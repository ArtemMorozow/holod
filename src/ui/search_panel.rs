use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use dispatch2::DispatchQueue;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{ProtocolObject, Sel};
use objc2::{MainThreadMarker, MainThreadOnly, define_class, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSColor, NSControl,
    NSControlTextEditingDelegate, NSFloatingWindowLevel, NSFont, NSLineBreakMode, NSPanel,
    NSScrollView, NSTableColumn, NSTableView, NSTableViewDataSource, NSTableViewDelegate,
    NSTableViewStyle, NSTextField, NSTextFieldDelegate, NSTextView, NSView,
    NSWindowCollectionBehavior, NSWindowStyleMask, NSWindowTitleVisibility,
};
use objc2_foundation::{
    NSIndexSet, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
};

use crate::core::search::matching_rows;
use crate::core::window::{WindowBackend, WindowError, WindowInfo};
use crate::macos::{hotkey, window_backend::MacWindowBackend};

thread_local! {
    static PANEL: RefCell<Option<Rc<SearchPanel>>> = const { RefCell::new(None) };
}

pub fn run(custom_hotkey: Option<&str>) -> Result<(), Box<dyn Error>> {
    let mtm = MainThreadMarker::new().ok_or("AppKit must run on the main thread.")?;
    let app = NSApplication::sharedApplication(mtm);
    if app.activationPolicy() != NSApplicationActivationPolicy::Accessory
        && !app.setActivationPolicy(NSApplicationActivationPolicy::Accessory)
    {
        return Err("Could not start the AppKit accessory application.".into());
    }

    let (manager, keyboard_shortcut) = hotkey::register(mtm, custom_hotkey, || {
        DispatchQueue::main().exec_async(|| with_panel(SearchPanel::toggle));
    })?;
    let (requests, rx) = mpsc::channel();
    let worker = thread::Builder::new()
        .name("window-accessibility".into())
        .spawn(move || work(rx))?;
    let panel = SearchPanel::new(mtm, requests);
    PANEL.with(|slot| slot.replace(Some(Rc::new(panel))));

    eprintln!("Window Switcher is running. Press {keyboard_shortcut} to search.");
    app.run();

    drop(PANEL.with(|slot| slot.take()));
    if let Err(error) = manager.unregister(keyboard_shortcut) {
        tracing::warn!(%error, "Could not unregister hotkey");
    }
    if worker.join().is_err() {
        tracing::error!("Window worker exited unexpectedly");
    }
    Ok(())
}

fn with_panel<R>(callback: impl FnOnce(&SearchPanel) -> R) -> R
where
    R: Default,
{
    let panel = PANEL.with(|slot| slot.borrow().clone());
    panel.as_deref().map(callback).unwrap_or_default()
}

enum Request {
    List(u64),
    Focus(u64, String),
}

fn work(requests: Receiver<Request>) {
    let backend = MacWindowBackend::default();
    for request in requests {
        autoreleasepool(|_| match request {
            Request::List(generation) => {
                let result = backend.list_windows();
                DispatchQueue::main().exec_async(move || {
                    with_panel(|panel| panel.receive_windows(generation, result));
                });
            }
            Request::Focus(generation, id) => {
                let result = backend.focus_window(&id);
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
    window: Retained<NSPanel>,
    search: Retained<NSTextField>,
    table: Retained<NSTableView>,
    status: Retained<NSTextField>,
    state: RefCell<State>,
    requests: Sender<Request>,
    _controller: Retained<PanelController>,
}

const WIDTH: f64 = 640.0;
const HEIGHT: f64 = 372.0;
const INSET: f64 = 18.0;
const CONTENT_WIDTH: f64 = WIDTH - 2.0 * INSET;
const APP_COLUMN: &str = "application";

fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
}

impl SearchPanel {
    fn new(mtm: MainThreadMarker, requests: Sender<Request>) -> Self {
        let window = NSPanel::initWithContentRect_styleMask_backing_defer(
            mtm.alloc(),
            rect(0.0, 0.0, WIDTH, HEIGHT),
            NSWindowStyleMask::Titled
                | NSWindowStyleMask::NonactivatingPanel
                | NSWindowStyleMask::FullSizeContentView,
            NSBackingStoreType::Buffered,
            false,
        );
        window.setTitle(&NSString::from_str("Window Switcher"));
        window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        window.setTitlebarAppearsTransparent(true);
        window.setFloatingPanel(true);
        window.setLevel(NSFloatingWindowLevel);
        window.setHidesOnDeactivate(false);
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::MoveToActiveSpace
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );

        let content = NSView::initWithFrame(mtm.alloc(), rect(0.0, 0.0, WIDTH, HEIGHT));
        window.setContentView(Some(&content));
        let controller = PanelController::new(mtm);

        let search = NSTextField::textFieldWithString(&NSString::new(), mtm);
        search.setFrame(rect(INSET, 315.0, CONTENT_WIDTH, 35.0));
        search.setFont(Some(&NSFont::systemFontOfSize(19.0)));
        search.setPlaceholderString(Some(&NSString::from_str("Search windows…")));
        // SAFETY: SearchPanel retains the delegate until it clears the field's delegate.
        unsafe { search.setDelegate(Some(ProtocolObject::from_ref(&*controller))) };
        content.addSubview(&search);
        window.setInitialFirstResponder(Some(&search));

        let scroll_frame = rect(INSET, 57.0, CONTENT_WIDTH, 244.0);
        let scroll = NSScrollView::initWithFrame(mtm.alloc(), scroll_frame);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setDrawsBackground(false);
        let table = NSTableView::initWithFrame(
            mtm.alloc(),
            rect(0.0, 0.0, CONTENT_WIDTH, scroll_frame.size.height),
        );
        table.setHeaderView(None);
        table.setStyle(NSTableViewStyle::Plain);
        table.setRowHeight(30.0);
        table.setIntercellSpacing(NSSize::new(0.0, 2.0));
        table.setUsesAlternatingRowBackgroundColors(true);
        table.setRefusesFirstResponder(true);
        table.setAllowsMultipleSelection(false);
        // SAFETY: SearchPanel retains the controller until it clears both table references.
        unsafe {
            table.setDataSource(Some(ProtocolObject::from_ref(&*controller)));
            table.setDelegate(Some(ProtocolObject::from_ref(&*controller)));
        }
        for (name, width) in [("title", CONTENT_WIDTH - 175.0), (APP_COLUMN, 158.0)] {
            let column = NSTableColumn::initWithIdentifier(mtm.alloc(), &NSString::from_str(name));
            column.setWidth(width);
            table.addTableColumn(&column);
        }
        scroll.setDocumentView(Some(&table));
        content.addSubview(&scroll);

        let status = NSTextField::labelWithString(&NSString::new(), mtm);
        status.setFrame(rect(INSET, 12.0, CONTENT_WIDTH, 35.0));
        status.setFont(Some(&NSFont::systemFontOfSize(12.0)));
        status.setTextColor(Some(&NSColor::secondaryLabelColor()));
        status.setMaximumNumberOfLines(2);
        status.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
        content.addSubview(&status);

        Self {
            window,
            search,
            table,
            status,
            state: RefCell::new(State::default()),
            requests,
            _controller: controller,
        }
    }

    fn toggle(&self) {
        if self.window.isVisible() {
            self.close();
            return;
        }
        let generation = {
            let mut state = self.state.borrow_mut();
            state.generation += 1;
            state.windows.clear();
            state.matches.clear();
            state.status = Status::Loading;
            state.generation
        };
        self.search.setStringValue(&NSString::new());
        self.refresh_rows();
        self.window.center();
        self.window.makeKeyAndOrderFront(None);
        self.window.makeFirstResponder(Some(&self.search));
        if self.requests.send(Request::List(generation)).is_err() {
            self.worker_unavailable();
        }
    }

    fn close(&self) {
        self.state.borrow_mut().generation += 1;
        self.window.orderOut(None);
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
        let query = self.search.currentEditor().map_or_else(
            || self.search.stringValue().to_string(),
            |editor| editor.string().to_string(),
        );
        let mut state = self.state.borrow_mut();
        state.matches = matching_rows(&state.windows, &query);
        drop(state);
        self.refresh_rows();
    }

    fn refresh_rows(&self) {
        self.table.reloadData();
        if !self.state.borrow().matches.is_empty() {
            self.select(0);
        }
        self.refresh_status();
    }

    fn refresh_status(&self) {
        let state = self.state.borrow();
        let text = match &state.status {
            Status::Loading => "Loading windows…",
            Status::Focusing => "Switching window…",
            Status::Error(message) => message,
            Status::Ready if state.matches.is_empty() => "No matching windows",
            Status::Ready => "↑ ↓  Navigate       ↵  Switch       Esc  Close",
        };
        self.status.setStringValue(&NSString::from_str(text));
    }

    fn move_selection(&self, offset: isize) {
        let count = self.state.borrow().matches.len() as isize;
        if count > 0 {
            let current = self.table.selectedRow();
            let next = if current < 0 {
                0
            } else {
                (current + offset).clamp(0, count - 1)
            };
            self.select(next as usize);
        }
    }

    fn select(&self, row: usize) {
        self.table
            .selectRowIndexes_byExtendingSelection(&NSIndexSet::indexSetWithIndex(row), false);
        self.table.scrollRowToVisible(row as isize);
    }

    fn focus_selected(&self) {
        let Ok(row) = usize::try_from(self.table.selectedRow()) else {
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
            Status::Error("The window worker stopped. Restart Window Switcher.".into());
        self.refresh_status();
    }
}

impl Drop for SearchPanel {
    fn drop(&mut self) {
        // SAFETY: The controller is still retained while the AppKit references are cleared.
        unsafe {
            self.search.setDelegate(None);
            self.table.setDataSource(None);
            self.table.setDelegate(None);
        }
        self.window.orderOut(None);
    }
}

// SAFETY: This NSObject subclass installs main-thread AppKit delegate callbacks only.
define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    struct PanelController;

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
            column: Option<&NSTableColumn>,
            row: isize,
        ) -> Option<Retained<NSView>> {
            with_panel(|panel| {
                let column = column?;
                let row = usize::try_from(row).ok()?;
                let state = panel.state.borrow();
                let window = state.windows.get(*state.matches.get(row)?)?;
                let is_app = *column.identifier() == *NSString::from_str(APP_COLUMN);
                let value = if is_app {
                    &window.app_name
                } else {
                    &window.title
                };
                let label = NSTextField::labelWithString(&NSString::from_str(value), self.mtm());
                label.setFont(Some(&NSFont::systemFontOfSize(13.0)));
                if is_app {
                    label.setTextColor(Some(&NSColor::secondaryLabelColor()));
                }
                label.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
                Some(label.into_super().into_super())
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
