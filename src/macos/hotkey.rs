use std::error::Error;

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use objc2::MainThreadMarker;

pub fn register(
    _main_thread: MainThreadMarker,
    custom: Option<&str>,
    open_panel: impl Fn() + Send + Sync + 'static,
) -> Result<(GlobalHotKeyManager, HotKey), Box<dyn Error>> {
    let manager = GlobalHotKeyManager::new()?;
    let hotkey = if let Some(custom) = custom {
        let hotkey: HotKey = custom.parse()?;
        if hotkey.mods.is_empty() {
            return Err("The hotkey must include a modifier, for example Alt+Tab.".into());
        }
        manager.register(hotkey)?;
        hotkey
    } else {
        let preferred = HotKey::new(Some(Modifiers::ALT), Code::Tab);
        match manager.register(preferred) {
            Ok(()) => preferred,
            Err(error) => {
                tracing::warn!(%error, "Option+Tab unavailable. Trying Control+Option+Space");
                let fallback = HotKey::new(Some(Modifiers::ALT | Modifiers::CONTROL), Code::Space);
                manager.register(fallback).map_err(|error| {
                    format!(
                        "Could not register either default hotkey. Set --hotkey explicitly. {error}"
                    )
                })?;
                fallback
            }
        }
    };
    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
        if event.id() == hotkey.id() && event.state() == HotKeyState::Pressed {
            open_panel();
        }
    }));
    Ok((manager, hotkey))
}
