//! Converter registry (format bridges landing in lumen-factory).
//!
//! | Converter | Source → target | Stage |
//! |---|---|---|
//! | `pmx-import/1` | PMX 2.0/2.1 → census record + canonical GLB | Stage 1 (this tree) |
//! | `vmd-import/1` | VMD motion → torogao-motion-clip/1 | Stage 2 scope — not built here |

pub mod pmx;
