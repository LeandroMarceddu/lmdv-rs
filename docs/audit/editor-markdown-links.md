# Audit — Editor (E), Markdown rendering (M), Links and media (L)

**Baseline:** `a8c11cf` (2026-11-20), spec `docs/BEHAVIOR_SPEC.md` §2.3 (E-01–E-40), §2.4 (M-01–M-34), §2.10 (L-01–L-15), with §3.9 (link resolution) and §3.10 (math) read as supporting detail.

## Method

- For every row I read the feature text, priority and acceptance criterion, then looked for a **named test**, fixture, scene key, script, golden or a concrete `docs/STATUS.md` / `docs/OPEN_ITEMS.md` note.
- Evidence was searched in `crates/*/src/*tests.rs`, `crates/lmdv-app/src/**/tests` modules, `crates/*/tests/`, `tests/fixtures/`, `tests/scripts/` and the scene table in `crates/lmdv-app/src/app.rs`.
- The `src/...` source references inside the spec point at the reference WebView application. They were used to understand the behavior, never as evidence that lmdv implements it.
- A test name proves the behavior is pinned only when the test asserts it. Helper tests that merely exercise a building block were not counted as coverage of a row.
- **No suite was run.** The only execution was the focused spot-check `cargo test -p lmdv-app --lib links::tests` (7 passed), used to confirm the link classifier tests exist and pass. Every other “done” is static evidence from the checkout; where a test is named I did not rerun it.
- Status meanings: `done` = behavior plus concrete evidence; `substitute` = deliberate different implementation accepted in `FEATURE_PARITY.md`/`OPEN_ITEMS.md`; `partial` = behavior exists but a specified detail is missing or tracked as open; `missing` = no implementation found; `n/a` = the row’s premise does not exist in a native Rune app (reason given).

## Counts

| Area | Rows | done | substitute | partial | missing | n/a |
|---|---:|---:|---:|---:|---:|---:|
| Editor (E) | 40 | 20 | 2 | 15 | 2 | 1 |
| Markdown rendering (M) | 34 | 25 | 3 | 6 | 0 | 0 |
| Links and media (L) | 15 | 3 | 0 | 7 | 4 | 1 |
| **Total** | **89** | **48** | **5** | **28** | **6** | **2** |

Two P0 rows are wholly missing: **L-03** (wiki resolution) and **L-11** (link click placement). The other missing rows are E-34 (P2), E-36 (P1), L-06 (P1) and L-12 (P2).

## Rows

| ID | P | Status | Evidence or gap |
|---|---|---|---|
| E-01 | P0 | done | The toolbar label/icon toggle, pane restore, reader-find close and section-line reset are pinned by `editor_tests::edit_mode_hides_the_reading_panes_and_restores_them`; the editor surface now enters on `Transition::opacity()` (~0.2 s, paint-only; `Transition::none()` under Reduce Motion), and `ctrl_e_twice_returns_to_the_same_reading_view` asserts an identical tree/roles/anchor/focus/scroll round trip. |
| E-02 | P0 | done | `editor_tests.rs::the_editor_scene_has_source_and_preview_and_a_narrow_window_keeps_the_source_only` (1100 pt split; 700 pt source-only plus preview toggle); scene `editor`. |
| E-03 | P1 | done | `enter_editor` takes the caret from the measured reading block, honors a pending `--line` first, falls back to the heading only when no block was measured, clamps to the buffer, and reveals the source after its first frame. Tests: `entering_edit_mode_puts_the_caret_at_the_section_being_read`, the block/line cases in `editor_tests`. |
| E-04 | P1 | done | `preview_nav_tests.rs::the_block_menu_enters_edit_at_the_clicked_line`, `::edit_from_here_moves_the_caret_when_already_editing`, `::shift_f10_opens_the_focused_blocks_menu`. |
| E-05 | P0 | done | The preview renders 200 ms after the last keystroke; a failed parse keeps the last good model and shows a dismissible “Preview error: …” once per failed revision (`editor_tests`). |
| E-06 | P1 | done | A source-keyed `MermaidCache` (`RenderOptions.mermaid`) reuses a rendered card across preview rebuilds and drops it on theme change (24 entries, LRU). Tests: renderer `an_unchanged_diagram_is_reused_by_its_source`; app `an_unchanged_diagram_is_reused_across_a_preview_refresh`, `the_diagram_cache_drops_its_cards_when_the_theme_changes`. |
| E-07 | P0 | done | One-way sync, interpolation and manual-preview freedom are pinned; mapping now uses exact per-source-line visual geometry through `rune::text::measure` with the field's font/spans/wrap width (cached per revision/width/font/block), block mode maps locally, every table crossing the viewport top contributes its measured header, front matter interpolates as its own anchor, and the section-line reserve comes from `Cx::layout`. Tests: `wrapped_lines_take_one_entry_each_and_map_by_their_drawn_line`, `a_block_row_maps_wrapped_lines_inside_its_own_block`, `the_app_maps_wrapped_prose_to_the_line_it_draws`, `every_table_crossing_the_viewport_top_contributes_its_header`, `a_front_matter_line_interpolates_across_the_card`, `the_section_line_reserve_moves_the_preview_below_it`. Fixed the inert `b{n}/table/head` lookup. |
| E-08 | P0 | done | `editor_tests.rs::edit_marks_dirty_and_save_writes_the_file_and_cleans_it`, `::the_saved_check_expires_after_its_seconds`; only the task tick writes in viewer mode (`tasks_tests.rs::a_checkbox_tap_writes_the_file_and_its_undo_restores_it`). |
| E-09 | P0 | done | `editor_tests.rs::crlf_and_a_bom_round_trip_without_readding_the_bom`; `lmdv-edit/src/session.rs::new_normalizes_crlf_strips_bom_and_keeps_eol`, `::save_text_preserves_the_final_newline_and_never_adds_a_bom`. |
| E-10 | P0 | done | `editor_tests.rs::a_conflict_asks_first_and_overwrite_wins`, `::a_conflict_keep_theirs_leaves_the_buffer_dirty_and_the_disk_copy_alone`. |
| E-11 | P0 | done | `editor_tests.rs::an_untitled_document_asks_for_a_path_on_ctrl_s`, `::the_save_as_name_is_file_safe_and_ends_in_md`, `::save_as_starts_in_the_pane_root_and_falls_back_to_documents`, `::saving_pasted_text_makes_the_tab_that_file_and_moves_its_places`. |
| E-12 | P1 | done | `editor_tests.rs::save_as_refuses_a_file_open_with_unsaved_changes`, `::save_as_merges_with_a_clean_file_open_in_this_window`, `::save_as_finds_a_duplicate_through_a_symlink`, `::save_as_releases_a_clean_file_open_in_another_window`. |
| E-13 | P0 | done | Leaving edit mode with a dirty buffer asks “You have unsaved changes. Discard them?” with **Discard**/**Cancel**; Discard reloads the saved version, drops the draft and leaves edit mode (`editor_tests`). |
| E-14 | P0 | substitute | Accepted substitute (user, 2026-11-18): OI-072 and `FEATURE_PARITY.md` §4 — each tab owns its session, so opening another document prompts nothing; destructive closes still ask. |
| E-15 | P0 | done | A same-spelling reopen now runs `check_open_document` through `ReloadIntent::ReopenCheck`: dirty keeps the buffer and raises the existing `DiskChanged` question when the disk differs (`editor_tests::a_reopen_of_a_dirty_file_keeps_the_buffer_and_asks_about_a_newer_disk_copy`, `::a_dirty_reopen_whose_buffer_already_matches_the_disk_keeps_the_buffer_dirty`); clean reloads differing text at its reading anchor (`::a_reopen_of_a_clean_file_reloads_it_at_its_reading_anchor`); identical text is a no-op (`::a_reopen_without_a_disk_change_keeps_the_buffer_and_its_parse`). A `--line` on an open editing tab moves the source caret and scrolls both panes (`::a_line_target_on_an_open_editing_tab_scrolls_the_source_and_the_preview`). The check reads the file instead of an mtime probe. |
| E-16 | P0 | done | `editor_tests.rs::the_one_second_draft_timer_writes_while_dirty`; switch/close flush at `window.rs:721`, `:979`. Base is the LF-normalized starting text (`draft.base` assertion). |
| E-17 | P0 | done | `editor_tests.rs::a_crash_draft_is_offered_and_restore_enters_edit_mode_without_writing_the_file`, `::a_draft_equal_to_the_file_is_dropped_silently`, `::a_deleted_file_still_offers_its_draft_and_restore_leads_to_save_again`, and the new `::a_draft_is_not_offered_while_another_window_holds_the_edit_lease` (offer → lease held → suppressed → release → offered again). Dismiss deletes; the shared `Document::lease_holder()` is the cross-window check. |
| E-18 | P0 | done | Same crash-draft test: Restore enters edit mode, `lastSaved` stays the disk base, buffer is dirty and the file is untouched. |
| E-19 | P1 | done | `lmdv-io/src/store.rs::drafts_are_capped_by_count_and_characters` covers 20 entries, the 2 M-character total and oldest-first eviction. File-store write failures surface as one notice rather than the WebView quota retry. |
| E-20 | P0 | done | `edit_ui_tests.rs::ctrl_f_opens_the_bar_and_steps_matches_both_ways`, `::find_options_and_an_invalid_regex_show_an_error_instead_of_panicking`, `::find_fields_are_single_line_and_the_replace_row_follows_ctrl_h`; F3/Shift+F3 in `edit_ui.rs:855-878`. |
| E-21 | P1 | done | `edit_ui_tests.rs::ctrl_f_opens_the_bar_and_steps_matches_both_ways` drives `editor_context_key` with focus outside the field; palette “Find in document” opens the editor bar with the query (`palette.rs:1305-1321`). |
| E-22 | P0 | done | The format bar keeps B/I/S/Code/Link, Text/H1–H3 (Setext-aware), bullet/ordered/task/quote, then **Insert** (table, code, formula, diagram, three callouts, divider) and **Tools** (find, find & replace, go to line, move/duplicate/delete/select line, spellcheck), with the plain “Type / for blocks” hint. `edit_ui_tests::the_insert_menu_and_the_tools_menu_draw_their_rows_and_the_hint_is_plain`, `::the_insert_menu_rows_insert_the_same_blocks_as_the_slash_menu`, `::the_tools_menu_opens_find_runs_line_commands_and_toggles_spellcheck`, plus the two pointer-open tests. The eight block buttons moved into Insert. |
| E-23 | P0 | done | `lmdv-edit/src/format.rs::bold_toggle_wraps_and_unwraps_in_one_undo_step`, `::whitespace_stays_outside_the_markers`, `::word_under_caret_is_wrapped`, `::italic_and_bold_runs_do_not_eat_each_other`, `::strike_and_code_markers_round_trip`. |
| E-24 | P1 | done | `lmdv-edit/src/format.rs::links_cover_url_text_and_empty_selections` covers the three selection cases and the caret/URL selections. |
| E-25 | P0 | done | The format bar reads Setext H1/H2 (text line and underline) for its active level while the commands still toggle ATX (`edit_ui_tests`). |
| E-26 | P0 | done | `lmdv-edit/src/format.rs::bullet_quote_and_task_round_trip`, `::ordered_lists_renumber_and_convert`, `::empty_lines_are_untouched_when_content_lines_exist`. |
| E-27 | P1 | done | `lmdv-edit/src/format.rs::table_block_replaces_an_empty_line_and_selects_the_placeholder`, `::blocks_after_content_get_blank_lines_around_them`, `::block_templates_select_their_placeholders`. |
| E-28 | P1 | done | The slash panel is a layer popover anchored to the measured caret (inline fallback for the opening frame), and replace+insert is one `undo_group`/structural journal entry. Existing filter/escape/fence tests plus `a_slash_insert_is_one_undo`. |
| E-29 | P1 | partial | Line commands are covered; `Ctrl+D` now selects the word and its next occurrence in both modes. `Ctrl+Alt+↑/↓` add-cursor remains blocked: pinned Rune has one `TextEdit` selection, one caret rect and no multi-caret edit/undo/paint model (E-MULTI gap). |
| E-30 | P1 | done | Known fence/CSS comments keep their forms; unknown fence languages and indented code are documented no-ops (a Markdown comment there would become code, and guessing a language marker can corrupt). Tests cover doc start, blank lines, indented/list content, headings/rules/quotes/fences. |
| E-31 | P1 | done | Block mode shares `goto_target_offset` with the single field: `40`, `+N`, `-N`, `N%`, `line:col`, clamped. Tested in block mode. |
| E-32 | P1 | substitute | Linux `lmdv-edit::spell` (Spellbook/Hunspell) replaces WebView2’s dictionary: `FEATURE_PARITY.md` E-SPELL, OI-174/OI-175. Evidence: `editor_tests.rs::turning_spellcheck_off_stops_the_checker_and_the_underlines`, `::the_app_discovers_a_dictionary_beside_its_data_and_checks_with_it`; suggestions menu accepted as absent. |
| E-33 | P0 | done | `block_ui_tests.rs::tab_indents_and_shift_tab_outdents_in_the_single_field`, `::tab_does_not_move_focus_between_block_fields`, `::escape_then_tab_leaves_the_source_without_indenting`, `::escape_then_tab_in_the_single_field_does_not_indent`, `::the_first_escape_clears_a_selection_and_does_not_arm_the_leave`. |
| E-34 | P2 | done | `editor-tail` adds 60% of the measured viewport (min 160 pt) after the source body; the scroll-extent test covers it. |
| E-35 | P1 | done | Padding clicks map by the press's fraction of the block height; text boxes guard links/tasks/pictures/summaries; keyless texts get stable `b{i}-text-{n}` identities; preview-height placement is wired. Tests: `a_press_on_a_blocks_padding_lands_on_its_preview_line`, renderer `two_identical_keyless_texts_keep_distinct_stable_identities`, plus the click guards. |
| E-36 | P1 | done | Pasting/dropping an image writes `assets/pasted-<ms>.png` (or `<stem>[-n].<ext>`) through `lmdv-io::assets`, inserts `![](assets/…)` URL-encoded, and asks Save as first for pasted documents. Tests in `lmdv-io` and `editor_tests`; headless image-drop proof. |
| E-37 | P2 | done | `editor_tests.rs::the_single_source_field_follows_the_reading_size`, `::block_mode_fields_follow_the_reading_size`, `edit_ui.rs::the_editor_font_is_the_reading_size_at_875`; scene `editor-size`. |
| E-38 | P1 | n/a | There is no lazily imported editor chunk in the native binary, so a chunk load cannot fail. The editor-missing card (`editor.notAvailable`) is the only analogous fallback and reading is independent of it. |
| E-39 | P0 | done | `editor_tests.rs::pasted_documents_draft_under_a_pasted_key` (typed pasted text reaches the buffer and draft), `::an_untitled_document_asks_for_a_path_on_ctrl_s`, `::a_clean_pasted_document_still_asks_on_window_close`. |
| E-40 | P0 | done | `editor_tests.rs::each_tab_keeps_its_editor_history_across_a_switch`; a moved editing tab keeps its session and lease (`transfer_tests.rs::an_editing_tab_transfers_the_lease_and_the_source_becomes_a_viewer`). |
| M-01 | P0 | done | `lmdv-render/src/tests.rs::features_renders_headings_paragraphs_tables_and_code`; `lmdv-md/src/tests.rs::test_features_fixture_projection`, `::commonmark_math_fixture_projection`. |
| M-02 | P0 | done | `lmdv-md/src/tests.rs::github_line_breaks_become_spaces_or_hard_breaks`. |
| M-03 | P0 | done | `lmdv-md/src/tests.rs::github_heading_slug_matches_the_original` (`1-introduzione`, `qa`, `attività`, empty-title legacy fallback). |
| M-04 | P0 | done | `lmdv-md/src/tests.rs::legacy_ids_are_recorded_when_they_differ` (asserts `resolve_heading("q%26a")`), `::legacy_slug_matches_encode_uri_component`; `model.rs:90-105` resolves by current id, legacy id or anchor. |
| M-05 | P1 | done | `lmdv-md/src/tests.rs::reserved_heading_ids_get_user_content_prefix` (Toolbar, mmd-12, tree-group-3; both names resolve). |
| M-06 | P0 | done | `lmdv-md/src/tests.rs::tables_keep_alignment_and_inline_cells`; `tables_tests.rs::every_table_gets_a_copy_and_preview_bar_and_none_without_a_table`, `::a_table_wider_than_the_reading_column_fits_the_column` (columns keep their longest word, unbounded above); renderer sticky header test `a_tables_header_row_is_keyed_and_sticky` and the column-width regressions `a_long_word_widens_its_column_past_the_cap_and_the_table_scrolls`, `a_long_code_word_widens_its_table_column_in_the_monospace_face`. |
| M-07 | P0 | done | `lmdv-md/src/tests.rs::task_lists_are_live_checkbox_objects`; `lmdv-render/src/tests.rs::a_task_list_renders_checkboxes_with_item_labels`; `tasks_tests.rs::a_checkbox_tap_writes_the_file_and_its_undo_restores_it`, `::a_text_click_never_toggles_or_writes`, `::the_viewer_tree_has_one_real_checkbox_per_item`. |
| M-08 | P0 | done | `lmdv-md/src/tests.rs::strikethrough_is_a_style_span`. |
| M-09 | P0 | done | `lmdv-md/src/tests.rs::explicit_urls_and_www_are_linkified`, `::bare_filenames_and_partial_www_stay_text` (README.md, main.rs, file.csv, dev-www/fq.www stay text). |
| M-10 | P1 | done | `lmdv-md/src/tests.rs::quotes_curl_but_replacements_do_not` (dashes and `(c)` stay literal). |
| M-11 | P1 | done | `lmdv-md/src/tests.rs::footnotes_are_collected_and_numbered_by_reference`, `::footnote_definitions_may_have_several_blocks`; `lmdv-render/src/tests.rs::footnotes_get_entries_and_reference_links`. |
| M-12 | P1 | done | `lmdv-md/src/tests.rs::front_matter_flat_subset`, `::front_matter_nested_or_odd_falls_back_to_raw`, `::front_matter_unclosed_runs_to_eof`; `lmdv-render/src/tests.rs::front_matter_is_a_collapsed_card_by_default`, `::an_open_front_matter_card_shows_the_entries`; rich copy omits it (`lmdv-export/tests/rich_copy.rs::front_matter_is_omitted_and_the_output_is_deterministic`). |
| M-13 | P0 | done | `MdDoc::code_texts` numbers code blocks in drawing order; the renderer emits `copy-code:{n}` for top-level, nested, duplicated, HTML `<pre>` and footnote blocks, and the app copies exactly that block (`lmdv-md`/`lmdv-render`/`links` tests). |
| M-14 | P1 | substitute | `lmdv-render/src/tests.rs::mermaid_draws_or_falls_back_to_its_source` renders diagrams and keeps the source on failure. The banner is the generic localized text, without the parser error (OI-045, accepted-gap); diagram coverage is the 17 native families (`FEATURE_PARITY.md`). The WebView `asset:` host refusal has no native analogue. |
| M-15 | P0 | done | `lmdv-md/src/tests.rs::inline_math_follows_github_rules` / `::inline_math_follows_the_github_delimiters` cover `$ x $`, `$5 and $10`, `\$x\$`, `<span>$</span>5`, backtick math and code/link exclusions. |
| M-16 | P0 | done | `lmdv-md/src/tests.rs::display_blocks_need_a_clean_close` (`$$ 100 l’uno` stays prose, blank line cancels, indented/fenced ignored). |
| M-17 | P0 | done | `lmdv-md/src/tests.rs::display_math_in_a_sentence_stays_in_the_paragraph`, `::inline_display_math_keeps_the_paragraph_across_a_line_break`; `lmdv-render/src/tests.rs::display_math_in_a_paragraph_gets_its_own_line`. |
| M-18 | P0 | done | `lmdv-md/src/tests.rs::fenced_math_ignores_words_after_the_language`; failure falls back to the TeX (`lmdv-render/src/tests.rs::math_falls_back_to_tex_without_a_resolver`, `p2_tests.rs::a_broken_formula_falls_back_to_its_tex`). |
| M-19 | P1 | done | `lmdv-md/src/tests.rs::all_five_callouts_parse_with_bodies`, `::callouts_take_a_custom_title_and_are_case_insensitive`; `lmdv-render/src/tests.rs::callouts_are_labelled_cards_with_their_bodies`. |
| M-20 | P1 | done | `lmdv-md/src/tests.rs::toc_placeholder_is_real_content_and_headings_after_it_are_marked`; `lmdv-render/src/tests.rs::toc_lists_the_headings_after_it`; nesting is drawn by indentation (`parts.rs:425-437`). |
| M-21 | P0 | done | `lmdv-md/src/tests.rs::tagfilter_shows_unsafe_tags_as_text` (`<title>` stays literal). |
| M-22 | P1 | done | `lmdv-md/src/tests.rs::unknown_tags_keep_their_children_as_text` (`<name>visible</name>` shows `visible`; an empty custom tag vanishes). |
| M-23 | P0 | done | `lmdv-md/src/tests.rs::html_attributes_are_filtered_and_ids_namespaced`, `::unsafe_urls_are_dropped`; security corpus cases `html-style`, `html-handler-attrs` (`crates/lmdv-md/tests/security.rs`). App-protocol schemes are inert in the native resolver. |
| M-24 | P0 | done | The image/media resolver drops the query and keeps the fragment as a hint across src/srcset/picture/video/audio; the 512-entry per-folder cache is unchanged. Poster/track remain the accepted media gap (OI-046). |
| M-25 | P1 | done | Block and inline missing images now fall back to the source address when alt is empty; inline `<img>`/`<picture>` and the localized “Image” label keep the block behavior (`lmdv-render::a_missing_inline_image_uses_its_source_when_alt_is_empty`). |
| M-26 | P2 | done | `srcset` candidates are filtered at parse time: network (`//host`), UNC/`\\` and Windows-drive candidates are dropped while local spellings and descriptors survive, in `<img>` and `<picture><source>` (`lmdv-md` mixed-candidate tests, renderer srcset-scale test). |
| M-27 | P1 | substitute | No Linux media backend: OI-046 and `FEATURE_PARITY.md` L-MEDIA — video/audio are a placeholder plus “Open externally”, so playback continuity cannot exist. |
| M-28 | P2 | substitute | Same accepted gap: no `<track>`/subtitle path on Linux (OI-046, L-MEDIA). |
| M-29 | P1 | done | `lmdv-render/src/tests.rs::a_picture_chooses_its_source_by_scheme`, `::scheme_only_images_hide_in_the_other_theme`; `image_visible`/`source_matches` recompute per theme, so no `data-media` rewrite is needed. |
| M-30 | P0 | done | A network path is refused unless it is on the document's own share; images from another share stay missing and links use the other-computer wording; absolute web URLs are unchanged. |
| M-31 | P0 | done | `lmdv-io/src/path_policy.rs::system_network_and_device_spellings_are_rejected`, `::outside_paths_need_the_opt_in`; `read_document` re-checks the canonical path (`read.rs:87-90`). Local `../` document links are permitted. |
| M-32 | P1 | done | `lmdv-render/src/tests.rs::every_top_level_block_has_its_source_key`, `::the_block_wire_sees_every_blocks_key_line_and_drawn_text`; `p2_tests.rs::a_line_target_scrolls_the_article_to_its_block`. |
| M-33 | P1 | done | Display math is a horizontally scrolling line (`block.rs:437-458`, `.scroll_x()`); `lmdv-render/src/tests.rs::display_math_in_a_paragraph_gets_its_own_line`. |
| M-34 | P0 | done | Rendering stays transactional: a failed parse keeps the last good model with a dismissible “Preview error: …” (E-05), and an invalid Mermaid only replaces that diagram (`lmdv-render` test). |
| L-01 | P0 | done | Heading/anchor jumps land 24 pt below the top plus the measured stuck table-header height; percent-encoded ids resolve, a missing target is silent, Back and the landing flash are unchanged (`nav_tests`, `links::`). |
| L-02 | P0 | done | `http`/`https`/`mailto`/`tel` open externally; unknown schemes are ignored silently; `javascript`/`vbscript`/`data`/`file` stay refused (`links::tests`). |
| L-03 | P0 | done | `lmdv_io::resolve_document_link` probes `rel` (file), then each Markdown extension, then `index`/`README`/`readme` with each extension, canonicalizes a hit and returns a directory for the folder-link path (`path_policy::document_links`, 6 tests). The app resolves non-Markdown spellings on a worker (`JobMsg::LinkResolved`, `links::` tests), keeps anchors through the strip/apply, and drops a delayed answer if the click's tab/document is no longer in front. An explicit `.md` spelling opens directly, so the fallback chain applies to extensionless links. |
| L-04 | P0 | done | `open_linked_document` compares the resolved path with the active path and only jumps the fragment (`links.rs:184-202`); `nav_tests.rs::a_same_document_anchor_link_jumps_and_records_the_place_left`. |
| L-05 | P1 | done | `browser_tests.rs::a_folder_link_reroots_the_pane_and_a_plain_file_falls_back`, `::a_failed_folder_link_with_no_previous_root_returns_to_the_empty_state`; folder links show the pane and open no tab. |
| L-06 | P1 | done | A resolved non-Markdown file with an image extension opens the full-window preview through the window image cache (`links::` + `lightbox::open_image_link`); a broken image falls back to L-07's honest notice, folders still root the pane, linked images may sit outside the document folder under L-07's policy, and focus returns to the link run. Tests cover decode, broken-image and folder fallback. |
| L-07 | P0 | done | An existing non-Markdown file never opens as text: the notice offers **Open** for the document/media/archive/web whitelist and **Show in folder** otherwise; scripts are only revealed (`links::` + `tasks`). |
| L-08 | P0 | done | Every local link is probed first; a missing target shows “Not found: <path>”, opens no tab and refreshes the listing (`links::an_unresolved_link_reports_not_found`). |
| L-09 | P0 | done | Pasted text shows the save-first notice with **Save as…**, which opens the dialog for that document; in-page anchors still work. |
| L-10 | P0 | done | Same-share rule in `lmdv-io` for resolve/read/write; another share's media stays missing and links show the other-computer notice; absolute URLs stay external. Linux canonicalizes `//host/share`; Windows keeps the UNC share spelling. |
| L-11 | P0 | partial | `LinkPlacement` + `WindowSpec::on_modifiers`: plain foreground, Ctrl background, Ctrl+Shift foreground, Shift new window, middle background (`links::`/`nav_tests` tests). Named gaps: plain opens a foreground tab rather than replacing the current tab's document (the app's tab-per-document model), Rune cannot suppress `on_link` after a middle-drag, and Shift loses the `#section` because `Launch` carries only the path/line. |
| L-12 | P2 | partial | Parse + renderer landed: `<map>`/`<area>`/`<svg>` and attributes are allowed, `<area>` has scaled rect/circle/poly hit boxes with role/label, `<svg>` draws as a labelled group, and `xlink:href`/`a` runs open their destination; the app area wire opens the link. Remaining: vector shape drawing, image-map geometry refinements and the right-click “Copy link address” menu (OI-195). |
| L-13 | P1 | done | `lmdv-md/src/tests.rs::legacy_ids_are_recorded_when_they_differ` asserts `resolve_heading("q%26a")`; `model.rs:90-105` checks legacy ids and anchors for every fragment jump. |
| L-14 | P0 | n/a | A Rune desktop window has no browsing context to navigate away; forms/SVG are not rendered, and every clickable destination goes through `classify_link`, so there is no page-level navigation to block. External schemes are covered by L-02. |
| L-15 | P1 | done | `lmdv_md::decode_url_path` decodes one layer, lossily, keeping stray `%` and splitting fragment before query; links, image resolution and lightbox copy/save use it. Encoded document and linked-image paths are tested, including `%23` in a name. |

## Partial and missing — proposed follow-ups

Each bullet is one row; the decision line says whether to fix it or record it.

### Editor

- **E-01** (P0, partial) — *Proposed OI: “Edit-mode fade-in and round-trip proof”* — Add a short opacity transition on enter/leave (Rune `Transition::opacity`) or accept the missing fade as cosmetic.
- **E-03** (P1, partial) — *Proposed OI: “Editor entry caret from the exact block and --line”* — Use the preview block at the reading top when it is measured, and carry an explicit `--line` into the caret.
- **E-06** (P1, partial) — *Proposed OI: “Source-keyed Mermaid reuse on preview refresh”* — Cache rendered diagrams by source+theme so unchanged diagrams are not laid out again.
- **E-07** (P0, partial) — *Existing OI-100/OI-101* — Close the exact rendered-line mapping and section-line reserve, or re-accept the approximations.
- **E-13** (P0, partial) — *Proposed OI: “Discard/Cancel wording for leaving edit”* — Align the prompt with E-13 (Discard / Cancel) or accept Save / Don’t save + Esc as the superset.
- **E-25** (P0, partial) — *Existing OI-061* — Recognize Setext headings in the format bar’s active state.
- **E-28** (P1, partial) — *Existing OI-082* — Anchor the slash panel to the caret and make replace+insert one undo step.
- **E-29** (P1, partial) — *Existing E-MULTI gap in FEATURE_PARITY* — Implement `Ctrl+D` next occurrence and add-cursor, or record the v1 gap explicitly.
- **E-30** (P1, partial) — *Existing OI-060* — Define comment syntax for unknown fence languages and indented code, or document the no-op.
- **E-31** (P1, partial) — *Proposed OI: “Block-mode go-to-line forms”* — Support `+N`, `−N`, percentages and `line:col` in block mode like the single field.
- **E-34** (P2, missing) — *Proposed OI: “Scroll past end in the source pane”* — Add tail space/virtual padding so the preview can follow to the document end.
- **E-35** (P1, partial) — *Existing OI-095* — Add padding→line fallback, stable keyless text identities and preview-height caret placement.
- **E-36** (P1, missing) — *Proposed OI: “Paste/drop images into assets/”* — Implement E-36 (assets naming, URL-encoded insert, Save-as-first for pasted documents).

### Markdown rendering

- **M-13** (P0, partial) — *Existing OI-048* — Key copy links by block (`copy-code:b{i}`) so every code block can be copied.
- **M-24** (P0, partial) — *Proposed OI: “Image URL query strings and asset scope”* — Strip `?…` before resolution and decide/record whether `../` assets are in scope; media remains OI-046.
- **M-25** (P1, partial) — *Proposed OI: “Inline missing-image alt falls back to src”* — Use the source address when an inline image has no alt, matching block images.
- **M-26** (P2, partial) — *Proposed OI: “Drop network srcset candidates at parse”* — Filter `//`/UNC candidates instead of leaving them to resolver refusal.
- **M-30** (P0, partial) — *Proposed OI: “Network-link wording and same-share exception”* — Use the specified “another computer” notice and allow a document’s own share.
- **M-34** (P0, partial) — *Proposed OI: “Transactional preview error containment”* — Catch a failed rebuild, keep the old article and show `msg.previewError`.

### Links and media

- **L-01** (P0, partial) — *Proposed OI: “Anchor landing inset and compensation”* — Use the 24 px inset with table/section-line compensation; decide whether a missing heading stays silent.
- **L-02** (P0, partial) — *Proposed OI: “tel: links and silent unknown schemes”* — Add `tel:` to external schemes and ignore unsupported schemes instead of notifying.
- **L-03** (P0, missing) — *Proposed OI: “Wiki-style link resolution”* — Probe `rel`, `rel.<md ext>`, `rel/index`, `rel/README`, `rel/readme` in order.
- **L-06** (P1, missing) — *Proposed OI: “Image file links open the preview”* — Decode-test the image and open the lightbox; fall through to L-07 on failure.
- **L-07** (P0, partial) — *Proposed OI: “Non-Markdown link actions”* — Add the notice plus **Open** (whitelist) / **Show in folder** and never execute scripts.
- **L-08** (P0, partial) — *Proposed OI: “Not-found link notice and refresh”* — Probe before opening, show `msg.notFound`, refresh recents/tree/search and do not navigate.
- **L-09** (P0, partial) — *Proposed OI: “Pasted-text link notice with Save as”* — Use `msg.pastedLink` with a **Save as…** action while keeping in-page anchors.
- **L-10** (P0, partial) — *Proposed OI: “Same-share network exception and notice”* — Permit the document’s own share and use the specified notice text.
- **L-11** (P0, missing) — *Proposed OI: “Link click placement modifiers”* — Wire Ctrl/Shift/middle placement (Rune `on_link_secondary` has the middle press; Ctrl/Shift still need pointer state).
- **L-12** (P2, missing) — *Proposed OI: “Image maps and SVG links”* — Support `area`/SVG anchors or record them as out of the HTML subset.
- **L-15** (P1, partial) — *Proposed OI: “Percent-decode relative link paths”* — Decode once before `local_target`, with the same lossy fallback as fragments.

## Substitutes and n/a (not counted as gaps)

- **E-14** — OI-072 / `FEATURE_PARITY.md` §4, accepted 2026-11-18: per-tab editor sessions remove the discard prompt.
- **E-32** — `FEATURE_PARITY.md` E-SPELL; OI-174/175 accepted: Linux Spellbook replaces WebView2, suggestions menu pending.
- **M-14** — OI-045 accepted: generic invalid-Mermaid text; native diagram family coverage is a substitute.
- **M-27/M-28** — OI-046 / `FEATURE_PARITY.md` L-MEDIA accepted: no Linux media backend.
- **E-38** — n/a: no lazy editor chunk exists.
- **L-14** — n/a: no browsing context exists to navigate away.
