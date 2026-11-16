use std::collections::VecDeque;

const HISTORY_LIMIT: usize = 128;

pub struct Recency<T> {
    recent: VecDeque<T>,
}

impl<T> Default for Recency<T> {
    fn default() -> Self {
        Self {
            recent: VecDeque::new(),
        }
    }
}

impl<T: PartialEq> Recency<T> {
    pub fn visit(&mut self, window: T) {
        if self.recent.front() == Some(&window) {
            return;
        }
        self.recent.retain(|visited| visited != &window);
        self.recent.push_front(window);
        self.recent.truncate(HISTORY_LIMIT);
    }

    pub fn sort_key(&self, window: &T, current: Option<&T>) -> (bool, usize) {
        let rank = self.recent.iter().position(|visited| visited == window);
        (current == Some(window), rank.unwrap_or(usize::MAX))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_previous_first_reorders_revisits_and_omits_closed_windows() {
        let mut history = Recency::default();
        for window in ["editor", "terminal", "browser", "closed", "terminal"] {
            history.visit(window);
        }

        let mut open = ["browser", "editor", "notes", "terminal", "workspace"];
        open.sort_by_cached_key(|window| history.sort_key(window, Some(&"terminal")));
        assert_eq!(
            open,
            ["browser", "editor", "notes", "workspace", "terminal"]
        );

        history.visit("browser");
        open.sort_by_cached_key(|window| history.sort_key(window, Some(&"browser")));
        assert_eq!(
            open,
            ["terminal", "editor", "notes", "workspace", "browser"]
        );
    }

    #[test]
    fn leaves_unvisited_windows_in_input_order_and_puts_the_active_window_last() {
        let history = Recency::default();
        let mut open = ["browser", "editor", "terminal"];
        open.sort_by_cached_key(|window| history.sort_key(window, Some(&"browser")));
        assert_eq!(open, ["editor", "terminal", "browser"]);
    }
}
