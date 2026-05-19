use std::{
    io::{self, Stdout},
    time::Duration,
};

use crossterm::{
    cursor::{Hide, Show},
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Gauge, Paragraph, Wrap},
};

const SPINNER: [&str; 4] = ["|", "/", "-", "\\"];

#[derive(Clone, Copy)]
pub enum Tone {
    Info,
    Good,
    Warn,
    Error,
}

impl Tone {
    fn color(self) -> Color {
        match self {
            Self::Info => Color::Cyan,
            Self::Good => Color::Green,
            Self::Warn => Color::Yellow,
            Self::Error => Color::Red,
        }
    }
}

pub enum TuiAction {
    Cancel,
    Confirm,
    Decline,
}

pub struct TuiTerminal {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    restored: bool,
}

pub struct BusyScreen<'a> {
    pub title: &'a str,
    pub message: &'a str,
    pub tone: Tone,
    pub tick: u64,
}

pub struct AuthScreen<'a> {
    pub user: &'a str,
    pub status: &'a str,
    pub status_tone: Tone,
    pub elapsed: Duration,
    pub tick: u64,
}

pub struct EnrollScreen<'a> {
    pub user: &'a str,
    pub face: &'a str,
    pub is_refine: bool,
    pub prompt: &'a str,
    pub capture: &'a str,
    pub capture_tone: Tone,
    pub progress: u32,
    pub max: u32,
    pub time_remaining: Option<f64>,
    pub confirm_cancel: bool,
    pub tick: u64,
}

impl TuiTerminal {
    pub fn new() -> anyhow::Result<Self> {
        enable_raw_mode()?;

        let mut stdout = io::stdout();
        if let Err(err) = execute!(stdout, EnterAlternateScreen, Hide) {
            let _ = disable_raw_mode();
            return Err(err.into());
        }

        let backend = CrosstermBackend::new(stdout);
        let mut terminal = match Terminal::new(backend) {
            Ok(terminal) => terminal,
            Err(err) => {
                let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
                let _ = disable_raw_mode();
                return Err(err.into());
            }
        };
        if let Err(err) = terminal.clear() {
            let _ = execute!(terminal.backend_mut(), Show, LeaveAlternateScreen);
            let _ = disable_raw_mode();
            return Err(err.into());
        }

        Ok(Self {
            terminal,
            restored: false,
        })
    }

    pub fn draw_busy(&mut self, screen: &BusyScreen<'_>) -> anyhow::Result<()> {
        self.terminal.draw(|frame| render_busy(frame, screen))?;
        Ok(())
    }

    pub fn draw_auth(&mut self, screen: &AuthScreen<'_>) -> anyhow::Result<()> {
        self.terminal.draw(|frame| render_auth(frame, screen))?;
        Ok(())
    }

    pub fn draw_enroll(&mut self, screen: &EnrollScreen<'_>) -> anyhow::Result<()> {
        self.terminal.draw(|frame| render_enroll(frame, screen))?;
        Ok(())
    }

    pub fn restore(&mut self) -> io::Result<()> {
        if self.restored {
            return Ok(());
        }

        let show_result = self.terminal.show_cursor();
        let leave_result = execute!(self.terminal.backend_mut(), Show, LeaveAlternateScreen);
        let raw_result = disable_raw_mode();
        self.restored = true;

        show_result?;
        leave_result?;
        raw_result?;
        Ok(())
    }
}

impl Drop for TuiTerminal {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

pub fn poll_action() -> anyhow::Result<Option<TuiAction>> {
    while event::poll(Duration::from_millis(0))? {
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind == KeyEventKind::Release {
            continue;
        }

        let is_ctrl_c = key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'));
        if is_ctrl_c || matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) {
            return Ok(Some(TuiAction::Cancel));
        }

        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => return Ok(Some(TuiAction::Confirm)),
            KeyCode::Char('n') | KeyCode::Char('N') => return Ok(Some(TuiAction::Decline)),
            _ => {}
        }
    }

    Ok(None)
}

fn render_busy(frame: &mut Frame<'_>, screen: &BusyScreen<'_>) {
    let area = centered_rect(frame.area(), 68, 11);
    let block = Block::default()
        .title(Line::from(vec![Span::styled(
            format!(" {} ", screen.title),
            Style::default()
                .fg(screen.tone.color())
                .add_modifier(Modifier::BOLD),
        )]))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(screen.tone.color()));
    frame.render_widget(block, area);

    let inner = inset(area, 3, 2);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(2),
            Constraint::Min(1),
        ])
        .split(inner);

    let spinner = SPINNER[screen.tick as usize % SPINNER.len()];
    let message = Paragraph::new(vec![
        Line::from(Span::styled(
            spinner,
            Style::default()
                .fg(screen.tone.color())
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            screen.message,
            Style::default().fg(Color::White),
        )),
    ])
    .alignment(Alignment::Center)
    .wrap(Wrap { trim: true });
    frame.render_widget(message, chunks[0]);

    render_pulse(frame, chunks[1], screen.tick, screen.tone, "working");

    let controls = Paragraph::new("Ctrl+C or q to cancel")
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center);
    frame.render_widget(controls, chunks[2]);
}

fn render_auth(frame: &mut Frame<'_>, screen: &AuthScreen<'_>) {
    let area = centered_rect(frame.area(), 76, 16);
    let block = Block::default()
        .title(Line::from(vec![Span::styled(
            " Gaze Auth ",
            Style::default()
                .fg(screen.status_tone.color())
                .add_modifier(Modifier::BOLD),
        )]))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(screen.status_tone.color()));
    frame.render_widget(block, area);

    let inner = inset(area, 3, 1);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(5),
            Constraint::Length(3),
            Constraint::Min(1),
        ])
        .split(inner);

    let header = Paragraph::new(Line::from(vec![
        Span::styled("User ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            screen.user,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("   "),
        Span::styled(
            format!("{}ms", screen.elapsed.as_millis()),
            Style::default().fg(Color::DarkGray),
        ),
    ]))
    .alignment(Alignment::Center);
    frame.render_widget(header, chunks[0]);

    let spinner = SPINNER[screen.tick as usize % SPINNER.len()];
    let status = Paragraph::new(vec![
        Line::from(Span::styled(
            "Looking for a matching face",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                spinner,
                Style::default()
                    .fg(screen.status_tone.color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                screen.status,
                Style::default().fg(screen.status_tone.color()),
            ),
        ]),
    ])
    .block(Block::default().borders(Borders::ALL).title("Camera"))
    .alignment(Alignment::Center)
    .wrap(Wrap { trim: true });
    frame.render_widget(status, chunks[1]);

    render_pulse(
        frame,
        chunks[2],
        screen.tick,
        screen.status_tone,
        "scanning",
    );

    let controls = Paragraph::new("Ctrl+C or q to cancel")
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center);
    frame.render_widget(controls, chunks[3]);
}

fn render_enroll(frame: &mut Frame<'_>, screen: &EnrollScreen<'_>) {
    let area = centered_rect(frame.area(), 82, 20);
    let accent = if screen.confirm_cancel {
        Tone::Warn.color()
    } else {
        screen.capture_tone.color()
    };
    let title = if screen.is_refine {
        " Gaze Refinement "
    } else {
        " Gaze Enrollment "
    };
    let block = Block::default()
        .title(Line::from(vec![Span::styled(
            title,
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        )]))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(accent));
    frame.render_widget(block, area);

    let inner = inset(area, 3, 1);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(5),
            Constraint::Length(4),
            Constraint::Length(3),
            Constraint::Min(1),
        ])
        .split(inner);

    let header = Paragraph::new(Line::from(vec![
        Span::styled("User ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            screen.user,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" / Face ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            screen.face,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
    ]))
    .alignment(Alignment::Center);
    frame.render_widget(header, chunks[0]);

    let remaining = screen
        .time_remaining
        .filter(|seconds| *seconds > 0.0)
        .map(|seconds| format!(" ({seconds:.1}s)"))
        .unwrap_or_default();
    let prompt = Paragraph::new(vec![
        Line::from(Span::styled(
            format!("{}{}", screen.prompt, remaining),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("Position your face as prompted. Capture is automatic when centered."),
    ])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title("Capture prompt"),
    )
    .alignment(Alignment::Center)
    .wrap(Wrap { trim: true });
    frame.render_widget(prompt, chunks[1]);

    let spinner = SPINNER[screen.tick as usize % SPINNER.len()];
    let capture = Paragraph::new(vec![
        Line::from(vec![
            Span::styled(
                spinner,
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                screen.capture,
                Style::default().fg(screen.capture_tone.color()),
            ),
        ]),
        Line::from("Keep your head inside the camera frame."),
    ])
    .block(Block::default().borders(Borders::ALL).title("Camera"))
    .alignment(Alignment::Center)
    .wrap(Wrap { trim: true });
    frame.render_widget(capture, chunks[2]);

    let max = screen.max.max(1);
    let progress = screen.progress.min(max);
    let ratio = f64::from(progress) / f64::from(max);
    let gauge = Gauge::default()
        .block(Block::default().borders(Borders::ALL).title("Progress"))
        .gauge_style(Style::default().fg(Color::Cyan).bg(Color::Black))
        .ratio(ratio)
        .label(format!("{progress}/{max}"));
    frame.render_widget(gauge, chunks[3]);

    let controls = Paragraph::new("Ctrl+C or q to cancel")
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center);
    frame.render_widget(controls, chunks[4]);

    if screen.confirm_cancel {
        render_cancel_popup(frame, frame.area());
    }
}

fn render_pulse(frame: &mut Frame<'_>, area: Rect, tick: u64, tone: Tone, label: &str) {
    let phase = (tick % 24) as f64 / 23.0;
    let ratio = if phase <= 0.5 {
        phase * 2.0
    } else {
        (1.0 - phase) * 2.0
    };
    let gauge = Gauge::default()
        .gauge_style(Style::default().fg(tone.color()).bg(Color::Black))
        .ratio(ratio.max(0.08))
        .label(label);
    frame.render_widget(gauge, area);
}

fn render_cancel_popup(frame: &mut Frame<'_>, area: Rect) {
    let popup = centered_rect(area, 54, 7);
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .title(" Cancel Enrollment ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));
    frame.render_widget(block, popup);

    let inner = inset(popup, 2, 1);
    let message = Paragraph::new(vec![
        Line::from("Discard captures from this session?"),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "Y",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::raw(": discard    "),
            Span::styled(
                "N/Esc",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": resume"),
        ]),
    ])
    .alignment(Alignment::Center)
    .wrap(Wrap { trim: true });
    frame.render_widget(message, inner);
}

fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

fn inset(area: Rect, horizontal: u16, vertical: u16) -> Rect {
    Rect {
        x: area.x.saturating_add(horizontal),
        y: area.y.saturating_add(vertical),
        width: area.width.saturating_sub(horizontal.saturating_mul(2)),
        height: area.height.saturating_sub(vertical.saturating_mul(2)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tone_maps_to_expected_terminal_colors() {
        assert_eq!(Tone::Info.color(), Color::Cyan);
        assert_eq!(Tone::Good.color(), Color::Green);
        assert_eq!(Tone::Warn.color(), Color::Yellow);
        assert_eq!(Tone::Error.color(), Color::Red);
    }

    #[test]
    fn centered_rect_centers_and_clamps_to_parent_area() {
        let area = Rect::new(10, 20, 100, 40);
        assert_eq!(centered_rect(area, 50, 10), Rect::new(35, 35, 50, 10));
        assert_eq!(centered_rect(area, 200, 80), area);
    }

    #[test]
    fn inset_offsets_origin_and_saturates_size() {
        let area = Rect::new(5, 7, 20, 10);
        assert_eq!(inset(area, 3, 2), Rect::new(8, 9, 14, 6));
        assert_eq!(inset(area, 20, 20), Rect::new(25, 27, 0, 0));
    }
}
