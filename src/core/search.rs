use super::window::WindowInfo;

pub fn matching_rows(windows: &[WindowInfo], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    windows
        .iter()
        .enumerate()
        .filter(|(_, window)| {
            query.is_empty()
                || window.app_name.to_lowercase().contains(&query)
                || window.title.to_lowercase().contains(&query)
        })
        .map(|(index, _)| index)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_app_and_title_case_insensitively_in_display_order() {
        let windows = [
            ("Code", "README.md"),
            ("Safari", "Документация"),
            ("Code", "Rust notes"),
        ]
        .map(|(app, title)| WindowInfo {
            id: title.into(),
            pid: 1,
            app_name: app.into(),
            title: title.into(),
        });

        assert_eq!(matching_rows(&windows, " CODE "), [0, 2]);
        assert_eq!(matching_rows(&windows, "ДОКУМЕНТ"), [1]);
        assert_eq!(matching_rows(&windows, ""), [0, 1, 2]);
        assert!(matching_rows(&windows, "missing").is_empty());
    }
}
