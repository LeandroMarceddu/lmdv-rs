# Audit — Reading (R), Full-window preview (V), Outline, section line and history (O)

**Date:** 2026-11-20
**Scope:** `docs/BEHAVIOR_SPEC.md` §2.1 R-01–R-30, §2.5 V-01–V-18, §2.8 O-01–O-15
(63 rows). The table keeps the spec's own section order: R, V, O.
**Mode:** read-only source audit. No code, spec, checklist, OI, status or parity file
was edited; no test was run (targeted tests were optional and the tree was not
rebuilt). `docs/PARITY_CHECKLIST.md` is 0/342 and was **not** used as evidence.

## Method

For each row I read the exact behavior, priority and acceptance criterion, then
looked for repository evidence in this order: an app test (`crate::module::test`),
a renderer/export/io test, a scene (`--scene name`), a script
(`tests/scripts/*.json`), a fixture, or a manual note in `docs/STATUS.md` /
`docs/OPEN_ITEMS.md`. Code was read to confirm the tested path and to find
clauses no test covers. A row is **done** only when the acceptance criterion is
exercised by at least one concrete test/scene/script; a row with a real
unimplemented clause is **partial** even when the rest is tested. Accepted
differences are **substitute** with the OI/FEATURE_PARITY row. Platform-only
rows would be **n/a**; none of these 63 is Linux-inapplicable.

Test citations are module paths inside `crates/lmdv-app` unless another crate is
named; when a cell lists several tests from one module, only the first carries
the full path. Inline test modules use `crate::module::tests::test`.

---

## R — Reading and viewer

| ID | P | Status | Evidence or gap |
|---|---|---|---|
| R-01 | P0 | done | Fresh tabs are `Mode::Viewer` and the editor is built only on entry: `crate::editor_tests::each_tab_keeps_its_editor_history_across_a_switch` (new tab asserted Viewer), `WindowState::enter_editor` lazily creates `tab.view.editor` (`editor.rs:417`), `--scene reader`. |
| R-02 | P0 | partial | Defaults exist: `READER_MAX_WIDTH=760`, `READER_PAD_X=48`, `READER_PAD_BOTTOM=96`, 16 pt/1.7 (`crates/lmdv-app/src/reader.rs:33-39`, `appearance.rs:522`), padding steps `20 pt @ ≤640`, `14/24 pt @ ≤420` (`reader.rs:279`). **Gap:** top padding is `READER_PAD_TOP=40`, the spec says 32; no 1280/480 measurement test or scene exercises the column/padding. |
| R-03 | P1 | done | `crate::section_tests::the_hairline_follows_the_scroll_and_takes_no_layout`, `crate::section_tests::a_short_document_and_the_top_of_a_long_one_have_no_hairline`, `crate::section::tests::the_hairline_hides_on_a_short_document_and_follows_the_offset` (24 pt floor), `--scene reader-progress`. |
| R-04 | P1 | done | `crate::reading_meta::tests::words_are_runs_of_letters_digits_and_joined_punctuation`, `cjk_is_counted_per_character_and_breaks_the_word`, `minutes_round_up_and_never_fall_below_one`, `the_description_carries_the_full_time_and_word_count`. |
| R-05 | P1 | done | `crate::reading_meta::tests::the_description_carries_the_full_time_and_word_count`, `a_long_name_gives_way_to_the_time_in_the_tabs_width`, `a_tab_keeps_its_time_only_while_the_strip_has_room`; `crate::section_tests::editing_hides_the_time_but_the_card_keeps_it_and_a_full_strip_drops_it`. |
| R-06 | P0 | done | `crate::reading_keys_tests::five_quick_j_presses_add_up_from_the_target`, `a_held_key_repeat_jumps_instead_of_gliding`; `tests/scripts/reading-keys.json`. |
| R-07 | P1 | substitute | Accepted UX substitute: OI-161 (user, 2026-11-20) — a focused interactive control keeps Space; PageDown always pages. `crate::reading_keys_tests::space_pages_ninety_percent_and_shift_space_pages_back`, `space_still_pages_after_a_click_in_the_article`. |
| R-08 | P0 | done | `crate::reading_keys_tests::dot_visits_every_heading_including_the_last`, `comma_walks_the_previous_headings_and_stops_at_the_first`; `tests/scripts/reading-end-spy.json`. |
| R-09 | P0 | done | `crate::reading_keys_tests::a_lone_g_does_nothing_and_g_g_goes_to_the_top`, `g_goes_to_the_end`. |
| R-10 | P1 | done | `crate::reader_find_tests::n_and_shift_n_step_only_while_the_bar_is_open`; `--scene reader-find`. |
| R-11 | P1 | partial | Opens/closes and traps focus: `crate::reading_keys_tests::question_mark_toggles_the_keys_overlay_in_the_reading_view`, `crate::app::tests::question_mark_opens_and_closes_the_keys_overlay`, `tests/scripts/reading-help.json`. **Gap:** the overlay is one flat command table plus Editing and Tabs headings (`view.rs:2072-2145`); the catalog keys `keys.reading`/`keys.app` exist but are unused, so there are no Reading/App groups. |
| R-12 | P0 | done | `crate::reading::tests::a_resting_viewport_is_remembered_after_three_hundred_ms`, `a_file_position_is_queued_in_the_shared_table`, `an_opened_file_starts_from_its_remembered_position`; `--scene reader-progress`. |
| R-13 | P0 | done | The stored anchor is heading id + offset: `crate::reading::tests::a_position_below_a_heading_stores_the_heading_id`. Inserting above cannot move that heading's top, so the criterion follows; no direct insert-30-lines scenario test exists. |
| R-14 | P1 | done | `crate::reading::tests::at_top_forgets_the_anchor`. |
| R-15 | P1 | done | `crate::reading::tests::a_remembered_heading_id_resolves_to_its_rendered_key` (missing id → `None` → top), `crate::links::tests::a_fragment_resolves_legacy_and_anchor_names` (legacy ids). |
| R-16 | P0 | done | `crate::reader_find_tests::the_runtime_opens_the_bar_with_ctrl_f_and_focuses_the_query`, `entering_edit_mode_closes_the_reading_bar_and_ctrl_f_is_the_editor_panel`. |
| R-17 | P1 | partial | Selection prefill: `crate::reader_find_tests::ctrl_f_prefills_from_a_non_collapsed_article_selection`, `prefill_takes_the_first_line_trimmed_and_capped`. **Gap:** the reader block menu has only Open preview / Edit from here / Cancel (`view.rs:136-165`), so there is no context-menu “Find in document” entry using that query. (`Ctrl+Shift+F` is folder search per §6.1, so that clause is contradictory in the spec.) |
| R-18 | P0 | done | `crate::reader_find_tests::matching_is_case_insensitive_plain_and_in_document_order`, `marks_and_link_targets_are_not_searchable_but_code_is`, `the_counter_reads_current_of_total_and_zero_of_zero`; `--scene reader-find`. |
| R-19 | P1 | substitute | Accepted substitute: OI-163 (user, 2026-11-20) — block hit proxy, 40 pt horizontal reveal margin, static 1.2 s landing band. Tests: `crate::reader_find_tests::a_revealed_hit_near_the_bottom_edge_is_centred`, `a_block_clear_of_both_bands_reveals_without_centring`, `the_top_band_grows_with_the_section_line_and_not_the_notice`. |
| R-20 | P0 | partial | `file.reload` is bound to `Ctrl+R`/`F5` (`commands.rs:717-722`); reload keeps the anchor (`crate::editor_tests::a_clean_document_reloads_on_a_watch_change_and_keeps_its_anchor`). **Gap:** on a dirty buffer `request_reload` opens `Prompt::DiskChanged` (“Reload” / “Keep mine”, `editor.rs:1858-1870`), while R-20/§3.5 say an explicit reload in edit mode does nothing unless clean. No key-level F5 test. |
| R-21 | P0 | partial | `crate::editor_tests::a_clean_document_reloads_on_a_watch_change_and_keeps_its_anchor`; the watcher ignores the app's own `Access` events (`crates/lmdv-io/src/watch.rs`, OI-168). **Gap:** every `Changed` runs `start_reload` (read + parse + anchor restore); there is no text comparison, so an mtime-only `touch` is not updated silently. |
| R-22 | P0 | done | `crate::editor_tests::a_dirty_document_never_auto_reloads_it_asks`, `reload_after_the_dirty_warning_replaces_the_buffer_and_clears_the_draft`. Implemented as a modal DiskChanged prompt (Reload / Keep mine) rather than a sticky bar, but buffer safety and explicit Reload match the acceptance. |
| R-23 | P0 | partial | Rename follows file/recents/positions/history/dirty buffer: `crate::editor_tests::a_rename_follows_the_document_registry_recents_and_positions`, `a_rename_keeps_an_unsaved_buffer_dirty`; `crate::nav_tests::history_places_follow_a_renamed_file`, `a_watcher_rename_reaches_every_window`. **Gap:** `on_file_renamed` (`editor.rs:1757`) shows no “Renamed to …” notice, and tree re-highlight is not asserted. |
| R-24 | P0 | done | `crate::editor_tests::a_deleted_file_keeps_the_buffer_and_save_again_recreates_it`, `a_deleted_file_still_offers_its_draft_and_restore_leads_to_save_again`. A modal FileDeleted prompt (“Save again” / “Cancel”) replaces the sticky notice; the notice is not auto-cleared if the file reappears while the prompt is open. |
| R-25 | P1 | done | The renderer marks the article column as one selection group (`crates/lmdv-render/src/block.rs:42`), so Rune's Cmd/Ctrl+A selects the group's texts (`vendor/rune/.../select_all_tests.rs::the_only_group_is_selected`); chrome is outside it. No app-level Ctrl+A/copy assertion exists. |
| R-26 | P1 | partial | Whole-document rich copy works: `crate::share_tests::rich_copy_puts_markdown_and_html_on_the_clipboard`. **Gap:** a selection copies as plain text; no Word-friendly HTML range is built. FEATURE_PARITY line 166 marks “Rich copy of a selection” as **build**; OI-096 is open. |
| R-27 | P0 | done | `crate::preview_nav_tests::a_viewer_double_click_only_selects_and_never_edits` (mode stays Viewer). |
| R-28 | P1 | missing | No drag-to-scroll handler: `article-scroll` wires `on_key`, `on_wheel` and magnify only (`reader.rs:460-478`); Rune's middle-click **autoscroll** is a different gesture (and the row asks for autoscroll suppression). No middle-click link open in the reader. PARITY_CHECKLIST R-28 unchecked, no OI. |
| R-29 | P1 | partial | Notices exist but are transient: `NOTICE_SECONDS=4` (`window.rs:31`), `crate::window::tests::a_notice_expires_on_the_virtual_clock`. **Gap:** there is no sticky notice class, and opening another document does not clear `ui.notice` (it is cleared only by a scene reset, `window.rs:760`, or expiry), so a previous document's notice can survive the open for its remaining lifetime. |
| R-30 | P0 | substitute | Print is the X-04 substitute (system dialog → PDF); exports are independent of reader prefs by construction: fixed 16 px sans/760 px column (`crates/lmdv-export/src/html.rs:6`), `crates/lmdv-export/tests/pdf_export.rs::output_is_deterministic_and_driven_only_by_print_options`. No “print preview” surface exists. |

## V — Full-window preview

| ID | P | Status | Evidence or gap |
|---|---|---|---|
| V-01 | P0 | done | `crate::lightbox_tests::an_image_opens_with_its_name_alt_and_a_fitted_stage`, `a_broken_image_never_opens`, `a_diagram_opens_from_its_mermaid_source`, `enter_and_space_open_a_focused_media_element`, `the_block_menu_offers_open_preview_for_media`, `an_inline_picture_opens_with_the_same_preview_from_its_block`; table expand bar: `crate::tables_tests::every_table_gets_a_copy_and_preview_bar_and_none_without_a_table`, `a_table_with_no_rows_does_not_open_a_preview`; `--scene lightbox`, `tests/scripts/lightbox.json`. |
| V-02 | P2 | partial | `crate::lightbox_tests::media_is_a_focusable_button_and_its_hint_stays_out_of_the_tab_order`, `the_hint_sits_at_the_media_top_right_corner`. **Gap:** inline (text-span) pictures have no hover hint (OI-182, open); hiding near the bars relies on the article clip and the bars' draw order, not an explicit hide test. |
| V-03 | P0 | done | `crate::lightbox_tests::the_buttons_and_keys_step_clamp_fit_width_and_actual`, `arrow_keys_pan_by_40_or_160_points_and_clamp`, `a_click_opens_focuses_the_viewport_and_escape_returns_to_the_media`. |
| V-04 | P1 | done | `crate::lightbox_tests::the_wheel_zooms_about_the_pointer_and_never_scrolls_the_article` (plain and Ctrl+wheel, pointer anchor, article unchanged). |
| V-05 | P1 | done | `crate::lightbox_tests::a_drag_pans_a_zoomed_image_and_does_not_close` (release outside still pans, no close). |
| V-06 | P1 | done | `crate::lightbox_tests::the_buttons_and_keys_step_clamp_fit_width_and_actual` (1.25 steps, 8× ceiling, `min(fit, 0.2)` floor), `an_image_opens...` (raster fit at 1:1), `a_diagram_opens_from_its_mermaid_source` (vector fit up to 4×); `settle()` re-fits Fit/Width every frame (`lightbox.rs:366`). |
| V-07 | P1 | done | `crate::lightbox_tests::an_image_opens_with_its_name_alt_and_a_fitted_stage` asserts dialog role/label “Preview”, viewport group + instruction label, stage `img` + caption; zoom level uses `live_region(LiveRegion::Polite)` (`lightbox.rs:1321`). |
| V-08 | P1 | partial | Copy 2×/4× and Save PNG… work: `the_copy_menu_lists_paste_sizes_and_save_for_a_picture`, `copy_as_image_puts_the_picture_png_on_the_clipboard`, `save_as_png_writes_the_picture_through_the_worker`, `escape_closes_the_copy_menu_before_the_preview`. **Gap:** no SVG entries and no diagram copy/save; `a_diagram_has_no_copy_menu_until_it_can_be_rasterized` pins the absence. Needs the rune-mermaid serializer, OI-181. Tables use their own copy action, not this menu. |
| V-09 | P1 | partial | PNG copy/save and non-PNG raster conversion work: `a_picture_that_is_not_png_is_converted_for_copy_and_save`, `copy_as_image_puts_the_picture_png_on_the_clipboard` (natural size). **Gap:** diagrams are not rasterized at a requested scale (no scene serializer, OI-181). |
| V-10 | P1 | done | `crate::lightbox_tests::a_plain_click_on_empty_stage_or_backdrop_closes_and_a_drag_does_not` (press/release target + drag), `a_tab_switch_closes_the_preview_and_coming_back_keeps_it_closed`; table backdrop: `crate::tables_tests::clicking_outside_the_panel_closes_the_preview` (scrim closes on press, so a drag from a cell cannot start there). Table selection-release is not directly tested. |
| V-11 | P1 | done | `crate::tables_tests::the_first_column_sticks_when_the_preview_scrolls_sideways`, `the_header_sticks_to_the_top_while_the_rows_scroll`, `a_wide_preview_freezes_its_first_column_only_when_it_is_narrow` (35 % rule, overflow only); `--scene table-preview`. |
| V-12 | P1 | done | `crate::tables_tests::the_preview_zooms_in_steps_and_by_ctrl_wheel`, `the_preview_zoom_keys_and_buttons_work_through_the_window`, `the_plus_and_minus_keys_zoom_the_open_preview`, `a_resize_refits_the_columns_without_reopening`; 0.5–2 in `TablePreview::set_zoom`. |
| V-13 | P1 | done | `crate::tables_tests::the_filter_keeps_rows_with_every_word_accent_and_case_folded`, `filter_matches_map_back_to_the_accented_characters`, `the_filter_applies_after_the_debounce`, `a_filter_with_no_matches_shows_the_message_and_the_count`, `widths_stay_locked_while_the_filter_changes`. |
| V-14 | P1 | done | `crate::tables_tests::escape_clears_the_filter_before_it_closes_the_preview`; Return/Down from the filter refocus the grid in `table_filter_key` (`tables.rs:545-556`), Esc is handled from the whole modal. |
| V-15 | P1 | done | `crate::tables_tests::a_header_click_cycles_ascending_descending_and_original`, `numeric_columns_strip_currency_units_and_vote_on_the_decimal_mark` (negative, grouped, empty last), `date_columns_sort_chronologically_and_infer_the_order`, `text_sorts_naturally_and_numbers_come_before_words`, `a_stable_sort_keeps_equal_rows_in_document_order`, `a_single_row_or_headless_table_does_not_sort`. |
| V-16 | P1 | done | `crate::tables_tests::a_preview_copies_only_its_shown_rows_in_their_shown_order`, `the_bar_copies_the_whole_table_as_tsv`; `crates/lmdv-export/src/tsv.rs` unit tests (tabs/newlines collapsed, no trailing newline). |
| V-17 | P1 | done | `crate::tables_tests::a_link_in_the_preview_closes_it_and_follows_the_destination`, `clicking_a_link_in_a_preview_cell_follows_it_and_closes_the_preview`; cells carry the document's link, and TSV is text-only (no ids/source lines). Task cells are plain text, so no checkbox exists to activate. |
| V-18 | P1 | done | `crate::tables_tests::widths_fit_the_view_and_cap_the_growth` (`plan_widths`: fit up to 1.5×, below minimum → natural width and scroll), `a_wide_preview_freezes_its_first_column_only_when_it_is_narrow` (opens at 100 % over minimum). |

## O — Outline, section line and history

| ID | P | Status | Evidence or gap |
|---|---|---|---|
| O-01 | P0 | done | `crate::reader::tests::the_outline_lists_levels_one_to_four` (h1–h4 only; h5/h6-only doc has no outline), `footnote_definition_headings_are_not_in_the_outline`; `crate::outline::tests::the_outline_follows_headings_and_the_user_choice`; `--scene reader-outline`. |
| O-02 | P0 | done | `crate::reading_keys_tests::an_outline_jump_glides_and_still_records_the_place_left`, `the_outline_keeps_the_heading_until_the_reader_scrolls`; landing flash: `crate::section_tests::a_heading_jump_holds_the_line_and_flashes_the_heading_once_it_lands`. The explicit “peek closes” clause is not asserted; rune-kit ends a peek on pointer-leave/window blur (`pane.rs:153-158`). |
| O-03 | P0 | partial | Activation line and last-viewport slide: `crate::reader::tests::the_activation_line_slides_through_the_last_viewport`, `crate::outline::tests::at_the_end_the_outline_active_row_reaches_the_last_heading`; `--scene reader-outline`. **Gap:** the spy only changes `.selected` (`outline.rs:174-240`); `scroll_into_view("outline-…")` is issued on a jump (`outline.rs:150`), never when the active row changes, so the active entry can leave the pane. |
| O-04 | P1 | done | `crate::reading_keys_tests::the_outline_keeps_the_heading_until_the_reader_scrolls`, `a_wheel_releases_the_heading_pin_and_starts_a_new_target`; `crate::reading_keys::tests::a_pin_releases_on_a_hand_scroll_or_after_the_glide_settles`, `a_reparse_drops_the_pin`. |
| O-05 | P1 | partial | Filter appears at ≥8 headings and filters per tab: `crate::outline::tests::the_filter_is_per_tab`, `crate::view::tests::the_outline_scene_lists_headings_and_its_filter` (15-heading scene), threshold `OUTLINE_FILTER_MIN_HEADINGS=8` (`outline.rs:17,197`). **Gaps:** no 7-heading negative test; Enter always jumps to the first visible (`first_visible_heading_key`, `outline.rs:102,208`), with no keyboard cursor or ↑/↓ navigation from the filter field (the kit sidebar's arrows only work after Tab into the list). |
| O-06 | P0 | done | `crate::editor_tests::edit_mode_hides_the_reading_panes_and_restores_them` (both panes, per-tab restoration); `crate::outline::tests::the_outline_follows_headings_and_the_user_choice` (no headings / manual close); peek overlay from the kit (`view.rs:84-86`). |
| O-07 | P1 | done | `crate::section_tests::the_section_line_names_the_heading_that_scrolled_out`, `the_line_stays_away_at_the_top_while_the_outline_shows_and_while_editing`; enabled only in viewer, outline closed and not peeking, with headings (`section.rs:613-616`); `--scene reader-section`. |
| O-08 | P1 | done | `crate::section::tests::the_path_leaves_out_a_single_first_h1`, `skipped_levels_hang_from_the_nearest_smaller_heading`, `several_h1s_stay_in_the_path`, `h5_h6_and_footnote_definitions_never_form_the_path`; image-alt/“§” fallback: `crate::reading_meta::tests::a_heading_line_collapses_whitespace_and_falls_back_to_the_section_sign`. |
| O-09 | P1 | substitute | Suppression implemented with the accepted approximation: OI-141 (user, 2026-11-20) — poll-time focus tracking, table top band instead of measured header height. Tests: `crate::section::tests::a_table_header_holding_the_top_hides_the_line`, `a_table_about_to_stick_also_hides_the_line`, `fast_scrolling_and_its_settle_hide_the_line`, `a_heading_jump_holds_the_line_until_it_lands`, `the_focus_ring_band_uses_the_line_and_the_ring`. |
| O-10 | P2 | done | `crate::section_tests::clicking_a_part_jumps_like_the_outline_and_the_second_press_is_swallowed`; 500 ms guard and shield: `crate::section::tests::a_second_press_is_swallowed_for_500_ms`; the line is `a11y_hidden` and its parts are spans (`section.rs:348-390`). |
| O-11 | P2 | partial | Jumps exist but the reserve rule does not: `--line` lands with a fixed 24 pt inset (`reading.rs:457`), footnotes/headings with `JUMP_INSET=12` (`nav.rs:37`, `reading.rs:208-215`); nothing checks `SECTION_LINE_HEIGHT` (30 pt) or the table-header exception. No test for a footnote landing under a visible line. |
| O-12 | P0 | done | `crate::nav_tests::opening_another_document_records_the_place_left`, `opening_the_same_document_records_nothing`, `a_local_link_records_the_place_left_and_opens_the_target`, `a_file_link_with_a_section_lands_on_that_heading`, `a_same_document_anchor_link_jumps_and_records_the_place_left`, `a_reload_records_nothing`, `a_jump_that_lands_within_eight_points_records_nothing`; `tests/scripts/nav-history.json`. |
| O-13 | P0 | done | `crate::nav_tests::the_back_list_is_capped_at_fifty_places`, `consecutive_places_under_four_points_are_suppressed`, `a_new_place_clears_forward`, `a_missing_file_is_dropped_with_a_not_found_notice`, `a_pasted_place_without_its_document_is_dropped_silently`, `an_overtaken_move_is_not_committed`. |
| O-14 | P0 | partial | Alt+←/→, mouse side buttons, tab menu and palette work: `crate::nav_tests::alt_left_and_right_dispatch_the_navigation_commands`, `the_mouse_side_buttons_navigate`, `the_tab_menu_starts_with_back_and_forward`, `the_palette_offers_back_and_forward_and_closes_on_the_move`; `tests/scripts/nav-buttons.json`. **Gap:** `BrowserBack`/`BrowserForward` keys are not exposed by pinned Rune, and Alt+arrows are consumed by focused fields — accepted gap OI-150 (no new OI needed). |
| O-15 | P1 | substitute | Locale changes apply live with no window reload (`settings.rs:219-228`), so the `sessionStorage`/`nav-history` mechanism is replaced by the session file, which stores each tab's lists: `crate::nav_tests::a_session_round_trip_keeps_each_tabs_history`, `a_tab_switch_keeps_each_tabs_history`; P4-02d / OI-121 (other per-tab state still not stored). |

---

## Summary

| Status | R | V | O | Total |
|---|---:|---:|---:|---:|
| done | 18 | 15 | 9 | **42** |
| substitute | 3 | 0 | 2 | **5** |
| partial | 8 | 3 | 4 | **15** |
| missing | 1 | 0 | 0 | **1** |
| **Total** | **30** | **18** | **15** | **63** |

Substitutes, all already recorded in the register: R-07 (OI-161), R-19 (OI-163),
R-30 (X-04) and O-09 (OI-141). O-15 uses the session-file extension
(P4-02d/ADR-021) instead of `sessionStorage`; OI-121 still covers other per-tab
state.

### Partial and missing rows

Each bullet gives the proposed OI title and the decision the orchestrator should
record. Existing OIs are marked so no duplicate row is added.

**R — Reading**

- **R-02 — Reader default top padding is 40 pt, not the specified 32 pt**; no
  1280/480 measurement test. *Decision:* change `READER_PAD_TOP` to 32 or record
  the 40 pt step as a deliberate substitute (needs the user's acceptance).
- **R-11 — Keys overlay has no Reading/App group headings** (flat command table
  plus Editing/Tabs; `keys.reading`/`keys.app` catalogs already exist).
  *Decision:* add the two headings and keep the command list as the body.
- **R-17 — Reader context menu has no “Find in document”** (selection prefill
  works only through Ctrl+F). *Decision:* add the menu item reusing
  `open_reader_find` and its selection prefill.
- **R-20 — F5 on a dirty buffer opens the DiskChanged prompt instead of doing
  nothing** (R-20/§3.5). *Decision:* do nothing when dirty, or record the prompt
  as a substitute; add an F5 key-path test either way.
- **R-21 — An mtime-only touch triggers a full reload/parse/anchor restore**
  instead of updating the baseline silently. *Decision:* compare the read text
  with the saved text before replacing; skip the parse/restore when equal.
- **R-23 — External rename shows no “Renamed to …” notice**, and tree
  re-highlight is unasserted. *Decision:* add the localized notice after the
  rekey and assert the tree refresh.
- **R-26 — Selection copy is plain text; no rich HTML range** (whole-document
  rich copy exists). *Decision:* already FEATURE_PARITY “build” and **OI-096**;
  no new row, keep the selection serializer on the P7 list.
- **R-28 — No drag-to-scroll and no middle-click link open** (middle-click
  currently triggers Rune autoscroll). *Decision:* implement the 3 pt drag
  threshold, middle/left-on-margin start, and middle-click link open; otherwise
  file an accepted gap (this is the only `missing` row in the three sections).
- **R-29 — Notices are 4 s transient, not sticky, and opening another document
  does not clear the previous notice.** *Decision:* add a sticky notice class
  and clear it on document change/save/reload, or record the transient model as
  a substitute.

**V — Full-window preview**

- **V-02 — Inline picture hover hint is unwired** (block pictures have it).
  *Decision:* already **OI-182**; no new row.
- **V-08 — Diagram preview has no SVG copy/save (and no PNG save)**, and tables
  use the grid's own copy action. *Decision:* already **OI-181** (with OI-140's
  residual); no new row.
- **V-09 — Diagram rasterization at a requested scale is absent** (raster
  non-PNG conversion is done). *Decision:* same **OI-181**; no new row.

**O — Outline, section line and history**

- **O-03 — Scroll-spy never scrolls the active outline row into view.**
  *Decision:* issue `scroll_into_view("outline-{key}")` when the active heading
  changes (not only on a click).
- **O-05 — Outline filter has no keyboard cursor/↑↓ and Enter always jumps to
  the first visible**; also no 7-heading negative test. *Decision:* add a
  filtered cursor with ↑/↓/Enter/Esc, and a 7-vs-10-heading visibility test.
- **O-11 — Non-heading jumps ignore the section line** (fixed 12/24 pt inset,
  no 30 pt reserve, no table-header exception). *Decision:* compute the line
  visibility at jump time and add the 30 pt reserve for `--line`, footnote and
  anchor targets.
- **O-14 — BrowserBack/BrowserForward keys absent, Alt+arrows eaten by focused
  fields.** *Decision:* already accepted **OI-150**; no new row.
