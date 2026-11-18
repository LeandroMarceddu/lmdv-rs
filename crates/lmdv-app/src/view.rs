//! The window's view: toolbar band, tab strip, start page, the parsed reader,
//! the source-preview fallback and the Help/shortcuts dialog.
//!
//! Views are pure: they only read state and return elements. Every button's
//! handler dispatches a command or mutates [`WindowState`] for the next frame.

use lmdv_core::{DocStatus, Document, ScrollAnchor, ThemePref};
use lmdv_render::{DetailsState, RenderWires};
use rune::{
    Align, Color, Cursor, Cx, El, GrowthSpec, Phase, PointerButton, PointerPress, ScrollSpec, TextStyle, Tx, col, el,
    icon, row, text, text_input,
};
use rune_kit::shell::{SplitView, TabItem, TabItemStatus, TabOrientation, TabStrip, Toolbar};
use rune_kit::syntax::SyntaxTheme;
use rune_kit::{
    Button, ButtonVariant, ControlSize, Dialog, IconButton, IconButtonVariant, Kit, Menu, MenuItem, Placement,
};

use crate::commands::{COMMANDS, lookup_command};
use crate::edit_ui::{FindField, GotoLine, SlashMenu};
use crate::editor::{NARROW_SPLIT_WIDTH, SPLIT_DEFAULT};
use crate::images::WindowImages;
use crate::math::WindowMath;
use crate::reader_find::{READER_FIND_BAND, READER_FIND_QUERY_KEY};
use crate::reading_meta::{TAB_META_GAP, TAB_META_TRAILING};
use crate::window::{HelpView, Mode, Notice, Tab, TabSource, WindowState, parse_tab_key, tab_key};
use crate::{icons, outline, reader};

/// Source lines drawn before the preview stops. The real renderer virtualizes
/// in P2; a cap keeps the placeholder honest for large files.
pub const PREVIEW_MAX_LINES: usize = 300;
/// Source bytes shaped before the preview stops, so one enormous line cannot
/// freeze the first frame.
pub const PREVIEW_MAX_BYTES: usize = 256 * 1024;

/// The editor header's line count and block count are catalog plurals.
const EDITOR_LINES: lmdv_i18n::PluralKey = lmdv_i18n::plural_key("editor.lines");
/// The block count's plural base.
const EDITOR_BLOCKS: lmdv_i18n::PluralKey = lmdv_i18n::plural_key("editor.blocks");

/// A localized count with its CLDR plural form and grouped number.
fn counted(key: lmdv_i18n::PluralKey, n: usize) -> String {
    let count = lmdv_i18n::number(n as i64);
    lmdv_i18n::render(lmdv_i18n::plural(key, n as u64), &[("n", &count)])
}

/// The window view.
pub(crate) fn view(state: &WindowState, cx: &mut Cx<WindowState>) -> El {
    // The window chrome, the article and every overlay draw from the same
    // scheme palette (J-01/OI-019): light, system and dark use the kit's neutral
    // palettes, sepia and black their original tokens.
    let kit = crate::appearance::palette(state.shared().prefs().theme, cx.system_dark());
    // Uncoloured text, icons and text fields take the scheme's ink, and their
    // keyboard focus rings its accent (the original's 2 px outline), so the
    // editor and every bar's fields stay readable in sepia and black too.
    cx.foreground(kit.c.text_base);
    cx.focus_ring(2.0, 1.5, kit.c.border_focus);
    // The panes draw at the side-panel text size; everything else keeps the
    // window's type scale.
    let pane_kit = crate::settings::pane_kit(&kit, state.shared().prefs().reader.side_size);
    let mut layer = Vec::new();
    // The table preview plans its column widths when it opens; this keeps the
    // last window body available to the command handler (V-18).
    if let Some(size) = cx.layout_size("window-body") {
        state.tables.viewport.set(size);
    }

    // The torn tab's ghost goes in first, under the menus and dialogs.
    let main = main_body(state, cx, &kit, &mut layer);
    let share = share_menu(state, &kit, &mut layer);
    // The panes read the previous frame's layout (the browser's keyboard menu
    // follows its row), so build them before the split mounts them.
    let sidebar = crate::browser::pane(state, cx, &pane_kit);
    let inspector = outline::pane(state, cx, &pane_kit);
    let shell = SplitView::new(main)
        .sidebar(&state.left, |state: &mut WindowState| &mut state.left, sidebar)
        .inspector(&state.right, |state: &mut WindowState| &mut state.right, inspector)
        .toolbar(toolbar(state, share, &kit))
        .titlebar_inset(cx.titlebar_inset())
        .frame_inset(8.0)
        .toolbar_in_panel()
        .hide_sidebar_toggle()
        .reduced_motion(cx.is_reduced_motion())
        .build(&kit)
        // O-14: the mouse's side buttons navigate over the whole window, as
        // they do in a browser. Only elements that opt in hear a non-primary
        // button, so this handler never takes a plain click; `role("group")`
        // puts back the role a press handler would otherwise turn into
        // `button`.
        .pointer_buttons([PointerButton::Back, PointerButton::Forward])
        .on_press_tx(|state: &mut WindowState, tx: &mut Tx| {
            state.nav_pointer_button(tx.button(), tx);
        })
        .role("group");

    // T-16's card floats under its tab, below every menu and dialog.
    crate::tab_card::card(state, &kit, &mut layer);
    crate::browser::context_menu(state, &kit, &mut layer);
    crate::help::full_help_dialog(state, cx, &kit, &mut layer);
    help_dialog(state, cx, &kit, &mut layer);
    editor_dialog(state, &kit, &mut layer);
    block_context_menu(state, &kit, &mut layer);
    tab_context_menu(state, &kit, &mut layer);
    crate::table_ui::preview_overlay(state, cx, &kit, &mut layer);
    crate::lightbox::overlay(state, cx, &kit, &mut layer);
    crate::palette_ui::overlay(state, cx, &kit, &mut layer);
    crate::settings_ui::overlay(state, cx, &kit, &mut layer);
    // A-08: Escape and Tab close a menu and hand focus back to the tree. Rune's
    // menus take Escape themselves when they are modal; F-12's is not (its Tab
    // must close too), so the window handles both here. Every other key falls
    // through untouched.
    rune_kit::root(shell, layer).on_key(|state: &mut WindowState, event, tx| {
        if state.browser_menu().is_some() && matches!(event.key, rune::Key::Escape | rune::Key::Tab) {
            state.close_browser_menu();
            tx.focus(crate::browser::BROWSER_TREE_KEY, false);
            return true;
        }
        false
    })
}

/// The preview block menu (E-04): “Edit from here” at the pointer, plus
/// Cancel. `rune_kit::context_menu` owns the modal's Escape/outside dismissal
/// and returns focus to its anchor (a zero-size element in the layer).
fn block_context_menu(state: &WindowState, kit: &Kit, layer: &mut Vec<El>) {
    let open = state.block_menu().cloned();
    let point = open.as_ref().map(|menu| menu.point);
    let trigger = el().key(crate::preview_nav::BLOCK_MENU_ANCHOR).absolute(0.0, 0.0).size(0.0, 0.0);
    let mut menu = rune_kit::context_menu("block-menu", trigger, point);
    if let Some(block) = open.as_ref().map(|menu| menu.block.clone()) {
        // V-01: a block that draws a picture or diagram offers the preview
        // before the source-navigation item. Other blocks keep E-04's menu.
        if state.media_for_block(&block.key).is_some() {
            let media_block = block.key.clone();
            menu = menu.item(MenuItem::new(lmdv_i18n::t!("zoom.open")).on_select_tx(
                move |state: &mut WindowState, tx: &mut Tx| {
                    state.close_block_menu();
                    state.open_media_block(&media_block, tx);
                },
            ));
        }
        menu = menu.item(MenuItem::new(lmdv_i18n::t!("ctx.editHere")).on_select_tx(
            move |state: &mut WindowState, tx: &mut Tx| {
                state.close_block_menu();
                state.edit_from_here(&block, tx);
            },
        ));
    }
    let menu = menu
        .item(MenuItem::new(lmdv_i18n::t!("msg.cancel")).on_select(|state: &mut WindowState| state.close_block_menu()))
        .on_close(|state: &mut WindowState| state.close_block_menu());
    let trigger = menu.build(kit, layer);
    layer.push(trigger);
}

/// The tab's context menu (T-21/T-22's menu path) and the “Move tab to
/// window…” chooser. Both are P4-02b's Wayland-safe substitute for dragging a
/// tab onto another window's strip; the drop itself is P4-02c.
fn tab_context_menu(state: &WindowState, kit: &Kit, layer: &mut Vec<El>) {
    let Some(open) = state.tab_menu() else { return };
    let trigger = el().key("tab-menu-anchor").absolute(0.0, 0.0).size(0.0, 0.0);
    let mut menu = rune_kit::context_menu("tab-menu", trigger, Some(open.point));
    if open.chooser {
        menu = menu.group_label(lmdv_i18n::t!("tab.moveGroup"));
        let targets = state.window_targets();
        if targets.is_empty() {
            menu = menu.item(MenuItem::new(lmdv_i18n::t!("tab.noOtherWindows")).disabled(true));
        } else {
            for target in targets {
                let tab = open.tab;
                let target_id = target.id;
                menu = menu.item(MenuItem::new(target.title).on_select(move |state: &mut WindowState| {
                    state.close_tab_menu();
                    if let Err(error) = state.move_tab_to_window(tab, target_id) {
                        state.notice(error);
                    }
                }));
            }
        }
        menu = menu.separator().item(
            MenuItem::new(lmdv_i18n::t!("msg.cancel")).on_select(|state: &mut WindowState| state.close_tab_menu()),
        );
    } else {
        let tab = open.tab;
        let point = open.point;
        let Some(current) = state.tab(tab) else {
            // The tab left between opening the menu and drawing it.
            let menu = menu.min_width(280.0).on_close(|state: &mut WindowState| state.close_tab_menu());
            let trigger = menu.build(kit, layer);
            layer.push(trigger);
            return;
        };
        let movable = state.can_move_tab(tab);
        let has_targets = !state.window_targets().is_empty();
        let several = state.tabs().len() > 1;
        let last = state.tabs().last().map(|tab| tab.id) == Some(tab);
        let path = current.document().and_then(|doc| doc.key.borrow().path().map(std::path::Path::to_path_buf));
        let pasted =
            current.document().is_some_and(|doc| matches!(*doc.key.borrow(), lmdv_core::DocumentKey::Untitled(_)));
        let can_reopen = state.can_reopen_tab();
        // First, as in a browser (O-14): the places this window's active tab
        // can go back or forward to. They act on the window, like the keys.
        let can_back = state.nav_can_back();
        let can_forward = state.nav_can_forward();
        menu = menu
            .item(MenuItem::new(lmdv_i18n::t!("ctx.back")).shortcut("Alt+\u{2190}").disabled(!can_back).on_select_tx(
                |state: &mut WindowState, tx: &mut Tx| {
                    state.close_tab_menu();
                    state.nav_back(tx);
                },
            ))
            .item(
                MenuItem::new(lmdv_i18n::t!("ctx.forward"))
                    .shortcut("Alt+\u{2192}")
                    .disabled(!can_forward)
                    .on_select_tx(|state: &mut WindowState, tx: &mut Tx| {
                        state.close_tab_menu();
                        state.nav_forward(tx);
                    }),
            )
            .separator()
            // T-18: a new tab, then the tab's own document actions, each
            // present only for the tab kind that has it. T-04: the menu's New
            // tab opens after the tab it was opened on (`Ctrl+T` appends).
            .item(MenuItem::new(lmdv_i18n::t!("tab.newTab")).icon(&icons::PLUS).shortcut("Ctrl+T").on_select_tx(
                move |state: &mut WindowState, tx: &mut Tx| {
                    state.close_tab_menu();
                    state.new_start_tab_after(tab);
                    state.reveal_active_tab(tx);
                },
            ));
        if several {
            menu = menu.item(
                MenuItem::new(lmdv_i18n::t!("tab.moveToWindow")).icon(&icons::WINDOW).disabled(!movable).on_select(
                    move |state: &mut WindowState| {
                        state.close_tab_menu();
                        state.move_tab_to_new_window(tab);
                    },
                ),
            );
        }
        menu = menu.item(
            MenuItem::new(lmdv_i18n::t!("tab.moveToWindowMenu")).disabled(!movable || !has_targets).on_select(
                move |state: &mut WindowState| {
                    state.show_tab_window_menu(tab, Some(point));
                },
            ),
        );
        if path.is_some() || pasted {
            menu = menu.separator();
            if path.is_some() {
                menu = menu
                    .item(MenuItem::new(lmdv_i18n::t!("file.copyPath")).on_select_tx(
                        move |state: &mut WindowState, tx: &mut Tx| {
                            state.close_tab_menu();
                            state.copy_tab_path(tab, tx);
                        },
                    ))
                    .item(MenuItem::new(lmdv_i18n::t!("file.reveal")).icon(&icons::FOLDER).on_select(
                        move |state: &mut WindowState| {
                            state.close_tab_menu();
                            state.reveal_tab(tab);
                        },
                    ));
            }
            if pasted {
                menu = menu.item(MenuItem::new(lmdv_i18n::t!("tab.saveAs")).shortcut("Ctrl+S").on_select_tx(
                    move |state: &mut WindowState, tx: &mut Tx| {
                        state.close_tab_menu();
                        if state.activate_tab(tab) {
                            state.request_save_as_active(tx);
                        }
                    },
                ));
            }
        }
        menu = menu.separator().item(MenuItem::new(lmdv_i18n::t!("tab.closeTab")).shortcut("Ctrl+W").on_select_tx(
            move |state: &mut WindowState, tx: &mut Tx| {
                state.close_tab_menu();
                state.request_close_tab(tab);
                state.apply_pending_scroll(tx);
            },
        ));
        if several {
            menu = menu.item(MenuItem::new(lmdv_i18n::t!("tab.closeOthers")).on_select_tx(
                move |state: &mut WindowState, tx: &mut Tx| {
                    state.close_tab_menu();
                    state.close_other_tabs(tab);
                    state.apply_pending_scroll(tx);
                },
            ));
        }
        if !last {
            menu = menu.item(MenuItem::new(lmdv_i18n::t!("tab.closeRight")).on_select_tx(
                move |state: &mut WindowState, tx: &mut Tx| {
                    state.close_tab_menu();
                    state.close_tabs_to_the_right(tab);
                    state.apply_pending_scroll(tx);
                },
            ));
        }
        if can_reopen {
            menu = menu.separator().item(
                MenuItem::new(lmdv_i18n::t!("tab.reopen")).shortcut("Ctrl+Shift+T").on_select_tx(
                    |state: &mut WindowState, tx: &mut Tx| {
                        state.close_tab_menu();
                        if state.reopen_tab() {
                            state.apply_pending_scroll(tx);
                        }
                    },
                ),
            );
        }
    }
    let menu = menu.min_width(280.0).on_close(|state: &mut WindowState| state.close_tab_menu());
    let trigger = menu.build(kit, layer);
    layer.push(trigger);
}

fn main_body(state: &WindowState, cx: &mut Cx<WindowState>, kit: &Kit, layer: &mut Vec<El>) -> El {
    let mut body = col()
        .key("window-body")
        .w_full()
        .h_full()
        .min_h(0.0)
        .child(tab_band(state, cx, kit, layer))
        .child(el().key("tab-separator").w_full().h(1.0).fill(kit.c.border_muted))
        .child(content(state, cx, kit).grow(1.0).min_h(0.0));
    if let Some(notice) = &state.ui.notice {
        body = body.child(notice_bar(notice, kit));
    }
    body
}

/// The Share menu under its toolbar button (X-01, X-06).
fn share_menu(state: &WindowState, kit: &Kit, layer: &mut Vec<El>) -> El {
    let icon = if state.share.flashing() { &icons::CHECK } else { &icons::SHARE };
    let button = IconButton::new(icon)
        .variant(IconButtonVariant::GhostMuted)
        .expanded(state.share.menu_open)
        .disabled(state.active_document_id().is_none())
        .label(lmdv_i18n::t!("toolbar.share"))
        .help(lmdv_i18n::t!("keys.share"))
        .shortcut("Ctrl+Shift+E")
        .on_press_tx(|state: &mut WindowState, tx: &mut Tx| {
            state.run_command("share.menu", tx);
        })
        .build(kit)
        .key(crate::share::TOOLBAR_KEY);
    let mut menu = Menu::new(crate::share::MENU_KEY)
        .trigger(button)
        .open(state.share.menu_open)
        .min_width(280.0)
        .placement(Placement::BottomEnd)
        .on_close(|state: &mut WindowState| state.close_share_menu());
    for item in crate::share::SHARE_ITEMS {
        let command = lookup_command(item.command);
        let enabled = command.is_some_and(|command| (command.enabled)(state));
        let checked = command.and_then(|command| command.checked).map(|checked| checked(state));
        let label = lmdv_i18n::text(lmdv_i18n::key(item.label)).to_owned();
        let hint = lmdv_i18n::text(lmdv_i18n::key(item.hint)).to_owned();
        let mut row = MenuItem::new(label).description(hint).disabled(!enabled);
        if item.checkbox {
            row = row.checked(checked.unwrap_or(false)).on_select_tx(|state: &mut WindowState, tx: &mut Tx| {
                let _ = state.run_command("share.toggle-break-h1", tx);
            });
        } else if enabled {
            let id = item.command;
            row = row.on_select_tx(move |state: &mut WindowState, tx: &mut Tx| {
                state.close_share_menu();
                let _ = state.run_command(id, tx);
            });
        }
        menu = menu.item(row);
    }
    menu.build(kit, layer)
}

fn toolbar(state: &WindowState, share: El, kit: &Kit) -> Toolbar {
    let c = &kit.c;
    let title = state.active_tab().map(Tab::full_title).unwrap_or_else(|| "lmdv".to_owned());
    let open = IconButton::new(&icons::FOLDER)
        .variant(IconButtonVariant::GhostMuted)
        .label(lmdv_i18n::t!("toolbar.open"))
        .help(lmdv_i18n::t!("toolbar.openHint"))
        .shortcut("Ctrl+O")
        .on_press_tx(|state: &mut WindowState, tx: &mut Tx| {
            state.run_command("file.open", tx);
        })
        .build(kit)
        .key("open-file-toolbar");
    let browser = IconButton::new(&rune_kit::shell::SIDEBAR)
        .variant(IconButtonVariant::GhostMuted)
        .expanded(state.left.is_open())
        .label(lmdv_i18n::t!("toolbar.tree"))
        .help(lmdv_i18n::t!("toolbar.treeHint"))
        .shortcut("Ctrl+B")
        .on_press_tx(|state: &mut WindowState, tx: &mut Tx| {
            state.run_command("view.toggle-browser", tx);
        })
        .build(kit)
        .key("toggle-browser");
    let outline = IconButton::new(&rune_kit::shell::INSPECTOR)
        .variant(IconButtonVariant::GhostMuted)
        .expanded(state.right.is_open())
        .label(lmdv_i18n::t!("toolbar.toc"))
        .help(lmdv_i18n::t!("toolbar.tocHint"))
        .shortcut("Ctrl+I")
        .on_press_tx(|state: &mut WindowState, tx: &mut Tx| {
            state.run_command("view.toggle-outline", tx);
        })
        .build(kit)
        .key("toggle-outline");
    let help = IconButton::new(&icons::HELP)
        .variant(IconButtonVariant::GhostMuted)
        .label(lmdv_i18n::t!("toolbar.help"))
        .help(lmdv_i18n::t!("cmd.keys"))
        .shortcut("F1")
        .on_press_tx(|state: &mut WindowState, tx: &mut Tx| {
            state.run_command("app.help", tx);
        })
        .build(kit)
        .key("help");
    let settings = IconButton::new(&icons::SETTINGS)
        .variant(IconButtonVariant::GhostMuted)
        .expanded(state.settings_is_open())
        .label(lmdv_i18n::t!("toolbar.settings"))
        .help(lmdv_i18n::t!("toolbar.settingsHint"))
        .shortcut("Ctrl+,")
        .on_press_tx(|state: &mut WindowState, tx: &mut Tx| {
            state.run_command("app.settings", tx);
        })
        .build(kit)
        .key(crate::settings::SETTINGS_TOOLBAR);
    let editing = state.active_tab().is_some_and(|tab| tab.view.mode == Mode::Editor);
    let edit = IconButton::new(if editing { &icons::BOOK } else { &icons::PENCIL })
        .variant(IconButtonVariant::GhostMuted)
        .expanded(editing)
        .label(if editing { lmdv_i18n::t!("toolbar.read") } else { lmdv_i18n::t!("toolbar.edit") })
        .help(if editing { lmdv_i18n::t!("toolbar.readHint") } else { lmdv_i18n::t!("toolbar.editHint") })
        .shortcut("Ctrl+E")
        .on_press_tx(|state: &mut WindowState, tx: &mut Tx| {
            state.run_command("edit.toggle", tx);
        })
        .build(kit)
        .key("edit-toggle");
    let preview = IconButton::new(&icons::PREVIEW)
        .variant(IconButtonVariant::GhostMuted)
        .expanded(state.editor.preview_visible)
        .label(lmdv_i18n::t!("toolbar.preview"))
        .help(lmdv_i18n::t!("toolbar.previewHint"))
        .on_press_tx(|state: &mut WindowState, tx: &mut Tx| {
            let _ = state.run_command("view.toggle-preview", tx);
        })
        .build(kit)
        .key(crate::edit_ui::PREVIEW_TOGGLE_KEY);

    let mut bar = Toolbar::new()
        .leading(text("lmdv").font(TextStyle::new(14.0, 600.0)).color(c.text_muted))
        .title(title)
        .trailing(open)
        .trailing(edit)
        .trailing(browser)
        .trailing(outline)
        .trailing(share)
        .trailing(settings)
        .trailing(help);
    if editing {
        bar = bar.trailing(preview);
    }
    if state.saved_flash() {
        bar = bar.trailing(text(lmdv_i18n::t!("msg.saved")).font(kit.ty.small).color(c.success.fg).key("saved-flash"));
    }
    bar
}

fn tab_band(state: &WindowState, cx: &Cx<WindowState>, kit: &Kit, layer: &mut Vec<El>) -> El {
    let ids: Vec<String> = state.tabs().iter().map(|tab| tab_key(tab.id)).collect();
    let one_tab = state.tabs().len() == 1;
    let active = state.active_tab().map(|tab| tab_key(tab.id));
    let active_id = state.active_tab().map(|tab| tab.id);
    let items: Vec<TabItem<'_, WindowState>> = state
        .tabs()
        .iter()
        .zip(ids.iter())
        .map(|(tab, id)| {
            let tab_id = tab.id;
            let full = tab.full_title();
            // R-05: the reading time follows the active tab's name, in full while
            // it is the window's only tab, short among several, and only while
            // the strip has room to spare (T-35).
            let is_active = Some(tab.id) == active_id;
            let stats = tab.view.reading;
            // The card and the accessible description keep the full time while
            // editing, when the inline one is hidden (R-05); a dirty tab's card
            // says so instead (T-16), as the original's does.
            let described = if is_active && !tab.is_start() {
                if tab.view.dirty {
                    Some(lmdv_i18n::t!("tab.unsaved").to_owned())
                } else {
                    stats.map(|stats| stats.description())
                }
            } else {
                None
            };
            // A tab in hand, or while a pointer close froze the tabs' widths,
            // shows no time; the card keeps it (T-35), and the strip does not
            // move under the pointer.
            let inline = stats.filter(|_| {
                is_active
                    && tab.view.mode == Mode::Viewer
                    && !tab.is_start()
                    && state.drag.dragging().is_none()
                    && state.drag.width_lock.is_none()
            });
            let font = TextStyle { weight: kit.ty.ui_medium.weight, ..kit.ty.ui_compact };
            let meta = inline.map(|stats| if one_tab { stats.full() } else { stats.short() });
            let strip_fits = meta.as_deref().is_some_and(|meta| {
                let meta_width = rune::text::measure(meta, &[], font, None).0 + TAB_META_GAP;
                let on = tab.view.reading_meta.get();
                match (cx.layout_size("tabs"), cx.layout_size("tabs-list")) {
                    (Some(strip), Some(list)) => {
                        crate::reading_meta::tab_meta_fits(strip[0], list[0], TAB_META_TRAILING, meta_width, on)
                    }
                    // The first frame has no layout yet: assume the window is
                    // wide, which the next frame corrects before it is drawn.
                    _ => true,
                }
            });
            let drawn = match (meta, strip_fits) {
                (Some(meta), true) => {
                    let title = tab.title();
                    let space = cx.layout_size(&format!("t-{}/pill/title", tab_key(tab.id))).map(|size| size[0]);
                    let label = match space {
                        Some(space) => crate::reading_meta::fit_tab_label(&title, &meta, space, font),
                        // No layout yet: the time comes with the next frame's width.
                        None => Some(format!("{title}  {meta}")),
                    };
                    if is_active {
                        tab.view.reading_meta.set(label.is_some());
                    }
                    label.unwrap_or(title)
                }
                _ => {
                    if is_active {
                        tab.view.reading_meta.set(false);
                    }
                    tab.title()
                }
            };
            let item = TabItem::new(id.clone(), drawn).closable(true).status(if tab.view.dirty {
                Some(TabItemStatus::Attention)
            } else {
                None
            });
            let item = match &tab.source {
                TabSource::Start => item.icon(&icons::HOME),
                TabSource::Document(_) => {
                    item.icon(if tab.view.mode == Mode::Editor { &icons::PENCIL } else { &icons::DOCUMENT })
                }
            };
            item.decorate(move |el, _| {
                let el = el.label(full).on_secondary_press(move |state: &mut WindowState, point, _| {
                    state.show_tab_menu(tab_id, point);
                });
                match described {
                    Some(described) => el.description(described.clone()).help(described),
                    None => el,
                }
            })
        })
        .collect();
    let plus = IconButton::new(&icons::PLUS)
        .variant(IconButtonVariant::GhostMuted)
        .control_size(ControlSize::Regular)
        .label(lmdv_i18n::t!("tab.newTabHint"))
        .help(lmdv_i18n::t!("tab.newTab"))
        .shortcut("Ctrl+T")
        .on_press_tx(|state: &mut WindowState, tx: &mut Tx| {
            state.run_command("file.new-tab", tx);
        })
        .build(kit)
        .key("new-tab");
    // T-17: the ⌄ list follows the last tab once the tabs pass the strip's
    // viewport, measured from the rows the kit laid out last frame. Its menu is
    // part of the trailing action so it anchors under the button; the plus
    // stays after it and keeps its place.
    let overflow = crate::tab_list::strip_overflows(
        cx.layout_size(crate::tab_list::TAB_STRIP_SCROLL_KEY),
        cx.layout_size(crate::tab_list::TAB_STRIP_LIST_KEY),
    );
    let trailing = if overflow {
        row().key("tab-strip-actions").gap(6.0).center_y().child(tab_list_button(state, kit, layer)).child(plus)
    } else {
        plus
    };

    let strip = TabStrip::new("tabs", TabOrientation::Horizontal)
        .tabs(items)
        .active(active.as_deref())
        .drag_state(&state.drag, |state: &mut WindowState| &mut state.drag)
        // X11 only: Wayland and headless strips keep no tear-out and the tab
        // menu is the substitute (T-20/T-21).
        .tear_out(state.tear_out_spec())
        .close_icon(kit.glyph.close)
        .trailing_action(trailing)
        .label(lmdv_i18n::t!("tab.list"))
        .fill(kit.c.bg_base)
        .on_select(|state: &mut WindowState, id: &str, tx: &mut Tx| {
            if let Some(tab) = parse_tab_key(id) {
                state.activate_tab_with_tx(tab, tx);
            }
        })
        // T-16: after a 550 ms rest (at once from tab to tab) a floating card
        // names the document, its folder and its time or unsaved state.
        .on_hover(|state: &mut WindowState, id: &str, inside, tx| state.tab_hover_changed(id, inside, tx))
        .on_close(|state: &mut WindowState, id: &str, tx: &mut Tx| {
            if let Some(tab) = parse_tab_key(id) {
                state.request_close_tab(tab);
                state.apply_pending_scroll(tx);
            }
        })
        .on_reorder(|state: &mut WindowState, reorder, _| {
            state.reorder_tab(reorder.from, reorder.to);
        })
        .on_tear_move(|state: &mut WindowState, tear, _| {
            state.handle_tear_move(tear);
        })
        .on_tear_out(|state: &mut WindowState, tear, _| {
            state.handle_tear_out(tear);
        })
        // A torn tab leaves the 28-point strip band, which the strip's scroll
        // viewport clips; the layered ghost floats over the window instead.
        .build_layered(kit, layer);

    row().key("tab-band").w_full().h(40.0).px(8.0).center_y().child(strip.grow(1.0))
}

/// The strip's ⌄ control and its all-tabs list (T-17): every tab with the
/// front one marked ● and a file's folder as its hint, then New tab.
///
/// The list replaces the kit's own tab strip menu for overflow only; the rest
/// of the strip (fades, wheel, drag) stays the kit's. Choosing a tab closes the
/// list and brings that tab forward; New tab appends a start page, as the
/// original's list does (`src/tabs/strip.ts:796-815`).
fn tab_list_button(state: &WindowState, kit: &Kit, layer: &mut Vec<El>) -> El {
    let button = IconButton::new(&icons::CHEVRON_DOWN)
        .variant(IconButtonVariant::GhostMuted)
        .control_size(ControlSize::Regular)
        .expanded(state.tab_list_is_open())
        .label(lmdv_i18n::t!("tab.allTabs"))
        .help(lmdv_i18n::t!("tab.allTabs"))
        .on_press(|state: &mut WindowState| state.toggle_tab_list())
        .build(kit)
        .key(crate::tab_list::TAB_LIST_BUTTON_KEY);
    let mut menu = Menu::new(crate::tab_list::TAB_LIST_MENU_KEY)
        .trigger(button)
        .open(state.tab_list_is_open())
        .min_width(280.0)
        .placement(Placement::BottomEnd)
        .on_close(|state: &mut WindowState| state.close_tab_list());
    let active = state.active_tab().map(|tab| tab.id);
    for tab in state.tabs() {
        let id = tab.id;
        let mut item = MenuItem::new(tab.full_title()).icon(match &tab.source {
            TabSource::Start => &icons::HOME,
            TabSource::Document(_) => &icons::DOCUMENT,
        });
        if let Some(folder) = tab.document().and_then(|document| {
            document.key.borrow().path().and_then(std::path::Path::parent).map(|parent| parent.to_path_buf())
        }) {
            item = item.description(folder.display().to_string());
        }
        if Some(id) == active {
            item = item.shortcut("\u{25cf}");
        }
        menu = menu.item(item.on_select_tx(move |state: &mut WindowState, tx: &mut Tx| {
            state.close_tab_list();
            state.activate_tab_with_tx(id, tx);
            state.reveal_active_tab(tx);
        }));
    }
    let menu = menu.separator().item(
        MenuItem::new(lmdv_i18n::t!("tab.newTab")).icon(&icons::PLUS).shortcut("Ctrl+T").on_select_tx(
            |state: &mut WindowState, tx: &mut Tx| {
                state.close_tab_list();
                state.new_start_tab();
                state.reveal_active_tab(tx);
            },
        ),
    );
    menu.build(kit, layer)
}

fn content(state: &WindowState, cx: &mut Cx<WindowState>, kit: &Kit) -> El {
    match state.active_tab() {
        Some(tab @ Tab { source: TabSource::Document(_), .. }) => document_view(tab, state, cx, kit),
        Some(Tab { source: TabSource::Start, .. }) | None => start_page(state, kit),
    }
}

fn start_page(state: &WindowState, kit: &Kit) -> El {
    let c = &kit.c;
    let open = Button::new(lmdv_i18n::t!("empty.open"))
        .icon(&icons::FOLDER)
        .variant(ButtonVariant::Outline)
        .on_press_tx(|state: &mut WindowState, tx: &mut Tx| {
            state.run_command("file.open", tx);
        })
        .build(kit)
        .key("open-file");

    let page = col()
        .key("start-page")
        .w_full()
        .pt(56.0)
        .pb(32.0)
        .center()
        .gap(10.0)
        .child(text("lmdv").font(TextStyle::new(32.0, 650.0)).color(c.text_base))
        .child(text(lmdv_i18n::t!("start.tagline")).font(kit.ty.body).color(c.text_muted))
        .child(row().h(8.0))
        .child(open)
        .child(recents_panel(state, kit))
        .child(
            text(lmdv_i18n::t!("start.version", "version" => env!("CARGO_PKG_VERSION")))
                .font(kit.ty.small)
                .color(c.text_faint),
        )
        .child(row().h(4.0));

    col().key("start-page-scroll").w_full().h_full().scroll(ScrollSpec { stick_bottom: false }).child(page)
}

fn recents_panel(state: &WindowState, kit: &Kit) -> El {
    let c = &kit.c;
    let recents = state.shared().recents();
    let mut panel = col().key("recents").w_full().max_w(560.0).gap(4.0);
    if recents.is_empty() {
        panel = panel.child(
            text(lmdv_i18n::t!("start.noRecents"))
                .key("recents-empty")
                .font(kit.ty.small)
                .color(c.text_faint)
                .w_full()
                .align(Align::Center),
        );
        return panel;
    }

    panel = panel.child(
        text(lmdv_i18n::t!("start.recentFiles")).key("recents-title").font(kit.ty.small_medium).color(c.text_muted),
    );
    for (index, recent) in recents.iter().enumerate() {
        let path = recent.path.clone();
        let accessible = path.display().to_string();
        let mut line = row()
            .key(format!("recent-{index}"))
            .w_full()
            .gap(10.0)
            .center_y()
            .padding(8.0)
            .radius(kit.radius.md)
            .fill(c.layer_01)
            .border(1.0, c.border_muted)
            .role("button")
            .label(accessible)
            .focusable(true)
            .cursor(rune::Cursor::Pointer)
            .on_press_tx(move |state: &mut WindowState, tx: &mut Tx| {
                state.open_path(path.clone());
                // The new tab must draw at its own place on this frame, not at
                // the offset the shared article viewport still carries.
                state.apply_pending_scroll(tx);
            })
            .child(icon(&icons::DOCUMENT, 16.0).color(c.icon_muted))
            .child(text(recent.name.clone()).font(kit.ty.ui_medium).color(c.text_base).truncate().grow(1.0));
        if !recent.dir.as_os_str().is_empty() {
            line = line.child(
                text(recent.dir.display().to_string()).font(kit.ty.small).color(c.text_faint).truncate().shrink(),
            );
        }
        if let Some(relative) = relative_time(recent.opened_at, state.ui.now_unix_ms) {
            line = line.child(text(relative).font(kit.ty.small).color(c.text_faint).shrink());
        }
        panel = panel.child(line);
    }
    panel
}

/// A compact age for a recent file: “just now”, “12 min”, “3 h”, “5 d”.
///
/// The units are catalog plurals (`rel.minutes.one`/`.other`, …), so each
/// locale picks its own CLDR form; the number uses the locale's grouping.
fn relative_time(opened_at: u64, now: u64) -> Option<String> {
    if opened_at == 0 || now < opened_at {
        return None;
    }
    const MINUTES: lmdv_i18n::PluralKey = lmdv_i18n::plural_key("rel.minutes");
    const HOURS: lmdv_i18n::PluralKey = lmdv_i18n::plural_key("rel.hours");
    const DAYS: lmdv_i18n::PluralKey = lmdv_i18n::plural_key("rel.days");
    let unit = |key: lmdv_i18n::PluralKey, n: u64| {
        let count = lmdv_i18n::number(n as i64);
        lmdv_i18n::render(lmdv_i18n::plural(key, n), &[("n", &count)])
    };
    let seconds = (now - opened_at) / 1000;
    Some(match seconds {
        0..=59 => lmdv_i18n::t!("rel.justNow").to_owned(),
        60..=3_599 => unit(MINUTES, seconds / 60),
        3_600..=86_399 => unit(HOURS, seconds / 3_600),
        _ => unit(DAYS, seconds / 86_400),
    })
}

fn document_view(tab: &Tab, state: &WindowState, cx: &mut Cx<WindowState>, kit: &Kit) -> El {
    let Some(doc) = tab.document() else { return start_page(state, kit) };
    let status = doc.status.borrow().clone();
    match status {
        DocStatus::Loading => status_card(kit, lmdv_i18n::t!("msg.loading"), lmdv_i18n::t!("msg.loadingDetail"), None),
        DocStatus::Error(message) => error_card(doc, &message, kit),
        DocStatus::Gone => gone_card(doc, kit),
        // The parsed article. A document whose first parse has not landed yet
        // shows a placeholder; a failed or oversized one keeps the source
        // preview (M-34). A tab in edit mode shows the source and its preview.
        DocStatus::Ready => {
            if tab.view.mode == Mode::Editor && tab.view.editor.is_some() {
                return editor_view(tab, doc, state, cx, kit);
            }
            match &tab.view.reader {
                Some(model) => reader_pane(doc, model, tab, state, cx, kit),
                None => match &tab.view.reader_error {
                    Some(reason) => source_preview(doc, kit, Some(reason)),
                    None => status_card(
                        kit,
                        lmdv_i18n::t!("editor.preparingDocument"),
                        lmdv_i18n::t!("editor.previewDetail"),
                        None,
                    ),
                },
            }
        }
    }
}

/// Edit mode: source on the left, live preview on the right, with a draggable
/// divider; below 760 pt the source alone, with the preview one toggle away
/// (E-02).
fn editor_view(tab: &Tab, doc: &lmdv_core::Document, state: &WindowState, cx: &mut Cx<WindowState>, kit: &Kit) -> El {
    let width = cx.layout_size("window-body").map_or(1100.0, |size| size[0]);
    let wide = width >= NARROW_SPLIT_WIDTH;
    if !wide {
        // Below 760 pt the source is alone; its header's Preview button swaps
        // in the live preview, which has a Source button back (E-02).
        if !(state.editor.preview_narrow && state.editor.preview_visible) {
            return col()
                .key("editor-single")
                .w_full()
                .h_full()
                .min_h(0.0)
                .child(source_editor(cx, tab, state, kit, true));
        }
        let preview: El = match &tab.view.reader {
            Some(model) => reader_article(doc, model, tab, state, cx, kit),
            None => {
                status_card(kit, lmdv_i18n::t!("editor.preparingPreview"), lmdv_i18n::t!("editor.previewDetail"), None)
            }
        };
        let header = row()
            .key("preview-only-header")
            .w_full()
            .px(16.0)
            .py(8.0)
            .gap(8.0)
            .center_y()
            .child(text(lmdv_i18n::t!("editor.preview")).font(kit.ty.small_medium).color(kit.c.text_muted))
            .child(row().grow(1.0))
            .child(
                Button::new(lmdv_i18n::t!("editor.source"))
                    .variant(ButtonVariant::Ghost)
                    .on_press(|state: &mut WindowState| state.hide_narrow_preview())
                    .build(kit)
                    .key("editor-source-open"),
            );
        return col()
            .key("editor-preview-only")
            .w_full()
            .h_full()
            .min_h(0.0)
            .child(header)
            .child(el().w_full().h(1.0).fill(kit.c.border_muted))
            .child(col().grow(1.0).min_h(0.0).child(preview));
    }
    let show_preview = state.editor.preview_visible;
    let source = source_editor(cx, tab, state, kit, false);
    if !show_preview {
        return col().key("editor-single").w_full().h_full().min_h(0.0).child(source);
    }

    let split = state.editor_split();
    let total = (width - 8.0).max(320.0);
    let divider = el()
        .key("editor-divider")
        .w(8.0)
        .h_full()
        .fill(kit.c.border_muted)
        .cursor(Cursor::ResizeColumn)
        .on_drag(move |state: &mut WindowState, drag: &mut rune::Drag, _: &mut Tx| match drag.phase {
            Phase::Down => drag.data = state.editor_split(),
            Phase::Move => state.set_editor_split(drag.data + drag.dx / total),
            Phase::Up => {}
        })
        .on_double_press(|state: &mut WindowState| state.set_editor_split(SPLIT_DEFAULT));

    let preview: El = match &tab.view.reader {
        Some(model) => reader_article(doc, model, tab, state, cx, kit),
        None => status_card(kit, lmdv_i18n::t!("editor.preparingPreview"), lmdv_i18n::t!("editor.previewDetail"), None),
    };
    row()
        .key("editor-split")
        .w_full()
        .h_full()
        .min_h(0.0)
        .child(source.grow(split).min_w(0.0))
        .child(divider)
        .child(col().key("preview-pane").grow(1.0 - split).min_w(0.0).h_full().child(preview))
}

/// The source field inside its scroll viewport (6.2).
fn source_editor(cx: &Cx<WindowState>, tab: &Tab, state: &WindowState, kit: &Kit, narrow: bool) -> El {
    let c = &kit.c;
    let Some(editor) = tab.view.editor.as_ref() else {
        return status_card(
            kit,
            lmdv_i18n::t!("editor.notAvailable"),
            lmdv_i18n::t!("editor.notAvailableDetail"),
            None,
        );
    };
    // E-07: measure where the first visible source line sits. A view without a
    // preview (narrow source-only) leaves the preview geometry empty.
    state.clear_preview_geometry();
    state.publish_editor_top(cx);
    let theme = SyntaxTheme::from_kit(kit);
    // E-37: the source is 87.5% of the reading size at a proportional line
    // height, in both the single field and the block fields. The Aa stepper
    // changes the preference; the next frame rebuilds the view and Rune
    // re-measures the field.
    let source_font = state.editor_metrics().field(kit.ty.mono);
    let caret = kit.c.text_accent;
    let caret_selection = kit.alpha(caret, 0.28);
    let block_mode = editor.block_mode_active();
    let field = if block_mode {
        el()
    } else {
        text_input(&editor.edit, source_font)
            .key("editor-source")
            .label(lmdv_i18n::t!("editor.markdownSource"))
            .caret(caret, caret_selection)
            .w_full()
            .spans(lmdv_edit::syntax::syntax_spans(&editor.edit, &theme, None))
            .on_edit(|state: &mut WindowState| state.active_editor_edit())
            .on_input_change(|state: &mut WindowState, _| state.editor_changed())
            .on_input_key(|state: &mut WindowState, event, tx| state.editor_key(event, tx))
            .on_submit(|state: &mut WindowState, _| state.editor_insert_newline())
    };

    let mut header = row()
        .key("editor-header")
        .w_full()
        .px(16.0)
        .py(8.0)
        .gap(8.0)
        .center_y()
        .child(text(lmdv_i18n::t!("editor.source")).font(kit.ty.small_medium).color(c.text_muted))
        .child(
            text(counted(EDITOR_LINES, editor.line_count())).key("editor-lines").font(kit.ty.small).color(c.text_faint),
        );
    if block_mode {
        let blocks = editor.block_editor().map_or(0, |blocks| blocks.blocks().len());
        header = header
            .child(
                text(lmdv_i18n::t!("editor.blockMode")).key("editor-block-mode").font(kit.ty.small).color(c.text_faint),
            )
            .child(
                text(counted(EDITOR_BLOCKS, blocks)).key("editor-block-count").font(kit.ty.small).color(c.text_faint),
            );
    }
    header = header.child(row().grow(1.0));
    let updating =
        tab.view.preview_pending || tab.document().is_some_and(|doc| tab.view.reader_revision != Some(doc.revision()));
    if updating {
        header = header.child(
            text(lmdv_i18n::t!("editor.previewPending")).key("preview-pending").font(kit.ty.small).color(c.text_faint),
        );
    }
    if tab.view.disk_changed {
        header = header.child(
            text(lmdv_i18n::t!("editor.diskChanged")).key("disk-changed").font(kit.ty.small).color(c.warning.fg),
        );
    }
    header = header.child(
        IconButton::new(&icons::SEARCH)
            .variant(IconButtonVariant::GhostMuted)
            .control_size(ControlSize::Small)
            .expanded(tab.view.find_bar.open)
            .label(lmdv_i18n::t!("cm.find"))
            .help(lmdv_i18n::t!("cmd.find"))
            .shortcut("Ctrl+F")
            .on_press_tx(|state: &mut WindowState, tx: &mut Tx| {
                state.run_command("edit.find", tx);
            })
            .build(kit)
            .key("editor-find"),
    );
    if narrow {
        header = header.child(
            IconButton::new(&icons::PREVIEW)
                .variant(IconButtonVariant::GhostMuted)
                .control_size(ControlSize::Small)
                .label(lmdv_i18n::t!("editor.showPreview"))
                .on_press(|state: &mut WindowState| state.show_narrow_preview())
                .build(kit)
                .key("editor-preview-open"),
        );
    } else {
        header = header.child(
            IconButton::new(&icons::PREVIEW)
                .variant(IconButtonVariant::GhostMuted)
                .control_size(ControlSize::Small)
                .expanded(state.editor.preview_visible)
                .label(lmdv_i18n::t!("toolbar.preview"))
                .on_press_tx(|state: &mut WindowState, tx: &mut Tx| {
                    let _ = state.run_command("view.toggle-preview", tx);
                })
                .build(kit)
                .key("editor-preview-toggle"),
        );
    }

    let mut pane = col().key("editor-pane").w_full().h_full().min_w(0.0).min_h(0.0).child(header);
    // P6-PREF: the Editor section's format-bar switch hides the bar (and its
    // rule) without leaving a gap. The keymap and slash menu keep working.
    if state.shared().prefs().editor.format_bar {
        pane = pane.child(el().w_full().h(1.0).fill(c.border_muted)).child(format_bar(tab, kit));
    }
    if tab.view.find_bar.open {
        pane = pane.child(find_bar(tab, state, kit));
    }
    if let Some(goto) = state.editor.goto_line.as_ref() {
        pane = pane.child(goto_line_bar(goto, kit));
    }
    if let Some(menu) = state.slash_menu() {
        pane = pane.child(slash_menu_view(kit, menu));
    }
    let body = if block_mode {
        // One virtualized field per blank-line block (ADR-016): the mounted
        // window is all that builds, so a 100k-block document types like a
        // short one. Rows are keyed by index, which is also the field's key.
        let count = editor.block_editor().map_or(0, |blocks| blocks.blocks().len());
        col()
            .key("editor-blocks")
            .w_full()
            .px(20.0)
            .py(crate::editor::scroll_sync::EDITOR_PAD_TOP)
            .growth(GrowthSpec::default())
            .virtual_rows_version(count)
            .virtual_rows(
                count,
                crate::block_ui::BLOCK_ROW_ESTIMATE,
                |_: &WindowState, index| crate::block_ui::block_field_key(index).into(),
                move |state: &WindowState, index| {
                    block_field_row(state, index, source_font, theme, caret, caret_selection)
                },
            )
            // E-07: the first visible row anchors the source line mapping; the
            // view re-runs once per row crossing to measure its geometry.
            .on_visible_rows(|state: &mut WindowState, range| state.note_visible_blocks(range))
    } else {
        col().key("editor-body").w_full().px(20.0).py(crate::editor::scroll_sync::EDITOR_PAD_TOP).child(field)
    };
    let scroll = state.editor.scroll.clone();
    // Only this window draws the source offset; waking it is enough.
    let waker = state.window_waker();
    pane.child(el().w_full().h(1.0).fill(c.border_muted)).child(
        col()
            .key("editor-scroll")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .scroll(ScrollSpec { stick_bottom: false })
            // The source's offset is the only input of E-07's one-way sync.
            // `on_scroll_offset` cannot commit, so it records the report and
            // wakes `poll`; the preview is never read back.
            .on_scroll_offset(move |at| {
                scroll.set(reader::ScrollSnapshot::from_pos(at));
                waker();
            })
            .child(body),
    )
}

/// One block row: a full-width source field bound to `block`, with its own
/// syntax spans. The virtual row gives it the `editor-block-{index}` key, so
/// `Tx::focus` and `scroll_into_view` address the field.
fn block_field_row(
    state: &WindowState,
    index: usize,
    font: TextStyle,
    theme: SyntaxTheme,
    caret: Color,
    selection: Color,
) -> El {
    let Some(block) = state
        .active_tab()
        .and_then(|tab| tab.view.editor.as_ref())
        .and_then(|editor| editor.block_editor())
        .and_then(|blocks| blocks.block(index))
    else {
        return el().h(0.0);
    };
    let spans = lmdv_edit::syntax::syntax_spans(&block.edit, &theme, None);
    text_input(&block.edit, font)
        .label(lmdv_i18n::t!("editor.markdownBlock", "n" => index + 1))
        .caret(caret, selection)
        .w_full()
        .spans(spans)
        .pb(crate::block_ui::BLOCK_ROW_GAP)
        .on_edit(move |state: &mut WindowState| state.active_block_edit(index))
        .on_input_change(move |state: &mut WindowState, tx| state.block_changed(index, tx))
        .on_input_key(move |state: &mut WindowState, event, tx| {
            state.block_key(index, event, tx) || state.editor_key(event, tx)
        })
        // A plain Return is consumed by `block_key`; this is the safety net
        // if a platform path reaches submit instead, so it never presses the
        // window's default button.
        .on_submit(move |state: &mut WindowState, tx| {
            state.editor.block_kind = lmdv_edit::BlockEditKind::Typing;
            state.active_block_edit(index).insert("\n");
            state.block_changed(index, tx);
        })
}

/// The format bar above the source (E-22): inline marks, headings and the
/// block/list groups. Buttons dispatch the same commands as the keymap and the
/// `/` menu; their pressed state comes from the selection.
fn format_bar(tab: &Tab, kit: &Kit) -> El {
    let c = &kit.c;
    let state = crate::edit_ui::format_state(tab);
    let inline = row()
        .key("format-inline")
        .w_full()
        .px(10.0)
        .py(3.0)
        .gap(2.0)
        .center_y()
        .child(format_button("B", lmdv_i18n::t!("cmd.bold"), "Ctrl+B", "edit.bold", state.bold, kit))
        .child(format_button("I", lmdv_i18n::t!("cmd.italic"), "Ctrl+I", "edit.italic", state.italic, kit))
        .child(format_button("S", lmdv_i18n::t!("cmd.strike"), "Ctrl+Shift+X", "edit.strike", state.strike, kit))
        .child(format_button("Code", lmdv_i18n::t!("cmd.code"), "Ctrl+Shift+M", "edit.code", state.code, kit))
        .child(format_button("Link", lmdv_i18n::t!("cmd.link"), "Ctrl+K", "edit.link", false, kit))
        .child(group_divider(kit))
        .child(format_button("Text", lmdv_i18n::t!("cmd.paragraph"), "", "edit.paragraph", state.heading == 0, kit))
        .child(format_button("H1", lmdv_i18n::t!("cmd.h1"), "Ctrl+1", "edit.heading-1", state.heading == 1, kit))
        .child(format_button("H2", lmdv_i18n::t!("cmd.h2"), "Ctrl+2", "edit.heading-2", state.heading == 2, kit))
        .child(format_button("H3", lmdv_i18n::t!("cmd.h3"), "Ctrl+3", "edit.heading-3", state.heading == 3, kit));
    let blocks = row()
        .key("format-blocks")
        .w_full()
        .px(10.0)
        .py(3.0)
        .gap(2.0)
        .center_y()
        .child(format_button("•", lmdv_i18n::t!("cmd.bullet"), "", "edit.list-bullet", state.bullet, kit))
        .child(format_button("1.", lmdv_i18n::t!("cmd.ordered"), "", "edit.list-ordered", state.ordered, kit))
        .child(format_button("[ ]", lmdv_i18n::t!("cmd.task"), "", "edit.list-task", state.task, kit))
        .child(format_button(">", lmdv_i18n::t!("cmd.quote"), "", "edit.quote", state.quote, kit))
        .child(group_divider(kit))
        .child(format_button("Tbl", lmdv_i18n::t!("cmd.table"), "", "edit.insert-table", false, kit))
        .child(format_button("{}", lmdv_i18n::t!("cmd.codeblock"), "", "edit.insert-code-block", false, kit))
        .child(format_button("fx", lmdv_i18n::t!("cmd.math"), "", "edit.insert-math", false, kit))
        .child(format_button("Mmd", lmdv_i18n::t!("cmd.diagram"), "", "edit.insert-mermaid", false, kit))
        .child(format_button("Note", lmdv_i18n::t!("cmd.note"), "", "edit.insert-note", false, kit))
        .child(format_button("Tip", lmdv_i18n::t!("cmd.tip"), "", "edit.insert-tip", false, kit))
        .child(format_button("Warn", lmdv_i18n::t!("cmd.warning"), "", "edit.insert-warning", false, kit))
        .child(format_button("—", lmdv_i18n::t!("cmd.divider"), "", "edit.insert-divider", false, kit))
        .child(row().grow(1.0))
        .child(
            text(format!("/ {}", lmdv_i18n::t!("keys.slash")))
                .key("format-hint")
                .font(kit.ty.small)
                .color(c.text_faint)
                .truncate()
                .shrink(),
        );
    col().key("format-bar").w_full().child(inline).child(blocks).child(el().w_full().h(1.0).fill(c.border_muted))
}

/// One format-bar button: a compact visible glyph and the full accessible
/// label. `expanded` gives the ghost variant its pressed fill.
fn format_button(
    visible: &str,
    label: impl Into<String>,
    shortcut: &'static str,
    command: &'static str,
    active: bool,
    kit: &Kit,
) -> El {
    let label = label.into();
    let visible = visible.to_owned();
    let mut button = rune_kit::Button::new(label.clone())
        .variant(ButtonVariant::GhostMuted)
        .control_size(ControlSize::Small)
        .expanded(active)
        .help(label)
        .label_with(move |font, ink| text(visible.clone()).font(font).color(ink))
        .on_press_tx(move |state: &mut WindowState, tx: &mut Tx| {
            let _ = state.run_command(command, tx);
        });
    if !shortcut.is_empty() {
        button = button.shortcut(shortcut);
    }
    button.build(kit).key(format!("format-{command}"))
}

fn group_divider(kit: &Kit) -> El {
    el().w(1.0).h(16.0).mx(3.0).fill(kit.c.border_muted)
}

/// The find/replace bar above the source (E-20). `Ctrl+F` shows the find row;
/// `Ctrl+H` adds the replace row. The match count or the regex error sits
/// beside the query.
fn find_bar(tab: &Tab, state: &WindowState, kit: &Kit) -> El {
    let c = &kit.c;
    let Some(editor) = tab.view.editor.as_ref() else {
        return el().key("find-bar");
    };
    let find = &editor.find;
    let bar = &tab.view.find_bar;
    let query = text_input(&bar.query, kit.ty.ui)
        .key(crate::edit_ui::FIND_QUERY_KEY)
        .single_line()
        .label(lmdv_i18n::t!("cm.find"))
        .placeholder(lmdv_i18n::t!("find.placeholder"), c.text_faint)
        .w(190.0)
        .on_edit(|state: &mut WindowState| state.active_find_edit(FindField::Query))
        .on_input_change(|state: &mut WindowState, _| state.find_query_changed())
        .on_input_key(|state: &mut WindowState, event, tx| state.find_key(FindField::Query, event, tx));

    let count: El = if let Some(error) = state.find_error() {
        row()
            .key("find-count")
            .grow(1.0)
            .child(text(error).key("find-error").font(kit.ty.small).color(c.danger.fg).truncate())
    } else if find.query.is_empty() {
        row().key("find-count").grow(1.0)
    } else if find.matches.is_empty() {
        text(lmdv_i18n::t!("pane.noMatches")).key("find-count").font(kit.ty.small).color(c.text_faint).grow(1.0)
    } else {
        text(format!("{} / {}", find.current + 1, find.matches.len()))
            .key("find-count")
            .font(kit.ty.small)
            .color(c.text_muted)
            .grow(1.0)
    };

    let previous = find_option_button(
        "↑",
        lmdv_i18n::t!("find.prev"),
        "Shift+Enter",
        "edit.find-previous",
        false,
        kit,
        |state, tx| state.find_prev_cmd(tx),
    );
    let next =
        find_option_button("↓", lmdv_i18n::t!("find.next"), "Enter", "edit.find-next", false, kit, |state, tx| {
            state.find_next_cmd(tx)
        });
    let case =
        find_toggle_button("find-case", "Aa", lmdv_i18n::t!("cm.matchCase"), find.case_sensitive, kit, |state, _| {
            state.toggle_find_case()
        });
    let whole =
        find_toggle_button("find-whole-word", "W", lmdv_i18n::t!("cm.byWord"), find.whole_word, kit, |state, _| {
            state.toggle_find_whole_word()
        });
    let regex = find_toggle_button("find-regex", ".*", lmdv_i18n::t!("cm.regexp"), find.regex, kit, |state, _| {
        state.toggle_find_regex()
    });
    let close = rune_kit::Button::new(lmdv_i18n::t!("cm.close"))
        .variant(ButtonVariant::Ghost)
        .control_size(ControlSize::Small)
        .help(lmdv_i18n::t!("find.close"))
        .on_press_tx(|state: &mut WindowState, tx: &mut Tx| state.close_find(tx))
        .build(kit)
        .key("find-close");

    let query_row = row()
        .key("find-row")
        .w_full()
        .px(10.0)
        .py(4.0)
        .gap(4.0)
        .center_y()
        .child(text(lmdv_i18n::t!("cm.find")).font(kit.ty.small).color(c.text_muted))
        .child(query)
        .child(count)
        .child(previous)
        .child(next)
        .child(case)
        .child(whole)
        .child(regex)
        .child(close);

    let mut stack = col().key("find-bar").w_full().fill(c.layer_01).child(query_row);
    if find.replace_open {
        let replace = text_input(&bar.replace, kit.ty.ui)
            .key(crate::edit_ui::FIND_REPLACE_KEY)
            .single_line()
            .label(lmdv_i18n::t!("cm.replace"))
            .placeholder(lmdv_i18n::t!("cm.replace"), c.text_faint)
            .w(190.0)
            .on_edit(|state: &mut WindowState| state.active_find_edit(FindField::Replace))
            .on_input_change(|state: &mut WindowState, _| state.find_replace_changed())
            .on_input_key(|state: &mut WindowState, event, tx| state.find_key(FindField::Replace, event, tx));
        let replace_one = rune_kit::Button::new(lmdv_i18n::t!("cm.replaceOne"))
            .variant(ButtonVariant::Outline)
            .control_size(ControlSize::Small)
            .disabled(find.matches.is_empty())
            .on_press_tx(|state: &mut WindowState, tx: &mut Tx| state.replace_current_cmd(tx))
            .build(kit)
            .key("find-replace-one");
        let replace_all = rune_kit::Button::new(lmdv_i18n::t!("cm.replaceAll"))
            .variant(ButtonVariant::Outline)
            .control_size(ControlSize::Small)
            .disabled(find.matches.is_empty())
            .on_press_tx(|state: &mut WindowState, _| state.replace_all_cmd())
            .build(kit)
            .key("find-replace-all");
        stack = stack.child(
            row()
                .key("replace-row")
                .w_full()
                .px(10.0)
                .pb(4.0)
                .gap(4.0)
                .center_y()
                .child(text(lmdv_i18n::t!("cm.replace")).font(kit.ty.small).color(c.text_muted))
                .child(replace)
                .child(row().grow(1.0))
                .child(replace_one)
                .child(replace_all),
        );
    }
    stack
}

fn find_toggle_button(
    key: &'static str,
    visible: &str,
    label: impl Into<String>,
    active: bool,
    kit: &Kit,
    action: impl Fn(&mut WindowState, &mut Tx) + 'static,
) -> El {
    let label = label.into();
    let visible = visible.to_owned();
    rune_kit::Button::new(label.clone())
        .variant(ButtonVariant::GhostMuted)
        .control_size(ControlSize::Small)
        .expanded(active)
        .help(label)
        .label_with(move |font, ink| text(visible.clone()).font(font).color(ink))
        .on_press_tx(move |state: &mut WindowState, tx: &mut Tx| action(state, tx))
        .build(kit)
        .key(key)
}

fn find_option_button(
    visible: &str,
    label: impl Into<String>,
    shortcut: &'static str,
    key: &'static str,
    active: bool,
    kit: &Kit,
    action: impl Fn(&mut WindowState, &mut Tx) + 'static,
) -> El {
    let label = label.into();
    let visible = visible.to_owned();
    rune_kit::Button::new(label.clone())
        .variant(ButtonVariant::GhostMuted)
        .control_size(ControlSize::Small)
        .expanded(active)
        .help(label)
        .shortcut(shortcut)
        .label_with(move |font, ink| text(visible.clone()).font(font).color(ink))
        .on_press_tx(move |state: &mut WindowState, tx: &mut Tx| action(state, tx))
        .build(kit)
        .key(key)
}

/// The go-to-line prompt (E-31).
fn goto_line_bar(goto: &GotoLine, kit: &Kit) -> El {
    let c = &kit.c;
    let field = text_input(&goto.input, kit.ty.ui)
        .key(crate::edit_ui::GOTO_FIELD_KEY)
        .single_line()
        .label(lmdv_i18n::t!("cm.gotoLine"))
        .w(110.0)
        .on_edit(|state: &mut WindowState| state.active_goto_edit())
        .on_input_key(|state: &mut WindowState, event, tx| state.goto_key(event, tx))
        .on_submit(|state: &mut WindowState, tx: &mut Tx| state.submit_goto_line(tx));
    let mut row = row()
        .key("goto-line-bar")
        .w_full()
        .fill(c.layer_01)
        .px(10.0)
        .py(4.0)
        .gap(6.0)
        .center_y()
        .child(text(lmdv_i18n::t!("cm.gotoLine")).font(kit.ty.small_medium).color(c.text_muted))
        .child(field)
        .child(
            rune_kit::Button::new(lmdv_i18n::t!("cm.go"))
                .variant(ButtonVariant::Submit)
                .control_size(ControlSize::Small)
                .on_press_tx(|state: &mut WindowState, tx: &mut Tx| state.submit_goto_line(tx))
                .build(kit)
                .key("goto-line-go"),
        )
        .child(
            rune_kit::Button::new(lmdv_i18n::t!("cm.close"))
                .variant(ButtonVariant::Ghost)
                .control_size(ControlSize::Small)
                .on_press(|state: &mut WindowState| state.cancel_goto_line())
                .build(kit)
                .key("goto-line-cancel"),
        );
    if let Some(error) = &goto.error {
        row = row.child(text(error.clone()).key("goto-line-error").font(kit.ty.small).color(c.danger.fg).truncate());
    }
    row
}

/// The filtered `/` block menu (E-28).
fn slash_menu_view(kit: &Kit, menu: &SlashMenu) -> El {
    let c = &kit.c;
    let items = crate::edit_ui::slash_items(&menu.query);
    let mut list = col()
        .key("slash-menu")
        .w_full()
        .mx(10.0)
        .mb(4.0)
        .padding(4.0)
        .gap(1.0)
        .radius(kit.radius.md)
        .fill(c.layer_01)
        .border(1.0, c.border_muted)
        .role("menu")
        .label(lmdv_i18n::t!("cm.completions"));
    for (index, item) in items.iter().enumerate() {
        let selected = index == menu.selected;
        let label = item.label.clone();
        let visible = label.clone();
        list = list.child(
            rune_kit::Button::new(label.clone())
                .variant(if selected { ButtonVariant::Contrast } else { ButtonVariant::Ghost })
                .control_size(ControlSize::Small)
                .expanded(selected)
                .help(label)
                .label_with(move |font, ink| text(visible.clone()).font(font).color(ink))
                .on_press_tx(move |state: &mut WindowState, tx: &mut Tx| state.pick_slash_item(index, tx))
                .build(kit)
                .w_full()
                .role("menuitem")
                .key(format!("slash-item-{index}")),
        );
    }
    list
}

/// The Save / Discard / conflict / draft dialogs (E-10/E-13/E-17).
fn editor_dialog(state: &WindowState, kit: &Kit, layer: &mut Vec<El>) {
    let c = &kit.c;
    let Some(prompt) = state.prompt() else { return };
    let title = prompt.title();
    let body = match prompt {
        crate::editor::Prompt::DiscardEdit { .. } => lmdv_i18n::t!("prompt.discardBody").to_owned(),
        crate::editor::Prompt::CloseTab { tab, .. } => {
            let name = state.tab(*tab).map(Tab::full_title).unwrap_or_else(|| "lmdv".to_owned());
            lmdv_i18n::t!("prompt.closeTabBody", "name" => name)
        }
        crate::editor::Prompt::CloseWindow => state.close_window_question(),
        crate::editor::Prompt::Quit => {
            let names = state.quit_document_names();
            lmdv_i18n::t!("prompt.quitBody", "names" => lmdv_i18n::list(&names))
        }
        crate::editor::Prompt::SaveConflict { .. } => lmdv_i18n::t!("prompt.saveConflictBody").to_owned(),
        crate::editor::Prompt::TaskConflict { .. } => lmdv_i18n::t!("prompt.taskConflictBody").to_owned(),
        crate::editor::Prompt::FileDeleted { .. } => lmdv_i18n::t!("prompt.deletedBody").to_owned(),
        crate::editor::Prompt::DiskChanged { .. } => lmdv_i18n::t!("prompt.diskChangedBody").to_owned(),
        crate::editor::Prompt::Draft { draft, .. } => lmdv_i18n::t!("prompt.draftBody", "at" => draft.at.clone()),
        crate::editor::Prompt::LeaseHeld { holder } => {
            lmdv_i18n::t!("prompt.leaseBody", "n" => holder.window.0)
        }
    };
    let mut actions = row().key("prompt-actions").w_full().gap(8.0).justify_end();
    match prompt {
        crate::editor::Prompt::DiscardEdit { .. } | crate::editor::Prompt::CloseTab { .. } => {
            actions = actions
                .child(
                    Button::new(lmdv_i18n::t!("msg.save"))
                        .variant(ButtonVariant::Contrast)
                        .on_press_tx(|state: &mut WindowState, tx: &mut Tx| state.prompt_primary(tx))
                        .build(kit)
                        .key("prompt-primary"),
                )
                .child(
                    Button::new(lmdv_i18n::t!("msg.dontSave"))
                        .variant(ButtonVariant::Outline)
                        .on_press_tx(|state: &mut WindowState, tx: &mut Tx| state.prompt_secondary(tx))
                        .build(kit)
                        .key("prompt-secondary"),
                );
        }
        crate::editor::Prompt::CloseWindow | crate::editor::Prompt::Quit => {
            actions = actions
                .child(
                    Button::new(lmdv_i18n::t!("msg.saveAll"))
                        .variant(ButtonVariant::Contrast)
                        .on_press_tx(|state: &mut WindowState, tx: &mut Tx| state.prompt_primary(tx))
                        .build(kit)
                        .key("prompt-primary"),
                )
                .child(
                    Button::new(lmdv_i18n::t!("msg.dontSave"))
                        .variant(ButtonVariant::Outline)
                        .on_press_tx(|state: &mut WindowState, tx: &mut Tx| state.prompt_secondary(tx))
                        .build(kit)
                        .key("prompt-secondary"),
                );
        }
        crate::editor::Prompt::SaveConflict { .. } => {
            actions = actions
                .child(
                    Button::new(lmdv_i18n::t!("msg.overwrite"))
                        .variant(ButtonVariant::Danger)
                        .on_press_tx(|state: &mut WindowState, tx: &mut Tx| state.prompt_primary(tx))
                        .build(kit)
                        .key("prompt-primary"),
                )
                .child(
                    Button::new(lmdv_i18n::t!("msg.keepTheirs"))
                        .variant(ButtonVariant::Outline)
                        .on_press_tx(|state: &mut WindowState, tx: &mut Tx| state.prompt_secondary(tx))
                        .build(kit)
                        .key("prompt-secondary"),
                );
        }
        crate::editor::Prompt::TaskConflict { .. } => {
            actions = actions
                .child(
                    Button::new(lmdv_i18n::t!("msg.overwrite"))
                        .variant(ButtonVariant::Danger)
                        .on_press_tx(|state: &mut WindowState, tx: &mut Tx| state.prompt_primary(tx))
                        .build(kit)
                        .key("prompt-primary"),
                )
                .child(
                    Button::new(lmdv_i18n::t!("msg.keepTheirs"))
                        .variant(ButtonVariant::Outline)
                        .on_press_tx(|state: &mut WindowState, tx: &mut Tx| state.prompt_secondary(tx))
                        .build(kit)
                        .key("prompt-secondary"),
                );
        }
        crate::editor::Prompt::FileDeleted { .. } => {
            actions = actions.child(
                Button::new(lmdv_i18n::t!("msg.saveAgain"))
                    .variant(ButtonVariant::Contrast)
                    .on_press_tx(|state: &mut WindowState, tx: &mut Tx| state.prompt_primary(tx))
                    .build(kit)
                    .key("prompt-primary"),
            );
        }
        crate::editor::Prompt::DiskChanged { .. } => {
            actions = actions
                .child(
                    Button::new(lmdv_i18n::t!("msg.reload"))
                        .variant(ButtonVariant::Danger)
                        .on_press_tx(|state: &mut WindowState, tx: &mut Tx| state.prompt_primary(tx))
                        .build(kit)
                        .key("prompt-primary"),
                )
                .child(
                    Button::new(lmdv_i18n::t!("prompt.keepMine"))
                        .variant(ButtonVariant::Outline)
                        .on_press_tx(|state: &mut WindowState, tx: &mut Tx| state.prompt_secondary(tx))
                        .build(kit)
                        .key("prompt-secondary"),
                );
        }
        crate::editor::Prompt::Draft { .. } => {
            actions = actions
                .child(
                    Button::new(lmdv_i18n::t!("msg.restore"))
                        .variant(ButtonVariant::Contrast)
                        .on_press_tx(|state: &mut WindowState, tx: &mut Tx| state.prompt_primary(tx))
                        .build(kit)
                        .key("prompt-primary"),
                )
                .child(
                    Button::new(lmdv_i18n::t!("msg.discard"))
                        .variant(ButtonVariant::Outline)
                        .on_press_tx(|state: &mut WindowState, tx: &mut Tx| state.prompt_secondary(tx))
                        .build(kit)
                        .key("prompt-secondary"),
                );
        }
        crate::editor::Prompt::LeaseHeld { .. } => {
            actions = actions.child(
                Button::new(lmdv_i18n::t!("prompt.ok"))
                    .variant(ButtonVariant::Contrast)
                    .on_press_tx(|state: &mut WindowState, tx: &mut Tx| state.prompt_primary(tx))
                    .build(kit)
                    .key("prompt-primary"),
            );
        }
    }
    actions = actions.child(
        Button::new(lmdv_i18n::t!("msg.cancel"))
            .variant(ButtonVariant::Ghost)
            .on_press(|state: &mut WindowState| state.prompt_cancel())
            .build(kit)
            .key("prompt-cancel"),
    );
    Dialog::new("editor", title)
        .open(true)
        .description(body)
        .fit()
        .on_close(|state: &mut WindowState| state.prompt_cancel())
        .child(
            col()
                .key("prompt-body")
                .w_full()
                .child(actions)
                .child(text(lmdv_i18n::t!("editor.escapeCancels")).font(kit.ty.small).color(c.text_faint)),
        )
        .build(kit, layer);
}

/// The reading article with the floating find bar over it (R-16–R-19).
///
/// The bar is out of flow at the top of the pane, so the article keeps the full
/// viewport; while it shows, the reader's scroll margin reserves its height so
/// `Tx::scroll_into_view` lands a match below it (R-19).
fn reader_pane(
    doc: &lmdv_core::Document,
    model: &std::sync::Arc<lmdv_md::MdDoc>,
    tab: &Tab,
    state: &WindowState,
    cx: &mut Cx<WindowState>,
    kit: &Kit,
) -> El {
    let article = reader_article(doc, model, tab, state, cx, kit);
    // O-07–O-10/R-03: the line and hairline float over the scrolling article and
    // take no layout, so the text passes under them without moving. They belong
    // to the reading view; the editor's preview never shows them.
    let viewer = tab.view.mode == Mode::Viewer;
    let reader_width = cx.layout_size(reader::ARTICLE_SCROLL_KEY).map_or(reader::READER_MAX_WIDTH, |size| size[0]);
    let line = if viewer { state.section_line_overlay(kit, reader_width) } else { None };
    let line_visible = line.is_some();
    let progress = if viewer { state.progress_overlay(kit) } else { None };
    let shield = if viewer { state.section_swallow_shield(state.ui.last_now) } else { None };
    let mut pane = col().key("reader-pane").w_full().h_full().min_h(0.0).child(article);
    if let Some(line) = line {
        pane = pane.child(line);
    }
    if let Some(progress) = progress {
        pane = pane.child(progress);
    }
    if let Some(shield) = shield {
        pane = pane.child(shield);
    }
    if tab.view.reader_find.open {
        let bar = reader_find_bar(tab, kit);
        // The floating find bar steps below the section line while it shows
        // (the original's `ui.css` does the same).
        let top = if line_visible { 10.0 + crate::section::SECTION_LINE_HEIGHT } else { 10.0 };
        pane = pane.child(
            row().key("reader-find-layer").pin(Some(0.0), Some(top), Some(0.0), None).justify_center().child(bar),
        );
    }
    pane
}

/// The floating reading find bar: query, `current/total`, previous, next and
/// close (R-16/R-18).
fn reader_find_bar(tab: &Tab, kit: &Kit) -> El {
    let c = &kit.c;
    let find = &tab.view.reader_find;
    let query = text_input(&find.query, kit.ty.ui)
        .key(READER_FIND_QUERY_KEY)
        .single_line()
        .label(lmdv_i18n::t!("cm.find"))
        .placeholder(lmdv_i18n::t!("find.placeholder"), c.text_faint)
        .w(220.0)
        .on_edit(|state: &mut WindowState| state.active_reader_find_edit())
        .on_input_change(|state: &mut WindowState, tx| state.reader_find_changed(tx))
        .on_input_key(|state: &mut WindowState, event, tx| state.reader_find_key(event, tx));
    let count = text(find.counter())
        .key("reader-find-count")
        .font(kit.ty.small)
        .color(if find.matches.is_empty() { c.text_faint } else { c.text_muted })
        .min_w(38.0);
    let previous = find_option_button(
        "↑",
        lmdv_i18n::t!("find.prev"),
        "Shift+Enter",
        "reader-find-prev",
        false,
        kit,
        |state, tx| {
            state.reader_find_prev(tx);
        },
    );
    let next =
        find_option_button("↓", lmdv_i18n::t!("find.next"), "Enter", "reader-find-next", false, kit, |state, tx| {
            state.reader_find_next(tx);
        });
    let close = IconButton::new(kit.glyph.close)
        .variant(IconButtonVariant::GhostMuted)
        .control_size(ControlSize::Small)
        .label(lmdv_i18n::t!("find.close"))
        .help(lmdv_i18n::t!("find.close"))
        .on_press_tx(|state: &mut WindowState, tx: &mut Tx| {
            state.close_reader_find(tx);
        })
        .build(kit)
        .key("reader-find-close");
    row()
        .key("reader-find-bar")
        .px(10.0)
        .py(6.0)
        .gap(6.0)
        .center_y()
        .radius(kit.radius.md)
        .fill(c.layer_01)
        .border(1.0, c.border_base)
        .shadow(4.0, 16.0, rune::Color { r: 0.0, g: 0.0, b: 0.0, a: 0.22 })
        .role("search")
        .label(lmdv_i18n::t!("cm.find"))
        .child(query)
        .child(count)
        .child(previous)
        .child(next)
        .child(close)
}

fn reader_article(
    doc: &lmdv_core::Document,
    model: &std::sync::Arc<lmdv_md::MdDoc>,
    tab: &Tab,
    state: &WindowState,
    cx: &mut Cx<WindowState>,
    kit: &Kit,
) -> El {
    let look = state.current_appearance(cx.system_dark());
    let width = cx.layout_size(reader::ARTICLE_SCROLL_KEY).map_or(reader::READER_MAX_WIDTH, |size| size[0]);
    let column_width = crate::settings::column_max_width(state.shared().prefs().reader.width);
    let scale = cx.env().scale;
    let root = state.document_root(doc);
    let image_size = width.min(look.theme.image_max_width);
    let math = WindowMath::new(&state.math, look.theme.text, scale);
    let images = WindowImages::new(
        state.image_cell(),
        state.window_waker(),
        state.shared().is_headless(),
        root,
        image_size,
        scale,
    );

    let anchor = reader::viewport_anchor(cx, model, state.scroll_snapshot());
    state.set_viewport_anchor(anchor);
    // O-12: the drawn headings' tops, from this frame's layout, so a heading
    // jump can tell whether it would move the page at all. The editor's preview
    // is not a reading surface, so it publishes none.
    let headings = if tab.view.mode == Mode::Viewer {
        let headings = crate::section::measure_headings(cx, model);
        state.set_heading_tops(headings.clone());
        headings
    } else {
        state.set_heading_tops(Vec::new());
        Vec::new()
    };
    if tab.view.mode == Mode::Editor {
        // E-07: measure the preview blocks around the editor's top and caret
        // once per frame. `Cx::layout` is unscrolled layout, so scrolling the
        // preview does not re-run this view through these reads.
        state.publish_preview_geometry(model, cx);
    } else {
        // O-07–O-10: this frame's heading path, once the page has settled. The
        // reads also make the view follow a scroll, as the anchor above does.
        crate::section::compute(state, cx, model, &headings);
        // OI-098: the top-level block under the viewport's top, for the
        // keyboard's “Edit from here”.
        state.reading.reader_block.set(crate::reader::reading_block(cx, model, state.scroll_snapshot()));
        // R-19: where the current find hit's block sits, for the scroll band.
        crate::reader_find::publish_band(cx, model, tab);
    }
    let find_margin = if tab.view.mode == Mode::Viewer && tab.view.reader_find.open {
        READER_FIND_BAND + if state.section_line.visible() { crate::section::SECTION_LINE_HEIGHT } else { 0.0 }
    } else {
        0.0
    };

    reader::ReaderView::new(model, &look.theme, &look.style, state.scroll_cell())
        .width(width)
        .max_width(column_width)
        .scroll_signal(state.scroll_signal_cell())
        .math(&math)
        .images(&images)
        .wires(RenderWires {
            link: Some(state.link_wire()),
            // M-07: task boxes are live only in the reading view. The editor's
            // preview and source keep every task click read-only. The wire goes
            // on the real checkbox control, not its row (OI-091).
            task: None,
            task_control: (tab.view.mode == Mode::Viewer).then(|| state.task_control_wire()),
            // E-35/E-04: a block maps a preview double-click or right-click to
            // its source line in both modes (the double-click acts only while
            // editing).
            block: Some(state.cached_block_wire().clone()),
            // M-06/X-16: every table gets the hover action bar, copy and preview.
            table: Some(state.table_wire(kit)),
            // V-01: every drawn image and diagram opens the full-window preview.
            // The editor's preview keeps its own click (E-35), as task boxes do.
            media: (tab.view.mode == Mode::Viewer).then(|| state.media_wire(kit)),
        })
        .details(Some(&tab.view as &dyn DetailsState))
        .scale(scale)
        .background(kit.c.bg_base)
        // R-19/OI-084: paint the find hits and the `--line` flash, and keep the
        // floating bar's band clear when revealing a match.
        .highlights(state.reader_highlights(tab))
        .scroll_margin(find_margin)
        .build()
}

/// A deleted backing file: the buffer survives and Save again recreates it.
fn gone_card(doc: &Document, kit: &Kit) -> El {
    let c = &kit.c;
    let actions = row()
        .key("gone-actions")
        .gap(8.0)
        .child(
            Button::new(lmdv_i18n::t!("msg.saveAgain"))
                .variant(ButtonVariant::Contrast)
                .on_press(|state: &mut WindowState| state.request_save_again())
                .build(kit)
                .key("save-again"),
        )
        .child(
            Button::new(lmdv_i18n::t!("tab.closeTab"))
                .variant(ButtonVariant::Ghost)
                .on_press(|state: &mut WindowState| state.close_active_tab())
                .build(kit)
                .key("close-gone-tab"),
        );
    col()
        .key("document-gone")
        .w_full()
        .h_full()
        .center()
        .gap(10.0)
        .child(text(doc.title.borrow().clone()).font(TextStyle::new(18.0, 600.0)).color(c.text_base))
        .child(text(lmdv_i18n::t!("msg.deletedDetail")).font(kit.ty.small).color(c.text_muted))
        .child(actions)
}

fn status_card(kit: &Kit, title: &str, detail: &str, action: Option<El>) -> El {
    let c = &kit.c;
    let mut card = col()
        .key("document-status")
        .w_full()
        .h_full()
        .center()
        .gap(8.0)
        .child(text(title.to_owned()).font(kit.ty.ui_medium).color(c.text_base))
        .child(text(detail.to_owned()).font(kit.ty.small).color(c.text_muted));
    if let Some(action) = action {
        card = card.child(action);
    }
    card
}

fn error_card(doc: &Document, message: &str, kit: &Kit) -> El {
    let c = &kit.c;
    let path = doc.key.borrow().path().map(|path| path.to_path_buf());
    let mut actions = row().key("error-actions").gap(8.0);
    if let Some(path) = path {
        actions = actions.child(
            Button::new(lmdv_i18n::t!("msg.tryAgain"))
                .variant(ButtonVariant::Outline)
                .on_press_tx(move |state: &mut WindowState, tx: &mut Tx| {
                    state.open_path(path.clone());
                    state.apply_pending_scroll(tx);
                })
                .build(kit)
                .key("retry-open"),
        );
    }
    actions = actions.child(
        Button::new(lmdv_i18n::t!("tab.closeTab"))
            .variant(ButtonVariant::Ghost)
            .on_press(|state: &mut WindowState| state.close_active_tab())
            .build(kit)
            .key("close-error-tab"),
    );

    col()
        .key("document-error")
        .w_full()
        .h_full()
        .center()
        .gap(10.0)
        .child(text(doc.title.borrow().clone()).font(TextStyle::new(18.0, 600.0)).color(c.text_base))
        .child(text(lmdv_i18n::t!("msg.openFailedCard")).font(kit.ty.body).color(c.text_muted))
        .child(text(message.to_owned()).font(kit.ty.small).color(c.danger.fg).wrap())
        .child(actions)
}

fn source_preview(doc: &Document, kit: &Kit, note: Option<&str>) -> El {
    let c = &kit.c;
    let buffer = doc.text();
    let total = buffer.line_count();
    let line_limit = total.min(PREVIEW_MAX_LINES);
    let mut drawn = 0usize;
    let mut bytes = 0usize;
    let mut lines = col().key("document-lines").w_full().gap(0.0);

    for line in 0..line_limit {
        let range = buffer.line_range(line);
        let raw = &buffer.text()[range];
        if drawn > 0 && bytes + raw.len() > PREVIEW_MAX_BYTES {
            break;
        }
        bytes += raw.len();
        drawn += 1;
        let display = raw.strip_suffix('\n').unwrap_or(raw);
        let body = if display.is_empty() { " ".to_owned() } else { display.to_owned() };
        lines = lines.child(
            row()
                .key(format!("line-{line}"))
                .w_full()
                .items_start()
                .gap(14.0)
                .child(
                    text((line + 1).to_string())
                        .font(kit.ty.mono)
                        .color(c.text_faint)
                        .w(44.0)
                        .align(Align::End)
                        .a11y_hidden(),
                )
                .child(text(body).font(kit.ty.mono).color(c.text_base).wrap().grow(1.0)),
        );
    }

    let mut footer = col().key("document-footer").w_full().px(24.0).pt(8.0).pb(16.0).gap(4.0);
    if let Some(note) = note {
        footer = footer.child(text(note.to_owned()).key("document-note").font(kit.ty.small).color(c.danger.fg).wrap());
    }
    if drawn < total {
        footer = footer.child(
            text(lmdv_i18n::t!("msg.sourcePreviewTruncated", "drawn" => drawn, "total" => total))
                .font(kit.ty.small)
                .color(c.text_faint),
        );
    }

    col()
        .key("document")
        .w_full()
        .h_full()
        .min_h(0.0)
        .child(
            row()
                .key("document-header")
                .w_full()
                .px(24.0)
                .py(12.0)
                .gap(12.0)
                .center_y()
                .child(
                    text(doc.title.borrow().clone())
                        .key("document-title")
                        .font(kit.ty.ui_medium)
                        .color(c.text_base)
                        .truncate()
                        .grow(1.0),
                )
                .child(
                    text(lmdv_i18n::t!("msg.sourcePreview", "n" => total))
                        .font(kit.ty.small)
                        .color(c.text_muted)
                        .shrink(),
                ),
        )
        .child(el().key("document-separator").w_full().h(1.0).fill(c.border_muted))
        .child(
            col()
                .key("document-scroll")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec { stick_bottom: false })
                .child(col().w_full().px(24.0).py(12.0).child(lines))
                .child(footer),
        )
}

fn notice_bar(notice: &Notice, kit: &Kit) -> El {
    let c = &kit.c;
    let mut bar = row()
        .key("notice")
        .w_full()
        .px(16.0)
        .py(8.0)
        .gap(12.0)
        .center_y()
        .fill(c.layer_02)
        .child(text(notice.text.clone()).font(kit.ty.small).color(c.text_base).wrap().grow(1.0));
    if let Some(action) = &notice.action {
        bar = bar.child(
            Button::new(action.label())
                .variant(ButtonVariant::Ghost)
                .control_size(ControlSize::Small)
                .on_press(|state: &mut WindowState| state.run_notice_action())
                .build(kit)
                .key("notice-action"),
        );
    }
    bar.child(
        IconButton::new(kit.glyph.close)
            .variant(IconButtonVariant::GhostMuted)
            .control_size(ControlSize::Small)
            .label(lmdv_i18n::t!("notice.dismiss"))
            .on_press(|state: &mut WindowState| state.dismiss_notice())
            .build(kit)
            .key("dismiss-notice"),
    )
}

fn help_dialog(state: &WindowState, cx: &mut Cx<WindowState>, kit: &Kit, layer: &mut Vec<El>) {
    let c = &kit.c;
    let mut rows = col().key("help-keys").w_full().gap(6.0).child(
        text(lmdv_i18n::t!("help.description"))
            .key("help-keys-description")
            .font(kit.ty.small)
            .color(c.text_muted)
            .w_full()
            .wrap(),
    );
    for command in COMMANDS {
        let keys = if command.default_keys.is_empty() { "—".to_owned() } else { command.default_keys.join(" · ") };
        rows = rows.child(
            row()
                .key(format!("help-{}", command.id))
                .w_full()
                .gap(12.0)
                .center_y()
                .child(
                    text(crate::commands::command_title(state, command))
                        .font(kit.ty.small)
                        .color(c.text_muted)
                        .grow(1.0)
                        .min_w(0.0)
                        .wrap(),
                )
                .child(text(keys).font(kit.ty.mono).color(c.text_base).shrink().wrap()),
        );
    }
    rows = rows.child(el().w_full().h(1.0).my(4.0).fill(c.border_muted)).child(
        text(lmdv_i18n::t!("keys.editing")).key("help-editing-heading").font(kit.ty.small_medium).color(c.text_base),
    );
    for (index, (label, keys)) in crate::edit_ui::editor_keys().into_iter().enumerate() {
        rows = rows.child(
            row()
                .key(format!("help-editor-{index}"))
                .w_full()
                .gap(12.0)
                .center_y()
                .child(text(label).font(kit.ty.small).color(c.text_muted).grow(1.0).min_w(0.0).wrap())
                .child(text(keys).font(kit.ty.mono).color(c.text_base).shrink().wrap()),
        );
    }
    // T-08/T-09/T-10: the tab keys the command table cannot spell as one
    // chord each — the numbered tabs and the clamped move (the original's
    // compact key list has the same rows).
    rows = rows
        .child(el().w_full().h(1.0).my(4.0).fill(c.border_muted))
        .child(text(lmdv_i18n::t!("keys.tabs")).key("help-tabs-heading").font(kit.ty.small_medium).color(c.text_base));
    for (index, (label, keys)) in [
        (lmdv_i18n::t!("keys.tabN"), "Ctrl+1 \u{2026} Ctrl+8 \u{b7} Ctrl+9"),
        (lmdv_i18n::t!("keys.moveTab"), "Ctrl+Shift+PgUp \u{b7} Ctrl+Shift+PgDn"),
    ]
    .into_iter()
    .enumerate()
    {
        rows = rows.child(
            row()
                .key(format!("help-tab-key-{index}"))
                .w_full()
                .gap(12.0)
                .center_y()
                .child(text(label).font(kit.ty.small).color(c.text_muted).grow(1.0).min_w(0.0).wrap())
                .child(text(keys).font(kit.ty.mono).color(c.text_base).shrink().wrap()),
        );
    }
    Dialog::new("help", lmdv_i18n::t!("cmd.keys"))
        .open(state.help_view() == Some(HelpView::Shortcuts))
        .fit()
        .on_close(|state: &mut WindowState| state.close_help())
        .child(
            col()
                .key("help-keys-scroll")
                .w_full()
                .max_h(cx.layout_size("window-body").map_or(560.0, |[_, height]| (height - 200.0).clamp(160.0, 620.0)))
                .min_h(0.0)
                .px(16.0)
                .scroll(ScrollSpec { stick_bottom: false })
                .child(rows),
        )
        .build(kit, layer);
}

/// The still, deterministic start page scene.
pub fn scene_start(state: &mut WindowState) {
    state.reset_to_start_page();
    state.left.set_open(false);
    state.right.set_open(false);
}

/// Three synthetic documents so the strip, panes and source preview are all
/// visible without touching the filesystem.
pub fn scene_tabs(state: &mut WindowState) {
    state.reset_to_start_page();
    state.open_synthetic("Welcome to lmdv", WELCOME_MARKDOWN);
    state.open_synthetic("Reading notes", READING_MARKDOWN);
    state.open_synthetic("A note being edited", EDITING_MARKDOWN);
    state.activate_index(1);
    state.left.set_open(true);
    state.right.set_open(true);
    state.ui.notice = None;
    state.ui.help = None;
}

/// T-17's overflow strip: twelve tabs, so the strip scrolls and its ⌄ list is
/// showing with the front tab marked ● and New tab after the separator.
pub fn scene_tab_overflow(state: &mut WindowState) {
    state.reset_to_start_page();
    for index in 1..=12 {
        state.open_synthetic(&format!("Note {index}"), &format!("# Note {index}\n\nBody.\n"));
    }
    state.activate_index(1);
    if let Some(tab) = state.active_tab_mut() {
        tab.view.outline_hidden = true;
    }
    state.left.set_open(false);
    state.right.set_open(false);
    state.ui.notice = None;
    state.ui.help = None;
    state.open_tab_list();
}

/// The tab scene with the compact shortcut list open (R-11's `?`).
pub fn scene_help(state: &mut WindowState) {
    scene_tabs(state);
    state.open_help(HelpView::Shortcuts);
}

/// The tab scene with the full localized guide open (F1 / `app.help`).
pub fn scene_help_full(state: &mut WindowState) {
    scene_tabs(state);
    // The guide is the subject: keep the narrow-window scene's panes folded so
    // the modal is the only thing above the article.
    state.left.set_open(false);
    state.right.set_open(false);
    state.open_help(HelpView::Full);
}

/// The tab scene with the tab context menu open: T-18's items for a pasted
/// tab (Save as… is live, the path items are not), plus a closed tab to
/// reopen.
pub fn scene_tab_menu(state: &mut WindowState) {
    scene_tabs(state);
    // A tab closed just before: “Reopen closed tab” is enabled (T-07).
    let extra = state.open_synthetic("Closed note", "# Closed note\n\nText.\n");
    state.close_tab(extra);
    state.activate_index(1);
    if let Some(tab) = state.active_tab().map(|tab| tab.id) {
        state.show_tab_menu(tab, PointerPress { x: 180.0, y: 74.0 });
    }
}

/// [`scene_tab_menu`] on a file's tab: Copy path and Show in File Explorer
/// are live, Save as… is not (T-18's per-kind matrix).
pub fn scene_tab_menu_file(state: &mut WindowState) {
    state.reset_to_start_page();
    state.open_path(browser_root().join("nested/deeper.md"));
    state.open_synthetic("Reading notes", READING_MARKDOWN);
    let extra = state.open_synthetic("Closed note", "# Closed note\n\nText.\n");
    state.close_tab(extra);
    state.left.set_open(false);
    state.right.set_open(false);
    state.activate_index(0);
    state.ui.notice = None;
    state.ui.help = None;
    if let Some(tab) = state.active_tab().map(|tab| tab.id) {
        state.show_tab_menu(tab, PointerPress { x: 180.0, y: 74.0 });
    }
}

/// The tab scene with T-16's hover card under the active tab: its full name,
/// its folder and its reading time.
pub fn scene_tab_card(state: &mut WindowState) {
    scene_tabs(state);
    if let Some(tab) = state.active_tab().map(|tab| tab.id) {
        state.show_tab_card_now(tab);
    }
}

/// The “Move tab to window…” chooser. With `LMDV_TEST_WINDOW` the headless
/// gate lists the second window by its published title; alone it shows the
/// disabled no-windows row.
pub fn scene_tab_window_menu(state: &mut WindowState) {
    scene_tabs(state);
    if let Some(tab) = state.active_tab().map(|tab| tab.id) {
        state.show_tab_window_menu(tab, Some(PointerPress { x: 180.0, y: 74.0 }));
    }
}

/// The file browser: a fixture folder, its tree, and a document in a nested
/// folder selected. `LMDV_BROWSER_ROOT` replaces the fixture root, so gates and
/// evidence runs can point at a temporary tree.
pub fn scene_browser(state: &mut WindowState) {
    state.reset_to_start_page();
    let root = browser_root();
    state.open_path(root.join("nested/deeper.md"));
    state.set_browser_root(root);
    state.left.set_open(true);
    state.right.set_open(false);
    if let Some(tab) = state.active_tab_mut() {
        tab.view.outline_hidden = true;
    }
    state.ui.notice = None;
    state.ui.help = None;
}

/// The file browser with a file's F-12 context menu open under its row, as
/// Shift+F10 draws it (built from the keyboard, then anchored by the view).
pub fn scene_browser_menu(state: &mut WindowState) {
    scene_browser(state);
    let Some(path) = state.active_path() else { return };
    state.show_browser_menu_keyboard_for(path, false);
}

/// The fixture (or `LMDV_BROWSER_ROOT`) folder the browser scenes show.
fn browser_root() -> std::path::PathBuf {
    std::env::var_os("LMDV_BROWSER_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures"))
}

/// One document rendered by the temporary P1 reader, outline closed.
pub fn scene_reader(state: &mut WindowState) {
    state.reset_to_start_page();
    state.open_synthetic("The quiet page", READER_MARKDOWN);
    state.left.set_open(false);
    state.right.set_open(false);
    if let Some(tab) = state.active_tab_mut() {
        tab.view.outline_hidden = true;
    }
    state.outline_filter.replace_text("");
    state.ui.notice = None;
    state.ui.help = None;
}

/// The reader in the sepia palette (J-01): the scheme reaches the chrome, the
/// article, the code and table tokens and the Mermaid style. The scene sets an
/// in-memory preference, so a snapshot never rewrites `prefs.json`.
pub fn scene_reader_sepia(state: &mut WindowState) {
    scene_reader(state);
    state.scene_prefs(|prefs| prefs.theme = ThemePref::Sepia);
}

/// The reader in the black palette (J-01), with the same in-memory rule.
pub fn scene_reader_black(state: &mut WindowState) {
    scene_reader(state);
    state.scene_prefs(|prefs| prefs.theme = ThemePref::Black);
}

/// The reader with the floating find bar open on a query that has matches
/// (R-16–R-19), for the headless snapshot and tree gate.
pub fn scene_reader_find(state: &mut WindowState) {
    scene_reader(state);
    state.show_reader_find("native");
}

/// The reader with a `--line` flash painted on one line, for the headless
/// snapshot of the [`HighlightKind::Line`](lmdv_render::HighlightKind) paint.
pub fn scene_reader_line(state: &mut WindowState) {
    scene_reader(state);
    let Some(tab) = state.active_tab() else { return };
    let Some(model) = tab.view.reader.clone() else { return };
    let line = 4.min(model.line_count().max(1));
    let Some(range) = reader::line_range(&model, line) else { return };
    if let Some(doc) = tab.document_id() {
        state.reading.line_flash = Some(crate::reading::LineFlash { doc, line, range, until: f64::INFINITY });
    }
}

/// The reader scrolled into a long page: the 2-point progress hairline is part
/// way across the top of the content pane (R-03).
pub fn scene_reader_progress(state: &mut WindowState) {
    scene_reader(state);
    // A raw offset, through the same restore path a remembered position uses, so
    // the scroll viewport really moves and reports its own offset.
    state.begin_restore(&ScrollAnchor { id: String::new(), offset: 1500.0 });
}

/// The reader past a section's heading, outline closed: the section line names
/// the path at the top of the article (O-07–O-10).
pub fn scene_reader_section(state: &mut WindowState) {
    scene_reader(state);
    let Some(model) = state.active_tab().and_then(|tab| tab.view.reader.clone()) else { return };
    let Some(heading) = model.headings.get(2) else { return };
    state.begin_restore(&ScrollAnchor { id: heading.id.clone(), offset: 300.0 });
}

/// The reader with the outline open: thirteen headings and the filter field.
pub fn scene_reader_outline(state: &mut WindowState) {
    state.reset_to_start_page();
    state.open_synthetic("The quiet page", READER_MARKDOWN);
    state.left.set_open(false);
    state.right.set_open(true);
    if let Some(tab) = state.active_tab_mut() {
        tab.view.outline_hidden = false;
    }
    state.outline_filter.replace_text("");
    state.ui.notice = None;
    state.ui.help = None;
}

/// A page with headings, prose, a list, a quote, code and a table, for the
/// `reader` and `reader-outline` scenes.
pub const READER_MARKDOWN: &str = r#"# The quiet page

A native Markdown reader should disappear while you read. This page is parsed
by `lmdv-md` on a background job and drawn by `lmdv-render`: the same pipeline
that renders every file the reader opens.

## What this page shows

- headings one to four, with their levels kept for the outline
- paragraphs, lists and quotes
- fenced code and a GFM table
- math, callouts, footnotes and a remembered reading position

Task checkboxes render in the viewer; ticking them writes the file once the
editor slice lands.

- [x] Parse the document without blocking the frame
- [x] Draw formulas with `lmdv-math`
- [ ] Make task ticks live in the viewer

### A first section

The reading column is 760 points wide, with generous side padding. Long lines
wrap at the column, not at the window. Inline math such as $E = mc^2$ sits in
the line, and a display formula gets its own centred row:

$$
\int_{-\infty}^{\infty} e^{-x^2}\,dx = \sqrt{\pi}
$$

#### A fourth-level heading

Fourth-level headings are the deepest the outline lists (O-01).

> [!NOTE]
> The outline is a map of the page, not a second table of contents.

A footnote reference follows this sentence.[^reader]

```rust
fn main() {
    println!("the reader is native");
}
```

## A table

| Surface | State |
| --- | --- |
| Parser | `lmdv-md` |
| Renderer | `lmdv-render` |
| Editor | P3 |

### Another nested section

A table keeps its header while the page scrolls.

## Reading positions

The viewport remembers where you stopped. Switching tabs keeps each document's
place, and reopening a file returns to it.

### What comes next

1. editing, save and drafts
2. multi-window and session restore
3. export and the remaining parity extras

[^reader]: Footnotes are collected at the end with their backlinks.

## Seven

A seventh section.

### Seven point one

A nested section.

## Eight

An eighth section.

## Nine

A ninth section.

## Ten

A tenth section closes the page.
"#;

/// The three synthetic documents' source.
pub const WELCOME_MARKDOWN: &str = r#"# Welcome to lmdv

A native Markdown viewer for Linux, built with Rune.

## What works now

- one tab per document per window
- the process-wide document registry
- generation-guarded background loading
- parsed Markdown with tables, math, callouts, footnotes and a `[TOC]`
- an outline, `--line` jumps and remembered reading positions

## What comes next

1. editing, save and drafts (P3)
2. multi-window and session restore (P4)
3. export, the file browser and the remaining parity extras

> This page is parsed by `lmdv-md` and drawn by `lmdv-render`, the same pipeline
> every opened file uses.
"#;

/// A second synthetic document.
pub const READING_MARKDOWN: &str = r#"# Reading notes

## A quiet page

The viewer should disappear while you read. Type is shaped once, tables keep
their header, and long notes never block the frame.

| Surface | State |
| --- | --- |
| Viewer | P2 rendered |
| Renderer | `lmdv-render` |
| Editor | P3 |

Inline math such as $E = mc^2$ and fenced code are drawn by the same pipeline
as every other block.
"#;

/// A third synthetic document.
pub const EDITING_MARKDOWN: &str = r#"# A note being edited

Checkboxes stay live in the viewer, and saving is always explicit.

- [x] Keep the buffer LF-normalized
- [ ] Write a draft one second after typing
- [ ] Ask before overwriting a changed file

```rust
let text = document.snapshot();
```
"#;

/// The reader with the Share menu open (X-01): six items in the original's
/// order and the H1 page-break checkbox, with rich copy enabled (X-02).
pub fn scene_share(state: &mut WindowState) {
    state.reset_to_start_page();
    state.open_synthetic("Sharing notes", EDITOR_MARKDOWN);
    state.left.set_open(false);
    state.right.set_open(false);
    if let Some(tab) = state.active_tab_mut() {
        tab.view.outline_hidden = true;
    }
    state.sync_outline();
    state.open_share_menu();
    state.ui.notice = None;
    state.ui.help = None;
}

/// Edit mode on a synthetic document, with the split and no dialogs.
pub fn scene_editor(state: &mut WindowState) {
    state.reset_to_start_page();
    state.open_synthetic("Editing notes", EDITOR_MARKDOWN);
    state.left.set_open(false);
    state.right.set_open(false);
    if let Some(tab) = state.active_tab_mut() {
        tab.view.outline_hidden = true;
    }
    state.sync_outline();
    let mut tx = Tx::default();
    state.enter_editor(None, &mut tx);
    state.open_find(true, &mut tx);
    state.set_find_query("source");
    state.set_find_replace("buffer");
    state.ui.notice = None;
    state.ui.help = None;
}

/// Edit mode on a generated large document: above the 64 KiB threshold `Auto`
/// selects block mode, so the source pane is the virtualized block column and
/// the header shows “Block mode” and the block count.
pub fn scene_editor_block(state: &mut WindowState) {
    state.reset_to_start_page();
    let text = block_scene_markdown();
    state.open_synthetic("A long document", &text);
    state.left.set_open(false);
    state.right.set_open(false);
    if let Some(tab) = state.active_tab_mut() {
        tab.view.outline_hidden = true;
    }
    state.sync_outline();
    let mut tx = Tx::default();
    state.enter_editor(None, &mut tx);
    state.ui.notice = None;
    state.ui.help = None;
}

/// The editor at the largest reading size (E-37): 22 pt reads, so the source
/// field draws at 19.25 pt on a 30.8 pt line. The scene leaves edit mode on
/// and sets an in-memory preference, so the snapshot shows the metrics without
/// rewriting `prefs.json`.
pub fn scene_editor_size(state: &mut WindowState) {
    scene_editor(state);
    state.scene_prefs(|prefs| prefs.reader.size = 22);
}

/// The block scene's source: about 130 KB in 1,200 blank-line blocks, so the
/// editor's measured threshold crosses and several fields fit one screen.
pub fn block_scene_markdown() -> String {
    use std::fmt::Write as _;

    let mut text = String::from(
        "# A long document\n\nA generated document for the large-file block editor. Every section is one blank-line block.\n\n",
    );
    for index in 0..1_200 {
        let _ = write!(
            text,
            "## Section {index}\nParagraph {index} keeps the blank-line block layout true while typing in a generated long document.\n\n"
        );
    }
    text
}

/// The source edited by the `editor` scene: headings, prose, a task list, math,
/// code and a table so both panes have something real to draw.
pub const EDITOR_MARKDOWN: &str = r#"# Editing notes

The source pane on the left is the canonical buffer; the preview on the
right is parsed 120 ms after the last keystroke.

## What the editor keeps

- [x] LF in the buffer, CRLF on write
- [x] A draft one second after typing
- [ ] A save check that fades after 1.4 seconds

> [!TIP]
> Press `Ctrl+E` again to return to reading; unsaved changes ask first.

Inline math such as $a^2 + b^2 = c^2$ stays in the line.

```rust
fn save() {
    println!("explicit saves only");
}
```

| Surface | State |
| --- | --- |
| Source | `lmdv-edit` |
| Preview | `lmdv-render` |
| Watcher | `lmdv-io` |

## A second section

Closing prose so the preview scrolls a little.
"#;

#[cfg(test)]
mod tests {
    use std::io::Write;

    use rune::ElementInfo;
    use tempfile::{NamedTempFile, TempDir};

    use super::*;
    use crate::test_support;

    fn window(temp: &TempDir) -> WindowState {
        WindowState::new(test_support::shared(temp))
    }

    /// Lays the state out headlessly and returns every element it drew.
    fn render(state: WindowState) -> Vec<ElementInfo> {
        let mut runtime = rune::Runtime::new(state, view, (1100.0, 760.0), rune::Env::new(true, 2.0));
        runtime.frame(0.0);
        runtime.elements()
    }

    /// Polls until the active tab has a model, or fails after five seconds.
    fn wait_for_reader(state: &mut WindowState) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while state.active_tab().is_none_or(|tab| tab.view.reader.is_none()) {
            assert!(std::time::Instant::now() < deadline, "the reader never received a model");
            let mut tx = Tx::default();
            state.poll(0.0, &mut tx);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn the_overflow_scene_opens_the_list_over_twelve_tabs() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        scene_tab_overflow(&mut state);
        assert_eq!(state.tabs().len(), 12);
        assert_eq!(state.active_index(), 1);
        assert!(state.tab_list_is_open());

        let mut runtime = rune::Runtime::new(state, view, (1280.0, 900.0), rune::Env::new(true, 2.0));
        runtime.frame(0.0);
        runtime.frame(1.0 / 60.0);
        runtime.frame(2.0 / 60.0);
        let elements = runtime.elements();
        assert!(
            elements.iter().any(|element| element.key.as_deref() == Some(crate::tab_list::TAB_LIST_BUTTON_KEY)),
            "the crowded strip shows the ⌄ button"
        );
        let rows = elements
            .iter()
            .filter(|element| element.key.as_deref().is_some_and(|key| key.starts_with("tab-list-menu-item")))
            .count();
        assert_eq!(rows, 13, "twelve tabs and New tab");
        assert!(
            elements.iter().any(|element| element.text.as_deref() == Some("\u{25cf}")),
            "the front tab is marked ●"
        );
    }

    #[test]
    fn the_start_scene_is_one_start_page_with_the_panes_closed() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        state.left.set_open(true);
        state.right.set_open(true);

        scene_start(&mut state);
        assert_eq!(state.tabs().len(), 1);
        assert!(state.active_tab().is_some_and(Tab::is_start));
        assert!(!state.left.is_open());
        assert!(!state.right.is_open());
    }

    #[test]
    fn the_tabs_scene_has_three_documents_and_no_start_page() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        scene_tabs(&mut state);

        assert_eq!(state.tabs().len(), 3);
        assert!(state.tabs().iter().all(|tab| !tab.is_start()));
        assert_eq!(state.active_index(), 1);
        assert_eq!(state.active_tab().map(Tab::title).as_deref(), Some("Reading notes"));
        assert!(state.left.is_open() && state.right.is_open());
        assert!(state.active_tab().and_then(Tab::document).is_some_and(|doc| {
            matches!(*doc.status.borrow(), DocStatus::Ready) && doc.text().text().contains("Reading notes")
        }));
    }

    #[test]
    fn the_help_scene_opens_the_dialog() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        scene_help(&mut state);
        assert_eq!(state.help_view(), Some(HelpView::Shortcuts));
        assert_eq!(state.tabs().len(), 3);

        scene_help_full(&mut state);
        assert_eq!(state.help_view(), Some(HelpView::Full));
        assert_eq!(state.tabs().len(), 3);
    }

    #[test]
    fn the_preview_stops_at_the_line_cap() {
        assert!(WELCOME_MARKDOWN.lines().count() <= PREVIEW_MAX_LINES);
        assert!(READING_MARKDOWN.lines().count() <= PREVIEW_MAX_LINES);
        assert!(EDITING_MARKDOWN.lines().count() <= PREVIEW_MAX_LINES);
    }

    #[test]
    fn the_reader_scene_renders_heading_roles_and_text() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        scene_reader(&mut state);

        let elements = render(state);
        let headings: Vec<&ElementInfo> =
            elements.iter().filter(|element| element.role == "heading" && element.level.is_some()).collect();
        assert!(headings.len() >= 13, "the fixture has thirteen headings, found {}", headings.len());
        assert!(
            headings.iter().any(|heading| heading.text.as_deref() == Some("The quiet page")),
            "the h1 text is in the tree"
        );
        assert!(headings.iter().any(|heading| heading.level == Some(4)), "h4 keeps its level");
        assert!(
            elements.iter().any(|element| element.role == "text"
                && element.text.as_deref().is_some_and(|t| t.contains("native Markdown reader"))),
            "paragraph text renders"
        );
        assert!(elements.iter().any(|element| element.role == "checkbox"), "task checkboxes render");
        assert!(elements.iter().any(|element| element.role == "note"), "the callout card renders");
        assert!(elements.iter().any(|element| element.role == "math"), "display math renders");
        assert!(
            elements.iter().any(|element| element.key.as_deref() == Some("fn-reader")),
            "the footnote entry renders"
        );
    }

    #[test]
    fn the_outline_scene_lists_headings_and_its_filter() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        scene_reader_outline(&mut state);

        let elements = render(state);
        assert!(
            elements
                .iter()
                .any(|element| element.role == "textbox" && element.label.as_deref() == Some("Filter headings"))
        );
        // OI-017: the rows are buttons (the original draws `<button>`s).
        let rows: Vec<&ElementInfo> = elements
            .iter()
            .filter(|element| element.role == "button" && element.label.as_deref().is_some_and(|l| !l.is_empty()))
            .collect();
        assert!(
            rows.iter().any(|row| row.label.as_deref() == Some("What this page shows")),
            "the outline lists the h2 as a button: {rows:#?}"
        );
        assert!(rows.iter().any(|row| row.label.as_deref() == Some("A fourth-level heading")));
        assert!(rows.iter().any(|row| row.item_selected), "the first heading is active at the top");
    }

    #[test]
    fn a_restore_request_scrolls_the_article_viewport() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let long = state.open_synthetic("Long", READER_MARKDOWN);
        state.open_synthetic("Other", "# Other\n");
        assert!(state.activate_tab(long));
        state.begin_restore(&lmdv_core::ScrollAnchor { id: String::new(), offset: 420.0 });

        let mut runtime = rune::Runtime::new(state, view, (1100.0, 760.0), rune::Env::new(true, 2.0));
        runtime.frame(0.0);
        runtime.update(|state, tx| state.apply_pending_scroll(tx));
        runtime.frame(0.0);

        let article = runtime
            .elements()
            .into_iter()
            .find(|element| element.key.as_deref() == Some(reader::ARTICLE_KEY))
            .expect("the article column");
        assert!((article.scrolled[1] - 420.0).abs() < 2.0, "scrolled to {}", article.scrolled[1]);
    }

    #[test]
    fn recent_times_are_compact() {
        assert_eq!(relative_time(0, 10_000).as_deref(), None, "no timestamp means no age");
        assert_eq!(relative_time(1_000, 900).as_deref(), None, "a future timestamp is ignored");
        assert_eq!(relative_time(1_000, 31_000).as_deref(), Some("just now"));
        assert_eq!(relative_time(0, 0).as_deref(), None);
        assert_eq!(relative_time(1_000, 1_000 + 12 * 60_000).as_deref(), Some("12 min"));
        assert_eq!(relative_time(1_000, 1_000 + 3 * 3_600_000).as_deref(), Some("3 h"));
        assert_eq!(relative_time(1_000, 1_000 + 5 * 86_400_000).as_deref(), Some("5 d"));
    }

    #[test]
    fn a_loaded_file_builds_the_reader_with_heading_text() {
        let temp = TempDir::new().unwrap();
        let mut file = NamedTempFile::new_in(temp.path()).unwrap();
        write!(file, "# Welcome\n\nA body paragraph.\n\n## Next section\n\nMore text.\n").unwrap();
        file.flush().unwrap();

        let shared = test_support::shared(&temp);
        let mut state = WindowState::new(shared);
        state.open_path(file.path().to_path_buf());
        wait_for_reader(&mut state);

        let tab = state.active_tab().expect("the file tab");
        let reader = tab.view.reader.as_ref().expect("the loaded reader");
        let headings: Vec<_> = reader.headings.iter().map(|heading| heading.text.as_str()).collect();
        assert_eq!(headings, vec!["Welcome", "Next section"]);
        assert_eq!(
            state.tab(tab.id).map(|tab| tab.full_title()).as_deref(),
            Some(file.path().file_name().unwrap().to_string_lossy().as_ref())
        );
    }

    #[test]
    fn a_render_failure_keeps_the_source_preview() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        state.open_synthetic("Too large", "small enough");
        if let Some(tab) = state.active_tab_mut() {
            tab.view.reader = None;
            tab.view.reader_revision = None;
            tab.view.reader_error = Some("This document is too large to render.".to_owned());
        }
        let elements = render(state);
        assert!(
            elements.iter().any(|element| element.role == "text" && element.text.as_deref() == Some("small enough"))
        );
        assert!(
            elements.iter().any(|element| element.role == "text"
                && element.text.as_deref().is_some_and(|t| t.contains("Source preview")))
        );
        assert!(elements.iter().any(|element| element.key.as_deref() == Some("document-note")));
    }

    #[test]
    fn a_pending_first_parse_shows_a_placeholder_not_the_source() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        state.open_synthetic("Note", "# A heading\n\nBody\n");
        if let Some(tab) = state.active_tab_mut() {
            tab.view.reader = None;
            tab.view.reader_revision = None;
            tab.view.reader_error = None;
        }
        let elements = render(state);
        assert!(elements.iter().any(|element| element.key.as_deref() == Some("document-status")));
        assert!(!elements.iter().any(|element| element.key.as_deref() == Some("document-lines")));
    }
}
