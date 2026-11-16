use std::io::{self, Write};

use crate::core::window::WindowBackend;

pub const HELP: &str = "\
Usage:
  holod
  holod gui
  holod list
  holod focus <id>
  holod --help";

pub enum Command<'a> {
    Help,
    Gui,
    List,
    Focus(&'a str),
}

impl<'a> Command<'a> {
    pub fn parse(args: &'a [String]) -> Result<Self, String> {
        match args {
            [] => Ok(Self::Gui),
            [arg] if arg == "-h" || arg == "--help" => Ok(Self::Help),
            [command] if command == "gui" => Ok(Self::Gui),
            [command] if command == "list" => Ok(Self::List),
            [command, id] if command == "focus" => Ok(Self::Focus(id)),
            _ => Err(format!("Invalid arguments.\n\n{HELP}")),
        }
    }
}

pub fn run(
    command: Command<'_>,
    backend: &impl WindowBackend,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Command::Gui = command {
        return crate::ui::search_panel::run();
    }
    run_with_output(command, backend, &mut io::stdout().lock())
}

fn run_with_output(
    command: Command<'_>,
    backend: &impl WindowBackend,
    output: &mut impl Write,
) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        Command::Help => writeln!(output, "{HELP}")?,
        Command::Gui => unreachable!("GUI command handled before locking stdout"),
        Command::List => {
            for window in backend.list_windows()? {
                let app = window.app_name.replace(['\t', '\n', '\r'], " ");
                let title = window.title.replace(['\t', '\n', '\r'], " ");
                writeln!(output, "{}\t{app}\t{title}", window.id)?;
            }
        }
        Command::Focus(id) => backend.focus_window(id)?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use crate::core::window::{WindowError, WindowInfo};

    use super::*;

    #[test]
    fn lists_and_focuses_distinct_windows_of_one_app() {
        struct Backend(RefCell<Option<String>>);

        impl WindowBackend for Backend {
            fn list_windows(&self) -> Result<Vec<WindowInfo>, WindowError> {
                Ok([("1", "First"), ("2", "Second")]
                    .into_iter()
                    .map(|(id, title)| WindowInfo {
                        id: id.into(),
                        pid: 42,
                        app_name: "Code".into(),
                        bundle_id: Some("com.microsoft.VSCode".into()),
                        title: title.into(),
                    })
                    .collect())
            }

            fn focus_window(&self, id: &str) -> Result<(), WindowError> {
                if id != "1" && id != "2" {
                    return Err(WindowError::WindowUnavailable);
                }
                self.0.replace(Some(id.into()));
                Ok(())
            }
        }

        let backend = Backend(RefCell::new(None));
        let mut output = Vec::new();
        run_with_output(Command::List, &backend, &mut output).unwrap();
        assert_eq!(output, b"1\tCode\tFirst\n2\tCode\tSecond\n");
        run_with_output(Command::Focus("2"), &backend, &mut output).unwrap();
        assert_eq!(backend.0.borrow().as_deref(), Some("2"));
        let error = run_with_output(Command::Focus("closed"), &backend, &mut output).unwrap_err();
        assert_eq!(error.downcast_ref(), Some(&WindowError::WindowUnavailable));
    }
}
