//! Pure ratatui widget constructors (DESIGN §D2).
//!
//! A panel is a function from content lines + area to a widget; tests
//! render through `TestBackend` and read the buffer back. All text is
//! truncated to named bounds before it reaches a widget.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::screens::Screen;
use crate::truncate_chars;

/// Maximum chars in one rendered panel line.
pub const PANEL_LINE_CHARS_MAX: usize = 200;
/// Maximum lines rendered in one panel.
pub const PANEL_LINES_MAX: usize = 200;

/// The header line: member, screen tabs, mode.
pub fn header_lines(active: Screen, mode: &str) -> Vec<String> {
    let tabs: Vec<String> = Screen::ALL
        .iter()
        .map(|s| {
            if *s == active {
                format!("[{}]", s.name())
            } else {
                s.name().to_string()
            }
        })
        .collect();
    vec![
        format!(
            "lumen-tui {} — lumen {} (released origin/main)",
            crate::TUI_VERSION,
            crate::MEMBER_VERSION
        ),
        format!("{}   mode: {mode}", tabs.join("  ")),
    ]
}

/// Build a bordered paragraph panel from content lines, truncated to
/// [`PANEL_LINE_CHARS_MAX`] per line and [`PANEL_LINES_MAX`] lines.
pub fn text_panel(title: &str, lines: &[String]) -> Paragraph<'static> {
    let rendered: Vec<Line<'static>> = lines
        .iter()
        .take(PANEL_LINES_MAX)
        .map(|l| Line::from(Span::raw(truncate_chars(l, PANEL_LINE_CHARS_MAX))))
        .collect();
    Paragraph::new(rendered)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title.to_string()),
        )
        .wrap(Wrap { trim: false })
}

/// Full-screen layout helper: header (3 rows) over the active panel.
pub fn panel_area(area: Rect) -> Rect {
    Rect {
        x: area.x,
        y: area.y.saturating_add(3),
        width: area.width,
        height: area.height.saturating_sub(3),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render_to_text(lines: &[String]) -> String {
        let backend = TestBackend::new(60, 12);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                let area = frame.area();
                frame.render_widget(text_panel("Test", lines), area);
            })
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();
        let mut text = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                text.push_str(buffer[(x, y)].symbol());
            }
            text.push('\n');
        }
        text
    }

    #[test]
    fn panel_renders_content_lines() {
        let text = render_to_text(&["hello lumen".to_string(), "second line".to_string()]);
        assert!(text.contains("hello lumen"));
        assert!(text.contains("second line"));
    }

    #[test]
    fn panel_truncates_oversized_lines_without_panic() {
        let huge = "x".repeat(100_000);
        let text = render_to_text(&[huge]);
        assert!(text.contains("truncated"));
    }

    #[test]
    fn header_marks_the_active_screen() {
        let lines = header_lines(Screen::Media, "plan");
        assert!(lines[1].contains("[Media]"));
        assert!(lines[1].contains("mode: plan"));
        assert!(lines[0].contains("2.0.2"));
    }
}
