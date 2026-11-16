use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAlertSecondButtonReturn, NSAlertStyle,
    NSAlertThirdButtonReturn, NSApplication, NSButton, NSEvent, NSEventModifierFlags, NSFont,
    NSTextField, NSView,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString, NSUserDefaults};

use crate::macos::hotkey::{Combination, DoubleTap, HOTKEY_EXISTS, Registration, Shortcut};

const PREFERENCE: &str = "OpenSearchShortcut";

#[derive(Default)]
struct ActiveShortcut {
    shortcut: Shortcut,
    _registration: Option<Registration>,
}

impl ActiveShortcut {
    fn install(mtm: MainThreadMarker, shortcut: Shortcut, open: fn()) -> Result<Self, i32> {
        let registration = match shortcut {
            Shortcut::DoubleTap(_) => None,
            Shortcut::Combination(combination) => {
                Some(Registration::install(mtm, combination, open)?)
            }
        };
        Ok(Self {
            shortcut,
            _registration: registration,
        })
    }
}

pub struct ShortcutSettings {
    defaults: Retained<NSUserDefaults>,
    active: RefCell<ActiveShortcut>,
    taps: RefCell<DoubleTap>,
    editing: Cell<bool>,
    open: fn(),
}

impl ShortcutSettings {
    pub fn new(mtm: MainThreadMarker, open: fn()) -> Self {
        let defaults = NSUserDefaults::standardUserDefaults();
        let saved =
            Shortcut::from_preference(defaults.integerForKey(&NSString::from_str(PREFERENCE)));
        Self {
            defaults,
            active: RefCell::new(restore(mtm, saved, open)),
            taps: RefCell::default(),
            editing: Cell::new(false),
            open,
        }
    }

    pub fn needs_setup(&self) -> bool {
        self.defaults
            .objectForKey(&NSString::from_str(PREFERENCE))
            .is_none()
    }

    pub fn is_editing(&self) -> bool {
        self.editing.get()
    }

    pub fn observe_tap(&self, event: &NSEvent) -> bool {
        let Shortcut::DoubleTap(key) = self.active.borrow().shortcut else {
            return false;
        };
        !self.is_editing() && self.taps.borrow_mut().observe(event) == Some(key)
    }

    pub fn edit(&self, mtm: MainThreadMarker) {
        if self.editing.replace(true) {
            return;
        }
        let first_launch = self.needs_setup();
        // Release the current shortcut so the recorder can receive those keys too.
        let previous = self.active.take().shortcut;
        self.taps.take();
        let mut message = None;
        loop {
            let Some(shortcut) = prompt(mtm, first_launch, message) else {
                self.active.replace(restore(mtm, previous, self.open));
                break;
            };
            match ActiveShortcut::install(mtm, shortcut, self.open) {
                Ok(active) => {
                    self.defaults
                        .setInteger_forKey(shortcut.preference(), &NSString::from_str(PREFERENCE));
                    self.active.replace(active);
                    break;
                }
                Err(status) => {
                    tracing::debug!(status, "Could not register the chosen shortcut");
                    message = Some(if status == HOTKEY_EXISTS {
                        "That shortcut is already in use. Press another shortcut."
                    } else {
                        "macOS could not register that shortcut. Press another shortcut."
                    });
                }
            }
        }
        self.editing.set(false);
    }
}

fn restore(mtm: MainThreadMarker, saved: Shortcut, open: fn()) -> ActiveShortcut {
    match ActiveShortcut::install(mtm, saved, open) {
        Ok(active) => active,
        Err(status) => {
            tracing::warn!(
                status,
                "The saved shortcut is unavailable. Using double Option."
            );
            ActiveShortcut::default()
        }
    }
}

fn prompt(mtm: MainThreadMarker, first_launch: bool, message: Option<&str>) -> Option<Shortcut> {
    let alert = NSAlert::new(mtm);
    alert.setAlertStyle(NSAlertStyle::Informational);
    alert.setMessageText(&NSString::from_str("Choose a shortcut for Window Search"));
    alert.setInformativeText(&NSString::from_str(message.unwrap_or(
        "Press a shortcut such as ⌃⌥K, or quickly tap the same key twice, such as ⌘ → ⌘.",
    )));
    let save = alert.addButtonWithTitle(&NSString::from_str("Save Shortcut"));
    save.setEnabled(false);
    let cancel = alert.addButtonWithTitle(&NSString::from_str(if first_launch {
        "Skip"
    } else {
        "Cancel"
    }));
    cancel.setKeyEquivalent(&NSString::from_str("\u{1b}"));
    if !first_launch {
        alert.addButtonWithTitle(&NSString::from_str("Use Double Option"));
    }

    let recorder = ShortcutRecorder::new(mtm, save);
    alert.setAccessoryView(Some(&recorder));
    alert.layout();
    let window = alert.window();
    window.setInitialFirstResponder(Some(&recorder));
    window.makeFirstResponder(Some(&recorder));
    NSApplication::sharedApplication(mtm).activate();
    let response = alert.runModal();
    if response == NSAlertFirstButtonReturn {
        return recorder.ivars().shortcut.get();
    }
    if (first_launch && response == NSAlertSecondButtonReturn)
        || (!first_launch && response == NSAlertThirdButtonReturn)
    {
        return Some(Shortcut::default());
    }
    None
}

struct RecorderState {
    shortcut: Cell<Option<Shortcut>>,
    taps: RefCell<DoubleTap>,
    label: Retained<NSTextField>,
    save: Retained<NSButton>,
}

// SAFETY: This main-thread NSView receives AppKit keyboard events while its alert is open.
define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[ivars = RecorderState]
    struct ShortcutRecorder;

    impl ShortcutRecorder {
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(performKeyEquivalent:))]
        fn perform_key_equivalent(&self, event: &NSEvent) -> bool {
            self.record_combination(event)
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            if self.record_combination(event) {
                return;
            }
            // Keep Escape, Return, and Tab available for the dialog buttons and focus.
            if matches!(event.keyCode(), 0x24 | 0x30 | 0x35 | 0x4c) {
                self.ivars().taps.take();
                // SAFETY: NSView inherits keyDown: and this is the original AppKit event.
                unsafe { msg_send![super(self), keyDown: event] }
            } else {
                self.record_tap(event);
            }
        }

        #[unsafe(method(keyUp:))]
        fn key_up(&self, event: &NSEvent) {
            self.record_tap(event);
        }

        #[unsafe(method(flagsChanged:))]
        fn flags_changed(&self, event: &NSEvent) {
            self.record_tap(event);
        }
    }
);

impl ShortcutRecorder {
    fn new(mtm: MainThreadMarker, save: Retained<NSButton>) -> Retained<Self> {
        let bounds = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(260.0, 28.0));
        let label = NSTextField::labelWithString(&NSString::from_str("Press a shortcut…"), mtm);
        label.setFont(Some(&NSFont::systemFontOfSize(15.0)));
        label.setFrame(bounds);
        let state = RecorderState {
            shortcut: Cell::new(None),
            taps: RefCell::default(),
            label,
            save,
        };
        // SAFETY: The recorder ivars are initialized before NSView's designated initializer.
        let recorder: Retained<Self> = unsafe {
            msg_send![super(mtm.alloc::<Self>().set_ivars(state)), initWithFrame: bounds]
        };
        recorder.addSubview(&recorder.ivars().label);
        recorder
    }

    fn record_combination(&self, event: &NSEvent) -> bool {
        let flags = event.modifierFlags();
        let modifiers = NSEventModifierFlags::Command
            | NSEventModifierFlags::Control
            | NSEventModifierFlags::Option;
        if !flags.intersects(modifiers) {
            return false;
        }
        self.ivars().taps.borrow_mut().observe(event);
        if !event.isARepeat()
            && let Some((combination, label)) = Combination::record(event)
        {
            self.select(Shortcut::Combination(combination), &label);
        }
        true
    }

    fn record_tap(&self, event: &NSEvent) {
        let key = self.ivars().taps.borrow_mut().observe(event);
        if let Some(key) = key {
            self.select(Shortcut::DoubleTap(key), &key.label(event));
        }
    }

    fn select(&self, shortcut: Shortcut, label: &str) {
        self.ivars()
            .label
            .setStringValue(&NSString::from_str(label));
        self.ivars().shortcut.set(Some(shortcut));
        self.ivars().save.setEnabled(true);
    }
}
