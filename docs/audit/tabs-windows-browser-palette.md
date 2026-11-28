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
| T-01 | P0 | partial | The strip now passes the measured `viewport_width`, so tabs shrink toward the kit's 28 pt minimum before scrolling (`view.rs`, `tab_list` tests; `--tree` proof). Still partial: the kit has no per-name intrinsic width (equal shares after the 224 pt cap) and no flexible toolbar middle slot, so the strip stays in its own band. Named API gap: a `Toolbar::center` that flexes between leading/trailing controls, and a `TabOverflow` width policy that measures the title. |
| T-02 | P0 | done | Pasted (`DocumentKey::Untitled`) tabs draw the new `CLIPBOARD` glyph, files the document/pencil glyph and start tabs the home glyph; the strip title stays stem-only and the full name remains the accessible label/card. Proven by `--scene tabs --tree` (`tab-clipboard-*`) and `window::tests::strip_titles_drop_the_extension`. |
| T-03 | P0 | partial | Dirty shows the orange Attention dot; after a save the tab's leading slot becomes a check in the success colour for the existing 1.4 s flash (`window.rs` `track_saved_flash`/`tab_shows_saved_check`, `--tree` `tab-saved-*`), and `tx.announce("Saved", Polite)` announces it; typing restores the dot immediately. Named gap: the check is the leading mark, not the close slot, because rune-kit has no Saved status and the close button owns that slot. |
| T-04 | P0 | done | `window::tests::new_tab_appends_at_the_end_and_brings_it_forward`; `window::tests::the_tab_menus_new_tab_inserts_after_its_tab`; palette `file.new-tab` (`palette_tests::commands_ignore_the_modifiers_they_were_chosen_with`); `+` button at `view.rs:585-608`. |
| T-05 | P0 | done | Delete/Backspace on a focused tab (`t-tab-{id}`) call `request_close_tab`, so the dirty Save/Don't save/Cancel path and the right-then-left neighbour choice apply; `tab_keys_tests` cover Delete, Backspace and the dirty prompt, and middle click still closes. |
| T-06 | P0 | done | `window::tests::closing_a_lone_start_page_asks_the_window_to_close`; `window.rs:1024-1136`. |
| T-07 | P0 | done | `actions_tests::reopen_restores_a_file_tab_at_its_index_and_place`, `the_closed_stack_stops_at_the_original_cap`, `reopen_restores_pasted_text_with_its_paste_id_and_place`, `reopen_reuses_a_document_another_window_still_shows`. |
| T-08 | P0 | done | `tab_keys_tests::pgdn_pgup_and_ctrl_tab_wrap_around_the_strip`; commands `tab.next`/`tab.previous` carry both chords. |
| T-09 | P1 | substitute | `tab_keys_tests::the_digits_are_read_from_the_character_or_the_physical_key`, `numbered_digits_select_a_tab_and_nine_is_the_last`. OI-166 (accepted): no `keyCode` fallback when a backend names neither a Command digit nor a physical key. |
| T-10 | P1 | done | `tab_keys_tests::shift_page_keys_move_the_front_tab_and_clamp_at_the_ends`. |
| T-11 | P0 | done | `window::tests::opening_a_file_twice_activates_its_existing_tab`; a background open keeps the front tab (`browser_tests::the_menus_open_rows_use_the_background_tab_and_new_window_paths`). The specified pulse of the existing tab is absent and is counted under T-15. |
| T-12 | P0 | done | All four click sources take the four gestures. Document links use `LinkPlacement::for_click(self.link_modifiers(), tx.button())` (`links.rs::open_link`), tree/search rows already honored Ctrl/Ctrl+Shift/Shift, and recents now map plain/Ctrl/Ctrl+Shift/Shift through `open_recent_placed` (`links::tests::a_recent_row_takes_the_four_click_placements`, `::a_recent_row_press_passes_the_held_modifiers`). Middle stays background. The earlier audit note that `open_link` ignored modifiers was stale. |
| T-13 | P0 | done | `open_path_placed`/`attach_document_placed` only replace a front start page for a foreground open; a background document and any pasted tab are never consumed. The browser/search/palette background path now calls `open_path_background` (no switch-back), and `window::tests` cover both cases. |
| T-14 | P1 | done | A background open inserts after the last background tab (else after the front tab), successive opens queue, and any tab coming forward resets the queue; foreground/external/drop opens still append or use their drop index. Duplicates return the existing tab without moving it. Runtime palette test pins the order. |
| T-15 | P2 | done | A background-opened tab gets a 2.4 s Unread dot plus the kit's 900 ms ring, clears on activation, wakes at expiry and is revealed in the strip. Background-opening an already-open document pulses that tab too. |
| T-16 | P1 | done | `tab_card_tests::moving_from_tab_to_tab_keeps_the_card_up_at_once`, `the_card_names_the_file_its_folder_and_the_time_or_unsaved_changes`, `pasted_and_start_tabs_say_what_they_are`; scene `tab-card`; script `tests/scripts/tab-card.json`. |
| T-17 | P1 | substitute | `tab_list` tests `the_button_appears_only_when_the_tabs_overflow`, `choosing_a_tab_from_the_list_activates_it_and_closes_the_list`, `file_tabs_show_their_folder_and_new_tab_appends_a_start_page`; scene `tab-overflow`. OI-158 fixed; OI-166 accepted for the plain vertical wheel and the one-frame delay. |
| T-18 | P1 | done | Menu at `view.rs:167-330`; `actions_tests::the_tab_menu_copy_and_reveal_act_on_that_tab`, `close_other_tabs_keeps_only_the_chosen_tab`, `tab_menu_opens_on_a_background_tab`; scenes `tab-menu`, `tab-menu-file`. |
| T-19 | P0 | done | A runtime reorder test drives tab 1 past tab 2's midpoint, Esc-cancel is tested, and the strip is built without `drag_state`/`tear_out` while an export pins the document, so no reorder or tear-out can start. The kit's drag slop stays 4 pt against the spec's 5 (private constant; accepted). |
| T-20 | P1 | substitute | `tear_out_tests::a_release_over_another_window_moves_the_tab_there`, `a_drop_outside_every_window_opens_one_at_the_pointer_and_adopts_the_tab`; kit detach distance is 36 pt. Release-only: the new window does not follow the pointer until mouse-up (OI-125, STATUS P4-02c). |
| T-21 | P1 | done | The lone reading tab uses `TabItem::window_drag(true)`; multiple tabs keep select/reorder/tear handlers. Runtime proof with `TitlebarAction::Drag`. |
| T-22 | P1 | substitute | `transfer_tests::a_clean_tab_moves_with_its_view_state_and_the_watch`, `a_clean_duplicate_merges_into_the_existing_tab`, `a_dirty_duplicate_is_refused_and_returned_with_text_and_lease_intact`, `hover_opens_and_clears_the_incoming_slot`. STATUS P4-02b: in-process protocol, no 15 s timeout; a vanished target restores at once. |
| T-23 | P1 | done | `actions_tests::close_other_tabs_keeps_only_the_chosen_tab`, `close_tabs_to_the_right_closes_from_the_right`, `a_dirty_tab_in_the_sequence_asks_and_cancel_stops_the_run`. |
| T-24 | P0 | done | `editor_tests::a_clean_pasted_document_still_asks_on_window_close`, `save_all_waits_for_a_pasted_save_as_before_closing`, `the_last_view_of_a_dirty_document_asks_and_cancel_keeps_everything`; localized list in `window.rs:1266-1286`; Save all / Don't save / Cancel in `view.rs`. |
| T-25 | P1 | done | `editor_tests::a_second_close_request_never_stacks_a_second_question`; `shared.set_window_closing`; STATUS P4-02a. |
| T-26 | P0 | done | Each tab keeps its own sticky notice and rendered scheme; notices are stashed/restored across switches, attach and transfer, saves clear only that document's copy, and a failed background load keeps its notice on its tab. Ordinary notices stay window-transient and reach the front tab. Tree root stays per-window under OI-129's accepted model. Tests + `--scene tab-notice`. |
| T-27 | P1 | substitute | No rendered-DOM fragments exist: the native view is rebuilt per frame and every tab keeps its parsed model (`TabView.reader`), with no 6-tab budget. Memory is model-sized rather than DOM-sized; the specified eviction/re-render behavior has no equivalent. |
| T-28 | P1 | substitute | A language change applies live (`settings.rs:219-227 apply_locale`; “there is no reload”), so no `sessionStorage` transfer exists and the language switch cannot discard tab state. OI-176: the live switch is skipped under `cfg(test)`, so no test exercises it. |
| T-29 | P1 | done | The command/menu precedence is audited, and `close_active_tab` now consumes Ctrl+W/Ctrl+F4 during an in-hand tab drag (a second press closes). A runtime test pins the whole key path; the tab-key cancel test remains. |
| T-30 | P0 | done | `commands.rs` test `close_window_goes_through_the_dirty_guard`; `guard_close_window` refuses a second question while one is open; chord is Ctrl+Shift+W. |
| T-31 | P1 | done | `WindowPlan`/`new_window_plan` implement the 3 s chain anchor, 32 pt cascade from the last runtime-created window, display-edge restart, and maximized/minimized inheritance; tests cover the chain and placements. Forwarded `--new-window`, browser/link and tear-out paths still use the plain cascade (tracked in S-13's residual). |
| T-32 | P1 | done | `Ctrl+O` uses `Tx::pick_files`; headless proof opens three files with the first in front and two background tabs in selection order. |
| T-33 | P0 | done | `window::tests::two_windows_share_one_document_one_lease_and_one_registry`; `windows::tests::an_open_document_selects_the_most_recently_focused_holder`, `a_file_prefers_the_first_idle_normal_window_in_focus_order`. |
| T-34 | P1 | done | Toolbar rules disappear <720, Open/Help <640, `+` <560; Ctrl+T/Ctrl+O/F1 and the all-tabs list remain. The centred title is dropped <720 because the kit has no flexible middle slot. |
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
| F-11 | P1 | done | Arrow/Home/End/Enter/Space traversal is covered from a real focus or active file, including Up/Down, Home/End, Left/Right open/close, inert Right on a file, empty folders, plain/Ctrl/Shift opens and search hits. Fixed Down from an unfocused tree skipping the first row. |
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
| C-01 | P0 | done | `Ctrl+K`/`Ctrl+Shift+P` are suppressed while a field holds an IME composition and while the palette query composes; normal chords, AltGr/Meta rejection and fresh-press closing unchanged. Headless proof at compositing and after commit. |
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
| C-12 | P1 | done | `palette_tests::the_input_advertises_list_autocomplete_and_controls_the_list`, `an_empty_palette_does_not_name_a_missing_list` and the older role/count tests; `--scene palette --tree` proof shows `autoComplete: "list"` and `controls: ["palette-list"]` on the input. |
| C-13 | P1 | substitute | `palette_tests::a_tab_switch_or_a_document_change_closes_it`, `another_modal_closes_it`; `poll_palette` closes on prompt/help/Share/table/lightbox/block-menu/tab-menu (`palette.rs:695-711`). The full-window scrim (`palette_ui.rs:49-78`) makes a tab drag unreachable while the palette is open, so no drag trigger is needed; static linkage makes the module-load-failure clause n/a. |
| W-01 | P0 | done | Linux CSD (ADR-028): the rune-kit toolbar is a `.window_drag()` band, so empty parts move the window and double-click toggles maximize/restore; native frame stays on X11/KDE/Hyprland. |
| W-02 | P0 | done | Rune draws caption buttons over the toolbar from the desktop button layout; caption Close routes through `CloseResponse::Defer` and the unsaved prompt. |
| W-03 | P2 | n/a | Windows 11 Snap flyout over app-drawn caption buttons; Linux uses the native frame and the compositor's own snap. FEATURE_PARITY §4 lists it n/a. |
| W-04 | P1 | done | Rune swaps the max/restore icon and aria label on configure/resize; its wording is “Restore” rather than “Restore down” (cosmetic follow-up). |
| W-05 | P2 | n/a | The native titlebar supplies the system menu; there is no app-drawn caption to right-click. FEATURE_PARITY §4 lists it n/a. |
| W-06 | P2 | n/a | The native frame greys its own caption on blur. FEATURE_PARITY §4 lists it n/a. |
| W-07 | P1 | done | `view.pin` keeps its checked state, per-window (never persisted), and the toolbar now has the visible `pin-toggle` button reading the same predicate; the palette check and the Wayland refusal test remain. |
| W-08 | P2 | n/a | No app-drawn caption zones exist over the lightbox; the native frame cannot be covered by the page. FEATURE_PARITY §4 lists it n/a. |
| W-09 | P1 | done | `window_title` now yields `“name” — lmdv`, `• “name” — lmdv` when dirty, and exactly `lmdv` on the start page; registry/session titles inherit it. Tree proofs cover clean/start/dirty, and the transfer/editor tests use the quoted form. |
| W-10 | P1 | done | Toolbar order now includes tree, Open, Edit, Save, Share, Aa (reading settings), Pin, Help, Outline and the settings gear; Save/Aa/Pin use existing commands and accessible names. The tab strip remains a separate band (OI-003). |
| D-01 | P1 | done | The window root accepts Markdown files and reports hover/rejection (`window.rs` `DropState`, `view.rs` handlers); a drag over the window draws the “Drop to open” card, clears on leave/drop, and a non-Markdown drop posts `msg.notMarkdown`. Scripted drag/drop proofs in `view::tests`. X11/Windows only (Wayland reports no file drags). |
| D-02 | P0 | done | Markdown-only; several files open one tab each, first front and rest background; a drop on the tab band converts `Tx::drop_point` through the measured tab-slot rects (`Cx::window_layout`) and inserts the files at that index, successive files following (`window.rs` `dropped_files`/`open_dropped_at`, 7 tests + scripted `drop-strip` proof). Already-open files are not duplicated. Above 40 tabs the kit builds only near-viewport slots, so an unbuilt slot resolves to the nearest built boundary. |
| D-03 | P1 | done | Image paste/drop writes into the document's `assets/` (`pasted-<ms>.png`/`<stem>[-n].<ext>`), inserts the URL-encoded Markdown link, and asks Save as first for pasted documents. Shared with E-36; tests in `lmdv-io`/`editor_tests` and a headless drop proof. |
| D-04 | P0 | partial | Tab drag exists through the kit and the T-22 transfer path, but see T-19: no reorder test, 4 pt slop, and the export pin refuses the drop rather than the gesture. |
| D-05 | P1 | substitute | An in-app ghost gap (`transfer_tests::hover_opens_and_clears_the_incoming_slot`; `build_layered` ghost in STATUS P4-02c) replaces the DWM-cloaked moving window; DWM does not exist on Linux. |
| D-06 | P1 | done | The reader has middle/empty-margin drag-to-scroll with a 3 pt threshold, cursor, autoscroll and Rune middle-autoscroll suppression (R-28); see the R-28 row for the link-glyph routing residual. |
| D-07 | P2 | done | Lone-tab window drag is wired through the kit hook; the menu remains as an alternate. |
| D-08 | P1 | done | Escape, `Tx::cancel_pointer` and opening a modal all settle `TabDragPhase::Idle` with original tab order; no kit callback was needed. |

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
