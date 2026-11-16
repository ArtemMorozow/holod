use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAccessibility, NSApplication, NSBackingStoreType, NSBox, NSBoxType, NSColor, NSEventType,
    NSFloatingWindowLevel, NSFocusRingType, NSFont, NSFontWeightMedium, NSGlassEffectView,
    NSGlassEffectViewStyle, NSImage, NSImageScaling, NSImageView, NSLineBreakMode, NSMenu,
    NSMenuItem, NSPanel, NSScrollView, NSTableColumn, NSTableView, NSTableViewStyle,
    NSTextAlignment, NSTextField, NSView, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

use crate::core::window::WindowInfo;
use objc2::sel;

const WIDTH: f64 = 620.0;
const HEIGHT: f64 = 516.0;
const ROW_WIDTH: f64 = 576.0;
const ROW_HEIGHT: f64 = 36.0;
const LIST_BOTTOM: f64 = 50.0;
const LIST_HEIGHT: f64 = HEIGHT - 70.0 - LIST_BOTTOM;
const SEARCH_MARGIN: f64 = 24.0;

pub const HELP: &str =
    "↑ ↓  Navigate      ↵  Switch      ⌘W  Close window      esc  Hide      ⌘Q  Quit";

pub fn install_edit_menu(mtm: MainThreadMarker, app: &NSApplication) {
    let main = NSMenu::initWithTitle(mtm.alloc(), &NSString::new());
    let edit = NSMenu::initWithTitle(mtm.alloc(), &NSString::from_str("Edit"));
    // SAFETY: These are standard AppKit actions sent through the responder chain.
    unsafe {
        let app_menu = NSMenu::initWithTitle(mtm.alloc(), &NSString::from_str("holod"));
        let app_item = NSMenuItem::initWithTitle_action_keyEquivalent(
            mtm.alloc(),
            &NSString::from_str("holod"),
            None,
            &NSString::new(),
        );
        let quit = NSMenuItem::initWithTitle_action_keyEquivalent(
            mtm.alloc(),
            &NSString::from_str("Quit holod"),
            Some(sel!(terminate:)),
            &NSString::from_str("q"),
        );
        let close_window = NSMenuItem::initWithTitle_action_keyEquivalent(
            mtm.alloc(),
            &NSString::from_str("Close Selected Window"),
            Some(sel!(closeSelectedWindow:)),
            &NSString::from_str("w"),
        );
        app_menu.addItem(&close_window);
        app_menu.addItem(&NSMenuItem::separatorItem(mtm));
        app_menu.addItem(&quit);
        app_item.setSubmenu(Some(&app_menu));
        main.addItem(&app_item);
        let edit_item = NSMenuItem::initWithTitle_action_keyEquivalent(
            mtm.alloc(),
            &NSString::from_str("Edit"),
            None,
            &NSString::new(),
        );
        edit_item.setSubmenu(Some(&edit));
        main.addItem(&edit_item);
        for (title, action, key) in [
            ("Undo", sel!(undo:), "z"),
            ("Redo", sel!(redo:), "Z"),
            ("Cut", sel!(cut:), "x"),
            ("Copy", sel!(copy:), "c"),
            ("Paste", sel!(paste:), "v"),
            ("Select All", sel!(selectAll:), "a"),
        ] {
            let item = NSMenuItem::initWithTitle_action_keyEquivalent(
                mtm.alloc(),
                &NSString::from_str(title),
                Some(action),
                &NSString::from_str(key),
            );
            edit.addItem(&item);
        }
    }
    app.setMainMenu(Some(&main));
}

pub struct PanelView {
    pub window: Retained<SearchWindow>,
    pub search: Retained<NSTextField>,
    pub table: Retained<NSTableView>,
    pub message: Retained<NSTextField>,
    pub shortcuts: Retained<NSTextField>,
    pub count: Retained<NSTextField>,
}

impl PanelView {
    pub fn new(mtm: MainThreadMarker) -> Self {
        let bounds = rect(0.0, 0.0, WIDTH, HEIGHT);
        let window = SearchWindow::new(mtm, bounds);
        window.setTitle(&NSString::from_str("holod"));
        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        window.setHasShadow(true);
        window.setFloatingPanel(true);
        window.setLevel(NSFloatingWindowLevel);
        window.setHidesOnDeactivate(false);
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::MoveToActiveSpace
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );

        let content = NSView::initWithFrame(mtm.alloc(), bounds);
        let glass = NSGlassEffectView::initWithFrame(mtm.alloc(), bounds);
        glass.setCornerRadius(30.0);
        glass.setStyle(NSGlassEffectViewStyle::Regular);
        glass.setContentView(Some(&content));
        window.setContentView(Some(&glass));

        let search_icon = image_view(mtm, rect(24.0, HEIGHT - 47.0, 23.0, 23.0));
        let magnifying_glass = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str("magnifyingglass"),
            None,
        );
        search_icon.setImage(magnifying_glass.as_deref());
        search_icon.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
        content.addSubview(&search_icon);

        let search = NSTextField::textFieldWithString(&NSString::new(), mtm);
        search.setFrame(rect(
            58.0,
            HEIGHT - 55.0,
            WIDTH - 58.0 - SEARCH_MARGIN,
            38.0,
        ));
        search.setFont(Some(&NSFont::systemFontOfSize(24.0)));
        search.setBordered(false);
        search.setBezeled(false);
        search.setDrawsBackground(false);
        search.setFocusRingType(NSFocusRingType::None);
        let search_label = NSString::from_str("Search windows");
        search.setPlaceholderString(Some(&search_label));
        search.setAccessibilityLabel(Some(&search_label));
        content.addSubview(&search);
        window.setInitialFirstResponder(Some(&search));
        add_separator(mtm, &content, HEIGHT - 67.0);

        let scroll_frame = rect(12.0, LIST_BOTTOM, WIDTH - 24.0, LIST_HEIGHT);
        let scroll = NSScrollView::initWithFrame(mtm.alloc(), scroll_frame);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setDrawsBackground(false);
        let table = NSTableView::initWithFrame(
            mtm.alloc(),
            rect(0.0, 0.0, scroll_frame.size.width, scroll_frame.size.height),
        );
        table.setHeaderView(None);
        table.setStyle(NSTableViewStyle::Inset);
        table.setBackgroundColor(&NSColor::clearColor());
        table.setRowHeight(ROW_HEIGHT);
        table.setIntercellSpacing(NSSize::new(0.0, 2.0));
        table.setRefusesFirstResponder(true);
        table.setAllowsMultipleSelection(false);
        let column = NSTableColumn::initWithIdentifier(mtm.alloc(), &NSString::from_str("window"));
        column.setWidth(ROW_WIDTH);
        table.addTableColumn(&column);
        scroll.setDocumentView(Some(&table));
        content.addSubview(&scroll);

        let message = secondary_label(
            mtm,
            rect(
                44.0,
                LIST_BOTTOM + (LIST_HEIGHT - 76.0) / 2.0,
                WIDTH - 88.0,
                76.0,
            ),
            14.0,
        );
        message.setAlignment(NSTextAlignment::Center);
        message.setMaximumNumberOfLines(3);
        message.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
        message.setHidden(true);
        content.addSubview(&message);

        add_separator(mtm, &content, 46.0);
        let shortcuts = secondary_label(mtm, rect(24.0, 8.0, WIDTH - 48.0, 29.0), 12.0);
        shortcuts.setStringValue(&NSString::from_str(HELP));
        shortcuts.setMaximumNumberOfLines(2);
        shortcuts.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
        content.addSubview(&shortcuts);

        let count = secondary_label(mtm, rect(456.0, 8.0, 140.0, 29.0), 12.0);
        count.setAlignment(NSTextAlignment::Right);
        content.addSubview(&count);

        Self {
            window,
            search,
            table,
            message,
            shortcuts,
            count,
        }
    }
}

pub fn window_row(
    mtm: MainThreadMarker,
    window: &WindowInfo,
    icon: Option<&NSImage>,
) -> Retained<NSView> {
    let row = NSView::initWithFrame(mtm.alloc(), rect(0.0, 0.0, ROW_WIDTH, ROW_HEIGHT));
    let image_view = image_view(mtm, rect(12.0, 6.0, 24.0, 24.0));
    image_view.setImage(icon);
    row.addSubview(&image_view);

    let title = NSTextField::labelWithString(&NSString::from_str(&window.title), mtm);
    title.setFrame(rect(46.0, 17.0, ROW_WIDTH - 64.0, 16.0));
    // SAFETY: NSFontWeightMedium is an immutable AppKit constant.
    let medium = unsafe { NSFontWeightMedium };
    title.setFont(Some(&NSFont::systemFontOfSize_weight(13.0, medium)));
    title.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    row.addSubview(&title);

    let app = secondary_label(mtm, rect(46.0, 3.0, ROW_WIDTH - 64.0, 13.0), 11.0);
    app.setStringValue(&NSString::from_str(&window.app_name));
    app.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    row.addSubview(&app);
    row
}

fn add_separator(mtm: MainThreadMarker, content: &NSView, y: f64) {
    let separator = NSBox::initWithFrame(mtm.alloc(), rect(20.0, y, WIDTH - 40.0, 1.0));
    separator.setBoxType(NSBoxType::Separator);
    content.addSubview(&separator);
}

fn secondary_label(mtm: MainThreadMarker, frame: NSRect, size: f64) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(&NSString::new(), mtm);
    label.setFrame(frame);
    label.setFont(Some(&NSFont::systemFontOfSize(size)));
    label.setTextColor(Some(&NSColor::secondaryLabelColor()));
    label
}

fn image_view(mtm: MainThreadMarker, frame: NSRect) -> Retained<NSImageView> {
    let view = NSImageView::initWithFrame(mtm.alloc(), frame);
    view.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
    view
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
}

// SAFETY: This main-thread subclass enables focus and handles an AppKit responder action.
define_class!(
    #[unsafe(super(NSPanel))]
    #[thread_kind = MainThreadOnly]
    pub struct SearchWindow;

    impl SearchWindow {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key(&self) -> bool {
            true
        }

        #[unsafe(method(closeSelectedWindow:))]
        fn close_selected_window(&self, _sender: &NSMenuItem) {
            let repeated = NSApplication::sharedApplication(self.mtm())
                .currentEvent()
                .is_some_and(|event| event.r#type() == NSEventType::KeyDown && event.isARepeat());
            if self.isKeyWindow() && !repeated {
                super::search_panel::close_selection();
            }
        }
    }
);

impl SearchWindow {
    fn new(mtm: MainThreadMarker, bounds: NSRect) -> Retained<Self> {
        // SAFETY: The subclass's ivars are initialized and the arguments match NSPanel's initializer.
        unsafe {
            msg_send![
                super(mtm.alloc::<Self>().set_ivars(())),
                initWithContentRect: bounds,
                styleMask: NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
                backing: NSBackingStoreType::Buffered,
                defer: false,
            ]
        }
    }
}
