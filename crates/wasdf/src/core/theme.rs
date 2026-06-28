use ratatui::style::Color;

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
    // Global base colors (fallback when no specific role applies)
    Fg,
    Bg,
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
        Role::Fg,
        Role::Bg,
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
            Role::Fg => "fg",
            Role::Bg => "bg",
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

/// A fully-resolved palette: one color per role, ready for the renderer.
/// Built once per frame by ThemeRegistry::resolve_active; cheap to construct.
#[derive(Debug, Clone)]
pub struct Theme {
    colors: [Color; Role::COUNT],
}

impl Theme {
    /// The built-in palette reproducing today's exact appearance.
    /// This is the `default` preset and the fallback base for every custom theme.
    pub fn default() -> Self {
        let mut colors = [Color::Reset; Role::COUNT];
        colors[Role::Border.idx()] = Color::DarkGray;
        colors[Role::Title.idx()] = Color::Gray;
        colors[Role::TitleFocused.idx()] = Color::Cyan;
        // Brackets default to the title color so today's look is unchanged.
        colors[Role::TitleBracket.idx()] = Color::Gray;
        colors[Role::TitleBracketFocused.idx()] = Color::Cyan;
        colors[Role::Nameplate.idx()] = Color::Gray;
        colors[Role::PathActive.idx()] = Color::White;
        colors[Role::PathMuted.idx()] = Color::DarkGray;
        colors[Role::NavCursorFg.idx()] = Color::Black;
        colors[Role::NavCursorBg.idx()] = Color::Cyan;
        colors[Role::PathSeparator.idx()] = Color::Yellow;
        colors[Role::Entry.idx()] = Color::Reset;
        colors[Role::EntryDir.idx()] = Color::Blue;
        colors[Role::EntrySymlink.idx()] = Color::Magenta;
        colors[Role::CursorFg.idx()] = Color::Black;
        colors[Role::CursorBg.idx()] = Color::Cyan;
        colors[Role::SelectionMarker.idx()] = Color::Green;
        colors[Role::Muted.idx()] = Color::DarkGray;
        colors[Role::MatchBg.idx()] = Color::Yellow;
        colors[Role::MatchFg.idx()] = Color::Black;
        colors[Role::MatchCurrentBg.idx()] = Color::LightCyan;
        colors[Role::MatchCurrentFg.idx()] = Color::Black;
        colors[Role::PropLabel.idx()] = Color::DarkGray;
        colors[Role::ExecRunning.idx()] = Color::Yellow;
        colors[Role::ExecOk.idx()] = Color::Green;
        colors[Role::ExecFail.idx()] = Color::Red;
        colors[Role::NoticeInfoFg.idx()] = Color::Black;
        colors[Role::NoticeInfoBg.idx()] = Color::Green;
        colors[Role::NoticeErrorFg.idx()] = Color::White;
        colors[Role::NoticeErrorBg.idx()] = Color::Red;
        colors[Role::OverlayDangerFg.idx()] = Color::Red;
        // OverlayDangerBg stays Color::Reset (terminal default behind the overlay).
        colors[Role::Accent.idx()] = Color::Cyan;
        // Per-metric monitor labels (today's monitor label was Cyan).
        colors[Role::MonitorCpuLabel.idx()] = Color::Cyan;
        colors[Role::MonitorMemLabel.idx()] = Color::Cyan;
        colors[Role::MonitorDskLabel.idx()] = Color::Cyan;
        colors[Role::MonitorNetLabel.idx()] = Color::Cyan;
        // Per-metric monitor bars (today's monitor bar was Green).
        colors[Role::MonitorCpuBar.idx()] = Color::Green;
        colors[Role::MonitorMemBar.idx()] = Color::Green;
        colors[Role::MonitorDskBar.idx()] = Color::Green;
        colors[Role::MonitorNetBar.idx()] = Color::Green;
        // MonitorCpu/Mem/Dsk/NetValue, Fg, Bg, PropSize/Owner/Perm/Time/InfoValue
        // all stay at Color::Reset (terminal default fg/bg).
        // Per-field prop labels inherit the generic PropLabel default.
        colors[Role::PropSizeLabel.idx()] = Color::DarkGray;
        colors[Role::PropOwnerLabel.idx()] = Color::DarkGray;
        colors[Role::PropPermLabel.idx()] = Color::DarkGray;
        colors[Role::PropTimeLabel.idx()] = Color::DarkGray;
        colors[Role::PropInfoLabel.idx()] = Color::DarkGray;
        // Help bar: a dark status strip with negative (inverted) key chips.
        colors[Role::HelpFg.idx()] = Color::White;
        colors[Role::HelpBg.idx()] = Color::DarkGray;
        colors[Role::HelpKeyFg.idx()] = Color::Black;
        colors[Role::HelpKeyBg.idx()] = Color::White;
        Theme { colors }
    }

    pub fn role(&self, r: Role) -> Color {
        self.colors[r.idx()]
    }

    pub fn set(&mut self, r: Role, c: Color) {
        self.colors[r.idx()] = c;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_theme_has_one_color_per_role() {
        let t = Theme::default();
        assert_eq!(t.colors.len(), Role::COUNT);
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
