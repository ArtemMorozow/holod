use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowInfo {
    pub id: String,
    pub pid: i32,
    pub app_name: String,
    pub bundle_id: Option<String>,
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
    CloseUnsupported,
    CloseUnconfirmed,
    Accessibility { operation: &'static str, code: i32 },
}

impl fmt::Display for WindowError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AccessibilityDenied => write!(
                output,
                "Accessibility access is required. In System Settings → Privacy & Security → Accessibility, allow Holod or the terminal that launched it, then retry."
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
            Self::CloseUnsupported => write!(output, "This window has no available close button."),
            Self::CloseUnconfirmed => write!(
                output,
                "Could not confirm the window closed. Check the app for a confirmation dialog."
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
