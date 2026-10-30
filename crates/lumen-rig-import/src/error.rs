//! Typed conversion errors.
//!
//! The converter fails closed: every malformed input, unverifiable
//! layout, unrepresentable construct, and license refusal is a
//! distinct typed error. Nothing is silently dropped, clamped, or
//! substituted; constructs with no native counterpart travel through
//! the conversion report, never through a silent code path.

use std::fmt;
use std::path::PathBuf;

/// Maximum bytes of an input string quoted inside an error.
const ERROR_QUOTE_BYTES_MAX: usize = 96;

/// Truncate a string to at most `ERROR_QUOTE_BYTES_MAX` bytes on a
/// char boundary, so attacker-sized input cannot bloat error values.
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

/// A conversion failure. Conversion stops at the first of these.
#[derive(Debug)]
pub enum ConvertError {
    /// The command line did not match the expected shape.
    Cli { message: String },
    /// A required file could not be read.
    Io { path: PathBuf, message: String },
    /// The license record is missing, malformed, or incomplete.
    /// The gate fails closed: an unreadable record is a refusal.
    LicenseUnreadable { message: String },
    /// The license record parses but does not grant game use and
    /// conversion. Conversion is refused before any model file is
    /// read beyond the license itself.
    LicenseDenied { reason: String },
    /// A Cubism JSON sidecar (model3/cdi3/physics3/motion3/exp3)
    /// failed to parse or violated the expected schema.
    Json { file: String, message: String },
    /// The moc3 byte stream is structurally broken: bad magic,
    /// out-of-range offset, truncated section, invalid scalar.
    Moc3 { message: String },
    /// The moc3 version byte names a format revision this converter
    /// does not carry a verified layout for.
    UnsupportedMoc3Version { found: u8 },
    /// A parsed moc3 failed the layout self-verification battery
    /// (count identities, binding products). This is the guard that
    /// keeps an unverified per-version layout from degrading
    /// silently: a wrong layout fails here, loudly.
    LayoutCheck { message: String },
    /// A source construct has no faithful native representation
    /// (mirrored rotation keyforms, inverted clipping masks in use,
    /// physics outputs that would break the one-writer rule, ...).
    Mapping { message: String },
    /// The converted model asset failed `lumen-rig-core`
    /// validation. Carries the native `LRF-xxx` code verbatim.
    NativeValidation { code: &'static str, message: String },
    /// A converted motion or expression asset failed the SPEC
    /// section 4 / 9 self-checks the converter performs in place of
    /// the (not yet landed) native motion validator.
    MotionValidation { message: String },
    /// An export (native -> moc3) failure: an unrepresentable
    /// construct the mapping refuses rather than approximates
    /// (MAPPING.md D-1/D-3/D-4 class decisions), a malformed
    /// native input the writer cannot encode, or an unimplemented
    /// moc3 version. Export fails closed exactly like conversion:
    /// no partial file is ever produced.
    Export { message: String },
    /// An INP (.inx / Inochi2D puppet) ingest failure: a broken
    /// container, a payload that violates the INP schema this
    /// stage implements, or a source construct the INP -> native
    /// mapping refuses rather than approximates (MAPPING.md
    /// sections 3.4 / 6.3 / 9.2 / 14 dispositions). Ingest fails
    /// closed exactly like conversion: no partial model is ever
    /// produced.
    Inp { message: String },
}

impl fmt::Display for ConvertError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConvertError::Cli { message } => write!(f, "usage: {message}"),
            ConvertError::Io { path, message } => {
                write!(f, "cannot read {}: {message}", path.display())
            }
            ConvertError::LicenseUnreadable { message } => {
                write!(f, "license record unusable, refusing: {message}")
            }
            ConvertError::LicenseDenied { reason } => {
                write!(f, "license gate refusal: {reason}")
            }
            ConvertError::Json { file, message } => {
                write!(f, "{file}: JSON error: {message}")
            }
            ConvertError::Moc3 { message } => write!(f, "moc3: {message}"),
            ConvertError::UnsupportedMoc3Version { found } => {
                write!(f, "moc3: unsupported version byte {found}")
            }
            ConvertError::LayoutCheck { message } => {
                write!(f, "moc3 layout self-check failed: {message}")
            }
            ConvertError::Mapping { message } => write!(f, "conversion: {message}"),
            ConvertError::NativeValidation { code, message } => {
                write!(f, "native validation {code}: {message}")
            }
            ConvertError::MotionValidation { message } => {
                write!(f, "motion/expression validation: {message}")
            }
            ConvertError::Export { message } => write!(f, "export: {message}"),
            ConvertError::Inp { message } => write!(f, "inp: {message}"),
        }
    }
}

impl std::error::Error for ConvertError {}

impl From<lumen_rig_core::ParseError> for ConvertError {
    fn from(error: lumen_rig_core::ParseError) -> Self {
        ConvertError::NativeValidation {
            code: error.code(),
            message: error.to_string(),
        }
    }
}
