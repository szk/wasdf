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
    NavCursor,
    PathSeparator,
    Entry,
    EntryDir,
    EntrySymlink,
    Cursor,
    CursorActive,
    SelectionMarker,
    Muted,
    Match,
    MatchCurrent,
    PropLabel,
    ExecRunning,
    ExecOk,
    ExecFail,
    NoticeInfo,
    NoticeError,
    OverlayDanger,
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
    Help,
    HelpKey,
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
        Role::NavCursor,
        Role::PathSeparator,
        Role::Entry,
        Role::EntryDir,
        Role::EntrySymlink,
        Role::Cursor,
        Role::CursorActive,
        Role::SelectionMarker,
        Role::Muted,
        Role::Match,
        Role::MatchCurrent,
        Role::PropLabel,
        Role::ExecRunning,
        Role::ExecOk,
        Role::ExecFail,
        Role::NoticeInfo,
        Role::NoticeError,
        Role::OverlayDanger,
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
        Role::Help,
        Role::HelpKey,
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
            Role::NavCursor => "nav-cursor",
            Role::PathSeparator => "path-separator",
            Role::Entry => "entry",
            Role::EntryDir => "entry-dir",
            Role::EntrySymlink => "entry-symlink",
            Role::Cursor => "cursor",
            Role::CursorActive => "cursor-active",
            Role::SelectionMarker => "selection-marker",
            Role::Muted => "muted",
            Role::Match => "match",
            Role::MatchCurrent => "match-current",
            Role::PropLabel => "prop-label",
            Role::ExecRunning => "exec-running",
            Role::ExecOk => "exec-ok",
            Role::ExecFail => "exec-fail",
            Role::NoticeInfo => "notice-info",
            Role::NoticeError => "notice-error",
            Role::OverlayDanger => "overlay-danger",
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
            Role::Help => "help",
            Role::HelpKey => "help-key",
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
    /// The built-in palette.
    /// This is the `default` preset and the fallback base for every custom theme.
    pub fn default() -> Self {
        let mut fg = [Color::Reset; Role::COUNT];
        fg[Role::Border.idx()] = Color::DarkGray;
        fg[Role::Title.idx()] = Color::Gray;
        fg[Role::TitleFocused.idx()] = Color::Cyan;
        // Brackets default to the title color.
        fg[Role::TitleBracket.idx()] = Color::Gray;
        fg[Role::TitleBracketFocused.idx()] = Color::Cyan;
        fg[Role::Nameplate.idx()] = Color::Gray;
        fg[Role::PathActive.idx()] = Color::White;
        fg[Role::PathMuted.idx()] = Color::DarkGray;
        fg[Role::NavCursor.idx()] = Color::Black;
        fg[Role::PathSeparator.idx()] = Color::Yellow;
        fg[Role::Entry.idx()] = Color::Reset;
        fg[Role::EntryDir.idx()] = Color::Blue;
        fg[Role::EntrySymlink.idx()] = Color::Magenta;
        // Cursor: bright while its panel is focused, subdued while it is not.
        fg[Role::CursorActive.idx()] = Color::Black;
        fg[Role::Cursor.idx()] = Color::White;
        fg[Role::SelectionMarker.idx()] = Color::Green;
        fg[Role::Muted.idx()] = Color::DarkGray;
        fg[Role::Match.idx()] = Color::Black;
        fg[Role::MatchCurrent.idx()] = Color::Black;
        fg[Role::PropLabel.idx()] = Color::DarkGray;
        fg[Role::ExecRunning.idx()] = Color::Yellow;
        fg[Role::ExecOk.idx()] = Color::Green;
        fg[Role::ExecFail.idx()] = Color::Red;
        fg[Role::NoticeInfo.idx()] = Color::Black;
        fg[Role::NoticeError.idx()] = Color::White;
        fg[Role::OverlayDanger.idx()] = Color::Red;
        // OverlayDanger's bg stays nil (terminal default behind the overlay).
        fg[Role::Accent.idx()] = Color::Cyan;
        // Per-metric monitor labels.
        fg[Role::MonitorCpuLabel.idx()] = Color::Cyan;
        fg[Role::MonitorMemLabel.idx()] = Color::Cyan;
        fg[Role::MonitorDskLabel.idx()] = Color::Cyan;
        fg[Role::MonitorNetLabel.idx()] = Color::Cyan;
        // Per-metric monitor bars.
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
        fg[Role::Help.idx()] = Color::White;
        fg[Role::HelpKey.idx()] = Color::Black;
        // The file-size bar's gradient: the highlight color at its left end
        // (the largest files reach here), gray at its right end (the zero
        // point, every file's bar tip lands here).
        fg[Role::FileRowSizeGraph.idx()] = Color::Cyan;

        // No style modifier carries a built-in opinion here: `Theme` is a
        // neutral palette primitive. Which roles are bold/italic/underline
        // "by default" is the theme extension's concern (see
        // `extension/theme/registry.rs`'s `DEFAULT_BOLD_ROLES`, applied by
        // `ThemeRegistry::resolve`).
        let modifiers = [Modifier::empty(); Role::COUNT];

        let mut bg = [Color::Reset; Role::COUNT];
        bg[Role::FileRowSizeGraph.idx()] = Color::Rgb(0x80, 0x80, 0x80);
        bg[Role::NavCursor.idx()] = Color::Cyan;
        bg[Role::CursorActive.idx()] = Color::Cyan;
        bg[Role::Cursor.idx()] = Color::DarkGray;
        bg[Role::Match.idx()] = Color::Yellow;
        bg[Role::MatchCurrent.idx()] = Color::LightCyan;
        bg[Role::NoticeInfo.idx()] = Color::Green;
        bg[Role::NoticeError.idx()] = Color::Red;
        bg[Role::Help.idx()] = Color::DarkGray;
        bg[Role::HelpKey.idx()] = Color::White;

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
    /// together are the whole frame's fallback pair (the fg/bg the whole
    /// frame is painted with, see `ui/render.rs`) — unlike every
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
    let lower = s.to_ascii_lowercase();
    if matches!(lower.as_str(), "default" | "reset") {
        return Some(Color::Reset);
    }
    // `dark-grey`, `light-red`, …: hyphens and the British spelling are
    // accepted on input; `color_token` writes the canonical name.
    let name = lower.replace('-', "").replace("grey", "gray");
    ANSI.iter()
        .find(|(_, n, _)| *n == name)
        .map(|(c, _, _)| *c)
        .or_else(|| lower.parse::<u8>().ok().map(Color::Indexed))
}

/// The 16 named ANSI colors in palette order (index = ANSI number): each
/// with its token and its standard terminal-palette RGB approximation.
const ANSI: [(Color, &str, (u8, u8, u8)); 16] = [
    (Color::Black, "black", (0, 0, 0)),
    (Color::Red, "red", (170, 0, 0)),
    (Color::Green, "green", (0, 170, 0)),
    (Color::Yellow, "yellow", (170, 85, 0)),
    (Color::Blue, "blue", (0, 0, 170)),
    (Color::Magenta, "magenta", (170, 0, 170)),
    (Color::Cyan, "cyan", (0, 170, 170)),
    (Color::Gray, "gray", (170, 170, 170)),
    (Color::DarkGray, "darkgray", (85, 85, 85)),
    (Color::LightRed, "lightred", (255, 85, 85)),
    (Color::LightGreen, "lightgreen", (85, 255, 85)),
    (Color::LightYellow, "lightyellow", (255, 255, 85)),
    (Color::LightBlue, "lightblue", (85, 85, 255)),
    (Color::LightMagenta, "lightmagenta", (255, 85, 255)),
    (Color::LightCyan, "lightcyan", (85, 255, 255)),
    (Color::White, "white", (255, 255, 255)),
];

fn ansi(c: Color) -> Option<&'static (Color, &'static str, (u8, u8, u8))> {
    ANSI.iter().find(|(a, _, _)| *a == c)
}

/// Inverse of `parse_color`: a round-trippable token for display and persistence.
pub fn color_token(c: Color) -> String {
    match c {
        Color::Reset => "default".into(),
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Indexed(n) => n.to_string(),
        named => ansi(named).map_or_else(String::new, |(_, name, _)| (*name).into()),
    }
}

/// A best-effort RGB approximation of any `Color`, used only to seed the
/// `InputKind::Color` popup's R/G/B sliders at a sensible starting position.
/// `Rgb` passes through exactly; named ANSI colors use their standard
/// terminal-palette approximation; `Indexed` follows the xterm 256-color
/// formula (0..15 as the ANSI table, 16..231 as the 6x6x6 cube, 232..255 as
/// the grayscale ramp); `Reset` has no color, so it maps to mid-gray.
pub fn color_to_rgb(c: Color) -> (u8, u8, u8) {
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Indexed(n) if n < 16 => ANSI[n as usize].2,
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
        named => ansi(named).map_or((128, 128, 128), |(_, _, rgb)| *rgb),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_color_token_parses_back_and_aliases_are_accepted() {
        let named = ANSI.iter().map(|(c, _, _)| *c);
        for c in named.chain([Color::Reset, Color::Indexed(42), Color::Rgb(1, 2, 3)]) {
            assert_eq!(parse_color(&color_token(c)), Some(c), "{c:?}");
        }
        assert_eq!(parse_color("Dark-Grey"), Some(Color::DarkGray));
        assert_eq!(parse_color("light-red"), Some(Color::LightRed));
        assert_eq!(parse_color("grey"), Some(Color::Gray));
        assert_eq!(color_to_rgb(Color::Indexed(9)), color_to_rgb(Color::LightRed));
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
        // neighboring cell's modifier through (e.g. the first letter after an
        // Underlined `[ ` bracket would render underlined too).
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
    fn a_role_with_no_fg_or_bg_of_its_own_falls_back_to_base() {
        let mut t = Theme::default();
        t.set_fg(Role::Base, Color::Rgb(1, 1, 1));
        t.set_bg(Role::Base, Color::Rgb(2, 2, 2));
        let s = t.style(Role::PropSizeValue); // stays nil by default
        assert_eq!(s.fg, Some(Color::Rgb(1, 1, 1)));
        assert_eq!(s.bg, Some(Color::Rgb(2, 2, 2)));
    }

}
