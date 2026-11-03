//! pmx-import/1 — PMX 2.0/2.1 → census record, then → canonical GLB.
//!
//! Clean-room: written from the published PMX format specification
//! (see `~/workspace/mmd-port-20261102/SPEC.md`). Census first
//! (`census`), bounded retained parse (`parse`), PMX→GLB emission
//! (`glb`), and the MMD mapping profile as data (`profile`).

pub mod census;
pub mod glb;
pub mod parse;
pub mod profile;
pub mod sha256;

pub use census::{Census, census, census_of};
pub use glb::{ConversionReport, ConvertOptions, ResolvedTexture, convert};
pub use parse::{CensusError, PmxModel, parse_model};
