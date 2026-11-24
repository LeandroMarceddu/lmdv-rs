//! The outline pane and its per-tab filter (O-01, O-05, O-06).
//!
//! The outline is built from [`lmdv_md::MdDoc::headings`]: levels 1 to 4, in
//! document order, each jumping to the heading's rendered key (`h{i}`). It is
//! hidden for documents without such headings and remembers, per tab, whether
//! the user closed it and what the filter said. Footnote-definition headings
//! (`block == usize::MAX`, OI-028) never appear.

use rune::{Cx, El, Tx, text};
use rune_kit::shell::{Sidebar, SidebarHeader, SidebarRow};
use rune_kit::{Kit, TextInput};

use crate::reader::{active_heading, has_toc_headings, toc_headings};
use crate::window::WindowState;

/// The outline filter appears from this many headings on (O-05).
pub const OUTLINE_FILTER_MIN_HEADINGS: usize = 8;
/// The most outline rows drawn for one document. A generated file with
/// thousands of headings keeps the frame bounded until P6 virtualizes the pane.
pub const OUTLINE_MAX_ROWS: usize = 500;

/// One row the outline draws.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Row {
    key: String,
    level: u8,
    text: String,
    /// The heading's index in the document, for the filter cursor (O-05).
    index: usize,
}

impl WindowState {
    /// Whether the active document has any rendered headings.
    pub(crate) fn has_active_headings(&self) -> bool {
        self.active_reader().is_some_and(|doc| has_toc_headings(&doc))
    }

    /// Shows or hides the outline for the active tab.
    ///
    /// A document without headings hides the pane and says why. The start page
    /// accepts the toggle; the next sync closes the empty pane.
    pub(crate) fn toggle_outline(&mut self) -> bool {
        let Some(tab) = self.active_tab() else { return false };
        let has_headings = tab.view.reader.as_ref().is_some_and(|doc| has_toc_headings(doc));
        if !tab.is_start() && !has_headings {
            if self.right.is_open() {
                self.right.set_open(false);
                return true;
            }
            self.notice(lmdv_i18n::t!("outline.noHeadings"));
            return true;
        }
        let open = !self.right.is_open();
        self.right.set_open(open);
        if let Some(tab) = self.active_tab_mut() {
            tab.view.outline_hidden = !open;
        }
        true
    }

    /// Makes the pane follow the active tab's headings and its own last choice.
    pub(crate) fn sync_outline(&mut self) -> bool {
        let want = self.active_tab().is_some_and(|tab| {
            tab.view.reader.as_ref().is_some_and(|doc| has_toc_headings(doc)) && !tab.view.outline_hidden
        });
        if self.right.is_open() == want {
            return false;
        }
        self.right.set_open(want);
        true
    }

    /// Copies the filter field into the active tab before a switch or close.
    pub(crate) fn save_outline_filter(&mut self) {
        let text = self.outline_filter.text.clone();
        if let Some(tab) = self.active_tab_mut() {
            tab.view.toc_filter = text;
        }
    }

    /// Restores the active tab's filter into the field after a switch.
    pub(crate) fn load_outline_filter(&mut self) {
        let text = self.active_tab().map(|tab| tab.view.toc_filter.clone()).unwrap_or_default();
        if self.outline_filter.text != text {
            self.outline_filter.replace_text(text);
        }
        // O-05: the cursor is the filter's, not the tab's; a switch starts it
        // on the new tab's first visible row.
        self.reset_outline_cursor();
    }

    /// Applies a typed filter to the active tab immediately (the view reads it).
    ///
    /// O-05: the first visible row becomes the keyboard cursor, so Enter jumps
    /// where the filter points without a second key.
    pub(crate) fn outline_filter_changed(&mut self) {
        let text = self.outline_filter.text.clone();
        if let Some(tab) = self.active_tab_mut() {
            tab.view.toc_filter = text;
        }
        self.reset_outline_cursor();
    }

    /// Clears the filter field and the active tab's filter.
    pub(crate) fn clear_outline_filter(&mut self) {
        self.outline_filter.replace_text("");
        self.outline_filter_changed();
    }

    /// Puts the filter cursor on the first visible row, or drops it when the
    /// filter is empty or the field is not drawn (O-05).
    fn reset_outline_cursor(&mut self) {
        let first = self
            .active_tab()
            .and_then(|tab| tab.view.reader.as_ref().map(|doc| (tab.view.toc_filter.clone(), doc)))
            .filter(|(filter, doc)| cursor_eligible(filter, doc))
            .and_then(|(filter, doc)| visible_heading_indices(doc, &filter).first().copied());
        self.reading.outline_cursor.set(first);
    }

    /// `↑`/`↓` in the outline filter: moves the keyboard cursor among the
    /// visible rows and keeps `Enter` on it (O-05).
    ///
    /// False when the field or the filter is absent, so the arrow keeps its
    /// ordinary caret movement.
    pub(crate) fn move_outline_cursor(&mut self, delta: isize) -> bool {
        let Some(tab) = self.active_tab() else { return false };
        let Some(doc) = tab.view.reader.as_ref() else { return false };
        if !cursor_eligible(&tab.view.toc_filter, doc) || delta == 0 {
            return false;
        }
        let visible = visible_heading_indices(doc, &tab.view.toc_filter);
        let Some(last) = visible.len().checked_sub(1) else {
            self.reading.outline_cursor.set(None);
            return true;
        };
        let current = self.reading.outline_cursor.get().and_then(|index| visible.iter().position(|row| *row == index));
        let next = match current {
            // The first visible row is the implicit cursor after typing; Down
            // steps off it, Up stays on it.
            None if delta > 0 => 1.min(last),
            None => 0,
            Some(position) => (position as isize + delta).clamp(0, last as isize) as usize,
        };
        self.reading.outline_cursor.set(Some(visible[next]));
        true
    }

    /// The heading `Enter` in the filter jumps to: the cursor's row, or the
    /// first visible row when the cursor is not on one (O-05).
    pub(crate) fn outline_cursor_heading_key(&self) -> Option<String> {
        let tab = self.active_tab()?;
        let doc = tab.view.reader.as_ref()?;
        let visible = visible_heading_indices(doc, &tab.view.toc_filter);
        let index = self
            .reading
            .outline_cursor
            .get()
            .filter(|index| visible.contains(index))
            .or_else(|| visible.first().copied())?;
        Some(lmdv_render::heading_key(index))
    }

    /// The first heading that passes the active filter, for tests of the
    /// filter's threshold rule.
    ///
    /// Below [`OUTLINE_FILTER_MIN_HEADINGS`] the field is absent, so a filter
    /// left on the tab (a tab that lost headings, a transfer) is not applied:
    /// the first outline heading is the target.
    #[cfg(test)]
    pub(crate) fn first_visible_heading_key(&self) -> Option<String> {
        let tab = self.active_tab()?;
        let doc = tab.view.reader.as_ref()?;
        let total = toc_headings(doc).count();
        let needle = if total >= OUTLINE_FILTER_MIN_HEADINGS {
            tab.view.toc_filter.trim().to_lowercase()
        } else {
            String::new()
        };
        toc_headings(doc)
            .find(|(_, heading)| needle.is_empty() || heading.text.to_lowercase().contains(&needle))
            .map(|(index, _)| lmdv_render::heading_key(index))
    }

    /// Scrolls the article to a heading and keeps its outline row in view.
    ///
    /// The place being left goes on the tab's Back list unless the jump would
    /// not move the page (O-02/O-12). The section line steps aside as the jump
    /// starts and stays away until the page has landed (O-09/O-10): both the
    /// outline this pane draws and the section line's parts go through here.
    pub(crate) fn jump_to_heading(&mut self, key: &str, tx: &mut Tx) {
        self.note_jump(key);
        self.scroll_to_heading(key, tx);
    }

    /// The scroll half of [`Self::jump_to_heading`], with no place recorded.
    ///
    /// A `file.md#section` link's landing scrolls through here: it is part of
    /// the open that already recorded its place, not a place of its own (O-12).
    ///
    /// OI-156: the jump glides on Rune's spring instead of snapping, landing at
    /// the heading's last measured top less [`JUMP_INSET`](crate::nav::JUMP_INSET),
    /// clamped to the document; reduced motion makes the same request jump
    /// (A-15). The heading is pinned (O-04) so the outline keeps its row while
    /// the page moves, and the section line steps aside until it lands (O-09).
    /// A heading the last frame did not measure — the first frame after a
    /// parse — falls back to the key-based instant request.
    pub(crate) fn scroll_to_heading(&mut self, key: &str, tx: &mut Tx) {
        let now = tx.clock();
        let scroll = self.reading.scroll.get();
        let index = key.strip_prefix('h').and_then(|digits| digits.parse::<usize>().ok());
        // L-01: 24 points below the top, plus a measured sticky table header
        // the landing point would sit under. The measured top is only useful
        // while the page can scroll.
        let measured = index.and_then(|index| self.heading_top(index));
        let inset = crate::nav::landing_inset(&self.section_line.tables(), scroll, measured.unwrap_or(0.0));
        let destination =
            measured.filter(|_| scroll.max > 0.0).map(|top| crate::nav::heading_destination(scroll, top, inset));
        match destination {
            Some(y) => tx.scroll_to(crate::reader::ARTICLE_SCROLL_KEY, 0.0, y),
            None => tx.scroll_key_to(key, inset),
        }
        if let (Some(index), Some(revision)) = (index, self.active_reader().map(|doc| doc.revision)) {
            let signal = self.scroll_signal();
            self.reading_keys.pin_heading(index, revision, now, signal);
        }
        tx.scroll_into_view(format!("outline-{key}"));
        // O-03: the pane already shows this row; the per-frame spy check must
        // not issue its own reveal while the jump's pin owns the highlight.
        self.reading.outline_revealed = Some(key.to_owned());
        // O-09/OI-142: the line stays away until the glide settles, and the
        // landed heading lights up for a moment (O-10). The landing poll
        // releases the line and wakes the view; no fixed timer here.
        if index.is_some_and(|index| self.expect_landing(index, now)) {
            self.section_line.hold_until_landing();
        }
    }

    /// Keeps the scroll spy's active row inside the outline's viewport (O-03).
    ///
    /// Once per active-heading change rather than every frame: a reader who
    /// scrolls the outline by hand keeps that place until the page moves the
    /// spy. A jump already reveals its own row ([`Self::scroll_to_heading`]),
    /// so this only answers the spy and the pinned heading.
    pub(crate) fn reveal_active_outline_row(&mut self, tx: &mut Tx) -> bool {
        if !self.right.is_open() {
            self.reading.outline_revealed = None;
            return false;
        }
        let Some(doc) = self.active_reader() else { return false };
        let active = self.pinned_heading(&doc).or_else(|| {
            spy_heading(&doc, &self.reading.heading_tops.borrow(), self.scroll_snapshot()).map(lmdv_render::heading_key)
        });
        let Some(key) = active else { return false };
        if self.reading.outline_revealed.as_deref() == Some(key.as_str()) {
            return false;
        }
        self.reading.outline_revealed = Some(key.clone());
        tx.scroll_into_view(format!("outline-{key}"));
        true
    }
}

/// The scroll spy's active heading from the tops the reading view measured (O-03).
///
/// [`crate::reader::active_heading`] reads the same layout through `Cx` in the
/// view. `poll` has no `Cx`, so this copy works from the tops and viewport the
/// view published: the same activation line, including its slide through the
/// last viewport so the trailing headings can still become active.
pub(crate) fn spy_heading(
    doc: &lmdv_md::MdDoc,
    tops: &[crate::section::MeasuredHeading],
    scroll: crate::reader::ScrollSnapshot,
) -> Option<usize> {
    let activation = crate::reader::activation_line(scroll.viewport, scroll);
    let mut active = None;
    for (index, heading) in doc.headings.iter().enumerate() {
        // A footnote definition's heading is not a document block (OI-028).
        if !heading.in_toc || heading.block == usize::MAX {
            continue;
        }
        let Some(measured) = tops.iter().find(|measured| measured.index == index) else { continue };
        if measured.top > activation {
            break;
        }
        active = Some(index);
    }
    active.or_else(|| toc_headings(doc).next().map(|(index, _)| index))
}

/// The heading indices the outline draws, in order, for the active tab's
/// filter (O-05).
///
/// The same 500-row cap and accent-insensitive lowercase substring rule the
/// pane uses, so the keyboard cursor can never point at a row that is not on
/// screen.
fn visible_heading_indices(doc: &lmdv_md::MdDoc, filter: &str) -> Vec<usize> {
    let needle = filter.trim().to_lowercase();
    toc_headings(doc)
        .take(OUTLINE_MAX_ROWS)
        .filter(|(_, heading)| needle.is_empty() || heading.text.to_lowercase().contains(&needle))
        .map(|(index, _)| index)
        .collect()
}

/// Whether the filter field is drawn with text in it: the only state where the
/// outline has a keyboard cursor (O-05).
fn cursor_eligible(filter: &str, doc: &lmdv_md::MdDoc) -> bool {
    toc_headings(doc).count() >= OUTLINE_FILTER_MIN_HEADINGS && !filter.trim().is_empty()
}

/// The inspector's content for the active tab.
pub(crate) fn pane(state: &WindowState, cx: &Cx<WindowState>, kit: &Kit) -> El {
    // The closed pane is still built (then clipped), so skip the layout reads
    // the scroll-spy needs until the user actually opens it.
    if !state.right.is_open() {
        return Sidebar::new().label(lmdv_i18n::t!("pane.outline")).build(kit);
    }
    let Some(tab) = state.active_tab() else { return empty(kit) };
    if !state.has_active_headings() {
        return empty(kit);
    }
    let Some(doc) = state.active_reader() else { return empty(kit) };
    // O-04: while a jump lands, its heading owns the highlight, so the scroll
    // spy cannot flick through the headings the page passes on the way. OI-017:
    // the activation line slides through the last viewport, so the active row
    // reaches the trailing headings too.
    let active = state.pinned_heading(&doc).or_else(|| active_heading(cx, &doc, state.scroll_snapshot()));
    let rows: Vec<Row> = toc_headings(&doc)
        .map(|(index, heading)| Row {
            key: lmdv_render::heading_key(index),
            level: heading.level,
            text: heading.text.clone(),
            index,
        })
        .collect();
    let total = rows.len();
    // Below the threshold the field is not drawn, so the tab's filter is not
    // applied either: a filter left from a longer revision of the document
    // must never hide every row with no visible way to clear it.
    let filtered = total >= OUTLINE_FILTER_MIN_HEADINGS;
    let needle = if filtered { tab.view.toc_filter.trim().to_lowercase() } else { String::new() };
    let visible: Vec<&Row> = rows
        .iter()
        .take(OUTLINE_MAX_ROWS)
        .filter(|row| needle.is_empty() || row.text.to_lowercase().contains(&needle))
        .collect();
    // O-05: the first visible row is the cursor after typing; the stored index
    // survives the frame, and a cursor that the new filter hid falls back to
    // the first visible row. Without a filter text there is no cursor, so the
    // scroll spy's active heading keeps the selection.
    let cursor = if filtered && !needle.is_empty() {
        state
            .reading
            .outline_cursor
            .get()
            .filter(|index| visible.iter().any(|row| row.index == *index))
            .or_else(|| visible.first().map(|row| row.index))
    } else {
        None
    };

    let mut sidebar = Sidebar::new()
        .label(lmdv_i18n::t!("pane.outline"))
        .header(SidebarHeader::new(lmdv_i18n::t!("pane.outline")).count(total).build(kit));

    if filtered {
        sidebar = sidebar.header(
            TextInput::new(&state.outline_filter)
                .placeholder(lmdv_i18n::t!("pane.filterPlaceholder"))
                .label(lmdv_i18n::t!("outline.filter"))
                .width(None)
                .plain()
                .clear_button()
                .on_edit(|state: &mut WindowState| &mut state.outline_filter)
                .on_change(|state: &mut WindowState, _: &mut Tx| state.outline_filter_changed())
                .on_key(|state: &mut WindowState, event: &rune::KeyEvent, tx: &mut Tx| {
                    // O-05: Esc clears and leaves the field; the arrows move the
                    // filter's keyboard cursor among the visible rows.
                    match event.key {
                        rune::Key::Escape => {
                            state.clear_outline_filter();
                            tx.blur();
                            true
                        }
                        rune::Key::Down => state.move_outline_cursor(1),
                        rune::Key::Up => state.move_outline_cursor(-1),
                        _ => false,
                    }
                })
                .on_submit(|state: &mut WindowState, tx: &mut Tx| {
                    if let Some(key) = state.outline_cursor_heading_key() {
                        state.jump_to_heading(&key, tx);
                    }
                })
                .on_action(|state: &mut WindowState, tx: &mut Tx| {
                    state.clear_outline_filter();
                    tx.focus("outline-filter", false);
                })
                .build(kit)
                .key("outline-filter"),
        );
    }

    if visible.is_empty() {
        sidebar = sidebar.child(
            text(lmdv_i18n::t!("outline.noMatches"))
                .key("outline-empty")
                .font(kit.ty.small)
                .color(kit.c.text_faint)
                .px(8.0)
                .py(8.0)
                .w_full(),
        );
        return sidebar.build(kit);
    }

    for row in visible {
        let selected = match cursor {
            Some(cursor) => row.index == cursor,
            None => active.as_deref() == Some(row.key.as_str()),
        };
        let key = row.key.clone();
        let mut row_builder = SidebarRow::new(row.text.clone())
            .depth(row.level.saturating_sub(1))
            .selected(selected)
            .on_select(move |state: &mut WindowState, tx: &mut Tx| state.jump_to_heading(&key, tx));
        if cursor.is_some() {
            // O-05: with the keyboard cursor on, the filter field steers the
            // list (↑/↓, Enter), so a row never takes the Tab stop or the
            // keyboard; a press still selects and keeps the field focused.
            // An empty filter leaves the rows ordinary Tab stops.
            row_builder = row_builder.select_only();
        }
        let sidebar_row = row_builder
            .build(kit)
            .key(format!("outline-{}", row.key))
            // OI-017: the row is a button (the original draws `<button>`s under
            // a navigation landmark). The kit ends every row as a `listitem`
            // after its handlers; the explicit role restores the button the
            // press handler made it. The list wrapper stays (rune-kit's shape).
            .role("button");
        sidebar = sidebar.child(sidebar_row);
    }
    if total > OUTLINE_MAX_ROWS {
        sidebar = sidebar.footer(
            text(lmdv_i18n::t!("outline.truncated", "max" => OUTLINE_MAX_ROWS, "total" => total))
                .font(kit.ty.small)
                .color(kit.c.text_faint),
        );
    }
    sidebar.build(kit)
}

fn empty(kit: &Kit) -> El {
    Sidebar::new()
        .label(lmdv_i18n::t!("pane.outline"))
        .header(SidebarHeader::new(lmdv_i18n::t!("pane.outline")).build(kit))
        .child(SidebarRow::new(lmdv_i18n::t!("outline.empty")).subtitle(lmdv_i18n::t!("outline.emptyHint")).build(kit))
        .build(kit)
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::test_support;

    fn window(temp: &TempDir) -> WindowState {
        WindowState::new(test_support::shared(temp))
    }

    /// A document with `count` h2 headings, each with a body paragraph.
    fn headings(count: usize, prefix: &str) -> String {
        let mut source = String::new();
        for index in 0..count {
            source.push_str(&format!("## {prefix} {index}\n\nBody {index}.\n\n"));
        }
        source
    }

    /// A window showing `count` headings with the outline open.
    fn outlined(temp: &TempDir, count: usize, prefix: &str) -> WindowState {
        let mut state = window(temp);
        state.open_synthetic("Outline", &headings(count, prefix));
        state.right.set_open(true);
        if let Some(tab) = state.active_tab_mut() {
            tab.view.outline_hidden = false;
        }
        state
    }

    /// One headless frame of the window, as `--tree` sees it.
    fn draw(state: WindowState) -> Vec<rune::ElementInfo> {
        let mut runtime = rune::Runtime::new(state, crate::view::view, (1100.0, 760.0), rune::Env::new(false, 2.0));
        runtime.frame(0.0);
        runtime.elements()
    }

    fn has_key(elements: &[rune::ElementInfo], key: &str) -> bool {
        elements.iter().any(|element| element.key.as_deref() == Some(key))
    }

    /// The drawn outline rows, by label.
    fn row_labels(elements: &[rune::ElementInfo]) -> Vec<String> {
        elements
            .iter()
            .filter(|element| !element.exiting)
            .filter(|element| element.key.as_deref().is_some_and(|key| key.starts_with("outline-h")))
            .filter_map(|element| element.label.clone())
            .collect()
    }

    #[test]
    fn the_filter_appears_from_eight_headings_and_never_at_seven() {
        let temp = TempDir::new().unwrap();
        let seven = outlined(&temp, 7, "Section");
        let elements = draw(seven);
        assert!(!has_key(&elements, "outline-filter"), "O-05: seven headings have no filter");
        assert_eq!(row_labels(&elements).len(), 7);

        // The field is absent below the threshold, so a filter left on the tab
        // must not hide rows with no visible way to clear it.
        let mut seven = outlined(&temp, 7, "Section");
        seven.outline_filter.replace_text("no such heading");
        seven.outline_filter_changed();
        assert_eq!(
            seven.first_visible_heading_key().as_deref(),
            Some("h0"),
            "Enter in an absent field still targets the first heading"
        );
        let elements = draw(seven);
        assert!(!has_key(&elements, "outline-filter"));
        assert!(!has_key(&elements, "outline-empty"));
        assert_eq!(row_labels(&elements).len(), 7, "a stale filter is not applied without its field");

        let eight = outlined(&temp, 8, "Section");
        let elements = draw(eight);
        assert!(has_key(&elements, "outline-filter"), "O-05: eight headings show the filter");
    }

    #[test]
    fn a_filter_with_no_matches_shows_the_empty_state() {
        let temp = TempDir::new().unwrap();
        let mut state = outlined(&temp, 10, "Section");
        state.outline_filter.replace_text("no such heading");
        state.outline_filter_changed();
        let elements = draw(state);
        assert!(has_key(&elements, "outline-filter"), "the field stays, so the filter can be cleared");
        assert!(has_key(&elements, "outline-empty"), "the no-matches row is drawn");
        assert!(row_labels(&elements).is_empty(), "no heading rows remain");
    }

    #[test]
    fn escape_clears_the_filter_and_leaves_the_field() {
        let temp = TempDir::new().unwrap();
        let mut state = outlined(&temp, 9, "Section");
        state.outline_filter.replace_text("no such heading");
        state.outline_filter_changed();
        let mut runtime = rune::Runtime::new(state, crate::view::view, (1100.0, 760.0), rune::Env::new(false, 2.0));
        runtime.frame(0.0);
        runtime.update(|_, tx| tx.focus("outline-filter", false));
        runtime.frame(0.0);
        runtime.key(rune::KeyEvent { key: rune::Key::Escape, mods: rune::Mods::default() });
        runtime.frame(0.0);
        assert_eq!(runtime.state.outline_filter.text, "", "O-05: Esc clears the filter");
        assert_eq!(
            runtime.state.active_tab().map(|tab| tab.view.toc_filter.as_str()),
            Some(""),
            "and the tab's filter"
        );
        let input = runtime
            .elements()
            .into_iter()
            .find(|element| element.key.as_deref() == Some("input"))
            .expect("the filter's input");
        assert!(!input.focused, "and leaves the field");
    }

    #[test]
    fn the_filter_survives_a_tab_switch_and_filters_that_tab() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let first = state.open_synthetic("Alpha", &headings(9, "Alpha"));
        state.right.set_open(true);
        state.outline_filter.replace_text("Alpha 3");
        state.outline_filter_changed();

        let second = state.open_synthetic("Beta", &headings(9, "Beta"));
        assert_eq!(state.outline_filter.text, "", "a new tab starts unfiltered");
        state.outline_filter.replace_text("Beta 5");
        state.outline_filter_changed();

        let mut runtime = rune::Runtime::new(state, crate::view::view, (1100.0, 760.0), rune::Env::new(false, 2.0));
        runtime.frame(0.0);
        runtime.update(|state, _| {
            assert!(state.activate_tab(first));
            if let Some(tab) = state.active_tab_mut() {
                tab.view.outline_hidden = false;
            }
            state.right.set_open(true);
        });
        runtime.frame(0.0);
        assert_eq!(runtime.state.outline_filter.text, "Alpha 3", "the first tab's filter comes back");
        let elements = runtime.elements();
        assert_eq!(row_labels(&elements), ["Alpha 3"], "and it filters the first tab");
        assert!(!has_key(&elements, "outline-empty"));

        runtime.update(|state, _| {
            assert!(state.activate_tab(second));
            if let Some(tab) = state.active_tab_mut() {
                tab.view.outline_hidden = false;
            }
            state.right.set_open(true);
        });
        runtime.frame(0.0);
        assert_eq!(runtime.state.outline_filter.text, "Beta 5");
        let elements = runtime.elements();
        assert_eq!(row_labels(&elements), ["Beta 5"], "each tab keeps its own filter");
    }

    #[test]
    fn the_filter_cursor_starts_on_the_first_visible_row_and_the_arrows_move_it() {
        let temp = TempDir::new().unwrap();
        let mut state = outlined(&temp, 10, "Section");
        state.outline_filter.replace_text("Section");
        state.outline_filter_changed();
        assert_eq!(state.reading.outline_cursor.get(), Some(0), "O-05: typing puts the cursor on the first visible");

        assert!(state.move_outline_cursor(1));
        assert_eq!(state.reading.outline_cursor.get(), Some(1), "↓ steps to the second row");
        assert!(state.move_outline_cursor(-1));
        assert_eq!(state.reading.outline_cursor.get(), Some(0), "↑ comes back");
        assert!(state.move_outline_cursor(-1));
        assert_eq!(state.reading.outline_cursor.get(), Some(0), "the first row clamps");
        for _ in 0..40 {
            state.move_outline_cursor(1);
        }
        assert_eq!(state.reading.outline_cursor.get(), Some(9), "the last visible row clamps");

        // The drawn selection follows the cursor, not the scroll spy's heading.
        let elements = draw(state);
        let row = |key: &str| {
            elements.iter().find(|element| element.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("{key}"))
        };
        assert!(row("outline-h9").item_selected, "the cursor row is selected");
        assert!(!row("outline-h0").item_selected, "the first row is not while the cursor is elsewhere");
    }

    #[test]
    fn enter_in_the_filter_jumps_to_the_cursor_row() {
        let temp = TempDir::new().unwrap();
        let mut state = outlined(&temp, 10, "Section");
        state.outline_filter.replace_text("Section");
        state.outline_filter_changed();
        let mut runtime = rune::Runtime::new(state, crate::view::view, (1100.0, 760.0), rune::Env::new(false, 2.0));
        runtime.frame(0.0);
        runtime.update(|_, tx| tx.focus("outline-filter", false));
        runtime.frame(0.0);
        runtime.key(rune::KeyEvent { key: rune::Key::Down, mods: rune::Mods::default() });
        runtime.frame(0.0);
        assert_eq!(runtime.state.reading.outline_cursor.get(), Some(1), "↓ moved the cursor");
        runtime.key(rune::KeyEvent { key: rune::Key::Enter, mods: rune::Mods::default() });
        for _ in 0..4 {
            runtime.frame(1.0 / 60.0);
        }
        let doc = runtime.state.active_reader().expect("a model");
        assert_eq!(
            runtime.state.pinned_heading(&doc).as_deref(),
            Some("h1"),
            "O-05: Enter jumps to the cursor row, not the first visible heading"
        );
    }

    #[test]
    fn clearing_the_filter_drops_the_cursor_back_to_the_scroll_spy() {
        let temp = TempDir::new().unwrap();
        let mut state = outlined(&temp, 10, "Section");
        state.outline_filter.replace_text("Section 4");
        state.outline_filter_changed();
        assert_eq!(state.reading.outline_cursor.get(), Some(4));
        state.clear_outline_filter();
        assert_eq!(state.reading.outline_cursor.get(), None, "an empty filter has no keyboard cursor");
        assert!(!state.move_outline_cursor(1), "and the arrows stay the field's own");
    }

    #[test]
    fn the_outline_follows_headings_and_the_user_choice() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        assert!(!state.has_active_headings());
        assert!(state.toggle_outline(), "the start page keeps a manual toggle");
        assert!(state.right.is_open());

        state.open_synthetic("Note", "# One\n\n## Two\n");
        state.right.set_open(true);
        assert!(state.has_active_headings());
        assert!(state.toggle_outline(), "closing the outline is remembered");
        assert!(!state.right.is_open());
        assert!(!state.sync_outline(), "a closed outline stays closed");

        state.open_synthetic("Plain", "No headings here.\n");
        assert!(!state.sync_outline(), "the pane was already closed");
        assert!(!state.right.is_open());
        assert!(state.toggle_outline());
        assert!(!state.right.is_open(), "a document without headings cannot open it");
        assert!(state.ui.notice.is_some());
    }

    #[test]
    fn the_filter_is_per_tab() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let first = state.open_synthetic("One", &headings(9, "Alpha"));
        state.outline_filter.replace_text("Alpha 7");
        state.outline_filter_changed();
        assert_eq!(state.tab(first).map(|tab| tab.view.toc_filter.as_str()), Some("Alpha 7"));

        let second = state.open_synthetic("Two", &headings(9, "Beta"));
        assert_eq!(state.outline_filter.text, "", "the new tab starts unfiltered");
        state.outline_filter.replace_text("Beta 5");
        state.outline_filter_changed();

        assert!(state.activate_tab(first));
        assert_eq!(state.outline_filter.text, "Alpha 7");
        assert_eq!(state.first_visible_heading_key().as_deref(), Some("h7"));

        assert!(state.activate_tab(second));
        assert_eq!(state.outline_filter.text, "Beta 5");
        assert_eq!(state.first_visible_heading_key().as_deref(), Some("h5"));
    }

    #[test]
    fn footnote_definitions_never_reach_the_outline() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        state.open_synthetic("Note", "# One\n\nText.[^1]\n\n[^1]: Body\n");
        assert!(state.has_active_headings());
        assert_eq!(state.first_visible_heading_key().as_deref(), Some("h0"));
    }

    #[test]
    fn the_spy_heading_follows_the_activation_line_and_the_last_viewport() {
        use crate::reader::ScrollSnapshot;
        use crate::section::MeasuredHeading;

        let doc = lmdv_md::parse("# One\n\n## Two\n\n### Three\n");
        let tops = |values: &[(usize, f32)]| -> Vec<MeasuredHeading> {
            values
                .iter()
                .map(|(index, top)| MeasuredHeading { index: *index, top: *top, bottom: *top + 20.0 })
                .collect()
        };

        // Far from the end the line is the fixed 90 pt.
        let scroll = ScrollSnapshot { offset: 100.0, max: 2000.0, viewport: 700.0 };
        assert_eq!(spy_heading(&doc, &tops(&[(0, -300.0), (1, -100.0), (2, 140.0)]), scroll), Some(1));
        // At the end the line reaches the viewport's bottom, so the last
        // heading activates even though it never reaches the top (O-03).
        let end = ScrollSnapshot { offset: 2000.0, max: 2000.0, viewport: 700.0 };
        assert_eq!(spy_heading(&doc, &tops(&[(0, -2000.0), (1, -1500.0), (2, 600.0)]), end), Some(2));
        // Without a measurement the first outline heading stands in.
        assert_eq!(spy_heading(&doc, &[], ScrollSnapshot::default()), Some(0));
    }

    #[test]
    fn the_scroll_spy_reveals_the_active_row_in_the_outline() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let mut markdown = String::from("# Top\n\n");
        for index in 0..40 {
            markdown.push_str(&format!("## Section {index}\n\nA paragraph.\n\n"));
        }
        state.open_synthetic("Long", &markdown);
        state.right.set_open(true);
        if let Some(tab) = state.active_tab_mut() {
            tab.view.outline_hidden = false;
        }
        let mut runtime = rune::Runtime::new(state, crate::view::view, (1100.0, 760.0), rune::Env::new(false, 2.0));
        runtime.poll = Some(|state: &mut WindowState, now, tx| state.poll(now, tx));
        for _ in 0..6 {
            runtime.frame(1.0 / 60.0);
        }
        assert!(!runtime.state.reading.heading_tops.borrow().is_empty(), "the reading view measured the headings");

        runtime.update(|_, tx| tx.scroll_to_bottom_instant(crate::reader::ARTICLE_SCROLL_KEY));
        for _ in 0..12 {
            runtime.frame(1.0 / 60.0);
        }

        let doc = runtime.state.active_reader().expect("a model");
        let last = crate::reader::toc_headings(&doc).last().map(|(index, _)| index).expect("the last heading");
        let key = format!("outline-{}", lmdv_render::heading_key(last));
        assert_eq!(
            runtime.state.reading.outline_revealed.as_deref(),
            Some(lmdv_render::heading_key(last).as_str()),
            "O-03: the spy moved the outline to the last heading"
        );
        let elements = runtime.elements();
        let row = elements.iter().find(|element| element.key.as_deref() == Some(key.as_str())).expect("the last row");
        let scroll = runtime.state.scroll_snapshot();
        assert!((scroll.offset - scroll.max).abs() < 1.0, "the page really is at the end: {scroll:?}");
        assert!(
            row.visible[3] >= row.rect[3] - 0.5 && row.visible[3] > 0.0,
            "O-03: the active row is scrolled into the pane: rect {:?}, visible {:?}",
            row.rect,
            row.visible
        );
    }

    #[test]
    fn at_the_end_the_outline_active_row_reaches_the_last_heading() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        crate::view::scene_reader_outline(&mut state);
        let mut runtime = rune::Runtime::new(state, crate::view::view, (1100.0, 760.0), rune::Env::new(false, 2.0));
        runtime.poll = Some(|state: &mut WindowState, now, tx| state.poll(now, tx));
        for _ in 0..4 {
            runtime.frame(1.0 / 60.0);
        }
        runtime.update(|_, tx| tx.scroll_to_bottom_instant(crate::reader::ARTICLE_SCROLL_KEY));
        for _ in 0..8 {
            runtime.frame(1.0 / 60.0);
        }

        let doc = runtime.state.active_reader().expect("a model");
        let last = crate::reader::toc_headings(&doc).last().map(|(index, _)| index).expect("the last heading");
        let elements = runtime.elements();
        let row = elements
            .iter()
            .find(|element| {
                element.key.as_deref() == Some(format!("outline-{}", lmdv_render::heading_key(last)).as_str())
            })
            .expect("the last row");
        assert!(row.item_selected, "OI-017: the end of the document selects the last reachable heading");
        let scroll = runtime.state.scroll_snapshot();
        assert!((scroll.offset - scroll.max).abs() < 1.0, "the page really is at the end: {scroll:?}");
    }
}
