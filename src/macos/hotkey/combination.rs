use std::ffi::{c_ulong, c_void};
use std::ptr::{self, NonNull};

use objc2::MainThreadMarker;
use objc2_app_kit::{NSEvent, NSEventModifierFlags};

const COMMAND: u32 = 1 << 8;
const SHIFT: u32 = 1 << 9;
const OPTION: u32 = 1 << 11;
const CONTROL: u32 = 1 << 12;
const PRIMARY_MODIFIERS: u32 = COMMAND | OPTION | CONTROL;
const MODIFIERS: [(NSEventModifierFlags, u32, char); 4] = [
    (NSEventModifierFlags::Control, CONTROL, '⌃'),
    (NSEventModifierFlags::Option, OPTION, '⌥'),
    (NSEventModifierFlags::Shift, SHIFT, '⇧'),
    (NSEventModifierFlags::Command, COMMAND, '⌘'),
];

#[derive(Clone, Copy)]
pub struct Combination {
    key_code: u16,
    modifiers: u32,
}

impl Combination {
    pub fn record(event: &NSEvent) -> Option<(Self, String)> {
        let flags = event.modifierFlags();
        let mut modifiers = 0;
        let mut label = String::new();
        for (flag, mask, symbol) in MODIFIERS {
            if flags.contains(flag) {
                modifiers |= mask;
                label.push(symbol);
            }
        }
        if modifiers & PRIMARY_MODIFIERS == 0 {
            return None;
        }

        label.push_str(&key_name(event));
        Some((
            Self {
                key_code: event.keyCode(),
                modifiers,
            },
            label,
        ))
    }

    pub fn from_preference(value: isize) -> Option<Self> {
        let value = u32::try_from(value).ok()?;
        let modifiers = value & u32::from(u16::MAX);
        (modifiers & PRIMARY_MODIFIERS != 0 && modifiers & !(PRIMARY_MODIFIERS | SHIFT) == 0)
            .then_some(Self {
                key_code: (value >> 16) as u16,
                modifiers,
            })
    }

    pub fn preference(self) -> isize {
        ((u32::from(self.key_code) << 16) | self.modifiers) as isize
    }
}

pub(super) fn key_name(event: &NSEvent) -> String {
    let characters = event
        .charactersIgnoringModifiers()
        .map_or_else(String::new, |text| text.to_string());
    match characters.as_str() {
        " " => "Space".into(),
        "\r" | "\u{3}" => "↵".into(),
        "\t" | "\u{19}" => "⇥".into(),
        "\u{1b}" => "Esc".into(),
        "\u{7f}" | "\u{8}" => "⌫".into(),
        "\u{f700}" => "↑".into(),
        "\u{f701}" => "↓".into(),
        "\u{f702}" => "←".into(),
        "\u{f703}" => "→".into(),
        "\u{f728}" => "⌦".into(),
        _ => match characters.chars().next() {
            Some(function @ '\u{f704}'..='\u{f726}') => format!("F{}", function as u32 - 0xf703),
            Some(key) if !key.is_control() && !('\u{f700}'..='\u{f8ff}').contains(&key) => {
                characters.to_uppercase()
            }
            _ => format!("Key {}", event.keyCode()),
        },
    }
}

pub const HOTKEY_EXISTS: i32 = -9878;
const EVENT_NOT_HANDLED: i32 = -9874;
const EVENT_INTERNAL_ERROR: i32 = -9868;
const EXCLUSIVE: u32 = 1;
const KEYBOARD_RELEASED: EventType = EventType {
    class: u32::from_be_bytes(*b"keyb"),
    kind: 6,
};
const SEARCH_HOTKEY_ID: EventHotKeyId = EventHotKeyId {
    signature: u32::from_be_bytes(*b"holo"),
    id: 1,
};

pub struct Registration {
    _main_thread: MainThreadMarker,
    key: NonNull<c_void>,
    handler: NonNull<c_void>,
    callback: Option<Box<fn()>>,
}

impl Registration {
    pub fn install(mtm: MainThreadMarker, shortcut: Combination, open: fn()) -> Result<Self, i32> {
        let mut key = ptr::null_mut();
        // SAFETY: AppKit has initialized the event dispatcher on the main thread.
        let target = unsafe { GetEventDispatcherTarget() }.ok_or(EVENT_INTERNAL_ERROR)?;
        // SAFETY: The target is live and the system writes one hotkey token to `key`.
        let result = unsafe {
            RegisterEventHotKey(
                u32::from(shortcut.key_code),
                shortcut.modifiers,
                SEARCH_HOTKEY_ID,
                target,
                EXCLUSIVE,
                &mut key,
            )
        };
        if result != 0 {
            return Err(result);
        }
        let key = NonNull::new(key).expect("Successful hotkey registration returns a token");

        let mut callback = Box::new(open);
        let mut handler = ptr::null_mut();
        // SAFETY: The event type matches the handler. Its data stays owned until removal on main.
        let result = unsafe {
            InstallEventHandler(
                target,
                received_hotkey,
                1,
                &KEYBOARD_RELEASED,
                ptr::from_mut(callback.as_mut()).cast(),
                &mut handler,
            )
        };
        if result != 0 {
            // SAFETY: This token was registered on this thread, with no installed handler.
            unsafe { UnregisterEventHotKey(key) };
            return Err(result);
        }
        Ok(Self {
            _main_thread: mtm,
            key,
            handler: NonNull::new(handler)
                .expect("Successful handler registration returns a token"),
            callback: Some(callback),
        })
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        // SAFETY: Both tokens are owned by this registration and are removed on the main thread.
        let (handler, key) = unsafe {
            (
                RemoveEventHandler(self.handler),
                UnregisterEventHotKey(self.key),
            )
        };
        if handler != 0 {
            // A handler that could not be removed must retain access to its callback data.
            if let Some(callback) = self.callback.take() {
                let _ = Box::leak(callback);
            }
            tracing::warn!(
                status = handler,
                "Could not remove the hotkey event handler"
            );
        }
        if key != 0 {
            tracing::warn!(status = key, "Could not unregister the hotkey");
        }
    }
}

unsafe extern "C" fn received_hotkey(
    _call: *mut c_void,
    event: NonNull<c_void>,
    data: NonNull<c_void>,
) -> i32 {
    let mut id = EventHotKeyId {
        signature: 0,
        id: 0,
    };
    // SAFETY: Carbon provides a live event and the parameter buffer is sized for EventHotKeyID.
    let result = unsafe {
        GetEventParameter(
            event,
            u32::from_be_bytes(*b"----"),
            u32::from_be_bytes(*b"hkid"),
            ptr::null_mut(),
            size_of::<EventHotKeyId>() as c_ulong,
            ptr::null_mut(),
            ptr::from_mut(&mut id).cast(),
        )
    };
    if result != 0 || id != SEARCH_HOTKEY_ID {
        return EVENT_NOT_HANDLED;
    }
    // SAFETY: Registration keeps a boxed function pointer alive for Carbon's callback.
    let open = unsafe { *data.cast::<fn()>().as_ref() };
    open();
    0
}

#[repr(C)]
struct EventType {
    class: u32,
    kind: u32,
}

#[derive(PartialEq)]
#[repr(C)]
struct EventHotKeyId {
    signature: u32,
    id: u32,
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetEventDispatcherTarget() -> Option<NonNull<c_void>>;
    fn RegisterEventHotKey(
        key: u32,
        modifiers: u32,
        id: EventHotKeyId,
        target: NonNull<c_void>,
        options: u32,
        out_key: *mut *mut c_void,
    ) -> i32;
    fn UnregisterEventHotKey(key: NonNull<c_void>) -> i32;
    fn InstallEventHandler(
        target: NonNull<c_void>,
        handler: unsafe extern "C" fn(*mut c_void, NonNull<c_void>, NonNull<c_void>) -> i32,
        type_count: c_ulong,
        types: *const EventType,
        data: *mut c_void,
        out_handler: *mut *mut c_void,
    ) -> i32;
    fn RemoveEventHandler(handler: NonNull<c_void>) -> i32;
    fn GetEventParameter(
        event: NonNull<c_void>,
        name: u32,
        value_type: u32,
        actual_type: *mut u32,
        buffer_size: c_ulong,
        actual_size: *mut c_ulong,
        buffer: *mut c_void,
    ) -> i32;
}
