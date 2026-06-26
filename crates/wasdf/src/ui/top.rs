//! The top panel: a container whose children are the nav and prop panels, side
//! by side (see doc/UI.md). nav is a macOS Finder-style column directory
//! navigator; prop shows the cursor entry's properties, with a live load/clock
//! readout in its top border.

use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::core::AppState;
use crate::ui::chrome::{self, PanelFrame};
use crate::ui::render::{ratio_left, split_h};

const NAME_CAP: usize = 14;

/// Render the top panel, splitting it into its nav and prop children.
pub fn render_top(frame: &mut Frame, area: Rect, state: &AppState, panels: &mut Vec<PanelFrame>) {
    let (nav, prop) = split_h(area, ratio_left(area.width, 3, 2));
    render_nav(frame, nav, state, panels);
    render_prop(frame, prop, state, panels);
}

/// One column of the navigator: the parent directory's sorted child directories
/// and the index of the one on the path (the breadcrumb element).
struct NavColumn {
    sibs: Vec<String>,
    idx: usize,
    is_cwd: bool,
}

/// A macOS Finder-style column (Miller) directory navigator: the path from root
/// to cwd runs along the panel's horizontal axis, each level's sibling
/// directories stack vertically in its column, the current directory is
/// highlighted, and columns are right-aligned so the cwd stays visible.
fn render_nav(frame: &mut Frame, area: Rect, state: &AppState, panels: &mut Vec<PanelFrame>) {
    let banner = format!("// wasdf ver.{} //", env!("CARGO_PKG_VERSION"));
    panels.push(PanelFrame::new(area).banner(banner));
    let inner = chrome::inner(area);
    if inner.width < 4 || inner.height < 1 {
        return;
    }

    // Path levels: root, then each normal component, each with its siblings.
    let mut names: Vec<String> = vec!["/".into()];
    for c in state.cwd.components() {
        if let std::path::Component::Normal(s) = c {
            names.push(s.to_string_lossy().into_owned());
        }
    }
    let mut cols: Vec<NavColumn> =
        vec![NavColumn { sibs: vec!["/".into()], idx: 0, is_cwd: names.len() == 1 }];
    let mut parent = std::path::PathBuf::from("/");
    for i in 1..names.len() {
        let sel = names[i].clone();
        let mut sibs: Vec<String> = std::fs::read_dir(&parent)
            .map(|rd| {
                rd.flatten()
                    .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| !n.starts_with('.') || *n == sel)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        sibs.sort();
        let idx = sibs.iter().position(|s| *s == sel).unwrap_or_else(|| {
            sibs.push(sel.clone());
            sibs.len() - 1
        });
        cols.push(NavColumn { sibs, idx, is_cwd: i == names.len() - 1 });
        parent.push(sel);
    }

    let rows = inner.height as usize;
    let mid = rows / 2;
    // The names visible in a column's vertical window (centered on the path).
    let window = |c: &NavColumn| -> Vec<Option<String>> {
        (0..rows)
            .map(|r| {
                let j = c.idx as isize + r as isize - mid as isize;
                (j >= 0 && (j as usize) < c.sibs.len()).then(|| trunc(&c.sibs[j as usize]))
            })
            .collect()
    };
    let col_w = |c: &NavColumn| -> u16 {
        window(c).into_iter().flatten().map(|s| s.chars().count()).max().unwrap_or(1) as u16
    };
    let widths: Vec<u16> = cols.iter().map(col_w).collect();

    // Right-align: choose the rightmost columns that fit (3-cell gaps), keeping
    // a one-cell margin on each side.
    const GAP: u16 = 3;
    let avail = inner.width.saturating_sub(2);
    let mut total = 0u16;
    let mut start = cols.len() - 1;
    for i in (0..cols.len()).rev() {
        let add = widths[i] + if i + 1 < cols.len() { GAP } else { 0 };
        if total + add > avail {
            break;
        }
        total += add;
        start = i;
    }
    let x0 = inner.x + 1 + avail.saturating_sub(total);
    let center_y = inner.y + mid as u16;

    let path_style = Style::default().fg(Color::White).add_modifier(Modifier::BOLD);
    let cwd_style = Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD);
    let sib_style = Style::default().fg(Color::DarkGray);
    let link_style = Style::default().fg(Color::DarkGray);

    let buf = frame.buffer_mut();
    // Leading connector when columns are clipped on the left.
    if start > 0 && x0 >= inner.x + 3 {
        nav_put(buf, inner, x0 - 2, center_y, '─', link_style);
    }
    let mut x = x0;
    for i in start..cols.len() {
        let c = &cols[i];
        let cw = widths[i];
        for (r, name) in window(c).into_iter().enumerate() {
            let Some(name) = name else { continue };
            let style = if r == mid {
                if c.is_cwd { cwd_style } else { path_style }
            } else {
                sib_style
            };
            let pad = (cw as usize).saturating_sub(name.chars().count()) / 2;
            put_str(buf, inner, x + pad as u16, inner.y + r as u16, &name, style);
        }
        if i + 1 < cols.len() {
            nav_put(buf, inner, x + cw + GAP / 2, center_y, '─', link_style);
            x += cw + GAP;
        }
    }
}

fn trunc(s: &str) -> String {
    if s.chars().count() > NAME_CAP {
        format!("{}…", s.chars().take(NAME_CAP - 1).collect::<String>())
    } else {
        s.to_string()
    }
}

fn nav_put(buf: &mut Buffer, inner: Rect, x: u16, y: u16, ch: char, style: Style) {
    if x >= inner.x && y >= inner.y && x < inner.x + inner.width && y < inner.y + inner.height {
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.set_char(ch);
            cell.set_style(style);
        }
    }
}

fn put_str(buf: &mut Buffer, inner: Rect, x: u16, y: u16, s: &str, style: Style) {
    for (i, ch) in s.chars().enumerate() {
        nav_put(buf, inner, x + i as u16, y, ch, style);
    }
}

/// The prop panel: the cursor entry's properties, plus a live load/clock readout
/// in the top border.
fn render_prop(frame: &mut Frame, area: Rect, state: &AppState, panels: &mut Vec<PanelFrame>) {
    let inner = chrome::inner(area);
    let width = inner.width as usize;
    let lines = match state.current_entry() {
        Some(e) => vec![
            prop_line("PATH", &e.path.display().to_string(), width),
            prop_line("SIZE", &format!("{} ({} B)", wasdf_sdk::format_size(e.size), e.size), width),
            prop_line(
                "PERM",
                &format!("{} {} {}", wasdf_sdk::format_permissions(e.mode), e.uid, e.gid),
                width,
            ),
            prop_line(
                "TIME",
                &format!(
                    "C: {}  M: {}  A: {}",
                    fmt_time(e.created, false),
                    fmt_time(e.modified, false),
                    fmt_time(e.accessed, false)
                ),
                width,
            ),
        ],
        None => vec![Line::from("(empty)")],
    };
    frame.render_widget(Paragraph::new(lines), inner);

    // Top-border info: load averages and the current local time.
    let clock = fmt_time(Some(SystemTime::now()), true);
    let info = match loadavg() {
        Some([a, b, c]) => format!("[ {a:.2}, {b:.2}, {c:.2} ]──[ {clock} ]"),
        None => format!("[ {clock} ]"),
    };
    panels.push(PanelFrame::new(area).info(info));
}

/// A `LABEL: value` line truncated to `width` columns (with an ellipsis); the
/// label is dimmed.
fn prop_line(label: &str, value: &str, width: usize) -> Line<'static> {
    let prefix = format!("{label}: ");
    let avail = width.saturating_sub(prefix.chars().count());
    let value: String = if value.chars().count() > avail && avail > 1 {
        format!("{}…", value.chars().take(avail - 1).collect::<String>())
    } else {
        value.to_string()
    };
    Line::from(vec![
        Span::styled(prefix, Style::default().fg(Color::DarkGray)),
        Span::raw(value),
    ])
}

/// Format a SystemTime in local time. `full` → `YY-MM-DD HH:MM:SS`, else
/// `MM-DD HH:MM`.
fn fmt_time(t: Option<SystemTime>, full: bool) -> String {
    let Some(t) = t else { return "-".into() };
    let Ok(d) = t.duration_since(UNIX_EPOCH) else { return "-".into() };
    let secs = d.as_secs() as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&secs, &mut tm) }.is_null() {
        return "-".into();
    }
    let (mon, day, h, m, s) = (tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec);
    if full {
        format!("{:02}-{mon:02}-{day:02} {h:02}:{m:02}:{s:02}", (tm.tm_year + 1900) % 100)
    } else {
        format!("{mon:02}-{day:02} {h:02}:{m:02}")
    }
}

/// The 1/5/15-minute load averages, if available.
fn loadavg() -> Option<[f64; 3]> {
    let mut a = [0f64; 3];
    if unsafe { libc::getloadavg(a.as_mut_ptr(), 3) } == 3 {
        Some(a)
    } else {
        None
    }
}
