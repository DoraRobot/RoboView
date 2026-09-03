//! The bottom status band of the four-region skeleton (004 spec §6, tasks
//! T15/A9): the always-resident readout segments — load state, pointer
//! world coordinate, frames per second — plus the lightweight message
//! strip (004 spec D2: inline timestamps, per-level colors; the error
//! window of the 003 era coexists until the 007 message center migration).
//!
//! Layout of the 26 px band (single row, spec A7 readouts + the strip):
//!
//! ```text
//! [strip?][Loading…|Ready][Navigate]…… x=1.23 y=4.56 z=7.89 …… FPS: 60.2
//! ```
//!
//! - the **message strip** appears only while the app holds at least one
//!   error/warning ([`MessageItem`]); it shows the most recent
//!   [`MAX_VISIBLE_MESSAGES`] entries, newest first, each as a colored dot
//!   + `HH:MM:SS` clock + the message text elided to the strip's share of
//!   the row. Newlines are flattened so one message never grows a second
//!   line (the error templates of texts.rs are multi-line copy).
//! - the **load-state segment** resolves [`texts::ViewportLoading`] /
//!   [`texts::StatusReady`] in the current locale;
//! - the **tool segment** shows the current interaction-mode hint. 004
//!   introduces no tool state and the texts.rs key space is closed, so the
//!   default mode is the module constant [`TOOL_NAVIGATE`] — an
//!   untranslated mode token (like the axis letters), not user copy; an
//!   empty string hides the segment;
//! - the **coordinate segment** is centered on the remaining row width and
//!   prints the pointer's world intersection `x=… y=… z=…` ({:.2}, the
//!   scene's meter unit); while the pointer is outside the viewport or the
//!   ray misses the reference plane ([`pointer_world`] semantics: Z=0
//!   ground plane, or the camera-target plane when the grid is hidden) it
//!   shows dimmed `x=– y=– z=–` placeholders so the segment stays
//!   resident;
//! - the **FPS segment** is flush right: `{fps-label}: {value:.1}` with a
//!   `–` placeholder before the first recorded frame.
//!
//! Copy discipline: every user-facing word above flows through the keyed
//! tables of `texts.rs` — the readout labels resolve per frame in the
//! current locale. The two placeholders (`x=– y=– z=–`, the unrecorded
//! FPS) and the tool token are status glyphs, never translated (see the
//! table comments in texts.rs for the same distinction for axis letters).
//!
//! # Call sites (wired by the 004 T15 integration commit)
//!
//! The app owns one [`StatusBar`] and one `Vec<MessageItem>`; per frame
//! (main.rs `update`), before the bottom panel is drawn:
//!
//! ```text
//! self.status_bar.record(Duration::from_secs_f64(ctx.input(|i| i.unstable_dt)));
//! ```
//!
//! …and the bottom-panel body calls [`StatusBar::ui`] with a [`StatusInfo`]
//! snapshot assembled from app state (`loading` = a background load is in
//! flight, `pointer_world` = the per-frame intersection computed with
//! `roboview_core::render::camera_math::pointer_world`, `tool` =
//! [`TOOL_NAVIGATE`] until tools exist, `messages` = the app's message
//! log). The 26 px band needs its vertical inner margin trimmed (≈2 px,
//! instead of the shared 8 px region frame) so the row keeps its text
//! unclipped; the coordinate math itself is the task of the T13 viewport
//! wiring, which reports into `StatusInfo`.
//!
//! FPS smoothing is a plain sliding window of the most recent
//! [`MAX_FRAME_SAMPLES`] frame durations — one value per frame, O(1) per
//! record; the frame-rate readout is `frames / total_time` over the
//! window. Timestamps are formatted from [`std::time::SystemTime`] without
//! a time dependency (004 adds none, plan §2): the strip prints UTC clock
//! time (`HH:MM:SS`) — deterministic and unit-testable; locale-aware
//! wall-clock rendering belongs to the 007 message center.

// # Dead-code note (delete as the module gets wired)
//!
//! The app crate is a binary, so rustc's `dead_code` analysis has no
//! external-interface notion for this module: until the 004 T15
//! integration commit calls [`StatusBar::ui`] from main.rs, every public
//! item here is unreachable from `main` and would warn on every build. The
//! module-wide allow below is the single point to remove once the wiring
//! lands.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use eframe::egui;
use glam::Vec3;

use super::texts::{self, Locale};

/// Frame-time samples kept for the FPS readout: at 60 fps this is a one
/// second window, short enough that the readout still reacts to a load
/// change within a moment, long enough that it does not flicker per frame.
const MAX_FRAME_SAMPLES: usize = 60;

/// How many of the most recent messages the strip may show at once
/// (004 spec D2: a *lightweight* strip; the error window stays the
/// primary channel while it coexists).
pub const MAX_VISIBLE_MESSAGES: usize = 3;

/// Narrowest strip the message area may occupy; below this the row keeps
/// the three readout segments only (messages reappear once width allows).
const MIN_MESSAGE_STRIP_WIDTH: f32 = 96.0;

/// Diameter of the per-message severity dot, points.
const SEVERITY_DOT_SIZE: f32 = 6.0;

/// Seconds in one day — the clock-text period (the strip prints `HH:MM:SS`
/// only, so instants more than a day apart wrap the same way a clock does).
const SECONDS_PER_DAY: u64 = 86_400;

/// Tool hint of the default interaction mode, shown in the status bar's
/// tool segment (004 spec §6: the bar reports the current tool; A7 lists
/// it among the resident readouts). 004 defines no tool state beyond the
/// universal click-to-select / drag-to-orbit navigation, and the texts.rs
/// key space is closed, so this is an invariant mode token — like
/// [`texts::AXIS_X`], never translated — rather than keyed copy. An empty
/// string passed as `StatusInfo::tool` hides the segment. The tool system
/// of a later feature replaces the token with keyed copy.
pub const TOOL_NAVIGATE: &str = "Navigate";

/// Placeholder of the coordinate segment while the pointer has no world
/// intersection this frame (outside the viewport, or the reference-plane
/// ray misses). En dashes read as "no value" glyphs, not as minus signs;
/// the segment stays resident so the bottom row never shifts.
const NO_COORDS: &str = "x=– y=– z=–";

/// Placeholder of the FPS value before the first frame duration has been
/// recorded.
const NO_FPS: &str = "–";

/// Status bar owning the per-frame frame-time samples. The app owns one
/// instance across frames ([`StatusBar::record`] once per `update`,
/// [`StatusBar::fps`] feeds the readout the same frame).
pub struct StatusBar {
    /// Durations of the most recent frames, seconds, oldest first (ring
    /// capped at [`MAX_FRAME_SAMPLES`]).
    frame_seconds: VecDeque<f32>,
}

impl Default for StatusBar {
    fn default() -> Self {
        Self::new()
    }
}

impl StatusBar {
    /// An empty bar: no samples yet, the FPS readout shows its placeholder.
    pub fn new() -> Self {
        Self {
            frame_seconds: VecDeque::with_capacity(MAX_FRAME_SAMPLES),
        }
    }

    /// Record one finished frame's duration. Slides the window: older
    /// samples beyond [`MAX_FRAME_SAMPLES`] drop out. A zero duration is
    /// kept (it only lowers the mean), a stall shows up as one long sample
    /// and decays out of the window over the following frames.
    pub fn record(&mut self, frame_time: Duration) {
        self.frame_seconds.push_back(frame_time.as_secs_f32());
        while self.frame_seconds.len() > MAX_FRAME_SAMPLES {
            self.frame_seconds.pop_front();
        }
    }

    /// Smoothed frames per second over the recorded window; `None` before
    /// any sample exists or while every sample is zero (no meaningful
    /// rate). Same formula as [`fps_from_frame_seconds`], applied to the
    /// live deque without copying it.
    pub fn fps(&self) -> Option<f32> {
        if self.frame_seconds.is_empty() {
            return None;
        }
        let total: f32 = self.frame_seconds.iter().sum();
        if total <= 0.0 || !total.is_finite() {
            return None;
        }
        Some(self.frame_seconds.len() as f32 / total)
    }

    /// Draw the bottom status band: the message strip (when messages
    /// exist), the readout segments, and the flush-right FPS. Pure
    /// layout/paint code; the app assembles the per-frame [`StatusInfo`]
    /// snapshot and calls this inside the bottom `TopBottomPanel`.
    pub fn ui(&mut self, ui: &mut egui::Ui, locale: Locale, info: &StatusInfo<'_>) {
        let row_size = ui.available_size();
        let row = egui::Layout::left_to_right(egui::Align::Center);
        ui.allocate_ui_with_layout(row_size, row, |ui| {
            let spacing = ui.spacing().item_spacing.x;

            // Per-frame composed strings (all copy resolved in the locale;
            // values formatted below by the pure helpers).
            let state_text = if info.loading {
                texts::viewport_loading(locale)
            } else {
                texts::status_ready(locale)
            };
            let coords_text = match info.pointer_world {
                Some(point) => coords_text(point),
                None => NO_COORDS.to_owned(),
            };
            let fps_value = match self.fps() {
                Some(value) => fps_value_text(value),
                None => NO_FPS.to_owned(),
            };
            let fps_text = format!("{}: {fps_value}", texts::status_fps(locale));

            // Row budget: the fixed segments measured, the message strip
            // gets everything that remains (down to its minimum width), and
            // the leftover between the leading cluster and the coordinates
            // becomes the slack that centers the coordinate segment.
            let has_tool = !info.tool.is_empty();
            let tool_w = if has_tool {
                text_width(ui, info.tool)
            } else {
                0.0
            };
            let core_w = text_width(ui, state_text)
                + tool_w
                + text_width(ui, &coords_text)
                + text_width(ui, &fps_text);
            // Children of the row: [strip][state][tool][slack][coords][fps].
            let mut children = if has_tool { 5 } else { 4 };
            let mut strip_w = 0.0;
            if !info.messages.is_empty() {
                let free = ui.available_width() - core_w - spacing * children as f32;
                if free >= MIN_MESSAGE_STRIP_WIDTH {
                    strip_w = free;
                    children += 1;
                }
            }
            let gaps = spacing * (children - 1) as f32;
            let slack_w = (ui.available_width() - core_w - strip_w - gaps).max(0.0);

            if strip_w > 0.0 {
                message_strip(ui, strip_w, info.messages);
            }
            ui.label(state_text);
            if has_tool {
                ui.label(egui::RichText::new(info.tool).color(ui.visuals().weak_text_color()));
            }
            // The slack child centers the coordinate segment between the
            // leading cluster and the FPS segment (zero when messages take
            // the space; the row then reads strip…state…coords…FPS).
            ui.allocate_ui_with_layout(
                egui::vec2(slack_w, ui.available_height()),
                egui::Layout::left_to_right(egui::Align::Center),
                |_| {},
            );
            let coords_color = if info.pointer_world.is_some() {
                ui.visuals().text_color()
            } else {
                ui.visuals().weak_text_color()
            };
            ui.label(egui::RichText::new(coords_text).color(coords_color));

            // Flush right: a right-to-left scope paints from the row's far
            // edge, so the FPS readout always ends at the band's margin.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(fps_text);
            });
        });
    }
}

/// One lightweight message of the bottom strip (004 spec D2): severity
/// level, already-localized text, and the moment it happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageItem {
    /// Severity of the message; selects the strip's dot/time color
    /// (`error` red / `warning` orange of the theme's visuals).
    pub level: MessageLevel,
    /// The message, already resolved into the producing locale (the
    /// templates of texts.rs render in the locale of the moment the event
    /// happened; the strip re-renders the stored text, it never rekeys).
    pub text: String,
    /// When the message happened; the strip prints [`clock_text`] of it and
    /// sorts newest first. Stored as an instant (not as text) so ordering
    /// is exact and tests can inject fixed times.
    pub time: SystemTime,
}

/// Severity of a [`MessageItem`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageLevel {
    /// A failure (the strip colors it with the theme's error color).
    Error,
    /// A recoverable condition (the theme's warning color).
    Warning,
}

impl MessageItem {
    /// A message stamped with the current moment.
    pub fn new(level: MessageLevel, text: impl Into<String>) -> Self {
        Self::at(level, text, SystemTime::now())
    }

    /// A message with an explicit instant (deterministic construction for
    /// tests and for replaying recorded events).
    pub fn at(level: MessageLevel, text: impl Into<String>, time: SystemTime) -> Self {
        Self {
            level,
            text: text.into(),
            time,
        }
    }
}

/// Per-frame snapshot the app hands to [`StatusBar::ui`] (004 spec §6
/// 状态栏, A7). Assembled by the caller each frame — this module never
/// touches app or viewport state.
pub struct StatusInfo<'a> {
    /// True while a background file load is in flight: the state segment
    /// shows [`texts::ViewportLoading`] instead of [`texts::StatusReady`].
    pub loading: bool,
    /// World coordinate under the viewport pointer on the reference plane
    /// (Z=0 ground grid while shown, the camera-target plane while hidden —
    /// `roboview_core::render::camera_math::pointer_world`). `None` while
    /// the pointer is outside the viewport or the ray misses the plane; the
    /// segment then shows dimmed placeholders.
    pub pointer_world: Option<Vec3>,
    /// Current tool hint ([`TOOL_NAVIGATE`] in 004, until a tool state
    /// exists); an empty string hides the segment.
    pub tool: &'a str,
    /// The app's message log, oldest first; the strip picks the most recent
    /// [`MAX_VISIBLE_MESSAGES`].
    pub messages: &'a [MessageItem],
}

/// Frames per second over a window of per-frame durations (seconds):
/// `frames / total_time`. Equivalent to the harmonic mean of the per-frame
/// rates — the rate that reproduces the same frame count in the same total
/// time, which is what a frame-rate readout should report.
///
/// `None` for an empty window or when the total time is zero or not finite
/// (no sample, or every duration zero: division is undefined). Never
/// panics, never returns a non-finite value.
pub fn fps_from_frame_seconds(samples: &[f32]) -> Option<f32> {
    if samples.is_empty() {
        return None;
    }
    let total: f32 = samples.iter().sum();
    if total <= 0.0 || !total.is_finite() {
        return None;
    }
    Some(samples.len() as f32 / total)
}

/// One-decimal value text of the FPS readout ("60.2", en locale number
/// format; numerals are locale-independent glyphs).
pub fn fps_value_text(value: f32) -> String {
    format!("{value:.1}")
}

/// `x=… y=… z=…` text of one world point, two decimals (scene units are
/// meters; the coordinate segment shows centimeter resolution).
pub fn coords_text(point: Vec3) -> String {
    format!("x={:.2} y={:.2} z={:.2}", point.x, point.y, point.z)
}

/// `HH:MM:SS` clock text of `time`, seconds since the Unix epoch.
///
/// Pure and deterministic (a given instant always prints the same text),
/// so the unit tests pin exact strings. The clock reads **UTC**: std
/// offers no timezone conversion and 004 adds no time dependency (plan §2),
/// so the strip prints epoch-based clock time; locale-aware wall-clock
/// rendering belongs to the 007 message center. Instants before the epoch
/// clamp to the epoch.
pub fn clock_text(time: SystemTime) -> String {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let day = seconds % SECONDS_PER_DAY;
    let hours = day / 3_600;
    let minutes = day % 3_600 / 60;
    let seconds = day % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

/// The most recent `max` messages of `log`, newest first. The ordering is
/// stable: two messages with the same instant keep their log order (older
/// arrival first in the slice stays first between equals). Pure —
/// unit-tested with fixed instants.
pub fn recent_messages<'a>(log: &'a [MessageItem], max: usize) -> Vec<&'a MessageItem> {
    if max == 0 {
        return Vec::new();
    }
    let mut newest_first: Vec<&MessageItem> = log.iter().collect();
    newest_first.sort_by(|a, b| b.time.cmp(&a.time));
    newest_first.truncate(max);
    newest_first
}

/// Width of `text` laid out single-line in the row's body font, points.
/// The row budget splits the band's width between the strip and the fixed
/// segments by measuring the fixed ones once per frame.
fn text_width(ui: &egui::Ui, text: &str) -> f32 {
    let font_id = egui::TextStyle::Body.resolve(ui.style());
    let color = ui.visuals().text_color();
    ui.fonts(|fonts| {
        fonts
            .layout_no_wrap(text.to_owned(), font_id, color)
            .size()
            .x
    })
}

/// Paint the message strip: the newest [`MAX_VISIBLE_MESSAGES`] messages,
/// each in a share of `width` — severity dot, clock time (both in the
/// level color), then the text elided to the share. Messages are
/// `single_line`d first so the multi-line error templates of texts.rs can
/// never grow the 26 px band a second row.
fn message_strip(ui: &mut egui::Ui, width: f32, messages: &[MessageItem]) {
    let items = recent_messages(messages, MAX_VISIBLE_MESSAGES);
    let n = items.len() as f32;
    let spacing = ui.spacing().item_spacing.x;
    let share = ((width - spacing * (n - 1.0)) / n).max(0.0);
    let height = ui.available_height();
    ui.allocate_ui_with_layout(
        egui::vec2(width, height),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            for item in items {
                ui.allocate_ui_with_layout(
                    egui::vec2(share, ui.available_height()),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        let level_color = level_color(ui.visuals(), item.level);
                        let (dot_rect, _) = ui.allocate_exact_size(
                            egui::vec2(SEVERITY_DOT_SIZE, SEVERITY_DOT_SIZE),
                            egui::Sense::hover(),
                        );
                        ui.painter().circle_filled(
                            dot_rect.center(),
                            SEVERITY_DOT_SIZE * 0.5,
                            level_color,
                        );
                        ui.label(egui::RichText::new(clock_text(item.time)).color(level_color));
                        let text = single_line(&item.text);
                        ui.add(egui::Label::new(text).truncate());
                    },
                );
            }
        },
    );
}

/// Strip color of one severity: the theme's error (red) / warning (orange)
/// text colors — style values of the visuals, not palette tokens (004 spec
/// §6 keeps style values out of the A9 token set).
fn level_color(visuals: &egui::Visuals, level: MessageLevel) -> egui::Color32 {
    match level {
        MessageLevel::Error => visuals.error_fg_color,
        MessageLevel::Warning => visuals.warn_fg_color,
    }
}

/// Flatten `text` onto one line for the strip: CR/LF become single spaces,
/// a run of line breaks collapses to one space (a `\r\n` pair yields one
/// space, never two). Other whitespace passes through untouched. The
/// message stays readable when the strip paints it elided.
fn single_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut after_break = false;
    for ch in text.chars() {
        match ch {
            '\r' | '\n' => {
                if !after_break {
                    out.push(' ');
                }
                after_break = true;
            }
            _ => {
                out.push(ch);
                after_break = false;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    /// Deterministic instant: `seconds` after the Unix epoch.
    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    fn item(level: MessageLevel, text: &str, seconds: u64) -> MessageItem {
        MessageItem::at(level, text, at(seconds))
    }

    #[test]
    fn fps_from_frame_seconds_reports_frames_over_total_time() {
        // Ten 20 ms frames: 10 / 0.2 = 50 fps.
        let samples = vec![0.02; 10];
        let fps = fps_from_frame_seconds(&samples).expect("non-empty window");
        assert!((fps - 50.0).abs() < 1e-2, "got {fps}");
        // Mixed durations average the same way: 1 / mean(dt).
        let mixed = [0.01, 0.03, 0.02, 0.02];
        let fps = fps_from_frame_seconds(&mixed).expect("non-empty window");
        assert!((fps - 50.0).abs() < 1e-2, "got {fps}");
        // A single long frame (an app stall) drags the window down:
        // 31 samples totaling 1.6 s report 31 / 1.6 = 19.375 fps.
        let stalled = [0.02; 30].into_iter().chain([1.0]).collect::<Vec<_>>();
        let fps = fps_from_frame_seconds(&stalled).expect("non-empty window");
        assert!((fps - 19.375).abs() < 1e-2, "got {fps}");
    }

    #[test]
    fn fps_from_frame_seconds_is_none_without_meaningful_samples() {
        assert_eq!(fps_from_frame_seconds(&[]), None);
        assert_eq!(fps_from_frame_seconds(&[0.0]), None);
        assert_eq!(fps_from_frame_seconds(&[0.0, 0.0]), None);
    }

    #[test]
    fn status_bar_records_and_slides_the_window() {
        let mut bar = StatusBar::new();
        assert_eq!(bar.fps(), None);
        bar.record(Duration::from_secs_f32(0.02));
        let fps = bar.fps().expect("one sample");
        assert!((fps - 50.0).abs() < 1e-2, "got {fps}");
        // More records than the window cap: the oldest drop out, the window
        // stays bounded and the rate reflects the retained samples only.
        for _ in 0..10_000 {
            bar.record(Duration::from_secs_f32(0.01));
        }
        assert_eq!(bar.frame_seconds.len(), MAX_FRAME_SAMPLES);
        let fps = bar.fps().expect("window holds samples");
        assert!((fps - 100.0).abs() < 1e-2, "got {fps}");
    }

    #[test]
    fn fps_value_text_uses_one_decimal() {
        assert_eq!(fps_value_text(60.2), "60.2");
        assert_eq!(fps_value_text(0.0), "0.0");
        assert_eq!(fps_value_text(120.04), "120.0");
    }

    #[test]
    fn coords_text_prints_xyz_with_two_decimals() {
        assert_eq!(
            coords_text(Vec3::new(1.5, -2.25, 0.0)),
            "x=1.50 y=-2.25 z=0.00"
        );
        assert_eq!(
            coords_text(Vec3::new(0.1234, -0.1, 3.0)),
            "x=0.12 y=-0.10 z=3.00"
        );
    }

    #[test]
    fn clock_text_formats_hh_mm_ss_of_epoch_seconds() {
        assert_eq!(clock_text(at(0)), "00:00:00");
        assert_eq!(clock_text(at(3_723)), "01:02:03");
        assert_eq!(clock_text(at(86_400 * 2 + 45_296)), "12:34:56");
        assert_eq!(clock_text(at(86_399)), "23:59:59");
        // Past midnight the day wraps like a clock.
        assert_eq!(clock_text(at(90_000)), "01:00:00");
        // Instants before the epoch clamp to the epoch floor.
        let pre_epoch = UNIX_EPOCH - Duration::from_secs(1);
        assert_eq!(clock_text(pre_epoch), "00:00:00");
    }

    #[test]
    fn recent_messages_orders_newest_first_and_caps() {
        let log = vec![
            item(MessageLevel::Error, "oldest error", 100),
            item(MessageLevel::Warning, "middle warning", 200),
            item(MessageLevel::Error, "newest error", 300),
            item(MessageLevel::Warning, "second newest warning", 250),
        ];
        let recent: Vec<&str> = recent_messages(&log, MAX_VISIBLE_MESSAGES)
            .into_iter()
            .map(|m| m.text.as_str())
            .collect();
        assert_eq!(
            recent,
            ["newest error", "second newest warning", "middle warning"]
        );
        // A cap beyond the log returns everything, newest first.
        let all: Vec<&str> = recent_messages(&log, 10)
            .into_iter()
            .map(|m| m.text.as_str())
            .collect();
        assert_eq!(
            all,
            [
                "newest error",
                "second newest warning",
                "middle warning",
                "oldest error"
            ]
        );
        // A zero cap shows nothing.
        assert!(recent_messages(&log, 0).is_empty());
    }

    #[test]
    fn recent_messages_keeps_log_order_on_equal_instants() {
        let log = vec![
            item(MessageLevel::Error, "first of the same second", 100),
            item(MessageLevel::Warning, "second of the same second", 100),
            item(MessageLevel::Error, "third of the same second", 100),
        ];
        let texts: Vec<&str> = recent_messages(&log, MAX_VISIBLE_MESSAGES)
            .into_iter()
            .map(|m| m.text.as_str())
            .collect();
        // Stable: same-instant messages keep their arrival order.
        assert_eq!(
            texts,
            [
                "first of the same second",
                "second of the same second",
                "third of the same second"
            ]
        );
    }

    #[test]
    fn single_line_flattens_line_breaks_without_double_spacing() {
        assert_eq!(single_line("plain"), "plain");
        assert_eq!(single_line("line one\nline two"), "line one line two");
        assert_eq!(single_line("a\r\nb"), "a b");
        assert_eq!(single_line("a\n\nb"), "a b");
        assert_eq!(single_line("head\n\r\ntail"), "head tail");
        // Non-break whitespace is data, left untouched.
        assert_eq!(single_line("two  spaces\tkeep"), "two  spaces\tkeep");
    }

    #[test]
    fn message_item_new_stamps_now_and_at_injects() {
        let now = SystemTime::now();
        let made = MessageItem::new(MessageLevel::Error, "boom");
        // new() stamps the current moment: within a generous bound of now.
        let delta = made
            .time
            .duration_since(now)
            .or_else(|_| now.duration_since(made.time))
            .expect("instant near now");
        assert!(delta < Duration::from_secs(5), "stamped far from now");
        assert_eq!(made.level, MessageLevel::Error);
        assert_eq!(made.text, "boom");

        let fixed = MessageItem::at(MessageLevel::Warning, "slow", at(60));
        assert_eq!(fixed.time, at(60));
        assert_eq!(fixed.level, MessageLevel::Warning);
    }

    /// Smoke: the paint path runs headless in every state — ready and
    /// loading, with and without a pointer hit, with and without messages,
    /// tool hidden. egui's test context carries empty fonts (all measured
    /// widths are zero), so the assertions are panic-freedom and branch
    /// coverage, not pixels.
    #[test]
    fn ui_runs_headless_in_every_status_state() {
        let empty: &[MessageItem] = &[];
        let pointer = Some(Vec3::new(1.0, 2.0, 3.0));
        let run = |locale: Locale,
                   pointer_world: Option<Vec3>,
                   messages: &[MessageItem],
                   loading: bool,
                   tool: &str| {
            egui::__run_test_ui(|ui| {
                let mut bar = StatusBar::new();
                bar.record(Duration::from_secs_f32(0.02));
                bar.record(Duration::from_secs_f32(0.02));
                let info = StatusInfo {
                    loading,
                    pointer_world,
                    tool,
                    messages,
                };
                bar.ui(ui, locale, &info);
            });
        };
        // Ready/loading x pointer-on/off x tool shown/hidden.
        run(Locale::En, pointer, empty, false, TOOL_NAVIGATE);
        run(Locale::En, None, empty, false, TOOL_NAVIGATE);
        run(Locale::En, pointer, empty, true, TOOL_NAVIGATE);
        run(Locale::En, None, empty, true, "");
        // Message strip with a full log, in both locales.
        let log = vec![
            item(MessageLevel::Error, "could not open the file", 10),
            item(MessageLevel::Warning, "slow load", 20),
        ];
        run(Locale::En, pointer, &log, false, TOOL_NAVIGATE);
        run(Locale::ZhCn, None, &log, true, "");
        // A multi-line message must flatten, never grow a second row.
        let multiline = vec![MessageItem::at(
            MessageLevel::Error,
            "first line\nsecond line",
            at(5),
        )];
        run(Locale::En, None, &multiline, false, TOOL_NAVIGATE);
    }

    #[test]
    fn message_strip_allocates_only_recent_messages_budget() {
        // Budget bookkeeping is private; exercise the strip headless with a
        // log longer than the visible cap — painting must not panic and the
        // truncation path must run.
        let log: Vec<MessageItem> = (0..8)
            .map(|i| item(MessageLevel::Warning, &format!("message {i}"), i))
            .collect();
        egui::__run_test_ui(|ui| {
            message_strip(ui, 400.0, &log);
        });
    }
}
