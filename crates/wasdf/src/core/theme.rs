use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    Border,
    Title,
    TitleFocused,
    TitleBracket,
    TitleBracketFocused,
    Nameplate,
    PathActive,
    PathMuted,
    NavCursorFg,
    NavCursorBg,
    PathSeparator,
    Entry,
    EntryDir,
    EntrySymlink,
    CursorFg,
    CursorBg,
    CursorActiveFg,
    CursorActiveBg,
    SelectionMarker,
    Muted,
    MatchBg,
    MatchFg,
    MatchCurrentBg,
    MatchCurrentFg,
    PropLabel,
    ExecRunning,
    ExecOk,
    ExecFail,
    NoticeInfoFg,
    NoticeInfoBg,
    NoticeErrorFg,
    NoticeErrorBg,
    OverlayDangerFg,
    OverlayDangerBg,
    Accent,
    // Per-metric monitor roles
    MonitorCpuLabel,
    MonitorMemLabel,
    MonitorDskLabel,
    MonitorNetLabel,
    MonitorCpuBar,
    MonitorMemBar,
    MonitorDskBar,
    MonitorNetBar,
    MonitorCpuValue,
    MonitorMemValue,
    MonitorDskValue,
    MonitorNetValue,
    // "Base": the whole frame's fallback fg/bg — see `resolved_fg_bg`. The
    // one role whose own `fg`/`bg` slots are both meaningful in their own
    // right (every other role's `bg` is just "this role's own background";
    // Base's `bg` *is* the fallback background everything else falls onto).
    Base,
    // Per-field prop roles
    PropSizeLabel,
    PropOwnerLabel,
    PropPermLabel,
    PropTimeLabel,
    PropInfoLabel,
    PropSizeValue,
    PropOwnerValue,
    PropPermValue,
    PropTimeValue,
    PropInfoValue,
    // The Rows layout's right-aligned PERM/OWNR/SIZE/TIME columns (PANEL_FILE
    // only — distinct from the PropXxxValue roles above, which style the top
    // prop panel's own per-field rows). One role for all four fields: see
    // ui/middle.rs's `prop_values`.
    FileRowProps,
    // The Rows layout's logarithmic file-size bar: fg is the gradient's left
    // end (the largest files reach), bg its right end (the zero point, every
    // file's tip). See ui/middle.rs's `gradient_at`.
    FileRowSizeGraph,
    // Help bar (bottom row)
    HelpFg,
    HelpBg,
    HelpKeyFg,
    HelpKeyBg,
}

impl Role {
    pub const ALL: &'static [Role] = &[
        Role::Border,
        Role::Title,
        Role::TitleFocused,
        Role::TitleBracket,
        Role::TitleBracketFocused,
        Role::Nameplate,
        Role::PathActive,
        Role::PathMuted,
        Role::NavCursorFg,
        Role::NavCursorBg,
        Role::PathSeparator,
        Role::Entry,
        Role::EntryDir,
        Role::EntrySymlink,
        Role::CursorFg,
        Role::CursorBg,
        Role::CursorActiveFg,
        Role::CursorActiveBg,
        Role::SelectionMarker,
        Role::Muted,
        Role::MatchBg,
        Role::MatchFg,
        Role::MatchCurrentBg,
        Role::MatchCurrentFg,
        Role::PropLabel,
        Role::ExecRunning,
        Role::ExecOk,
        Role::ExecFail,
        Role::NoticeInfoFg,
        Role::NoticeInfoBg,
        Role::NoticeErrorFg,
        Role::NoticeErrorBg,
        Role::OverlayDangerFg,
        Role::OverlayDangerBg,
        Role::Accent,
        Role::MonitorCpuLabel,
        Role::MonitorMemLabel,
        Role::MonitorDskLabel,
        Role::MonitorNetLabel,
        Role::MonitorCpuBar,
        Role::MonitorMemBar,
        Role::MonitorDskBar,
        Role::MonitorNetBar,
        Role::MonitorCpuValue,
        Role::MonitorMemValue,
        Role::MonitorDskValue,
        Role::MonitorNetValue,
        Role::Base,
        Role::PropSizeLabel,
        Role::PropOwnerLabel,
        Role::PropPermLabel,
        Role::PropTimeLabel,
        Role::PropInfoLabel,
        Role::PropSizeValue,
        Role::PropOwnerValue,
        Role::PropPermValue,
        Role::PropTimeValue,
        Role::PropInfoValue,
        Role::FileRowProps,
        Role::FileRowSizeGraph,
        Role::HelpFg,
        Role::HelpBg,
        Role::HelpKeyFg,
        Role::HelpKeyBg,
    ];
    pub const COUNT: usize = Role::ALL.len();

    pub fn idx(self) -> usize {
        Role::ALL.iter().position(|r| *r == self).unwrap()
    }

    pub fn token(self) -> &'static str {
        match self {
            Role::Border => "border",
            Role::Title => "title",
            Role::TitleFocused => "title-focused",
            Role::TitleBracket => "title-bracket",
            Role::TitleBracketFocused => "title-bracket-focused",
            Role::Nameplate => "nameplate",
            Role::PathActive => "path-active",
            Role::PathMuted => "path-muted",
            Role::NavCursorFg => "nav-cursor-fg",
            Role::NavCursorBg => "nav-cursor-bg",
            Role::PathSeparator => "path-separator",
            Role::Entry => "entry",
            Role::EntryDir => "entry-dir",
            Role::EntrySymlink => "entry-symlink",
            Role::CursorFg => "cursor-fg",
            Role::CursorBg => "cursor-bg",
            Role::CursorActiveFg => "cursor-active-fg",
            Role::CursorActiveBg => "cursor-active-bg",
            Role::SelectionMarker => "selection-marker",
            Role::Muted => "muted",
            Role::MatchBg => "match-bg",
            Role::MatchFg => "match-fg",
            Role::MatchCurrentBg => "match-current-bg",
            Role::MatchCurrentFg => "match-current-fg",
            Role::PropLabel => "prop-label",
            Role::ExecRunning => "exec-running",
            Role::ExecOk => "exec-ok",
            Role::ExecFail => "exec-fail",
            Role::NoticeInfoFg => "notice-info-fg",
            Role::NoticeInfoBg => "notice-info-bg",
            Role::NoticeErrorFg => "notice-error-fg",
            Role::NoticeErrorBg => "notice-error-bg",
            Role::OverlayDangerFg => "overlay-danger-fg",
            Role::OverlayDangerBg => "overlay-danger-bg",
            Role::Accent => "accent",
            Role::MonitorCpuLabel => "monitor-cpu-label",
            Role::MonitorMemLabel => "monitor-mem-label",
            Role::MonitorDskLabel => "monitor-dsk-label",
            Role::MonitorNetLabel => "monitor-net-label",
            Role::MonitorCpuBar => "monitor-cpu-bar",
            Role::MonitorMemBar => "monitor-mem-bar",
            Role::MonitorDskBar => "monitor-dsk-bar",
            Role::MonitorNetBar => "monitor-net-bar",
            Role::MonitorCpuValue => "monitor-cpu-value",
            Role::MonitorMemValue => "monitor-mem-value",
            Role::MonitorDskValue => "monitor-dsk-value",
            Role::MonitorNetValue => "monitor-net-value",
            Role::Base => "base",
            Role::PropSizeLabel => "prop-size-label",
            Role::PropOwnerLabel => "prop-ownr-label",
            Role::PropPermLabel => "prop-perm-label",
            Role::PropTimeLabel => "prop-time-label",
            Role::PropInfoLabel => "prop-info-label",
            Role::PropSizeValue => "prop-size-value",
            Role::PropOwnerValue => "prop-ownr-value",
            Role::PropPermValue => "prop-perm-value",
            Role::PropTimeValue => "prop-time-value",
            Role::PropInfoValue => "prop-info-value",
            Role::FileRowProps => "file-row-props",
            Role::FileRowSizeGraph => "file-row-size-graph",
            Role::HelpFg => "help-fg",
            Role::HelpBg => "help-bg",
            Role::HelpKeyFg => "help-key-fg",
            Role::HelpKeyBg => "help-key-bg",
        }
    }

    pub fn from_token(s: &str) -> Option<Role> {
        Role::ALL.iter().copied().find(|r| r.token() == s)
    }
}

/// A fully-resolved palette: one foreground color, one background color, and
/// one style-modifier set (bold, italic, underline) per role, ready for the
/// renderer. Built once per frame by ThemeRegistry::resolve_active; cheap to
/// construct. `Color::Reset` is "nil" — no color painted, the role's fg or bg
/// simply resets to the terminal default.
#[derive(Debug, Clone)]
pub struct Theme {
    fg: [Color; Role::COUNT],
    bg: [Color; Role::COUNT],
    modifiers: [Modifier; Role::COUNT],
}

impl Theme {
    /// The built-in palette reproducing today's exact appearance.
    /// This is the `default` preset and the fallback base for every custom theme.
    pub fn default() -> Self {
        let mut fg = [Color::Reset; Role::COUNT];
        fg[Role::Border.idx()] = Color::DarkGray;
        fg[Role::Title.idx()] = Color::Gray;
        fg[Role::TitleFocused.idx()] = Color::Cyan;
        // Brackets default to the title color so today's look is unchanged.
        fg[Role::TitleBracket.idx()] = Color::Gray;
        fg[Role::TitleBracketFocused.idx()] = Color::Cyan;
        fg[Role::Nameplate.idx()] = Color::Gray;
        fg[Role::PathActive.idx()] = Color::White;
        fg[Role::PathMuted.idx()] = Color::DarkGray;
        fg[Role::NavCursorFg.idx()] = Color::Black;
        fg[Role::NavCursorBg.idx()] = Color::Cyan;
        fg[Role::PathSeparator.idx()] = Color::Yellow;
        fg[Role::Entry.idx()] = Color::Reset;
        fg[Role::EntryDir.idx()] = Color::Blue;
        fg[Role::EntrySymlink.idx()] = Color::Magenta;
        // Cursor: bright while its panel is focused, subdued while it is not.
        fg[Role::CursorActiveFg.idx()] = Color::Black;
        fg[Role::CursorActiveBg.idx()] = Color::Cyan;
        fg[Role::CursorFg.idx()] = Color::White;
        fg[Role::CursorBg.idx()] = Color::DarkGray;
        fg[Role::SelectionMarker.idx()] = Color::Green;
        fg[Role::Muted.idx()] = Color::DarkGray;
        fg[Role::MatchBg.idx()] = Color::Yellow;
        fg[Role::MatchFg.idx()] = Color::Black;
        fg[Role::MatchCurrentBg.idx()] = Color::LightCyan;
        fg[Role::MatchCurrentFg.idx()] = Color::Black;
        fg[Role::PropLabel.idx()] = Color::DarkGray;
        fg[Role::ExecRunning.idx()] = Color::Yellow;
        fg[Role::ExecOk.idx()] = Color::Green;
        fg[Role::ExecFail.idx()] = Color::Red;
        fg[Role::NoticeInfoFg.idx()] = Color::Black;
        fg[Role::NoticeInfoBg.idx()] = Color::Green;
        fg[Role::NoticeErrorFg.idx()] = Color::White;
        fg[Role::NoticeErrorBg.idx()] = Color::Red;
        fg[Role::OverlayDangerFg.idx()] = Color::Red;
        // OverlayDangerBg stays Color::Reset (terminal default behind the overlay).
        fg[Role::Accent.idx()] = Color::Cyan;
        // Per-metric monitor labels (today's monitor label was Cyan).
        fg[Role::MonitorCpuLabel.idx()] = Color::Cyan;
        fg[Role::MonitorMemLabel.idx()] = Color::Cyan;
        fg[Role::MonitorDskLabel.idx()] = Color::Cyan;
        fg[Role::MonitorNetLabel.idx()] = Color::Cyan;
        // Per-metric monitor bars (today's monitor bar was Green).
        fg[Role::MonitorCpuBar.idx()] = Color::Green;
        fg[Role::MonitorMemBar.idx()] = Color::Green;
        fg[Role::MonitorDskBar.idx()] = Color::Green;
        fg[Role::MonitorNetBar.idx()] = Color::Green;
        // MonitorCpu/Mem/Dsk/NetValue, Fg, Bg, PropSize/Owner/Perm/Time/InfoValue
        // all stay at Color::Reset (terminal default fg/bg).
        // Per-field prop labels inherit the generic PropLabel default.
        fg[Role::PropSizeLabel.idx()] = Color::DarkGray;
        fg[Role::PropOwnerLabel.idx()] = Color::DarkGray;
        fg[Role::PropPermLabel.idx()] = Color::DarkGray;
        fg[Role::PropTimeLabel.idx()] = Color::DarkGray;
        fg[Role::PropInfoLabel.idx()] = Color::DarkGray;
        // Help bar: a dark status strip with negative (inverted) key chips.
        fg[Role::HelpFg.idx()] = Color::White;
        fg[Role::HelpBg.idx()] = Color::DarkGray;
        fg[Role::HelpKeyFg.idx()] = Color::Black;
        fg[Role::HelpKeyBg.idx()] = Color::White;
        // The file-size bar's gradient: red at its left end (the largest
        // files reach here), dark gray at its right end (the zero point,
        // every file's bar tip lands here).
        fg[Role::FileRowSizeGraph.idx()] = Color::Red;

        // No style modifier carries a built-in opinion here: `Theme` is a
        // neutral palette primitive. Which roles are bold/italic/underline
        // "by default" is the theme extension's concern (see
        // `extension/theme/registry.rs`'s `DEFAULT_BOLD_ROLES`, applied by
        // `ThemeRegistry::resolve`).
        let modifiers = [Modifier::empty(); Role::COUNT];

        let mut bg = [Color::Reset; Role::COUNT];
        bg[Role::FileRowSizeGraph.idx()] = Color::DarkGray;

        Theme { fg, bg, modifiers }
    }

    pub fn role(&self, r: Role) -> Color {
        self.fg[r.idx()]
    }

    pub fn bg(&self, r: Role) -> Color {
        self.bg[r.idx()]
    }

    pub fn set_bg(&mut self, r: Role, c: Color) {
        self.bg[r.idx()] = c;
    }

    pub fn bold(&self, r: Role) -> bool {
        self.modifiers[r.idx()].contains(Modifier::BOLD)
    }

    pub fn set_bold(&mut self, r: Role, b: bool) {
        self.set_modifier(r, Modifier::BOLD, b);
    }

    pub fn italic(&self, r: Role) -> bool {
        self.modifiers[r.idx()].contains(Modifier::ITALIC)
    }

    pub fn set_italic(&mut self, r: Role, b: bool) {
        self.set_modifier(r, Modifier::ITALIC, b);
    }

    pub fn underline(&self, r: Role) -> bool {
        self.modifiers[r.idx()].contains(Modifier::UNDERLINED)
    }

    pub fn set_underline(&mut self, r: Role, b: bool) {
        self.set_modifier(r, Modifier::UNDERLINED, b);
    }

    fn set_modifier(&mut self, r: Role, m: Modifier, on: bool) {
        let idx = r.idx();
        if on { self.modifiers[idx].insert(m); } else { self.modifiers[idx].remove(m); }
    }

    /// The ready-to-use `Style` for `r`: its `fg` and `bg` colors, plus
    /// whichever of Bold/Italic/Underline the role carries. Call sites should
    /// use this instead of `Style::default().fg(theme.role(r))` so `bg` and
    /// every modifier take effect everywhere. A call site that wants its own
    /// `bg` can still chain `.bg(..)` on the result — it simply overrides the
    /// role's, the same way any later `Style` field write does.
    ///
    /// Explicitly sets *both* colors and *removes* every modifier this role
    /// does not carry, rather than leaving them unspecified:
    /// `ratatui::buffer::Cell::set_style` patches (merges) onto whatever style
    /// the cell already carries from an earlier pass in the same frame (e.g.
    /// chrome's border-glyph pass stamping a whole row before per-character
    /// title text overwrites part of it) — an unspecified attribute survives
    /// that patch, so a role with no bg/modifier of its own does not silently
    /// inherit one from underneath. `Color::Reset` ("nil") is itself an
    /// explicit value here, not an absence, so this still holds for a role
    /// that wants no background at all.
    pub fn style(&self, r: Role) -> Style {
        let (fg, bg) = self.resolved_fg_bg(r);
        let m = self.modifiers[r.idx()];
        Style::default()
            .fg(fg)
            .bg(bg)
            .remove_modifier(Modifier::all().difference(m))
            .add_modifier(m)
    }

    /// `r`'s fg/bg after the "base" fallback: `Role::Base`'s own fg and bg
    /// together are the whole frame's fallback pair (the historical fg/bg
    /// the whole frame is painted with, see `ui/render.rs`) — unlike every
    /// other role, where `bg` only ever means "this role's own background",
    /// `Base`'s `bg` slot *is* the fallback background, used directly, not
    /// borrowed from anywhere else. `Role::Base` terminates the chain: asking
    /// for its own style resolves straight to its own fg/bg, no fallback.
    ///
    /// For any other role: an explicit (non-nil) fg or bg always wins. A nil
    /// bg falls back to base's bg. A nil fg falls back to base's bg *if* this
    /// role set its own bg (the "knockout" look: a role with its own
    /// background but no foreground of its own reads in base's bg color,
    /// like text punched out of it) — otherwise (this role's bg is nil too)
    /// it falls back to base's fg, the plain case. If even the base color a
    /// fallback lands on is itself nil, the result is nil — the terminal's
    /// own color shows through, same as it always did.
    fn resolved_fg_bg(&self, r: Role) -> (Color, Color) {
        let base_fg = self.fg[Role::Base.idx()];
        let base_bg = self.bg[Role::Base.idx()];
        if r == Role::Base {
            return (base_fg, base_bg);
        }
        let raw_fg = self.fg[r.idx()];
        let raw_bg = self.bg[r.idx()];
        let bg = if raw_bg != Color::Reset { raw_bg } else { base_bg };
        let fg = if raw_fg != Color::Reset {
            raw_fg
        } else if raw_bg != Color::Reset {
            base_bg
        } else {
            base_fg
        };
        (fg, bg)
    }

    /// Resolve an extension-facing style role *name* (from a `PanelView` span) to
    /// a color. A small curated vocabulary an extension can rely on; unknown names
    /// fall back to the default text color so a typo degrades gracefully rather
    /// than failing. Keeps dynamic extensions off the internal `Role` enum.
    pub fn role_by_name(&self, name: &str) -> Color {
        self.role(role_for_name(name))
    }

    /// Like [`Theme::style`], resolved through the extension-facing name
    /// vocabulary — carries the role's bold flag too (e.g. "dir" is bold by
    /// default, matching `Role::EntryDir`).
    pub fn style_by_name(&self, name: &str) -> Style {
        self.style(role_for_name(name))
    }

    pub fn set_fg(&mut self, r: Role, c: Color) {
        self.fg[r.idx()] = c;
    }
}

/// The curated name → `Role` vocabulary backing `role_by_name` / `style_by_name`.
/// Unknown names fall back to `Base` so a typo degrades gracefully.
fn role_for_name(name: &str) -> Role {
    match name {
        "muted" => Role::Muted,
        "accent" => Role::Accent,
        "label" => Role::PropLabel,
        "value" | "text" | "default" => Role::Base,
        "dir" | "directory" => Role::EntryDir,
        "symlink" => Role::EntrySymlink,
        "ok" | "success" => Role::ExecOk,
        "fail" | "error" => Role::ExecFail,
        _ => Role::Base,
    }
}

/// Parse a theme color token into a ratatui Color.
/// Accepts: "#rrggbb" (hex), the 16 ANSI names, "default"/"reset", "N" (0..255).
pub fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        if hex.len() == 6 {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            return Some(Color::Rgb(r, g, b));
        }
        return None;
    }
    match s.to_ascii_lowercase().as_str() {
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "gray" | "grey" => Some(Color::Gray),
        "darkgray" | "dark-gray" | "darkgrey" | "dark-grey" => Some(Color::DarkGray),
        "lightred" | "light-red" => Some(Color::LightRed),
        "lightgreen" | "light-green" => Some(Color::LightGreen),
        "lightyellow" | "light-yellow" => Some(Color::LightYellow),
        "lightblue" | "light-blue" => Some(Color::LightBlue),
        "lightmagenta" | "light-magenta" => Some(Color::LightMagenta),
        "lightcyan" | "light-cyan" => Some(Color::LightCyan),
        "white" => Some(Color::White),
        "default" | "reset" => Some(Color::Reset),
        other => other.parse::<u8>().ok().map(Color::Indexed),
    }
}

/// Inverse of `parse_color`: a round-trippable token for display and persistence.
pub fn color_token(c: Color) -> String {
    match c {
        Color::Black => "black".into(),
        Color::Red => "red".into(),
        Color::Green => "green".into(),
        Color::Yellow => "yellow".into(),
        Color::Blue => "blue".into(),
        Color::Magenta => "magenta".into(),
        Color::Cyan => "cyan".into(),
        Color::Gray => "gray".into(),
        Color::DarkGray => "darkgray".into(),
        Color::LightRed => "lightred".into(),
        Color::LightGreen => "lightgreen".into(),
        Color::LightYellow => "lightyellow".into(),
        Color::LightBlue => "lightblue".into(),
        Color::LightMagenta => "lightmagenta".into(),
        Color::LightCyan => "lightcyan".into(),
        Color::White => "white".into(),
        Color::Reset => "default".into(),
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Indexed(n) => n.to_string(),
    }
}

/// A best-effort RGB approximation of any `Color`, used only to seed the
/// `InputKind::Color` popup's R/G/B sliders at a sensible starting position.
/// `Rgb` passes through exactly; named ANSI colors use their standard
/// terminal-palette approximation; `Indexed` follows the xterm 256-color
/// formula (0..15 as the ANSI table, 16..231 as the 6x6x6 cube, 232..255 as
/// the grayscale ramp); `Reset` has no color, so it maps to mid-gray.
pub fn color_to_rgb(c: Color) -> (u8, u8, u8) {
    fn ansi16(n: u8) -> (u8, u8, u8) {
        match n {
            0 => (0, 0, 0),
            1 => (170, 0, 0),
            2 => (0, 170, 0),
            3 => (170, 85, 0),
            4 => (0, 0, 170),
            5 => (170, 0, 170),
            6 => (0, 170, 170),
            7 => (170, 170, 170),
            8 => (85, 85, 85),
            9 => (255, 85, 85),
            10 => (85, 255, 85),
            11 => (255, 255, 85),
            12 => (85, 85, 255),
            13 => (255, 85, 255),
            14 => (85, 255, 255),
            _ => (255, 255, 255),
        }
    }
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Black => ansi16(0),
        Color::Red => ansi16(1),
        Color::Green => ansi16(2),
        Color::Yellow => ansi16(3),
        Color::Blue => ansi16(4),
        Color::Magenta => ansi16(5),
        Color::Cyan => ansi16(6),
        Color::Gray => ansi16(7),
        Color::DarkGray => ansi16(8),
        Color::LightRed => ansi16(9),
        Color::LightGreen => ansi16(10),
        Color::LightYellow => ansi16(11),
        Color::LightBlue => ansi16(12),
        Color::LightMagenta => ansi16(13),
        Color::LightCyan => ansi16(14),
        Color::White => ansi16(15),
        Color::Indexed(n) if n < 16 => ansi16(n),
        Color::Indexed(n) if n < 232 => {
            let n = n - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            (level(n / 36), level((n / 6) % 6), level(n % 6))
        }
        Color::Indexed(n) => {
            let v = 8 + (n - 232) * 10;
            (v, v, v)
        }
        Color::Reset => (128, 128, 128),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_theme_has_one_fg_and_bg_per_role() {
        let t = Theme::default();
        assert_eq!(t.fg.len(), Role::COUNT);
        assert_eq!(t.bg.len(), Role::COUNT);
    }

    #[test]
    fn color_round_trips_named_and_hex() {
        for name in ["black", "red", "green", "yellow", "blue", "magenta", "cyan",
                     "gray", "darkgray", "white", "lightcyan", "default"] {
            let c = parse_color(name).unwrap_or_else(|| panic!("parse_color({name:?}) failed"));
            assert_eq!(parse_color(&color_token(c)), Some(c), "round-trip for {name}");
        }
        let hex = "#1a2b3c";
        let c = parse_color(hex).unwrap();
        assert_eq!(color_token(c), hex);
        assert!(parse_color("nonsense").is_none());
    }

    #[test]
    fn style_applies_bold_only_when_set() {
        // Theme itself carries no bold opinion — "default" role bold comes
        // from the theme extension's DEFAULT_BOLD_ROLES, layered on at
        // `ThemeRegistry::resolve` time, not from `Theme::default()`.
        let mut t = Theme::default();
        assert!(!t.style(Role::EntryDir).add_modifier.contains(Modifier::BOLD));
        assert!(!t.style(Role::Title).add_modifier.contains(Modifier::BOLD));
        t.set_bold(Role::Title, true);
        assert!(t.style(Role::Title).add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn style_clears_a_stray_modifier_when_patched_over_a_differently_styled_cell() {
        // `ratatui::buffer::Cell::set_style` patches (merges) onto whatever a
        // cell already carries from an earlier draw pass in the same frame —
        // e.g. chrome's border-glyph pass stamps a whole row with one role's
        // style before per-character title text overwrites part of it with
        // another role's. `Style::default()` alone leaves an unspecified
        // modifier for `patch` to inherit; `theme.style()` must explicitly
        // remove it instead, or a role with no modifier of its own bleeds a
        // neighboring cell's modifier through — the same bug that made the
        // first letter after an Underlined `[ ` bracket render underlined too.
        let mut t = Theme::default();
        t.set_underline(Role::TitleBracket, true);
        let bracket_cell = t.style(Role::TitleBracket);
        let text_cell = t.style(Role::Title); // carries no modifier of its own
        let patched = bracket_cell.patch(text_cell);
        assert!(
            !patched.add_modifier.contains(Modifier::UNDERLINED),
            "a role with no modifier must not inherit UNDERLINED from the cell it overwrites"
        );
    }

    #[test]
    fn style_sets_bg_explicitly_so_it_never_bleeds_through_a_patch_either() {
        let mut t = Theme::default();
        t.set_bg(Role::TitleBracket, Color::Rgb(0x0c, 0x7f, 0x14));
        let bracket_cell = t.style(Role::TitleBracket);
        let text_cell = t.style(Role::Title); // no bg override — nil/Color::Reset
        let patched = bracket_cell.patch(text_cell);
        assert_eq!(
            patched.bg,
            Some(Color::Reset),
            "a role with no bg override must reset bg explicitly, not inherit a neighbor's"
        );
    }

    #[test]
    fn a_role_with_no_fg_or_bg_of_its_own_falls_back_to_base() {
        let mut t = Theme::default();
        t.set_fg(Role::Base, Color::Rgb(1, 1, 1));
        t.set_bg(Role::Base, Color::Rgb(2, 2, 2));
        let s = t.style(Role::PropSizeValue); // stays nil by default
        assert_eq!(s.fg, Some(Color::Rgb(1, 1, 1)));
        assert_eq!(s.bg, Some(Color::Rgb(2, 2, 2)));
    }

    #[test]
    fn a_roles_own_explicit_fg_or_bg_always_wins_over_base() {
        let mut t = Theme::default();
        t.set_fg(Role::Base, Color::Rgb(1, 1, 1));
        t.set_bg(Role::Base, Color::Rgb(2, 2, 2));
        t.set_fg(Role::Title, Color::Rgb(9, 9, 9));
        t.set_bg(Role::Title, Color::Rgb(8, 8, 8));
        let s = t.style(Role::Title);
        assert_eq!(s.fg, Some(Color::Rgb(9, 9, 9)));
        assert_eq!(s.bg, Some(Color::Rgb(8, 8, 8)));
    }

    #[test]
    fn a_role_with_its_own_bg_but_no_fg_reads_in_bases_bg_as_a_knockout() {
        // A role that sets its own background but leaves fg nil reads as
        // "punched out" in base's bg color, not base's fg — the replacement
        // for the old reverse-video "Negative" trick, using real colors.
        let mut t = Theme::default();
        t.set_fg(Role::Base, Color::Rgb(1, 1, 1));
        t.set_bg(Role::Base, Color::Rgb(2, 2, 2));
        t.set_bg(Role::PropSizeValue, Color::Rgb(7, 7, 7)); // its own bg, no fg of its own
        let s = t.style(Role::PropSizeValue);
        assert_eq!(s.fg, Some(Color::Rgb(2, 2, 2)), "nil fg + a role's own bg -> base's bg as ink");
        assert_eq!(s.bg, Some(Color::Rgb(7, 7, 7)), "the role's own bg always wins");
    }

    #[test]
    fn base_itself_never_falls_back_it_terminates_the_chain() {
        let mut t = Theme::default();
        t.set_fg(Role::Base, Color::Rgb(1, 1, 1));
        t.set_bg(Role::Base, Color::Rgb(2, 2, 2));
        let s = t.style(Role::Base);
        assert_eq!(s.fg, Some(Color::Rgb(1, 1, 1)));
        assert_eq!(s.bg, Some(Color::Rgb(2, 2, 2)));
    }

    #[test]
    fn a_nil_base_bg_leaves_a_falling_back_role_nil_too() {
        // No base color set at all (Theme::default()): every fallback bottoms
        // out at Color::Reset, same as before this cascade existed.
        let t = Theme::default();
        let s = t.style(Role::Title);
        assert_eq!(s.bg, Some(Color::Reset));
    }

    #[test]
    fn every_role_has_a_unique_token() {
        let tokens: Vec<&str> = Role::ALL.iter().map(|r| r.token()).collect();
        let mut seen = std::collections::HashSet::new();
        for t in &tokens {
            assert!(seen.insert(*t), "duplicate token: {t}");
        }
        for r in Role::ALL {
            assert_eq!(Role::from_token(r.token()), Some(*r));
        }
    }
}
