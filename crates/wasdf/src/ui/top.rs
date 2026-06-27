//! The top panel: three children side-by-side — nav / prop / monitor.
//!
//! nav     Finder-style column navigator (Miller columns).
//! prop    Selected entry metadata: SIZE, OWNR, PERM, TIME, INFO.
//! monitor System vitals: CPU load, memory usage, disk, network interfaces.
//!
//! Visibility and relative widths are controlled by TopOptions (sourced from
//! kernel Options, configurable under @/settings/Layout). When the terminal is
//! too narrow to fit all three, optional panels collapse in order: monitor
//! first, then prop.

use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::core::AppState;
use crate::ui::chrome::{self, PanelFrame};
use crate::ui::render::split_h;

// ── TopOptions ────────────────────────────────────────────────────────────────

/// Options that control the top-panel layout. Extracted once from
/// `Arc<RwLock<Options>>` at the start of each render frame.
pub struct TopOptions {
    pub border_style: String,
    /// Relative width weight for the nav panel (default 5).
    pub nav_weight: u32,
    /// Whether the prop panel is shown (default true).
    pub prop_visible: bool,
    /// Relative width weight for the prop panel (default 2).
    pub prop_weight: u32,
    /// Whether the monitor panel is shown (default true).
    pub monitor_visible: bool,
    /// Relative width weight for the monitor panel (default 2).
    pub monitor_weight: u32,
}

impl Default for TopOptions {
    fn default() -> Self {
        TopOptions {
            border_style: "single".into(),
            nav_weight: 5,
            prop_visible: true,
            prop_weight: 2,
            monitor_visible: true,
            monitor_weight: 2,
        }
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

const NAME_CAP: usize = 14;

/// Render the top panel, splitting it into nav / prop / monitor.
///
/// `func_x` is the absolute column of the function panel's left border when it
/// is visible; `None` means no function panel. This is used to position the
/// path/filename labels in the nav panel's bottom border.
pub fn render_top(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    vfs: &crate::vfs::Vfs,
    panels: &mut Vec<PanelFrame>,
    opts: &TopOptions,
    func_x: Option<u16>,
) {
    let (nav_r, prop_r, mon_r) = layout_panels(area, opts);

    // The bottom border is one connected line spanning the whole top area, so the
    // path/filename labels are budgeted against the full top width (`area`), not
    // the nav panel's slice — they may flow across the prop/monitor sections.
    let cwd_str = state.cwd.to_string_lossy().into_owned();
    let name = state.current_entry().map(|e| e.name.as_str()).unwrap_or("");
    // The padding and separators woven into the bottom border must match the
    // active border style's horizontal rule (e.g. `─` for rounded, `═` for double).
    let sep = h_rule(&opts.border_style);
    let (nav_bottom, nav_bottom_right) = plan_bottom_labels(&cwd_str, name, area, func_x, sep);

    render_nav(frame, nav_r, state, vfs, panels, nav_bottom, nav_bottom_right);
    if let Some(r) = prop_r {
        render_prop(frame, r, state, panels);
    }
    if let Some(r) = mon_r {
        render_monitor(frame, r, state, panels, &opts.border_style);
    }
}

/// Compute the top area's bottom-border labels given the current path, selected
/// filename, the full top-area rect, and the optional func-panel junction column.
///
/// The labels span the **whole** top-area bottom border (`╚`…`╣`), not just the
/// nav slice, so a long path may flow across the prop/monitor sections. All
/// widths are measured in display columns (`chars().count()`), consistent with
/// the rest of the chrome.
///
/// Returns `(bottom_info, bottom_right)`:
/// - `bottom_info`: written left-to-right starting at `top_area.x + 2`.
/// - `bottom_right`: `Some((x, text))` — the filename segment starting at `x`.
///
/// `sep` is the horizontal rule character of the active border style (`─` / `═` /
/// `-`), used for the padding and the separators between labels.
///
/// Four layout cases (func panel visible):
/// - **A** (path fits before ╦): `bottom_info` = padded path (stops before ╦),
///   `bottom_right` = `<sep>[ name ]` starting at `╦+1`.
/// - **B** (path past ╦, name fits): `bottom_info` = `[ path ]<sep>[ name ]`; ╦ is
///   overwritten (chrome's N-only junction check allows this).
/// - **C** (path past ╦, name truncated): `bottom_info` = `[ path ]<sep>[ name… ]`.
/// - **D** (path overflows total width): `bottom_info` = truncated `[ path… ]`.
fn plan_bottom_labels(
    cwd: &str,
    name: &str,
    top_area: Rect,
    func_x: Option<u16>,
    sep: char,
) -> (String, Option<(u16, String)>) {
    // bottom_info starts writing at top_area.x + 2 (clearing the `╚═` corner).
    let start_x = top_area.x + 2;
    // Max display columns the border text may occupy (leave the corners untouched).
    let available = top_area.width.saturating_sub(4) as usize;

    let pb = format!("[ {cwd} ]");
    let pb_w = pb.chars().count();

    let Some(fx) = func_x else {
        // No func panel: path only, truncated if needed.
        let s = if pb_w <= available { pb } else { path_bottom_text(cwd, available) };
        return (s, None);
    };

    // Columns from start_x to the ╦ column (exclusive): the "left zone".
    let junc_offset = (fx as usize).saturating_sub(start_x as usize);

    if name.is_empty() {
        // Right zone belongs to the function panel title — keep the path in the
        // left zone only so it does not overwrite the function title.
        let limit = junc_offset.min(available);
        let s = if pb_w <= limit { pb } else { path_bottom_text(cwd, limit) };
        return (s, None);
    }

    let nb = format!("[ {name} ]");
    let nb_w = nb.chars().count();

    if pb_w <= junc_offset {
        // Case A: path fits entirely before ╦.
        // Pad with the rule char so bottom_info is exactly junc_offset columns —
        // the string ends just before the junction cell, leaving ╦ untouched.
        let pad = junc_offset - pb_w;
        let bottom_left = format!("{}{}", pb, sep.to_string().repeat(pad));

        // Right zone: from fx+1 to the closing `╣` (top_area right edge - 1).
        let right_avail =
            (top_area.x as usize + top_area.width as usize).saturating_sub(fx as usize + 2);
        let right_text = build_right(name, &nb, right_avail, sep);
        let bottom_right = if right_text.is_empty() { None } else { Some((fx + 1, right_text)) };
        (bottom_left, bottom_right)
    } else {
        // Cases B/C/D: path extends past ╦.
        // Chrome's N-only junction check lets path chars overwrite ╦ cleanly.
        let bottom_left = if pb_w + 1 + nb_w <= available {
            // Case B: path + sep + full name fits.
            format!("{}{}{}", pb, sep, nb)
        } else if pb_w + 1 < available {
            // Case C: truncate name.
            let nb_avail = available - pb_w - 1;
            let nb_trunc = truncate_name_bracket(name, nb_avail);
            if nb_trunc.is_empty() { pb } else { format!("{}{}{}", pb, sep, nb_trunc) }
        } else if pb_w <= available {
            // Name doesn't fit at all.
            pb
        } else {
            // Case D: path itself overflows.
            path_bottom_text(cwd, available)
        };
        (bottom_left, None)
    }
}

/// Build the `<sep>[ name ]` right-segment, truncating if it exceeds `avail`
/// columns. `sep` is the active border style's horizontal rule character.
fn build_right(name: &str, nb: &str, avail: usize, sep: char) -> String {
    // Full: sep + "[ name ]"
    if nb.chars().count() + 1 <= avail {
        return format!("{}{}", sep, nb);
    }
    // Truncated: "<sep>[ name_trunc… ]"; overhead for "<sep>[ … ]" = 6 columns.
    if avail >= 6 {
        let name_chars = avail - 6;
        if name_chars > 0 {
            return format!("{}[ {}… ]", sep, name.chars().take(name_chars).collect::<String>());
        }
    }
    String::new()
}

/// The horizontal rule character for a border style, matching chrome's `glyph()`.
fn h_rule(border_style: &str) -> char {
    match border_style {
        "double" => '═',
        "ascii" => '-',
        _ => '─', // single, rounded, and any unrecognised value
    }
}

/// Format `[ name ]`, truncating content to fit in `avail` columns.
fn truncate_name_bracket(name: &str, avail: usize) -> String {
    let full = format!("[ {name} ]");
    if full.chars().count() <= avail {
        return full;
    }
    // "[ name_trunc… ]"; overhead for "[ … ]" = 5 columns.
    if avail >= 5 {
        let chars = avail - 5;
        if chars > 0 {
            return format!("[ {}… ]", name.chars().take(chars).collect::<String>());
        }
    }
    String::new()
}

/// Format `[ {path} ]` truncated to `max_width` columns with a trailing `…`.
fn path_bottom_text(path: &str, max_width: usize) -> String {
    let full = format!("[ {path} ]");
    if full.chars().count() <= max_width {
        return full;
    }
    let overhead = 5; // "[ " + "…" + " ]"
    let avail = max_width.saturating_sub(overhead);
    format!("[ {}… ]", path.chars().take(avail).collect::<String>())
}

// ── 3-way layout ─────────────────────────────────────────────────────────────

/// Minimum content widths (excluding chrome borders) for each panel.
const MIN_NAV: u16 = 22;
const MIN_PROP: u16 = 18;
const MIN_MON: u16 = 16;

/// Compute which panels are shown and their rects given the available area.
/// Optional panels collapse gracefully when the terminal is too narrow:
/// monitor drops first, then prop.
fn layout_panels(area: Rect, opts: &TopOptions) -> (Rect, Option<Rect>, Option<Rect>) {
    let w = area.width;

    let (show_prop, show_mon) = if opts.prop_visible && opts.monitor_visible {
        if w >= MIN_NAV + MIN_PROP + MIN_MON { (true, true) }
        else if w >= MIN_NAV + MIN_PROP { (true, false) }
        else { (false, false) }
    } else if opts.prop_visible {
        (w >= MIN_NAV + MIN_PROP, false)
    } else if opts.monitor_visible {
        (false, w >= MIN_NAV + MIN_MON)
    } else {
        (false, false)
    };

    if !show_prop && !show_mon {
        return (area, None, None);
    }

    let nw = opts.nav_weight.max(1) as u32;
    let pw = opts.prop_weight.max(1) as u32;
    let mw = opts.monitor_weight.max(1) as u32;

    if show_prop && show_mon {
        let tw = nw + pw + mw;
        let nav_px = ((w as u32 * nw / tw) as u16)
            .max(MIN_NAV)
            .min(w.saturating_sub(MIN_PROP + MIN_MON));
        // Remaining area after nav (split_h overlaps by 1 for shared border).
        let rest_w = w.saturating_sub(nav_px) + 1;
        let prop_px = ((rest_w as u32 * pw / (pw + mw)) as u16)
            .max(MIN_PROP)
            .min(rest_w.saturating_sub(MIN_MON));
        let (nav_r, rest) = split_h(area, nav_px);
        let (prop_r, mon_r) = split_h(rest, prop_px);
        (nav_r, Some(prop_r), Some(mon_r))
    } else if show_prop {
        let tw = nw + pw;
        let nav_px = ((w as u32 * nw / tw) as u16)
            .max(MIN_NAV)
            .min(w.saturating_sub(MIN_PROP));
        let (nav_r, prop_r) = split_h(area, nav_px);
        (nav_r, Some(prop_r), None)
    } else {
        // show_mon only
        let tw = nw + mw;
        let nav_px = ((w as u32 * nw / tw) as u16)
            .max(MIN_NAV)
            .min(w.saturating_sub(MIN_MON));
        let (nav_r, mon_r) = split_h(area, nav_px);
        (nav_r, None, Some(mon_r))
    }
}

// ── Nav panel ─────────────────────────────────────────────────────────────────

/// One column of the navigator.
struct NavColumn {
    sibs: Vec<String>,
    idx: usize,
    is_cwd: bool,
    is_contents: bool,
}

fn render_nav(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    vfs: &crate::vfs::Vfs,
    panels: &mut Vec<PanelFrame>,
    nav_bottom: String,
    nav_bottom_right: Option<(u16, String)>,
) {
    let banner = format!("/ wasdf ver.{} /", env!("CARGO_PKG_VERSION"));
    let mut pf = PanelFrame::new(area).banner(banner).bottom_info(nav_bottom);
    if let Some((x, text)) = nav_bottom_right {
        pf = pf.bottom_right(x, text);
    }
    let inner = chrome::inner(area);
    if inner.width < 4 || inner.height < 1 {
        panels.push(pf);
        return;
    }

    let mut cols = if crate::vfs::is_virtual(&state.cwd) {
        build_virtual_cols(&state.cwd, vfs)
    } else {
        build_real_cols(&state.cwd)
    };

    if !state.entries.is_empty() {
        cols.push(NavColumn {
            sibs: state.entries.iter().map(|e| e.name.clone()).collect(),
            idx: state.cursor,
            is_cwd: false,
            is_contents: true,
        });
    }
    if cols.is_empty() {
        panels.push(pf);
        return;
    }

    let rows = inner.height as usize;
    let mid = rows / 2;
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

    const GAP: u16 = 3;
    let avail = inner.width.saturating_sub(2);

    let mut col_x: Vec<u16> = vec![0u16; cols.len()];
    for i in 1..cols.len() {
        col_x[i] = col_x[i - 1] + widths[i - 1] + GAP;
    }
    let total_w = col_x.last().copied().unwrap_or(0) + widths.last().copied().unwrap_or(0);
    let scroll_x: u16 = total_w.saturating_sub(avail);

    let path_style = Style::default().fg(Color::White).add_modifier(Modifier::BOLD);
    let cwd_style = Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD);
    let sib_style = Style::default().fg(Color::DarkGray);
    let link_style = Style::default().fg(Color::DarkGray);

    let buf = frame.buffer_mut();
    let center_y = inner.y + mid as u16;

    if scroll_x > 0 {
        nav_put(buf, inner, inner.x + 1, center_y, '…', link_style);
    }

    for (i, c) in cols.iter().enumerate() {
        let cw = widths[i];
        let lx0 = col_x[i];
        let lx1 = lx0 + cw;
        if lx1 <= scroll_x || lx0 >= scroll_x + avail {
            continue;
        }
        if i + 1 < cols.len() {
            let conn_lx = lx0 + cw + GAP / 2;
            if conn_lx >= scroll_x && conn_lx < scroll_x + avail {
                let sx = inner.x + 1 + (conn_lx - scroll_x);
                nav_put(buf, inner, sx, center_y, '─', link_style);
            }
        }
        for (r, name) in window(c).into_iter().enumerate() {
            let Some(name) = name else { continue };
            let style = if r == mid {
                if c.is_cwd { cwd_style }
                else if c.is_contents { Style::default().add_modifier(Modifier::REVERSED) }
                else { path_style }
            } else {
                sib_style
            };
            let pad = (cw as usize).saturating_sub(name.chars().count()) / 2;
            let name_lx = lx0 + pad as u16;
            let screen_y = inner.y + r as u16;
            for (ci, ch) in name.chars().enumerate() {
                let lx = name_lx + ci as u16;
                if lx < scroll_x || lx >= scroll_x + avail { continue; }
                let sx = inner.x + 1 + (lx - scroll_x);
                nav_put(buf, inner, sx, screen_y, ch, style);
            }
        }
    }
    panels.push(pf);
}

fn build_real_cols(cwd: &std::path::Path) -> Vec<NavColumn> {
    let mut names: Vec<String> = vec!["/".into()];
    for c in cwd.components() {
        if let std::path::Component::Normal(s) = c {
            names.push(s.to_string_lossy().into_owned());
        }
    }
    let mut cols: Vec<NavColumn> =
        vec![NavColumn { sibs: vec!["/".into()], idx: 0, is_cwd: names.len() == 1, is_contents: false }];
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
        cols.push(NavColumn { sibs, idx, is_cwd: i == names.len() - 1, is_contents: false });
        parent.push(sel);
    }
    cols
}

fn build_virtual_cols(cwd: &std::path::Path, vfs: &crate::vfs::Vfs) -> Vec<NavColumn> {
    use crate::vfs::vfs_root;
    let mut levels: Vec<std::path::PathBuf> = vec![vfs_root()];
    let mut cur = vfs_root();
    for c in cwd.components() {
        if let std::path::Component::Normal(s) = c {
            if s == std::ffi::OsStr::new("@") { continue; }
            cur = cur.join(s);
            levels.push(cur.clone());
        }
    }
    let n = levels.len();
    let mut cols: Vec<NavColumn> = Vec::new();
    for (i, level) in levels.iter().enumerate() {
        let is_cwd = i == n - 1;
        if level == &vfs_root() {
            cols.push(NavColumn { sibs: vec!["@".into()], idx: 0, is_cwd, is_contents: false });
        } else {
            let parent = level.parent().map(|p| p.to_path_buf()).unwrap_or_else(vfs_root);
            let all_entries = vfs.list(&parent, false).unwrap_or_default();
            let siblings: Vec<String> = all_entries.iter().map(|e| e.name.clone()).collect();
            let path_name = level.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
            let idx = all_entries
                .iter()
                .position(|e| e.path.file_name().map(|f| f.to_string_lossy() == path_name.as_str()).unwrap_or(false))
                .unwrap_or(0);
            cols.push(NavColumn { sibs: siblings, idx, is_cwd, is_contents: false });
        }
    }
    cols
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

// ── Prop panel ────────────────────────────────────────────────────────────────

/// The prop panel: SIZE / OWNR / PERM / TIME / INFO.
fn render_prop(frame: &mut Frame, area: Rect, state: &AppState, panels: &mut Vec<PanelFrame>) {
    let inner = chrome::inner(area);
    let w = inner.width as usize;

    let lines = match state.current_entry() {
        Some(e) => vec![
            prop_line(
                "SIZE",
                &format!("{} ({} B)", wasdf_sdk::format_size(e.size), e.size),
                w,
            ),
            prop_line(
                "OWNR",
                &format!("{}  {}", uid_name(e.uid), gid_name(e.gid)),
                w,
            ),
            prop_line("PERM", &wasdf_sdk::format_permissions(e.mode), w),
            prop_line("TIME", &fmt_time(e.modified, true), w),
            prop_line("INFO", &file_info(e), w),
        ],
        None => vec![Line::from("(empty)")],
    };
    frame.render_widget(Paragraph::new(lines), inner);
    panels.push(PanelFrame::new(area));
}

// ── Monitor panel ─────────────────────────────────────────────────────────────

/// The monitor panel: CPU load, memory, disk, network interfaces.
fn render_monitor(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    panels: &mut Vec<PanelFrame>,
    border_style: &str,
) {
    let inner = chrome::inner(area);
    let w = inner.width as usize;

    let mut lines: Vec<Line> = Vec::new();
    // BAR_OVERHEAD: "▐" + "▌ " = 3 chars wrapping the bar; LABEL_W: "XYZ " = 4 chars.
    const BAR_OVERHEAD: usize = 3;
    const LABEL_W: usize = 4;

    // CPU: load averages
    if let Some([la1, la5, la15]) = loadavg() {
        let ratio = (la1 / 4.0_f64).clamp(0.0, 1.0);
        let val = format!("{la1:.2} {la5:.2} {la15:.2}");
        let bar_w = w.saturating_sub(LABEL_W + BAR_OVERHEAD + val.chars().count());
        if bar_w >= 1 {
            lines.push(monitor_row("CPU", &mini_bar(ratio, bar_w), &val, w));
        } else {
            lines.push(monitor_row("CPU", "", &format!("{la1:.2}"), w));
        }
    }

    // MEM
    if let Some((mem_total, mem_used)) = mem_info() {
        let ratio = if mem_total > 0 { mem_used as f64 / mem_total as f64 } else { 0.0 };
        let val = format!("{}/{}", wasdf_sdk::format_size(mem_used), wasdf_sdk::format_size(mem_total));
        let bar_w = w.saturating_sub(LABEL_W + BAR_OVERHEAD + val.chars().count());
        if bar_w >= 1 {
            lines.push(monitor_row("MEM", &mini_bar(ratio, bar_w), &val, w));
        } else {
            lines.push(monitor_row("MEM", "", &wasdf_sdk::format_size(mem_used), w));
        }
    }

    // DSK: disk usage of the current directory's filesystem
    if let Some((dsk_total, dsk_avail)) = disk_info(&state.cwd) {
        let dsk_used = dsk_total.saturating_sub(dsk_avail);
        let ratio = if dsk_total > 0 { dsk_used as f64 / dsk_total as f64 } else { 0.0 };
        let val = format!("{} free", wasdf_sdk::format_size(dsk_avail));
        let bar_w = w.saturating_sub(LABEL_W + BAR_OVERHEAD + val.chars().count());
        if bar_w >= 1 {
            lines.push(monitor_row("DSK", &mini_bar(ratio, bar_w), &val, w));
        } else {
            lines.push(monitor_row("DSK", "", &wasdf_sdk::format_size(dsk_avail), w));
        }
    }

    // NET: active non-loopback interfaces
    let ifaces = active_interfaces();
    if !ifaces.is_empty() {
        lines.push(monitor_row("NET", "", &ifaces.join(" "), w));
    }

    frame.render_widget(Paragraph::new(lines), inner);

    // Border: clock in the right-aligned info slot.
    let clock_full = fmt_time(Some(SystemTime::now()), true); // "YYYY-MM-DD HH:MM:SS"
    let h_sep = h_rule(border_style);
    // Pick the richest clock form that fits in the info slot (area.width − 4).
    let avail = area.width.saturating_sub(4) as usize;
    let time_ss = &clock_full[11..]; // "HH:MM:SS"
    let time_mm = &clock_full[11..16]; // "HH:MM"
    let candidates = [
        format!("[ {clock_full} ]{h_sep}{h_sep}"),
        format!("[ {clock_full} ]"),
        format!("[ {time_ss} ]"),
        format!("[ {time_mm} ]"),
    ];
    if let Some(info) = candidates.iter().find(|s| s.chars().count() <= avail) {
        panels.push(PanelFrame::new(area).info(info.clone()));
    } else {
        panels.push(PanelFrame::new(area));
    }
}

// ── Monitor helpers ───────────────────────────────────────────────────────────

/// A filled-░ progress bar of `width` characters representing `ratio` ∈ [0,1].
fn mini_bar(ratio: f64, width: usize) -> String {
    let filled = (ratio.clamp(0.0, 1.0) * width as f64).round() as usize;
    format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
}

/// One monitor row: `LABEL ▐bar▌ value` (all truncated to `width`).
fn monitor_row(label: &str, bar: &str, value: &str, width: usize) -> Line<'static> {
    let lbl = format!("{label} ");
    let bar_part: String = if bar.is_empty() { String::new() } else { format!("▐{bar}▌ ") };
    let used = lbl.chars().count() + bar_part.chars().count();
    let avail = width.saturating_sub(used);
    let val: String = value.chars().take(avail).collect();
    Line::from(vec![
        Span::styled(lbl, Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::styled(bar_part, Style::default().fg(Color::Green)),
        Span::raw(val),
    ])
}

/// Total and used physical memory in bytes.
fn mem_info() -> Option<(u64, u64)> {
    #[cfg(target_os = "macos")]
    {
        use std::ffi::CStr;
        let mut total = 0u64;
        let mut sz = std::mem::size_of::<u64>();
        let name = CStr::from_bytes_with_nul(b"hw.memsize\0").unwrap();
        unsafe {
            libc::sysctlbyname(
                name.as_ptr(), &mut total as *mut u64 as *mut libc::c_void,
                &mut sz, std::ptr::null_mut(), 0,
            );
        }
        if total == 0 { return None; }

        let mut free_pages = 0u32;
        let mut fp_sz = std::mem::size_of::<u32>();
        let fp_name = CStr::from_bytes_with_nul(b"vm.page_free_count\0").unwrap();
        unsafe {
            libc::sysctlbyname(
                fp_name.as_ptr(), &mut free_pages as *mut u32 as *mut libc::c_void,
                &mut fp_sz, std::ptr::null_mut(), 0,
            );
        }

        let mut page_size = 4096u64;
        let mut ps_sz = std::mem::size_of::<u64>();
        let ps_name = CStr::from_bytes_with_nul(b"hw.pagesize\0").unwrap();
        unsafe {
            libc::sysctlbyname(
                ps_name.as_ptr(), &mut page_size as *mut u64 as *mut libc::c_void,
                &mut ps_sz, std::ptr::null_mut(), 0,
            );
        }

        let available = free_pages as u64 * page_size;
        Some((total, total.saturating_sub(available)))
    }
    #[cfg(target_os = "linux")]
    {
        let content = std::fs::read_to_string("/proc/meminfo").ok()?;
        let mut total = 0u64;
        let mut available = 0u64;
        for line in content.lines() {
            if let Some(rest) = line.strip_prefix("MemTotal:") {
                total = rest.trim().split_whitespace().next()?.parse::<u64>().ok()? * 1024;
            } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
                available = rest.trim().split_whitespace().next()?.parse::<u64>().ok()? * 1024;
            }
        }
        if total == 0 { None } else { Some((total, total.saturating_sub(available))) }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    { None }
}

/// Total and available bytes on the filesystem containing `path`.
fn disk_info(path: &std::path::Path) -> Option<(u64, u64)> {
    let cstr = std::ffi::CString::new(
        path.as_os_str().to_string_lossy().as_bytes()
    ).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(cstr.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let bs = if st.f_frsize > 0 { st.f_frsize } else { st.f_bsize } as u64;
    Some((st.f_blocks as u64 * bs, st.f_bavail as u64 * bs))
}

/// Names of UP non-loopback network interfaces.
fn active_interfaces() -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    unsafe {
        let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut ifap) != 0 || ifap.is_null() {
            return names;
        }
        let mut ifa = ifap;
        while !ifa.is_null() {
            let flags = (*ifa).ifa_flags as u32;
            let up = flags & (libc::IFF_UP as u32) != 0;
            let loopback = flags & (libc::IFF_LOOPBACK as u32) != 0;
            if up && !loopback {
                if let Ok(name) = std::ffi::CStr::from_ptr((*ifa).ifa_name).to_str() {
                    let s = name.to_string();
                    if !names.contains(&s) {
                        names.push(s);
                    }
                }
            }
            ifa = (*ifa).ifa_next;
        }
        libc::freeifaddrs(ifap);
    }
    names
}

// ── Prop helpers ──────────────────────────────────────────────────────────────

/// Describe the file type shown in the INFO row.
fn file_info(e: &crate::core::Entry) -> String {
    if e.is_symlink {
        return match &e.symlink_target {
            Some(t) => format!("→ {}", t.display()),
            None => "symbolic link".into(),
        };
    }
    if e.is_dir {
        return "directory".into();
    }
    #[cfg(unix)]
    let executable = e.mode & 0o111 != 0;
    #[cfg(not(unix))]
    let executable = false;
    if executable {
        return "executable".into();
    }
    match e.path.extension().and_then(|s| s.to_str()) {
        Some(ext) => format!(".{ext} file"),
        None => "regular file".into(),
    }
}

/// `LABEL: value` line, value truncated to fit `width`. Label is dimmed.
fn prop_line(label: &str, value: &str, width: usize) -> Line<'static> {
    let prefix = format!("{label} ");
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

/// Resolve a uid to `name (uid)`, or just `uid` if the name is unavailable.
fn uid_name(uid: u32) -> String {
    unsafe {
        let pw = libc::getpwuid(uid as libc::uid_t);
        if !pw.is_null() {
            if let Ok(s) = std::ffi::CStr::from_ptr((*pw).pw_name).to_str() {
                if !s.is_empty() {
                    return format!("{s} ({uid})");
                }
            }
        }
    }
    uid.to_string()
}

/// Resolve a gid to `name (gid)`, or just `gid` if the name is unavailable.
fn gid_name(gid: u32) -> String {
    unsafe {
        let gr = libc::getgrgid(gid as libc::gid_t);
        if !gr.is_null() {
            if let Ok(s) = std::ffi::CStr::from_ptr((*gr).gr_name).to_str() {
                if !s.is_empty() {
                    return format!("{s} ({gid})");
                }
            }
        }
    }
    gid.to_string()
}

/// Format a `SystemTime` in local time.
/// `full=true`  → `YYYY-MM-DD HH:MM:SS`
/// `full=false` → `MM-DD HH:MM`
fn fmt_time(t: Option<SystemTime>, full: bool) -> String {
    let Some(t) = t else { return "-".into() };
    let Ok(d) = t.duration_since(UNIX_EPOCH) else { return "-".into() };
    let secs = d.as_secs() as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&secs, &mut tm) }.is_null() {
        return "-".into();
    }
    let (yr, mon, day, h, m, s) =
        (tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec);
    if full {
        format!("{yr:04}-{mon:02}-{day:02} {h:02}:{m:02}:{s:02}")
    } else {
        format!("{mon:02}-{day:02} {h:02}:{m:02}")
    }
}

/// 1/5/15-minute load averages, if available.
fn loadavg() -> Option<[f64; 3]> {
    let mut a = [0f64; 3];
    if unsafe { libc::getloadavg(a.as_mut_ptr(), 3) } == 3 { Some(a) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn top(width: u16) -> Rect {
        Rect { x: 0, y: 0, width, height: 7 }
    }

    #[test]
    fn no_func_panel_shows_path_only() {
        let (info, right) = plan_bottom_labels("/a/b", "file.rs", top(90), None, '═');
        assert!(info.contains("/a/b"), "path is shown");
        assert!(right.is_none(), "no right segment without a func panel");
    }

    #[test]
    fn long_path_without_func_truncates_with_ellipsis() {
        let long = "/very/long/path/".repeat(10);
        let (info, right) = plan_bottom_labels(&long, "f.rs", top(40), None, '═');
        assert!(info.ends_with("… ]"), "overflowing path is …-truncated: {info:?}");
        assert!(info.chars().count() <= 36, "stays within the available width");
        assert!(right.is_none());
    }

    #[test]
    fn case_a_places_name_after_the_junction() {
        // Short path, func panel at column 30 → path fits before ╦, name goes right.
        let (info, right) = plan_bottom_labels("/a", "file.rs", top(90), Some(30), '═');
        // bottom_info is padded to end exactly before the junction column (30).
        // start_x = 2, so junc_offset = 28 columns of left text.
        assert_eq!(info.chars().count(), 28, "left text stops just before ╦");
        let (x, text) = right.expect("name segment present in Case A");
        assert_eq!(x, 31, "name starts at func_x + 1");
        assert!(text.contains("file.rs"), "name segment carries the filename");
        assert!(text.starts_with('═'), "name segment leads with a rule into the bracket");
    }

    #[test]
    fn case_a_works_even_when_junction_exceeds_nav_width() {
        // Regression for the nav-width binding bug: func_x can sit far to the right
        // (past where a narrow nav panel would end). The label is budgeted against
        // the full top width, so the filename is still placed rather than dropped.
        let (info, right) = plan_bottom_labels("/a", "k.rs", top(90), Some(70), '═');
        assert_eq!(info.chars().count(), 68, "left text reaches the distant ╦");
        let (x, text) = right.expect("name still placed past a narrow nav slice");
        assert_eq!(x, 71);
        assert!(text.contains("k.rs"));
    }

    #[test]
    fn case_b_flows_path_past_junction_with_name_appended() {
        // Path longer than the junction offset → it overruns ╦; name is appended.
        let cwd = "/Users/szk/src/github/wasdf/crates/wasdf/src/app";
        let (info, right) = plan_bottom_labels(cwd, "kernel.rs", top(90), Some(30), '═');
        assert!(info.contains(cwd), "full path present");
        assert!(info.contains("═[ kernel.rs ]"), "name appended after the path: {info:?}");
        assert!(right.is_none(), "Case B carries everything in bottom_info");
    }

    #[test]
    fn case_c_truncates_the_name_when_it_overflows() {
        let cwd = "/Users/szk/src/github/wasdf/crates/wasdf/src/app";
        // Narrow enough that path fits but the full name does not.
        let (info, right) = plan_bottom_labels(cwd, "a_very_long_filename.rs", top(64), Some(30), '═');
        assert!(info.contains(cwd), "path is kept");
        assert!(info.ends_with("… ]"), "name is …-truncated: {info:?}");
        assert!(right.is_none());
    }

    #[test]
    fn empty_name_never_emits_a_right_segment() {
        let (info, right) = plan_bottom_labels("/a/b", "", top(90), Some(30), '═');
        assert!(info.contains("/a/b"));
        assert!(right.is_none(), "no selection → no filename");
    }

    #[test]
    fn empty_name_with_long_path_stays_in_left_zone() {
        // A path that exceeds junc_offset but fits in available — without the fix
        // it would overflow into the function panel's title area.
        // func_x=30 → start_x=2 → junc_offset=28; path "[ /Users/szk/src/github/dotfiles ]" = 35 chars.
        let cwd = "/Users/szk/src/github/dotfiles";
        let (info, right) = plan_bottom_labels(cwd, "", top(90), Some(30), '═');
        assert!(info.chars().count() <= 28, "path clipped to left zone: {}", info.chars().count());
        assert!(info.ends_with("… ]"), "overflowing path is truncated with ellipsis: {info:?}");
        assert!(right.is_none());
    }
}
