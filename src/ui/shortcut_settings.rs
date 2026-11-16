use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAlertSecondButtonReturn, NSAlertStyle,
    NSAlertThirdButtonReturn, NSApplication, NSButton, NSEvent, NSEventModifierFlags, NSFont,
    NSTextField, NSView,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString, NSUserDefaults};

use crate::macos::hotkey::{Combination, HOTKEY_EXISTS, Registration};

const PREFERENCE: &str = "OpenSearchShortcut";
const DOUBLE_OPTION: isize = 0;

pub struct ShortcutSettings {
    defaults: Retained<NSUserDefaults>,
    active: RefCell<Option<(Combination, Registration)>>,
    editing: Cell<bool>,
    open: fn(),
}

impl ShortcutSettings {
    pub fn new(mtm: MainThreadMarker, open: fn()) -> Self {
        let defaults = NSUserDefaults::standardUserDefaults();
        let saved =
            Combination::from_preference(defaults.integerForKey(&NSString::from_str(PREFERENCE)));
        Self {
            defaults,
            active: RefCell::new(restore(mtm, saved, open)),
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

    pub fn uses_double_option(&self) -> bool {
        !self.is_editing() && self.active.borrow().is_none()
    }

    pub fn edit(&self, mtm: MainThreadMarker) {
        if self.editing.replace(true) {
            return;
        }
        let first_launch = self.needs_setup();
        // Release the current shortcut so the recorder can receive those keys too.
        let previous = self.active.take().map(|(combination, _)| combination);
        let mut message = None;
        loop {
            match prompt(mtm, first_launch, message) {
                Choice::Save(combination) => {
                    match Registration::install(mtm, combination, self.open) {
                        Ok(registration) => {
                            self.defaults.setInteger_forKey(
                                combination.preference(),
                                &NSString::from_str(PREFERENCE),
                            );
                            self.active.replace(Some((combination, registration)));
                            break;
                        }
                        Err(status) => {
                            tracing::debug!(status, "Could not register the chosen shortcut");
                            message = Some(if status == HOTKEY_EXISTS {
                                "That shortcut is already in use. Press a different combination."
                            } else {
                                "macOS could not register that shortcut. Press a different combination."
                            });
                        }
                    }
                }
                Choice::DoubleOption => {
                    self.defaults
                        .setInteger_forKey(DOUBLE_OPTION, &NSString::from_str(PREFERENCE));
                    break;
                }
                Choice::Cancel => {
                    self.active.replace(restore(mtm, previous, self.open));
                    break;
                }
            }
        }
        self.editing.set(false);
    }
}

fn restore(
    mtm: MainThreadMarker,
    saved: Option<Combination>,
    open: fn(),
) -> Option<(Combination, Registration)> {
    let combination = saved?;
    match Registration::install(mtm, combination, open) {
        Ok(registration) => Some((combination, registration)),
        Err(status) => {
            tracing::warn!(
                status,
                "The saved shortcut is unavailable. Using double Option."
            );
            None
        }
    }
}

enum Choice {
    Save(Combination),
    DoubleOption,
    Cancel,
}

fn prompt(mtm: MainThreadMarker, first_launch: bool, message: Option<&str>) -> Choice {
    let alert = NSAlert::new(mtm);
    alert.setAlertStyle(NSAlertStyle::Informational);
    alert.setMessageText(&NSString::from_str("Choose a shortcut for Window Search"));
    alert.setInformativeText(&NSString::from_str(message.unwrap_or(
        "Press the keys you want to use to open Window Search. Include Command, Control, or Option.",
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
        return recorder
            .ivars()
            .combination
            .get()
            .map_or(Choice::Cancel, Choice::Save);
    }
    if (first_launch && response == NSAlertSecondButtonReturn)
        || (!first_launch && response == NSAlertThirdButtonReturn)
    {
        return Choice::DoubleOption;
    }
    Choice::Cancel
}

struct RecorderState {
    combination: Cell<Option<Combination>>,
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
            self.record(event)
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            if !self.record(event) {
                // SAFETY: NSView inherits keyDown: and this is the original AppKit event.
                unsafe { msg_send![super(self), keyDown: event] }
            }
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
            combination: Cell::new(None),
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

    fn record(&self, event: &NSEvent) -> bool {
        let flags = event.modifierFlags();
        let modifiers = NSEventModifierFlags::Command
            | NSEventModifierFlags::Control
            | NSEventModifierFlags::Option;
        // Leave bare Escape, Return, and Tab available for the dialog buttons and focus.
        if !flags.intersects(modifiers) && matches!(event.keyCode(), 0x24 | 0x30 | 0x35 | 0x4c) {
            return false;
        }
        if event.isARepeat() {
            return true;
        }

        let (combination, label) = match Combination::record(event) {
            Some((combination, label)) => (Some(combination), label),
            None => (None, "Include Command, Control, or Option.".into()),
        };
        self.ivars()
            .label
            .setStringValue(&NSString::from_str(&label));
        self.ivars().combination.set(combination);
        self.ivars().save.setEnabled(combination.is_some());
        true
    }
}
