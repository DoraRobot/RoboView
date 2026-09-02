//! Marker display type (display-types spec §7 F4): overlay text labels and
//! arrows with a head, UI-added.
//!
//! The two marker shapes share one display kind:
//!
//! - [`MarkerText`] is a viewport overlay (spec §6: text labels never
//!   participate in the depth protocol). Core holds only the data — anchor
//!   and text — and the pure projection the app's painter needs
//!   ([`render::anchor_to_screen`]); the text itself is rendered by the
//!   app's egui painter.
//! - [`MarkerArrow`] is real 3D line geometry: a shaft plus two short head
//!   lines, provisioned by [`render::LinePipeline::upload_arrow`]. The head
//!   is drawn as short line segments, not a triangle fan (spec §7 F4: the
//!   app has no mesh for the arrow cap; core generates the segment list).

use std::sync::Arc;

use glam::Vec3;

use crate::render;

use super::DisplayKind;

/// An overlay text label anchored at a world-space point (spec §7 F4).
///
/// This is data only — no GPU handle. The app projects [`anchor`] through
/// [`render::anchor_to_screen`] and paints the text with its egui painter,
/// so the label always sits on top of the scene and never occludes or is
/// occluded by it (spec §6 overlay policy).
pub struct MarkerText {
    /// The world-space point the label points at.
    pub anchor: Vec3,
    /// The label text, editable in the app's add dialog (spec A5).
    pub text: String,
}

/// A 3D arrow from `start` to `end` with a line-drawn head (spec §7 F4):
/// a shaft segment plus two short head lines angled off the shaft direction.
pub struct MarkerArrow {
    /// The arrow's tail, in world space.
    pub start: Vec3,
    /// The arrow's tip, in world space.
    pub end: Vec3,
    /// GPU representation of the shaft and head lines, present once the
    /// renderer has uploaded them. `None` before the first upload or while
    /// the endpoints are being edited.
    pub gpu: Option<Arc<render::LineMesh>>,
}

/// A marker display: either an overlay text label or a 3D arrow
/// (spec §7 F4).
pub enum Marker {
    /// An overlay text label: data only, painted by the app.
    Text(MarkerText),
    /// A 3D arrow with a head, drawn through the line pipeline.
    Arrow(MarkerArrow),
}

impl Marker {
    /// An overlay text label at `anchor` (spec F4: anchor editable, text
    /// editable, default English).
    pub fn text(anchor: Vec3, text: impl Into<String>) -> Self {
        Marker::Text(MarkerText {
            anchor,
            text: text.into(),
        })
    }

    /// A 3D arrow from `start` to `end`. The GPU handle starts empty; the
    /// host uploads it through [`render::LinePipeline::upload_arrow`] and
    /// stores the returned handle in the arrow's `gpu` field.
    pub fn arrow(start: Vec3, end: Vec3) -> Self {
        Marker::Arrow(MarkerArrow {
            start,
            end,
            gpu: None,
        })
    }
}

/// Report removals to the render handle ledger (spec A6): only arrows hold
/// GPU handles, so only an arrow display that was actually uploaded counts
/// a destroyed event when the marker leaves the scene; text labels and
/// never-uploaded arrows leave the ledger untouched.
impl Drop for Marker {
    fn drop(&mut self) {
        let uploaded = match self {
            Marker::Arrow(arrow) => arrow.gpu.is_some(),
            Marker::Text(_) => false,
        };
        if uploaded {
            render::counters::note_object_dropped(DisplayKind::Marker);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_marker_stores_anchor_and_text_without_a_gpu_handle() {
        let marker = Marker::text(Vec3::new(1.0, 2.0, 3.0), "label");
        match &marker {
            Marker::Text(text) => {
                assert_eq!(text.anchor, Vec3::new(1.0, 2.0, 3.0));
                assert_eq!(text.text, "label");
            }
            Marker::Arrow(_) => panic!("text() must build the Text variant"),
        }
    }

    #[test]
    fn arrow_marker_stores_the_endpoints_without_a_gpu_handle() {
        let marker = Marker::arrow(Vec3::ZERO, Vec3::X * 4.0);
        match &marker {
            Marker::Arrow(arrow) => {
                assert_eq!(arrow.start, Vec3::ZERO);
                assert_eq!(arrow.end, Vec3::X * 4.0);
                assert!(arrow.gpu.is_none());
            }
            Marker::Text(_) => panic!("arrow() must build the Arrow variant"),
        }
    }
}
