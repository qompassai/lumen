//! lumen-engine-tui: the complete-TUI operator surface
//! for the lumen engine (D-895 Stage 2, SPEC section 5).
//!
//! One operation core ([`ops`]) is the single
//! implementation of every workflow verb; two views
//! drive it — `lumen-engine-cli` for headless/agent use
//! and `lumen-engine-tui` for the Owner's terminal —
//! with identical semantics (SPEC section 5.3). The
//! terminal capability/image layer ([`termcap`]) is a
//! separable module (D-902); the ratatui rendering
//! lives in [`view`].

#![forbid(unsafe_code)]

pub mod ops;
pub mod termcap;
pub mod view;
