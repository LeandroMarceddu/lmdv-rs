# Audit — Tabs/windows, file browser, command palette, window chrome, drag and drop

**Scope:** `docs/BEHAVIOR_SPEC.md` §2.2 T (35 rows), §2.7 F (22), §2.9 C (13), §2.12 W (10), §2.13 D (8) — 88 rows.
**Checkout:** audit run 2026-11-20 against the working tree (`docs/STATUS.md` last updated 2026-11-18, Rune `cca5df6`).

## Method

- Read every row's behavior, priority and acceptance criterion, then looked for evidence that the behavior exists in this checkout.
- Evidence accepted: a named test (file + test name), a scene key, a script under `tests/scripts/`, a fixture, or a note in `docs/STATUS.md` / `docs/OPEN_ITEMS.md`. Source was opened only to confirm or qualify a claim (for example, whether a modifier is threaded through, or whether a field exists).
- `done` requires evidence that exercises the row's behavior; a test for a neighboring behavior does not count.
- `substitute` means the behavior is delivered by another mechanism or is an accepted gap, cited to `docs/FEATURE_PARITY.md` §4, an ADR or an OI row.
- `partial` means the behavior is largely present but a named clause is missing or untested; the gap is named in the cell.
- `missing` means no implementation or no evidence of one. `n/a` is platform-specific and says why.
- Tests were read by name, not executed. No workspace suite was run (AGENTS §4); no code, spec, checklist, `OPEN_ITEMS`, `STATUS` or `FEATURE_PARITY` file was changed. This file is the only change.
- Table order follows the spec's own section order: T, F, C, W, D.

| ID | P | Status | Evidence or gap |
|---|---|---|---|
| T-01 | P0 | partial | Strip is drawn in its own 40 pt band under the toolbar (`view.rs:489-649`), not in the toolbar as specified; OI-003 is still `scheduled`. The app never passes `viewport_width`, so the kit keeps tabs at its 224 pt maximum instead of sizing each to its name down to a minimum (`tab_list.rs:27-38`). Scene `tabs` shows the band. |
| T-02 | P0 | partial | File tabs use the stem and the full name as label (`window.rs:188-216`; `window::tests::strip_titles_drop_the_extension`); pasted titles are covered by `actions_tests::pasted_titles_follow_the_first_level_one_heading_then_the_first_line` and its siblings. Icons are home/document/pencil (`view.rs:568-573`); there is no clipboard glyph in `icons.rs`, so a pasted tab shows the document icon. |
| T-03 | P0 | partial | Dirty replaces the close slot with `TabItemStatus::Attention` (`view.rs:563-567`). After save the flash is toolbar text “Saved” (`view.rs:483-485`; `editor_tests::the_saved_check_expires_after_its_seconds`), not a check in the tab, and it is not a live-region announcement. |
| T-04 | P0 | done | `window::tests::new_tab_appends_at_the_end_and_brings_it_forward`; `window::tests::the_tab_menus_new_tab_inserts_after_its_tab`; palette `file.new-tab` (`palette_tests::commands_ignore_the_modifiers_they_were_chosen_with`); `+` button at `view.rs:585-608`. |
| T-05 | P0 | partial | Neighbor choice `window::tests::closing_the_middle_tab_brings_the_right_neighbour_forward`; last document `window::tests::closing_the_last_document_leaves_a_start_page`; dirty prompt `editor_tests::closing_a_dirty_tab_asks_and_dont_save_discards_the_draft`; middle click closes (`vendor/rune/crates/rune-kit/src/shell/tab_strip/tab.rs:495`). No Delete/Backspace handler for a focused tab exists. |
| T-06 | P0 | done | `window::tests::closing_a_lone_start_page_asks_the_window_to_close`; `window.rs:1024-1136`. |
| T-07 | P0 | done | `actions_tests::reopen_restores_a_file_tab_at_its_index_and_place`, `the_closed_stack_stops_at_the_original_cap`, `reopen_restores_pasted_text_with_its_paste_id_and_place`, `reopen_reuses_a_document_another_window_still_shows`. |
| T-08 | P0 | done | `tab_keys_tests::pgdn_pgup_and_ctrl_tab_wrap_around_the_strip`; commands `tab.next`/`tab.previous` carry both chords. |
| T-09 | P1 | substitute | `tab_keys_tests::the_digits_are_read_from_the_character_or_the_physical_key`, `numbered_digits_select_a_tab_and_nine_is_the_last`. OI-166 (accepted): no `keyCode` fallback when a backend names neither a Command digit nor a physical key. |
| T-10 | P1 | done | `tab_keys_tests::shift_page_keys_move_the_front_tab_and_clamp_at_the_ends`. |
| T-11 | P0 | done | `window::tests::opening_a_file_twice_activates_its_existing_tab`; a background open keeps the front tab (`browser_tests::the_menus_open_rows_use_the_background_tab_and_new_window_paths`). The specified pulse of the existing tab is absent and is counted under T-15. |
| T-12 | P0 | partial | Tree and search hits honor Ctrl/Ctrl+Shift/Shift (`browser.rs:744-790`; `browser_tests::the_menus_open_rows_use_the_background_tab_and_new_window_paths`). Recents open on a plain press only (`view.rs:782`), and `links.rs:136 open_link` takes no modifiers, so Ctrl/Shift on a document link does nothing. |
| T-13 | P0 | partial | Foreground replacement is tested (`window::tests::a_document_replaces_the_front_start_page`). `attach_document_at` (`window.rs:847-890`) replaces a front start tab without a background flag, and `palette_tests::a_middle_click_on_a_file_row_opens_it_behind` has to put a document in front first because a background open would otherwise consume the start page. |
| T-14 | P1 | partial | Background opens are switched back but appended at the end (`window.rs:781-817`); there is no insertion after the opening tab nor a queue reset on activation. The drop-index clause is absent with D-02. |
| T-15 | P2 | missing | No pulse or flash exists anywhere in `lmdv-app`; the kit exposes `Unread`/`Attention` marks, and `view.rs` only maps dirty to Attention. |
| T-16 | P1 | done | `tab_card_tests::moving_from_tab_to_tab_keeps_the_card_up_at_once`, `the_card_names_the_file_its_folder_and_the_time_or_unsaved_changes`, `pasted_and_start_tabs_say_what_they_are`; scene `tab-card`; script `tests/scripts/tab-card.json`. |
| T-17 | P1 | substitute | `tab_list` tests `the_button_appears_only_when_the_tabs_overflow`, `choosing_a_tab_from_the_list_activates_it_and_closes_the_list`, `file_tabs_show_their_folder_and_new_tab_appends_a_start_page`; scene `tab-overflow`. OI-158 fixed; OI-166 accepted for the plain vertical wheel and the one-frame delay. |
| T-18 | P1 | done | Menu at `view.rs:167-330`; `actions_tests::the_tab_menu_copy_and_reveal_act_on_that_tab`, `close_other_tabs_keeps_only_the_chosen_tab`, `tab_menu_opens_on_a_background_tab`; scenes `tab-menu`, `tab-menu-file`. |
| T-19 | P0 | partial | Reorder is the kit's (`vendor/rune/crates/rune-kit/src/shell/tab_strip/drag.rs` SLOP is 4 pt, spec says 5) and `view.rs:635-637` calls `reorder_tab`; an exporting document refuses the drop (`window.rs:1150-1163`) rather than blocking the gesture. No test drives a reorder (the kit tests cover tear-out only), and Esc cancelling a reorder is untested (D-08). |
| T-20 | P1 | substitute | `tear_out_tests::a_release_over_another_window_moves_the_tab_there`, `a_drop_outside_every_window_opens_one_at_the_pointer_and_adopts_the_tab`; kit detach distance is 36 pt. Release-only: the new window does not follow the pointer until mouse-up (OI-125, STATUS P4-02c). |
| T-21 | P1 | substitute | Kit refuses a lone tab (`tear_out_tests::a_lone_tab_never_tears_out`); the tab menu's “Move to a new window” is the substitute; OI-126 accepted (no Rune `drag_window`). |
| T-22 | P1 | substitute | `transfer_tests::a_clean_tab_moves_with_its_view_state_and_the_watch`, `a_clean_duplicate_merges_into_the_existing_tab`, `a_dirty_duplicate_is_refused_and_returned_with_text_and_lease_intact`, `hover_opens_and_clears_the_incoming_slot`. STATUS P4-02b: in-process protocol, no 15 s timeout; a vanished target restores at once. |
| T-23 | P1 | done | `actions_tests::close_other_tabs_keeps_only_the_chosen_tab`, `close_tabs_to_the_right_closes_from_the_right`, `a_dirty_tab_in_the_sequence_asks_and_cancel_stops_the_run`. |
| T-24 | P0 | done | `editor_tests::a_clean_pasted_document_still_asks_on_window_close`, `save_all_waits_for_a_pasted_save_as_before_closing`, `the_last_view_of_a_dirty_document_asks_and_cancel_keeps_everything`; localized list in `window.rs:1266-1286`; Save all / Don't save / Cancel in `view.rs`. |
| T-25 | P1 | done | `editor_tests::a_second_close_request_never_stacks_a_second_question`; `shared.set_window_closing`; STATUS P4-02a. |
| T-26 | P0 | partial | `TabView` keeps anchor, find, reader find, nav, fragment, outline filter, parsed model, editor session, find bar, slash menu, details and dirty (`window.rs:78-140`); `window::tests::switching_tabs_keeps_each_scroll_anchor`. But the sticky notice is one per window (`UiState.notice`), the tree root is per window/shared (OI-129), and no per-tab scheme is stored. |
| T-27 | P1 | substitute | No rendered-DOM fragments exist: the native view is rebuilt per frame and every tab keeps its parsed model (`TabView.reader`), with no 6-tab budget. Memory is model-sized rather than DOM-sized; the specified eviction/re-render behavior has no equivalent. |
| T-28 | P1 | substitute | A language change applies live (`settings.rs:219-227 apply_locale`; “there is no reload”), so no `sessionStorage` transfer exists and the language switch cannot discard tab state. OI-176: the live switch is skipped under `cfg(test)`, so no test exercises it. |
| T-29 | P1 | partial | Order in `app.rs:402-470` (palette, Help, Share, table preview, lightbox before the command table) matches the intent, but no test pins the precedence, and only a tab key cancels a drag (`tab_keys.rs:93`), so Ctrl+W does not cancel a tab drag first. |
| T-30 | P0 | done | `commands.rs` test `close_window_goes_through_the_dirty_guard`; `guard_close_window` refuses a second question while one is open; chord is Ctrl+Shift+W. |
| T-31 | P1 | partial | `shared.rs cascade_position` and `windows::tests::a_new_window_cascades_32_points_from_the_focused_window`. No 3 s chain rule, and no maximized/minimized inheritance: `request_new_window` always cascades from the current window. |
| T-32 | P1 | substitute | OI-002 accepted: Rune's native file dialog is single-select. |
| T-33 | P0 | done | `window::tests::two_windows_share_one_document_one_lease_and_one_registry`; `windows::tests::an_open_document_selects_the_most_recently_focused_holder`, `a_file_prefers_the_first_idle_normal_window_in_focus_order`. |
| T-34 | P1 | missing | No width breakpoints exist: the toolbar is fixed (`view.rs:470-486`) and neither `lmdv-app` nor the kit toolbar has 720/640/560 logic. |
| T-35 | P2 | done | `reading_meta` tests `a_tab_keeps_its_time_only_while_the_strip_has_room`, `a_long_name_gives_way_to_the_time_in_the_tabs_width`; `tab_card_tests::a_drag_hides_the_card_and_the_time_while_a_pointer_close_freezes_the_widths`; OI-142 fixed. |
| F-01 | P0 | done | `browser_tests::opening_outside_the_root_reroots_and_opening_inside_keeps_it`; `window.rs follow_document_root`. |
| F-02 | P0 | done | `browser_tests::up_moves_to_the_parent_and_a_filesystem_root_cannot_go_up`. |
| F-03 | P0 | done | `lmdv_io::tree::tests::a_listing_keeps_markdown_only_and_hides_empty_or_skipped_folders` asserts four components listed and five excluded; `the_listing_caps_markdown_files_at_two_thousand`. |
| F-04 | P0 | done | Same listing test (`.hidden`, `node_modules`, `Target`, `dist`, folders without Markdown); `extension_and_skip_rules_are_explicit`. |
| F-05 | P0 | done | Same listing test (`image.png`, `notes.txt` absent, `README.mkd` present); `extension_and_skip_rules_are_explicit`. |
| F-06 | P1 | done | Same listing test: directories before files, lowercase-name order (`Zdir` after `nested`). |
| F-07 | P0 | done | `browser_tests::collapsed_folders_survive_rebuilds_and_open_when_their_file_is_shown`, `startup_restores_the_root_and_collapsed_folders`. |
| F-08 | P0 | done | `browser_tests::a_watch_report_relists_only_the_watched_root`; `lmdv_io::watch::tests::the_tree_watch_ignores_content_writes`; `TREE_DEBOUNCE` 400 ms. |
| F-09 | P1 | done | `browser_tests::a_rebuild_keeps_the_focused_row_or_its_folder`; `an_unchanged_listing_does_not_rebuild_or_move_focus`. |
| F-10 | P1 | substitute | Roving focus via `active_descendant` (`browser.rs:1577-1583, 1964-2074`); OI-131 accepted: one roving stop with `aria-activedescendant`, rows are not individually focusable. |
| F-11 | P1 | partial | Traversal exists in `browser_key` (`browser.rs:1543-1650`: arrows, Home/End, Enter/Space with modifiers, Left/Right open/close), but no test covers plain arrow/Home/End/Enter/Space on the tree; existing key tests cover the Ctrl resize chords and menus. |
| F-12 | P1 | done | `browser_tests::a_files_menu_lists_open_reveal_copy_and_a_relative_link`, `a_folders_menu_has_no_open_rows`, `a_markdown_link_is_relative_to_the_document_and_percent_encoded`, `shift_f10_opens_the_focused_rows_menu_under_it_and_escape_returns_focus`. |
| F-13 | P0 | done | `browser_tests::the_folder_search_shortcut_is_ctrl_shift_f_and_shows_the_pane`, `the_search_prefill_is_the_first_trimmed_selection_line`, `search_is_debounced_and_an_empty_query_closes_the_results`. |
| F-14 | P0 | done | `lmdv_io::tree::tests::search_reports_names_lines_and_snippets`, `search_stops_at_two_hundred_hits`, `a_long_preview_is_centred_on_the_match_and_bounded`. |
| F-15 | P1 | done | `browser_tests::search_results_render_with_no_matches_and_with_grouped_rows`. |
| F-16 | P0 | done | `browser_tests::a_search_hit_opens_the_reading_find_bar_with_its_query`, `search_results_group_by_file_and_open_at_the_line`; `open_browser_file_with_query` carries the query. |
| F-17 | P1 | done | `browser_search_key` clears and blurs on Esc (`browser.rs:1520-1523`); `finish_search` selects the first result so Enter opens it; `search_is_debounced_and_an_empty_query_closes_the_results`. |
| F-18 | P0 | done | `browser_tests::ctrl_b_toggles_and_remembers_and_a_keyboard_show_focuses_the_tree`, `a_hidden_pane_is_never_auto_opened_and_a_manual_show_overrides_the_fold`. |
| F-19 | P0 | done | `browser_tests::the_outline_folds_first_then_the_files_and_both_return_when_widened`, `a_folded_pane_opens_as_a_peek_over_the_text`. |
| F-20 | P1 | substitute | `browser_tests::ctrl_arrows_resize_the_tree_by_16_and_64_and_persist`, `the_stored_hidden_choice_and_width_come_back_with_the_root`; OI-179 accepted: the kit grip is `focusable(false)`, so Ctrl+arrows on the tree plus palette commands replace plain arrows on the grip. |
| F-21 | P1 | done | `browser_tests::ctrl_wheel_over_the_tree_changes_only_the_panel_size`, `ctrl_wheel_over_the_article_steps_the_reader_size_only`. |
| F-22 | P2 | done | `browser_tests::a_failed_folder_link_with_no_previous_root_returns_to_the_empty_state`; `lmdv_io::tree::tests::an_empty_or_missing_folder_is_reported_honestly`. |
| C-01 | P0 | partial | `palette_tests::ctrl_k_opens_and_closes_the_palette_while_reading`, `the_editor_keeps_ctrl_k_for_its_link_command`, `ctrl_shift_p_opens_it_anywhere`, `a_held_opening_key_does_not_close_it`, `alt_meta_and_shift_chords_do_not_open_it`. No IME-composition guard exists (`rune::KeyEvent` exposes no composing state), so the “not during composition” clause is unmet. |
| C-02 | P0 | done | `palette_tests::the_start_list_shows_commands_tabs_and_recent_files_with_their_caps`, `typing_caps_every_group`, `the_front_tab_and_open_files_are_left_out`; caps match the spec (`palette.rs:74-88`). |
| C-03 | P0 | done | `palette_tests::accents_and_letter_order_find_a_heading`, `the_english_words_find_a_command`; `fuzzy::tests::accents_and_case_are_folded`, `every_word_must_be_found_in_order`, `word_starts_beat_letters_in_the_middle`. |
| C-04 | P1 | done | `palette_tests::greater_than_lists_only_commands_and_all_of_them`, `hash_lists_the_outline_in_order_and_caps_it`, `hash_with_words_ranks_headings_only`. |
| C-05 | P1 | done | `palette_tests::a_command_run_from_the_palette_is_remembered_first`, `a_better_score_beats_a_recent_command`, `the_recent_list_stays_at_eight_and_never_repeats`. |
| C-06 | P0 | done | `palette_tests::enter_opens_a_file_here`, `ctrl_enter_leaves_the_tab_behind_in_front`, `ctrl_shift_enter_opens_the_file_in_front`, `shift_enter_asks_for_a_new_window`, `a_middle_click_on_a_file_row_opens_it_behind`. |
| C-07 | P1 | done | `palette_tests::the_search_rows_come_after_the_normal_groups`, `the_search_rows_need_a_document_and_a_folder`, `find_in_the_document_opens_the_reading_bar_with_the_query`, `search_the_folder_starts_the_pane_search`. |
| C-08 | P1 | done | `palette_tests::up_and_down_wrap`, `page_down_moves_to_the_next_group_and_page_up_back`, `tab_is_swallowed`, `the_pointer_moves_the_highlight_only_when_it_moves`. |
| C-09 | P1 | done | `palette_tests::shortcuts_do_not_reach_the_window_behind`, `the_text_box_keeps_its_editing_keys`, `f1_closes_the_palette_and_opens_help`. |
| C-10 | P1 | done | `palette_tests::focus_returns_to_where_it_was`, `closing_returns_the_keyboard_to_the_control_it_left`, `closing_restores_the_page_selection`, `escape_and_the_backdrop_close_it`. Right-click suppression on rows is not asserted, but no app context menu is wired to palette rows. |
| C-11 | P0 | done | 101 command specs in `commands.rs`, including `view.pin`, five themes, reader size, reading settings, help, Share/PDF/HTML/DOCX, the H1 break toggle and the editor commands; `actions_tests::the_palette_lists_the_new_commands`, `the_start_group_leads_with_open_and_paste`; `palette_tests::only_commands_that_can_run_now_are_listed`. |
| C-12 | P1 | substitute | `palette_tests::the_input_and_list_carry_the_combobox_roles`, `the_count_is_announced_after_the_typing_pauses`; OI-148: Rune has no `aria-autocomplete`/`aria-controls` state. |
| C-13 | P1 | substitute | `palette_tests::a_tab_switch_or_a_document_change_closes_it`, `another_modal_closes_it`; `poll_palette` closes on prompt/help/Share/table/lightbox/block-menu/tab-menu (`palette.rs:695-711`). The full-window scrim (`palette_ui.rs:49-78`) makes a tab drag unreachable while the palette is open, so no drag trigger is needed; static linkage makes the module-load-failure clause n/a. |
| W-01 | P0 | substitute | Native Linux frame (ADR-007; FEATURE_PARITY §4 W-01/02/03/05/06/08). The app toolbar is not a window-drag region (there is no `data-tauri-drag-region` equivalent), so dragging or double-clicking the toolbar does not move/maximize the window; the native frame owns both. |
| W-02 | P0 | substitute | Native caption buttons; Close still runs the unsaved prompt through Rune patch 0002 and `app.rs:480-485 close_request` (STATUS P0). |
| W-03 | P2 | n/a | Windows 11 Snap flyout over app-drawn caption buttons; Linux uses the native frame and the compositor's own snap. FEATURE_PARITY §4 lists it n/a. |
| W-04 | P1 | substitute | The native frame draws the max/restore icon and state; there is no app caption to switch label or aria state (ADR-007). |
| W-05 | P2 | n/a | The native titlebar supplies the system menu; there is no app-drawn caption to right-click. FEATURE_PARITY §4 lists it n/a. |
| W-06 | P2 | n/a | The native frame greys its own caption on blur. FEATURE_PARITY §4 lists it n/a. |
| W-07 | P1 | partial | `view.pin` command with a checked state (`commands.rs:1343-1351`; `actions_tests::pin_toggles_the_checked_state_and_sends_the_native_request`, `pin_refuses_on_wayland_with_a_clear_notice`). There is no Pin button in the toolbar, so the spec's visible control is missing (see W-10). |
| W-08 | P2 | n/a | No app-drawn caption zones exist over the lightbox; the native frame cannot be covered by the page. FEATURE_PARITY §4 lists it n/a. |
| W-09 | P1 | partial | `window_title` (`window.rs:1289-1303`) omits the typographic quotes and puts the dirty dot after the name (`dot.md • — lmdv`), while the spec prefixes `• `; `editor_tests::the_dirty_dot_and_title_clear_after_save` pins `dot.md — lmdv`. |
| W-10 | P1 | partial | Toolbar order is Open, Edit, tree, outline, Share, Settings, Help (`view.rs:470-486`): no Save, no Aa (reading settings), no Pin button, and the tab strip is a separate band (OI-003). |
| D-01 | P1 | missing | No `on_drop_files`/`accepts_files` is wired anywhere in `crates/`; Rune exposes the API (`vendor/rune/crates/rune/src/el/drop.rs`). No `.dropping` class or “Drop to open” card exists. |
| D-02 | P0 | missing | With no external drop path there is no Markdown filtering, background placement or tab-strip index insertion. |
| D-03 | P1 | missing | No drop handling at all, so the editor image copy into `assets/` and link insertion do not exist. |
| D-04 | P0 | partial | Tab drag exists through the kit and the T-22 transfer path, but see T-19: no reorder test, 4 pt slop, and the export pin refuses the drop rather than the gesture. |
| D-05 | P1 | substitute | An in-app ghost gap (`transfer_tests::hover_opens_and_clears_the_incoming_slot`; `build_layered` ghost in STATUS P4-02c) replaces the DWM-cloaked moving window; DWM does not exist on Linux. |
| D-06 | P1 | missing | No middle-button or empty-margin drag-to-scroll: `view.rs` has no middle-press handler for the article and the reader has no pan gesture. R-28 is likewise unimplemented. |
| D-07 | P2 | substitute | `tear_out_tests::a_lone_tab_never_tears_out`; “Move to a new window” is the menu substitute; OI-126 (no Rune window-drag API). |
| D-08 | P1 | partial | The kit settles a cancelled drag to Idle (`tab_strip/drag.rs release(cancelled)`), and `tear_out_tests::cancelling_a_tear_closes_the_target_gap_on_the_next_poll` covers the hover; no test cancels a reorder with Esc. |

## Counts

| Area | Done | Partial | Substitute | Missing | n/a | Total |
|---|---:|---:|---:|---:|---:|---:|
| T — Tabs and windows | 14 | 11 | 8 | 2 | 0 | 35 |
| F — File browser and folder search | 19 | 1 | 2 | 0 | 0 | 22 |
| C — Command palette | 10 | 1 | 2 | 0 | 0 | 13 |
| W — Window chrome | 0 | 3 | 3 | 0 | 4 | 10 |
| D — Drag and drop | 0 | 2 | 2 | 4 | 0 | 8 |
| **Total** | **43** | **18** | **17** | **6** | **4** | **88** |

## Every partial and missing row

### Partial (18)

- **T-01** — Proposed OI: “Tab strip band and name-sized tabs”. Decision: keep OI-003 for the band and pass a strip viewport width so tabs shrink from their name width to the kit minimum instead of staying at 224 pt.
- **T-02** — Proposed OI: “Pasted tabs use a clipboard icon”. Decision: add a clipboard glyph and select it for `DocumentKey::Untitled` tabs.
- **T-03** — Proposed OI: “Saved check in the tab slot”. Decision: draw the 1.4 s check where the close button sits and expose it as a live status, rather than a toolbar sentence.
- **T-05** — Proposed OI: “Delete closes the focused tab”. Decision: add Delete/Backspace to the strip's focused tab, matching the other close paths.
- **T-12** — Proposed OI: “Placement modifiers for recents and document links”. Decision: thread `Mods` through the recent-file rows and `LinkWire`, or record the omission as an accepted substitute in FEATURE_PARITY.
- **T-13** — Proposed OI: “Background open must not consume a front start page”. Decision: make start-page replacement foreground-only; a background open inserts a tab instead.
- **T-14** — Proposed OI: “Background insertion after the origin tab”. Decision: insert at the originating tab plus a per-window background queue, reset when another tab is activated.
- **T-19** — Proposed OI: “Tab drag reorder: 5 pt slop, export guard, test”. Decision: align the slop, refuse the gesture (not only the drop) while exporting, and add a runtime reorder test.
- **T-26** — Proposed OI: “Per-tab sticky notices and scheme”. Decision: decide whether window-level notices are the accepted model; if not, move `UiState.notice` and the tree root onto `TabView`.
- **T-29** — Proposed OI: “Ctrl+W cancels a tab drag before closing”. Decision: cancel `state.drag` in the close command path and test the full precedence chain.
- **T-31** — Proposed OI: “New-window cascade chain and max/min inheritance”. Decision: add the 3 s previous-window chain and inherit maximized/minimized placement.
- **F-11** — Proposed OI: “Tree keyboard traversal test”. Decision: add runtime coverage for Up/Down/Home/End/Left/Right/Enter/Space and the Shift+Enter window path.
- **C-01** — Proposed OI: “IME-composition guard for the palette chord”. Decision: skip the chord while composing if Rune exposes that state; otherwise record the platform gap in FEATURE_PARITY.
- **W-07** — Proposed OI: “Pin toggle in the toolbar”. Decision: add the visible Pin control with `aria-pressed`, or get explicit acceptance for the palette-only command.
- **W-09** — Proposed OI: “Window title format”. Decision: match `• name — lmdv` with quotes, or amend the spec to the current `name • — lmdv`.
- **W-10** — Proposed OI: “Toolbar layout: Save, Aa, Pin”. Decision: add the missing controls and order them as specified; the tab band stays OI-003.
- **D-04** — Proposed OI: “Tab drag coverage”. Decision: same work as T-19; add drag/reorder tests so D-04 is evidenced rather than inferred from the transfer tests.
- **D-08** — Proposed OI: “Esc cancels a reorder (test)”. Decision: add a runtime Esc-mid-drag test over the kit's cancel path.

### Missing (6)

- **T-15** — Proposed OI: “Background tab pulse”. Decision: mark a tab opened behind with the kit's unread/attention animation for a short interval and reveal it.
- **T-34** — Proposed OI: “Narrow toolbar breakpoints”. Decision: hide separators under 720 pt, Open/Help under 640 and `+` under 560, leaving Ctrl+T and the tab list.
- **D-01** — Proposed OI: “External file-drop overlay”. Decision: wire the root's `on_drop_files` and `on_drop_hover` handlers and draw the “Drop to open” card.
- **D-02** — Proposed OI: “Markdown file drop and strip index”. Decision: filter Markdown paths, open one tab each at the drop index with the device-pixel x conversion.
- **D-03** — Proposed OI: “Image drop in the editor”. Decision: copy a dropped image into `assets/` and insert its Markdown link when editing.
- **D-06** — Proposed OI: “Drag-to-scroll”. Decision: implement R-28's middle-drag anywhere and left-drag on the pane margin with the 3 pt threshold and suppressed autoscroll.
