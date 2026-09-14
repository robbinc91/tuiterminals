//! Rendering: pane layout and drawing each pane's cell grid into a Frame.

use portable_pty::ExitStatus;

use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Rgb};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color as RatColor, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::app::{App, Pane, PaneStatus};
use crate::config::Colors;
use crate::resources::Resources;
use crate::ui::Modal;

/// Number of rows the top resource monitor bar occupies.
pub const RESOURCE_BAR_ROWS: u16 = 2;

/// The region below the resource bar — where the panes live. This is the
/// single source of truth for pane placement and is reused in `main.rs`.
pub fn pane_area(area: Rect) -> Rect {
    Rect::new(
        area.x,
        area.y + RESOURCE_BAR_ROWS,
        area.width,
        area.height.saturating_sub(RESOURCE_BAR_ROWS),
    )
}

/// Outer rects for `count` panes: 1 fills the area, 2 splits horizontally,
/// 3–4 form a 2×2 grid.
pub fn layout_panes(area: Rect, count: usize) -> Vec<Rect> {
    match count {
        0 => vec![],
        1 => vec![area],
        2 => {
            let left_w = area.width / 2;
            vec![
                Rect::new(area.x, area.y, left_w, area.height),
                Rect::new(area.x + left_w, area.y, area.width - left_w, area.height),
            ]
        }
        _ => {
            let w = area.width / 2;
            let h = area.height / 2;
            vec![
                Rect::new(area.x, area.y, w, h),
                Rect::new(area.x + w, area.y, area.width - w, h),
                Rect::new(area.x, area.y + h, w, area.height - h),
                Rect::new(area.x + w, area.y + h, area.width - w, area.height - h),
            ]
        }
    }
}

/// The region inside a pane's 1-cell border.
pub fn inner_area(rect: Rect) -> Rect {
    Rect::new(
        rect.x,
        rect.y,
        rect.width.saturating_sub(2),
        rect.height.saturating_sub(2),
    )
}

/// Draw every pane: a border (highlighted for the active pane) and either
/// the pane's terminal grid or a "process exited" message. The panes occupy the
/// region below the top resource bar ([`pane_area`]); the bar is drawn after
/// the panes, and an open modal's prompt is drawn last, pinned to the bottom.
pub fn draw_panes(
    frame: &mut Frame,
    app: &App,
    colors: &Colors,
    res: &Resources,
    modal: &Modal,
) {
    let area = frame.area();
    let areas = layout_panes(pane_area(area), app.panes.len());

    // Black background under everything.
    frame.buffer_mut().set_style(area, Style::default().bg(RatColor::Black));

    // Borders (active pane highlighted), each carrying a status dot that
    // reads the pane's activity state at a glance.
    for (i, (pane, rect)) in app.panes.iter().zip(&areas).enumerate() {
        let border = if i == app.active {
            Style::default().fg(colors.active_border.to_ratatui())
        } else {
            Style::default().fg(colors.inactive_border.to_ratatui())
        };
        let (dot, label, dot_fg) = match pane.status() {
            PaneStatus::Working => ("●", "working", RatColor::Green),
            PaneStatus::Idle => ("●", "idle", RatColor::Yellow),
            PaneStatus::Dead => ("✕", "dead", RatColor::DarkGray),
        };
        let mut title = vec![
            Span::styled(format!("{dot} "), Style::default().fg(dot_fg)),
            Span::raw(label),
        ];
        if let Some(name) = &pane.agent_name {
            title.push(Span::raw(format!(" · {name}")));
        }
        if !pane.shared {
            title.push(Span::raw(" (private)"));
        }
        let title = Line::from(title);
        frame.render_widget(
            Block::bordered()
                .border_style(border)
                .title(title),
            *rect,
        );
    }

    // Pane contents.
    let buffer = frame.buffer_mut();
    for (i, (pane, rect)) in app.panes.iter().zip(&areas).enumerate() {
        let inner = inner_area(*rect);
        if pane.alive {
            draw_term(buffer, pane, inner, i == app.active);
        } else {
            // Keep the frozen last screen (often the very reason the process
            // died) and mark the bottom row with the exit status.
            draw_term(buffer, pane, inner, false);
            draw_dead(buffer, inner, &pane.exit_status);
        }
    }

    // The resource monitor bar, pinned to the top two rows.
    draw_resource_bar(frame, res, colors);

    // The modal prompt, if one is open, pinned to the bottom of the frame.
    draw_modal(frame, app, modal);
}

/// Render the open modal's prompt line(s) in a black-background bar pinned to
/// the bottom of the frame. Does nothing when no modal is open.
fn draw_modal(frame: &mut Frame, app: &App, modal: &Modal) {
    let lines = modal.prompt(app, &app.agents);
    if lines.is_empty() {
        return;
    }
    let area = frame.area();
    let rows = lines.len().min(area.height as usize) as u16;
    let y = area.y + area.height - rows;
    let bar = Paragraph::new(lines).style(Style::default().bg(RatColor::Black));
    frame.render_widget(bar, Rect::new(area.x, y, area.width, rows));
}

/// Render the two-row resource monitor into the top [`RESOURCE_BAR_ROWS`] of
/// the frame. Row 0 is the whole machine (`SYS`), row 1 is this process
/// (`APP`). The `SYS`/`APP` labels use the active-border color; the values use
/// the default foreground. A TUI uses no GPU memory, so the app's VRAM reads
/// `n/a`.
pub fn draw_resource_bar(frame: &mut Frame, res: &Resources, colors: &Colors) {
    let area = frame.area();
    if area.height < RESOURCE_BAR_ROWS {
        return;
    }

    let label = Style::default().fg(colors.active_border.to_ratatui());
    let value = Style::default();

    let sys_row = Line::from(vec![
        Span::styled("SYS  ", label),
        Span::styled(format!("CPU {:>3.0}%  ", res.sys_cpu), value),
        Span::styled(
            format!("RAM {}  ", format_ram(res.sys_ram_used, res.sys_ram_total)),
            value,
        ),
        Span::styled(
            format!("VRAM {}", format_vram(res.sys_vram_used, res.sys_vram_total)),
            value,
        ),
    ]);
    let app_row = Line::from(vec![
        Span::styled("APP  ", label),
        Span::styled(format!("CPU {:>3.0}%  ", res.app_cpu), value),
        Span::styled(format!("RAM {}  ", format_bytes_gb(res.app_ram)), value),
        Span::styled("VRAM n/a".to_string(), value),
    ]);

    let bar =
        Paragraph::new(vec![sys_row, app_row]).style(Style::default().bg(RatColor::Black));
    frame.render_widget(bar, Rect::new(area.x, area.y, area.width, RESOURCE_BAR_ROWS));
}

/// `used/total` in GB (one decimal) for system RAM.
fn format_ram(used: u64, total: u64) -> String {
    let gb = 1024.0 * 1024.0 * 1024.0;
    format!("{:.1}/{:.1} GB", used as f64 / gb, total as f64 / gb)
}

/// A single value in GB (two decimals — this app's own footprint is small).
fn format_bytes_gb(bytes: u64) -> String {
    let gb = 1024.0 * 1024.0 * 1024.0;
    format!("{:.2} GB", bytes as f64 / gb)
}

/// `used/total` VRAM in GB, or `n/a` when the value is unavailable.
fn format_vram(used: Option<u64>, total: Option<u64>) -> String {
    match (used, total) {
        (Some(u), Some(t)) => {
            let gb = 1024.0 * 1024.0 * 1024.0;
            format!("{:.1}/{:.1} GB", u as f64 / gb, t as f64 / gb)
        }
        _ => "n/a".to_string(),
    }
}

/// Copy one pane's visible grid into `inner`, plus the cursor when active.
fn draw_term(buffer: &mut Buffer, pane: &Pane, inner: Rect, active: bool) {
    let content = pane.term.renderable_content();
    let colors = content.colors;
    let default_fg = colors[256]; // Foreground
    let default_bg = colors[257]; // Background

    for indexed in content.display_iter {
        let row = indexed.point.line.0 as usize;
        let col = indexed.point.column.0;
        if row >= inner.height as usize || col >= inner.width as usize {
            continue;
        }

        let cell = &indexed.cell;
        let ch = if cell
            .flags
            .contains(Flags::HIDDEN | Flags::WIDE_CHAR_SPACER)
        {
            ' '
        } else {
            cell.c
        };

        let fg = resolve_color(&cell.fg, default_fg);
        let bg = resolve_color(&cell.bg, default_bg);
        let style = apply_flags(Style::default().fg(fg).bg(bg), cell.flags);

        let target = &mut buffer[(inner.x + col as u16, inner.y + row as u16)];
        target.set_symbol(&ch.to_string());
        target.set_style(style);
    }

    if active {
        let cursor = &content.cursor;
        if cursor.shape != CursorShape::Hidden {
            let row = cursor.point.line.0 as usize;
            let col = cursor.point.column.0;
            if row < inner.height as usize && col < inner.width as usize {
                let cell = &mut buffer[(inner.x + col as u16, inner.y + row as u16)];
                match cursor.shape {
                    CursorShape::Block => {
                        cell.set_style(Style::default().add_modifier(Modifier::REVERSED));
                    }
                    CursorShape::Underline => {
                        cell.set_style(Style::default().add_modifier(Modifier::UNDERLINED));
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Mark a dead pane's bottom row with its exit status. The pane's last screen
/// is *not* wiped — `draw_panes` draws the frozen grid first, so the process's
/// final output (often the very reason it died) stays visible above this notice.
/// Without that, an agent that dies on startup shows only a bare "dead" and the
/// user can't tell why.
fn draw_dead(buffer: &mut Buffer, rect: Rect, exit: &Option<ExitStatus>) {
    if rect.height == 0 || rect.width == 0 {
        return;
    }
    // `ExitStatus`'s Display is already a complete phrase: "Exited with code N",
    // "Terminated by <signal>", or "Success".
    let message = match exit {
        Some(status) if !status.success() => status.to_string(),
        _ => "process exited".to_string(),
    };
    let style = Style::default().fg(RatColor::DarkGray);
    let y = rect.y + rect.height - 1;
    for col in 0..rect.width {
        let ch = message.chars().nth(col as usize).unwrap_or(' ');
        buffer[(rect.x + col, y)]
            .set_symbol(&ch.to_string())
            .set_style(style);
    }
}

/// Map a terminal color to a ratatui color, resolving Foreground/Background
/// through the palette and falling back to white-on-black.
fn resolve_color(color: &Color, default: Option<Rgb>) -> RatColor {
    match color {
        Color::Named(NamedColor::Foreground) => {
            default.map(|c| RatColor::Rgb(c.r, c.g, c.b)).unwrap_or(RatColor::White)
        }
        Color::Named(NamedColor::Background) => {
            default.map(|c| RatColor::Rgb(c.r, c.g, c.b)).unwrap_or(RatColor::Black)
        }
        Color::Named(n) => match n {
            NamedColor::Black => RatColor::Black,
            NamedColor::Red => RatColor::Red,
            NamedColor::Green => RatColor::Green,
            NamedColor::Yellow => RatColor::Yellow,
            NamedColor::Blue => RatColor::Blue,
            NamedColor::Magenta => RatColor::Magenta,
            NamedColor::Cyan => RatColor::Cyan,
            NamedColor::White => RatColor::White,
            NamedColor::BrightBlack => RatColor::DarkGray,
            NamedColor::BrightRed => RatColor::LightRed,
            NamedColor::BrightGreen => RatColor::LightGreen,
            NamedColor::BrightYellow => RatColor::LightYellow,
            NamedColor::BrightBlue => RatColor::LightBlue,
            NamedColor::BrightMagenta => RatColor::LightMagenta,
            NamedColor::BrightCyan => RatColor::LightCyan,
            NamedColor::BrightWhite => RatColor::Gray,
            NamedColor::Cursor => RatColor::White,
            _ => RatColor::White,
        },
        Color::Spec(rgb) => RatColor::Rgb(rgb.r, rgb.g, rgb.b),
        Color::Indexed(i) => RatColor::Indexed(*i),
    }
}

/// Translate alacritty cell flags into ratatui modifiers.
fn apply_flags(mut style: Style, flags: Flags) -> Style {
    if flags.contains(Flags::BOLD | Flags::DIM_BOLD | Flags::BOLD_ITALIC) {
        style = style.add_modifier(Modifier::BOLD);
    }
    if flags.contains(Flags::ITALIC | Flags::BOLD_ITALIC) {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if flags.contains(Flags::UNDERLINE | Flags::DOUBLE_UNDERLINE) {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    if flags.contains(Flags::DIM | Flags::DIM_BOLD) {
        style = style.add_modifier(Modifier::DIM);
    }
    if flags.contains(Flags::STRIKEOUT) {
        style = style.add_modifier(Modifier::CROSSED_OUT);
    }
    if flags.contains(Flags::INVERSE) {
        style = style.add_modifier(Modifier::REVERSED);
    }
    style
}
