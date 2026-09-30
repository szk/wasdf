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
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::core::{AppState, Mode, Role, SubLayout, Theme};
use crate::ui::chrome::{self, PanelArea, PanelFrame};

// ── TopOptions ────────────────────────────────────────────────────────────────

/// The per-frame snapshot of the display options the renderers read (the
/// panel geometry itself comes from the layout).
pub struct TopOptions {
    pub border_style: String,
    /// Per-metric monitor customization.
    pub monitor: MonitorOptions,
    /// Scrollbar glyphs.
    pub scrollbar: crate::ui::chrome::ScrollbarChars,
    /// Whether the Rows layout's logarithmic file-size bar is drawn at all
    /// (`@/settings/Display/file-row-size-graph`, default on). Consumed by
    /// `ui/middle.rs`, not the top panel — lives here anyway since this is
    /// already the per-frame options snapshot every renderer reads from.
    pub file_row_size_graph: bool,
}

impl Default for TopOptions {
    fn default() -> Self {
        TopOptions {
            border_style: "single".into(),
            monitor: MonitorOptions::default(),
            scrollbar: crate::ui::chrome::ScrollbarChars::default(),
            file_row_size_graph: true,
        }
    }
}

/// Per-metric monitor display customization (CPU / Memory / Disk / Network).
#[derive(Clone)]
pub struct MetricCfg {
    /// Show this metric's row at all.
    pub visible: bool,
    /// Show the gauge.
    pub bar: bool,
    /// Show the value (label) text.
    pub label: bool,
    /// Columns always reserved for the value at the right edge of the row.
    pub label_max: u32,
    /// Resample interval in seconds.
    pub refresh: u64,
    /// Window (seconds) over which the graph's full-scale peak is taken.
    pub graph_window: u64,
    /// Gauge style: `bar` (horizontal fill) | `blocks` | `braille` (sparklines).
    pub bar_kind: String,
}

impl Default for MetricCfg {
    fn default() -> Self {
        MetricCfg {
            visible: true,
            bar: true,
            label: true,
            label_max: 24,
            refresh: 2,
            graph_window: 600,
            bar_kind: "bar".into(),
        }
    }
}

#[derive(Clone, Default)]
pub struct MonitorOptions {
    pub cpu: MetricCfg,
    pub mem: MetricCfg,
    pub dsk: MetricCfg,
    pub net: MetricCfg,
}

impl MonitorOptions {
    /// Build per-metric config from the option store (`monitor.<metric>.*`).
    pub fn from_store(s: &crate::core::OptionStore) -> Self {
        let cfg = |m: &str| MetricCfg {
            visible: s.get_bool(&format!("monitor.{m}.visible")),
            bar: s.get_bool(&format!("monitor.{m}.bar")),
            label: s.get_bool(&format!("monitor.{m}.label")),
            label_max: s.get_u32(&format!("monitor.{m}.label-max")),
            refresh: s.get_i64(&format!("monitor.{m}.refresh")).max(1) as u64,
            graph_window: s.get_i64(&format!("monitor.{m}.graph-window")).max(1) as u64,
            bar_kind: s.get_str(&format!("monitor.{m}.bar-kind")),
        };
        MonitorOptions { cpu: cfg("cpu"), mem: cfg("mem"), dsk: cfg("dsk"), net: cfg("net") }
    }

    fn get(&self, id: &str) -> &MetricCfg {
        match id {
            "cpu" => &self.cpu,
            "mem" => &self.mem,
            "dsk" => &self.dsk,
            _ => &self.net,
        }
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

const NAME_CAP: usize = 14;

/// The path and file-name labels (doc/UI.md, Screen chrome): the path on the
/// top border of the file panel — in the select layout, of the panel in its
/// place — and, while the function panel previews a file and shares that line,
/// the file name after the junction. `function` is the function panel's rect
/// when it is shown; `function_title` is the title it drew.
pub fn border_labels(
    state: &AppState,
    anchor: Rect,
    function: Option<Rect>,
    function_title: Option<&str>,
    border_style: &str,
) -> Vec<chrome::BorderLabel> {
    // Only a function panel on the same line joins the path's line; one placed
    // elsewhere keeps its own title on its own border.
    let beside = function.filter(|f| f.y == anchor.y && f.x > anchor.x);
    let span = match beside {
        Some(f) => Rect { width: f.x + f.width - anchor.x, ..anchor },
        None => anchor,
    };
    let cwd_str = state.cwd.to_string_lossy().into_owned();
    // The file name is placed after the junction only while the function panel
    // previews file content — the one frame where its title *is* the shown
    // file's name. In Command / Exec / Select it keeps its own mode title.
    let name: Option<String> = if beside.is_some()
        && !matches!(state.mode(), Mode::Select(_))
        && state.function.visible
        && state.function.sublayout == SubLayout::Content
    {
        function_title.map(str::to_string)
    } else {
        None
    };
    let (path, named) = plan_bottom_labels(&cwd_str, name.as_deref(), span, beside.map(|f| f.x), border_style);
    let right = span.x + span.width - 1;
    let mut out = vec![chrome::BorderLabel {
        x: span.x + 2,
        y: span.y,
        right,
        text: path,
        focused: state.focused_panel == crate::core::PANEL_FILE,
    }];
    if let Some((x, text)) = named {
        out.push(chrome::BorderLabel { x, y: span.y, right, text, focused: state.focused_panel == crate::core::PANEL_FUNCTION });
    }
    out
}

/// Lay out the top area's bottom-border labels — the current **path** (left,
/// owned by the file panel) and the optional cursor **filename** (right, the
/// function panel's title) — across the shared border, per the five cases in
/// doc/UI.md. Returns `(path_label, Some((x, name_segment)))`, where the name
/// segment is positioned at absolute column `x`.
///
/// The path is `[ cwd ]` written from `top_area.x + 2`; the filename is
/// `[ name ]`. The five outcomes follow from whether the function panel is
/// visible and how the path length compares to the junction column `func_x`:
///
/// 1. No func panel, path fits — the path fills from the left, no filename.
/// 2. Func visible, path shorter than the junction — the `╦` is kept and the
///    filename starts just after it (`func_x + 1`).
/// 3. Func visible, path longer than the junction — the path overwrites the `╦`
///    and flows past it; the filename follows after a one-cell separator.
/// 4. As Case 3, but the filename no longer fits, so it is `…`-truncated.
/// 5. The path alone overflows the whole width — it is `…`-truncated, no filename.
///
/// `name` is `None` for non-content frames (Command / Exec / Select, or an empty
/// directory); then the path is clamped to the file panel's zone and no filename
/// is drawn, so the function panel's own title is left intact. The name segment is
/// padded with trailing border rule out to the frame edge so any function title
/// bled onto this shared row is erased (chrome's bottom painter restores the
/// crossed junctions). Widths are display columns (`chars().count()`).
fn plan_bottom_labels(
    cwd: &str,
    name: Option<&str>,
    top_area: Rect,
    func_x: Option<u16>,
    border_style: &str,
) -> (String, Option<(u16, String)>) {
    // The label starts at top_area.x + 2 (clearing the `╚═` corner); paint stops
    // at the closing corner column on the right.
    let start = top_area.x + 2;
    let frame_right = top_area.x + top_area.width - 1;
    // Max display columns the path may occupy before it overflows the whole width.
    let available = top_area.width.saturating_sub(4) as usize;
    let rule = h_rule(border_style);

    let path_label = format!("[ {cwd} ]");
    let pl = path_label.chars().count();

    // Case 5: the path alone overflows the border — truncate it, show no filename,
    // regardless of the function panel.
    if pl > available {
        return (bracket_truncated(cwd, available), None);
    }

    let Some(fx) = func_x else {
        // Case 1: no function panel and the path fits — fill from the left.
        return (path_label, None);
    };

    // Pad a bracket label with trailing rule out to the frame's right edge so the
    // function panel's title, bled onto this shared row, is fully erased (chrome
    // restores the crossed junction glyphs from the rule cells).
    let pad_to_edge = |label: String, x: u16| -> String {
        let span = frame_right.saturating_sub(x) as usize;
        let len = label.chars().count();
        if len >= span {
            label
        } else {
            let mut s = label;
            s.extend(std::iter::repeat_n(rule, span - len));
            s
        }
    };

    let Some(name) = name else {
        // Function panel visible but not previewing content: keep the path inside
        // the file panel's zone (before the ╦) so it never overwrites the function
        // panel's own mode title.
        let zone = (fx as usize).saturating_sub(start as usize);
        return if pl <= zone {
            (path_label, None)
        } else {
            (bracket_truncated(cwd, zone), None)
        };
    };

    let name_label = format!("[ {name} ]");
    let nl = name_label.chars().count();
    let junc_off = (fx as usize).saturating_sub(start as usize);

    // The right-zone start column, and a closure to finish the segment (pad it to
    // the edge if it fits, else `…`-truncate it to the remaining span).
    let segment = |nx: u16| -> Option<(u16, String)> {
        if nx >= frame_right {
            return None;
        }
        let span = frame_right.saturating_sub(nx) as usize;
        let seg = if nl <= span { pad_to_edge(name_label.clone(), nx) } else { bracket_truncated(name, span) };
        Some((nx, seg))
    };

    if pl <= junc_off {
        // Case 2: the path ends before the junction; the ╦ is preserved and the
        // filename sits in the right zone, starting just after the junction.
        (path_label, segment(fx + 1))
    } else {
        // Cases 3 & 4: the path is longer than the junction, so it overwrites the
        // ╦ and flows past it; the filename follows after a one-cell separator. The
        // separator rule is appended to the path so the single cell between the
        // path's `]` and the filename is a border rule (which erases a bled title /
        // restores a crossed junction), never a stray title letter.
        match segment(start + pl as u16 + 1) {
            Some(seg) => (format!("{path_label}{rule}"), Some(seg)),
            None => (path_label, None),
        }
    }
}

/// The horizontal rule character for a border style, matching chrome's `glyph()`.
fn h_rule(border_style: &str) -> char {
    match border_style {
        "double" => '═',
        "ascii" => '-',
        _ => '─', // single, rounded, and any unrecognised value
    }
}

/// Format `[ {text} ]` truncated to `max_width` columns with a trailing `…`.
fn bracket_truncated(text: &str, max_width: usize) -> String {
    let full = format!("[ {text} ]");
    if full.chars().count() <= max_width {
        return full;
    }
    let overhead = 5; // "[ " + "…" + " ]"
    let avail = max_width.saturating_sub(overhead);
    format!("[ {}… ]", text.chars().take(avail).collect::<String>())
}

// ── Nav panel ─────────────────────────────────────────────────────────────────

/// One column of the navigator.
struct NavColumn {
    sibs: Vec<String>,
    idx: usize,
    is_cwd: bool,
    is_contents: bool,
}

#[allow(clippy::too_many_arguments)]
pub fn render_nav(
    frame: &mut Frame,
    pa: PanelArea,
    state: &AppState,
    vfs: &crate::vfs::Vfs,
    panels: &mut Vec<PanelFrame>,
    theme: &Theme,
) {
    let banner = format!("/ wasdf v{} /", env!("CARGO_PKG_VERSION"));
    let pf = pa.frame().banner(banner);
    let inner = pa.inner();
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

    let path_style = theme.style(Role::PathActive);
    let cwd_style = theme.style(Role::NavCursorFg).bg(theme.role(Role::NavCursorBg));
    let sib_style = theme.style(Role::PathMuted);
    let link_style = theme.style(Role::PathMuted);

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
pub fn render_prop(frame: &mut Frame, pa: PanelArea, state: &AppState, panels: &mut Vec<PanelFrame>, theme: &Theme) {
    let inner = pa.inner();
    let w = inner.width as usize;

    let lines = match state.current_entry() {
        Some(e) => vec![
            prop_line(
                "SIZE",
                &format!("{} ({} B)", wasdf_sdk::format_size(e.size), e.size),
                w, Role::PropSizeLabel, Role::PropSizeValue, theme,
            ),
            prop_line(
                "OWNR",
                &format!("{}  {}", uid_name(e.uid), gid_name(e.gid)),
                w, Role::PropOwnerLabel, Role::PropOwnerValue, theme,
            ),
            prop_line("PERM", &wasdf_sdk::format_permissions(e.mode), w,
                Role::PropPermLabel, Role::PropPermValue, theme),
            prop_line("TIME", &fmt_time(e.modified, true), w,
                Role::PropTimeLabel, Role::PropTimeValue, theme),
            prop_line("INFO", &file_info(e), w,
                Role::PropInfoLabel, Role::PropInfoValue, theme),
        ],
        None => vec![Line::from("(empty)")],
    };
    frame.render_widget(Paragraph::new(lines), inner);
    panels.push(pa.frame());
}

// ── Monitor panel ─────────────────────────────────────────────────────────────

/// The monitor panel: CPU load, memory, disk, network throughput. Each metric's
/// visibility, gauge/value, ratio, refresh cadence, and gauge style are driven by
/// its `MetricCfg` (configured under `@/settings/Layout/Monitor/<Metric>`).
pub fn render_monitor(
    frame: &mut Frame,
    pa: PanelArea,
    state: &AppState,
    panels: &mut Vec<PanelFrame>,
    border_style: &str,
    mon: &MonitorOptions,
    theme: &Theme,
) {
    let area = pa.rect;
    let inner = pa.inner();
    let w = inner.width as usize;

    let mut lines: Vec<Line> = Vec::new();
    // (id, display name, label/bar/value roles).
    let metrics: [(&str, &str, Role, Role, Role); 4] = [
        ("cpu", "CPU", Role::MonitorCpuLabel, Role::MonitorCpuBar, Role::MonitorCpuValue),
        ("mem", "MEM", Role::MonitorMemLabel, Role::MonitorMemBar, Role::MonitorMemValue),
        ("dsk", "DSK", Role::MonitorDskLabel, Role::MonitorDskBar, Role::MonitorDskValue),
        ("net", "NET", Role::MonitorNetLabel, Role::MonitorNetBar, Role::MonitorNetValue),
    ];
    for (id, name, lbl_role, bar_role, val_role) in metrics {
        let cfg = mon.get(id);
        if !cfg.visible {
            continue;
        }
        // Disk is special: it shows read/write operation rates as two gauges.
        if id == "dsk" {
            let ds = disk_runtime(cfg.refresh, cfg.graph_window);
            lines.push(disk_row(&cfg, &ds, w, lbl_role, bar_role, val_role, theme));
            continue;
        }
        let line = match metric_runtime(id, cfg.refresh, cfg.graph_window, &state.cwd) {
            Some((ratio, value, history)) => {
                metric_row(name, &cfg, ratio, &value, &history, w, lbl_role, bar_role, val_role, theme)
            }
            // No data yet (startup or unsupported platform): muted placeholder.
            None => muted_metric_row(name, &cfg, w, lbl_role, theme),
        };
        lines.push(line);
    }

    frame.render_widget(Paragraph::new(lines), inner);

    // Border: clock in the right-aligned info slot.
    let clock_full = fmt_time(Some(SystemTime::now()), true); // "YYYY-MM-DD HH:MM:SS"
    let h_sep = h_rule(border_style);
    // Pick the richest clock form that fits in the info slot (area.width − 4).
    let avail = area.width.saturating_sub(4) as usize;
    let time_ss = &clock_full[11..]; // "HH:MM:SS"
    let time_mm = &clock_full[11..16]; // "HH:MM"
    // Every candidate keeps at least one trailing rule char so the info
    // bracket never sits flush against the panel's right corner — `]┐` reads
    // as a rendering glitch, not a border seam.
    let candidates = [
        format!("[ {clock_full} ]{h_sep}{h_sep}"),
        format!("[ {clock_full} ]{h_sep}"),
        format!("[ {time_ss} ]{h_sep}"),
        format!("[ {time_mm} ]{h_sep}"),
    ];
    if let Some(info) = candidates.iter().find(|s| s.chars().count() <= avail) {
        panels.push(pa.frame().info(info.clone()));
    } else {
        panels.push(pa.frame());
    }
}

// ── Monitor helpers ───────────────────────────────────────────────────────────

/// A filled-░ progress bar of `width` characters representing `ratio` ∈ [0,1].
fn mini_bar(ratio: f64, width: usize) -> String {
    let filled = (ratio.clamp(0.0, 1.0) * width as f64).round() as usize;
    format!("{}{}", "█".repeat(filled), "░".repeat(width.saturating_sub(filled)))
}

/// A block-character sparkline of `width` columns over the most recent history
/// (`▁▂▃▄▅▆▇█`), left-padded with spaces when there are fewer samples than columns.
fn block_sparkline(history: &[f64], width: usize) -> String {
    const BLOCKS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let recent = &history[history.len().saturating_sub(width)..];
    let mut s = String::new();
    for _ in 0..width.saturating_sub(recent.len()) {
        s.push(' ');
    }
    for &r in recent {
        let level = (r.clamp(0.0, 1.0) * 8.0).round() as usize;
        s.push(BLOCKS[level.min(8)]);
    }
    s
}

/// A braille sparkline of `width` characters (each cell packs two samples × four
/// dot rows) over the most recent history, left-padded with blanks.
fn braille_sparkline(history: &[f64], width: usize) -> String {
    // Per-column dot bits, bottom→top: left col [dot7,dot3,dot2,dot1], right col
    // [dot8,dot6,dot5,dot4]. A value of height h fills the bottom h dots.
    const LEFT: [u8; 4] = [0x40, 0x04, 0x02, 0x01];
    const RIGHT: [u8; 4] = [0x80, 0x20, 0x10, 0x08];
    let needed = width * 2;
    let recent = &history[history.len().saturating_sub(needed)..];
    // Left-pad the sample window so the latest sample sits at the right edge.
    let mut samples: Vec<f64> = std::iter::repeat(0.0)
        .take(needed.saturating_sub(recent.len()))
        .chain(recent.iter().copied())
        .collect();
    samples.truncate(needed);

    let height = |r: f64| (r.clamp(0.0, 1.0) * 4.0).round() as usize;
    let mut s = String::new();
    for col in 0..width {
        let lh = height(samples[col * 2]);
        let rh = height(samples[col * 2 + 1]);
        let mut bits = 0u8;
        for b in LEFT.iter().take(lh) {
            bits |= b;
        }
        for b in RIGHT.iter().take(rh) {
            bits |= b;
        }
        s.push(char::from_u32(0x2800 + bits as u32).unwrap_or(' '));
    }
    s
}

/// A muted-colored `░`-fill span of `width` cells, used to mark graph areas as
/// "not yet loaded" on startup before any samples have been collected.
fn muted_fill(width: usize, theme: &Theme) -> Span<'static> {
    Span::styled("░".repeat(width), theme.style(Role::Muted))
}

/// Placeholder row for a metric that has no data yet (startup / unsupported
/// platform). Shows the metric name in the normal label style and fills the
/// gauge area with muted `░` characters so the graph layout is visible.
fn muted_metric_row(
    name: &str,
    cfg: &MetricCfg,
    width: usize,
    label_role: Role,
    theme: &Theme,
) -> Line<'static> {
    let name_str = format!("{name} ");
    let avail = width.saturating_sub(name_str.chars().count());
    let value_w = if cfg.label { (cfg.label_max as usize).min(avail) } else { 0 };
    let sep = usize::from(value_w > 0);
    let bar_w = avail.saturating_sub(value_w + sep);

    let muted_style = theme.style(Role::Muted);
    let mut spans = vec![Span::styled(name_str, theme.style(label_role))];
    if bar_w >= 1 {
        spans.push(muted_fill(bar_w, theme));
    }
    if value_w > 0 {
        spans.push(Span::styled(" ".repeat(sep), muted_style));
        spans.push(Span::styled(fit_right("---", value_w), muted_style));
    }
    Line::from(spans)
}

/// Render one metric row honoring its `MetricCfg`: the name, an optional gauge
/// (style per `bar_kind`), and an optional value. `label_max` columns are always
/// reserved for the value at the right edge, so the gauge width stays stable
/// regardless of how the value's text length changes.
#[allow(clippy::too_many_arguments)]
fn metric_row(
    name: &str,
    cfg: &MetricCfg,
    ratio: f64,
    value: &str,
    history: &[f64],
    width: usize,
    label_role: Role,
    bar_role: Role,
    value_role: Role,
    theme: &Theme,
) -> Line<'static> {
    let name_str = format!("{name} ");
    let avail = width.saturating_sub(name_str.chars().count());
    // The value field: a fixed `label_max` columns reserved at the right edge.
    let value_w = if cfg.label { (cfg.label_max as usize).min(avail) } else { 0 };
    let sep = usize::from(value_w > 0); // one column between gauge and value
    let bar_w = avail.saturating_sub(value_w + sep);

    let val_style = Style::default().fg(cascade_fg(theme, value_role));
    let mut spans = vec![Span::styled(name_str, theme.style(label_role))];
    if bar_w >= 1 {
        if cfg.bar {
            // Sparklines auto-range to the window's min..max so movement shows
            // even for a near-constant signal; the fill bar shows the absolute ratio.
            let (mn, mx) = range(history);
            let bar = match cfg.bar_kind.as_str() {
                "blocks" => block_sparkline(&scale_minmax(history, mn, mx), bar_w),
                "braille" => braille_sparkline(&scale_minmax(history, mn, mx), bar_w),
                _ => mini_bar(ratio, bar_w), // "bar" and any unknown value
            };
            spans.push(Span::styled(bar, theme.style(bar_role)));
        } else {
            // Keep the value at the right edge even with the gauge hidden.
            spans.push(Span::styled(" ".repeat(bar_w), val_style));
        }
    }
    if value_w > 0 {
        spans.push(Span::styled(" ".repeat(sep), val_style));
        spans.push(Span::styled(fit_right(value, value_w), val_style));
    }
    Line::from(spans)
}

/// Right-align `s` in `w` columns (truncate the tail when too long, pad left).
fn fit_right(s: &str, w: usize) -> String {
    if w == 0 {
        return String::new();
    }
    let t: String = s.chars().take(w).collect();
    format!("{t:>w$}")
}

/// Left-align `s` in `w` columns (truncate the tail when too long, pad right).
fn fit_left(s: &str, w: usize) -> String {
    if w == 0 {
        return String::new();
    }
    let t: String = s.chars().take(w).collect();
    format!("{t:<w$}")
}

/// Sample one metric: its current gauge ratio (0..1) and value text. CPU/MEM may
/// be `None` on unsupported platforms; NET always yields a value. (Disk is
/// handled separately by `disk_runtime` — it shows read/write op rates.)
fn sample_metric(id: &str, _cwd: &std::path::Path) -> Option<(f64, String)> {
    match id {
        "cpu" => loadavg().map(|[la1, _, _]| ((la1 / 4.0).clamp(0.0, 1.0), format!("{la1:.2}"))),
        "mem" => mem_info().map(|(total, used)| {
            // Show available memory only (Linux MemAvailable; the free-memory
            // equivalent on macOS) — the total/max is intentionally not shown.
            // The gauge still tracks used pressure.
            let ratio = if total > 0 { used as f64 / total as f64 } else { 0.0 };
            let available = total.saturating_sub(used);
            (ratio, wasdf_sdk::format_size(available))
        }),
        "net" => {
            let (avg5, avg60) = net_stats();
            let ratio = if avg60 > 0 { (avg5 as f64 / avg60 as f64).clamp(0.0, 1.0) } else { 0.0 };
            Some((ratio, format!("{}/s", wasdf_sdk::format_size(avg5))))
        }
        _ => None,
    }
}

/// The number of samples spanning `window` seconds at the `refresh` cadence — the
/// rolling history length, so the graph's full-scale peak is the max over the
/// configured window.
fn window_samples(refresh: u64, window: u64) -> usize {
    (window / refresh.max(1)).max(1) as usize
}

/// Per-metric runtime: throttles resampling to the configured `refresh` cadence
/// and keeps a `graph_window`-long rolling ratio history for the sparkline gauges.
/// Returns the cached `(ratio, value, history)` or `None` if the metric has never
/// produced a sample.
fn metric_runtime(id: &'static str, refresh: u64, graph_window: u64, cwd: &std::path::Path)
    -> Option<(f64, String, Vec<f64>)>
{
    use std::collections::{HashMap, VecDeque};
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;

    #[derive(Default)]
    struct MetricState {
        last: Option<Instant>,
        ratio: f64,
        value: String,
        history: VecDeque<f64>,
        have: bool,
    }
    static RT: OnceLock<Mutex<HashMap<&'static str, MetricState>>> = OnceLock::new();

    let cap = window_samples(refresh, graph_window);
    let now = Instant::now();
    let mut map = RT.get_or_init(|| Mutex::new(HashMap::new())).lock().ok()?;
    let st = map.entry(id).or_default();
    let due = st.last.map(|t| now.duration_since(t).as_secs() >= refresh).unwrap_or(true);
    if due {
        if let Some((ratio, value)) = sample_metric(id, cwd) {
            st.ratio = ratio;
            st.value = value;
            st.history.push_back(ratio);
            st.last = Some(now);
            st.have = true;
        } else if !st.have {
            return None;
        }
    }
    while st.history.len() > cap {
        st.history.pop_front();
    }
    if !st.have {
        return None;
    }
    Some((st.ratio, st.value.clone(), st.history.iter().copied().collect()))
}

/// `Fg`-cascade: if the specific role resolves to Reset, fall back to the
/// global `Fg` role, which itself defaults to Reset (terminal default).
#[inline]
fn cascade_fg(theme: &Theme, role: Role) -> ratatui::style::Color {
    let c = theme.role(role);
    if c == ratatui::style::Color::Reset { theme.role(Role::Base) } else { c }
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

/// Cumulative (reads, writes) operation counts across all connected block
/// devices, via IOKit's `IOBlockStorageDriver` statistics.
#[cfg(target_os = "macos")]
fn disk_ops_total() -> (u64, u64) {
    use std::ffi::c_void;
    type CFRef = *const c_void;
    type IoObject = u32;
    const KERN_SUCCESS: i32 = 0;
    const UTF8: u32 = 0x0800_0100;
    const SINT64: i32 = 4;

    #[link(name = "IOKit", kind = "framework")]
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn IOServiceMatching(name: *const libc::c_char) -> CFRef;
        fn IOServiceGetMatchingServices(port: u32, matching: CFRef, iter: *mut IoObject) -> i32;
        fn IOIteratorNext(iter: IoObject) -> IoObject;
        fn IOObjectRelease(obj: IoObject) -> i32;
        fn IORegistryEntryCreateCFProperties(
            entry: IoObject, props: *mut CFRef, allocator: CFRef, options: u32,
        ) -> i32;
        fn CFStringCreateWithCString(alloc: CFRef, s: *const libc::c_char, enc: u32) -> CFRef;
        fn CFDictionaryGetValue(dict: CFRef, key: CFRef) -> CFRef;
        fn CFNumberGetValue(num: CFRef, ty: i32, value: *mut c_void) -> u8;
        fn CFRelease(cf: CFRef);
    }

    let (mut reads, mut writes) = (0u64, 0u64);
    unsafe {
        let matching = IOServiceMatching(c"IOBlockStorageDriver".as_ptr());
        if matching.is_null() {
            return (0, 0);
        }
        let mut iter: IoObject = 0;
        if IOServiceGetMatchingServices(0, matching, &mut iter) != KERN_SUCCESS {
            return (0, 0);
        }
        let null: CFRef = std::ptr::null();
        let k_stats = CFStringCreateWithCString(null, c"Statistics".as_ptr(), UTF8);
        let k_read = CFStringCreateWithCString(null, c"Operations (Read)".as_ptr(), UTF8);
        let k_write = CFStringCreateWithCString(null, c"Operations (Write)".as_ptr(), UTF8);
        loop {
            let entry = IOIteratorNext(iter);
            if entry == 0 {
                break;
            }
            let mut props: CFRef = std::ptr::null();
            if IORegistryEntryCreateCFProperties(entry, &mut props, null, 0) == KERN_SUCCESS
                && !props.is_null()
            {
                let stats = CFDictionaryGetValue(props, k_stats);
                if !stats.is_null() {
                    let mut v: i64 = 0;
                    let r = CFDictionaryGetValue(stats, k_read);
                    if !r.is_null() && CFNumberGetValue(r, SINT64, &mut v as *mut i64 as *mut c_void) != 0 {
                        reads += v as u64;
                    }
                    v = 0;
                    let wv = CFDictionaryGetValue(stats, k_write);
                    if !wv.is_null() && CFNumberGetValue(wv, SINT64, &mut v as *mut i64 as *mut c_void) != 0 {
                        writes += v as u64;
                    }
                }
                CFRelease(props);
            }
            IOObjectRelease(entry);
        }
        CFRelease(k_stats);
        CFRelease(k_read);
        CFRelease(k_write);
        IOObjectRelease(iter);
    }
    (reads, writes)
}

/// Cumulative (reads, writes) completed across all whole block devices, from
/// `/proc/diskstats` (field 4 = reads completed, field 8 = writes completed).
#[cfg(target_os = "linux")]
fn disk_ops_total() -> (u64, u64) {
    let (mut reads, mut writes) = (0u64, 0u64);
    if let Ok(content) = std::fs::read_to_string("/proc/diskstats") {
        for line in content.lines() {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() < 8 {
                continue;
            }
            let name = f[2];
            if name.starts_with("loop") || name.starts_with("ram") || is_partition(name) {
                continue;
            }
            reads += f[3].parse::<u64>().unwrap_or(0);
            writes += f[7].parse::<u64>().unwrap_or(0);
        }
    }
    (reads, writes)
}

/// Heuristic: is this `/proc/diskstats` device name a partition (skip, to avoid
/// double-counting its whole disk)? e.g. `sda1`, `nvme0n1p2`, `mmcblk0p1`.
#[cfg(target_os = "linux")]
fn is_partition(name: &str) -> bool {
    let ends_digit = name.chars().last().map(|c| c.is_ascii_digit()).unwrap_or(false);
    if !ends_digit {
        return false;
    }
    // nvme/mmcblk whole disks end in a digit (nvme0n1); their partitions carry a `p`.
    if name.starts_with("nvme") || name.starts_with("mmcblk") {
        return name.contains('p');
    }
    // sd*/hd*/vd*/xvd*: a trailing digit marks a partition (sda1).
    true
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn disk_ops_total() -> (u64, u64) {
    (0, 0)
}

/// Compact count formatter for op rates: `999`, `1K`, `12M`.
fn fmt_count(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{}M", n / 1_000_000)
    } else if n >= 1_000 {
        format!("{}K", n / 1_000)
    } else {
        n.to_string()
    }
}

/// Disk read/write op-rate sample: current per-second rates and their rolling
/// rate histories (for the sparkline gauges).
struct DiskSample {
    r_rate: f64,
    w_rate: f64,
    r_hist: Vec<f64>,
    w_hist: Vec<f64>,
}

/// Per-disk runtime: throttles op-counter sampling to `refresh`, diffs the
/// cumulative counters into read/write rates (ops/sec), and keeps rolling
/// histories for the gauges.
fn disk_runtime(refresh: u64, graph_window: u64) -> DiskSample {
    use std::collections::VecDeque;
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;

    #[derive(Default)]
    struct DiskState {
        last: Option<Instant>,
        prev: Option<(u64, u64)>,
        r_rate: f64,
        w_rate: f64,
        r_hist: VecDeque<f64>,
        w_hist: VecDeque<f64>,
    }
    static RT: OnceLock<Mutex<DiskState>> = OnceLock::new();

    let now = Instant::now();
    let st_mutex = RT.get_or_init(|| Mutex::new(DiskState::default()));
    let Ok(mut st) = st_mutex.lock() else {
        return DiskSample { r_rate: 0.0, w_rate: 0.0, r_hist: vec![], w_hist: vec![] };
    };
    let due = st.last.map(|t| now.duration_since(t).as_secs() >= refresh).unwrap_or(true);
    if due {
        let (reads, writes) = disk_ops_total();
        if let Some((pr, pw)) = st.prev {
            let dt = st.last.map(|t| now.duration_since(t).as_secs_f64()).unwrap_or(0.0);
            if dt > 0.0 {
                let r = reads.saturating_sub(pr) as f64 / dt;
                let w = writes.saturating_sub(pw) as f64 / dt;
                st.r_rate = r;
                st.w_rate = w;
                st.r_hist.push_back(r);
                st.w_hist.push_back(w);
            }
        }
        st.prev = Some((reads, writes));
        st.last = Some(now);
    }
    let cap = window_samples(refresh, graph_window);
    while st.r_hist.len() > cap {
        st.r_hist.pop_front();
    }
    while st.w_hist.len() > cap {
        st.w_hist.pop_front();
    }
    DiskSample {
        r_rate: st.r_rate,
        w_rate: st.w_rate,
        r_hist: st.r_hist.iter().copied().collect(),
        w_hist: st.w_hist.iter().copied().collect(),
    }
}

/// Render one gauge of `width` chars per the `bar_kind`. `cur` is the current
/// ratio (for the fill bar); `hist` is the normalized history (for sparklines).
fn gauge_str(bar_kind: &str, cur: f64, hist: &[f64], width: usize) -> String {
    match bar_kind {
        "blocks" => block_sparkline(hist, width),
        "braille" => braille_sparkline(hist, width),
        _ => mini_bar(cur, width),
    }
}

/// The `(min, max)` of a history window. Empty → a degenerate range that
/// `scale_minmax` / `lerp_minmax` treat as flat.
fn range(hist: &[f64]) -> (f64, f64) {
    hist.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(mn, mx), &v| (mn.min(v), mx.max(v)))
}

/// Auto-range a history to `0..1` over `[mn, mx]`: the window trough maps to empty
/// and the window peak to full, so a graph shows movement even when the absolute
/// signal is nearly constant (e.g. memory pressure). A flat window → all zero.
fn scale_minmax(hist: &[f64], mn: f64, mx: f64) -> Vec<f64> {
    let span = mx - mn;
    if !span.is_finite() || span <= 1e-9 {
        return vec![0.0; hist.len()];
    }
    hist.iter().map(|v| ((v - mn) / span).clamp(0.0, 1.0)).collect()
}

/// Auto-range a single value to `0..1` over `[mn, mx]` (the current-sample
/// counterpart of `scale_minmax`, for the fill-bar gauge).
fn lerp_minmax(v: f64, mn: f64, mx: f64) -> f64 {
    let span = mx - mn;
    if !span.is_finite() || span <= 1e-9 {
        0.0
    } else {
        ((v - mn) / span).clamp(0.0, 1.0)
    }
}

/// The disk row: `DSK R<gauge>W<gauge> r:w`. `label_max` columns are reserved at
/// the right for the value, whose `:` is centered so its column does not shift as
/// the read/write counts (`r`/`w`) change width. The two gauges share the rest.
#[allow(clippy::too_many_arguments)]
fn disk_row(
    cfg: &MetricCfg,
    ds: &DiskSample,
    width: usize,
    label_role: Role,
    bar_role: Role,
    value_role: Role,
    theme: &Theme,
) -> Line<'static> {
    let name_str = "DSK ".to_string();
    let avail = width.saturating_sub(name_str.chars().count());
    // Fixed value field at the right edge; `:` centered within it.
    let value_w = if cfg.label { (cfg.label_max as usize).min(avail) } else { 0 };
    let sep = usize::from(value_w > 0);
    let gauge_area = avail.saturating_sub(value_w + sep);

    let lbl = theme.style(label_role);
    let bar = theme.style(bar_role);
    let val = Style::default().fg(cascade_fg(theme, value_role));
    let mut spans = vec![Span::styled(name_str, lbl)];

    // The gauge area holds: "R" + gauge + " W" + gauge → 3 decoration chars (a
    // space precedes W so the read gauge does not run into the W label).
    let drew_gauges = if cfg.bar && gauge_area >= 5 {
        let each = (gauge_area - 3) / 2;
        spans.push(Span::styled("R", lbl));
        if ds.r_hist.is_empty() {
            // No rate samples yet (first render): show muted fill on both gauges.
            spans.push(muted_fill(each, theme));
            spans.push(Span::styled(" W", lbl));
            spans.push(muted_fill(each, theme));
        } else {
            // Shared auto-range across both series over the graph window.
            let (rmn, rmx) = range(&ds.r_hist);
            let (wmn, wmx) = range(&ds.w_hist);
            let (mn, mx) = (rmn.min(wmn), rmx.max(wmx));
            let r_g = gauge_str(&cfg.bar_kind, lerp_minmax(ds.r_rate, mn, mx), &scale_minmax(&ds.r_hist, mn, mx), each);
            let w_g = gauge_str(&cfg.bar_kind, lerp_minmax(ds.w_rate, mn, mx), &scale_minmax(&ds.w_hist, mn, mx), each);
            spans.push(Span::styled(r_g, bar));
            spans.push(Span::styled(" W", lbl));
            spans.push(Span::styled(w_g, bar));
        }
        true
    } else {
        false
    };
    if value_w > 0 {
        // Keep the value at the right edge even if the gauges were not drawn.
        if !drew_gauges && gauge_area >= 1 {
            spans.push(Span::styled(" ".repeat(gauge_area), val));
        }
        spans.push(Span::styled(" ".repeat(sep), val));
        spans.push(Span::styled(
            disk_value(ds.r_rate as u64, ds.w_rate as u64, value_w),
            val,
        ));
    }
    Line::from(spans)
}

/// A fixed-width `r:w` field of `width` columns with the colon centered: the read
/// count is right-aligned in the left half, the write count left-aligned in the
/// right half, so the colon's column is stable regardless of the digit counts.
fn disk_value(reads: u64, writes: u64, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return ":".into();
    }
    let left = (width - 1) / 2;
    let right = width - 1 - left;
    format!("{}:{}", fit_right(&fmt_count(reads), left), fit_left(&fmt_count(writes), right))
}

/// Cumulative bytes (rx + tx) across all UP non-loopback interfaces.
#[cfg(target_os = "macos")]
fn net_total_bytes() -> u64 {
    let mut total: u64 = 0;
    unsafe {
        let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut ifap) != 0 || ifap.is_null() {
            return 0;
        }
        let mut ifa = ifap;
        while !ifa.is_null() {
            let flags = (*ifa).ifa_flags as u32;
            let up = flags & (libc::IFF_UP as u32) != 0;
            let loopback = flags & (libc::IFF_LOOPBACK as u32) != 0;
            let addr = (*ifa).ifa_addr;
            // Link-level rows (AF_LINK) carry the per-interface byte counters in
            // their ifa_data as a `struct if_data`.
            if up && !loopback && !addr.is_null()
                && (*addr).sa_family as i32 == libc::AF_LINK
                && !(*ifa).ifa_data.is_null()
            {
                let data = (*ifa).ifa_data as *const libc::if_data;
                total += (*data).ifi_ibytes as u64 + (*data).ifi_obytes as u64;
            }
            ifa = (*ifa).ifa_next;
        }
        libc::freeifaddrs(ifap);
    }
    total
}

/// Cumulative bytes (rx + tx) across all non-loopback interfaces, from
/// `/proc/net/dev`.
#[cfg(target_os = "linux")]
fn net_total_bytes() -> u64 {
    let mut total: u64 = 0;
    if let Ok(content) = std::fs::read_to_string("/proc/net/dev") {
        for line in content.lines() {
            let Some((name, rest)) = line.split_once(':') else { continue };
            if name.trim() == "lo" { continue; }
            let cols: Vec<&str> = rest.split_whitespace().collect();
            // rx bytes = column 0, tx bytes = column 8.
            let rx = cols.first().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
            let tx = cols.get(8).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
            total += rx + tx;
        }
    }
    total
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn net_total_bytes() -> u64 {
    0
}

/// Network throughput stats, summed over all non-loopback devices: the average
/// byte/second rate over the last ~5 seconds and over the last ~60 seconds.
/// Keeps a rolling 60-second history of cumulative-byte samples, one per frame.
fn net_stats() -> (u64, u64) {
    use std::collections::VecDeque;
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;
    static HIST: OnceLock<Mutex<VecDeque<(Instant, u64)>>> = OnceLock::new();

    let total = net_total_bytes();
    let now = Instant::now();
    let Ok(mut hist) = HIST.get_or_init(|| Mutex::new(VecDeque::new())).lock() else {
        return (0, 0);
    };
    hist.push_back((now, total));
    // Drop samples older than the 60-second window.
    while let Some(&(t, _)) = hist.front() {
        if now.duration_since(t).as_secs_f64() > 60.0 {
            hist.pop_front();
        } else {
            break;
        }
    }
    (window_rate(&hist, now, total, 5.0), window_rate(&hist, now, total, 60.0))
}

/// Average byte/second rate over roughly the last `min_age` seconds: diff the
/// current cumulative byte count against the newest sample at least `min_age`
/// old (or the oldest available if the history is shorter than the window).
fn window_rate(
    hist: &std::collections::VecDeque<(std::time::Instant, u64)>,
    now: std::time::Instant,
    total: u64,
    min_age: f64,
) -> u64 {
    let mut base: Option<&(std::time::Instant, u64)> = None;
    for s in hist.iter() {
        if now.duration_since(s.0).as_secs_f64() >= min_age {
            base = Some(s);
        } else {
            break;
        }
    }
    match base.or_else(|| hist.front()) {
        Some(&(t0, b0)) => {
            let dt = now.duration_since(t0).as_secs_f64();
            if dt > 0.0 { (total.saturating_sub(b0) as f64 / dt) as u64 } else { 0 }
        }
        None => 0,
    }
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

/// `LABEL: value` line, value truncated to fit `width`.
fn prop_line(
    label: &str,
    value: &str,
    width: usize,
    label_role: Role,
    value_role: Role,
    theme: &Theme,
) -> Line<'static> {
    let prefix = format!("{label} ");
    let avail = width.saturating_sub(prefix.chars().count());
    let value: String = if value.chars().count() > avail && avail > 1 {
        format!("{}…", value.chars().take(avail - 1).collect::<String>())
    } else {
        value.to_string()
    };
    Line::from(vec![
        Span::styled(prefix, theme.style(label_role)),
        Span::styled(value, Style::default().fg(cascade_fg(theme, value_role))),
    ])
}

/// The plain username for a uid via `getpwuid`, or `None` if unresolvable —
/// the lookup `uid_name` (this module's `name (uid)` prop-panel format) and
/// `ui::middle`'s compact Rows-layout OWNR column both build on.
pub(crate) fn username(uid: u32) -> Option<String> {
    unsafe {
        let pw = libc::getpwuid(uid as libc::uid_t);
        if pw.is_null() {
            return None;
        }
        let s = std::ffi::CStr::from_ptr((*pw).pw_name).to_str().ok()?;
        (!s.is_empty()).then(|| s.to_string())
    }
}

/// Resolve a uid to `name (uid)`, or just `uid` if the name is unavailable.
fn uid_name(uid: u32) -> String {
    username(uid).map(|s| format!("{s} ({uid})")).unwrap_or_else(|| uid.to_string())
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
pub(crate) fn fmt_time(t: Option<SystemTime>, full: bool) -> String {
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
