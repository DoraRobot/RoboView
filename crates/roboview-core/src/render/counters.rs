//! Render handle ledger (display-types spec §4 A6).
//!
//! The ledger counts, per display kind, how many GPU handles were created and
//! how many display objects that held one were dropped. The acceptance
//! criterion A6 runs repeated add → toggle-visibility ×10 → delete cycles and
//! requires the number of live handles at the end of the last round to equal
//! the first round (constant residency allowed). Because the headless unit
//! tests have no GPU, the two hook points are deliberate and thin:
//!
//! - **created** — noted by the upload entry of each display kind the moment
//!   a GPU handle is provisioned (`Renderer::upload`, `MeshPipeline::upload`,
//!   `LinePipeline::upload_path`/`upload_frame`/`upload_arrow`);
//! - **destroyed** — noted by the `Drop` implementation of each display type
//!   when it is removed from the scene *and* actually holds an uploaded
//!   handle (display types that were never uploaded — or overlay-only marker
//!   texts — do not decrement).
//!
//! One display object therefore balances its ledger row over an add/delete
//! cycle: the upload at add time increments, the drop at delete time
//! decrements. Two documented qualifications:
//!
//! - Re-uploading the same object (renderer rebuild after a target format or
//!   depth/sample change, plan §3.3) provisions a fresh handle and counts one
//!   more created event; the ledger measures upload events against display
//!   removals, not against wgpu's deferred buffer destruction (which is
//!   unobservable headless — the host's deferred destruction semantics make
//!   the real resource balance a corollary of this one).
//! - Visibility toggling never touches the ledger: hidden objects keep their
//!   handles (spec §6: visibility only skips drawing).
//!
//! The ledger is process-global (headless tests and the A6 manual run share
//! one counter per kind) and is read through the public [`snapshot`].

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use crate::displays::DisplayKind;

/// Per-kind created/destroyed counts since process start.
static HANDLE_LEDGER: LazyLock<Mutex<HashMap<&'static str, (u64, u64)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Record the provisioning of one GPU handle of `kind` (spec A6: render layer
/// keeps a per-kind created counter; called by the upload entry points).
pub(crate) fn note_uploaded(kind: DisplayKind) {
    let mut ledger = HANDLE_LEDGER.lock().expect("handle ledger poisoned");
    let entry = ledger.entry(kind.as_str()).or_insert((0, 0));
    entry.0 += 1;
}

/// Record the drop of one display object of `kind` that held an uploaded
/// handle (called from the display types' `Drop` implementations, gated on
/// the object actually carrying a handle — see the module docs).
pub(crate) fn note_object_dropped(kind: DisplayKind) {
    let mut ledger = HANDLE_LEDGER.lock().expect("handle ledger poisoned");
    let entry = ledger.entry(kind.as_str()).or_insert((0, 0));
    entry.1 += 1;
}

/// Process-global snapshot of the ledger: for each display kind (keyed by
/// [`DisplayKind::as_str`]), the `(created, destroyed)` counts since process
/// start. The live handle count of a kind is `created − destroyed`; the A6
/// acceptance cycle asserts this difference returns to its first-round value
/// after the deferred destruction of the last round has drained.
pub fn snapshot() -> HashMap<String, (u64, u64)> {
    let ledger = HANDLE_LEDGER.lock().expect("handle ledger poisoned");
    ledger
        .iter()
        .map(|(kind, counts)| ((*kind).to_string(), *counts))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::displays::{Frame, Marker, Mesh, Path, PointCloud};

    /// All counter-touching assertions live in this one test: the ledger is
    /// process-global, and concurrent writer tests would interleave their
    /// deltas. Display types that never hold a handle (plain constructors,
    /// no upload) must not write to the ledger when dropped, so this test
    /// also drops one of each and checks the ledger is untouched.
    #[test]
    fn ledger_balances_uploads_against_handle_holding_drops() {
        // Dropping displays that hold no GPU handle writes nothing: the
        // destroyed note is gated on an uploaded handle being present.
        {
            let _cloud = PointCloud::from_data(crate::io::PointCloudData {
                positions: Vec::new(),
                colors: None,
                bounds: None,
                format: crate::io::Format::Ply,
            });
            let _mesh = Mesh::from_data(crate::io::MeshData {
                positions: Vec::new(),
                normals: None,
                indices: None,
                bounds: None,
            });
            let _path = Path::from_data(crate::io::PathData {
                points: Vec::new(),
                bounds: None,
            });
            let _frame = Frame::new(glam::Vec3::ZERO, 1.0);
            let _text = Marker::text(glam::Vec3::ZERO, "label");
            let _arrow = Marker::arrow(glam::Vec3::ZERO, glam::Vec3::X);
        }
        let untouched = snapshot();
        assert!(
            untouched
                .values()
                .all(|&(created, destroyed)| created == 0 && destroyed == 0),
            "dropping handle-less displays must not touch the ledger: {untouched:?}"
        );

        // 50 add/delete rounds per kind (spec A6): one created note per add,
        // one destroyed note per delete, so the live count (created −
        // destroyed) returns to zero after every round.
        for kind in [
            DisplayKind::PointCloud,
            DisplayKind::Mesh,
            DisplayKind::Path,
            DisplayKind::Frame,
            DisplayKind::Marker,
        ] {
            for _ in 0..50 {
                note_uploaded(kind);
                assert_eq!(live(kind), 1, "one pending handle after the add");
                note_object_dropped(kind);
            }
            assert_eq!(live(kind), 0, "round {kind:?} ends balanced");
        }
    }

    /// Live count of one kind, read as a delta of the process-global ledger.
    /// Safe because this test file is the only ledger writer.
    fn live(kind: DisplayKind) -> u64 {
        let snapshot = snapshot();
        let &(created, destroyed) = snapshot
            .get(kind.as_str())
            .expect("kind must have an entry after a note");
        created - destroyed
    }
}
