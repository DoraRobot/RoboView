//! PLY parser for the supported subsets (spec F1, plan §5).
//!
//! Scope: ASCII and `binary_little_endian` bodies whose `vertex` element
//! declares scalar `x y z` coordinates (types float, double, uchar, ushort,
//! uint, int) and optionally colors as three `uchar` properties named
//! `r g b`, or as the `rgb` dialect — one `property uchar rgb` that still
//! occupies three bytes per vertex in file field order (spec F1).
//!
//! Rejected with `PointCloudError::Malformed`: big-endian declarations,
//! list properties on the vertex element (and on any element whose records
//! would precede the vertex records), unknown types and header keywords,
//! unparseable numbers, truncated bodies, and vertex counts that exceed the
//! remaining body bytes (allocation guard). Elements declared after the
//! vertex element (e.g. `face`) are ignored.
//!
//! Non-finite coordinates (spec G1) are kept in `positions`; the bounding
//! box excludes them — `Aabb::from_points` is the single implementation.

use std::path::Path;

use glam::Vec3;

use super::{Aabb, Color, Format, PointCloudData, PointCloudError};

/// The scalar types of the PLY type table, with their on-disk sizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScalarType {
    Char,
    UChar,
    Short,
    UShort,
    Int,
    UInt,
    Float,
    Double,
}

impl ScalarType {
    fn from_keyword(keyword: &[u8]) -> Option<Self> {
        Some(match keyword {
            b"char" => Self::Char,
            b"uchar" => Self::UChar,
            b"short" => Self::Short,
            b"ushort" => Self::UShort,
            b"int" => Self::Int,
            b"uint" => Self::UInt,
            b"float" => Self::Float,
            b"double" => Self::Double,
            _ => return None,
        })
    }

    /// Bytes occupied per vertex in a binary body.
    fn size(self) -> usize {
        match self {
            Self::Char | Self::UChar => 1,
            Self::Short | Self::UShort => 2,
            Self::Int | Self::UInt | Self::Float => 4,
            Self::Double => 8,
        }
    }

    /// PLY keyword, used in error messages.
    fn keyword(self) -> &'static str {
        match self {
            Self::Char => "char",
            Self::UChar => "uchar",
            Self::Short => "short",
            Self::UShort => "ushort",
            Self::Int => "int",
            Self::UInt => "uint",
            Self::Float => "float",
            Self::Double => "double",
        }
    }

    /// Whether this type may back an `x`/`y`/`z` property (plan §5 matrix).
    fn supports_coordinates(self) -> bool {
        matches!(
            self,
            Self::Float | Self::Double | Self::UChar | Self::UShort | Self::UInt | Self::Int
        )
    }
}

/// Body storage mode declared by the `format` header line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlyFormat {
    Ascii,
    BinaryLe,
}

#[derive(Debug, Clone)]
enum PropertyHeader {
    Scalar {
        name: String,
        ty: ScalarType,
    },
    /// `property list <count type> <item type> <name>` — unsupported.
    List {
        name: String,
    },
}

impl PropertyHeader {
    fn name(&self) -> &str {
        match self {
            Self::Scalar { name, .. } | Self::List { name } => name,
        }
    }

    fn is_list(&self) -> bool {
        matches!(self, Self::List { .. })
    }

    fn scalar_ty(&self) -> Option<ScalarType> {
        match self {
            Self::Scalar { ty, .. } => Some(*ty),
            Self::List { .. } => None,
        }
    }
}

#[derive(Debug)]
struct ElementHeader {
    name: String,
    count: u64,
    props: Vec<PropertyHeader>,
}

#[derive(Debug)]
struct Header {
    format: PlyFormat,
    elements: Vec<ElementHeader>,
    /// Byte offset of the first data byte, directly after `end_header`.
    data_start: usize,
}

/// What a vertex property contributes to the output data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FieldRole {
    X,
    Y,
    Z,
    ChannelR,
    ChannelG,
    ChannelB,
    /// `property uchar rgb`: three file bytes per vertex (spec F1 dialect).
    DialectRgb,
    /// Parsed and validated, but not part of the output.
    Skip,
}

#[derive(Debug, Clone, Copy)]
struct Field {
    role: FieldRole,
    ty: ScalarType,
    /// Bytes consumed per vertex in a binary body.
    size: usize,
    /// ASCII tokens consumed per vertex (3 for the `rgb` dialect, else 1).
    tokens: usize,
}

#[derive(Debug)]
struct VertexPlan {
    fields: Vec<Field>,
    count: u64,
    stride: usize,
    tokens_per_vertex: usize,
    has_color: bool,
    /// Bytes (binary) or record lines (ASCII) before the vertex records.
    skip_bytes: u128,
    skip_records: u128,
}

/// Per-vertex values assembled while decoding one record.
#[derive(Debug, Clone, Copy)]
struct VertexAccumulator {
    x: f32,
    y: f32,
    z: f32,
    r: u8,
    g: u8,
    b: u8,
}

impl VertexAccumulator {
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

fn malformed(reason: impl Into<String>) -> PointCloudError {
    PointCloudError::Malformed {
        reason: reason.into(),
    }
}

fn store_vertex(
    positions: &mut Vec<Vec3>,
    colors: &mut Option<Vec<Color>>,
    accumulator: VertexAccumulator,
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

/// Load a PLY point cloud from a file path.
pub fn load(path: &Path) -> Result<PointCloudData, PointCloudError> {
    let bytes = std::fs::read(path)?;
    parse_ply(&bytes)
}

fn parse_ply(bytes: &[u8]) -> Result<PointCloudData, PointCloudError> {
    let header = parse_header(bytes)?;
    let plan = plan_vertex(&header)?;
    let data = &bytes[header.data_start..];
    let (positions, colors) = match header.format {
        PlyFormat::Ascii => read_ascii_vertices(data, &plan)?,
        PlyFormat::BinaryLe => read_binary_vertices(data, &plan)?,
    };
    let bounds = Aabb::from_points(&positions);
    Ok(PointCloudData {
        positions,
        colors,
        bounds,
        format: Format::Ply,
    })
}

fn parse_header(bytes: &[u8]) -> Result<Header, PointCloudError> {
    if bytes.is_empty() {
        return Err(malformed("PLY: not a PLY file: the file is empty"));
    }
    let mut pos = 0usize;
    let mut first_line = true;
    let mut format: Option<PlyFormat> = None;
    let mut elements: Vec<ElementHeader> = Vec::new();
    let mut current_element: Option<usize> = None;

    while pos < bytes.len() {
        let (line_end, has_newline) = match bytes[pos..].iter().position(|&b| b == b'\n') {
            Some(offset) => (pos + offset, true),
            None => (bytes.len(), false),
        };
        let mut line = &bytes[pos..line_end];
        if line.last() == Some(&b'\r') {
            line = &line[..line.len() - 1];
        }
        let tokens: Vec<&[u8]> = line
            .split(|b| b.is_ascii_whitespace())
            .filter(|token| !token.is_empty())
            .collect();

        if first_line {
            first_line = false;
            if line != b"ply" {
                return Err(malformed(
                    "PLY: not a PLY file: the first line must be \"ply\"",
                ));
            }
        } else if let Some(keyword) = tokens.first().copied() {
            match keyword {
                b"comment" | b"obj_info" => {}
                b"format" => {
                    if tokens.len() != 3 {
                        return Err(malformed(
                            "PLY header: \"format\" must be followed by a storage mode and a version",
                        ));
                    }
                    if format.is_some() {
                        return Err(malformed("PLY header: duplicate \"format\" line"));
                    }
                    format = Some(match tokens[1] {
                        b"ascii" => PlyFormat::Ascii,
                        b"binary_little_endian" => PlyFormat::BinaryLe,
                        b"binary_big_endian" => {
                            return Err(malformed(
                                "PLY header: storage mode \"binary_big_endian\" is not supported",
                            ));
                        }
                        other => {
                            return Err(malformed(format!(
                                "PLY header: unknown storage mode \"{}\"",
                                String::from_utf8_lossy(other)
                            )));
                        }
                    });
                    let version = std::str::from_utf8(tokens[2])
                        .map_err(|_| malformed("PLY header: invalid format version"))?;
                    version
                        .parse::<f32>()
                        .map_err(|_| malformed("PLY header: invalid format version"))?;
                }
                b"element" => {
                    if tokens.len() != 3 {
                        return Err(malformed(
                            "PLY header: \"element\" must be followed by a name and a count",
                        ));
                    }
                    let name = String::from_utf8(tokens[1].to_vec())
                        .map_err(|_| malformed("PLY header: element name is not valid UTF-8"))?;
                    let count_text = std::str::from_utf8(tokens[2])
                        .map_err(|_| malformed("PLY header: invalid element count"))?;
                    let count: u64 = count_text.parse().map_err(|_| {
                        malformed(format!(
                            "PLY header: invalid element count \"{count_text}\""
                        ))
                    })?;
                    elements.push(ElementHeader {
                        name,
                        count,
                        props: Vec::new(),
                    });
                    current_element = Some(elements.len() - 1);
                }
                b"property" => {
                    if tokens.len() < 3 {
                        return Err(malformed(
                            "PLY header: \"property\" needs at least a type and a name",
                        ));
                    }
                    let element_index = current_element.ok_or_else(|| {
                        malformed("PLY header: \"property\" declared outside of an element")
                    })?;
                    let property = if tokens[1] == b"list" {
                        if tokens.len() != 5 {
                            return Err(malformed(
                                "PLY header: \"property list\" must be followed by a count type, an item type, and a name",
                            ));
                        }
                        for type_token in [tokens[2], tokens[3]] {
                            if ScalarType::from_keyword(type_token).is_none() {
                                return Err(malformed(format!(
                                    "PLY header: unknown list type \"{}\"",
                                    String::from_utf8_lossy(type_token)
                                )));
                            }
                        }
                        PropertyHeader::List {
                            name: String::from_utf8(tokens[4].to_vec()).map_err(|_| {
                                malformed("PLY header: property name is not valid UTF-8")
                            })?,
                        }
                    } else {
                        if tokens.len() != 3 {
                            return Err(malformed(
                                "PLY header: scalar \"property\" must be a type and a name",
                            ));
                        }
                        let ty = ScalarType::from_keyword(tokens[1]).ok_or_else(|| {
                            malformed(format!(
                                "PLY header: unknown property type \"{}\"",
                                String::from_utf8_lossy(tokens[1])
                            ))
                        })?;
                        PropertyHeader::Scalar {
                            name: String::from_utf8(tokens[2].to_vec()).map_err(|_| {
                                malformed("PLY header: property name is not valid UTF-8")
                            })?,
                            ty,
                        }
                    };
                    elements[element_index].props.push(property);
                }
                b"end_header" => {
                    if tokens.len() != 1 {
                        return Err(malformed(
                            "PLY header: unexpected tokens after \"end_header\"",
                        ));
                    }
                    let header_format = format
                        .ok_or_else(|| malformed("PLY header: missing \"format\" declaration"))?;
                    return Ok(Header {
                        format: header_format,
                        elements,
                        data_start: if has_newline {
                            line_end + 1
                        } else {
                            bytes.len()
                        },
                    });
                }
                other => {
                    return Err(malformed(format!(
                        "PLY header: unrecognized keyword \"{}\"",
                        String::from_utf8_lossy(other)
                    )));
                }
            }
        }
        // Blank lines are tolerated in the header (never in the data).
        pos = if has_newline {
            line_end + 1
        } else {
            bytes.len()
        };
    }
    Err(malformed("PLY header: missing \"end_header\" line"))
}

/// Validate the header and derive the per-vertex layout and body offsets.
fn plan_vertex(header: &Header) -> Result<VertexPlan, PointCloudError> {
    let vertex_index = header
        .elements
        .iter()
        .position(|element| element.name == "vertex")
        .ok_or_else(|| malformed("PLY header: no \"vertex\" element is declared"))?;
    let vertex = &header.elements[vertex_index];

    // Color layout (spec F1): either three uchar channels named r/g/b, or
    // the `rgb` dialect (three uchar bytes under a single property).
    let is_uchar = |name: &str| {
        vertex.props.iter().any(|property| {
            !property.is_list()
                && property.name() == name
                && property.scalar_ty() == Some(ScalarType::UChar)
        })
    };
    let channel_triple = is_uchar("r") && is_uchar("g") && is_uchar("b");
    let dialect = is_uchar("rgb");
    if channel_triple && dialect {
        return Err(malformed(
            "PLY header: conflicting color layouts (both \"r g b\" channels and an \"rgb\" property)",
        ));
    }
    let has_color = channel_triple || dialect;

    let mut fields = Vec::with_capacity(vertex.props.len());
    let mut saw_x = false;
    let mut saw_y = false;
    let mut saw_z = false;
    for property in &vertex.props {
        if property.is_list() {
            return Err(malformed(format!(
                "PLY header: list property \"{}\" on the vertex element is not supported",
                property.name()
            )));
        }
        // All vertex properties are scalar here; the list check above proves it.
        let Some(ty) = property.scalar_ty() else {
            return Err(malformed(
                "PLY header: list property on the vertex element is not supported",
            ));
        };
        let name = property.name();
        let role = match name {
            "x" => {
                saw_x = true;
                FieldRole::X
            }
            "y" => {
                saw_y = true;
                FieldRole::Y
            }
            "z" => {
                saw_z = true;
                FieldRole::Z
            }
            "rgb" if ty == ScalarType::UChar => FieldRole::DialectRgb,
            "r" if channel_triple => FieldRole::ChannelR,
            "g" if channel_triple => FieldRole::ChannelG,
            "b" if channel_triple => FieldRole::ChannelB,
            _ => FieldRole::Skip,
        };
        if matches!(role, FieldRole::X | FieldRole::Y | FieldRole::Z) && !ty.supports_coordinates()
        {
            return Err(malformed(format!(
                "PLY header: coordinate property \"{name}\" uses type \"{}\", which is not supported for coordinates",
                ty.keyword()
            )));
        }
        let (size, tokens) = match role {
            FieldRole::DialectRgb => (3, 3),
            _ => (ty.size(), 1),
        };
        fields.push(Field {
            role,
            ty,
            size,
            tokens,
        });
    }
    if !(saw_x && saw_y && saw_z) {
        return Err(malformed(
            "PLY header: the vertex element must declare x, y and z coordinate properties",
        ));
    }

    // Records of elements declared before the vertex element precede the
    // vertex records in the body; skip them. List-using elements cannot be
    // skipped (variable-length records) and are rejected when non-empty.
    let mut skip_bytes: u128 = 0;
    let mut skip_records: u128 = 0;
    for element in header.elements.iter().take(vertex_index) {
        skip_records += u128::from(element.count);
        if element.count == 0 {
            continue;
        }
        if element.props.iter().any(PropertyHeader::is_list) {
            return Err(malformed(format!(
                "PLY header: element \"{}\" is declared before the vertex element and uses list properties; its records cannot be skipped",
                element.name
            )));
        }
        // No list properties remain here, so `0` for a list is unreachable.
        let stride: usize = element
            .props
            .iter()
            .map(|property| match property {
                PropertyHeader::Scalar { ty, .. } => ty.size(),
                PropertyHeader::List { .. } => 0,
            })
            .sum();
        skip_bytes += u128::from(element.count) * stride as u128;
    }

    let stride = fields.iter().map(|field| field.size).sum();
    let tokens_per_vertex = fields.iter().map(|field| field.tokens).sum();
    Ok(VertexPlan {
        fields,
        count: vertex.count,
        stride,
        tokens_per_vertex,
        has_color,
        skip_bytes,
        skip_records,
    })
}

fn read_binary_vertices(
    data: &[u8],
    plan: &VertexPlan,
) -> Result<(Vec<Vec3>, Option<Vec<Color>>), PointCloudError> {
    // Allocation guard: the declared vertices must fit into the remaining
    // body bytes exactly (plan §5), otherwise the file is truncated or the
    // count is bogus — reject before allocating anything.
    let required = plan.skip_bytes + u128::from(plan.count) * plan.stride as u128;
    if required > data.len() as u128 {
        return Err(malformed(format!(
            "PLY binary data: header declares {} vertices at a {}-byte stride ({} bytes required), but only {} bytes of body data remain",
            plan.count,
            plan.stride,
            required,
            data.len()
        )));
    }
    // `required <= data.len()` bounds every conversion below.
    let skip = usize::try_from(plan.skip_bytes)
        .map_err(|_| malformed("PLY binary data: vertex region exceeds platform limits"))?;
    let count = usize::try_from(plan.count)
        .map_err(|_| malformed("PLY binary data: vertex count exceeds platform limits"))?;
    let vertex_bytes = count * plan.stride;
    let region = data
        .get(skip..skip + vertex_bytes)
        .ok_or_else(|| malformed("PLY binary data: vertex region out of bounds"))?;

    let mut positions = Vec::with_capacity(count);
    let mut colors = plan.has_color.then(|| Vec::with_capacity(count));
    for record in region.chunks_exact(plan.stride) {
        let mut accumulator = VertexAccumulator::new();
        let mut offset = 0usize;
        for field in &plan.fields {
            let chunk = record
                .get(offset..offset + field.size)
                .ok_or_else(|| malformed("PLY binary data: vertex record is truncated"))?;
            offset += field.size;
            if field.role == FieldRole::DialectRgb {
                if chunk.len() < 3 {
                    return Err(malformed(
                        "PLY binary data: the \"rgb\" property must span three bytes",
                    ));
                }
                accumulator.r = chunk[0];
                accumulator.g = chunk[1];
                accumulator.b = chunk[2];
                continue;
            }
            let value = decode_binary_scalar(chunk, field.ty).ok_or_else(|| {
                malformed(format!(
                    "PLY binary data: cannot decode a \"{}\" property",
                    field.ty.keyword()
                ))
            })?;
            match field.role {
                FieldRole::X => accumulator.x = value as f32,
                FieldRole::Y => accumulator.y = value as f32,
                FieldRole::Z => accumulator.z = value as f32,
                FieldRole::ChannelR => accumulator.r = value as u8,
                FieldRole::ChannelG => accumulator.g = value as u8,
                FieldRole::ChannelB => accumulator.b = value as u8,
                // Skip fields are decoded (bounds-validated) and discarded.
                FieldRole::Skip | FieldRole::DialectRgb => {}
            }
        }
        store_vertex(&mut positions, &mut colors, accumulator);
    }
    Ok((positions, colors))
}

fn read_ascii_vertices(
    data: &[u8],
    plan: &VertexPlan,
) -> Result<(Vec<Vec3>, Option<Vec<Color>>), PointCloudError> {
    // Each record occupies at least one body byte, so the declared record
    // count is bounded by the body length (allocation guard).
    let required_records = plan.skip_records + u128::from(plan.count);
    if required_records > data.len() as u128 {
        return Err(malformed(format!(
            "PLY ASCII data: header declares {} vertex records, but the body is too short",
            plan.count
        )));
    }
    let skip = usize::try_from(plan.skip_records)
        .map_err(|_| malformed("PLY ASCII data: record count exceeds platform limits"))?;
    let count = usize::try_from(plan.count)
        .map_err(|_| malformed("PLY ASCII data: vertex count exceeds platform limits"))?;

    let mut lines = LineIter::new(data);
    for _ in 0..skip {
        if lines.next().is_none() {
            return Err(malformed(
                "PLY ASCII data: file ends before the vertex records",
            ));
        }
    }
    let mut positions = Vec::with_capacity(count);
    let mut colors = plan.has_color.then(|| Vec::with_capacity(count));
    for record_index in 0..count {
        let line = lines.next().ok_or_else(|| {
            malformed(format!(
                "PLY ASCII data: file ends after {record_index} of {count} declared vertex records"
            ))
        })?;
        let tokens: Vec<&[u8]> = line
            .split(|b| b.is_ascii_whitespace())
            .filter(|token| !token.is_empty())
            .collect();
        if tokens.len() != plan.tokens_per_vertex {
            return Err(malformed(format!(
                "PLY ASCII data: vertex record {record_index} has {} tokens; expected {} (one per declared property)",
                tokens.len(),
                plan.tokens_per_vertex
            )));
        }
        // Token consumption sums to `tokens_per_vertex == tokens.len()`, so
        // every index below is in bounds.
        let mut token_index = 0usize;
        let mut accumulator = VertexAccumulator::new();
        for field in &plan.fields {
            if field.role == FieldRole::DialectRgb {
                for channel in [&mut accumulator.r, &mut accumulator.g, &mut accumulator.b] {
                    *channel = decode_ascii_scalar(tokens[token_index], ScalarType::UChar)? as u8;
                    token_index += 1;
                }
                continue;
            }
            let value = decode_ascii_scalar(tokens[token_index], field.ty)?;
            token_index += 1;
            match field.role {
                FieldRole::X => accumulator.x = value as f32,
                FieldRole::Y => accumulator.y = value as f32,
                FieldRole::Z => accumulator.z = value as f32,
                FieldRole::ChannelR => accumulator.r = value as u8,
                FieldRole::ChannelG => accumulator.g = value as u8,
                FieldRole::ChannelB => accumulator.b = value as u8,
                FieldRole::Skip | FieldRole::DialectRgb => {}
            }
        }
        store_vertex(&mut positions, &mut colors, accumulator);
    }
    Ok((positions, colors))
}

/// Decode one little-endian scalar field of a binary body. `None` when the
/// chunk does not have the exact size of the declared type.
fn decode_binary_scalar(bytes: &[u8], ty: ScalarType) -> Option<f64> {
    let value = match ty {
        ScalarType::Char => i8::from_le_bytes(bytes.try_into().ok()?) as f64,
        ScalarType::UChar => u8::from_le_bytes(bytes.try_into().ok()?) as f64,
        ScalarType::Short => i16::from_le_bytes(bytes.try_into().ok()?) as f64,
        ScalarType::UShort => u16::from_le_bytes(bytes.try_into().ok()?) as f64,
        ScalarType::Int => i32::from_le_bytes(bytes.try_into().ok()?) as f64,
        ScalarType::UInt => u32::from_le_bytes(bytes.try_into().ok()?) as f64,
        ScalarType::Float => f32::from_le_bytes(bytes.try_into().ok()?) as f64,
        ScalarType::Double => f64::from_le_bytes(bytes.try_into().ok()?),
    };
    Some(value)
}

/// Parse one ASCII token as the declared type, with exact integer range
/// checking. Non-finite tokens (`nan`/`inf`) parse for float and double
/// types only, which is how G1 data survives an ASCII round trip.
fn decode_ascii_scalar(token: &[u8], ty: ScalarType) -> Result<f64, PointCloudError> {
    let text = std::str::from_utf8(token)
        .map_err(|_| malformed("PLY ASCII data: numeric token is not valid UTF-8"))?;
    let kind = ty.keyword();
    let invalid = || malformed(format!("PLY ASCII data: expected a \"{kind}\" number"));
    let value = match ty {
        ScalarType::Char => text.parse::<i8>().map_err(|_| invalid())? as f64,
        ScalarType::UChar => text.parse::<u8>().map_err(|_| invalid())? as f64,
        ScalarType::Short => text.parse::<i16>().map_err(|_| invalid())? as f64,
        ScalarType::UShort => text.parse::<u16>().map_err(|_| invalid())? as f64,
        ScalarType::Int => text.parse::<i32>().map_err(|_| invalid())? as f64,
        ScalarType::UInt => text.parse::<u32>().map_err(|_| invalid())? as f64,
        ScalarType::Float => text.parse::<f32>().map_err(|_| invalid())? as f64,
        ScalarType::Double => text.parse::<f64>().map_err(|_| invalid())?,
    };
    Ok(value)
}

/// Body lines without terminators. A trailing `\r` is removed so CRLF files
/// behave like LF files (plan §5); the last line needs no newline.
struct LineIter<'a> {
    rest: &'a [u8],
}

impl<'a> LineIter<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { rest: bytes }
    }
}

impl<'a> Iterator for LineIter<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<&'a [u8]> {
        if self.rest.is_empty() {
            return None;
        }
        let (line, rest) = match self.rest.iter().position(|&b| b == b'\n') {
            Some(offset) => (&self.rest[..offset], &self.rest[offset + 1..]),
            None => (self.rest, &[][..]),
        };
        self.rest = rest;
        Some(if line.last() == Some(&b'\r') {
            &line[..line.len() - 1]
        } else {
            line
        })
    }
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

    fn f64_bytes(value: f64) -> [u8; 8] {
        value.to_le_bytes()
    }

    /// ASCII fixture: header over `props` (one header line each) and `body`.
    fn ascii_fixture(vertex_count: u64, props: &[&str], body: &str) -> Vec<u8> {
        let mut text = format!("ply\nformat ascii 1.0\nelement vertex {vertex_count}\n");
        for prop in props {
            text.push_str(prop);
            text.push('\n');
        }
        text.push_str("end_header\n");
        text.push_str(body);
        text.into_bytes()
    }

    /// Concatenate heterogeneous byte parts into one binary fixture body.
    fn body(parts: &[&[u8]]) -> Vec<u8> {
        let mut out = Vec::new();
        for part in parts {
            out.extend_from_slice(part);
        }
        out
    }

    /// Binary fixture: header over `props`, `body` bytes appended verbatim.
    fn binary_fixture(vertex_count: u64, props: &[&str], body: &[u8]) -> Vec<u8> {
        let mut text =
            format!("ply\nformat binary_little_endian 1.0\nelement vertex {vertex_count}\n");
        for prop in props {
            text.push_str(prop);
            text.push('\n');
        }
        text.push_str("end_header\n");
        let mut bytes = text.into_bytes();
        bytes.extend_from_slice(body);
        bytes
    }

    fn parse_ok(bytes: &[u8]) -> PointCloudData {
        parse_ply(bytes).expect("fixture must load")
    }

    fn parse_err(bytes: &[u8]) -> PointCloudError {
        parse_ply(bytes).expect_err("fixture must be rejected")
    }

    fn positions_of(data: &PointCloudData) -> &[Vec3] {
        &data.positions
    }

    #[test]
    fn ascii_xyz_float() {
        let file = ascii_fixture(
            2,
            &["property float x", "property float y", "property float z"],
            "1.5 -2.25 0\n10 0.2 3\n",
        );
        let data = parse_ok(&file);
        assert_eq!(
            data.positions,
            [Vec3::new(1.5, -2.25, 0.0), Vec3::new(10.0, 0.2, 3.0)]
        );
        assert!(data.colors.is_none());
        let bounds = data.bounds.expect("finite points must yield bounds");
        assert_eq!(bounds.min, Vec3::new(1.5, -2.25, 0.0));
        assert_eq!(bounds.max, Vec3::new(10.0, 0.2, 3.0));
        assert_eq!(data.format, Format::Ply);
    }

    #[test]
    fn ascii_scientific_notation_and_blank_lines_in_body_rejected() {
        // Scientific notation and a missing trailing newline are valid.
        let file = ascii_fixture(
            2,
            &["property float x", "property float y", "property float z"],
            "1e1 -2.5e-1 3e0\n4 5 6",
        );
        let data = parse_ok(&file);
        assert_eq!(positions_of(&data)[0].x, 10.0);
        assert_eq!(positions_of(&data)[0].y, -0.25);
        assert_eq!(positions_of(&data)[0].z, 3.0);

        // A blank line inside the vertex records breaks the token count.
        let file = ascii_fixture(
            2,
            &["property float x", "property float y", "property float z"],
            "1 2 3\n\n4 5 6\n",
        );
        assert!(matches!(
            parse_err(&file),
            PointCloudError::Malformed { .. }
        ));
    }

    #[test]
    fn ascii_crlf_header_and_body() {
        // CRLF line endings everywhere, multi-space separators.
        let mut file = b"ply\r\nformat ascii 1.0\r\nelement vertex 2\r\nproperty float x\r\nproperty float y\r\nproperty float z\r\nend_header\r\n".to_vec();
        file.extend_from_slice(b"1  2   3\r\n4\t5 6\r\n");
        let data = parse_ok(&file);
        assert_eq!(data.point_count(), 2);
        assert_eq!(positions_of(&data)[1], Vec3::new(4.0, 5.0, 6.0));
    }

    #[test]
    fn elements_before_and_after_vertex() {
        // A scalar face element declared before vertex: its record line is
        // skipped. A face element after vertex is simply ignored.
        let file = b"ply\nformat ascii 1.0\nelement face 1\nproperty int vertex_indices\nelement vertex 1\nproperty float x\nproperty float y\nproperty float z\nend_header\n42\n1 2 3\n".to_vec();
        let data = parse_ok(&file);
        assert_eq!(positions_of(&data), &[Vec3::new(1.0, 2.0, 3.0)]);

        // A list-using face element after the vertex element is fine.
        let file = b"ply\nformat ascii 1.0\nelement vertex 1\nproperty float x\nproperty float y\nproperty float z\nelement face 1\nproperty list uchar int vertex_indices\nend_header\n1 2 3\n3 0 0 0\n".to_vec();
        let data = parse_ok(&file);
        assert_eq!(positions_of(&data), &[Vec3::new(1.0, 2.0, 3.0)]);
    }

    #[test]
    fn list_property_rejected() {
        // List property on the vertex element: stride is unknowable.
        let file = ascii_fixture(
            1,
            &[
                "property float x",
                "property float y",
                "property float z",
                "property list uchar int vertex_indices",
            ],
            "1 2 3\n",
        );
        assert!(matches!(
            parse_err(&file),
            PointCloudError::Malformed { .. }
        ));

        // List-using element declared *before* vertex: cannot skip.
        let file = b"ply\nformat ascii 1.0\nelement face 1\nproperty list uchar int vertex_indices\nelement vertex 1\nproperty float x\nproperty float y\nproperty float z\nend_header\n".to_vec();
        assert!(matches!(
            parse_err(&file),
            PointCloudError::Malformed { .. }
        ));
    }

    #[test]
    fn missing_or_bad_header_pieces() {
        // Empty file.
        assert!(matches!(parse_err(b""), PointCloudError::Malformed { .. }));
        // Bad magic.
        let file = b"plyx\nformat ascii 1.0\nelement vertex 1\nproperty float x\nend_header\n";
        assert!(matches!(parse_err(file), PointCloudError::Malformed { .. }));
        // Magic without trailing newline still counts as the first line.
        let file = b"ply\nformat ascii 1.0\nelement vertex 0\nproperty float x\nproperty float y\nproperty float z\nend_header";
        assert!(parse_ok(file).point_count() == 0);
        // Missing end_header.
        let file = b"ply\nformat ascii 1.0\nelement vertex 0\n";
        assert!(matches!(parse_err(file), PointCloudError::Malformed { .. }));
        // Missing format declaration.
        let file = b"ply\nelement vertex 0\nend_header\n";
        assert!(matches!(parse_err(file), PointCloudError::Malformed { .. }));
        // Unknown keyword.
        let file = b"ply\nformat ascii 1.0\nmystery line here\nend_header\n";
        assert!(matches!(parse_err(file), PointCloudError::Malformed { .. }));
        // Unknown property type.
        let file = b"ply\nformat ascii 1.0\nelement vertex 0\nproperty float32 x\nend_header\n";
        assert!(matches!(parse_err(file), PointCloudError::Malformed { .. }));
    }

    #[test]
    fn unsupported_storage_modes() {
        for mode in [
            b"binary_big_endian".as_slice(),
            b"binary_little_endian2".as_slice(),
        ] {
            let mut file = b"ply\nformat ".to_vec();
            file.extend_from_slice(mode);
            file.extend_from_slice(b" 1.0\nelement vertex 0\nend_header\n");
            assert!(matches!(
                parse_err(&file),
                PointCloudError::Malformed { .. }
            ));
        }
        // Invalid format version token.
        let file = b"ply\nformat ascii nope\nelement vertex 0\nend_header\n";
        assert!(matches!(parse_err(file), PointCloudError::Malformed { .. }));
    }

    #[test]
    fn ascii_color_channels() {
        let file = ascii_fixture(
            2,
            &[
                "property float x",
                "property float y",
                "property float z",
                "property uchar r",
                "property uchar g",
                "property uchar b",
            ],
            "0 0 0 255 0 0\n1 2 3 10 20 30\n",
        );
        let data = parse_ok(&file);
        let colors = data.colors.as_ref().expect("colors must be present");
        assert_eq!(colors[0], Color { r: 255, g: 0, b: 0 });
        assert_eq!(
            colors[1],
            Color {
                r: 10,
                g: 20,
                b: 30
            }
        );
        assert_eq!(data.positions.len(), colors.len());
    }

    #[test]
    fn ascii_color_channels_out_of_order_and_extra_props() {
        // Channels declared in a different order are assembled by name; an
        // unrelated scalar property is skipped.
        let file = ascii_fixture(
            1,
            &[
                "property uchar b",
                "property uchar r",
                "property uchar g",
                "property float x",
                "property float y",
                "property float z",
                "property float nx",
            ],
            "3 1 2 0 0 0 0\n",
        );
        let data = parse_ok(&file);
        let colors = data.colors.as_ref().expect("colors must be present");
        assert_eq!(colors[0], Color { r: 1, g: 2, b: 3 });
    }

    #[test]
    fn ascii_rgb_dialect_three_tokens() {
        let file = ascii_fixture(
            1,
            &[
                "property float x",
                "property float y",
                "property float z",
                "property uchar rgb",
            ],
            "1 2 3 200 100 50\n",
        );
        let data = parse_ok(&file);
        let colors = data
            .colors
            .as_ref()
            .expect("dialect colors must be present");
        assert_eq!(
            colors[0],
            Color {
                r: 200,
                g: 100,
                b: 50
            }
        );
    }

    #[test]
    fn partial_or_float_channels_mean_no_color() {
        // Only two uchar channels: not a supported color layout.
        let file = ascii_fixture(
            1,
            &[
                "property float x",
                "property float y",
                "property float z",
                "property uchar r",
                "property uchar g",
            ],
            "1 2 3 4 5\n",
        );
        let data = parse_ok(&file);
        assert!(data.colors.is_none());

        // Float channels are not the supported uchar variant.
        let file = ascii_fixture(
            1,
            &[
                "property float x",
                "property float y",
                "property float z",
                "property float r",
                "property float g",
                "property float b",
            ],
            "1 2 3 0.5 0.5 0.5\n",
        );
        let data = parse_ok(&file);
        assert!(data.colors.is_none());
    }

    #[test]
    fn both_color_layouts_rejected() {
        let file = ascii_fixture(
            0,
            &[
                "property float x",
                "property float y",
                "property float z",
                "property uchar r",
                "property uchar g",
                "property uchar b",
                "property uchar rgb",
            ],
            "",
        );
        assert!(matches!(
            parse_err(&file),
            PointCloudError::Malformed { .. }
        ));
    }

    #[test]
    fn coordinate_type_matrix() {
        // uchar, ushort, uint, int, double are fine in ASCII.
        let file = ascii_fixture(
            1,
            &["property uchar x", "property short y", "property float z"],
            "",
        );
        assert!(matches!(
            parse_err(&file),
            PointCloudError::Malformed { .. }
        ));

        let file = ascii_fixture(
            1,
            &["property uchar x", "property double y", "property int z"],
            "1 2.5 3\n",
        );
        let data = parse_ok(&file);
        assert_eq!(positions_of(&data)[0], Vec3::new(1.0, 2.5, 3.0));

        // Missing coordinates.
        let file = ascii_fixture(0, &["property float x", "property float y"], "");
        assert!(matches!(
            parse_err(&file),
            PointCloudError::Malformed { .. }
        ));
        // No vertex element at all.
        let file = b"ply\nformat ascii 1.0\nelement face 0\nend_header\n";
        assert!(matches!(parse_err(file), PointCloudError::Malformed { .. }));
    }

    #[test]
    fn ascii_nan_inf_kept_and_excluded_from_bounds() {
        let file = ascii_fixture(
            3,
            &["property float x", "property float y", "property float z"],
            "nan inf 0\n1 2 3\n-inf -2 4\n",
        );
        let data = parse_ok(&file);
        assert!(positions_of(&data)[0].x.is_nan());
        assert_eq!(positions_of(&data)[0].y, f32::INFINITY);
        assert_eq!(positions_of(&data)[2].x, f32::NEG_INFINITY);
        // Bounds cover the finite point only.
        let bounds = data.bounds.expect("one finite point must yield bounds");
        assert_eq!(bounds.min, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(bounds.max, Vec3::new(1.0, 2.0, 3.0));
    }

    #[test]
    fn all_non_finite_gives_no_bounds() {
        let file = ascii_fixture(
            2,
            &["property float x", "property float y", "property float z"],
            "nan 1 2\ninf nan nan\n",
        );
        let data = parse_ok(&file);
        assert!(data.bounds.is_none());
        assert_eq!(data.point_count(), 2);
    }

    #[test]
    fn ascii_errors() {
        // Out-of-range color channel value.
        let file = ascii_fixture(
            1,
            &[
                "property float x",
                "property float y",
                "property float z",
                "property uchar r",
                "property uchar g",
                "property uchar b",
            ],
            "0 0 0 300 0 0\n",
        );
        assert!(matches!(
            parse_err(&file),
            PointCloudError::Malformed { .. }
        ));
        // Garbage token.
        let file = ascii_fixture(
            1,
            &["property float x", "property float y", "property float z"],
            "1 2 three\n",
        );
        assert!(matches!(
            parse_err(&file),
            PointCloudError::Malformed { .. }
        ));
        // Too few tokens in a record.
        let file = ascii_fixture(
            1,
            &["property float x", "property float y", "property float z"],
            "1 2\n",
        );
        assert!(matches!(
            parse_err(&file),
            PointCloudError::Malformed { .. }
        ));
        // Truncated body: one of two declared records missing.
        let file = ascii_fixture(
            2,
            &["property float x", "property float y", "property float z"],
            "1 2 3\n",
        );
        assert!(matches!(
            parse_err(&file),
            PointCloudError::Malformed { .. }
        ));
        // Integer coordinate with a fractional token.
        let file = ascii_fixture(
            1,
            &["property int x", "property int y", "property int z"],
            "1 2.5 3\n",
        );
        assert!(matches!(
            parse_err(&file),
            PointCloudError::Malformed { .. }
        ));
    }

    #[test]
    fn binary_xyz_float_and_colors() {
        let body = body(&[
            &f32_bytes(1.0),
            &f32_bytes(2.0),
            &f32_bytes(3.0),
            &[255, 0, 0],
            &f32_bytes(4.0),
            &f32_bytes(5.0),
            &f32_bytes(6.0),
            &[10, 20, 30],
        ]);
        let file = binary_fixture(
            2,
            &[
                "property float x",
                "property float y",
                "property float z",
                "property uchar r",
                "property uchar g",
                "property uchar b",
            ],
            &body,
        );
        let data = parse_ok(&file);
        assert_eq!(
            positions_of(&data),
            &[Vec3::new(1.0, 2.0, 3.0), Vec3::new(4.0, 5.0, 6.0)]
        );
        let colors = data.colors.as_ref().expect("colors must be present");
        assert_eq!(colors[0], Color { r: 255, g: 0, b: 0 });
        assert_eq!(
            colors[1],
            Color {
                r: 10,
                g: 20,
                b: 30
            }
        );
    }

    #[test]
    fn binary_rgb_dialect_15_byte_stride() {
        // xyz (12 bytes) + rgb dialect (3 bytes) = 15-byte stride; decoding
        // the second vertex proves the dialect does not desync the records.
        let body = body(&[
            &f32_bytes(1.0),
            &f32_bytes(2.0),
            &f32_bytes(3.0),
            &[200, 100, 50],
            &f32_bytes(-4.0),
            &f32_bytes(-5.0),
            &f32_bytes(6.0),
            &[1, 2, 3],
        ]);
        let file = binary_fixture(
            2,
            &[
                "property float x",
                "property float y",
                "property float z",
                "property uchar rgb",
            ],
            &body,
        );
        let data = parse_ok(&file);
        assert_eq!(
            positions_of(&data),
            &[Vec3::new(1.0, 2.0, 3.0), Vec3::new(-4.0, -5.0, 6.0)]
        );
        let colors = data
            .colors
            .as_ref()
            .expect("dialect colors must be present");
        assert_eq!(
            colors[0],
            Color {
                r: 200,
                g: 100,
                b: 50
            }
        );
        assert_eq!(colors[1], Color { r: 1, g: 2, b: 3 });
    }

    #[test]
    fn binary_integer_and_double_coordinates() {
        // uint coordinates (le32) and a double coordinate (le64).
        let body = body(&[
            &le32(7),
            &le32(8),
            &f64_bytes(9.5),
            &le32(3),
            &le32(4),
            &f64_bytes(-2.5),
        ]);
        let file = binary_fixture(
            2,
            &["property uint x", "property uint y", "property double z"],
            &body,
        );
        let data = parse_ok(&file);
        assert_eq!(
            positions_of(&data),
            &[Vec3::new(7.0, 8.0, 9.5), Vec3::new(3.0, 4.0, -2.5)]
        );
    }

    #[test]
    fn binary_extra_scalar_props_are_skipped() {
        let body = body(&[
            &f32_bytes(1.0),
            &f32_bytes(2.0),
            &f32_bytes(3.0),
            &le32(42), // quality, skipped
            &f32_bytes(4.0),
            &f32_bytes(5.0),
            &f32_bytes(6.0),
            &le32(7), // quality, skipped
        ]);
        let file = binary_fixture(
            2,
            &[
                "property float x",
                "property float y",
                "property float z",
                "property int quality",
            ],
            &body,
        );
        let data = parse_ok(&file);
        assert_eq!(
            positions_of(&data),
            &[Vec3::new(1.0, 2.0, 3.0), Vec3::new(4.0, 5.0, 6.0)]
        );
    }

    #[test]
    fn binary_truncated_and_huge_count_rejected() {
        // Half a record missing.
        let body: Vec<u8> = [f32_bytes(1.0), f32_bytes(2.0), f32_bytes(3.0)].concat();
        let file = binary_fixture(
            2,
            &["property float x", "property float y", "property float z"],
            &body,
        );
        assert!(matches!(
            parse_err(&file),
            PointCloudError::Malformed { .. }
        ));

        // Absurd declared count: guard rejects before any allocation.
        let file = binary_fixture(
            u64::from(u32::MAX),
            &["property float x", "property float y", "property float z"],
            &body,
        );
        assert!(matches!(
            parse_err(&file),
            PointCloudError::Malformed { .. }
        ));
    }

    #[test]
    fn binary_nan_inf_kept_and_colors_synced() {
        let body = body(&[
            &f32_bytes(f32::NAN),
            &f32_bytes(f32::INFINITY),
            &f32_bytes(0.0),
            &[1, 2, 3],
            &f32_bytes(f32::NEG_INFINITY),
            &f32_bytes(1.0),
            &f32_bytes(2.0),
            &[4, 5, 6],
            &f32_bytes(3.0),
            &f32_bytes(4.0),
            &f32_bytes(5.0),
            &[7, 8, 9],
        ]);
        let file = binary_fixture(
            3,
            &[
                "property float x",
                "property float y",
                "property float z",
                "property uchar r",
                "property uchar g",
                "property uchar b",
            ],
            &body,
        );
        let data = parse_ok(&file);
        assert!(positions_of(&data)[0].x.is_nan());
        assert_eq!(positions_of(&data)[0].y, f32::INFINITY);
        assert_eq!(positions_of(&data)[1].x, f32::NEG_INFINITY);
        let colors = data.colors.as_ref().expect("colors must stay in sync");
        assert_eq!(colors[0], Color { r: 1, g: 2, b: 3 });
        assert_eq!(colors[1], Color { r: 4, g: 5, b: 6 });
        assert_eq!(colors[2], Color { r: 7, g: 8, b: 9 });
        // Only the third vertex is finite, so the bounds collapse onto it.
        let bounds = data.bounds.expect("one finite point must yield bounds");
        assert_eq!(bounds.min, Vec3::new(3.0, 4.0, 5.0));
        assert_eq!(bounds.max, Vec3::new(3.0, 4.0, 5.0));
    }

    #[test]
    fn empty_vertex_element() {
        let file = ascii_fixture(
            0,
            &["property float x", "property float y", "property float z"],
            "",
        );
        let data = parse_ok(&file);
        assert_eq!(data.point_count(), 0);
        assert!(data.colors.is_none());
        assert!(data.bounds.is_none());

        let file = binary_fixture(
            0,
            &["property float x", "property float y", "property float z"],
            b"",
        );
        let data = parse_ok(&file);
        assert_eq!(data.point_count(), 0);
    }

    #[test]
    fn comment_and_obj_info_lines_ignored() {
        // Comments, obj_info lines and blank lines may appear in the header.
        let file = b"ply\nformat ascii 1.0\ncomment made by a robot\nobj_info xyz\ncomment\n\nelement vertex 1\nproperty float x\nproperty float y\nproperty float z\nend_header\n1 2 3\n".to_vec();
        let data = parse_ok(&file);
        assert_eq!(positions_of(&data), &[Vec3::new(1.0, 2.0, 3.0)]);
    }
}
