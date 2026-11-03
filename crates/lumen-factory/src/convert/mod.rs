//! Converter registry (format bridges landing in lumen-factory).
//!
//! | Converter | Source → target | Stage |
//! |---|---|---|
//! | `pmx-import/1` | PMX 2.0/2.1 → census record + canonical GLB | Stage 1 (this tree) |
//! | `gltf-import/1` | Stage 1 GLB → `torogao-mmd-ingest/1` record | Stage 2 (Route A, D-633) |
//! | `vmd-import/1` | VMD motion → torogao-motion-clip/1 | Stage 2 remainder — not built here |

pub mod gltf;
pub mod pmx;
