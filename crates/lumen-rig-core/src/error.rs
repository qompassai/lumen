//! Typed parse and validation errors.
//!
//! Every variant maps to exactly one stable code in the loud-error
//! table (SPEC.md section 9.1). Context strings are bounded: ids in
//! the format are at most 64 bytes, and free-form strings copied
//! from the input are truncated before they enter an error.

use std::fmt;

/// Maximum bytes of an input string quoted inside an error.
const ERROR_QUOTE_BYTES_MAX: usize = 96;

/// Truncate a string to at most `max` bytes on a char boundary.
/// Used so attacker-sized input strings cannot bloat error values.
pub(crate) fn bounded_quote(text: &str) -> String {
    if text.len() <= ERROR_QUOTE_BYTES_MAX {
        return text.to_string();
    }
    let mut end = ERROR_QUOTE_BYTES_MAX;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

/// A parse or validation failure. A conforming load stops at the
/// first of these, in the validation order of SPEC.md section 8.
#[derive(Debug, Clone, PartialEq)]
pub enum ParseError {
    /// LRF-101: input longer than `INPUT_BYTES_MAX`.
    InputTooLarge { bytes: usize, max: usize },
    /// LRF-102: JSON syntax error (including a leading BOM).
    Syntax {
        line: usize,
        column: usize,
        message: String,
    },
    /// LRF-103: schema error — missing field, wrong type,
    /// duplicate key, malformed integer.
    Schema { message: String },
    /// LRF-104: a field the format does not define.
    UnknownField { message: String },
    /// LRF-105: `format` is not `lumen-rig-model`.
    UnsupportedFormat { found: String },
    /// LRF-106: `format_version` is not implemented.
    UnsupportedVersion { found: u32 },
    /// LRF-107: a collection exceeds its section-11 bound.
    CountExceeded {
        collection: &'static str,
        count: usize,
        max: usize,
    },
    /// LRF-201: id violates the charset or length rule.
    InvalidId { id: String },
    /// LRF-202: duplicate id within one collection.
    DuplicateId {
        collection: &'static str,
        id: String,
    },
    /// LRF-203: a reference names an object that does not exist.
    DanglingReference {
        collection: &'static str,
        from_id: String,
        reference: String,
    },
    /// LRF-204: parent or mask reference cycle, or tree too deep.
    ReferenceCycle {
        collection: &'static str,
        id: String,
    },
    /// LRF-205: combined-parameter reference cycle.
    CombinedCycle { id: String },
    /// LRF-206: texture source path violates SPEC.md section 3.3.
    InvalidTexturePath { texture_id: String, source: String },
    /// LRF-207: a reference names an object of the wrong kind
    /// (for example a combined parameter used as a lattice axis).
    WrongKind { context: String, id: String },
    /// LRF-301: a numeric field is NaN, infinite, or overflows f32.
    NonFinite { context: String },
    /// LRF-302: parameter `min >= max`.
    ParameterOrder { id: String, min: f32, max: f32 },
    /// LRF-303: a stated value outside its declared range.
    OutOfRange { context: String },
    /// LRF-304: a combined parameter's reachable range is not
    /// contained in its declared range.
    CombinedRangeNotContained { id: String },
    /// LRF-305: a field-specific range violation (scale, damping,
    /// coordinate magnitude, particle length, and similar).
    FieldRange { context: String },
    /// LRF-401: a mesh index names a vertex that does not exist.
    IndexOutOfBounds {
        mesh_id: String,
        index: u32,
        vertex_count: usize,
    },
    /// LRF-402: index count is not a multiple of 3, or is below 3.
    IndexCountInvalid { mesh_id: String, count: usize },
    /// LRF-403: parallel arrays disagree in length.
    LengthMismatch { context: String },
    /// LRF-404: a keyform lattice is malformed.
    LatticeInvalid {
        context: String,
        reason: &'static str,
    },
    /// LRF-405: a blend mode string outside the section-3.5 set.
    UnsupportedBlendMode { mesh_id: String, value: String },
    /// LRF-501: keyframe times are not strictly increasing,
    /// or fall outside `[0, duration_seconds]`.
    KeyframeTimes {
        motion_id: String,
        parameter: String,
    },
    /// LRF-502: a cubic tangent is invalid (negative `dt`, a
    /// tangent reaching past its neighboring key, or a tangent
    /// carried by a non-cubic key).
    TangentInvalid { context: String },
    /// LRF-601: two physics strands write the same parameter.
    PhysicsOutputConflict { parameter: String },
    /// LRF-602: a motion curve targets a physics-output
    /// parameter (the one-writer rule, SPEC.md section 4.1).
    PhysicsOutputTarget {
        motion_id: String,
        parameter: String,
    },
}

impl ParseError {
    /// The stable `LRF-xxx` code from SPEC.md section 9.1.
    pub fn code(&self) -> &'static str {
        match self {
            ParseError::InputTooLarge { .. } => "LRF-101",
            ParseError::Syntax { .. } => "LRF-102",
            ParseError::Schema { .. } => "LRF-103",
            ParseError::UnknownField { .. } => "LRF-104",
            ParseError::UnsupportedFormat { .. } => "LRF-105",
            ParseError::UnsupportedVersion { .. } => "LRF-106",
            ParseError::CountExceeded { .. } => "LRF-107",
            ParseError::InvalidId { .. } => "LRF-201",
            ParseError::DuplicateId { .. } => "LRF-202",
            ParseError::DanglingReference { .. } => "LRF-203",
            ParseError::ReferenceCycle { .. } => "LRF-204",
            ParseError::CombinedCycle { .. } => "LRF-205",
            ParseError::InvalidTexturePath { .. } => "LRF-206",
            ParseError::WrongKind { .. } => "LRF-207",
            ParseError::NonFinite { .. } => "LRF-301",
            ParseError::ParameterOrder { .. } => "LRF-302",
            ParseError::OutOfRange { .. } => "LRF-303",
            ParseError::CombinedRangeNotContained { .. } => "LRF-304",
            ParseError::FieldRange { .. } => "LRF-305",
            ParseError::IndexOutOfBounds { .. } => "LRF-401",
            ParseError::IndexCountInvalid { .. } => "LRF-402",
            ParseError::LengthMismatch { .. } => "LRF-403",
            ParseError::LatticeInvalid { .. } => "LRF-404",
            ParseError::UnsupportedBlendMode { .. } => "LRF-405",
            ParseError::KeyframeTimes { .. } => "LRF-501",
            ParseError::TangentInvalid { .. } => "LRF-502",
            ParseError::PhysicsOutputConflict { .. } => "LRF-601",
            ParseError::PhysicsOutputTarget { .. } => "LRF-602",
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let code = self.code();
        match self {
            ParseError::InputTooLarge { bytes, max } => {
                write!(f, "{code}: input is {bytes} bytes, max is {max}")
            }
            ParseError::Syntax {
                line,
                column,
                message,
            } => write!(f, "{code}: JSON syntax at {line}:{column}: {message}"),
            ParseError::Schema { message } => write!(f, "{code}: schema: {message}"),
            ParseError::UnknownField { message } => {
                write!(f, "{code}: unknown field: {message}")
            }
            ParseError::UnsupportedFormat { found } => {
                write!(f, "{code}: unsupported format {found:?}")
            }
            ParseError::UnsupportedVersion { found } => {
                write!(f, "{code}: unsupported format_version {found}")
            }
            ParseError::CountExceeded {
                collection,
                count,
                max,
            } => write!(f, "{code}: {collection} count {count} exceeds max {max}"),
            ParseError::InvalidId { id } => write!(f, "{code}: invalid id {id:?}"),
            ParseError::DuplicateId { collection, id } => {
                write!(f, "{code}: duplicate id {id:?} in {collection}")
            }
            ParseError::DanglingReference {
                collection,
                from_id,
                reference,
            } => write!(
                f,
                "{code}: {collection} {from_id:?} references missing {reference:?}"
            ),
            ParseError::ReferenceCycle { collection, id } => {
                write!(f, "{code}: reference cycle at {collection} {id:?}")
            }
            ParseError::CombinedCycle { id } => {
                write!(f, "{code}: combined parameter cycle at {id:?}")
            }
            ParseError::InvalidTexturePath { texture_id, source } => {
                write!(
                    f,
                    "{code}: texture {texture_id:?} has invalid source {source:?}"
                )
            }
            ParseError::WrongKind { context, id } => {
                write!(f, "{code}: wrong kind at {context}: {id:?}")
            }
            ParseError::NonFinite { context } => {
                write!(f, "{code}: non-finite number at {context}")
            }
            ParseError::ParameterOrder { id, min, max } => {
                write!(f, "{code}: parameter {id:?} has min {min} >= max {max}")
            }
            ParseError::OutOfRange { context } => {
                write!(f, "{code}: value out of range at {context}")
            }
            ParseError::CombinedRangeNotContained { id } => {
                write!(f, "{code}: combined parameter {id:?} range not contained")
            }
            ParseError::FieldRange { context } => {
                write!(f, "{code}: field range violation at {context}")
            }
            ParseError::IndexOutOfBounds {
                mesh_id,
                index,
                vertex_count,
            } => write!(
                f,
                "{code}: mesh {mesh_id:?} index {index} >= vertex count {vertex_count}"
            ),
            ParseError::IndexCountInvalid { mesh_id, count } => {
                write!(f, "{code}: mesh {mesh_id:?} index count {count} invalid")
            }
            ParseError::LengthMismatch { context } => {
                write!(f, "{code}: length mismatch at {context}")
            }
            ParseError::LatticeInvalid { context, reason } => {
                write!(f, "{code}: lattice invalid at {context}: {reason}")
            }
            ParseError::UnsupportedBlendMode { mesh_id, value } => {
                write!(
                    f,
                    "{code}: mesh {mesh_id:?} blend mode {value:?} unsupported"
                )
            }
            ParseError::KeyframeTimes {
                motion_id,
                parameter,
            } => {
                write!(
                    f,
                    "{code}: motion {motion_id:?} curve {parameter:?} keyframe times invalid"
                )
            }
            ParseError::TangentInvalid { context } => {
                write!(f, "{code}: cubic tangent invalid at {context}")
            }
            ParseError::PhysicsOutputConflict { parameter } => {
                write!(
                    f,
                    "{code}: two physics outputs write parameter {parameter:?}"
                )
            }
            ParseError::PhysicsOutputTarget {
                motion_id,
                parameter,
            } => {
                write!(
                    f,
                    "{code}: motion {motion_id:?} curve targets physics output {parameter:?}"
                )
            }
        }
    }
}

impl std::error::Error for ParseError {}
