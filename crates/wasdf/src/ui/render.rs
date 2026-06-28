//! The frame compositor: the top-level `render` entry, the shared layout split
//! helpers, and the full-screen overlays (notice and Policy confirm). The three
//! row bands it lays out are rendered by their own modules — `top` (the top
//! panel), `middle` (the main file/select area), and `bottom` (the help row).
//!
//! Panels are drawn as borderless content into their inner rects; the connected
//! single-line border, titles, and right-edge scrollbars are overlaid by the
//! chrome layer (see chrome.rs and doc/UI.md). Panel rects overlap by one cell
//! on shared edges so neighbouring borders coincide into one line with proper
//! junctions.

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::core::{AppState, Mode, Notice, Role, Theme};
use crate::extension::ExtensionRegistry;
use crate::ui::chrome::{self, PanelFrame};
use crate::ui::middle::ScrollMemory;

fn top_rows(height: u16) -> u16 {
    if height >= 25 { 7 } else { 5 }
}

/// Split a rect into left and right sharing the divider column.
pub(crate) fn split_h(r: Rect, left_w: u16) -> (Rect, Rect) {
    let left_w = left_w.clamp(2, r.width.saturating_sub(2).max(2));
    let sx = r.x + left_w - 1;
    let left = Rect { x: r.x, y: r.y, width: sx - r.x + 1, height: r.height };
    let right = Rect { x: sx, y: r.y, width: r.x + r.width - sx, height: r.height };
    (left, right)
}

/// Split a rect into top and bottom sharing the divider row.
pub(crate) fn split_v(r: Rect, top_h: u16) -> (Rect, Rect) {
    let top_h = top_h.clamp(2, r.height.saturating_sub(2).max(2));
    let sy = r.y + top_h - 1;
    let top = Rect { x: r.x, y: r.y, width: r.width, height: sy - r.y + 1 };
    let bot = Rect { x: r.x, y: sy, width: r.width, height: r.y + r.height - sy };
    (top, bot)
}

pub(crate) fn ratio_left(width: u16, lw: u32, rw: u32) -> u16 {
    ((width as u32 * lw) / (lw + rw)) as u16
}

/// Pre-compute the absolute X column of the function panel's left border for the
/// current frame so `render_top` can place the bottom border path/filename labels.
/// Returns `None` when the function panel is not visible this frame.
fn compute_func_x(state: &AppState, main: Rect) -> Option<u16> {
    use crate::core::Mode;
    let show = match state.mode() {
        Mode::Select(spec) => spec.function_hint.is_some(),
        _ => state.function.visible,
    };
    if !show {
        return None;
    }
    let (lw, rw) = state.function.ratio.weights();
    // Replicate split_h's clamped left_w to find the shared column.
    let target = ratio_left(main.width, lw, rw);
    let left_w = target.clamp(2, main.width.saturating_sub(2).max(2));
    Some(main.x + left_w - 1)
}

pub fn render(
    frame: &mut Frame,
    state: &AppState,
    extensions: &ExtensionRegistry,
    scroll: &mut ScrollMemory,
    notice: Option<&Notice>,
    vfs: &crate::vfs::Vfs,
    opts: &crate::ui::top::TopOptions,
    theme: &Theme,
) {
    let area = frame.area();
    if area.width < 4 || area.height < 4 {
        return;
    }

    // Global base colors: fg/bg paint the entire frame — including the cells
    // behind the border and every panel — and are overridden per-cell wherever
    // a more specific role sets its own fg/bg. Applied first so everything that
    // follows composes on top of it.
    frame.buffer_mut().set_style(
        area,
        Style::default().fg(theme.role(Role::Fg)).bg(theme.role(Role::Bg)),
    );

    let tr = top_rows(area.height);
    // help on the last row (unboxed); the box spans the rest. Top and main
    // overlap by one row so their borders share a single divider line.
    let help_y = area.y + area.height - 1;
    let top = Rect { x: area.x, y: area.y, width: area.width, height: tr };
    let main = Rect {
        x: area.x,
        y: area.y + tr - 1,
        width: area.width,
        height: area.height.saturating_sub(tr),
    };
    let help = Rect { x: area.x, y: help_y, width: area.width, height: 1 };

    // Default to "no clamp"; the function-panel render records real bounds only
    // when it draws a scrollable frame this pass.
    scroll.reset_function_bounds();
    let mut panels: Vec<PanelFrame> = Vec::new();
    let func_x = compute_func_x(state, main);
    crate::ui::top::render_top(frame, top, state, vfs, &mut panels, opts, func_x, theme);
    match state.mode() {
        Mode::Select(_) => crate::ui::middle::render_select_main(frame, main, state, extensions, scroll, &mut panels, theme),
        _ => crate::ui::middle::render_file_main(frame, main, state, extensions, scroll, &mut panels, theme),
    }

    chrome::render(frame, &panels, &opts.border_style, &opts.scrollbar, theme);
    crate::ui::bottom::render_bottom(frame, help, state, theme);

    if let Some(n) = notice {
        render_notice(frame, area, n, theme);
    }
    if let Mode::Policy(pending) = state.mode() {
        render_policy(frame, area, pending, theme);
    }
}

fn render_notice(frame: &mut Frame, area: Rect, notice: &Notice, theme: &Theme) {
    let style = if notice.error {
        Style::default().fg(theme.role(Role::NoticeErrorFg)).bg(theme.role(Role::NoticeErrorBg))
    } else {
        Style::default().fg(theme.role(Role::NoticeInfoFg)).bg(theme.role(Role::NoticeInfoBg))
    };
    let w = (notice.text.len() as u16 + 2).min(area.width);
    let rect = Rect { x: area.width.saturating_sub(w), y: 0, width: w, height: 1 };
    frame.render_widget(Clear, rect);
    frame.render_widget(Paragraph::new(format!(" {} ", notice.text)).style(style), rect);
}

fn render_policy(frame: &mut Frame, area: Rect, pending: &crate::core::Intent, theme: &Theme) {
    let summary = match pending {
        crate::core::Intent::RunResolver(r) => format!("{} {} item(s)?", r.op, r.paths.len()),
        crate::core::Intent::Extension(e) => format!("Run {}:{}?", e.extension, e.intent),
        _ => "Confirm operation?".into(),
    };
    // Wide enough for the longer of the summary and the key-hint line.
    let keys = "y / Enter : confirm     n / Esc : cancel";
    let body = summary.len().max(keys.len()) as u16;
    let w = (body + 6).min(area.width.saturating_sub(2)).max(20);
    // Four body lines (blank, summary, blank, key hints) plus the two borders.
    let h = 6;
    let rect = Rect {
        x: (area.width.saturating_sub(w)) / 2,
        y: (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };
    frame.render_widget(Clear, rect);
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(summary, Style::default().add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from(Span::styled(keys, Style::default().fg(Color::DarkGray))),
    ];
    let overlay_style = Style::default()
        .fg(theme.role(Role::OverlayDangerFg))
        .bg(theme.role(Role::OverlayDangerBg));
    let p = Paragraph::new(lines).alignment(Alignment::Center).style(overlay_style).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(overlay_style)
            .title("Confirm"),
    );
    frame.render_widget(p, rect);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::AppState;
    use crate::extension::ExtensionRegistry;
    use crate::ui::top::TopOptions;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn top_opts() -> TopOptions {
        TopOptions::default()
    }

    fn exts() -> ExtensionRegistry {
        let mut e = ExtensionRegistry::new();
        for x in crate::extension::bundled() {
            e.register(x);
        }
        e
    }

    fn vfs() -> crate::vfs::Vfs {
        crate::vfs::Vfs::default()
    }

    fn theme() -> crate::core::Theme {
        crate::core::Theme::default()
    }

    #[test]
    fn image_content_paints_truecolor_cells() {
        // Hand a real PNG to the content provider, then render through its hook.
        let mut path = std::env::temp_dir();
        path.push(format!("wasdf-render-img-{}.png", std::process::id()));
        let img = image::RgbImage::from_fn(8, 8, |_, y| image::Rgb([(y * 30) as u8, 0, 200]));
        img.save(&path).unwrap();
        let bytes = std::fs::read(&path).unwrap();

        let exts = exts();
        exts.provider()
            .unwrap()
            .accept_content(&path, &crate::core::ReadResult::Bytes { offset: 0, bytes, eof: true });

        let mut state = AppState::new(std::env::temp_dir());
        state.function.visible = true;
        let mut terminal = Terminal::new(TestBackend::new(80, 30)).unwrap();
        terminal.draw(|f| render(f, &state, &exts, &mut ScrollMemory::default(), None, &vfs(), &top_opts(), &theme())).unwrap();
        let buf = terminal.backend().buffer();
        let rgb_cells = buf.content.iter().filter(|c| matches!(c.fg, Color::Rgb(..))).count();
        assert!(rgb_cells > 0, "image content rendered no truecolor cells");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn function_panel_renders_extension_content() {
        use crate::core::PanelContent;
        let mut state = AppState::new(std::env::temp_dir());
        state.function.visible = true;
        state.function.content =
            Some(PanelContent::Lines { lines: vec!["EXTLINE".into()], styles: vec![Vec::new()] });
        state.function.content_owner = Some("example".into());
        let mut t = Terminal::new(TestBackend::new(80, 24)).unwrap();
        t.draw(|f| render(f, &state, &exts(), &mut ScrollMemory::default(), None, &vfs(), &top_opts(), &theme())).unwrap();
        let b = t.backend().buffer();
        let text: String = (0..b.area.height)
            .flat_map(|y| (0..b.area.width).map(move |x| (x, y)))
            .map(|(x, y)| b[(x, y)].symbol().to_string())
            .collect();
        assert!(text.contains("EXTLINE"), "extension content shown in the function panel");
        assert!(text.contains("example"), "panel titled with the owning extension id");
    }

    #[test]
    fn extension_content_shows_search_highlight() {
        use crate::core::PanelContent;
        let mut state = AppState::new(std::env::temp_dir());
        state.function.visible = true;
        state.function.content =
            Some(PanelContent::Lines { lines: vec!["foobar".into()], styles: vec![Vec::new()] });
        state.function.content_owner = Some("example".into());
        state.function.search.matches = vec![(0, 0, 3)]; // "foo"
        state.function.search.current = 0;
        let mut t = Terminal::new(TestBackend::new(80, 24)).unwrap();
        t.draw(|f| render(f, &state, &exts(), &mut ScrollMemory::default(), None, &vfs(), &top_opts(), &theme())).unwrap();
        let buf = t.backend().buffer();
        // The current match renders on a LightCyan background (unique to the
        // search highlight; the nav uses plain Cyan).
        let hits = buf.content.iter().filter(|c| c.bg == Color::LightCyan).count();
        assert!(hits >= 3, "the 3-char match 'foo' is highlighted on ext content (got {hits})");
    }

    #[test]
    fn function_panel_scroll_is_bounded_against_overscroll() {
        use crate::core::PanelContent;
        // 100 lines of pushed content, the scroll cranked far past the end.
        let mut state = AppState::new(std::env::temp_dir());
        state.function.visible = true;
        let lines: Vec<String> = (0..100).map(|i| format!("line{i}")).collect();
        let styles = vec![Vec::new(); 100];
        state.function.content = Some(PanelContent::Lines { lines, styles });
        state.function.content_owner = Some("example".into());
        state.function.scroll = 9999;

        let mut mem = ScrollMemory::default();
        let mut t = Terminal::new(TestBackend::new(80, 30)).unwrap();
        t.draw(|f| render(f, &state, &exts(), &mut mem, None, &vfs(), &top_opts(), &theme())).unwrap();

        // The render measured a finite bound (content rows − viewport rows), well
        // below the over-scrolled value; the kernel's post-draw clamp pins to it so
        // the last row rests at the viewport bottom rather than scrolling into blank.
        let fmax = mem.function_max();
        assert!(fmax > 0, "100 lines exceed the viewport, expected a real bound");
        assert!(fmax < 9999, "bound should discard the over-scroll, got {fmax}");
        assert_eq!(state.function.scroll.min(fmax), fmax, "scroll clamps down to the bound");
    }

    #[test]
    fn function_panel_hscroll_is_bounded_against_overscroll() {
        use crate::core::PanelContent;
        // One very wide line, h-scroll cranked far past the right edge.
        let mut state = AppState::new(std::env::temp_dir());
        state.function.visible = true;
        let wide = "x".repeat(300);
        state.function.content =
            Some(PanelContent::Lines { lines: vec![wide], styles: vec![Vec::new()] });
        state.function.content_owner = Some("example".into());
        state.function.hscroll = 9999;

        let mut mem = ScrollMemory::default();
        let mut t = Terminal::new(TestBackend::new(80, 30)).unwrap();
        t.draw(|f| render(f, &state, &exts(), &mut mem, None, &vfs(), &top_opts(), &theme())).unwrap();

        // The render measured a finite horizontal bound (content cols − viewport
        // cols); the kernel's post-draw clamp pins hscroll to it so the line never
        // scrolls off into blank columns.
        let fhmax = mem.function_hmax();
        assert!(fhmax > 0, "a 300-col line exceeds the viewport, expected a real bound");
        assert!(fhmax < 9999, "bound should discard the over-scroll, got {fhmax}");
        assert_eq!(state.function.hscroll.min(fhmax), fhmax, "hscroll clamps down to the bound");
    }

    #[test]
    fn grid_layout_packs_entries_and_reports_geometry() {
        use crate::core::{Entry, ListLayout};
        let mut state = AppState::new(std::env::temp_dir());
        state.list_layout = ListLayout::Grid;
        state.entries = (0..12)
            .map(|i| Entry {
                path: std::path::PathBuf::from(format!("/d/f{i}")),
                name: format!("f{i}"),
                is_dir: false,
                is_symlink: false,
                size: 0,
                mode: 0,
                uid: 0,
                gid: 0,
                modified: None,
                created: None,
                accessed: None,
                symlink_target: None,
                activation: None,
            })
            .collect();
        state.cursor = 0;
        let mut mem = ScrollMemory::default();
        let mut t = Terminal::new(TestBackend::new(80, 30)).unwrap();
        t.draw(|f| render(f, &state, &exts(), &mut mem, None, &vfs(), &top_opts(), &theme())).unwrap();
        // End-to-end: the render path measures multi-column geometry and reports
        // it back through list_geom() (what the kernel copies into AppState).
        let geom = mem.list_geom();
        assert!(geom.cols > 1, "grid lays entries across multiple columns, got {}", geom.cols);
        assert!(geom.cols * geom.rows >= 12, "geometry covers all entries");
    }

    #[test]
    fn confirm_overlay_shows_title_and_key_hints() {
        use crate::core::{Intent, Mode, ResolverRequest};
        let mut state = AppState::new(std::env::temp_dir());
        state.push_mode(Mode::Policy(Box::new(Intent::RunResolver(ResolverRequest {
            op: "delete".into(),
            src: None,
            dst: None,
            path: None,
            paths: vec![std::path::PathBuf::from("/a")],
            opts: Vec::new(),
            label: "delete".into(),
        }))));
        let mut t = Terminal::new(TestBackend::new(80, 30)).unwrap();
        t.draw(|f| render(f, &state, &exts(), &mut ScrollMemory::default(), None, &vfs(), &top_opts(), &theme())).unwrap();
        let buf = t.backend().buffer();
        let text: String = (0..buf.area.height)
            .flat_map(|y| (0..buf.area.width).map(move |x| (x, y)))
            .map(|(x, y)| buf[(x, y)].symbol().to_string())
            .collect();
        assert!(text.contains("Confirm"), "overlay carries the Confirm title");
        assert!(text.contains("confirm") && text.contains("cancel"), "key hints are rendered inside the box");
    }

    #[test]
    fn bottom_border_padding_follows_border_style() {
        // The padding/separators woven into the bottom border must match the active
        // border style: a `rounded` frame uses `─`, never the double `═`.
        use crate::core::Entry;
        let cwd = std::path::PathBuf::from("/Users/szk/src/app");
        let mut state = AppState::new(cwd.clone());
        state.entries = vec![Entry {
            path: cwd.join("kernel.rs"),
            name: "kernel.rs".into(),
            is_dir: false,
            is_symlink: false,
            size: 0,
            mode: 0,
            uid: 0,
            gid: 0,
            modified: None,
            created: None,
            accessed: None,
            symlink_target: None,
            activation: None,
        }];
        state.cursor = 0;
        state.function.visible = true;
        let opts = TopOptions { border_style: "rounded".into(), ..TopOptions::default() };
        let mut t = Terminal::new(TestBackend::new(90, 14)).unwrap();
        t.draw(|f| render(f, &state, &exts(), &mut ScrollMemory::default(), None, &vfs(), &opts, &theme())).unwrap();
        let b = t.backend().buffer();
        let y = 4; // divider row
        let row: String = (0..b.area.width).map(|x| b[(x, y)].symbol()).collect();
        assert!(!row.contains('═'), "rounded style must not emit double-rule `═`: {row:?}");
        assert!(row.contains('─'), "rounded divider uses the single rule `─`: {row:?}");
        assert!(row.contains("[ kernel.rs ]"), "filename still present: {row:?}");
    }

    #[test]
    fn divider_separator_never_bleeds_function_title() {
        // The function panel's title shares the divider row with the path/filename.
        // When the `═` separator between the path and the filename lands exactly on
        // the nav↔prop `┴` junction, the rule character yields to the junction —
        // and the function title underneath must NOT show through (it used to bleed,
        // e.g. `[ debug ]e[ examples ]`). The cell must carry the junction glyph.
        use crate::core::{Entry, ReadResult};
        use crate::core::state::Ratio;
        let cwd = std::path::PathBuf::from("/Users/szk/src/github/wasdf/target/debug");
        let mk = |n: &str| Entry {
            path: cwd.join(n),
            name: n.into(),
            is_dir: true,
            is_symlink: false,
            size: 64,
            mode: 0o40755,
            uid: 501,
            gid: 20,
            modified: None,
            created: None,
            accessed: None,
            symlink_target: None,
            activation: None,
        };
        let mut state = AppState::new(cwd.clone());
        state.entries = vec![mk("build"), mk("deps"), mk("examples"), mk("incremental")];
        state.cursor = 2; // examples — its title equals the filename
        state.function.visible = true;
        let exts = exts();
        exts.provider()
            .unwrap()
            .accept_content(&cwd.join("examples"), &ReadResult::Dir { entries: vec![] });

        for ratio in [Ratio::R2_1, Ratio::R1_1, Ratio::R1_2] {
            state.function.ratio = ratio;
            for w in 78u16..=96 {
                let mut t = Terminal::new(TestBackend::new(w, 14)).unwrap();
                t.draw(|f| render(f, &state, &exts, &mut ScrollMemory::default(), None, &vfs(), &top_opts(), &theme())).unwrap();
                let b = t.backend().buffer();
                let row: String = (0..b.area.width).map(|x| b[(x, 4)].symbol()).collect();
                // The label `[ examples ]` must always be intact, and there must be
                // no stray title letter wedged just before it (the separator slot is
                // either a rule char or a junction glyph, never a letter).
                assert!(row.contains("[ examples ]"), "{ratio:?} w={w}: filename broken: {row:?}");
                if let Some(i) = row.find("[ examples ]") {
                    // The char just before the label must be the path's `]`, a rule,
                    // or a junction glyph — never a stray letter bled from the title.
                    let prev = row[..i].chars().last().unwrap();
                    assert!(
                        matches!(prev, '═' | '─' | '-' | '┬' | '┴' | '┼' | '╦' | '╩' | '╬' | '┤' | '╣' | ']'),
                        "{ratio:?} w={w}: stray '{prev}' bled before the filename: {row:?}",
                    );
                }
            }
        }
    }

    #[test]
    fn divider_filename_survives_junction_overlap() {
        // Regression: the bottom-border filename shares its row with the function
        // panel's title (the shared top↔main divider). Where a top-area divider
        // (`┴`/`╩`) lands inside the filename, the title used to bleed through —
        // e.g. "command.scm" rendered as "comeand.scm" (the `e` from "[ content ]").
        // The label's real characters must win over both the junction glyph and any
        // bled title, so the filename stays intact at every width that fits it.
        use crate::core::Entry;
        let mut state = AppState::new(std::path::PathBuf::from(
            "/Users/szk/src/github/wasdf/crates/wasdf/src/app",
        ));
        state.entries = vec![Entry {
            path: std::path::PathBuf::from(
                "/Users/szk/src/github/wasdf/crates/wasdf/src/app/command.scm",
            ),
            name: "command.scm".into(),
            is_dir: false,
            is_symlink: false,
            size: 0,
            mode: 0,
            uid: 0,
            gid: 0,
            modified: None,
            created: None,
            accessed: None,
            symlink_target: None,
            activation: None,
        }];
        state.cursor = 0;
        state.function.visible = true;
        // These widths all place a prop/monitor `┴` inside the filename region.
        for w in [80u16, 81, 91, 100, 105, 108] {
            let mut t = Terminal::new(TestBackend::new(w, 30)).unwrap();
            t.draw(|f| render(f, &state, &exts(), &mut ScrollMemory::default(), None, &vfs(), &top_opts(), &theme())).unwrap();
            let b = t.backend().buffer();
            let row: String = (0..b.area.width).map(|x| b[(x, 6)].symbol()).collect();
            assert!(
                row.contains("command.scm"),
                "filename corrupted by junction/title overlap at width {w}: {row:?}",
            );
        }
    }

    #[test]
    fn help_is_negative_colored() {
        let th = theme();
        let state = AppState::new(std::env::temp_dir());
        let mut t = Terminal::new(TestBackend::new(80, 30)).unwrap();
        t.draw(|f| render(f, &state, &exts(), &mut ScrollMemory::default(), None, &vfs(), &top_opts(), &th)).unwrap();
        let buf = t.backend().buffer();
        let y = buf.area.height - 1; // help row
        // The bar fills with HelpBg edge to edge, and the trigger-key chips are a
        // distinct (negative) HelpKeyBg — both colors must appear on the row.
        let has_bar = (0..buf.area.width).any(|x| buf[(x, y)].bg == th.role(Role::HelpBg));
        let has_chip = (0..buf.area.width).any(|x| buf[(x, y)].bg == th.role(Role::HelpKeyBg));
        assert!(has_bar, "help bar fills the row with the bar background");
        assert!(has_chip, "help bar shows negative key chips");
    }

    #[test]
    fn focused_cursor_reverses_unfocused_underlines() {
        use crate::core::Entry;
        let entries: Vec<Entry> = (0..3)
            .map(|i| Entry {
                path: std::path::PathBuf::from(format!("/d/f{i}")),
                name: format!("f{i}"),
                is_dir: false,
                is_symlink: false,
                size: 0,
                mode: 0,
                uid: 0,
                gid: 0,
                modified: None,
                created: None,
                accessed: None,
                symlink_target: None,
                activation: None,
            })
            .collect();
        // Count reverse-video cells (the help row reverses in both, so it cancels).
        let reversed = |function_focused: bool| {
            let mut s = AppState::new(std::env::temp_dir());
            s.entries = entries.clone();
            s.cursor = 0;
            s.function.visible = true;
            if function_focused {
                s.focused_panel = "function".into();
            }
            let mut t = Terminal::new(TestBackend::new(80, 30)).unwrap();
            t.draw(|f| render(f, &s, &exts(), &mut ScrollMemory::default(), None, &vfs(), &top_opts(), &theme())).unwrap();
            let buf = t.backend().buffer();
            buf.content.iter().filter(|c| c.modifier.contains(Modifier::REVERSED)).count()
        };
        assert!(
            reversed(false) > reversed(true),
            "the file cursor adds reverse-video cells only while the file panel is focused",
        );
    }

    #[test]
    fn nav_renders_breadcrumb_columns() {
        let state = AppState::new(std::env::temp_dir().join("a").join("b"));
        let mut t = Terminal::new(TestBackend::new(80, 30)).unwrap();
        t.draw(|f| render(f, &state, &exts(), &mut ScrollMemory::default(), None, &vfs(), &top_opts(), &theme())).unwrap();
        let b = t.backend().buffer();
        let text: String = (0..b.area.height)
            .flat_map(|y| (0..b.area.width).map(move |x| (x, y)))
            .map(|(x, y)| b[(x, y)].symbol().to_string())
            .collect();
        // The nav panel carries the app banner as its (unbracketed) title.
        assert!(text.contains("wasdf v"), "expected the nav banner title");
    }

    #[test]
    fn main_panels_open_onto_the_unboxed_help_row() {
        // doc/SCREEN.md: the file/function panels have no bottom border — their
        // side borders run down through the last content row and stop, with no
        // closing line or corners, and the help row below is plain unboxed text.
        let mut state = AppState::new(std::env::temp_dir());
        state.function.visible = true;
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| render(f, &state, &exts(), &mut ScrollMemory::default(), None, &vfs(), &top_opts(), &theme())).unwrap();
        let buf = terminal.backend().buffer();
        let (w, h) = (buf.area.width, buf.area.height);
        let row = |y: u16| -> String { (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>() };
        let last_content = row(h - 2);
        let help = row(h - 1);
        assert!(last_content.contains('│'), "last content row should still carry side borders");
        for glyph in ['─', '┌', '┐', '└', '┘', '├', '┤', '┬', '┴', '┼'] {
            assert!(!last_content.contains(glyph), "last content row should have no closing border: {last_content:?}");
            assert!(!help.contains(glyph), "help row should be unboxed: {help:?}");
        }
        assert!(!help.contains('│'), "help row should be unboxed: {help:?}");
    }

    #[test]
    fn middle_area_has_no_left_border() {
        // doc/SCREEN.md: the main (file/select) area has no left border — its
        // rows run flush to column 0 — so the top panel's bottom-left corner is
        // `└` (no southward stem) rather than `├`.
        let mut state = AppState::new(std::env::temp_dir());
        state.function.visible = true;
        let mut terminal = Terminal::new(TestBackend::new(80, 30)).unwrap();
        terminal.draw(|f| render(f, &state, &exts(), &mut ScrollMemory::default(), None, &vfs(), &top_opts(), &theme())).unwrap();
        let buf = terminal.backend().buffer();
        let h = buf.area.height;
        // The corner where the top panel meets the main area: `└`, not `├`.
        let corner = (0..h).map(|y| buf[(0, y)].symbol().to_string()).find(|s| s == "└");
        assert_eq!(corner.as_deref(), Some("└"), "top↔main left corner should be └");
        let corner_y = (0..h).find(|&y| buf[(0, y)].symbol() == "└").unwrap();
        // Every main-area row below that corner (down to the help row) is blank
        // at column 0 — no left border line or junction.
        for y in (corner_y + 1)..(h - 1) {
            assert_eq!(buf[(0, y)].symbol(), " ", "main row {y} should have no left border");
        }
    }

    #[test]
    fn borders_are_connected_with_junctions() {
        let state = AppState::new(std::env::temp_dir());
        let mut terminal = Terminal::new(TestBackend::new(80, 30)).unwrap();
        terminal.draw(|f| render(f, &state, &exts(), &mut ScrollMemory::default(), None, &vfs(), &top_opts(), &theme())).unwrap();
        let buf = terminal.backend().buffer();
        let (w, h) = (buf.area.width, buf.area.height);
        let row = |y: u16| -> String {
            (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>()
        };
        let text: String = (0..h).map(row).collect();
        // The tree/status divider meets the top frame as a junction, and the
        // top↔main divider line connects with junctions — single-line chrome.
        assert!(text.contains('┬'), "expected a top junction");
        assert!(text.contains('┴') || text.contains('┼'), "expected a divider junction");
        assert!(text.contains('│') && text.contains('─'), "expected single-line borders");
        // No doubled vertical borders within any row (nested boxes would double).
        for y in 0..h {
            assert!(!row(y).contains("││"), "row {y} has doubled borders: {:?}", row(y));
        }
    }
}
