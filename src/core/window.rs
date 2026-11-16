use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowInfo {
    pub id: String,
    pub pid: i32,
    pub app_name: String,
    pub title: String,
}

pub trait WindowBackend {
    fn list_windows(&self) -> Result<Vec<WindowInfo>, WindowError>;
    fn focus_window(&self, id: &str) -> Result<(), WindowError>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum WindowError {
    AccessibilityDenied,
    WindowUnavailable,
    AmbiguousWindow,
    ActivationFailed,
    Accessibility { operation: &'static str, code: i32 },
}

impl fmt::Display for WindowError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AccessibilityDenied => write!(
                output,
                "Accessibility access is required. In System Settings → Privacy & Security → Accessibility, allow the terminal or app that launched Window Switcher, then retry."
            ),
            Self::WindowUnavailable => write!(
                output,
                "The window is no longer available. Refresh the window list and try again."
            ),
            Self::AmbiguousWindow => write!(
                output,
                "The CLI cannot distinguish these windows. Select the window in the search panel or move one window and list again."
            ),
            Self::ActivationFailed => write!(
                output,
                "macOS did not focus the selected window. Switch to its Space and try again."
            ),
            Self::Accessibility { operation, code } => {
                write!(
                    output,
                    "Accessibility request {operation} failed (AX error {code})."
                )
            }
        }
    }
}

impl std::error::Error for WindowError {}
