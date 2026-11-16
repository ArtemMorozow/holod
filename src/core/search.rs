use super::window::WindowInfo;

// Keys that produce letters in either the macOS ABC or Russian layout.
const ENGLISH_KEYS: &str = "qwertyuiop[]asdfghjkl;'zxcvbnm,.\\";
const RUSSIAN_KEYS: &str = "йцукенгшщзхъфывапролджэячсмитьбюё";

pub fn matching_rows(windows: &[WindowInfo], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return (0..windows.len()).collect();
    }

    let alternatives = [
        remap_layout(&query, RUSSIAN_KEYS, ENGLISH_KEYS),
        remap_layout(&query, ENGLISH_KEYS, RUSSIAN_KEYS),
    ];
    let find_rows = |matches: &dyn Fn(&str) -> bool| -> Vec<usize> {
        windows
            .iter()
            .enumerate()
            .filter(|(_, window)| {
                matches(&window.app_name.to_lowercase())
                    || matches(&window.title.to_lowercase())
                    || app_aliases(window).iter().copied().any(matches)
            })
            .map(|(index, _)| index)
            .collect()
    };
    let rows = find_rows(&|text| {
        text.contains(&query)
            || alternatives
                .iter()
                .flatten()
                .any(|alternative| text.contains(alternative))
    });
    if !rows.is_empty() {
        return rows;
    }

    // Only offer typo matches for longer layout corrections when neither spelling matched.
    let typo_queries: Vec<_> = alternatives
        .iter()
        .flatten()
        .filter(|query| query.chars().filter(|c| c.is_alphabetic()).take(5).count() == 5)
        .collect();
    if typo_queries.is_empty() {
        return rows;
    }
    find_rows(&|text| {
        typo_queries
            .iter()
            .any(|query| contains_with_one_substitution(text, query))
    })
}

fn remap_layout(query: &str, from: &str, to: &str) -> Option<String> {
    if !query.chars().any(|c| c.is_alphabetic() && from.contains(c)) {
        return None;
    }
    Some(
        query
            .chars()
            .map(|c| {
                from.chars()
                    .zip(to.chars())
                    .find_map(|(key, translated)| (c == key).then_some(translated))
                    .unwrap_or(c)
            })
            .collect(),
    )
}

fn app_aliases(window: &WindowInfo) -> &'static [&'static str] {
    match window.bundle_id.as_deref() {
        Some("com.microsoft.VSCode") => &["vs code", "vs-code", "vscode"],
        _ => &[],
    }
}

fn contains_with_one_substitution(text: &str, query: &str) -> bool {
    text.char_indices().any(|(start, _)| {
        let mut candidate = text[start..].chars();
        let mut mismatches = 0;
        query.chars().all(|expected| {
            if let Some(actual) = candidate.next() {
                mismatches += usize::from(actual != expected);
                mismatches <= 1
            } else {
                false
            }
        })
    })
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
            bundle_id: None,
            title: title.into(),
        });

        assert_eq!(matching_rows(&windows, " CODE "), [0, 2]);
        assert_eq!(matching_rows(&windows, "ДОКУМЕНТ"), [1]);
        assert_eq!(matching_rows(&windows, ""), [0, 1, 2]);
        assert!(matching_rows(&windows, "missing").is_empty());
    }

    #[test]
    fn finds_vs_code_by_alias_with_the_wrong_layout_and_a_mistyped_key() {
        let windows = [
            Some("com.microsoft.VSCode"),
            None,
            Some("com.microsoft.VSCode"),
        ]
        .map(|bundle_id| WindowInfo {
            id: "1".into(),
            pid: 1,
            app_name: "Code".into(),
            bundle_id: bundle_id.map(str::to_owned),
            title: "main.rs".into(),
        });

        for query in ["vs-code", "vscode", "VS Code", " МЫ-СЩВУ ", "мы-сжву"] {
            assert_eq!(matching_rows(&windows, query), [0, 2], "{query}");
        }
    }

    #[test]
    fn matches_both_layouts_by_title_and_uses_typos_only_as_a_fallback() {
        let windows =
            ["Coda report", "Отчёт", "Code report", "сщву report"].map(|title| WindowInfo {
                id: title.into(),
                pid: 1,
                app_name: "Editor".into(),
                bundle_id: None,
                title: title.into(),
            });

        assert_eq!(matching_rows(&windows, "сщву"), [2, 3]);
        assert_eq!(matching_rows(&windows, "jnx\\n"), [1]);
        assert_eq!(matching_rows(&windows, "сщву кузщке"), [2]);
        assert!(matching_rows(&windows, "сщвщ").is_empty());
    }
}
