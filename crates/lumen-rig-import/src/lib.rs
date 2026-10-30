//! lumen-rig-import — the deterministic Cubism -> lumen-rig
//! converter (rig runtime stage R5a).
//!
//! Import-time conversion only: this crate reads a Cubism model
//! directory (a `.moc3` plus its model3/cdi3/physics3/motion3/
//! exp3 JSON sidecars), converts it into native `lumen-rig-core`
//! assets, and writes a conversion report accounting for every
//! source construct that was approximated or dropped. The shipped
//! game never parses moc3; it loads the native assets produced
//! here.
//!
//! The outbound direction (exporter R5c) lives in
//! [`crate::convert::outbound`] and [`crate::moc3_write`]: a
//! validated native model is written back out as a `.moc3`
//! (V3_0) with an export report under the same loud-error
//! discipline — typed refusals instead of silent degradation.
//!
//! The moc3 parser is written from scratch from the community
//! OpenL2D format specification (no official public spec exists),
//! with the layout verified byte-for-byte against official free
//! sample models and guarded at runtime by a layout
//! self-verification battery. No Live2D SDK code, no Cubism Core
//! binary, and no output of Live2D software is part of this
//! crate. Conversion is gated on a per-model license record that
//! must explicitly grant game use and conversion; the gate fails
//! closed.
//!
//! Contract summary:
//! - Conversion is a pure function of the input bytes: the same
//!   model directory and license record produce byte-identical
//!   output assets and report.
//! - Every invalid input, unverifiable layout, unrepresentable
//!   construct, and license refusal fails with a typed
//!   [`ConvertError`]. Nothing is silently degraded.

#![forbid(unsafe_code)]

pub mod convert;
pub mod cubism;
pub mod error;
pub mod license;
pub mod moc3;
pub mod moc3_write;
pub mod native;

pub use convert::inp::{InpOutcome, InpReport, convert_inp};
pub use convert::outbound::{ExportOutcome, ExportReport, export_model};
pub use convert::{ConversionInputs, ConversionOutcome, ConversionReport, convert};
pub use error::ConvertError;
pub use license::LicenseRecord;
pub use moc3_write::{CanvasSpec, Moc3Version};
