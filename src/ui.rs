use chrono::{DateTime, Utc};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;
use tui_term::widget::PseudoTerminal;

use std::collections::HashMap;

use crate::app::{App, Focus, Overlay, PickerState, RunId};
use crate::roster::Row;
use crate::sessions::Agent;
use crate::term::SessionStatus;

/// Fraction of the frame given to the session list while a terminal
/// is attached alongside it.
pub const LIST_PERCENT: u16 = 25;

/// Fraction given to the list when no terminal is attached and the
/// right pane only shows the placeholder hint: browsing sessions is
/// the main activity, so the list gets the room.
pub const EXPANDED_LIST_PERCENT: u16 = 80;

/// Every rect in the frame, computed once: the split `render` draws
/// and the PTY size from `terminal_pane_size` cannot disagree.
struct Panes {
    /// The list+terminal region, used for overlay centering.
    main: Rect,
    /// None when auto-hide has hidden the list.
    list: Option<Rect>,
    terminal: Rect,
    help: Rect,
}

fn panes(frame: Rect, app: &App) -> Panes {
    let [main, help] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(frame);
    if app.list_hidden() {
        return Panes { main, list: None, terminal: main, help };
    }
    let list_percent = if app.attached_run().is_none() {
        EXPANDED_LIST_PERCENT
    } else {
        LIST_PERCENT
    };
    let [list, terminal] = Layout::horizontal([
        Constraint::Percentage(list_percent),
        Constraint::Percentage(100 - list_percent),
    ])
    .areas(main);
    Panes { main, list: Some(list), terminal, help }
}

pub fn render(
    f: &mut Frame,
    app: &App,
    screen: Option<&vt100::Screen>,
    statuses: &HashMap<RunId, SessionStatus>,
) {
    let panes = panes(f.area(), app);
    if let Some(list) = panes.list {
        render_list(f, app, statuses, list);
    }
    render_terminal(f, app, screen, panes.terminal);
    render_help(f, app, panes.help);
    render_overlay(f, app, panes.main);
}

/// The inner size of the terminal pane for a given frame size; PTYs are
/// kept at exactly this size. The interior is derived from a `Block`
/// with the same borders `render_terminal` draws, so the border math
/// can't drift either. None while browsing: with nothing attached the
/// right pane is a placeholder hint, not a terminal, and sizing live
/// detached PTYs to it would rewrap their output.
pub fn terminal_pane_size(frame: Rect, app: &App) -> Option<(u16, u16)> {
    app.attached_run()?;
    let inner = Block::default()
        .borders(Borders::ALL)
        .inner(panes(frame, app).terminal);
    Some((inner.height, inner.width))
}

fn render_list(f: &mut Frame, app: &App, statuses: &HashMap<RunId, SessionStatus>, area: Rect) {
    let focused = app.focus == Focus::List;
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Sessions ")
        .border_style(border_style(focused));

    let items: Vec<ListItem> = app
        .roster()
        .rows()
        .iter()
        .map(|r| ListItem::new(session_line(r, statuses)))
        .collect();
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD));
    let mut state = ListState::default().with_selected(Some(app.roster().selected()));
    f.render_stateful_widget(list, area, &mut state);
}

fn session_line<'a>(row: &'a Row, statuses: &HashMap<RunId, SessionStatus>) -> Line<'a> {
    let (icon, icon_color) = match row.agent() {
        Agent::Claude => ("●", Color::LightMagenta),
        Agent::Codex => ("○", Color::LightCyan),
    };
    let marker = match row.run_id().map(|id| statuses.get(&id)) {
        Some(Some(SessionStatus::Busy)) => "⚡",
        Some(_) => "▶", // running, idle (or status not sampled yet)
        None => " ",
    };
    let project = sanitize(row.cwd().rsplit('/').next().unwrap_or(""));
    Line::from(vec![
        Span::styled(format!("{marker}{icon} "), Style::default().fg(icon_color)),
        Span::raw(sanitize(row.title())),
        Span::styled(
            format!("  {} · {}", project, relative_time(row.timestamp())),
            Style::default().fg(Color::DarkGray),
        ),
    ])
}

/// Transcript-derived strings (titles, cwds) are untrusted; never let
/// control bytes anywhere near the render path.
fn sanitize(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

fn relative_time(timestamp: DateTime<Utc>) -> String {
    let delta = Utc::now() - timestamp;
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
        // Notices can embed transcript-derived cwds; sanitize like
        // every other render of untrusted strings.
        Some(notice) => (sanitize(notice), Style::default().fg(Color::Yellow)),
        None => (
            match app.focus {
                Focus::List => "Enter resume · n new · h auto-hide · Ctrl+K kill · j/k move · q quit",
                Focus::Terminal => "Ctrl+\\ back to list · PgUp/PgDn scrollback",
            }
            .to_string(),
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

    let agent = match picker.agent() {
        Agent::Claude => "claude",
        Agent::Codex => "codex",
    };
    let mut lines = vec![Line::from(vec![
        Span::styled("agent: ", Style::default().fg(Color::DarkGray)),
        Span::styled(agent, Style::default().add_modifier(Modifier::BOLD)),
        Span::styled("  (Tab switches)   path: ", Style::default().fg(Color::DarkGray)),
        Span::raw(picker.input().to_string()),
        Span::styled("▏", Style::default().fg(Color::Yellow)),
    ])];
    for (i, dir) in picker.matches().iter().enumerate() {
        let style = if i == picker.highlighted() {
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
