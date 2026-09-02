//! Scene: the camera and an ordered set of display objects.
//!
//! A [`Scene`] owns the view state ([`camera::OrbitCamera`]) and every
//! display object in add order. The display type `D` is an opaque payload
//! chosen by the caller — the app instantiates `Scene` with its concrete
//! display type — so the scene API never names a display type. The one
//! capability a payload must provide is [`HasBounds`], the world-space box
//! a scene-level framing operates on; each display type implements it next
//! to the type it describes (the point cloud, this feature's only display
//! type, right below the trait).
//!
//! Multi-object semantics (display-types spec §1 replaces the first
//! feature's single-slot swap): objects are appended, never replaced.
//! Every [`Scene::add`] stores the new object alongside the existing ones
//! and hands out a stable id drawn from a monotonic counter — an id
//! identifies one object for its whole scene lifetime and is never reused
//! after a removal (spec §4). Removal is [`Vec::remove`] followed by drop,
//! so wgpu frees the object's buffers through the host's deferred
//! destruction semantics. Visibility only gates iteration and the bounds
//! union ([`Scene::iter_visible`], [`Scene::bounds_union`]); toggling it
//! never frees data.
//!
//! The camera-framing policy lives in the app (display-types spec §6): the
//! first object added to an empty scene frames the whole scene, later adds
//! never move the camera. [`Scene::bounds_union`] supplies the combined
//! world-space bounds such framing — and the future Fit control — operate
//! on, folded here over the visible objects so the camera never sees
//! hidden content.

pub mod camera;

use glam::Vec3;

use crate::io::Aabb;
use camera::OrbitCamera;

/// An object of a [`Scene`]: stable id, display name, visibility, payload.
///
/// `id` is assigned by [`Scene::add`] and never reused; `name` is the
/// display copy supplied by the caller (a file stem or a generated label);
/// `visible` starts `true` and is flipped by [`Scene::toggle_visible`];
/// `object` is the display payload chosen when the scene was instantiated.
#[derive(Debug)]
pub struct SceneObject<D> {
    /// Stable handle identifying this object within the scene (spec §4).
    pub id: u64,
    /// Display name, as shown by the object list.
    pub name: String,
    /// Whether the object is drawn. Toggling never releases data.
    pub visible: bool,
    /// The display payload (e.g. a point cloud with its GPU handle).
    pub object: D,
}

/// A scene: one camera plus display objects in add order.
#[derive(Debug)]
pub struct Scene<D> {
    /// Camera used to view the scene. Public so the app can swap in a fresh
    /// framing pose when it decides the camera should move (first object
    /// added to an empty scene, spec §6).
    pub camera: OrbitCamera,
    /// Objects in add order (the list order of spec §4).
    objects: Vec<SceneObject<D>>,
    /// Id handed to the next added object; every add advances it, so
    /// removed ids are never handed out again.
    next_id: u64,
}

impl<D> Scene<D> {
    /// Create an empty scene (no objects) viewed through `camera`.
    pub fn new(camera: OrbitCamera) -> Self {
        Self {
            camera,
            objects: Vec::new(),
            next_id: 1,
        }
    }

    /// Whether the scene holds no object (the empty-viewport state).
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    /// Append `object` to the scene under `name`, assigning it a fresh
    /// stable id (spec §4). Returns that id; ids increase by one per add
    /// and are never reused, so the return value identifies the object for
    /// its whole scene lifetime.
    pub fn add(&mut self, object: D, name: impl Into<String>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.objects.push(SceneObject {
            id,
            name: name.into(),
            visible: true,
            object,
        });
        id
    }

    /// Remove the object with `id` from the scene and return its payload
    /// (dropping the entry frees whatever the payload owns). `None` when
    /// no object carries that id.
    pub fn remove(&mut self, id: u64) -> Option<D> {
        let index = self.objects.iter().position(|object| object.id == id)?;
        Some(self.objects.remove(index).object)
    }

    /// Flip the visibility of the object with `id` and return its new
    /// state (`true` = visible). Unknown ids are a no-op that reports
    /// `false`, indistinguishable from an object flipped to hidden — call
    /// [`Scene::get`] when the distinction matters.
    pub fn toggle_visible(&mut self, id: u64) -> bool {
        let Some(object) = self.objects.iter_mut().find(|object| object.id == id) else {
            return false;
        };
        object.visible = !object.visible;
        object.visible
    }

    /// The object with `id`, if it is still in the scene.
    pub fn get(&self, id: u64) -> Option<&SceneObject<D>> {
        self.objects.iter().find(|object| object.id == id)
    }

    /// Mutable access to the object with `id`, if it is still in the scene.
    pub fn get_mut(&mut self, id: u64) -> Option<&mut SceneObject<D>> {
        self.objects.iter_mut().find(|object| object.id == id)
    }

    /// All objects in add order.
    pub fn iter(&self) -> std::slice::Iter<'_, SceneObject<D>> {
        self.objects.iter()
    }

    /// Mutable access to all objects in add order (used to re-upload GPU
    /// handles when the renderer is rebuilt).
    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, SceneObject<D>> {
        self.objects.iter_mut()
    }

    /// The visible objects in add order — the set the renderer draws and
    /// the bounds union sees.
    pub fn iter_visible(&self) -> impl Iterator<Item = &SceneObject<D>> {
        self.objects.iter().filter(|object| object.visible)
    }

    /// The most recently added object, if any.
    pub fn last(&self) -> Option<&SceneObject<D>> {
        self.objects.last()
    }
}

impl<D: HasBounds> Scene<D> {
    /// Combined world-space bounds of the visible objects: the union of
    /// every [`HasBounds::bounds`] the visible objects report.
    ///
    /// `None` when the union has nothing to fold — the scene is empty,
    /// every object is hidden, or none of the visible objects has bounds
    /// (their payloads are all invalid). Callers feed the result to
    /// [`OrbitCamera::framing`], which already treats `None` as "frame
    /// nothing, fall back to the default pose".
    pub fn bounds_union(&self) -> Option<Aabb> {
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        let mut any = false;
        for object in self.iter_visible() {
            let Some(bounds) = object.object.bounds() else {
                continue;
            };
            min = min.min(bounds.min);
            max = max.max(bounds.max);
            any = true;
        }
        any.then_some(Aabb { min, max })
    }
}

/// World-space bounds of a display payload, the only capability [`Scene`]
/// requires beyond the payload type itself.
///
/// Implementors report `None` when the object has no finite extent (no
/// points survived validation, spec G1 of the first feature); the reported
/// box must be finite.
pub trait HasBounds {
    /// World-space bounds of this object, if it has any.
    fn bounds(&self) -> Option<Aabb>;
}

/// The point cloud's bounds are the bounds of its loaded data (the box
/// `io` computed over the finite points at load time, spec G1). The impl
/// sits here because the point cloud is the only display type so far;
/// kinds added later implement the trait where they are defined.
impl HasBounds for crate::displays::PointCloud {
    fn bounds(&self) -> Option<Aabb> {
        self.data.bounds
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test payload whose bounds are whatever the test supplies (`None`
    /// simulates an invalid object, e.g. a cloud without finite points).
    #[derive(Debug, Clone, Copy, PartialEq)]
    struct Payload(Option<Aabb>);

    impl HasBounds for Payload {
        fn bounds(&self) -> Option<Aabb> {
            self.0
        }
    }

    fn box_from(min: Vec3, max: Vec3) -> Aabb {
        Aabb { min, max }
    }

    fn scene_of(payloads: &[(Option<Aabb>, &str)]) -> Scene<Payload> {
        let mut scene = Scene::new(OrbitCamera::new(Vec3::ZERO));
        for (bounds, name) in payloads {
            scene.add(Payload(*bounds), *name);
        }
        scene
    }

    #[test]
    fn add_assigns_ids_in_add_order_and_stores_the_name() {
        let mut scene = Scene::new(OrbitCamera::new(Vec3::ZERO));
        assert!(scene.is_empty());

        let a = scene.add(Payload(None), "first");
        let b = scene.add(Payload(None), "second");
        let c = scene.add(Payload(None), "third");

        // Ids are handed out in add order, starting at 1.
        assert_eq!((a, b, c), (1, 2, 3));
        assert!(!scene.is_empty());

        // Names and add order are stored as given; get() finds by id.
        let names: Vec<&str> = scene.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(names, ["first", "second", "third"]);
        assert_eq!(scene.get(b).unwrap().name, "second");
        assert!(scene.get(b).unwrap().visible);
    }

    #[test]
    fn remove_drops_only_the_target_and_ids_are_never_reused() {
        let mut scene = scene_of(&[
            (Some(box_from(Vec3::ZERO, Vec3::ONE)), "a"),
            (Some(box_from(Vec3::splat(2.0), Vec3::splat(3.0))), "b"),
            (None, "c"),
        ]);

        // remove returns the payload of the removed object only.
        assert!(scene.remove(2).is_some());
        assert_eq!(scene.remove(2), None); // second removal: unknown id
        let rest: Vec<u64> = scene.iter().map(|o| o.id).collect();
        assert_eq!(rest, [1, 3]); // order of the survivors is preserved

        // The counter never rolls back: a fresh add takes a fresh id.
        let fresh = scene.add(Payload(None), "d");
        assert_eq!(fresh, 4);
        assert!(scene.get(2).is_none());
        assert_eq!(scene.remove(99), None);
    }

    #[test]
    fn toggle_visible_flips_the_state_and_unknown_ids_are_no_ops() {
        let mut scene = scene_of(&[(None, "a"), (None, "b")]);

        // add() starts every object visible; each toggle flips and reports
        // the new state.
        assert!(!scene.toggle_visible(1));
        assert!(!scene.get(1).unwrap().visible);
        assert!(scene.toggle_visible(1));
        assert!(scene.get(1).unwrap().visible);

        // Unknown ids toggle nothing and report false.
        let before: Vec<u64> = scene.iter().map(|o| o.id).collect();
        assert!(!scene.toggle_visible(42));
        let after: Vec<u64> = scene.iter().map(|o| o.id).collect();
        assert_eq!(before, after);
    }

    #[test]
    fn iter_visible_filters_hidden_objects_but_iter_keeps_everyone() {
        let mut scene = scene_of(&[(None, "a"), (None, "b"), (None, "c")]);
        scene.toggle_visible(2); // hide "b"

        let all: Vec<u64> = scene.iter().map(|o| o.id).collect();
        assert_eq!(all, [1, 2, 3]);
        let visible: Vec<u64> = scene.iter_visible().map(|o| o.id).collect();
        assert_eq!(visible, [1, 3]);
        assert_eq!(scene.last().unwrap().id, 3);
    }

    #[test]
    fn bounds_union_is_none_for_an_empty_scene() {
        let scene: Scene<Payload> = Scene::new(OrbitCamera::new(Vec3::ZERO));
        assert_eq!(scene.bounds_union(), None);
    }

    #[test]
    fn bounds_union_folds_the_visible_objects_boxes() {
        let scene = scene_of(&[
            (Some(box_from(Vec3::splat(-1.0), Vec3::splat(1.0))), "a"),
            (
                Some(box_from(
                    Vec3::new(2.0, 0.0, -4.0),
                    Vec3::new(4.0, 5.0, 6.0),
                )),
                "b",
            ),
        ]);
        assert_eq!(
            scene.bounds_union(),
            Some(box_from(
                Vec3::new(-1.0, -1.0, -4.0),
                Vec3::new(4.0, 5.0, 6.0)
            ))
        );
    }

    #[test]
    fn bounds_union_skips_objects_without_bounds_but_folds_the_rest() {
        // "b" reports no bounds (all points invalid); the union must still
        // describe "a" and "c".
        let scene = scene_of(&[
            (Some(box_from(Vec3::splat(-1.0), Vec3::splat(1.0))), "a"),
            (None, "b"),
            (Some(box_from(Vec3::splat(2.0), Vec3::splat(3.0))), "c"),
        ]);
        assert_eq!(
            scene.bounds_union(),
            Some(box_from(Vec3::splat(-1.0), Vec3::splat(3.0)))
        );

        // Every object without bounds: nothing to fold.
        let scene = scene_of(&[(None, "a"), (None, "b")]);
        assert_eq!(scene.bounds_union(), None);
    }

    #[test]
    fn bounds_union_ignores_hidden_objects() {
        let mut scene = scene_of(&[
            (Some(box_from(Vec3::splat(-1.0), Vec3::splat(1.0))), "a"),
            (Some(box_from(Vec3::splat(2.0), Vec3::splat(3.0))), "b"),
        ]);
        assert_eq!(
            scene.bounds_union(),
            Some(box_from(Vec3::splat(-1.0), Vec3::splat(3.0)))
        );

        // Hiding everything leaves no visible bounds to fold.
        scene.toggle_visible(1);
        scene.toggle_visible(2);
        assert_eq!(scene.bounds_union(), None);

        // Re-showing only "a" shrinks the union to its box.
        scene.toggle_visible(1);
        assert_eq!(
            scene.bounds_union(),
            Some(box_from(Vec3::splat(-1.0), Vec3::splat(1.0)))
        );
    }

    #[test]
    fn get_mut_grants_mutable_access_to_the_matching_object() {
        let mut scene = scene_of(&[(None, "a"), (None, "b")]);
        scene.get_mut(1).unwrap().name = "renamed".to_owned();
        assert_eq!(scene.get(1).unwrap().name, "renamed");
        assert!(scene.get_mut(42).is_none());
    }
}
