//! Native macOS menu bridge — the transport half of the 004 main-menu dual
//! path (004 spec §6 "主菜单栏策略" D5, plan §3.4; T2 spike → T10).
//!
//! The bridge owns the *transport and the tree's lifetime*, nothing else:
//!
//! * **injection timing**: the real tree (built by `ui::menu::build_native`,
//!   task T10) is handed to [`init_bridge_with_menu`] and installed with
//!   `muda::Menu::init_for_nsapp()` inside `RoboViewApp::new` — after
//!   winit's `applicationDidFinishLaunching` installs the default menu
//!   (`menu::initialize` precedes `dispatch_init_events`, which drives the
//!   eframe creation callback) and before the first frame paints. Until the
//!   main.rs wiring lands, the spike-era call site keeps compiling through
//!   the transitional [`init_bridge`] (single-argument; forwards a
//!   default-English tree — delete it with the wiring, T10 recipe);
//! * **idle wake-up**: `MenuEvent::set_event_handler` (a process-wide
//!   `OnceCell`, exactly one `Some` registration) captures the `egui`
//!   context and calls `request_repaint()`, so a click on an idle app wakes
//!   the eframe loop; events queue internally and [`BridgeCtx::drain`]
//!   collects them once per `App::update`. `request_repaint()` is reliable
//!   end-to-end; pre-scheduled `request_repaint_after` wake-ups are not
//!   (T2 spike measurements, plan §5) — hence the probe's re-arm loop below;
//! * **locale relabel**: [`BridgeCtx::relabel`] delegates to
//!   `ui::menu::relabel`, which walks the *items* layer only (a top-level
//!   `Menu` has no `set_text`) — the verified T2 mechanism, now over the
//!   real tree;
//! * **single-flight guard**: [`BridgeCtx::set_open_enabled`] mirrors the
//!   app's loading state onto the File → Open… item;
//! * **check reconciliation**: [`BridgeCtx::set_grid_checked`] /
//!   [`BridgeCtx::set_axes_checked`] sync the native check marks when the
//!   authoritative toggle state changes through a non-menu door (T13
//!   wiring; muda auto-toggles the mark on a menu click itself, so the menu
//!   door needs no reconcile — see `ui/menu.rs` module docs).
//!
//! Ownership: muda menu objects are `Rc`-based (macOS-implementation
//! handles) and therefore not `Send`/`Sync` — the bridge is a main-thread
//! object owned by the app struct, never a `static`, and the app keeps it
//! alive for the whole process lifetime (muda's native items dereference
//! their Rust-side child storage on click). The event queue it drains is
//! the one `Send`-safe piece and lives in a static.
//!
//! Non-macOS targets compile a no-op module (muda is a macOS-only
//! dependency, 004 spec §6: the gate is a compile requirement).

#[cfg(target_os = "macos")]
mod platform {
    use std::collections::VecDeque;
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    use eframe::egui;
    use muda::{Menu, MenuEvent};

    // The tree builder/relabeler (`ui::menu`): the bridge never shapes the
    // tree itself — T10 moved the spike's placeholder build there, so the
    // placeholder and the real tree cannot coexist.
    use super::super::menu;
    use super::super::texts::Locale;

    /// Environment variable that arms the relabel probe: ~3 s after
    /// startup the bridge rebuilds the real tree's labels to Chinese and
    /// back to English in the same frame. The automated smoke run cannot
    /// click a menu, so this drives the same items-layer `set_text` path
    /// that the spec's locale switch prescribes (004 spec §6: label
    /// rebuild walks the items layer) over the *production* relabel
    /// function. The env name is kept from the T2 spike so existing smoke
    /// scripts keep working.
    const PROBE_ENV: &str = "ROBOVIEW_SPIKE_REBUILD";

    /// Delay after which the relabel probe fires.
    const PROBE_AFTER: Duration = Duration::from_secs(3);

    /// The probe fires once the deadline is within this slack. macOS timer
    /// wake-ups quantize to a few ms and the very last sub-ms re-arm inside
    /// a frame can get lost (observed in the two-stage probe run), so the
    /// deadline check is made forgiving instead.
    const PROBE_SLACK: Duration = Duration::from_millis(20);

    /// The registered bridge: the muda tree plus the state the per-frame
    /// drain needs. Owned by the app (see the module docs for why it cannot
    /// be a static).
    pub(crate) struct BridgeCtx {
        /// Root menu installed as the `NSApplication` main menu. Built by
        /// `ui::menu::build_native` and kept alive here — muda's native
        /// items dereference their Rust-side child storage on click, so the
        /// app must keep the bridge alive for the whole process lifetime.
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
        /// `App::update`; the app maps each event through
        /// `ui::menu::action_from_id` and dispatches the `AppAction` (the
        /// single dispatch point of the dual path).
        pub(crate) fn drain(&mut self) -> Vec<MenuEvent> {
            let events: Vec<MenuEvent> = EVENT_QUEUE.lock().unwrap().drain(..).collect();
            self.poll_relabel_probe();
            events
        }

        /// Rebuild every translatable label of the native tree for
        /// `locale` (items-layer walk, `ui::menu::relabel`). Called by the
        /// locale-switch dispatch right before the app-wide `Locale` change
        /// takes effect. Returns the number of labels set.
        pub(crate) fn relabel(&self, locale: Locale) -> usize {
            menu::relabel(&self.menu, locale)
        }

        /// Mirror the app's loading state onto the File → Open… item
        /// (single-flight loading, 004 spec §6): `false` while a background
        /// load runs, `true` again once it settles. Call sites: the
        /// `start_background_load` / `poll_background_load` pair in main.rs
        /// (T10 wiring recipe).
        #[allow(dead_code)] // wired with the main.rs single-flight dispatch
        pub(crate) fn set_open_enabled(&self, enabled: bool) {
            menu::set_open_enabled(&self.menu, enabled);
        }

        /// Sync the native Grid check mark to the authoritative toggle
        /// state (T13 wiring: the toolbar/HUD doors of the same toggle).
        #[allow(dead_code)] // wired with the T13 toggle doors
        pub(crate) fn set_grid_checked(&self, checked: bool) {
            menu::set_grid_checked(&self.menu, checked);
        }

        /// Sync the native Axes check mark; see [`Self::set_grid_checked`].
        #[allow(dead_code)] // wired with the T13 toggle doors
        pub(crate) fn set_axes_checked(&self, checked: bool) {
            menu::set_axes_checked(&self.menu, checked);
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
        /// items-layer rebuilds of the *real* tree (zh → en) in the same
        /// frame. Until then, re-arm the eframe wake-up on every frame — an
        /// idle app otherwise never polls this again (a one-shot request
        /// made before the event loop starts is not reliably honored,
        /// observed in the spike runs).
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
            let set_zh = self.relabel(Locale::ZhCn);
            tracing::info!(labels = set_zh, "menu relabel probe: labels now Chinese");
            let set_en = self.relabel(Locale::En);
            tracing::info!(labels = set_en, "menu relabel probe: labels now English");
        }
    }

    /// Set on a successful bridge install; guards the init so the
    /// `MenuEvent` handler is registered exactly once (muda's own OnceCell
    /// would silently ignore a second registration, but the menu install
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

    /// Transitional entry serving the spike-era `main.rs` call site
    /// (`RoboViewApp::new` — `ui::menu_bridge::init_bridge(&cc.egui_ctx)`,
    /// a main.rs text this module cannot change until the T10 wiring
    /// pass): forwards the real tree built at the default English locale to
    /// [`init_bridge_with_menu`], so the app keeps compiling and running in
    /// the interim. The wiring replaces this call site with
    /// `init_bridge_with_menu(&ctx, ui::menu::build_native(locale))` using
    /// the sys-locale value, then deletes this shim (T10 integration
    /// recipe).
    #[allow(dead_code)] // remove together with the main.rs call site it serves
    pub(crate) fn init_bridge(ctx: &egui::Context) -> Option<BridgeCtx> {
        init_bridge_with_menu(ctx, menu::build_native(Locale::En))
    }

    /// Register the bridge: install the real menu tree built by
    /// `ui::menu::build_native(locale)` and register the process-wide event
    /// handler. Must run early in `App::new` (main thread, after winit
    /// created `NSApplication`). Returns `None` when a bridge is already
    /// registered (the tree argument is then dropped uninstalled — the
    /// caller should not have built one in that case).
    #[allow(dead_code)] // awaited by the main.rs wiring call (T10 recipe)
    pub(crate) fn init_bridge_with_menu(ctx: &egui::Context, menu: Menu) -> Option<BridgeCtx> {
        if REGISTERED.get().is_some() {
            tracing::warn!("native menu bridge already registered; skipping duplicate init");
            return None;
        }
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
        tracing::info!("native menu bridge registered (004 T10: real menu tree installed)");
        Some(bridge)
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    //! No-op: the native menu bar exists on macOS only; the Win/Linux
    //! in-window path lives in `ui/menu.rs` (`egui_menu_bar`).
}

pub(crate) use platform::*;
