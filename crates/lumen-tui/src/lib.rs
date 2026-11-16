//! `lumen-tui` — the fleet-pattern terminal console for lumen (D-1562).
//!
//! Shape follows DESIGN §D2: the UI programs against a narrow
//! [`facade::LumenFacade`] trait, panels are pure constructors, screens
//! are content models shared by the full-screen and line modes, and the
//! trigger layer (§D4) plus the media ladder rung 1 (§D5) are separate
//! modules with their own bounds and tests.
//!
//! The real facade is an MCP stdio client spawning `lumen-mcp-server`
//! (the server-only binary surface). The `lumen` library crate is not
//! linked: it pulls native inference dependencies (ONNX Runtime, vendored
//! Lua) that a thin operator console must not carry. Filesystem surfaces
//! the MCP server does not expose (identity records, media browsing) are
//! read directly and read-only. This split is recorded in the member
//! report; nothing in the UI pretends a lib call happened.
//!
//! # Named bounds (units in the names)
//!
//! - [`LOG_LINES_MAX`] log lines retained in memory.
//! - [`INPUT_LINE_BYTES_MAX`] bytes per line-mode input line.
//! - [`TOOL_RESULT_CHARS_MAX`] chars of a tool result rendered in a panel.
//! - Media bounds live in [`media`]; trigger bounds in [`triggers`].

#![forbid(unsafe_code)]

pub mod app;
pub mod events;
pub mod facade;
pub mod media;
pub mod panels;
pub mod screens;
pub mod triggers;

/// Member name shown in the Overview screen.
pub const MEMBER_NAME: &str = "lumen";
/// Released lumen version this rung is built against (origin/main).
pub const MEMBER_VERSION: &str = "2.0.2";
/// This crate's own version.
pub const TUI_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Maximum log lines retained (older lines evict from the front).
pub const LOG_LINES_MAX: usize = 500;
/// Maximum bytes accepted for one line-mode input line.
pub const INPUT_LINE_BYTES_MAX: usize = 4096;
/// Maximum chars of one tool result rendered into a panel or line output.
pub const TOOL_RESULT_CHARS_MAX: usize = 4000;
/// Maximum audit lines retained in memory by the trigger engine.
pub const AUDIT_LINES_MAX: usize = 500;

/// Truncate `text` to at most `max_chars` chars on a char boundary,
/// appending a marker when truncation happened. Never panics.
pub fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let kept: String = text.chars().take(max_chars).collect();
    format!("{kept}…[truncated]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_keeps_short_text() {
        assert_eq!(truncate_chars("abc", 10), "abc");
    }

    #[test]
    fn truncate_bounds_long_text_on_char_boundary() {
        let out = truncate_chars("héllo wörld", 5);
        assert!(out.starts_with("héllo"));
        assert!(out.contains("truncated"));
    }
}
