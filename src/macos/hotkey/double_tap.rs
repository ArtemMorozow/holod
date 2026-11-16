use objc2_app_kit::{NSEvent, NSEventModifierFlags as Flags, NSEventType};

use super::combination::key_name;

const MAX_TAP: f64 = 0.24;
const MAX_GAP: f64 = 0.33;

fn modifier(key: u16) -> Option<(TapKey, Flags, &'static str)> {
    Some(match key {
        0x36 | 0x37 => (TapKey(0x37), Flags::Command, "⌘"),
        0x38 | 0x3c => (TapKey(0x38), Flags::Shift, "⇧"),
        0x3a | 0x3d => (TapKey::OPTION, Flags::Option, "⌥"),
        0x3b | 0x3e => (TapKey(0x3b), Flags::Control, "⌃"),
        _ => return None,
    })
}

fn modifiers() -> Flags {
    Flags::Command | Flags::Shift | Flags::Option | Flags::Control
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TapKey(u16);

impl TapKey {
    pub const OPTION: Self = Self(0x3a);

    pub fn from_key_code(code: u16) -> Self {
        modifier(code).map_or(Self(code), |(key, _, _)| key)
    }

    pub fn code(self) -> u16 {
        self.0
    }

    pub fn label(self, event: &NSEvent) -> String {
        let name = modifier(self.0).map_or_else(|| key_name(event), |(_, _, name)| name.to_owned());
        format!("{name} → {name}")
    }
}

pub struct DoubleTap {
    held_modifiers: Flags,
    press: Option<(u16, f64)>,
    previous_tap: Option<(TapKey, f64)>,
}

impl Default for DoubleTap {
    fn default() -> Self {
        Self {
            held_modifiers: Flags::empty(),
            press: None,
            previous_tap: None,
        }
    }
}

impl DoubleTap {
    pub fn observe(&mut self, event: &NSEvent) -> Option<TapKey> {
        let kind = event.r#type();
        match kind {
            NSEventType::FlagsChanged => {
                self.flags_changed(event.keyCode(), event.modifierFlags(), event.timestamp())
            }
            NSEventType::KeyDown | NSEventType::KeyUp => self.key_event(
                event.keyCode(),
                event.modifierFlags(),
                event.timestamp(),
                kind == NSEventType::KeyDown,
                kind == NSEventType::KeyDown && event.isARepeat(),
            ),
            _ => self.cancel_taps(),
        }
    }

    fn cancel_taps(&mut self) -> Option<TapKey> {
        self.press = None;
        self.previous_tap = None;
        None
    }

    fn flags_changed(&mut self, code: u16, flags: Flags, time: f64) -> Option<TapKey> {
        let previous_flags = std::mem::replace(&mut self.held_modifiers, flags);
        let Some((key, flag, _)) = modifier(code) else {
            return self.cancel_taps();
        };
        let held = flags.contains(flag);
        if held == previous_flags.contains(flag)
            || flags.intersects((modifiers() | Flags::Function) - flag)
        {
            return self.cancel_taps();
        }
        self.transition(key, code, time, held)
    }

    fn key_event(
        &mut self,
        code: u16,
        flags: Flags,
        time: f64,
        down: bool,
        repeat: bool,
    ) -> Option<TapKey> {
        self.held_modifiers = flags;
        if repeat || flags.intersects(modifiers()) {
            return self.cancel_taps();
        }
        self.transition(TapKey(code), code, time, down)
    }

    fn transition(&mut self, key: TapKey, code: u16, time: f64, down: bool) -> Option<TapKey> {
        if down {
            if self.press.is_some() {
                return self.cancel_taps();
            }
            self.press = Some((code, time));
            return None;
        }
        let Some((pressed_code, pressed_at)) = self.press.take() else {
            return self.cancel_taps();
        };
        if pressed_code != code || !(0.0..=MAX_TAP).contains(&(time - pressed_at)) {
            return self.cancel_taps();
        }
        match self.previous_tap.take() {
            Some((tapped_key, tapped_at))
                if tapped_key == key && (0.0..=MAX_GAP).contains(&(pressed_at - tapped_at)) =>
            {
                Some(key)
            }
            _ => {
                self.previous_tap = Some((key, time));
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modifier_tap(
        detector: &mut DoubleTap,
        code: u16,
        flag: Flags,
        start: f64,
    ) -> Option<TapKey> {
        assert_eq!(detector.flags_changed(code, flag, start), None);
        detector.flags_changed(code, Flags::empty(), start + 0.06)
    }

    fn key_tap(detector: &mut DoubleTap, code: u16, start: f64) -> Option<TapKey> {
        assert_eq!(
            detector.key_event(code, Flags::empty(), start, true, false),
            None
        );
        detector.key_event(code, Flags::empty(), start + 0.06, false, false)
    }

    #[test]
    fn detects_quick_modifier_taps_on_either_side_and_expires_old_taps() {
        for (left, right, flag) in [(0x3a, 0x3d, Flags::Option), (0x37, 0x36, Flags::Command)] {
            let mut detector = DoubleTap::default();
            assert_eq!(modifier_tap(&mut detector, left, flag, 1.0), None);
            assert_eq!(
                modifier_tap(&mut detector, right, flag, 1.2),
                Some(TapKey(left))
            );
            assert_eq!(modifier_tap(&mut detector, right, flag, 1.38), None);
            assert_eq!(modifier_tap(&mut detector, left, flag, 2.0), None);
            assert_eq!(
                modifier_tap(&mut detector, left, flag, 2.24),
                Some(TapKey(left))
            );
        }
    }

    #[test]
    fn ignores_modifier_chords_holds_and_overlapping_modifier_keys() {
        let mut detector = DoubleTap::default();
        assert_eq!(modifier_tap(&mut detector, 0x37, Flags::Command, 1.0), None);
        assert_eq!(detector.flags_changed(0x37, Flags::Command, 1.2), None);
        assert_eq!(
            detector.key_event(0x08, Flags::Command, 1.21, true, false),
            None
        );
        assert_eq!(detector.flags_changed(0x37, Flags::empty(), 1.23), None);
        assert_eq!(
            modifier_tap(&mut detector, 0x37, Flags::Command, 1.34),
            None
        );

        assert_eq!(detector.flags_changed(0x37, Flags::Command, 1.5), None);
        assert_eq!(detector.flags_changed(0x36, Flags::Command, 1.52), None);
        assert_eq!(detector.flags_changed(0x37, Flags::Command, 1.58), None);
        assert_eq!(detector.flags_changed(0x36, Flags::empty(), 1.6), None);
        assert_eq!(modifier_tap(&mut detector, 0x37, Flags::Command, 1.7), None);

        assert_eq!(detector.flags_changed(0x37, Flags::Command, 1.9), None);
        assert_eq!(detector.flags_changed(0x37, Flags::empty(), 2.2), None);
        assert_eq!(
            modifier_tap(&mut detector, 0x36, Flags::Command, 2.26),
            None
        );

        assert_eq!(
            detector.flags_changed(0x3a, Flags::Command | Flags::Option, 2.45),
            None
        );
        assert_eq!(detector.flags_changed(0x3a, Flags::Command, 2.51), None);
        assert_eq!(modifier_tap(&mut detector, 0x36, Flags::Command, 2.6), None);
    }

    #[test]
    fn detects_the_same_regular_key_twice_and_ignores_auto_repeat() {
        let mut detector = DoubleTap::default();
        assert_eq!(key_tap(&mut detector, 0, 1.0), None);
        assert_eq!(key_tap(&mut detector, 1, 1.12), None);
        assert_eq!(key_tap(&mut detector, 1, 1.24), Some(TapKey(1)));
        assert_eq!(key_tap(&mut detector, 1, 2.0), None);
        assert_eq!(
            detector.key_event(1, Flags::empty(), 2.12, true, false),
            None
        );
        assert_eq!(
            detector.key_event(1, Flags::empty(), 2.17, true, true),
            None
        );
        assert_eq!(
            detector.key_event(1, Flags::empty(), 2.21, false, false),
            None
        );
        assert_eq!(key_tap(&mut detector, 1, 2.27), None);
    }
}
