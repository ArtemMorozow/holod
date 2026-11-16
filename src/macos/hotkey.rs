use std::cell::RefCell;
use std::error::Error;
use std::ptr::NonNull;
use std::rc::Rc;

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSEvent, NSEventMask, NSEventModifierFlags, NSEventType};

const LEFT_OPTION: u16 = 0x3a; // kVK_Option
const RIGHT_OPTION: u16 = 0x3d; // kVK_RightOption
const MAX_TAP: f64 = 0.24;
const MAX_GAP: f64 = 0.33;

pub struct OptionMonitors {
    _main_thread: MainThreadMarker,
    monitors: Vec<Retained<AnyObject>>,
}

pub fn register(
    mtm: MainThreadMarker,
    open_panel: impl Fn() + 'static,
) -> Result<OptionMonitors, Box<dyn Error>> {
    let detector = RefCell::new(DoubleOption::default());
    let observe = Rc::new(move |event: &NSEvent| {
        if detector.borrow_mut().observe(event) {
            open_panel();
        }
    });
    let global_observe = observe.clone();
    let global_block = RcBlock::new(move |event: NonNull<NSEvent>| {
        // SAFETY: AppKit passes a live NSEvent for the duration of this callback.
        global_observe(unsafe { event.as_ref() });
    });
    let local_block = RcBlock::new(move |event: NonNull<NSEvent>| {
        // SAFETY: AppKit passes a live NSEvent for the duration of this callback.
        observe(unsafe { event.as_ref() });
        event.as_ptr()
    });

    let mask = NSEventMask::FlagsChanged
        | NSEventMask::KeyDown
        | NSEventMask::LeftMouseDown
        | NSEventMask::RightMouseDown
        | NSEventMask::OtherMouseDown
        | NSEventMask::ScrollWheel;
    let mut result = OptionMonitors {
        _main_thread: mtm,
        monitors: Vec::with_capacity(2),
    };
    let global = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(mask, &global_block)
        .ok_or("Could not monitor Option presses outside holod.")?;
    result.monitors.push(global);
    // SAFETY: The local block always returns the original event pointer while it is valid.
    let local =
        unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &local_block) }
            .ok_or("Could not monitor Option presses inside holod.")?;
    result.monitors.push(local);
    Ok(result)
}

impl Drop for OptionMonitors {
    fn drop(&mut self) {
        for monitor in &self.monitors {
            // SAFETY: The tokens came from AppKit's monitor registration functions.
            unsafe { NSEvent::removeMonitor(monitor) };
        }
    }
}

#[derive(Default)]
struct DoubleOption {
    held: bool,
    press: Option<(u16, f64)>,
    previous_tap: Option<f64>,
}

impl DoubleOption {
    fn observe(&mut self, event: &NSEvent) -> bool {
        if event.r#type() != NSEventType::FlagsChanged {
            self.cancel_taps();
            return false;
        }

        self.flags_changed(event.keyCode(), event.modifierFlags(), event.timestamp())
    }

    fn cancel_taps(&mut self) {
        self.press = None;
        self.previous_tap = None;
    }

    fn flags_changed(&mut self, key: u16, flags: NSEventModifierFlags, time: f64) -> bool {
        let was_held = self.held;
        self.held = flags.contains(NSEventModifierFlags::Option);
        let other_modifiers = NSEventModifierFlags::Shift
            | NSEventModifierFlags::Control
            | NSEventModifierFlags::Command
            | NSEventModifierFlags::Function;
        if !matches!(key, LEFT_OPTION | RIGHT_OPTION)
            || flags.intersects(other_modifiers)
            || self.held == was_held
        {
            self.cancel_taps();
            return false;
        }

        if self.held {
            self.press = Some((key, time));
            return false;
        }
        let Some((pressed_key, pressed_at)) = self.press.take() else {
            return false;
        };
        if pressed_key != key || !(0.0..=MAX_TAP).contains(&(time - pressed_at)) {
            self.cancel_taps();
            return false;
        }
        match self.previous_tap.take() {
            Some(tapped_at) if (0.0..=MAX_GAP).contains(&(pressed_at - tapped_at)) => true,
            _ => {
                self.previous_tap = Some(time);
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tap(detector: &mut DoubleOption, key: u16, start: f64) -> bool {
        assert!(!detector.flags_changed(key, NSEventModifierFlags::Option, start));
        detector.flags_changed(key, NSEventModifierFlags::empty(), start + 0.06)
    }

    #[test]
    fn accepts_two_quick_taps_once_and_expires_old_taps() {
        let mut detector = DoubleOption::default();
        assert!(!tap(&mut detector, LEFT_OPTION, 1.0));
        assert!(tap(&mut detector, RIGHT_OPTION, 1.2));
        assert!(!tap(&mut detector, RIGHT_OPTION, 1.38));
        assert!(!tap(&mut detector, RIGHT_OPTION, 2.0));
        assert!(tap(&mut detector, RIGHT_OPTION, 2.24));
    }

    #[test]
    fn ignores_option_chords_holds_and_overlapping_option_keys() {
        let mut detector = DoubleOption::default();
        assert!(!tap(&mut detector, LEFT_OPTION, 1.0));
        assert!(!detector.flags_changed(LEFT_OPTION, NSEventModifierFlags::Option, 1.2));
        detector.cancel_taps(); // A key or pointer event arrived while Option was held.
        assert!(!detector.flags_changed(LEFT_OPTION, NSEventModifierFlags::empty(), 1.23));
        assert!(!tap(&mut detector, LEFT_OPTION, 1.34));

        assert!(!detector.flags_changed(LEFT_OPTION, NSEventModifierFlags::Option, 1.5));
        assert!(!detector.flags_changed(RIGHT_OPTION, NSEventModifierFlags::Option, 1.52));
        assert!(!detector.flags_changed(LEFT_OPTION, NSEventModifierFlags::Option, 1.58));
        assert!(!detector.flags_changed(RIGHT_OPTION, NSEventModifierFlags::empty(), 1.6));
        assert!(!tap(&mut detector, LEFT_OPTION, 1.7));

        assert!(!detector.flags_changed(LEFT_OPTION, NSEventModifierFlags::Option, 1.9));
        assert!(!detector.flags_changed(LEFT_OPTION, NSEventModifierFlags::empty(), 2.2));
        assert!(!tap(&mut detector, RIGHT_OPTION, 2.26));

        let command_option = NSEventModifierFlags::Command | NSEventModifierFlags::Option;
        assert!(!detector.flags_changed(RIGHT_OPTION, command_option, 2.45));
        assert!(!detector.flags_changed(RIGHT_OPTION, NSEventModifierFlags::Command, 2.51));
        assert!(!tap(&mut detector, RIGHT_OPTION, 2.6));
    }
}
