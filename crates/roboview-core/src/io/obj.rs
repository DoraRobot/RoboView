//! OBJ parser for the locked subset (display-types spec §7 F1).
//!
//! Scope: ASCII files using the `v`, `vn` and `f` records only. Faces are
//! triangles — an `f` record must reference exactly three corners, and any
//! other count rejects the whole file (spec F1, "triangles only"). Corner
//! styles mix freely: bare `f v`, `f v/vt` (the `vt` part is ignored) and
//! `f v//vn` / `f v/vt/vn` (the `vn` part is range-checked against the
//! file's `vn` records but never used in shading, spec §6).
//!
//! Records outside the subset are ignored whole-line: `o`, `g`, `s`,
//! `mtllib`, `usemtl`, standalone `vt` records, `#` comments and blank
//! lines. The whole file is a single display object — `o`/`g` groups are
//! not split. A file without `f` records loads as a scatter
//! (`MeshData::indices` is `None`). Faces may reference vertices that
//! appear later in the file; record order is irrelevant.
//!
//! Rejected with [`ObjError`]: faces with other than three corners,
//! negative vertex/normal indices (spec F1: rejected explicitly, with the
//! index value), zero or out-of-range vertex and normal references,
//! `v`/`vn` records that do not carry exactly three numeric values, and
//! counts that exceed platform limits — the counting pre-validation (spec
//! §6) derives the record counts in a first pass and checks every
//! allocation size in u128 before any `Vec` is built.
//!
//! Non-finite coordinates (spec G1) are kept in `positions`; the bounding
//! box excludes them — `Aabb::from_points` is the single implementation.

use std::path::Path;

use glam::Vec3;

use super::ascii_text::{self, LineIter};
use super::{Aabb, MeshData, ObjError};

/// Load an OBJ mesh from a file path (spec F1).
pub fn load(path: &Path) -> Result<MeshData, ObjError> {
    let bytes = std::fs::read(path)?;
    parse_obj(&bytes)
}

/// Derived record counts of an OBJ body (counting pre-validation input).
#[derive(Debug, Clone, Copy, Default)]
struct Counts {
    vertices: u128,
    normals: u128,
    faces: u128,
}

/// First pass: count the `v`/`vn`/`f` records without parsing values.
/// Blank lines, comments and unknown keywords are skipped exactly as in
/// the second pass, so the counts bound every allocation of [`parse_obj`].
fn count_records(bytes: &[u8]) -> Counts {
    let mut counts = Counts::default();
    for line in LineIter::new(bytes) {
        match ascii_text::ws_tokens(line).first().copied() {
            Some(b"v") => counts.vertices += 1,
            Some(b"vn") => counts.normals += 1,
            Some(b"f") => counts.faces += 1,
            _ => {}
        }
    }
    counts
}

/// Allocation sizes for the record counts, checked in u128 before any
/// `Vec` exists (spec §6 allocation guard). Faces address vertices through
/// `u32` indices (0-based, i.e. 1-based references minus one), so a meshed
/// file is additionally capped at `2^32` vertices and normals.
fn allocation_sizes(counts: Counts) -> Result<(usize, usize, usize), ObjError> {
    let to_usize = |count: u128, what: &str| -> Result<usize, ObjError> {
        usize::try_from(count).map_err(|_| ObjError::Limit {
            reason: format!("{count} {what} records cannot fit in this platform's address space"),
        })
    };
    let vertices = to_usize(counts.vertices, "vertex")?;
    let normals = to_usize(counts.normals, "normal")?;
    let index_capacity = if counts.faces == 0 {
        0
    } else {
        // 2^32 == u32::MAX + 1: even the largest 1-based reference (2^32)
        // still stores as the 0-based u32::MAX.
        let index_space = u128::from(u32::MAX) + 1;
        if counts.vertices > index_space {
            return Err(ObjError::Limit {
                reason: format!(
                    "a meshed file with {} vertices exceeds the u32 index space",
                    counts.vertices
                ),
            });
        }
        if counts.normals > index_space {
            return Err(ObjError::Limit {
                reason: format!(
                    "a meshed file with {} normals exceeds the u32 index space",
                    counts.normals
                ),
            });
        }
        let corners = counts.faces.checked_mul(3).ok_or_else(|| ObjError::Limit {
            reason: format!(
                "the face record count {} overflows the counting arithmetic",
                counts.faces
            ),
        })?;
        to_usize(corners, "face corner")?
    };
    Ok((vertices, normals, index_capacity))
}

fn malformed(line: usize, reason: impl Into<String>) -> ObjError {
    ObjError::Malformed {
        line,
        reason: reason.into(),
    }
}

/// Second pass: parse and validate every record, appending into the
/// pre-sized vectors.
fn parse_obj(bytes: &[u8]) -> Result<MeshData, ObjError> {
    let counts = count_records(bytes);
    let (vertex_capacity, normal_capacity, index_capacity) = allocation_sizes(counts)?;

    let mut positions = Vec::with_capacity(vertex_capacity);
    let mut normals = Vec::with_capacity(normal_capacity);
    let mut indices = Vec::with_capacity(index_capacity);

    for (offset, line) in LineIter::new(bytes).enumerate() {
        let line_number = offset + 1; // 1-based physical line numbers.
        let tokens = ascii_text::ws_tokens(line);
        match tokens.first().copied() {
            Some(b"v") => positions.push(parse_vector_record("v", &tokens, line_number)?),
            Some(b"vn") => normals.push(parse_vector_record("vn", &tokens, line_number)?),
            Some(b"f") => parse_face(
                &tokens,
                line_number,
                counts.vertices,
                counts.normals,
                &mut indices,
            )?,
            // Blank lines, `#` comments and every unknown keyword
            // (`o`/`g`/`s`/`mtllib`/`usemtl`, standalone `vt` records, ...)
            // are ignored whole-line (spec F1): the file stays one display
            // object.
            _ => {}
        }
    }

    let bounds = Aabb::from_points(&positions);
    Ok(MeshData {
        positions,
        normals: (normal_capacity > 0).then_some(normals),
        indices: (index_capacity > 0).then_some(indices),
        bounds,
    })
}

/// Parse one `v` or `vn` record: exactly three numeric values after the
/// keyword. The record keyword is used in error messages.
fn parse_vector_record(keyword: &str, tokens: &[&[u8]], line: usize) -> Result<Vec3, ObjError> {
    if tokens.len() != 4 {
        return Err(malformed(
            line,
            format!(
                "{keyword} must declare exactly 3 numeric values, found {}",
                tokens.len() - 1
            ),
        ));
    }
    let mut values = [0.0f32; 3];
    for (slot, token) in values.iter_mut().zip(&tokens[1..]) {
        *slot = parse_value(token, line)?;
    }
    Ok(Vec3::new(values[0], values[1], values[2]))
}

/// Parse one numeric value of a `v`/`vn` record. The textual non-finite
/// spellings (`nan`/`inf`) parse for floats, so spec G1 values survive.
fn parse_value(token: &[u8], line: usize) -> Result<f32, ObjError> {
    ascii_text::parse_number::<f32>(token).map_err(|error| match error {
        ascii_text::NumberTokenError::NotUtf8 => {
            malformed(line, "numeric value is not valid UTF-8")
        }
        ascii_text::NumberTokenError::Invalid => malformed(
            line,
            format!("invalid number \"{}\"", String::from_utf8_lossy(token)),
        ),
    })
}

/// Parse one `f` record: exactly three corners (spec F1 triangles-only;
/// any other count rejects the whole file). Corners are `v`, `v/vt`,
/// `v//vn` or `v/vt/vn`; `vt` parts are ignored entirely and `vn` parts
/// are range-checked (spec §6) against the file's normal records.
fn parse_face(
    tokens: &[&[u8]],
    line: usize,
    vertex_count: u128,
    normal_count: u128,
    indices: &mut Vec<u32>,
) -> Result<(), ObjError> {
    if tokens.len() != 4 {
        return Err(malformed(
            line,
            format!(
                "faces must reference exactly 3 vertices (triangles only), found {}",
                tokens.len() - 1
            ),
        ));
    }
    for corner in &tokens[1..] {
        indices.push(parse_corner(corner, line, vertex_count, normal_count)?);
    }
    Ok(())
}

/// Parse one face corner: `v`, `v/vt`, `v//vn` or `v/vt/vn`. Returns the
/// 0-based vertex index. The `vt` part is ignored (never validated); a
/// present `vn` part is validated in range but does not participate in
/// shading (spec §6), so only its validity is checked.
fn parse_corner(
    corner: &[u8],
    line: usize,
    vertex_count: u128,
    normal_count: u128,
) -> Result<u32, ObjError> {
    let parts: Vec<&[u8]> = corner.split(|byte| *byte == b'/').collect();
    if parts.len() > 3 || parts[0].is_empty() {
        return Err(malformed(
            line,
            format!(
                "face corner \"{}\" must be v, v/vt, v//vn or v/vt/vn",
                String::from_utf8_lossy(corner)
            ),
        ));
    }
    let vertex = parse_reference(parts[0], "vertex", "vertices", vertex_count, line)?;
    if parts.len() == 3 && !parts[2].is_empty() {
        // Validated for range; the value itself is not part of the output.
        let _ = parse_reference(parts[2], "normal", "normals", normal_count, line)?;
    }
    Ok(vertex)
}

/// Parse one 1-based corner reference (a `v` or `vn` position). Negative
/// values are rejected explicitly with the index value (spec F1); zero is
/// invalid under the 1-based convention; values beyond the record count
/// are out of range. Returns the 0-based `u32` reference.
fn parse_reference(
    token: &[u8],
    kind: &str,
    plural: &str,
    count: u128,
    line: usize,
) -> Result<u32, ObjError> {
    if token.starts_with(b"-") {
        return Err(malformed(
            line,
            format!(
                "negative {kind} index {} on a face is not supported (OBJ indices are 1-based)",
                String::from_utf8_lossy(token)
            ),
        ));
    }
    let value = ascii_text::parse_number::<u64>(token).map_err(|error| match error {
        ascii_text::NumberTokenError::NotUtf8 => {
            malformed(line, format!("the {kind} index is not valid UTF-8"))
        }
        ascii_text::NumberTokenError::Invalid => malformed(
            line,
            format!(
                "invalid {kind} index \"{}\"",
                String::from_utf8_lossy(token)
            ),
        ),
    })?;
    if value == 0 {
        return Err(malformed(
            line,
            format!("{kind} index 0 is invalid: OBJ indices are 1-based"),
        ));
    }
    let index = u128::from(value);
    if index > count {
        return Err(malformed(
            line,
            format!("{kind} index {value} is out of range: the file declares {count} {plural}"),
        ));
    }
    // `index <= count <= 2^32` (allocation_sizes caps a meshed file), so
    // `value - 1` always fits a u32.
    Ok((value - 1) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(bytes: &[u8]) -> MeshData {
        parse_obj(bytes).expect("fixture must load")
    }

    fn parse_err(bytes: &[u8]) -> ObjError {
        parse_obj(bytes).expect_err("fixture must be rejected")
    }

    /// The (1-based line, reason) pair of a line-anchored rejection.
    fn malformed_of(bytes: &[u8]) -> (usize, String) {
        match parse_err(bytes) {
            ObjError::Malformed { line, reason } => (line, reason),
            other => panic!("expected a line-anchored error, got {other:?}"),
        }
    }

    fn indices_of(data: &MeshData) -> &[u32] {
        data.indices.as_deref().expect("fixture must carry faces")
    }

    #[test]
    fn scatter_without_faces() {
        // `v` without `f` → the whole file is a scatter (spec F1).
        let data = parse_ok(b"v 0 0 0\nv 1 0 0\nv 0 1 0");
        assert_eq!(
            data.positions,
            [
                Vec3::ZERO,
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0)
            ]
        );
        assert!(data.indices.is_none());
        assert!(data.normals.is_none());
        let bounds = data.bounds.expect("finite vertices must yield bounds");
        assert_eq!(bounds.min, Vec3::ZERO);
        assert_eq!(bounds.max, Vec3::new(1.0, 1.0, 0.0));
        assert_eq!(data.face_count(), 0);
    }

    #[test]
    fn empty_file_is_an_empty_scatter() {
        let data = parse_ok(b"");
        assert!(data.positions.is_empty());
        assert!(data.indices.is_none());
        assert!(data.normals.is_none());
        assert!(data.bounds.is_none());
    }

    #[test]
    fn normals_kept_without_faces() {
        // `vn` records are parsed and kept even when no face references
        // them (validated but unused in shading, spec §6).
        let data = parse_ok(b"v 0 0 0\nvn 1 0 0\nvn 0 1 0");
        let normals = data.normals.as_deref().expect("normals must be present");
        assert_eq!(normals, [Vec3::X, Vec3::Y]);
        assert!(data.indices.is_none());
    }

    #[test]
    fn triangle_mesh_with_normals() {
        let file = b"v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\n\
vn 0 0 1\nvn 0 0 1\nvn 0 0 1\nvn 0 0 1\n\
f 1//1 2//2 3//3\nf 1//1 3//3 4//4\n";
        let data = parse_ok(file);
        assert_eq!(data.vertex_count(), 4);
        assert_eq!(indices_of(&data), &[0, 1, 2, 0, 2, 3]);
        assert_eq!(data.face_count(), 2);
        let normals = data.normals.as_deref().expect("normals must be present");
        assert_eq!(normals.len(), 4);
        assert_eq!(normals[0], Vec3::Z);
    }

    #[test]
    fn mixed_face_styles_vt_ignored() {
        // `f v/vt`, `f v//vn` and bare `f v` mix freely in one file; the
        // `vt` part is ignored — including non-numeric and negative
        // values. Standalone `vt` record lines are ignored whole-line.
        let file =
            b"v 0 0 0\nv 1 0 0\nv 0 1 0\nv 1 1 0\nv 2 0 0\nv 2 1 0\nv 3 0 0\nv 3 1 0\nv 1 2 0\n\
vt 0.5 0.5\n\
vn 0 0 1\nvn 0 0 1\nvn 0 0 1\n\
f 1/1 2/2 3/3\n\
f 4//1 5//2 6//3\n\
f 7 8 9\n\
f 1/xyz 2/-2 3/3\n";
        let data = parse_ok(file);
        assert_eq!(indices_of(&data), &[0, 1, 2, 3, 4, 5, 6, 7, 8, 0, 1, 2]);
        assert_eq!(data.vertex_count(), 9);
    }

    #[test]
    fn unknown_records_are_ignored_whole_line() {
        // `o`/`g`/`s`/`mtllib`/`usemtl`, comments and blank lines are
        // ignored; the file stays a single display object across `o`
        // groups (spec F1).
        let file = b"# exported model\n\
o group one\n\
mtllib scene.mtl\n\
v 0 0 0\n\
v 1 0 0\n\
usemtl red\n\
o group two\n\
v 1 1 0\n\
v 0 1 0\n\
g extra\n\
s off\n\
f 1 2 4\n\
\n\
f 1 4 3\n";
        let data = parse_ok(file);
        assert_eq!(data.vertex_count(), 4);
        assert_eq!(indices_of(&data), &[0, 1, 3, 0, 3, 2]);
    }

    #[test]
    fn faces_may_precede_vertices() {
        // Record order is irrelevant: references are checked against the
        // whole file's counts, not against lines seen so far.
        let file = b"f 1 2 3\nv 0 0 0\nv 1 0 0\nv 0 1 0\n";
        let data = parse_ok(file);
        assert_eq!(indices_of(&data), &[0, 1, 2]);
    }

    #[test]
    fn negative_indices_rejected_with_value() {
        // A negative vertex index: readable error with the line and value.
        let (line, reason) = malformed_of(b"v 0 0 0\nv 1 0 0\nv 0 1 0\n\nf 1 -2 3\n");
        assert_eq!(line, 5, "blank lines must count into line numbers");
        assert!(
            reason.contains("-2"),
            "reason must carry the index: {reason}"
        );
        assert!(reason.contains("negative"), "reason: {reason}");

        // A negative normal index is rejected the same way.
        let (line, reason) = malformed_of(
            b"v 0 0 0\nv 1 0 0\nv 0 1 0\nvn 0 0 1\nvn 0 0 1\nvn 0 0 1\nf 1//-1 2//2 3//3\n",
        );
        assert_eq!(line, 7);
        assert!(
            reason.contains("negative normal index -1"),
            "reason: {reason}"
        );
    }

    #[test]
    fn zero_indices_rejected() {
        let (_, reason) = malformed_of(b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 0 2 3\n");
        assert!(reason.contains("index 0"), "reason: {reason}");
        assert!(reason.contains("1-based"), "reason: {reason}");
    }

    #[test]
    fn out_of_range_references_rejected() {
        // Vertex index beyond the file's `v` records.
        let (line, reason) = malformed_of(b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 4\n");
        assert_eq!(line, 4);
        assert!(
            reason.contains("vertex index 4 is out of range") && reason.contains("3 vertices"),
            "reason: {reason}"
        );

        // Normal index beyond the file's `vn` records.
        let (_, reason) =
            malformed_of(b"v 0 0 0\nv 1 0 0\nv 0 1 0\nvn 0 0 1\nvn 0 0 1\nf 1//1 2//3 3//2\n");
        assert!(
            reason.contains("normal index 3 is out of range") && reason.contains("2 normals"),
            "reason: {reason}"
        );

        // A normal reference with no `vn` records at all.
        let (_, reason) = malformed_of(b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1//1 2//1 3//1\n");
        assert!(reason.contains("0 normals"), "reason: {reason}");
    }

    #[test]
    fn non_triangle_faces_reject_the_whole_file() {
        // Four corners (and two corners): the message states the
        // triangles-only rule and names the offending line.
        let (line, reason) = malformed_of(b"v 0 0 0\nv 1 0 0\nv 0 1 0\nv 1 1 0\nf 1 2 3 4\n");
        assert_eq!(line, 5);
        assert!(reason.contains("triangles only"), "reason: {reason}");

        let (line, reason) = malformed_of(b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2\n");
        assert_eq!(line, 4);
        assert!(reason.contains("triangles only"), "reason: {reason}");

        // A valid face before the bad one does not save the file: the
        // whole load is rejected (spec F1 "whole file" semantics).
        let file = b"v 0 0 0\nv 1 0 0\nv 0 1 0\nv 1 1 0\nf 1 2 3\nf 1 2 3 4\n";
        assert!(matches!(
            parse_err(file),
            ObjError::Malformed { line: 6, .. }
        ));
    }

    #[test]
    fn malformed_corners_rejected() {
        // Too many slash-separated parts.
        let (line, reason) = malformed_of(b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1/2/3/4 2 3\n");
        assert_eq!(line, 4);
        assert!(reason.contains("v/vt/vn"), "reason: {reason}");

        // Missing vertex part.
        let (_, reason) = malformed_of(b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf /1 2 3\n");
        assert!(
            reason.contains("missing") || reason.contains("v, v/vt"),
            "reason: {reason}"
        );

        // A non-numeric vertex part.
        let (_, reason) = malformed_of(b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf x 2 3\n");
        assert!(
            reason.contains("invalid vertex index \"x\""),
            "reason: {reason}"
        );

        // An empty vn slot is fine (`f v//` has no normal part).
        let data = parse_ok(b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1// 2// 3//\n");
        assert_eq!(indices_of(&data), &[0, 1, 2]);
    }

    #[test]
    fn vector_record_shape_errors() {
        // Too few or too many values on `v`/`vn` records, and non-numeric
        // values — all with their line numbers.
        let (line, reason) = malformed_of(b"v 0 0 0\nv 1 2\n");
        assert_eq!(line, 2);
        assert!(
            reason.contains("exactly 3 numeric values, found 2"),
            "reason: {reason}"
        );

        let (line, reason) = malformed_of(b"vn 0 0 1\nvn 1 2 3 4\n");
        assert_eq!(line, 2);
        assert!(
            reason.contains("vn must declare exactly 3"),
            "reason: {reason}"
        );

        let (line, reason) = malformed_of(b"v 1 2 three\n");
        assert_eq!(line, 1);
        assert!(reason.contains("three"), "reason: {reason}");
    }

    #[test]
    fn crlf_and_missing_final_newline() {
        let file = b"v 0 0 0\r\nv 1 0 0\r\nv 0 1 0\r\nvn 0 0 1\r\nvn 0 0 1\r\nvn 0 0 1\r\n\
f 1//1 2//2 3//3\r\nf 1//1 2//2 3//3";
        let data = parse_ok(file);
        assert_eq!(data.vertex_count(), 3);
        assert_eq!(indices_of(&data), &[0, 1, 2, 0, 1, 2]);
    }

    #[test]
    fn non_finite_vertices_kept_bounds_exclude_them() {
        let file = b"v nan inf 0\nv -inf 1 2\nv 3 4 5\nf 1 2 3\n";
        let data = parse_ok(file);
        assert!(data.positions[0].x.is_nan());
        assert_eq!(data.positions[0].y, f32::INFINITY);
        assert_eq!(data.positions[1].x, f32::NEG_INFINITY);
        assert_eq!(indices_of(&data), &[0, 1, 2]);
        let bounds = data.bounds.expect("one finite vertex must yield bounds");
        assert_eq!(bounds.min, Vec3::new(3.0, 4.0, 5.0));
        assert_eq!(bounds.max, Vec3::new(3.0, 4.0, 5.0));

        let data = parse_ok(b"v nan nan nan\nv inf 0 0\n");
        assert!(data.bounds.is_none());
    }

    #[test]
    fn keyword_boundary_kept_unknown_lines_ignored() {
        // `vx` is not `v`: it is an unknown keyword, ignored whole-line.
        // A bare `v` (no values) is a `v` record and must be rejected.
        let data = parse_ok(b"vx 1 2 3\nvp 4 5\nv 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n");
        assert_eq!(data.vertex_count(), 3);
        let (line, _) = malformed_of(b"v 0 0 0\nv\n");
        assert_eq!(line, 2);
    }

    #[test]
    fn counting_prevalidation_rejects_unrepresentable_counts() {
        // The allocation guard runs before any `Vec` exists. These inputs
        // cannot come from an in-memory fixture (record counts are bounded
        // by the file size), so the guard is exercised directly.
        let over_index_space = |faces: u128| Counts {
            vertices: u128::from(u32::MAX) + 2,
            faces,
            ..Counts::default()
        };
        assert!(matches!(
            allocation_sizes(over_index_space(1)),
            Err(ObjError::Limit { .. })
        ));
        assert!(matches!(
            allocation_sizes(Counts {
                vertices: u128::MAX,
                ..Counts::default()
            }),
            Err(ObjError::Limit { .. })
        ));
        assert!(matches!(
            allocation_sizes(Counts {
                vertices: 3,
                faces: u128::MAX,
                ..Counts::default()
            }),
            Err(ObjError::Limit { .. })
        ));
        assert!(matches!(
            allocation_sizes(Counts {
                vertices: 3,
                normals: u128::from(u32::MAX) + 2,
                faces: 1,
            }),
            Err(ObjError::Limit { .. })
        ));
        // A scatter (no faces) of a huge count trips the platform check,
        // while sane counts pass through.
        let (vertices, normals, indices) =
            allocation_sizes(Counts::default()).expect("zero counts must be fine");
        assert_eq!((vertices, normals, indices), (0, 0, 0));
        let (vertices, normals, indices) = allocation_sizes(Counts {
            vertices: 9,
            normals: 3,
            faces: 4,
        })
        .expect("moderate counts must pass");
        assert_eq!((vertices, normals, indices), (9, 3, 12));
    }
}
