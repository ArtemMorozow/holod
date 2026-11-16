use std::error::Error;
use std::ptr::NonNull;
use std::rc::Rc;

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSEvent, NSEventMask};

mod combination;
mod double_tap;

pub use combination::{Combination, HOTKEY_EXISTS, Registration};
pub use double_tap::{DoubleTap, TapKey};

#[derive(Clone, Copy)]
pub enum Shortcut {
    DoubleTap(TapKey),
    Combination(Combination),
}

impl Default for Shortcut {
    fn default() -> Self {
        Self::DoubleTap(TapKey::OPTION)
    }
}

impl Shortcut {
    pub fn from_preference(value: isize) -> Self {
        // Existing preferences use zero for double Option and positive values for chords.
        if value < 0 {
            u16::try_from(-1 - value)
                .ok()
                .map(|key| Self::DoubleTap(TapKey::from_key_code(key)))
        } else {
            Combination::from_preference(value).map(Self::Combination)
        }
        .unwrap_or_default()
    }

    pub fn preference(self) -> isize {
        match self {
            Self::DoubleTap(TapKey::OPTION) => 0,
            Self::DoubleTap(key) => -1 - key.code() as isize,
            Self::Combination(combination) => combination.preference(),
        }
    }
}

pub struct TapMonitors {
    _main_thread: MainThreadMarker,
    monitors: Vec<Retained<AnyObject>>,
}

pub fn monitor_taps(
    mtm: MainThreadMarker,
    observe: impl Fn(&NSEvent) + 'static,
) -> Result<TapMonitors, Box<dyn Error>> {
    let observe = Rc::new(observe);
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
        | NSEventMask::KeyUp
        | NSEventMask::LeftMouseDown
        | NSEventMask::RightMouseDown
        | NSEventMask::OtherMouseDown
        | NSEventMask::ScrollWheel;
    let mut result = TapMonitors {
        _main_thread: mtm,
        monitors: Vec::with_capacity(2),
    };
    let global = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(mask, &global_block)
        .ok_or("Could not monitor double-tap shortcuts outside Holod.")?;
    result.monitors.push(global);
    // SAFETY: The local block always returns the original event pointer while it is valid.
    let local =
        unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &local_block) }
            .ok_or("Could not monitor double-tap shortcuts inside Holod.")?;
    result.monitors.push(local);
    Ok(result)
}

impl Drop for TapMonitors {
    fn drop(&mut self) {
        for monitor in &self.monitors {
            // SAFETY: The tokens came from AppKit's monitor registration functions.
            unsafe { NSEvent::removeMonitor(monitor) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_existing_shortcut_preferences_and_round_trips_double_taps() {
        let old_command_k = (0x28 << 16) | 0x100;
        for value in [0, old_command_k, -0x38, -1] {
            assert_eq!(Shortcut::from_preference(value).preference(), value);
        }
        assert_eq!(Shortcut::from_preference(isize::MIN).preference(), 0);
    }
}
