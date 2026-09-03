//! Native macOS menu bridge — 004 wiring spike (004 plan A3 / task T2).
//!
//! The spike validates the three claims of the 004 main-menu-bar strategy
//! (004 spec §6) that the full menu tree (T10, `ui/menu.rs`) will build on:
//!
//! * **injection timing**: `muda::Menu::init_for_nsapp()` runs inside
//!   `RoboViewApp::new` — i.e. after winit's `applicationDidFinishLaunching`
//!   installs the default menu (`menu::initialize` precedes
//!   `dispatch_init_events`, which drives the eframe creation callback) and
//!   before the first frame paints;
//! * **idle wake-up**: `MenuEvent::set_event_handler` (a process-wide
//!   `OnceCell`, exactly one `Some` registration) captures the `egui`
//!   context and calls `request_repaint()`, so a click on an idle app wakes
//!   the eframe loop; events queue internally and [`BridgeCtx::drain`]
//!   collects them once per `App::update`;
//! * **locale relabel**: rebuilding labels walks the *items* layer only (a
//!   top-level `Menu` has no `set_text`) and exercises `Submenu::set_text`
//!   / `MenuItem::set_text` on every reachable node.
//!
//! Ownership: muda menu objects are `Rc`-based (macOS-implementation
//! handles) and therefore not `Send`/`Sync` — the bridge is a main-thread
//! object owned by the app struct, never a `static`. The event queue it
//! drains is the one `Send`-safe piece and lives in a static.
//!
//! The spike tree is a placeholder (application-menu Quit plus
//! File → Language with English / 中文（简体） entries); its labels are spike
//! copy, deliberately not `ui::texts` keys — T10 sources the real copy.
//! Non-macOS targets compile a no-op module (muda is a macOS-only
//! dependency, 004 spec §6: the gate is a compile requirement).

#[cfg(target_os = "macos")]
mod platform {
    use std::collections::VecDeque;
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    use eframe::egui;
    use muda::{Menu, MenuEvent, MenuId, MenuItemKind};

    /// Environment variable that arms the relabel probe: ~3 s after
    /// startup the bridge relabels the spike tree to Chinese and back to
    /// English in the same frame. The automated smoke run cannot click a
    /// menu, so this drives the same items-layer `set_text` path that the
    /// spec's locale switch prescribes (004 spec §6: label rebuild walks
    /// the items layer).
    const PROBE_ENV: &str = "ROBOVIEW_SPIKE_REBUILD";

    /// Delay after which the relabel probe fires.
    const PROBE_AFTER: Duration = Duration::from_secs(3);

    /// The probe fires once the deadline is within this slack. macOS timer
    /// wake-ups quantize to a few ms and the very last sub-ms re-arm inside
    /// a frame can get lost (observed in the two-stage probe run), so the
    /// deadline check is made forgiving instead.
    const PROBE_SLACK: Duration = Duration::from_millis(20);

    /// Stable ids of the spike items: the drain log reports them verbatim,
    /// so a manual click test is identifiable per item. T10 maps ids to the
    /// `AppAction` enum instead.
    const ID_APP: &str = "menu_spike_app";
    const ID_FILE: &str = "menu_spike_file";
    const ID_OPEN: &str = "menu_spike_open";
    const ID_LANGUAGE: &str = "menu_spike_language";
    const ID_LANG_EN: &str = "menu_spike_lang_en";
    const ID_LANG_ZH: &str = "menu_spike_lang_zh";

    /// Spike copy for the relabel probe: (menu id, English, Chinese). The
    /// app-menu title, the quit item, and the self-named locale entries are
    /// intentionally absent (macOS replaces the first submenu title with the
    /// app name; quit and the locale entries keep their labels under both
    /// locales, mirroring the in-window menu).
    const SPIKE_LABELS: &[(&str, &str, &str)] = &[
        (ID_FILE, "File", "文件"),
        (ID_OPEN, "Open…", "打开…"),
        (ID_LANGUAGE, "Language", "语言"),
    ];

    /// The registered bridge: the muda tree plus the state the per-frame
    /// drain needs. Owned by the app (see the module docs for why it cannot
    /// be a static); muda's native items dereference their Rust-side child
    /// storage on click (muda 0.19.3 `platform_impl/macos`,
    /// `MenuItem::fire_menu_item_click`), so the app must keep it alive for
    /// the whole process lifetime.
    pub(crate) struct BridgeCtx {
        /// Root menu installed as the `NSApplication` main menu.
        menu: Menu,
        /// `egui` context of the hosting app: wake-up requests for the
        /// idle loop and for the relabel probe's future frames.
        ctx: egui::Context,
        /// Relabel probe state; `None` when the probe is unarmed or done.
        probe: Option<RelabelProbe>,
    }

    impl BridgeCtx {
        /// Take the queued native menu events since the last frame and run
        /// the relabel probe stages that are due. Called from the head of
        /// `App::update`; the spike logs each event, T10 dispatches them to
        /// `AppAction`.
        pub(crate) fn drain(&mut self) -> Vec<MenuEvent> {
            let events: Vec<MenuEvent> = EVENT_QUEUE.lock().unwrap().drain(..).collect();
            self.poll_relabel_probe();
            events
        }

        /// Schedule the relabel probe when [`PROBE_ENV`] is set.
        fn arm_relabel_probe(&mut self) {
            if std::env::var_os(PROBE_ENV).is_none() {
                return;
            }
            self.probe = Some(RelabelProbe {
                at: Instant::now() + PROBE_AFTER,
            });
            self.ctx.request_repaint_after(PROBE_AFTER);
            tracing::info!(env = PROBE_ENV, "menu relabel probe armed");
        }

        /// Fire the relabel probe once its deadline is reached: two full
        /// items-layer rebuilds (en → zh → en) in the same frame. Until
        /// then, re-arm the eframe wake-up on every frame — an idle app
        /// otherwise never polls this again (a one-shot request made before
        /// the event loop starts is not reliably honored, observed in the
        /// spike runs).
        fn poll_relabel_probe(&mut self) {
            let Some(probe) = self.probe.take() else {
                return;
            };
            let now = Instant::now();
            let remaining = probe.at.saturating_duration_since(now);
            if remaining > PROBE_SLACK {
                // Not due yet: re-arm the wake-up and keep waiting.
                self.ctx.request_repaint_after(remaining);
                self.probe = Some(probe);
                return;
            }
            let set_zh = self.rebuild_labels(true);
            tracing::info!(
                labels = set_zh,
                "menu relabel probe: spike labels now Chinese"
            );
            let set_en = self.rebuild_labels(false);
            tracing::info!(
                labels = set_en,
                "menu relabel probe: spike labels now English"
            );
        }

        /// Rebuild every translatable label of this bridge's tree. Returns
        /// the number of labels set; used by the relabel probe.
        fn rebuild_labels(&self, chinese: bool) -> usize {
            let mut count = 0;
            for kind in self.menu.items() {
                count += relabel_kind(&kind, chinese);
            }
            count
        }
    }

    /// Set on a successful bridge install; guards the init so the
    /// `MenuEvent` handler is registered exactly once (muda's own OnceCell
    /// would silently ignore a second registration, but the menu build
    /// below must not run twice either).
    static REGISTERED: OnceLock<()> = OnceLock::new();

    /// Events queued by the handler, drained once per frame from
    /// `App::update` (spec §6: polling the muda receiver per frame alone
    /// would miss clicks on an idle app — the wake-up must come from the
    /// handler).
    static EVENT_QUEUE: Mutex<VecDeque<MenuEvent>> = Mutex::new(VecDeque::new());

    /// One-shot probe schedule: when `at` is reached the probe relabels the
    /// tree to Chinese and back to English (two full items-layer rebuilds in
    /// one frame). The probe is dropped once fired.
    #[derive(Clone, Copy)]
    struct RelabelProbe {
        at: Instant,
    }

    /// Register the bridge: build and install the spike menu and register
    /// the process-wide event handler. Must run early in `App::new` (main
    /// thread, after winit created `NSApplication`). Returns `None` when a
    /// bridge is already registered.
    pub(crate) fn init_bridge(ctx: &egui::Context) -> Option<BridgeCtx> {
        if REGISTERED.get().is_some() {
            tracing::warn!("native menu bridge already registered; skipping duplicate init");
            return None;
        }
        // The menu tree must be built on the main thread; `App::new` is.
        let menu = build_spike_menu();
        let ctx = ctx.clone();
        let handler_ctx = ctx.clone();
        // Register the handler before installing the menu so no click can
        // slip between the two (muda dispatches on the main thread, so the
        // order is belt and braces in practice).
        MenuEvent::set_event_handler(Some(move |event| {
            // Wake the eframe loop; without this an idle app never reaches
            // the drain below.
            handler_ctx.request_repaint();
            if let Ok(mut queue) = EVENT_QUEUE.lock() {
                queue.push_back(event);
            }
        }));
        menu.init_for_nsapp();
        let _ = REGISTERED.set(());
        let mut bridge = BridgeCtx {
            menu,
            ctx,
            probe: None,
        };
        bridge.arm_relabel_probe();
        tracing::info!("native menu bridge registered (004 spike: main menu installed)");
        Some(bridge)
    }

    /// The placeholder spike tree: application menu (Quit) first — the
    /// first top-level submenu is the application menu on macOS — then
    /// File with a placeholder Open entry and the Language submenu, the
    /// same shape as the in-window menu (main.rs `menu_bar`).
    fn build_spike_menu() -> Menu {
        let menu = Menu::new();

        let app_menu = muda::Submenu::with_id(ID_APP, "RoboView", true);
        let quit = muda::PredefinedMenuItem::quit(Some("Quit RoboView"));
        app_menu.append(&quit).expect("append Quit to the app menu");
        menu.append(&app_menu).expect("append the app submenu");

        let file_menu = muda::Submenu::with_id(ID_FILE, "File", true);
        let open = muda::MenuItem::with_id(ID_OPEN, "Open…", true, None);
        file_menu
            .append(&open)
            .expect("append the Open placeholder");
        file_menu
            .append(&muda::PredefinedMenuItem::separator())
            .expect("append the separator");
        let language_menu = muda::Submenu::with_id(ID_LANGUAGE, "Language", true);
        // Self-named entries, like the in-window language menu: stable
        // identifiers under both locales (003 spec §6.2).
        let english = muda::MenuItem::with_id(ID_LANG_EN, "English", true, None);
        let chinese = muda::MenuItem::with_id(ID_LANG_ZH, "中文（简体）", true, None);
        language_menu
            .append(&english)
            .expect("append the English entry");
        language_menu
            .append(&chinese)
            .expect("append the Chinese entry");
        file_menu
            .append(&language_menu)
            .expect("append the Language submenu");

        menu.append(&file_menu).expect("append the File submenu");
        menu
    }

    /// Relabel one menu tree rooted at `kind`; returns the number of labels
    /// set. Recurses into submenus so every items layer of the tree is
    /// exercised (top-level `Menu` items are submenus here; a `Menu` itself
    /// has no `set_text` — spec §6).
    fn relabel_kind(kind: &MenuItemKind, chinese: bool) -> usize {
        match kind {
            MenuItemKind::Submenu(submenu) => {
                let mut count = 0;
                if let Some(text) = spike_label(submenu.id(), chinese) {
                    submenu.set_text(text);
                    count += 1;
                }
                for nested in submenu.items() {
                    count += relabel_kind(&nested, chinese);
                }
                count
            }
            MenuItemKind::MenuItem(item) => {
                if let Some(text) = spike_label(item.id(), chinese) {
                    item.set_text(text);
                    1
                } else {
                    0
                }
            }
            // Predefined (Quit, separator), check, and icon items keep
            // their labels under both locales.
            MenuItemKind::Predefined(_) | MenuItemKind::Check(_) | MenuItemKind::Icon(_) => 0,
        }
    }

    /// The spike label for `id` in the requested language, if the node
    /// carries a translatable label.
    fn spike_label(id: &MenuId, chinese: bool) -> Option<&'static str> {
        SPIKE_LABELS
            .iter()
            .copied()
            .find(|(key, _, _)| *key == id.0)
            .map(|(_, en, zh)| if chinese { zh } else { en })
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    //! No-op: the native menu bar of this spike exists on macOS only; the
    //! cross-platform tree lands in T10 (`ui/menu.rs`).
}

pub(crate) use platform::*;
