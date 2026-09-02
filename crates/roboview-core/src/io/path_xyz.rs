//! XYZ/CSV path parser for the locked subset (display-types spec §7 F2).
//!
//! Scope: text files with one point per line, three values separated by
//! runs of spaces, tabs or commas — the `[,\t ]+` delimiter class. A line
//! starting with `#` is a whole-line comment (trailing comments are not
//! supported and surface as a value-count error). Blank lines are skipped.
//! The first non-empty, non-comment line that does not carry exactly three
//! numeric values is skipped as a title row; every later line must be a
//! point, otherwise the file is rejected with a line-numbered error.
//!
//! The whole file is one open polyline through the points, in file order:
//! there is no closing or multi-segment concept (a coincident first and
//! last point is not a closure). Files with fewer than two points are
//! rejected with [`PathError::TooFewPoints`]. The counting pre-validation
//! (spec §6) derives the point count in a first pass and checks it in u128
//! before any `Vec` is built.
//!
//! Non-finite values (spec G1) are kept in `points`; the bounding box
//! excludes them — `Aabb::from_points` is the single implementation.

use std::path::Path;

use glam::Vec3;

use super::ascii_text::{self, LineIter};
use super::{Aabb, PathData, PathError};

/// Load a path file (`.xyz` or `.csv`) from a file path (spec F2).
pub fn load(path: &Path) -> Result<PathData, PathError> {
    let bytes = std::fs::read(path)?;
    parse_xyz(&bytes)
}

fn malformed(line: usize, reason: impl Into<String>) -> PathError {
    PathError::Malformed {
        line,
        reason: reason.into(),
    }
}

fn limit(reason: impl Into<String>) -> PathError {
    PathError::Limit {
        reason: reason.into(),
    }
}

/// A line whose first token starts with `#`: a whole-line comment (spec
/// F2; trailing comments are not supported).
fn is_comment(tokens: &[&[u8]]) -> bool {
    tokens.first().is_some_and(|token| token.starts_with(b"#"))
}

/// Whether a line already carries a full point: exactly three tokens that
/// each parse as a float. Used by the title-row test and by the counting
/// pass.
fn is_point_line(tokens: &[&[u8]]) -> bool {
    tokens.len() == 3
        && tokens
            .iter()
            .all(|token| ascii_text::parse_number::<f32>(token).is_ok())
}

/// First pass: count the point lines — after the optional title row and
/// skipping blank and comment lines — for the allocation guard. Line
/// errors are left to the second pass, which is authoritative; the count
/// equals the number of points of every successful parse.
fn count_points(bytes: &[u8]) -> u128 {
    let mut count = 0u128;
    let mut title_seen = false;
    for line in LineIter::new(bytes) {
        let tokens = ascii_text::xyz_tokens(line);
        if tokens.is_empty() || is_comment(&tokens) {
            continue;
        }
        if !title_seen {
            // The first surviving line may be the title row (spec F2);
            // whether it is, the title opportunity is now used up.
            title_seen = true;
        }
        if is_point_line(&tokens) {
            count += 1;
        }
    }
    count
}

/// The allocation guard of the counting pre-validation (spec §6): checked
/// in u128 before any `Vec` exists.
fn checked_capacity(counted: u128) -> Result<usize, PathError> {
    usize::try_from(counted).map_err(|_| {
        limit(format!(
            "{counted} points cannot fit in this platform's address space"
        ))
    })
}

/// Second pass: parse and validate every line, appending into the
/// pre-sized vector.
fn parse_xyz(bytes: &[u8]) -> Result<PathData, PathError> {
    let counted = count_points(bytes);
    let capacity = checked_capacity(counted)?;
    let mut points = Vec::with_capacity(capacity);

    let mut title_seen = false;
    for (offset, line) in LineIter::new(bytes).enumerate() {
        let line_number = offset + 1; // 1-based physical line numbers.
        let tokens = ascii_text::xyz_tokens(line);
        if tokens.is_empty() || is_comment(&tokens) {
            continue;
        }
        if !title_seen {
            // The first surviving line: non-numeric rows are the title
            // (spec F2) and are skipped.
            title_seen = true;
            if !is_point_line(&tokens) {
                continue;
            }
            points.push(parse_point(&tokens, line_number)?);
            continue;
        }
        // Every later line must be exactly one point (spec F2: exactly
        // 3 tokens, otherwise a line-numbered error).
        if tokens.len() != 3 {
            return Err(malformed(
                line_number,
                format!("expected exactly 3 values, found {}", tokens.len()),
            ));
        }
        points.push(parse_point(&tokens, line_number)?);
    }

    if points.len() < 2 {
        return Err(PathError::TooFewPoints {
            points: points.len(),
        });
    }
    let bounds = Aabb::from_points(&points);
    Ok(PathData { points, bounds })
}

/// Parse one validated point line into a vertex.
fn parse_point(tokens: &[&[u8]], line: usize) -> Result<Vec3, PathError> {
    let mut values = [0.0f32; 3];
    for (slot, token) in values.iter_mut().zip(tokens) {
        *slot = parse_value(token, line)?;
    }
    Ok(Vec3::new(values[0], values[1], values[2]))
}

/// Parse one numeric value of a point line. The textual non-finite
/// spellings (`nan`/`inf`) parse, so spec G1 values survive.
fn parse_value(token: &[u8], line: usize) -> Result<f32, PathError> {
    ascii_text::parse_number::<f32>(token).map_err(|error| match error {
        ascii_text::NumberTokenError::NotUtf8 => malformed(line, "value is not valid UTF-8"),
        ascii_text::NumberTokenError::Invalid => malformed(
            line,
            format!("invalid number \"{}\"", String::from_utf8_lossy(token)),
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(bytes: &[u8]) -> PathData {
        parse_xyz(bytes).expect("fixture must load")
    }

    fn parse_err(bytes: &[u8]) -> PathError {
        parse_xyz(bytes).expect_err("fixture must be rejected")
    }

    /// The (1-based line, reason) pair of a line-anchored rejection.
    fn malformed_of(bytes: &[u8]) -> (usize, String) {
        match parse_err(bytes) {
            PathError::Malformed { line, reason } => (line, reason),
            other => panic!("expected a line-anchored error, got {other:?}"),
        }
    }

    #[test]
    fn space_separated_points_in_file_order() {
        let file = b"1.5 -2.25 1e2\n10 0.2 -3\n4 5 6";
        let data = parse_ok(file);
        assert_eq!(
            data.points,
            [
                Vec3::new(1.5, -2.25, 100.0),
                Vec3::new(10.0, 0.2, -3.0),
                Vec3::new(4.0, 5.0, 6.0)
            ]
        );
        let bounds = data.bounds.expect("finite points must yield bounds");
        assert_eq!(bounds.min, Vec3::new(1.5, -2.25, -3.0));
        assert_eq!(bounds.max, Vec3::new(10.0, 5.0, 100.0));
        assert_eq!(data.point_count(), 3);
    }

    #[test]
    fn delimiter_class_is_space_tab_and_comma() {
        let file = b"1 2 3\n4,5,6\n7\t8\t9\n10,\t11 ,12\n";
        let data = parse_ok(file);
        assert_eq!(
            data.points,
            [
                Vec3::new(1.0, 2.0, 3.0),
                Vec3::new(4.0, 5.0, 6.0),
                Vec3::new(7.0, 8.0, 9.0),
                Vec3::new(10.0, 11.0, 12.0)
            ]
        );
    }

    #[test]
    fn title_row_skipped() {
        // A non-numeric first line is the title (spec F2).
        let data = parse_ok(b"x y z\n1 2 3\n4 5 6\n");
        assert_eq!(data.point_count(), 2);

        // Single-token comma-joined titles are skipped too.
        let data = parse_ok(b"X,Y,Z\n0,0,0\n1,1,1\n");
        assert_eq!(data.point_count(), 2);

        // Comments and blank lines before the title do not consume the
        // title opportunity.
        let data = parse_ok(b"# scan header\n\nx, y, z\n1 2 3\n4 5 6\n");
        assert_eq!(data.point_count(), 2);

        // A first line with trailing junk is also "non-numeric" by the
        // title rule (spec F2: the first non-numeric line is skipped), so
        // it is consumed instead of erroring.
        match parse_err(b"1 2 3 # note\n4 5 6\n") {
            PathError::TooFewPoints { points } => assert_eq!(points, 1),
            other => panic!("expected TooFewPoints, got {other:?}"),
        }
    }

    #[test]
    fn title_row_only_consumes_one_line() {
        // A second non-numeric line is an error, with its line number.
        let (line, reason) = malformed_of(b"x y z\n1 2 3\n4 5 6\nstill a title\n");
        assert_eq!(line, 4);
        assert!(reason.contains("still"), "reason: {reason}");
    }

    #[test]
    fn comments_and_blank_lines_skipped_anywhere() {
        // Whole-line comments may be indented; blank lines are skipped.
        let data = parse_ok(b"# first\n1 2 3\n   # indented comment\n\n4 5 6\n");
        assert_eq!(data.point_count(), 2);

        // Trailing comments are not supported: `# note` behind a point on
        // a non-first line makes five tokens — a line-numbered error.
        let (line, reason) = malformed_of(b"1 2 3\n4 5 6 # note\n");
        assert_eq!(line, 2);
        assert!(
            reason.contains("expected exactly 3 values, found 5"),
            "reason: {reason}"
        );
    }

    #[test]
    fn wrong_token_count_reports_line() {
        let (line, reason) = malformed_of(b"1 2 3\n4 5\n");
        assert_eq!(line, 2);
        assert!(
            reason.contains("expected exactly 3 values, found 2"),
            "reason: {reason}"
        );

        let (line, _) = malformed_of(b"1 2 3\n4 5 6 7\n");
        assert_eq!(line, 2);
    }

    #[test]
    fn non_numeric_value_reports_line() {
        let (line, reason) = malformed_of(b"1 2 3\n4 two 6\n");
        assert_eq!(line, 2);
        assert!(reason.contains("two"), "reason: {reason}");
    }

    #[test]
    fn too_few_points_rejected() {
        // One point cannot form a polyline.
        match parse_err(b"1 2 3\n") {
            PathError::TooFewPoints { points } => assert_eq!(points, 1),
            other => panic!("expected TooFewPoints, got {other:?}"),
        }

        // Empty file, comment-only file and title-only file have zero
        // points.
        for file in [
            b"".as_slice(),
            b"# nothing\n\n".as_slice(),
            b"x y z\n".as_slice(),
        ] {
            match parse_err(file) {
                PathError::TooFewPoints { points } => assert_eq!(points, 0),
                other => panic!("expected TooFewPoints, got {other:?}"),
            }
        }
    }

    #[test]
    fn non_finite_points_kept_bounds_exclude_them() {
        let data = parse_ok(b"nan inf 0\n1 2 3\n");
        assert!(data.points[0].x.is_nan());
        assert_eq!(data.points[0].y, f32::INFINITY);
        let bounds = data.bounds.expect("one finite point must yield bounds");
        assert_eq!(bounds.min, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(bounds.max, Vec3::new(1.0, 2.0, 3.0));

        let data = parse_ok(b"-inf 0 0\n0 nan 1\n");
        assert!(data.bounds.is_none());
    }

    #[test]
    fn crlf_and_missing_final_newline() {
        let data = parse_ok(b"1 2 3\r\n4\t5,6\r\n7 8 9");
        assert_eq!(data.point_count(), 3);
        assert_eq!(data.points[1], Vec3::new(4.0, 5.0, 6.0));
        assert_eq!(data.points[2], Vec3::new(7.0, 8.0, 9.0));
    }

    #[test]
    fn counting_prevalidation_rejects_unrepresentable_counts() {
        // The allocation guard runs before any `Vec` exists. The input
        // cannot come from an in-memory fixture (the point count is
        // bounded by the file size), so the guard is exercised directly.
        assert!(matches!(
            checked_capacity(u128::MAX),
            Err(PathError::Limit { .. })
        ));
        assert_eq!(checked_capacity(2).expect("two points must pass"), 2);

        // End to end, the counted capacity matches the parsed points.
        let data = parse_ok(b"1 2 3\n4 5 6\n");
        assert_eq!(data.points.capacity(), 2);
    }
}
