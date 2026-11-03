//! gltf-import/1 — Stage 1 GLB → `torogao-mmd-ingest/1` record
//! (SPEC §9 Route A, Owner commission D-633).
//!
//! Route A keeps Torogao's model format singular: PMX reaches
//! Torogao as PMX → GLB → Torogao records, with the GLB as the
//! auditable midpoint. This module is the second hop. It reads a
//! pmx-import/1 GLB through the crate's existing GLB reader
//! (`crate::gltf::parse_glb_parts`), extracts the `pmx_provenance`
//! extras and the per-morph `Fac_` mapping, applies the
//! `profile.rs` mapping tables (SPEC §6), and emits the
//! deterministic ingest record Torogao's `mmd_ingest` validator
//! consumes. Physics follows SPEC §6.4 with zero silent drops:
//! every census-counted feature is either mapped or carried in
//! the dropped ledger, and the record's accounting rows prove it.
//!
//! The same module renders `mmd-profile/1.ron` from the profile
//! tables (`--emit-profile` on the CLI), so the Torogao-side data
//! file is machine-derived from the single table source, never
//! hand-duplicated.

pub mod ingest;

pub use ingest::{IngestRecord, emit_profile_ron, ingest_glb};
