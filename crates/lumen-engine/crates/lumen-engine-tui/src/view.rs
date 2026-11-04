//! view — the ratatui rendering of the operation
//! core's outcomes.
//!
//! The view holds no workflow logic: it draws the six
//! workflow panels (SPEC section 5.1), the last
//! [`Outcome`] each panel's verb produced, and — on the
//! Stage panel — the termcap halfblock rendering of the
//! last preview frame. Everything it shows came from
//! [`crate::ops`]; the same outcomes render as text in
//! the CLI.

use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Tabs, Wrap};
use ratatui::Frame;

use crate::ops::Outcome;
use crate::termcap::{Capabilities, CellGrid};

/// The six workflow panels, SPEC section 5.1 order.
pub const TABS: [&str; 6] = ["Author", "Stage", "Inspect", "Play", "Gates", "Pack"];

/// Everything the view draws. Front-ends own the
/// state; the view only reads it.
pub struct AppState {
    /// Active panel index into [`TABS`].
    pub tab: usize,
    /// One-line context (project / scene / imports).
    pub header: String,
    /// Detected terminal capabilities (termcap).
    pub caps: Capabilities,
    /// Last outcome per panel, if its verb has run.
    pub outcomes: Vec<Option<Outcome>>,
    /// Last preview frame as a termcap cell grid.
    pub preview: Option<CellGrid>,
    /// Footer status line.
    pub status: String,
}

impl AppState {
    /// A fresh state with no outcomes yet.
    pub fn new(header: String, caps: Capabilities) -> Self {
        AppState {
            tab: 0,
            header,
            caps,
            outcomes: vec![None, None, None, None, None, None],
            preview: None,
            status: "keys: 1-6 panels, Tab next, r run panel verb, q quit".to_string(),
        }
    }
}

/// Convert a termcap cell grid into ratatui text: one
/// `▀` span per cell, foreground = top pixel,
/// background = bottom pixel.
pub fn grid_to_text(grid: &CellGrid) -> Text<'static> {
    let mut lines = Vec::with_capacity(grid.rows);
    for row in 0..grid.rows {
        let mut spans = Vec::with_capacity(grid.cols);
        for col in 0..grid.cols {
            let cell = grid.cells[row * grid.cols + col];
            spans.push(Span::styled(
                "▀",
                Style::default()
                    .fg(Color::Rgb(cell.fg[0], cell.fg[1], cell.fg[2]))
                    .bg(Color::Rgb(cell.bg[0], cell.bg[1], cell.bg[2])),
            ));
        }
        lines.push(Line::from(spans));
    }
    Text::from(lines)
}

/// Draw the whole surface.
pub fn draw(frame: &mut Frame, state: &AppState) {
    let area = frame.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(1),
        ])
        .split(area);
    let header = Paragraph::new(format!(
        "lumen-engine — {} | termcap: {} ({})",
        state.header,
        state.caps.protocol.name(),
        state.caps.evidence
    ))
    .style(Style::default().add_modifier(Modifier::BOLD));
    frame.render_widget(header, rows[0]);
    let tabs = Tabs::new(TABS.iter().map(|t| Line::from(*t)).collect::<Vec<_>>())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("workflows (SPEC 5.1)"),
        )
        .select(state.tab)
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_widget(tabs, rows[1]);
    // Body: outcome text, plus the preview pane on the
    // Stage panel when a preview exists.
    if state.tab == 1 {
        if let Some(grid) = &state.preview {
            let cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(52), Constraint::Percentage(48)])
                .split(rows[2]);
            frame.render_widget(outcome_paragraph(state), cols[0]);
            let preview = Paragraph::new(grid_to_text(grid)).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("preview (staging instrument, not a verdict surface)"),
            );
            frame.render_widget(preview, cols[1]);
            let footer = Paragraph::new(state.status.clone());
            frame.render_widget(footer, rows[3]);
            return;
        }
    }
    frame.render_widget(outcome_paragraph(state), rows[2]);
    let footer = Paragraph::new(state.status.clone());
    frame.render_widget(footer, rows[3]);
}

fn outcome_paragraph(state: &AppState) -> Paragraph<'static> {
    let title = format!("{} — last verb outcome", TABS[state.tab]);
    let text = match &state.outcomes[state.tab] {
        Some(outcome) => {
            let mut lines = vec![format!("== {}", outcome.title)];
            lines.extend(outcome.lines.iter().cloned());
            for artifact in &outcome.artifacts {
                lines.push(format!("artifact: {}", artifact.display()));
            }
            lines.push(format!(
                "verdict: {}",
                if outcome.ok { "PASS" } else { "FAIL" }
            ));
            lines.join("\n")
        }
        None => "no verb run yet on this panel — press r".to_string(),
    };
    Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: false })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::Outcome;
    use crate::termcap::{detect, Cell, CellGrid};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    #[test]
    fn every_panel_draws_its_title_and_outcome() {
        let backend = TestBackend::new(110, 32);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new(
            "project demo".to_string(),
            detect(Some("xterm-kitty"), None),
        );
        for tab in 0..TABS.len() {
            state.tab = tab;
            state.outcomes[tab] = Some(Outcome {
                title: format!("verb {tab}"),
                lines: vec![format!("readout line for panel {tab}")],
                artifacts: Vec::new(),
                ok: true,
            });
            terminal.draw(|f| draw(f, &state)).unwrap();
            let text = buffer_text(&terminal);
            assert!(text.contains(TABS[tab]), "tab {} title missing", TABS[tab]);
            assert!(
                text.contains(&format!("readout line for panel {tab}")),
                "panel {tab} outcome missing"
            );
            assert!(text.contains("verdict: PASS"));
        }
    }

    #[test]
    fn stage_panel_draws_the_preview_grid() {
        let backend = TestBackend::new(110, 32);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::new("project demo".to_string(), detect(None, None));
        state.tab = 1;
        state.preview = Some(CellGrid {
            cols: 2,
            rows: 1,
            cells: vec![
                Cell {
                    fg: [255, 0, 0],
                    bg: [0, 0, 255],
                },
                Cell {
                    fg: [0, 255, 0],
                    bg: [0, 0, 0],
                },
            ],
        });
        terminal.draw(|f| draw(f, &state)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains('▀'));
        assert!(text.contains("not a verdict surface"));
    }
}
