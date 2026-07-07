use chrono::Utc;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;
use tui_term::widget::PseudoTerminal;

use std::collections::HashMap;

use crate::app::{App, Focus, Overlay, PickerState, RunId};
use crate::sessions::{Agent, SessionMeta};
use crate::term::SessionStatus;

/// Fraction of the frame given to the session list.
pub const LIST_PERCENT: u16 = 25;

pub fn render(
    f: &mut Frame,
    app: &App,
    screen: Option<&vt100::Screen>,
    statuses: &HashMap<RunId, SessionStatus>,
) {
    let [main, help] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(f.area());
    let [left, right] = Layout::horizontal([
        Constraint::Percentage(LIST_PERCENT),
        Constraint::Percentage(100 - LIST_PERCENT),
    ])
    .areas(main);

    render_list(f, app, statuses, left);
    render_terminal(f, app, screen, right);
    render_help(f, app, help);
    render_overlay(f, app, main);
}

/// The inner size of the terminal pane for a given frame size; PTYs are
/// kept at exactly this size.
pub fn terminal_pane_size(frame: Rect) -> (u16, u16) {
    let [main, _] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(frame);
    let [_, right] = Layout::horizontal([
        Constraint::Percentage(LIST_PERCENT),
        Constraint::Percentage(100 - LIST_PERCENT),
    ])
    .areas(main);
    (right.height.saturating_sub(2), right.width.saturating_sub(2))
}

fn render_list(f: &mut Frame, app: &App, statuses: &HashMap<RunId, SessionStatus>, area: Rect) {
    let focused = app.focus == Focus::List;
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Sessions ")
        .border_style(border_style(focused));

    let items: Vec<ListItem> = app
        .sessions
        .iter()
        .map(|m| ListItem::new(session_line(app, m, statuses)))
        .collect();
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD));
    let mut state = ListState::default().with_selected(Some(app.selected));
    f.render_stateful_widget(list, area, &mut state);
}

fn session_line<'a>(
    app: &App,
    m: &'a SessionMeta,
    statuses: &HashMap<RunId, SessionStatus>,
) -> Line<'a> {
    let (icon, icon_color) = match m.agent {
        Agent::Claude => ("●", Color::LightMagenta),
        Agent::Codex => ("○", Color::LightCyan),
    };
    let marker = match app.run_id_for(&m.id).map(|id| statuses.get(&id)) {
        Some(Some(SessionStatus::Busy)) => "⚡",
        Some(_) => "▶", // running, idle (or status not sampled yet)
        None => " ",
    };
    let project = sanitize(m.cwd.rsplit('/').next().unwrap_or(""));
    Line::from(vec![
        Span::styled(format!("{marker}{icon} "), Style::default().fg(icon_color)),
        Span::raw(sanitize(&m.title)),
        Span::styled(
            format!("  {} · {}", project, relative_time(m)),
            Style::default().fg(Color::DarkGray),
        ),
    ])
}

/// Transcript-derived strings (titles, cwds) are untrusted; never let
/// control bytes anywhere near the render path.
fn sanitize(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

fn relative_time(m: &SessionMeta) -> String {
    let delta = Utc::now() - m.timestamp;
    if delta.num_days() > 0 {
        format!("{}d", delta.num_days())
    } else if delta.num_hours() > 0 {
        format!("{}h", delta.num_hours())
    } else {
        format!("{}m", delta.num_minutes().max(0))
    }
}

fn render_terminal(f: &mut Frame, app: &App, screen: Option<&vt100::Screen>, area: Rect) {
    let focused = app.focus == Focus::Terminal;
    let title = if app.scroll_offset > 0 {
        format!(" Terminal [scroll -{}] ", app.scroll_offset)
    } else {
        " Terminal ".to_string()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(border_style(focused));

    match screen {
        Some(screen) => {
            let term = PseudoTerminal::new(screen).block(block);
            f.render_widget(term, area);
        }
        None => {
            let hint = Paragraph::new(vec![
                Line::raw(""),
                Line::raw("  Enter  resume selected session"),
                Line::raw("  n      launch a new session"),
                Line::raw("  Ctrl+\\ toggle list/terminal focus"),
            ])
            .style(Style::default().fg(Color::DarkGray))
            .block(block);
            f.render_widget(hint, area);
        }
    }
}

fn render_help(f: &mut Frame, app: &App, area: Rect) {
    let (text, style) = match &app.notice {
        Some(notice) => (notice.as_str(), Style::default().fg(Color::Yellow)),
        None => (
            match app.focus {
                Focus::List => "Enter resume · n new · Ctrl+K kill · j/k move · q quit",
                Focus::Terminal => "Ctrl+\\ back to list · PgUp/PgDn scrollback",
            },
            Style::default().fg(Color::DarkGray),
        ),
    };
    f.render_widget(Paragraph::new(text).style(style), area);
}

fn render_overlay(f: &mut Frame, app: &App, area: Rect) {
    match &app.overlay {
        Overlay::None => {}
        Overlay::ConfirmQuit => {
            confirm_box(f, area, "Quit?", "Running sessions will be terminated. y/N")
        }
        Overlay::ConfirmKill { .. } => {
            confirm_box(f, area, "Kill session?", "The agent process will be killed. y/N")
        }
        Overlay::LaunchPicker(picker) => render_picker(f, picker, area),
    }
}

fn confirm_box(f: &mut Frame, area: Rect, title: &str, body: &str) {
    let w = (body.len() as u16 + 4).min(area.width);
    let popup = centered(area, w, 3);
    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(body).block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {title} "))
                .border_style(Style::default().fg(Color::Yellow)),
        ),
        popup,
    );
}

fn render_picker(f: &mut Frame, picker: &PickerState, area: Rect) {
    // max-then-min: never feed clamp a min above its max (that panics
    // on terminals shorter than 7 rows).
    let max_h = area.height.saturating_sub(2).max(1);
    let h = (picker.matches().len() as u16).saturating_add(4).max(5).min(max_h);
    let popup = centered(area, (area.width * 6 / 10).max(40), h);
    f.render_widget(Clear, popup);

    let agent = match picker.agent {
        Agent::Claude => "claude",
        Agent::Codex => "codex",
    };
    let mut lines = vec![Line::from(vec![
        Span::styled("agent: ", Style::default().fg(Color::DarkGray)),
        Span::styled(agent, Style::default().add_modifier(Modifier::BOLD)),
        Span::styled("  (Tab switches)   path: ", Style::default().fg(Color::DarkGray)),
        Span::raw(picker.input.clone()),
        Span::styled("▏", Style::default().fg(Color::Yellow)),
    ])];
    for (i, dir) in picker.matches().iter().enumerate() {
        let style = if i == picker.highlighted {
            Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        lines.push(Line::styled(format!("  {}", sanitize(dir)), style));
    }
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" New session ")
                .border_style(Style::default().fg(Color::Yellow)),
        ),
        popup,
    );
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

fn border_style(focused: bool) -> Style {
    if focused {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}
