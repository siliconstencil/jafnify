//! The laptop window's pages and its back stack.
//!
//! The window is a fixed home page plus one page per view, like the phone:
//! the user sees one short page at a time and returns with Back or Esc. The
//! model is pure so the tests can walk it without an egui context; the window
//! only reads the current page and sends open/back/escape events.
//!
//! The daemon does not know about pages. A tray entry names a page and leaves
//! a hint for the window, which turns the name into a page with [Page::parse].

/// One page of the window. `Home` is the fixed first page and never leaves
/// the stack; every other page is pushed on top of the current one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Page {
    Home,
    Details,
    Drops,
    Log,
    BestSpot,
    Statistics,
    Settings,
    HomeLayout,
    Pairing,
    About,
    Notices,
}

impl Page {
    /// Every page, in the order the tests and the menu walk them.
    pub const ALL: [Page; 11] = [
        Page::Home,
        Page::Details,
        Page::Drops,
        Page::Log,
        Page::BestSpot,
        Page::Statistics,
        Page::Settings,
        Page::HomeLayout,
        Page::Pairing,
        Page::About,
        Page::Notices,
    ];

    /// The stable wire spelling used for a tray hint (also the button id).
    pub fn key(self) -> &'static str {
        match self {
            Page::Home => "home",
            Page::Details => "details",
            Page::Drops => "drops",
            Page::Log => "log",
            Page::BestSpot => "best_spot",
            Page::Statistics => "statistics",
            Page::Settings => "settings",
            Page::HomeLayout => "home_layout",
            Page::Pairing => "pairing",
            Page::About => "about",
            Page::Notices => "notices",
        }
    }

    /// The heading drawn at the top of the page.
    pub fn title(self) -> &'static str {
        match self {
            Page::Home => "Jafnify",
            Page::Details => "Details",
            Page::Drops => "Drops",
            Page::Log => "Connection log",
            Page::BestSpot => "Best spot",
            Page::Statistics => "Statistics",
            Page::Settings => "Settings",
            Page::HomeLayout => "Home layout",
            Page::Pairing => "Pairing",
            Page::About => "About",
            Page::Notices => "Third-party notices",
        }
    }

    /// Parse a saved or tray page name. Unknown names are ignored rather than
    /// opening a wrong page.
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        Page::ALL
            .iter()
            .copied()
            .find(|page| page.key().eq_ignore_ascii_case(value))
    }
}

/// The pages of the back stack. The last element is the page on screen and
/// the first is always [Page::Home].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nav {
    stack: Vec<Page>,
}

impl Default for Nav {
    fn default() -> Self {
        Self::new()
    }
}

impl Nav {
    pub fn new() -> Self {
        Self {
            stack: vec![Page::Home],
        }
    }

    pub fn current(&self) -> Page {
        *self.stack.last().unwrap_or(&Page::Home)
    }

    pub fn is_home(&self) -> bool {
        self.current() == Page::Home
    }

    pub fn can_go_back(&self) -> bool {
        self.stack.len() > 1
    }

    pub fn depth(&self) -> usize {
        self.stack.len()
    }

    /// Open a page. Home is a jump back to the root; opening the page that is
    /// already on screen does nothing, so a second click cannot stack a page
    /// twice.
    pub fn open(&mut self, page: Page) {
        if page == Page::Home {
            self.home();
            return;
        }
        if self.current() == page {
            return;
        }
        self.stack.push(page);
    }

    /// Pop one page. Returns false when the home is already on screen.
    pub fn back(&mut self) -> bool {
        if self.can_go_back() {
            self.stack.pop();
            true
        } else {
            false
        }
    }

    /// Escape is Back. It also returns false at the home.
    pub fn escape(&mut self) -> bool {
        self.back()
    }

    /// Drop every page above the home.
    pub fn home(&mut self) {
        self.stack.truncate(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_starts_on_the_home() {
        let nav = Nav::new();
        assert_eq!(nav.current(), Page::Home);
        assert!(nav.is_home());
        assert!(!nav.can_go_back());
    }

    #[test]
    fn opening_a_page_and_going_back_returns_to_the_home() {
        let mut nav = Nav::new();
        nav.open(Page::Details);
        assert_eq!(nav.current(), Page::Details);
        assert!(nav.can_go_back());
        assert!(nav.back());
        assert_eq!(nav.current(), Page::Home);
        assert!(!nav.can_go_back());
    }

    #[test]
    fn escape_is_back_and_does_nothing_on_the_home() {
        let mut nav = Nav::new();
        nav.open(Page::Settings);
        assert!(nav.escape());
        assert_eq!(nav.current(), Page::Home);
        assert!(!nav.escape(), "Esc on the home is a no-op");
    }

    #[test]
    fn opening_the_same_page_twice_does_not_stack_it() {
        let mut nav = Nav::new();
        nav.open(Page::Drops);
        nav.open(Page::Drops);
        assert_eq!(nav.depth(), 2);
    }

    #[test]
    fn opening_the_home_drops_the_whole_stack() {
        let mut nav = Nav::new();
        nav.open(Page::Details);
        nav.open(Page::Log);
        nav.open(Page::Home);
        assert!(nav.is_home());
        assert_eq!(nav.depth(), 1);
    }

    #[test]
    fn pages_nest_and_unwind_in_order() {
        let mut nav = Nav::new();
        for page in [Page::Statistics, Page::Details, Page::Log] {
            nav.open(page);
        }
        assert_eq!(nav.current(), Page::Log);
        assert!(nav.back());
        assert_eq!(nav.current(), Page::Details);
        assert!(nav.back());
        assert_eq!(nav.current(), Page::Statistics);
        assert!(nav.back());
        assert_eq!(nav.current(), Page::Home);
    }

    #[test]
    fn every_page_key_parses_back_to_its_page() {
        for page in Page::ALL {
            assert_eq!(Page::parse(page.key()), Some(page), "tray hint {}", page.key());
        }
    }

    #[test]
    fn an_unknown_key_is_not_a_page() {
        assert_eq!(Page::parse("not-a-page"), None);
        assert_eq!(Page::parse(""), None);
    }

    #[test]
    fn every_page_has_a_unique_key_and_a_title() {
        let mut keys = std::collections::BTreeSet::new();
        for page in Page::ALL {
            assert!(keys.insert(page.key()), "duplicate key {}", page.key());
            assert!(!page.title().is_empty());
            assert_eq!(Page::parse(page.key()), Some(page));
        }
    }
}
