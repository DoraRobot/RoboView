//! PCD parser for the supported subsets (spec F2, plan §5).
//!
//! Scope: version 0.7 headers with `x y z` float32 columns and an optional
//! `rgb` column — either float-packed (PCL convention: the 32 bits are
//! `0x00RRGGBB`, extracted after a little-endian read as
//! `r=(v>>16)&0xff, g=(v>>8)&0xff, b=v&0xff`) or raw unsigned (four bytes
//! per point, file order `R G B ?` in binary bodies; in ASCII bodies the
//! token is the PCL-packed integer value — a documented asymmetry).
//! Additional columns (e.g. `intensity`) are accepted and skipped when they
//! are well-formed 4-byte scalars. Bodies are `ascii` or binary (`binary`
//! with `binary_little_endian` accepted as its explicit alias).
//!
//! Rejected with `PointCloudError::Malformed`: other versions, unknown
//! header keys or DATA modes, duplicate header rows, FIELDS/TYPE/SIZE/
//! COUNT rows of unequal length, columns outside the locked enumeration
//! (`COUNT > 1`, non-4-byte sizes incl. 8-byte floats), coordinate columns
//! that are not float32, missing required rows, unparseable tokens,
//! truncated bodies, and point counts that exceed the body size
//! (allocation guard).
//!
//! Non-finite coordinates (spec G1) are kept in `positions`; the bounding
//! box excludes them — `Aabb::from_points` is the single implementation.

use std::path::Path;

use glam::Vec3;

use super::ascii_text::{self, LineIter};
use super::{Aabb, Color, Format, PointCloudData, PointCloudError};

/// The 32-bit type codes of PCD v0.7 (I/U/F only; other sizes and the
/// char/short forms are outside the locked subset, plan §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColumnType {
    I,
    U,
    F,
}

impl ColumnType {
    fn from_keyword(keyword: &[u8]) -> Option<Self> {
        match keyword {
            b"I" => Some(Self::I),
            b"U" => Some(Self::U),
            b"F" => Some(Self::F),
            _ => None,
        }
    }

    fn keyword(self) -> &'static str {
        match self {
            Self::I => "I",
            Self::U => "U",
            Self::F => "F",
        }
    }
}

/// What a FIELDS column contributes to the output data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FieldRole {
    X,
    Y,
    Z,
    /// `rgb` as float32: the bits are `0x00RRGGBB` (PCL packed colors).
    RgbPackedF,
    /// `rgb` as uint32: bytes per point (see module docs for the two
    /// encodings of the raw and the packed value).
    RgbRawU,
    /// Parsed and validated, but not part of the output.
    Skipped,
}

/// One data column, in FIELDS order (the on-disk record order).
#[derive(Debug)]
struct Column {
    ty: ColumnType,
    role: FieldRole,
}

/// Body storage mode declared by the `DATA` header line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DataMode {
    Ascii,
    Binary,
}

#[derive(Debug)]
struct Header {
    columns: Vec<Column>,
    point_count: u64,
    mode: DataMode,
}

fn malformed(reason: impl Into<String>) -> PointCloudError {
    PointCloudError::Malformed {
        reason: reason.into(),
    }
}

fn store_point(
    positions: &mut Vec<Vec3>,
    colors: &mut Option<Vec<Color>>,
    accumulator: PointAccumulator,
) {
    positions.push(Vec3::new(accumulator.x, accumulator.y, accumulator.z));
    if let Some(colors) = colors {
        colors.push(Color {
            r: accumulator.r,
            g: accumulator.g,
            b: accumulator.b,
        });
    }
}

/// Load a PCD point cloud from a file path.
pub fn load(path: &Path) -> Result<PointCloudData, PointCloudError> {
    let bytes = std::fs::read(path)?;
    parse_pcd(&bytes)
}

fn parse_pcd(bytes: &[u8]) -> Result<PointCloudData, PointCloudError> {
    let (header, data_start) = parse_header(bytes)?;
    let data = &bytes[data_start..];
    let (positions, colors) = match header.mode {
        DataMode::Ascii => read_ascii_points(data, &header.columns, header.point_count)?,
        DataMode::Binary => read_binary_points(data, &header.columns, header.point_count)?,
    };
    let bounds = Aabb::from_points(&positions);
    Ok(PointCloudData {
        positions,
        colors,
        bounds,
        format: Format::Pcd,
    })
}

/// Store one FIELDS/TYPE/SIZE/COUNT row, rejecting duplicates and empty
/// rows.
fn set_row(
    slot: &mut Option<Vec<Vec<u8>>>,
    key: &[u8],
    values: &[&[u8]],
) -> Result<(), PointCloudError> {
    if slot.is_some() {
        return Err(malformed(format!(
            "PCD header: duplicate \"{}\" line",
            String::from_utf8_lossy(key)
        )));
    }
    if values.is_empty() {
        return Err(malformed(format!(
            "PCD header: \"{}\" needs at least one value",
            String::from_utf8_lossy(key)
        )));
    }
    *slot = Some(values.iter().map(|value| value.to_vec()).collect());
    Ok(())
}

/// Parse the header up to (and including) the `DATA` line, which is where
/// the body starts — the byte offset of the first body byte is returned.
/// Header rows may come in any order; comments (`#`) and the informational
/// WIDTH/HEIGHT/VIEWPOINT rows are ignored. CRLF endings are tolerated.
fn parse_header(bytes: &[u8]) -> Result<(Header, usize), PointCloudError> {
    if bytes.is_empty() {
        return Err(malformed("PCD: the file is empty"));
    }
    let mut version_row: Option<Vec<Vec<u8>>> = None;
    let mut fields_row: Option<Vec<Vec<u8>>> = None;
    let mut types_row: Option<Vec<Vec<u8>>> = None;
    let mut sizes_row: Option<Vec<Vec<u8>>> = None;
    let mut counts_row: Option<Vec<Vec<u8>>> = None;
    let mut points_row: Option<Vec<Vec<u8>>> = None;
    let mut pos = 0usize;

    while pos < bytes.len() {
        let (line_end, has_newline) = match bytes[pos..].iter().position(|&b| b == b'\n') {
            Some(offset) => (pos + offset, true),
            None => (bytes.len(), false),
        };
        let line = &bytes[pos..line_end];
        pos = if has_newline {
            line_end + 1
        } else {
            bytes.len()
        };
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() || line[0] == b'#' {
            continue;
        }
        let tokens = ascii_text::ws_tokens(line);
        if tokens.is_empty() {
            continue;
        }
        match tokens[0] {
            b"VERSION" => set_row(&mut version_row, b"VERSION", &tokens[1..])?,
            b"FIELDS" => set_row(&mut fields_row, b"FIELDS", &tokens[1..])?,
            b"TYPE" => set_row(&mut types_row, b"TYPE", &tokens[1..])?,
            b"SIZE" => set_row(&mut sizes_row, b"SIZE", &tokens[1..])?,
            b"COUNT" => set_row(&mut counts_row, b"COUNT", &tokens[1..])?,
            b"POINTS" => set_row(&mut points_row, b"POINTS", &tokens[1..])?,
            b"DATA" => {
                if tokens.len() != 2 {
                    return Err(malformed(
                        "PCD header: the \"DATA\" line must be exactly \"DATA <mode>\"",
                    ));
                }
                let mode = match tokens[1] {
                    b"ascii" => DataMode::Ascii,
                    // `binary_little_endian` is accepted as the explicit
                    // alias of `binary` (task wording; plan §5).
                    b"binary" | b"binary_little_endian" => DataMode::Binary,
                    other => {
                        return Err(malformed(format!(
                            "PCD header: unsupported DATA mode \"{}\"; only \"ascii\" and \"binary\" (\"binary_little_endian\") are supported",
                            String::from_utf8_lossy(other)
                        )));
                    }
                };
                let header = build_header(
                    mode,
                    version_row,
                    fields_row,
                    types_row,
                    sizes_row,
                    counts_row,
                    points_row,
                )?;
                return Ok((header, pos));
            }
            // Informational rows of the v0.7 header; tolerated and ignored.
            b"WIDTH" | b"HEIGHT" | b"VIEWPOINT" => {}
            other => {
                return Err(malformed(format!(
                    "PCD header: unrecognized header line \"{}\"",
                    String::from_utf8_lossy(other)
                )));
            }
        }
    }
    Err(malformed("PCD header: missing \"DATA\" line"))
}

/// Validate the required rows and derive the column layout (plan §5
/// enumeration).
fn build_header(
    mode: DataMode,
    version_row: Option<Vec<Vec<u8>>>,
    fields_row: Option<Vec<Vec<u8>>>,
    types_row: Option<Vec<Vec<u8>>>,
    sizes_row: Option<Vec<Vec<u8>>>,
    counts_row: Option<Vec<Vec<u8>>>,
    points_row: Option<Vec<Vec<u8>>>,
) -> Result<Header, PointCloudError> {
    let version = version_row
        .as_deref()
        .ok_or_else(|| malformed("PCD header: missing \"VERSION\" line"))?;
    if version.len() != 1 || version[0].as_slice() != b"0.7" {
        return Err(malformed(format!(
            "PCD header: unsupported VERSION \"{}\"; only \"0.7\" is supported",
            String::from_utf8_lossy(&version[0])
        )));
    }
    let field_names = fields_row
        .as_deref()
        .ok_or_else(|| malformed("PCD header: missing \"FIELDS\" line"))?;
    let type_tokens = types_row
        .as_deref()
        .ok_or_else(|| malformed("PCD header: missing \"TYPE\" line"))?;
    let size_tokens = sizes_row
        .as_deref()
        .ok_or_else(|| malformed("PCD header: missing \"SIZE\" line"))?;
    let count_tokens = counts_row
        .as_deref()
        .ok_or_else(|| malformed("PCD header: missing \"COUNT\" line"))?;
    let column_count = field_names.len();
    if type_tokens.len() != column_count
        || size_tokens.len() != column_count
        || count_tokens.len() != column_count
    {
        return Err(malformed(format!(
            "PCD header: \"FIELDS\", \"TYPE\", \"SIZE\" and \"COUNT\" must declare the same number of values ({} fields, but {} TYPE, {} SIZE and {} COUNT values)",
            column_count,
            type_tokens.len(),
            size_tokens.len(),
            count_tokens.len()
        )));
    }
    let points_text = points_row
        .as_deref()
        .ok_or_else(|| malformed("PCD header: missing \"POINTS\" line"))?;
    if points_text.len() != 1 {
        return Err(malformed(
            "PCD header: the \"POINTS\" line must be exactly \"POINTS <count>\"",
        ));
    }
    let point_count: u64 =
        ascii_text::parse_number(&points_text[0]).map_err(|error| match error {
            ascii_text::NumberTokenError::NotUtf8 => {
                malformed("PCD header: invalid \"POINTS\" count")
            }
            ascii_text::NumberTokenError::Invalid => malformed(format!(
                "PCD header: invalid \"POINTS\" count \"{}\"",
                String::from_utf8_lossy(&points_text[0])
            )),
        })?;

    let mut columns = Vec::with_capacity(column_count);
    for index in 0..column_count {
        let name = String::from_utf8(field_names[index].clone())
            .map_err(|_| malformed("PCD header: column name is not valid UTF-8"))?;
        let ty = ColumnType::from_keyword(&type_tokens[index]).ok_or_else(|| {
            malformed(format!(
                "PCD header: column \"{name}\" has unknown TYPE \"{}\"; only I, U and F are supported",
                String::from_utf8_lossy(&type_tokens[index])
            ))
        })?;
        let size: u64 = ascii_text::parse_number(&size_tokens[index])
            .map_err(|_| malformed("PCD header: invalid SIZE value"))?;
        let count: u64 = ascii_text::parse_number(&count_tokens[index])
            .map_err(|_| malformed("PCD header: invalid COUNT value"))?;
        columns.push(make_column(&name, ty, size, count)?);
    }
    let has_axis = |role: FieldRole| columns.iter().any(|column| column.role == role);
    if !(has_axis(FieldRole::X) && has_axis(FieldRole::Y) && has_axis(FieldRole::Z)) {
        return Err(malformed(
            "PCD header: \"FIELDS\" must name \"x\", \"y\" and \"z\" float32 columns",
        ));
    }
    Ok(Header {
        columns,
        point_count,
        mode,
    })
}

/// Map one FIELDS/TYPE/SIZE/COUNT row to a column (plan §5 enumeration).
fn make_column(
    name: &str,
    ty: ColumnType,
    size: u64,
    count: u64,
) -> Result<Column, PointCloudError> {
    if size != 4 || count != 1 {
        return Err(malformed(format!(
            "PCD header: column \"{name}\" declares SIZE {size} COUNT {count}; only single-value 4-byte columns are supported"
        )));
    }
    let role = match name {
        "x" | "y" | "z" => {
            if ty != ColumnType::F {
                return Err(malformed(format!(
                    "PCD header: coordinate column \"{name}\" has TYPE {}; coordinates must be TYPE F SIZE 4",
                    ty.keyword()
                )));
            }
            match name {
                "x" => FieldRole::X,
                "y" => FieldRole::Y,
                _ => FieldRole::Z,
            }
        }
        "rgb" => match ty {
            ColumnType::F => FieldRole::RgbPackedF,
            ColumnType::U => FieldRole::RgbRawU,
            ColumnType::I => {
                return Err(malformed(
                    "PCD header: the \"rgb\" column must be TYPE F or TYPE U, not TYPE I",
                ));
            }
        },
        // Any other column (e.g. intensity) is accepted and skipped when it
        // is a well-formed 4-byte scalar of the locked enumeration.
        _ => FieldRole::Skipped,
    };
    Ok(Column { ty, role })
}

/// Extract sRGB bytes from the PCL packed color value (plan §5): the 32
/// bits are `0x00RRGGBB`.
fn unpack_packed(value: u32) -> (u8, u8, u8) {
    ((value >> 16) as u8, (value >> 8) as u8, value as u8)
}

/// Per-point values assembled while decoding one record.
#[derive(Debug, Clone, Copy)]
struct PointAccumulator {
    x: f32,
    y: f32,
    z: f32,
    r: u8,
    g: u8,
    b: u8,
}

impl PointAccumulator {
    fn new() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            r: 0,
            g: 0,
            b: 0,
        }
    }
}

/// Decode one 4-byte column of a binary body as its declared type. Every
/// supported column is exactly 4 bytes, so a `[u8; 4]` input can never
/// fail; the `Option` keeps the API total.
fn decode_binary_scalar(bytes: [u8; 4], ty: ColumnType) -> Option<f64> {
    Some(match ty {
        ColumnType::I => i32::from_le_bytes(bytes) as f64,
        ColumnType::U => u32::from_le_bytes(bytes) as f64,
        ColumnType::F => f32::from_le_bytes(bytes) as f64,
    })
}

fn read_binary_points(
    data: &[u8],
    columns: &[Column],
    point_count: u64,
) -> Result<(Vec<Vec3>, Option<Vec<Color>>), PointCloudError> {
    // Allocation guard (plan §5): each point takes exactly one record of
    // `stride` bytes, so the declared count must fit the remaining body —
    // otherwise the file is truncated or the count is bogus.
    let stride = columns.len() * 4;
    let required = u128::from(point_count) * stride as u128;
    if required > data.len() as u128 {
        return Err(malformed(format!(
            "PCD binary data: header declares {point_count} points at a {stride}-byte stride ({required} bytes required), but only {} bytes of body data remain",
            data.len()
        )));
    }
    let count = usize::try_from(point_count)
        .map_err(|_| malformed("PCD binary data: point count exceeds platform limits"))?;
    let point_bytes = count * stride;
    let region = data
        .get(..point_bytes)
        .ok_or_else(|| malformed("PCD binary data: point region out of bounds"))?;
    let has_color = columns
        .iter()
        .any(|column| matches!(column.role, FieldRole::RgbPackedF | FieldRole::RgbRawU));
    let mut positions = Vec::with_capacity(count);
    let mut colors = has_color.then(|| Vec::with_capacity(count));
    for record in region.chunks_exact(stride) {
        let mut accumulator = PointAccumulator::new();
        for (index, column) in columns.iter().enumerate() {
            let offset = index * 4;
            let bytes: [u8; 4] = record
                .get(offset..offset + 4)
                .and_then(|chunk| chunk.try_into().ok())
                .ok_or_else(|| malformed("PCD binary data: point record is truncated"))?;
            match column.role {
                FieldRole::X => accumulator.x = f32::from_le_bytes(bytes),
                FieldRole::Y => accumulator.y = f32::from_le_bytes(bytes),
                FieldRole::Z => accumulator.z = f32::from_le_bytes(bytes),
                FieldRole::RgbPackedF => {
                    // PCL packed colors: the record bytes are the
                    // `0x00RRGGBB` value in little-endian order (plan §5).
                    let (r, g, b) = unpack_packed(u32::from_le_bytes(bytes));
                    accumulator.r = r;
                    accumulator.g = g;
                    accumulator.b = b;
                }
                FieldRole::RgbRawU => {
                    // Raw unsigned colors: four bytes in file order, R G B ?
                    // (the alpha byte, if present, is ignored).
                    accumulator.r = bytes[0];
                    accumulator.g = bytes[1];
                    accumulator.b = bytes[2];
                }
                // Skipped columns are still bounds-validated so malformed
                // bodies are rejected instead of silently misread.
                FieldRole::Skipped => {
                    let _ = decode_binary_scalar(bytes, column.ty);
                }
            }
        }
        store_point(&mut positions, &mut colors, accumulator);
    }
    Ok((positions, colors))
}

/// Parse one ASCII token as the declared type, with exact integer range
/// checking. Non-finite tokens (`nan`/`inf`) parse for float columns only,
/// which is how G1 data survives an ASCII round trip. The token-level
/// parse comes from the shared [`ascii_text`] helpers; the messages stay
/// PCD-specific.
fn decode_ascii_scalar(token: &[u8], ty: ColumnType) -> Result<f64, PointCloudError> {
    let kind = ty.keyword();
    let to_error = |error: ascii_text::NumberTokenError| match error {
        ascii_text::NumberTokenError::NotUtf8 => {
            malformed("PCD ASCII data: numeric value is not valid UTF-8")
        }
        ascii_text::NumberTokenError::Invalid => {
            malformed(format!("PCD ASCII data: expected a TYPE {kind} number"))
        }
    };
    let value = match ty {
        ColumnType::I => ascii_text::parse_number::<i32>(token).map_err(to_error)? as f64,
        ColumnType::U => ascii_text::parse_number::<u32>(token).map_err(to_error)? as f64,
        ColumnType::F => ascii_text::parse_number::<f32>(token).map_err(to_error)? as f64,
    };
    Ok(value)
}

/// Parse an ASCII token of a packed (TYPE F) `rgb` column. Writers
/// disagree on the encoding: PCL stores the `0x00RRGGBB` *bit pattern*
/// (the float is then a tiny subnormal), while many tools write the color
/// as the *integer value* `r<<16 | g<<8 | b` (e.g. `16711680.0` for red).
/// Integral floats in `[0, 2^24)` take the value route; everything else is
/// treated as a bit pattern.
fn decode_packed_float(token: &[u8]) -> Result<u32, PointCloudError> {
    let value: f32 = ascii_text::parse_number(token).map_err(|error| match error {
        ascii_text::NumberTokenError::NotUtf8 => {
            malformed("PCD ASCII data: numeric value is not valid UTF-8")
        }
        ascii_text::NumberTokenError::Invalid => {
            malformed("PCD ASCII data: expected a TYPE F number")
        }
    })?;
    let packed_max = (1u32 << 24) as f32;
    if value.is_finite() && value >= 0.0 && value < packed_max && value.fract() == 0.0 {
        Ok(value as u32)
    } else {
        Ok(value.to_bits())
    }
}

fn read_ascii_points(
    data: &[u8],
    columns: &[Column],
    point_count: u64,
) -> Result<(Vec<Vec3>, Option<Vec<Color>>), PointCloudError> {
    // Each record occupies at least one body byte, so the declared count is
    // bounded by the body length (allocation guard, plan §5).
    if u128::from(point_count) > data.len() as u128 {
        return Err(malformed(format!(
            "PCD ASCII data: header declares {point_count} points, but the body is too short"
        )));
    }
    let count = usize::try_from(point_count)
        .map_err(|_| malformed("PCD ASCII data: point count exceeds platform limits"))?;
    let has_color = columns
        .iter()
        .any(|column| matches!(column.role, FieldRole::RgbPackedF | FieldRole::RgbRawU));
    let mut positions = Vec::with_capacity(count);
    let mut colors = has_color.then(|| Vec::with_capacity(count));
    let mut lines = LineIter::new(data);
    for point_index in 0..count {
        let line = lines.next().ok_or_else(|| {
            malformed(format!(
                "PCD ASCII data: file ends after {point_index} of {count} declared points"
            ))
        })?;
        let tokens = ascii_text::ws_tokens(line);
        if tokens.len() != columns.len() {
            return Err(malformed(format!(
                "PCD ASCII data: point record {point_index} has {} values; expected {} (one per FIELDS column)",
                tokens.len(),
                columns.len()
            )));
        }
        // Token consumption equals `columns.len() == tokens.len()`, so every
        // index below is in bounds.
        let mut accumulator = PointAccumulator::new();
        for (index, column) in columns.iter().enumerate() {
            let token = tokens[index];
            match column.role {
                FieldRole::X => {
                    accumulator.x = decode_ascii_scalar(token, ColumnType::F)? as f32;
                }
                FieldRole::Y => {
                    accumulator.y = decode_ascii_scalar(token, ColumnType::F)? as f32;
                }
                FieldRole::Z => {
                    accumulator.z = decode_ascii_scalar(token, ColumnType::F)? as f32;
                }
                FieldRole::RgbPackedF => {
                    let (r, g, b) = unpack_packed(decode_packed_float(token)?);
                    accumulator.r = r;
                    accumulator.g = g;
                    accumulator.b = b;
                }
                FieldRole::RgbRawU => {
                    // The raw unsigned value of a U column travels through
                    // binary as four file-order bytes but through ASCII as
                    // one packed integer (module docs); both use the same
                    // `0x00RRGGBB` layout here.
                    let value: u32 =
                        ascii_text::parse_number(token).map_err(|error| match error {
                            ascii_text::NumberTokenError::NotUtf8 => {
                                malformed("PCD ASCII data: numeric value is not valid UTF-8")
                            }
                            ascii_text::NumberTokenError::Invalid => {
                                malformed("PCD ASCII data: expected a TYPE U number")
                            }
                        })?;
                    let (r, g, b) = unpack_packed(value);
                    accumulator.r = r;
                    accumulator.g = g;
                    accumulator.b = b;
                }
                // Skipped columns are validated and discarded.
                FieldRole::Skipped => {
                    let _ = decode_ascii_scalar(token, column.ty)?;
                }
            }
        }
        store_point(&mut positions, &mut colors, accumulator);
    }
    Ok((positions, colors))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Little-endian byte encoding helpers for binary fixtures.
    fn le32(value: u32) -> [u8; 4] {
        value.to_le_bytes()
    }

    fn f32_bytes(value: f32) -> [u8; 4] {
        value.to_le_bytes()
    }

    /// Concatenate heterogeneous byte parts into one binary fixture body.
    fn body(parts: &[&[u8]]) -> Vec<u8> {
        let mut out = Vec::new();
        for part in parts {
            out.extend_from_slice(part);
        }
        out
    }

    /// Complete PCD file: version 0.7 header over the given rows and body.
    fn fixture(
        fields: &str,
        types: &str,
        sizes: &str,
        counts: &str,
        points: u64,
        mode: &str,
        body: &[u8],
    ) -> Vec<u8> {
        let text = format!(
            "VERSION 0.7\nFIELDS {fields}\nTYPE {types}\nSIZE {sizes}\nCOUNT {counts}\nWIDTH {points}\nHEIGHT 1\nPOINTS {points}\nDATA {mode}\n"
        );
        let mut bytes = text.into_bytes();
        bytes.extend_from_slice(body);
        bytes
    }

    /// Plain x/y/z float32 fixture over `points` records.
    fn xyz_fixture(points: u64, mode: &str, body: &[u8]) -> Vec<u8> {
        fixture("x y z", "F F F", "4 4 4", "1 1 1", points, mode, body)
    }

    /// Hand-assembled file: `header` (final newline included) plus body.
    fn raw_file(header: &str, body: &[u8]) -> Vec<u8> {
        let mut out = header.as_bytes().to_vec();
        out.extend_from_slice(body);
        out
    }

    fn parse_ok(bytes: &[u8]) -> PointCloudData {
        parse_pcd(bytes).expect("fixture must load")
    }

    fn parse_err(bytes: &[u8]) -> PointCloudError {
        parse_pcd(bytes).expect_err("fixture must be rejected")
    }

    fn positions_of(data: &PointCloudData) -> &[Vec3] {
        &data.positions
    }

    fn colors_of(data: &PointCloudData) -> &[Color] {
        data.colors.as_ref().expect("fixture must carry colors")
    }

    #[test]
    fn ascii_xyz_positions_and_bounds() {
        let file = xyz_fixture(2, "ascii", b"1.5 -2.25 1e2\n10 0.2 -3\n");
        let data = parse_ok(&file);
        assert_eq!(
            positions_of(&data),
            &[Vec3::new(1.5, -2.25, 100.0), Vec3::new(10.0, 0.2, -3.0)]
        );
        assert!(data.colors.is_none());
        let bounds = data.bounds.expect("finite points must yield bounds");
        assert_eq!(bounds.min, Vec3::new(1.5, -2.25, -3.0));
        assert_eq!(bounds.max, Vec3::new(10.0, 0.2, 100.0));
        assert_eq!(data.format, Format::Pcd);
    }

    #[test]
    fn ascii_packed_rgb_float_column() {
        // 16711680 == 0xFF0000 (red), 65280 == 0x00FF00 (green).
        let file = fixture(
            "x y z rgb",
            "F F F F",
            "4 4 4 4",
            "1 1 1 1",
            2,
            "ascii",
            b"1 2 3 16711680\n4 5 6 65280\n",
        );
        let data = parse_ok(&file);
        assert_eq!(
            positions_of(&data),
            &[Vec3::new(1.0, 2.0, 3.0), Vec3::new(4.0, 5.0, 6.0)]
        );
        assert_eq!(
            colors_of(&data),
            &[Color { r: 255, g: 0, b: 0 }, Color { r: 0, g: 255, b: 0 }]
        );
    }

    #[test]
    fn binary_xyz_and_packed_rgb() {
        // Packed colors arrive as raw little-endian `0x00RRGGBB` bytes.
        // One extra trailing byte must be tolerated (records are sliced by
        // the declared count, not by the body length).
        let record = body(&[
            &f32_bytes(1.0),
            &f32_bytes(2.0),
            &f32_bytes(3.0),
            &[0x00, 0x00, 0xFF, 0x00],
            &f32_bytes(-4.0),
            &f32_bytes(-5.0),
            &f32_bytes(6.0),
            &[0x00, 0xFF, 0x00, 0x00],
            &[0xAA],
        ]);
        let file = fixture(
            "x y z rgb",
            "F F F F",
            "4 4 4 4",
            "1 1 1 1",
            2,
            "binary",
            &record,
        );
        let data = parse_ok(&file);
        assert_eq!(
            positions_of(&data),
            &[Vec3::new(1.0, 2.0, 3.0), Vec3::new(-4.0, -5.0, 6.0)]
        );
        assert_eq!(
            colors_of(&data),
            &[Color { r: 255, g: 0, b: 0 }, Color { r: 0, g: 255, b: 0 }]
        );
    }

    #[test]
    fn ascii_rgb_raw_u4_packed_token() {
        // 13132850 == 0xC86432: r=200, g=100, b=50.
        let file = fixture(
            "x y z rgb",
            "F F F U",
            "4 4 4 4",
            "1 1 1 1",
            1,
            "ascii",
            b"1 2 3 13132850\n",
        );
        let data = parse_ok(&file);
        assert_eq!(positions_of(&data), &[Vec3::new(1.0, 2.0, 3.0)]);
        assert_eq!(
            colors_of(&data),
            &[Color {
                r: 200,
                g: 100,
                b: 50
            }]
        );
    }

    #[test]
    fn binary_rgb_raw_u4_bytes_in_file_order() {
        // A U4 column stores raw bytes: R G B ? — the alpha byte is ignored.
        let record = body(&[
            &f32_bytes(1.0),
            &f32_bytes(2.0),
            &f32_bytes(3.0),
            &[200, 100, 50, 255],
        ]);
        let file = fixture(
            "x y z rgb",
            "F F F U",
            "4 4 4 4",
            "1 1 1 1",
            1,
            "binary",
            &record,
        );
        let data = parse_ok(&file);
        assert_eq!(positions_of(&data), &[Vec3::new(1.0, 2.0, 3.0)]);
        assert_eq!(
            colors_of(&data),
            &[Color {
                r: 200,
                g: 100,
                b: 50
            }]
        );
    }

    #[test]
    fn rgb_column_first_follows_fields_order() {
        // The record layout follows the FIELDS order, not a fixed one.
        let file = fixture(
            "rgb x y z",
            "F F F F",
            "4 4 4 4",
            "1 1 1 1",
            1,
            "ascii",
            b"16711680 1 2 3\n",
        );
        let data = parse_ok(&file);
        assert_eq!(positions_of(&data), &[Vec3::new(1.0, 2.0, 3.0)]);
        assert_eq!(colors_of(&data), &[Color { r: 255, g: 0, b: 0 }]);

        let record = body(&[
            &[0x00, 0x00, 0xFF, 0x00],
            &f32_bytes(4.0),
            &f32_bytes(5.0),
            &f32_bytes(6.0),
        ]);
        let file = fixture(
            "rgb x y z",
            "F F F F",
            "4 4 4 4",
            "1 1 1 1",
            1,
            "binary",
            &record,
        );
        let data = parse_ok(&file);
        assert_eq!(positions_of(&data), &[Vec3::new(4.0, 5.0, 6.0)]);
        assert_eq!(colors_of(&data), &[Color { r: 255, g: 0, b: 0 }]);
    }

    #[test]
    fn extra_columns_are_skipped() {
        // An `intensity` column (U) between z and rgb is validated and
        // skipped in both ASCII and binary bodies.
        let file = fixture(
            "x y z intensity rgb",
            "F F F U F",
            "4 4 4 4 4",
            "1 1 1 1 1",
            1,
            "ascii",
            b"1 2 3 9 16711680\n",
        );
        let data = parse_ok(&file);
        assert_eq!(positions_of(&data), &[Vec3::new(1.0, 2.0, 3.0)]);
        assert_eq!(colors_of(&data), &[Color { r: 255, g: 0, b: 0 }]);

        let record = body(&[
            &f32_bytes(1.0),
            &f32_bytes(2.0),
            &f32_bytes(3.0),
            &le32(9),
            &[0x00, 0x00, 0xFF, 0x00],
        ]);
        let file = fixture(
            "x y z intensity rgb",
            "F F F U F",
            "4 4 4 4 4",
            "1 1 1 1 1",
            1,
            "binary",
            &record,
        );
        let data = parse_ok(&file);
        assert_eq!(positions_of(&data), &[Vec3::new(1.0, 2.0, 3.0)]);
        assert_eq!(colors_of(&data), &[Color { r: 255, g: 0, b: 0 }]);
    }

    #[test]
    fn parallel_rows_must_match() {
        let file = fixture("x y z", "F F", "4 4 4", "1 1 1", 1, "ascii", b"1 2 3\n");
        assert!(matches!(
            parse_err(&file),
            PointCloudError::Malformed { .. }
        ));

        let file = fixture("x y z", "F F F", "4 4 4", "1 1", 1, "ascii", b"1 2 3\n");
        assert!(matches!(
            parse_err(&file),
            PointCloudError::Malformed { .. }
        ));
    }

    #[test]
    fn count_gt_one_rejected() {
        let file = fixture("x y z", "F F F", "4 4 4", "1 1 2", 1, "ascii", b"1 2 3\n");
        assert!(matches!(
            parse_err(&file),
            PointCloudError::Malformed { .. }
        ));
    }

    #[test]
    fn non_four_byte_sizes_rejected() {
        // 8-byte (f64) coordinates and 8-byte colors are outside the
        // locked enumeration (plan §5).
        let file = fixture("x y z", "F F F", "4 4 8", "1 1 1", 1, "ascii", b"1 2 3\n");
        assert!(matches!(
            parse_err(&file),
            PointCloudError::Malformed { .. }
        ));

        let file = fixture(
            "x y z rgb",
            "F F F F",
            "4 4 4 8",
            "1 1 1 1",
            1,
            "ascii",
            b"1 2 3 4\n",
        );
        assert!(matches!(
            parse_err(&file),
            PointCloudError::Malformed { .. }
        ));
    }

    #[test]
    fn unsupported_column_types_rejected() {
        // Coordinates must be float32; rgb must be F or U.
        let file = fixture("x y z", "I F F", "4 4 4", "1 1 1", 1, "ascii", b"1 2 3\n");
        assert!(matches!(
            parse_err(&file),
            PointCloudError::Malformed { .. }
        ));

        let file = fixture(
            "x y z rgb",
            "F F F I",
            "4 4 4 4",
            "1 1 1 1",
            1,
            "ascii",
            b"1 2 3 4\n",
        );
        assert!(matches!(
            parse_err(&file),
            PointCloudError::Malformed { .. }
        ));

        // Unknown TYPE letters are rejected too.
        let file = fixture("x y z", "F F D", "4 4 4", "1 1 1", 1, "ascii", b"1 2 3\n");
        assert!(matches!(
            parse_err(&file),
            PointCloudError::Malformed { .. }
        ));
    }

    #[test]
    fn unsupported_versions_rejected() {
        let file = raw_file(
            "VERSION 0.6\nFIELDS x y z\nTYPE F F F\nSIZE 4 4 4\nCOUNT 1 1 1\nPOINTS 1\nDATA ascii\n",
            b"1 2 3\n",
        );
        assert!(matches!(
            parse_err(&file),
            PointCloudError::Malformed { .. }
        ));

        let file = raw_file(
            "VERSION 0.7.0\nFIELDS x y z\nTYPE F F F\nSIZE 4 4 4\nCOUNT 1 1 1\nPOINTS 1\nDATA ascii\n",
            b"1 2 3\n",
        );
        assert!(matches!(
            parse_err(&file),
            PointCloudError::Malformed { .. }
        ));
    }

    #[test]
    fn data_modes_limited_to_ascii_and_binary() {
        for mode in [
            b"binary_big_endian".as_slice(),
            b"binary_compressed".as_slice(),
            b"compressed".as_slice(),
        ] {
            let mut file =
                b"VERSION 0.7\nFIELDS x y z\nTYPE F F F\nSIZE 4 4 4\nCOUNT 1 1 1\nPOINTS 1\nDATA "
                    .to_vec();
            file.extend_from_slice(mode);
            file.extend_from_slice(b"\n");
            assert!(
                matches!(parse_err(&file), PointCloudError::Malformed { .. }),
                "mode {:?} must be rejected",
                String::from_utf8_lossy(mode)
            );
        }

        // Extra tokens after the mode are rejected.
        let file = raw_file(
            "VERSION 0.7\nFIELDS x y z\nTYPE F F F\nSIZE 4 4 4\nCOUNT 1 1 1\nPOINTS 1\nDATA ascii extra\n",
            b"1 2 3\n",
        );
        assert!(matches!(
            parse_err(&file),
            PointCloudError::Malformed { .. }
        ));

        // `binary_little_endian` is accepted as the alias of `binary`.
        let record = body(&[&f32_bytes(1.0), &f32_bytes(2.0), &f32_bytes(3.0)]);
        let file = fixture(
            "x y z",
            "F F F",
            "4 4 4",
            "1 1 1",
            1,
            "binary_little_endian",
            &record,
        );
        let data = parse_ok(&file);
        assert_eq!(positions_of(&data), &[Vec3::new(1.0, 2.0, 3.0)]);
    }

    #[test]
    fn truncated_and_huge_counts_are_rejected() {
        // ASCII body with fewer records than declared.
        let file = xyz_fixture(3, "ascii", b"1 2 3\n4 5 6\n");
        assert!(matches!(
            parse_err(&file),
            PointCloudError::Malformed { .. }
        ));

        // Binary body with fewer records than declared.
        let record = body(&[&f32_bytes(1.0), &f32_bytes(2.0), &f32_bytes(3.0)]);
        let file = xyz_fixture(2, "binary", &record);
        assert!(matches!(
            parse_err(&file),
            PointCloudError::Malformed { .. }
        ));

        // A huge declared count must be rejected before any allocation.
        let file = xyz_fixture(u32::MAX as u64, "binary", &record);
        assert!(matches!(
            parse_err(&file),
            PointCloudError::Malformed { .. }
        ));

        let file = xyz_fixture(u32::MAX as u64, "ascii", b"1 2 3\n");
        assert!(matches!(
            parse_err(&file),
            PointCloudError::Malformed { .. }
        ));
    }

    #[test]
    fn missing_or_duplicate_header_rows_rejected() {
        // Each of the required rows may be missing (the DATA line too).
        let rows = [
            "VERSION 0.7",
            "FIELDS x y z",
            "TYPE F F F",
            "SIZE 4 4 4",
            "COUNT 1 1 1",
            "POINTS 1",
            "DATA ascii",
        ];
        for dropped in 0..rows.len() {
            let mut header = String::new();
            for (index, row) in rows.iter().enumerate() {
                if index != dropped {
                    header.push_str(row);
                    header.push('\n');
                }
            }
            assert!(
                matches!(
                    parse_err(header.as_bytes()),
                    PointCloudError::Malformed { .. }
                ),
                "dropping row {dropped} must be rejected"
            );
        }

        // Duplicates are rejected as well.
        let file = raw_file(
            "VERSION 0.7\nFIELDS x y z\nTYPE F F F\nTYPE U U U\nSIZE 4 4 4\nCOUNT 1 1 1\nPOINTS 1\nDATA ascii\n",
            b"1 2 3\n",
        );
        assert!(matches!(
            parse_err(&file),
            PointCloudError::Malformed { .. }
        ));
    }

    #[test]
    fn missing_axis_or_unknown_row_rejected() {
        let file = fixture("x y", "F F", "4 4", "1 1", 1, "ascii", b"1 2 3\n");
        assert!(matches!(
            parse_err(&file),
            PointCloudError::Malformed { .. }
        ));

        let file = raw_file(
            "VERSION 0.7\nFIELDS x y z\nTYPE F F F\nSIZE 4 4 4\nCOUNT 1 1 1\nPOINTS 1\nFOO bar\nDATA ascii\n",
            b"1 2 3\n",
        );
        assert!(matches!(
            parse_err(&file),
            PointCloudError::Malformed { .. }
        ));
    }

    #[test]
    fn ascii_nan_inf_kept_and_excluded_from_bounds() {
        let file = xyz_fixture(3, "ascii", b"nan inf 0\n-inf 1 2\n3 4 5\n");
        let data = parse_ok(&file);
        let positions = positions_of(&data);
        assert!(positions[0].x.is_nan());
        assert_eq!(positions[0].y, f32::INFINITY);
        assert_eq!(positions[1].x, f32::NEG_INFINITY);
        assert_eq!(positions[2], Vec3::new(3.0, 4.0, 5.0));
        // Bounds come from the single finite point.
        let bounds = data.bounds.expect("one finite point must yield bounds");
        assert_eq!(bounds.min, Vec3::new(3.0, 4.0, 5.0));
        assert_eq!(bounds.max, Vec3::new(3.0, 4.0, 5.0));
    }

    #[test]
    fn all_non_finite_gives_no_bounds() {
        let file = xyz_fixture(2, "ascii", b"nan nan nan\n-inf inf 1\n");
        let data = parse_ok(&file);
        assert_eq!(data.point_count(), 2);
        assert!(data.bounds.is_none());
    }

    #[test]
    fn crlf_multi_space_and_no_trailing_newline() {
        let file = raw_file(
            "VERSION 0.7\r\nFIELDS  x   y z\r\nTYPE F F F\r\nSIZE 4 4 4\r\nCOUNT 1 1 1\r\nPOINTS 2\r\nDATA ascii\r\n",
            b"1  2\t3\r\n4 5 6",
        );
        let data = parse_ok(&file);
        assert_eq!(data.point_count(), 2);
        assert_eq!(positions_of(&data)[1], Vec3::new(4.0, 5.0, 6.0));
    }

    #[test]
    fn comments_and_informational_rows_tolerated() {
        let file = raw_file(
            "# generated by a test writer\nVIEWPOINT 0 0 0 1 0 0 0\nVERSION 0.7\nWIDTH 2\nFIELDS x y z\n# mid-header comment\nTYPE F F F\nSIZE 4 4 4\nCOUNT 1 1 1\nHEIGHT 1\nPOINTS 2\nDATA ascii\n",
            b"1 2 3\n4 5 6\n",
        );
        let data = parse_ok(&file);
        assert_eq!(data.point_count(), 2);
        assert_eq!(
            positions_of(&data),
            &[Vec3::new(1.0, 2.0, 3.0), Vec3::new(4.0, 5.0, 6.0)]
        );
    }

    #[test]
    fn empty_cloud_in_both_modes() {
        let file = xyz_fixture(0, "ascii", b"");
        let data = parse_ok(&file);
        assert_eq!(data.point_count(), 0);
        assert!(data.bounds.is_none());

        let file = xyz_fixture(0, "binary", b"");
        let data = parse_ok(&file);
        assert_eq!(data.point_count(), 0);
        assert!(data.bounds.is_none());
    }

    #[test]
    fn ascii_token_errors() {
        // Garbage token.
        let file = xyz_fixture(1, "ascii", b"abc 2 3\n");
        assert!(matches!(
            parse_err(&file),
            PointCloudError::Malformed { .. }
        ));

        // Wrong token count on a record.
        let file = xyz_fixture(1, "ascii", b"1 2\n");
        assert!(matches!(
            parse_err(&file),
            PointCloudError::Malformed { .. }
        ));
        let file = xyz_fixture(1, "ascii", b"1 2 3 4\n");
        assert!(matches!(
            parse_err(&file),
            PointCloudError::Malformed { .. }
        ));

        // A U token out of the uint32 range.
        let file = fixture(
            "x y z rgb",
            "F F F U",
            "4 4 4 4",
            "1 1 1 1",
            1,
            "ascii",
            b"1 2 3 4294967296\n",
        );
        assert!(matches!(
            parse_err(&file),
            PointCloudError::Malformed { .. }
        ));

        // A skipped column still must be a well-formed number.
        let file = fixture(
            "x y z intensity",
            "F F F F",
            "4 4 4 4",
            "1 1 1 1",
            1,
            "ascii",
            b"1 2 3 nope\n",
        );
        assert!(matches!(
            parse_err(&file),
            PointCloudError::Malformed { .. }
        ));
    }
}
