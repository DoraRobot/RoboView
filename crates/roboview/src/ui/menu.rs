//! Main menu dual path (004 ui-blueprint spec §6 "主菜单栏策略", decision
//! D5; plan §3.4; task T10).
//!
//! # One vocabulary, two doors
//!
//! Both menu surfaces map into the same [`AppAction`] enum, so the app has
//! exactly one dispatch point no matter which door fired:
//!
//! * **macOS** — a native global menu bar injected into `NSApplication`
//!   with `muda` (the first submenu becomes the application menu). The tree
//!   is built by [`build_native`], installed by the transport half in
//!   `ui/menu_bridge.rs`, and *kept alive by the app*: muda menu objects
//!   are `Rc`-based (T2 spike, plan §5) and therefore not `Send`/`Sync`, so
//!   the app owns the bridge (which owns the tree) for the whole process
//!   lifetime — never a static.
//! * **Windows/Linux** — the same tree rendered as an in-window egui menu
//!   bar, [`egui_menu_bar`], consumed by `main.rs` on the non-macOS
//!   targets (macOS hides the in-window bar: the native bar replaces it).
//!
//! # Events, ids, and the locale rebuild
//!
//! Native clicks arrive as `muda::MenuEvent` values carrying a `MenuId` —
//! a plain `pub String` (T2 spike, plan §5: map events by string, not by
//! position). [`native::action_from_id`] maps the string of the fired id
//! through the pure [`action_from_id_str`] table. The id key space below is
//! stable and never positional, so later 004 waves (toolbar doors, 007
//! keyboard/accelerator work) can extend it without rewiring.
//!
//! A locale switch rebuilds every *translatable* label by walking the items
//! layer only — a top-level `Menu` has no `set_text` (spec §6); the
//! keyed-label table [`native::LABEL_KEYS`] is the single source of which
//! node translates to which text key. The self-named language entries are
//! deliberately absent from that table: they keep their own name under both
//! locales (003 spec §6.2, the switcher is its own stable identifier).
//!
//! # Tree shape (shared by both paths)
//!
//! ```text
//! [App menu (macOS only)]  Quit RoboView        — muda predefined quit,
//!                                                  native terminate: / Cmd+Q;
//!                                                  never emits a MenuEvent
//! File
//!   Open…                                          → Open
//!   Add… ▾  { Add frame…, Add marker… }           → AddFrame / AddMarker
//!   ───────────────────────────────
//!   Language ▾  { English, 中文（简体） }           → Language(En|ZhCn)
//!   ───────────────────────────────
//!   Grid (✓)                                       → ToggleGrid
//!   Axes (✓)                                       → ToggleAxes
//! ```
//!
//! The single Open… entry of the File menu (label `texts::ToolOpen`,
//! "Open…") matches the toolbar's single open door of the 004 layout; the
//! per-family open items the pre-004 File menu had move to the toolbar's
//! Open dropdown (004 spec §6) — until that door lands, the per-family
//! *dispatch* of `AppAction::Open` stays an app-side decision (main.rs
//! wiring picks the family, see the T10 integration report).
//!
//! The Grid/Axes toggles ride in the File tail because the 004 text-key
//! set (T9) has no "View" menu-title key yet and this module must not
//! inline unkeyed copy (003 spec §6.5). When a key exists, move the two
//! check items to a View submenu; their ids are stable, so no wiring
//! changes.
//!
//! # Check-item state ownership
//!
//! macOS check items display the authoritative toggle state (grid/axes,
//! default on). muda's macOS implementation auto-toggles the native check
//! before firing the click event, so a menu click keeps the native mark and
//! the app-side state in sync by itself; the two *other* doors of the same
//! toggles (toolbar buttons and viewport HUD badges, 004 spec §6) must
//! reconcile the native mark through [`native::set_grid_checked`] /
//! [`native::set_axes_checked`] when they change the state (wired with the
//! T13 viewport toggle work; until then the calls sit unused, the check
//! items only track their own clicks). The egui mirror renders the two
//! toggles as plain buttons this wave — no check visuals until the
//! authoritative state exists to render.
//!
//! # Dead-code note (delete as the wiring lands)
//!
//! The app crate is a binary, so rustc's `dead_code` analysis has no
//! external-interface notion for this module: until `main.rs` wires the
//! bridge drain and the egui bar (the 004 integration round after the
//! four-region skeleton, T11), every item here is unreachable from `main`
//! and would warn on every build. The module-wide allow below is the
//! single point to remove as each item finds its consumer; the unit tests
//! exercise the mapping and label tables in the meantime.
#![allow(dead_code)]

use super::texts::{self, Locale};
use eframe::egui;

// ---------------------------------------------------------------------------
// AppAction — the shared vocabulary of every menu/toolbar door
// ---------------------------------------------------------------------------

/// One semantic user action of the 004 main-menu surface (spec §6 D5):
/// native macOS items and in-window egui buttons both produce these, so the
/// app keeps exactly one dispatch point (`App::dispatch_action` in the
/// wiring recipe) instead of one handler per door (plan §3.4).
///
/// The enum holds the whole menu vocabulary up front — including actions no
/// T10 tree node can produce — so later consumers (toolbar, HUD toggles,
/// 007 shortcuts) reuse it instead of growing parallel action types.
/// Variants without a producing menu node this wave say so in their docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AppAction {
    /// File → Open…: open a file through the standard picker and load it in
    /// the background (the existing `RoboViewApp::open_file_dialog` flow).
    /// The single open door matches the toolbar Open▾ of the 004 layout;
    /// family selection is an app-side concern (see the module docs).
    Open,
    /// Fit the camera to the visible objects. Not produced by any T10 menu
    /// node — the Fit door is the toolbar button (004 spec §6, toolbar) —
    /// reserved here so the toolbar wiring maps into this enum unchanged.
    Fit,
    /// File → Add… → Add frame…: open the Add-frame dialog.
    AddFrame,
    /// File → Add… → Add marker…: open the Add-marker dialog.
    AddMarker,
    /// File → Grid: toggle the ground grid (Z = 0) of the viewport helper
    /// layer. The state itself lives in the viewport session state (004
    /// spec §6; T13 wires it).
    ToggleGrid,
    /// File → Axes: toggle the world-origin axes; see [`AppAction::ToggleGrid`].
    ToggleAxes,
    /// File → Language → self-named entry: switch the UI locale. The label
    /// of the entry is the language's own name under both locales (003 spec
    /// §6.2); the dispatch also rebuilds the native labels through the
    /// items layer.
    Language(Locale),
    /// Reset the camera to its default posture. Not produced by any T10
    /// menu node (no camera-reset door exists yet); reserved.
    ResetView,
    /// Quit the application. On macOS the App menu's Quit is a muda
    /// *predefined* item wired to the native `terminate:` action (Cmd+Q),
    /// so it never surfaces as a `MenuEvent` and no tree node maps to this
    /// variant; it exists for the Win/Linux window path and future doors
    /// (007). The in-window egui bar draws no Quit item — the window
    /// manager provides the close affordance there.
    Quit,
}

// ---------------------------------------------------------------------------
// Native node ids — the stable key space of the tree and its events
// ---------------------------------------------------------------------------

// The id of every native node. Structural nodes (menus/titles) and the
// predefined Quit never fire events; leaf ids map to AppActions through
// [`action_from_id_str`]. Ids are stable strings on purpose (T2 spike, plan
// §5: `MenuId` is a `pub String`); extending the tree later only appends.

/// The application submenu — macOS shows its first submenu as the app menu
/// (its title is replaced by the application name). Contains Quit.
const ID_APP: &str = "menu_app";
/// File menu (translates: `texts::MenuFile`).
const ID_FILE: &str = "menu_file";
/// File → Open… item → [`AppAction::Open`].
const ID_OPEN: &str = "menu_open";
/// File → Add… submenu (translates: `texts::ToolAdd`).
const ID_ADD: &str = "menu_add";
/// File → Add… → Add frame… → [`AppAction::AddFrame`].
const ID_ADD_FRAME: &str = "menu_add_frame";
/// File → Add… → Add marker… → [`AppAction::AddMarker`].
const ID_ADD_MARKER: &str = "menu_add_marker";
/// File → Language submenu (translates: `texts::LanguageMenu`).
const ID_LANGUAGE: &str = "menu_language";
/// File → Language → English — self-named, never translated.
const ID_LANG_EN: &str = "menu_language_en";
/// File → Language → 中文（简体） — self-named, never translated.
const ID_LANG_ZH: &str = "menu_language_zh";
/// File → Grid check item → [`AppAction::ToggleGrid`].
const ID_GRID: &str = "menu_grid";
/// File → Axes check item → [`AppAction::ToggleAxes`].
const ID_AXES: &str = "menu_axes";

/// Map the string of one fired native `MenuId` to its action — the single
/// mapping table of the macOS path (wrapper [`native::action_from_id`]) and
/// the pure core the unit tests pin. Structural ids (menus are never
/// actionable) and unknown strings map to `None`.
fn action_from_id_str(id: &str) -> Option<AppAction> {
    match id {
        ID_OPEN => Some(AppAction::Open),
        ID_ADD_FRAME => Some(AppAction::AddFrame),
        ID_ADD_MARKER => Some(AppAction::AddMarker),
        ID_LANG_EN => Some(AppAction::Language(Locale::En)),
        ID_LANG_ZH => Some(AppAction::Language(Locale::ZhCn)),
        ID_GRID => Some(AppAction::ToggleGrid),
        ID_AXES => Some(AppAction::ToggleAxes),
        // Structural titles, the app submenu, and any unknown string never
        // map (Fit/ResetView/Quit have no producing node this wave — see
        // the variant docs).
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// The in-window egui menu bar (Windows/Linux)
// ---------------------------------------------------------------------------

/// Draw the in-window menu bar of the Win/Linux targets — the same tree as
/// the macOS native bar, button-ized for a window menu row (spec §6 D5:
/// both paths map into the same [`AppAction`]). Every click pushes into
/// `actions`; the caller (`main.rs`) drains the vec through its single
/// dispatch point after the panel is drawn, which keeps one handler for
/// both doors.
///
/// `main.rs` calls this inside the top `TopBottomPanel` (replacing its
/// pre-004 `menu_bar`); macOS does not call it (the native bar replaces the
/// in-window one, spec §6).
///
/// This wave the two toggles render as plain buttons: the authoritative
/// grid/axes state lives in the viewport session state (T13), which the bar
/// does not see yet — the actions they push are the same ones the native
/// check items fire, so the dispatch and the later check rendering need no
/// rework. Likewise the Open… button has no loading state here; the app's
/// single-flight guard gates the dispatch (see the integration recipe) and
/// a later wave can extend this signature with an `enabled`/state argument.
pub fn egui_menu_bar(ui: &mut egui::Ui, locale: Locale, actions: &mut Vec<AppAction>) {
    egui::MenuBar::new().ui(ui, |ui| {
        if ui.button(texts::tool_open(locale)).clicked() {
            actions.push(AppAction::Open);
        }
        ui.menu_button(texts::tool_add(locale), |ui| {
            if ui.button(texts::objects_add_frame(locale)).clicked() {
                actions.push(AppAction::AddFrame);
                ui.close();
            }
            if ui.button(texts::objects_add_marker(locale)).clicked() {
                actions.push(AppAction::AddMarker);
                ui.close();
            }
        });
        ui.menu_button(texts::language_menu(locale), |ui| {
            for target in [Locale::En, Locale::ZhCn] {
                if ui.button(target.name()).clicked() {
                    actions.push(AppAction::Language(target));
                    ui.close();
                }
            }
        });
        let grid = ui
            .button(texts::toggle_grid(locale))
            .on_hover_text(texts::grid_toggle_tooltip(locale));
        if grid.clicked() {
            actions.push(AppAction::ToggleGrid);
        }
        let axes = ui
            .button(texts::toggle_axes(locale))
            .on_hover_text(texts::axes_toggle_tooltip(locale));
        if axes.clicked() {
            actions.push(AppAction::ToggleAxes);
        }
    });
}

// ---------------------------------------------------------------------------
// The macOS native muda tree
// ---------------------------------------------------------------------------

/// The native half of the dual path. Only compiles on macOS — muda is a
/// macOS-gated dependency and the 004 spec makes the gate a compile
/// requirement (muda's Linux implementation needs the LGPL gtk feature).
#[cfg(target_os = "macos")]
pub(crate) mod native {
    use muda::{CheckMenuItem, Menu, MenuId, MenuItem, MenuItemKind, PredefinedMenuItem, Submenu};

    use super::texts::TextKey;
    use super::{
        AppAction, ID_ADD, ID_ADD_FRAME, ID_ADD_MARKER, ID_APP, ID_AXES, ID_FILE, ID_GRID,
        ID_LANG_EN, ID_LANG_ZH, ID_LANGUAGE, ID_OPEN, Locale, action_from_id_str, texts,
    };

    /// Label of the App menu's Quit item. Deliberately constant — the T2
    /// spike verified quit and the app-menu titles stay in one language
    /// (macOS shows the app menu in the application's own language; the
    /// menu copy tables have no Quit key, and this module must not inline
    /// unkeyed translatable copy — an invariant, like `texts::WINDOW_TITLE`).
    const QUIT_LABEL: &str = "Quit RoboView";

    /// The initial check state of the Grid/Axes items: both helper layers
    /// start enabled (004 spec §6: the ground grid and the world-origin
    /// axes default to on).
    const INITIAL_TOGGLE_STATE: bool = true;

    /// The translatable nodes of the tree — every id whose label follows
    /// the locale, keyed to its text key (003 spec §6.5: the key table is
    /// the i18n boundary). This table is the single source for both the
    /// build-time labels ([`build_native`]) and the items-layer rebuild
    /// ([`relabel`]). The self-named language entries are absent on
    /// purpose: their label is the locale's own name and never switches.
    const LABEL_KEYS: &[(&str, TextKey)] = &[
        (ID_FILE, TextKey::MenuFile),
        (ID_OPEN, TextKey::ToolOpen),
        (ID_ADD, TextKey::ToolAdd),
        (ID_ADD_FRAME, TextKey::ObjectsAddFrame),
        (ID_ADD_MARKER, TextKey::ObjectsAddMarker),
        (ID_LANGUAGE, TextKey::LanguageMenu),
        (ID_GRID, TextKey::ToggleGrid),
        (ID_AXES, TextKey::ToggleAxes),
    ];

    /// The translatable copy of the node `id` in `locale`; `None` for the
    /// fixed-label nodes (app submenu, Quit, the self-named language
    /// entries) and unknown ids.
    fn keyed_copy(id: &str, locale: Locale) -> Option<&'static str> {
        LABEL_KEYS
            .iter()
            .find(|(key, _)| *key == id)
            .map(|(_, key)| texts::resolve(locale, *key))
    }

    /// Build the native macOS menu tree for `locale` (spec §6 D5; plan
    /// §3.4). The first appended submenu becomes the application menu (its
    /// title is replaced by the application name on macOS), so the tree
    /// starts with the app submenu holding Quit, then File. The app passes
    /// the tree to `ui::menu_bridge::init_bridge`, which installs it and
    /// keeps it alive; never store the tree in a static (plan §5).
    pub(crate) fn build_native(locale: Locale) -> Menu {
        let menu = Menu::new();

        // Application menu: macOS replaces the title with the app name, so
        // the text is a placeholder; Quit is the muda predefined item with
        // the native terminate: action and the Cmd+Q key equivalent.
        let app_menu = Submenu::with_id(ID_APP, texts::WINDOW_TITLE, true);
        app_menu
            .append(&PredefinedMenuItem::quit(Some(QUIT_LABEL)))
            .expect("append Quit to the app menu");
        menu.append(&app_menu).expect("append the app submenu");

        let file_menu = Submenu::with_id(ID_FILE, copy_of(ID_FILE, locale), true);

        // Open… — the single open door of the 004 layout (see the module
        // docs for the family question).
        file_menu
            .append(&MenuItem::with_id(
                ID_OPEN,
                copy_of(ID_OPEN, locale),
                true,
                None,
            ))
            .expect("append the Open item");

        // Add… submenu with the two create entries.
        let add_menu = Submenu::with_id(ID_ADD, copy_of(ID_ADD, locale), true);
        add_menu
            .append(&MenuItem::with_id(
                ID_ADD_FRAME,
                copy_of(ID_ADD_FRAME, locale),
                true,
                None,
            ))
            .expect("append the Add-frame item");
        add_menu
            .append(&MenuItem::with_id(
                ID_ADD_MARKER,
                copy_of(ID_ADD_MARKER, locale),
                true,
                None,
            ))
            .expect("append the Add-marker item");
        file_menu.append(&add_menu).expect("append the Add submenu");

        file_menu
            .append(&PredefinedMenuItem::separator())
            .expect("append the separator before Language");

        // Language submenu with the self-named entries (003 spec §6.2: each
        // entry is its own stable identifier under both locales).
        let language_menu = Submenu::with_id(ID_LANGUAGE, copy_of(ID_LANGUAGE, locale), true);
        language_menu
            .append(&MenuItem::with_id(
                ID_LANG_EN,
                Locale::En.name(),
                true,
                None,
            ))
            .expect("append the English entry");
        language_menu
            .append(&MenuItem::with_id(
                ID_LANG_ZH,
                Locale::ZhCn.name(),
                true,
                None,
            ))
            .expect("append the Chinese entry");
        file_menu
            .append(&language_menu)
            .expect("append the Language submenu");

        file_menu
            .append(&PredefinedMenuItem::separator())
            .expect("append the separator before the toggles");

        // Grid/Axes check items (see the module docs for why they live in
        // the File tail and how their state is owned). muda toggles the
        // native mark on click before the event fires, so the check display
        // tracks the menu clicks by itself; the other doors of the same
        // toggles reconcile through set_grid_checked/set_axes_checked.
        file_menu
            .append(&CheckMenuItem::with_id(
                ID_GRID,
                copy_of(ID_GRID, locale),
                true,
                INITIAL_TOGGLE_STATE,
                None,
            ))
            .expect("append the Grid check item");
        file_menu
            .append(&CheckMenuItem::with_id(
                ID_AXES,
                copy_of(ID_AXES, locale),
                true,
                INITIAL_TOGGLE_STATE,
                None,
            ))
            .expect("append the Axes check item");

        menu.append(&file_menu).expect("append the File submenu");
        menu
    }

    /// Build-time copy lookup of the tree construction: every node this
    /// function labels is keyed, so a missing row is a developer error
    /// caught at startup, not a silent gap.
    fn copy_of(id: &str, locale: Locale) -> &'static str {
        keyed_copy(id, locale).unwrap_or_else(|| panic!("no label key for menu node {id:?}"))
    }

    /// Map one fired native menu event id to its action. The pure string
    /// table is [`super::action_from_id_str`]; this wrapper is the drain
    /// entry the app calls once per event (menu_bridge drains, main.rs
    /// dispatches — T2 spike event flow).
    pub(crate) fn action_from_id(id: &MenuId) -> Option<AppAction> {
        action_from_id_str(&id.0)
    }

    /// Rebuild every translatable label of the tree for `locale` by walking
    /// the *items* layer only — a top-level `Menu` has no `set_text` (spec
    /// §6, T2 spike). Returns the number of labels set. Called by the
    /// locale-switch dispatch; the self-named language entries and the
    /// fixed labels (Quit) are skipped by design.
    pub(crate) fn relabel(menu: &Menu, locale: Locale) -> usize {
        let mut count = 0;
        for kind in menu.items() {
            count += relabel_kind(&kind, locale);
        }
        count
    }

    /// Relabel one tree node rooted at `kind`; recurses into submenus so
    /// every items layer of the tree is rebuilt.
    fn relabel_kind(kind: &MenuItemKind, locale: Locale) -> usize {
        match kind {
            MenuItemKind::Submenu(submenu) => {
                let mut count = 0;
                if let Some(copy) = keyed_copy(&submenu.id().0, locale) {
                    submenu.set_text(copy);
                    count += 1;
                }
                for nested in submenu.items() {
                    count += relabel_kind(&nested, locale);
                }
                count
            }
            MenuItemKind::MenuItem(item) => match keyed_copy(&item.id().0, locale) {
                Some(copy) => {
                    item.set_text(copy);
                    1
                }
                None => 0,
            },
            MenuItemKind::Check(item) => match keyed_copy(&item.id().0, locale) {
                Some(copy) => {
                    item.set_text(copy);
                    1
                }
                None => 0,
            },
            // Predefined (Quit, separators) and icon items keep their
            // labels under both locales.
            MenuItemKind::Predefined(_) | MenuItemKind::Icon(_) => 0,
        }
    }

    /// Single-flight loading guard: mirror the app's loading state onto the
    /// File → Open… item. Called by the app when a background load starts
    /// (`false`) and finishes (`true`); the menu disables itself while a
    /// load is in flight so at most one worker exists (the pre-004 File
    /// menu disabled its open entries the same way).
    pub(crate) fn set_open_enabled(menu: &Menu, enabled: bool) {
        apply_to_item(menu, ID_OPEN, "the Open item", |kind| match kind {
            MenuItemKind::MenuItem(item) => item.set_enabled(enabled),
            _ => log_kind_mismatch(ID_OPEN, "a MenuItem"),
        });
    }

    /// Reconcile the native Grid check mark with the authoritative toggle
    /// state after a change through another door (toolbar/HUD, 004 spec
    /// §6). muda auto-toggles the mark on a menu click itself, so the menu
    /// door needs no reconcile; the other doors call this instead of
    /// leaving the native mark stale.
    pub(crate) fn set_grid_checked(menu: &Menu, checked: bool) {
        apply_to_item(menu, ID_GRID, "the Grid check item", |kind| match kind {
            MenuItemKind::Check(item) => item.set_checked(checked),
            _ => log_kind_mismatch(ID_GRID, "a CheckMenuItem"),
        });
    }

    /// Reconcile the native Axes check mark; see [`set_grid_checked`].
    pub(crate) fn set_axes_checked(menu: &Menu, checked: bool) {
        apply_to_item(menu, ID_AXES, "the Axes check item", |kind| match kind {
            MenuItemKind::Check(item) => item.set_checked(checked),
            _ => log_kind_mismatch(ID_AXES, "a CheckMenuItem"),
        });
    }

    /// Log a tree/id mismatch — a developer error (an id moved to another
    /// item kind without updating its caller), never a runtime failure.
    fn log_kind_mismatch(id: &str, expected: &str) {
        tracing::warn!(
            id,
            "menu node {id} resolved to an unexpected kind; expected {expected}"
        );
    }

    /// Run `apply` on the node with the given id. The trees are tiny and
    /// the calls rare (state changes, not per-frame work), so the lookup
    /// walks depth-first instead of caching handles. The app keeps the
    /// bridge (and with it the tree) alive, which is what makes the walk
    /// sound; a missing node is a developer error and logs loudly.
    fn apply_to_item<F>(menu: &Menu, id: &str, what: &str, apply: F)
    where
        F: FnOnce(&MenuItemKind),
    {
        for kind in menu.items() {
            if let Some(found) = find_in_kind(&kind, id) {
                apply(&found);
                return;
            }
        }
        tracing::warn!(id, "menu node not found: {what}");
    }

    /// Depth-first search for the first node matching `id` (checked before
    /// descending, so an id collision between a submenu and its contents
    /// resolves to the outer node; the id space is unique by construction).
    fn find_in_kind(kind: &MenuItemKind, id: &str) -> Option<MenuItemKind> {
        if kind.id().0 == id {
            return Some(kind.clone());
        }
        if let MenuItemKind::Submenu(submenu) = kind {
            for nested in submenu.items() {
                if let Some(found) = find_in_kind(&nested, id) {
                    return Some(found);
                }
            }
        }
        None
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Every translatable node of the documented tree has exactly one
        /// label-table row, and every row resolves to copy in both locales
        /// (the texts coverage tests pin the key tables themselves; this
        /// test pins the *menu-side* id → key wiring to that key space).
        #[test]
        fn label_table_covers_the_translatable_nodes_exactly_once() {
            let mut seen = std::collections::HashSet::new();
            for (id, key) in LABEL_KEYS {
                assert!(seen.insert(*id), "duplicate label row for node {id:?}");
                assert!(!id.is_empty());
                assert!(
                    !texts::resolve(Locale::En, *key).is_empty(),
                    "En copy of {key:?}"
                );
                assert!(
                    !texts::resolve(Locale::ZhCn, *key).is_empty(),
                    "zh copy of {key:?}"
                );
            }
            // The documented translatable nodes are File, Open, Add, both
            // add entries, Language, and the two toggles — nothing more and
            // nothing less (a row added for a fixed-label node would set a
            // label the tree must keep stable).
            let documented: std::collections::HashSet<&&str> = [
                &ID_FILE,
                &ID_OPEN,
                &ID_ADD,
                &ID_ADD_FRAME,
                &ID_ADD_MARKER,
                &ID_LANGUAGE,
                &ID_GRID,
                &ID_AXES,
            ]
            .into_iter()
            .collect();
            let keyed: std::collections::HashSet<&&str> =
                LABEL_KEYS.iter().map(|(id, _)| id).collect();
            assert_eq!(keyed, documented);
        }

        /// The fired-id wrapper maps exactly like the pure string table
        /// (MenuId construction is pure data — no main-thread requirement).
        #[test]
        fn action_from_id_wrapper_matches_the_string_table() {
            for (id, action) in [
                (ID_OPEN, AppAction::Open),
                (ID_ADD_FRAME, AppAction::AddFrame),
                (ID_ADD_MARKER, AppAction::AddMarker),
                (ID_LANG_EN, AppAction::Language(Locale::En)),
                (ID_LANG_ZH, AppAction::Language(Locale::ZhCn)),
                (ID_GRID, AppAction::ToggleGrid),
                (ID_AXES, AppAction::ToggleAxes),
            ] {
                assert_eq!(action_from_id(&MenuId::new(id)), Some(action), "id {id:?}");
            }
            assert_eq!(action_from_id(&MenuId::new(ID_FILE)), None);
            assert_eq!(action_from_id(&MenuId::new("menu_unknown")), None);
        }

        /// The two toggle checks start on, mirroring the helper-layer
        /// defaults of the 004 spec (§6: grid and origin axes default on).
        #[test]
        fn toggle_checks_start_enabled() {
            assert!(INITIAL_TOGGLE_STATE);
        }
    }
}

// Crate-visible native entry points (same-crate glob idiom as
// menu_bridge.rs): the app wiring and the bridge consume them as
// `ui::menu::build_native` / `ui::menu::relabel` / `ui::menu::action_from_id`
// and the state mirrors.
#[cfg(target_os = "macos")]
// `action_from_id` stays unused until the main.rs wiring lands (it feeds
// the drain dispatch — the single door of the dual path, a main.rs call
// site outside this module's ownership). Remove the allow with that wiring.
#[allow(unused_imports)]
pub(crate) use native::{
    action_from_id, build_native, relabel, set_axes_checked, set_grid_checked, set_open_enabled,
};

// ---------------------------------------------------------------------------
// Unit tests (pure mapping, runs on every platform)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// The fired menu ids map to exactly the actions the tree documents —
    /// the drain dispatch's correctness table (T10 report: full-key map).
    #[test]
    fn fired_menu_ids_map_to_their_documented_actions() {
        let cases: &[(&str, AppAction)] = &[
            (ID_OPEN, AppAction::Open),
            (ID_ADD_FRAME, AppAction::AddFrame),
            (ID_ADD_MARKER, AppAction::AddMarker),
            (ID_LANG_EN, AppAction::Language(Locale::En)),
            (ID_LANG_ZH, AppAction::Language(Locale::ZhCn)),
            (ID_GRID, AppAction::ToggleGrid),
            (ID_AXES, AppAction::ToggleAxes),
        ];
        for (id, expected) in cases {
            assert_eq!(action_from_id_str(id), Some(*expected), "id {id:?}");
        }
        // The producing set is exactly the seven leaves: no other node of
        // the documented tree fires, so the mapping cannot drift into
        // returning actions for titles.
        let fired: Vec<&str> = cases.iter().map(|(id, _)| *id).collect();
        for structural in [ID_APP, ID_FILE, ID_ADD, ID_LANGUAGE] {
            assert!(
                !fired.contains(&structural),
                "structural id {structural:?} must never fire"
            );
            assert_eq!(
                action_from_id_str(structural),
                None,
                "structural id {structural:?}"
            );
        }
    }

    /// Unknown strings — including plausible future ids — never map; the
    /// un-wired vocabulary variants (Fit, ResetView, Quit) have no node and
    /// stay unreachable until their doors exist.
    #[test]
    fn unknown_and_unwired_ids_never_map_to_an_action() {
        for id in [
            "",
            "menu_quit",
            "menu_view",
            "menu_open_point_cloud",
            "menu_fit",
            "garbage",
        ] {
            assert_eq!(action_from_id_str(id), None, "id {id:?}");
        }
    }
}
