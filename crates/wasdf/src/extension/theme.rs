//! The `theme` bundled extension: registry, presets, selection, and authoring.

pub mod registry;

pub use registry::{NamedTheme, RoleStyle, ThemeRegistry};

use std::cell::RefCell;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock, RwLockReadGuard};

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Bar, BarChart, BarGroup, Paragraph};
use ratatui::Frame;

use crate::core::{
    AppState, ConfigWrite, Entry, ExtensionIntent, ExtensionValue, InputFormSpec, Intent, Mode, OptionStore,
    OptionValue, Role, KEY_ITEM,
};
use crate::extension::settings::SettingsNode;
use crate::extension::{Extension, FunctionRenderCtx};
use crate::script::condition::Conditions;
use crate::vfs::vfs_root;

const ID: &str = "theme";
const KEYMAP_SCHEME: &str = include_str!("theme/keymap.scm");
const OPTIONS_SCHEME: &str = include_str!("theme/options.scm");
const PRELUDE_SCHEME: &str = include_str!("theme/prelude.scm");

/// The registry as a projection of the `theme.active` / `theme.custom`
/// options: rebuilt whenever the OptionStore's generation moves, so every
/// reader — the extension, its settings node on a worker thread, the frame's
/// `resolve_active` — sees the store's state without being told.
struct ThemeState {
    registry: Arc<RwLock<ThemeRegistry>>,
    options: OnceLock<Arc<OptionStore>>,
    /// The options generation the registry was last built from, plus one
    /// (0 = never built).
    synced: AtomicU64,
}

impl ThemeState {
    fn sync(&self) {
        let Some(options) = self.options.get() else { return };
        let generation = options.generation() + 1;
        if self.synced.load(Ordering::Acquire) == generation {
            return;
        }
        let customs = options.value("theme.custom").map(|v| registry::customs_from_value(&v)).unwrap_or_default();
        let active = options.get_str("theme.active");
        if let Ok(mut reg) = self.registry.write() {
            reg.replace_customs(customs);
            if !reg.set_active(&active) {
                reg.set_active("default");
            }
        }
        self.synced.store(generation, Ordering::Release);
    }

    fn read(&self) -> Option<RwLockReadGuard<'_, ThemeRegistry>> {
        self.sync();
        self.registry.read().ok()
    }

    /// The `Configure` that stores `themes` as the custom themes.
    fn write_customs<'a>(themes: impl IntoIterator<Item = &'a NamedTheme>) -> Intent {
        Intent::Configure(ConfigWrite::SetOption { key: "theme.custom".into(), value: registry::customs_to_value(themes) })
    }

    /// The `Configure` that makes `name` the active theme.
    fn write_active(name: &str) -> Intent {
        Intent::Configure(ConfigWrite::SetOption { key: "theme.active".into(), value: OptionValue::Text(name.into()) })
    }
}

/// The live Color-in-panel editor (interior-mutable inside the extension,
/// mirroring CmdOp's own form — see `app/cmdop/builder.rs`). Hosted
/// in the function panel via `Intent::HostFunctionForm` (passive — the file
/// panel keeps focus), not a `Mode::InputForm` popup: nothing here round-trips
/// through `KEY_ITEM`, since the state is entirely local to this extension.
/// Cursor-follow: landing the file cursor on a custom theme's role entry opens
/// this automatically (`on_cursor_changed`); every hjkl edit writes straight
/// through — there is no confirm/cancel step, and no buffered value that could
/// diverge from what's on screen. Edits a role's fg, bg, and its three style
/// modifiers together — one leaf, one panel — instead of separate settings
/// entries per attribute.
struct ColorEdit {
    theme_name: String,
    role_tok: String,
    fg_rgb: (u8, u8, u8),
    /// `true` when fg is "nil" (`Color::Reset`, transparent) rather than
    /// `fg_rgb` — the RGB sliders keep their last value underneath so
    /// toggling nil off again restores it instead of resetting to black.
    fg_nil: bool,
    bg_rgb: (u8, u8, u8),
    bg_nil: bool,
    /// [bold, italic, underline] — indexed by `channel - MOD_CHANNEL_BASE`.
    mods: [bool; 3],
    /// Which control has focus — see the `*_CHANNEL*` constants below for the
    /// full layout (FG nil, then fg R/G/B, then the 3 modifiers, then BG nil,
    /// then bg R/G/B) — top-to-bottom, matching `render_color_form`.
    channel: usize,
}

/// `ColorEdit::channel` layout, top-to-bottom as drawn: the FG nil toggle
/// first, then fg's R/G/B (`FG_CHANNEL_BASE..`), then the three style
/// modifiers (`MOD_NAMES` order, `MOD_CHANNEL_BASE..`), then the BG nil
/// toggle, then bg's R/G/B (`BG_CHANNEL_BASE..`).
const FG_NIL_CHANNEL: usize = 0;
const FG_CHANNEL_BASE: usize = 1;
const MOD_CHANNEL_BASE: usize = 4;
const MOD_NAMES: [&str; 3] = ["Bold", "Italic", "Underline"];
const BG_NIL_CHANNEL: usize = MOD_CHANNEL_BASE + MOD_NAMES.len(); // 7
const BG_CHANNEL_BASE: usize = BG_NIL_CHANNEL + 1; // 8
const MAX_CHANNEL: usize = BG_CHANNEL_BASE + 2; // 10

/// The raw preset scheme source (include_str! at compile time).
pub const PRESETS_SCHEME: &str = include_str!("theme/presets.scm");

/// Extract `(theme_name, role_tok)` from a cursor path exactly two segments
/// below `@/settings/Display/Themes` — the shape `ThemeNode::list` gives every
/// per-role leaf. `None` for any other path (a shallower/deeper listing leaf,
/// or a path outside the Themes subtree entirely) — the cursor-follow gate in
/// `on_cursor_changed`.
fn theme_role_from_path(path: Option<&Path>) -> Option<(String, String)> {
    let base = vfs_root().join("settings").join("Display").join("Themes");
    let rel = path?.strip_prefix(&base).ok()?;
    let mut comps = rel.components();
    let theme = comps.next()?.as_os_str().to_str()?.to_string();
    let role = comps.next()?.as_os_str().to_str()?.to_string();
    if comps.next().is_some() {
        return None;
    }
    Some((theme, role))
}

// ── ThemeNode ─────────────────────────────────────────────────────────────────

/// Serves the `@/settings/Display/Themes` subtree via the `SettingsNode` graft.
struct ThemeNode {
    state: Arc<ThemeState>,
}

impl SettingsNode for ThemeNode {
    fn anchor(&self) -> (&str, &str) {
        ("Display", "Themes")
    }

    fn list(&self, path: &Path) -> io::Result<Vec<Entry>> {
        let reg = self.state.read().ok_or_else(|| io::Error::other("theme registry lock poisoned"))?;

        let depth = path.components().count();
        // @/settings/Display/Themes → depth 4
        // @/settings/Display/Themes/<name> → depth 5
        let base = vfs_root().join("settings").join("Display").join("Themes");

        if depth <= 4 {
            // Root of Themes: one dir entry per theme, then `+ New theme…`.
            let active = reg.active().to_string();
            let mut entries: Vec<Entry> = reg.names().into_iter().map(|name| {
                let nt = reg.get(&name).unwrap();
                let kind = if nt.preset { "preset" } else { "custom" };
                let badge = if name == active { format!("{name} ✓ ({kind})") } else { format!("{name} ({kind})") };
                Entry::virtual_dir(base.join(&name), badge)
            }).collect();
            let mut new_leaf = Entry::virtual_leaf(base.join("+ New theme…"), "+ New theme…".into());
            new_leaf.activation = Some(ExtensionIntent {
                extension: "theme".into(),
                intent: "new".into(),
                data: ExtensionValue::Nil,
            });
            entries.push(new_leaf);
            Ok(entries)
        } else {
            // Theme dir: get the theme name from component 4.
            let theme_name = path.components().nth(4)
                .and_then(|c| if let std::path::Component::Normal(s) = c { s.to_str() } else { None })
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "missing theme name in path"))?;

            let nt = reg.get(theme_name).ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, format!("theme '{theme_name}' not found"))
            })?;
            let is_preset = nt.preset;
            let active = reg.active().to_string();
            let resolved = reg.resolve(theme_name);
            let theme_base = base.join(theme_name);
            let mut entries: Vec<Entry> = Vec::new();

            // "Set active" leaf (omit if already active).
            if theme_name != active {
                let mut e = Entry::virtual_leaf(theme_base.join("set-active"), "Set active".into());
                e.activation = Some(ExtensionIntent {
                    extension: "theme".into(),
                    intent: "set-active".into(),
                    data: ExtensionValue::String(theme_name.to_string()),
                });
                entries.push(e);
            }

            // One leaf per role showing the current color.
            if is_preset {
                let mut header = Entry::virtual_leaf(theme_base.join("(preset)"), "preset — duplicate to edit".into());
                header.activation = Some(ExtensionIntent {
                    extension: "theme".into(),
                    intent: "duplicate".into(),
                    data: ExtensionValue::String(theme_name.to_string()),
                });
                entries.push(header);
            }
            // One leaf per role: fg, bg, and every style modifier are one
            // setting now, shown and edited together — activating it opens
            // the RGB-in-panel editor (`render_color_form`), which carries
            // Bold/Italic/Underline controls alongside both fg and bg R/G/B,
            // so there is nothing left to edit as a separate leaf.
            for role in Role::ALL {
                let tok = role.token();
                let fg = crate::core::color_token(resolved.role(*role));
                let bg = resolved.bg(*role);
                let set: Vec<&str> = [
                    (resolved.bold(*role), "bold"),
                    (resolved.italic(*role), "italic"),
                    (resolved.underline(*role), "underline"),
                ].into_iter().filter(|(on, _)| *on).map(|(_, name)| name).collect();
                let bg_part = if bg == Color::Reset { String::new() } else { format!(" / bg:{}", crate::core::color_token(bg)) };
                let mods_part = if set.is_empty() { String::new() } else { format!(" ({})", set.join(", ")) };
                let label = format!("{tok}: {fg}{bg_part}{mods_part}");
                let mut e = Entry::virtual_leaf(theme_base.join(tok), label);
                e.activation = Some(ExtensionIntent {
                    extension: "theme".into(),
                    intent: if is_preset { "duplicate" } else { "edit-role" }.to_string(),
                    data: ExtensionValue::Nil
                        .with("theme", ExtensionValue::String(theme_name.to_string()))
                        .with("role", ExtensionValue::String(tok.to_string())),
                });
                entries.push(e);
            }

            // Duplicate, Rename (custom only), Delete (custom only).
            let mut dup = Entry::virtual_leaf(theme_base.join("duplicate"), "Duplicate…".into());
            dup.activation = Some(ExtensionIntent {
                extension: "theme".into(),
                intent: "duplicate".into(),
                data: ExtensionValue::String(theme_name.to_string()),
            });
            entries.push(dup);

            if !is_preset {
                let mut rename = Entry::virtual_leaf(theme_base.join("rename"), "Rename…".into());
                rename.activation = Some(ExtensionIntent {
                    extension: "theme".into(),
                    intent: "rename".into(),
                    data: ExtensionValue::String(theme_name.to_string()),
                });
                entries.push(rename);

                let mut del = Entry::virtual_leaf(theme_base.join("delete"), "Delete".into());
                del.activation = Some(ExtensionIntent {
                    extension: "theme".into(),
                    intent: "delete".into(),
                    data: ExtensionValue::String(theme_name.to_string()),
                });
                entries.push(del);
            }

            Ok(entries)
        }
    }
}

// ── ThemeExtension ────────────────────────────────────────────────────────────

pub struct ThemeExtension {
    state: Arc<ThemeState>,
    color_edit: RefCell<Option<ColorEdit>>,
}

impl ThemeExtension {
    /// `registry` is shared with the frame, which resolves the active theme
    /// from it; the extension keeps it following the theme options.
    pub fn new(registry: Arc<RwLock<ThemeRegistry>>) -> Self {
        let state = ThemeState { registry, options: OnceLock::new(), synced: AtomicU64::new(0) };
        ThemeExtension { state: Arc::new(state), color_edit: RefCell::new(None) }
    }
}

impl Extension for ThemeExtension {
    fn id(&self) -> &str {
        ID
    }

    fn scheme_source(&self) -> Option<String> {
        Some(KEYMAP_SCHEME.to_string())
    }

    fn option_scheme(&self) -> Option<String> {
        Some(OPTIONS_SCHEME.to_string())
    }

    fn prelude(&self) -> Option<String> {
        Some(PRELUDE_SCHEME.to_string())
    }

    fn bind_store(&self, store: &Arc<OptionStore>) {
        let _ = self.state.options.set(store.clone());
        self.state.sync();
    }

    /// Follow the options before the next frame resolves the active theme.
    fn on_tick(&self, _state: &AppState) -> Vec<Intent> {
        self.state.sync();
        Vec::new()
    }

    fn register_conditions(&self, c: &mut Conditions) {
        // Gates the Color-in-panel editor's keys — reads only &AppState (the
        // generic form_owner), mirroring CmdOp's own "cmdop-active".
        c.register("theme-color-active", Box::new(|s| s.function.form_owner.as_deref() == Some(ID)));
    }

    fn about(&self) -> crate::extension::About {
        crate::extension::About::new("Theme", "Color themes: bundled presets and custom per-role overrides")
    }

    fn settings_nodes(&self) -> Vec<Arc<dyn SettingsNode>> {
        vec![Arc::new(ThemeNode { state: self.state.clone() })]
    }

    /// Cursor-follow: the file cursor landing on a custom theme's role leaf
    /// (`@/settings/Display/Themes/<theme>/<role>`) opens the color-in-panel
    /// editor for it automatically — no Enter, no separate "edit" step.
    /// Landing anywhere else (a preset's role, another leaf, another
    /// category) releases it, exactly like the content provider's own
    /// cursor-follow releases a stale preview. Passive: `HostFunctionForm`
    /// never touches focus, so the file panel keeps browsing with wasd while
    /// hjkl edits whatever role is currently under the cursor.
    fn on_cursor_changed(&self, state: &AppState) -> Vec<Intent> {
        if let Some((theme_name, role_tok)) = theme_role_from_path(state.cursor_path().as_deref())
            && let Some(out) = self.open_editor(&theme_name, &role_tok) {
                return out;
            }
        self.release()
    }

    fn handle_intent(&self, intent: &ExtensionIntent, _state: &AppState) -> Vec<Intent> {
        match intent.intent.as_str() {
            // ── Selection ──────────────────────────────────────────────────────
            "set-active" => {
                let Some(name) = intent.data.as_str() else { return vec![] };
                let known = self.state.read().is_some_and(|r| r.get(name).is_some());
                if !known { return vec![]; }
                vec![ThemeState::write_active(name)]
            }

            // ── New theme ──────────────────────────────────────────────────────
            "new" => {
                let template = ExtensionIntent {
                    extension: "theme".into(),
                    intent: "new-apply".into(),
                    data: ExtensionValue::Nil,
                };
                let spec = InputFormSpec::emit_input_form("New theme name", template, None);
                vec![Intent::PushMode(Box::new(Mode::InputForm(spec)))]
            }
            "new-apply" => {
                let Some(name_raw) = intent.data.get(KEY_ITEM).and_then(|v| v.as_str()) else {
                    return vec![];
                };
                let name = name_raw.trim().to_string();
                if name.is_empty() { return vec![]; }
                let Some(reg) = self.state.read() else { return vec![]; };
                if reg.get(&name).is_some() { return vec![]; }
                let new_theme = NamedTheme::seeded(name.clone(), reg.get(reg.active()));
                vec![
                    ThemeState::write_customs(reg.customs().chain(std::iter::once(&new_theme))),
                    ThemeState::write_active(&name),
                ]
            }

            // ── Duplicate ──────────────────────────────────────────────────────
            "duplicate" => {
                let source = match intent.data.as_str().map(str::to_string)
                    .or_else(|| intent.data.get("theme").and_then(|v| v.as_str()).map(str::to_string))
                {
                    Some(s) => s,
                    None => return vec![],
                };
                let template = ExtensionIntent {
                    extension: "theme".into(),
                    intent: "duplicate-apply".into(),
                    data: ExtensionValue::Nil
                        .with("source", ExtensionValue::String(source)),
                };
                let spec = InputFormSpec::emit_input_form("Duplicate as", template, None);
                vec![Intent::PushMode(Box::new(Mode::InputForm(spec)))]
            }
            "duplicate-apply" => {
                let Some(source_name) = intent.data.get("source").and_then(|v| v.as_str()) else {
                    return vec![];
                };
                let Some(new_name_raw) = intent.data.get(KEY_ITEM).and_then(|v| v.as_str()) else {
                    return vec![];
                };
                let source_name = source_name.to_string();
                let new_name = new_name_raw.trim().to_string();
                if new_name.is_empty() { return vec![]; }
                let Some(reg) = self.state.read() else { return vec![]; };
                if reg.get(&new_name).is_some() { return vec![]; }
                let new_theme = NamedTheme::seeded(new_name.clone(), reg.get(&source_name));
                vec![
                    ThemeState::write_customs(reg.customs().chain(std::iter::once(&new_theme))),
                    ThemeState::write_active(&new_name),
                ]
            }

            // ── Rename ─────────────────────────────────────────────────────────
            "rename" => {
                let Some(old_name) = intent.data.as_str() else { return vec![] };
                let template = ExtensionIntent {
                    extension: "theme".into(),
                    intent: "rename-apply".into(),
                    data: ExtensionValue::Nil
                        .with("source", ExtensionValue::String(old_name.to_string())),
                };
                let spec = InputFormSpec::emit_input_form("Rename to", template, Some(old_name.to_string()));
                vec![Intent::PushMode(Box::new(Mode::InputForm(spec)))]
            }
            "rename-apply" => {
                let Some(old_name) = intent.data.get("source").and_then(|v| v.as_str()) else {
                    return vec![];
                };
                let Some(new_name_raw) = intent.data.get(KEY_ITEM).and_then(|v| v.as_str()) else {
                    return vec![];
                };
                let old_name = old_name.to_string();
                let new_name = new_name_raw.trim().to_string();
                if new_name.is_empty() { return vec![]; }
                let Some(reg) = self.state.read() else { return vec![]; };
                if reg.get(&new_name).is_some() { return vec![]; }
                let Some(old_nt) = reg.get(&old_name) else { return vec![]; };
                if old_nt.preset { return vec![]; }
                let renamed = NamedTheme::seeded(new_name.clone(), Some(old_nt));
                let customs = reg.customs().map(|t| if t.name == old_name { &renamed } else { t });
                let mut out = vec![ThemeState::write_customs(customs)];
                if reg.active() == old_name {
                    out.push(ThemeState::write_active(&new_name));
                }
                out
            }

            // ── Edit role fg + bg + modifiers ─────────────────────────────────
            // Hosted in the function panel (two BarCharts — fg R/G/B, bg
            // R/G/B — a nil toggle for each, a Bold/Italic/Underline toggle
            // row, and a live swatch), not a Mode::InputForm popup: the RGB
            // sliders can't preserve a named-ANSI color's identity, so the
            // role's current fg/bg are seeded via `color_to_rgb` and every
            // edit writes back as a literal `#rrggbb` (or `nil` when the
            // color's own nil toggle is on) — an accepted tradeoff of this
            // widget. A role's whole look (fg, bg, weight, slant, rule) is
            // one setting. Primarily opened by cursor-follow
            // (`on_cursor_changed`, below); this intent is kept as an
            // idempotent manual fallback (harmless if the editor is already
            // open for the same role).
            "edit-role" => {
                let Some(theme_name) = intent.data.get("theme").and_then(|v| v.as_str()) else {
                    return vec![];
                };
                let Some(role_tok) = intent.data.get("role").and_then(|v| v.as_str()) else {
                    return vec![];
                };
                self.open_editor(theme_name, role_tok).unwrap_or_default()
            }
            "channel-prev" => {
                if let Some(ce) = self.color_edit.borrow_mut().as_mut() {
                    ce.channel = ce.channel.saturating_sub(1);
                }
                vec![]
            }
            "channel-next" => {
                if let Some(ce) = self.color_edit.borrow_mut().as_mut() {
                    ce.channel = (ce.channel + 1).min(MAX_CHANNEL);
                }
                vec![]
            }
            // Every step writes straight through — there is no confirm/cancel:
            // the editor never buffers a value the real theme doesn't have.
            "color-dec" => self.step_and_apply(-1),
            "color-inc" => self.step_and_apply(1),
            "color-dec-10" => self.step_and_apply(-10),
            "color-inc-10" => self.step_and_apply(10),

            // ── Delete ─────────────────────────────────────────────────────────
            "delete" => {
                let Some(name) = intent.data.as_str() else { return vec![] };
                let is_preset = self.state.read()
                    .and_then(|r| r.get(name).map(|nt| nt.preset))
                    .unwrap_or(true);
                if is_preset { return vec![]; }
                let confirm_intent = Intent::Extension(ExtensionIntent {
                    extension: "theme".into(),
                    intent: "delete-confirm".into(),
                    data: ExtensionValue::String(name.to_string()),
                });
                vec![Intent::PushMode(Box::new(Mode::Policy(Box::new(confirm_intent))))]
            }
            "delete-confirm" => {
                let Some(name) = intent.data.as_str() else { return vec![] };
                let Some(reg) = self.state.read() else { return vec![]; };
                let mut out = vec![ThemeState::write_customs(reg.customs().filter(|t| t.name != name))];
                if reg.active() == name {
                    out.push(ThemeState::write_active("default"));
                }
                // The deleted theme's directory is gone — return to the Themes root.
                out.push(Intent::NavigateTo(vfs_root().join("settings").join("Display").join("Themes")));
                out
            }

            _ => Vec::new(),
        }
    }

    fn render_form(&self, frame: &mut Frame, inner: Rect, ctx: &FunctionRenderCtx) -> Option<String> {
        self.render_color_form(frame, inner, ctx.theme)
    }
}

impl ThemeExtension {
    /// Set or remove the role's whole override (fg, bg, bold, italic,
    /// underline — one `RoleStyle`) in the named custom theme, then persist.
    /// Applying all of it at once is the point: a role's whole look is one
    /// setting from the editor's perspective, so there is one write, not
    /// several independent attributes racing each other. An override that now
    /// exactly matches `registry::default_style(role)` is removed rather than
    /// stored, keeping the theme's overrides list minimal.
    fn apply_role(&self, theme_name: &str, role_tok: &str, fg: Color, bg: Color, mods: [bool; 3]) -> Vec<Intent> {
        let Some(role) = Role::from_token(role_tok) else { return vec![]; };
        let new_style = RoleStyle { fg, bg, bold: mods[0], italic: mods[1], underline: mods[2] };
        let default = registry::default_style(role);

        let Some(reg) = self.state.read() else { return vec![]; };
        let Some(nt) = reg.get(theme_name) else { return vec![]; };
        if nt.preset { return vec![]; }

        let mut new_overrides = nt.overrides.clone();
        if new_style == default {
            new_overrides.retain(|(r, _)| *r != role);
        } else if let Some(pos) = new_overrides.iter().position(|(r, _)| *r == role) {
            new_overrides[pos].1 = new_style;
        } else {
            new_overrides.push((role, new_style));
        }
        let updated = NamedTheme { name: nt.name.clone(), preset: false, overrides: new_overrides };
        vec![ThemeState::write_customs(reg.customs().map(|t| if t.name == theme_name { &updated } else { t }))]
    }

    /// Seed (or refresh) the live `ColorEdit` from the named custom theme's
    /// resolved fg/bg/modifiers for `role_tok`, and passively host the panel.
    /// `None` when there is nothing to open: an unknown theme/role, or a
    /// preset (immutable — editing a preset's role goes through `duplicate`
    /// instead, unrelated to this seam). Idempotent: re-seeding the same
    /// theme+role that's already open is a harmless no-op re-affirmation
    /// (called both by cursor-follow on every cursor move and by the
    /// `edit-role` manual fallback).
    fn open_editor(&self, theme_name: &str, role_tok: &str) -> Option<Vec<Intent>> {
        let role = Role::from_token(role_tok)?;
        let reg = self.state.read()?;
        let nt = reg.get(theme_name)?;
        if nt.preset {
            return None;
        }
        let resolved = reg.resolve(theme_name);
        let (fg, bg) = (resolved.role(role), resolved.bg(role));
        *self.color_edit.borrow_mut() = Some(ColorEdit {
            theme_name: theme_name.to_string(),
            role_tok: role_tok.to_string(),
            fg_rgb: crate::core::color_to_rgb(fg),
            fg_nil: fg == Color::Reset,
            bg_rgb: crate::core::color_to_rgb(bg),
            bg_nil: bg == Color::Reset,
            mods: [resolved.bold(role), resolved.italic(role), resolved.underline(role)],
            channel: 0,
        });
        Some(vec![Intent::HostFunctionForm(Some(ID.into()))])
    }

    /// Close the live editor if one is open, releasing the panel. A no-op
    /// (no intents) when nothing was open, so cursor moves outside any
    /// theme role never spam a redundant release.
    fn release(&self) -> Vec<Intent> {
        if self.color_edit.borrow_mut().take().is_some() {
            vec![Intent::HostFunctionForm(None)]
        } else {
            vec![]
        }
    }

    /// Adjust the focused control of the live `ColorEdit` by `delta` (see
    /// `step_color`'s effect), then immediately persist the result via
    /// `apply_role` — every hjkl/HL press writes straight through, so the
    /// editor never buffers a value the real theme doesn't already have.
    fn step_and_apply(&self, delta: i32) -> Vec<Intent> {
        self.step_color(delta);
        let Some((theme_name, role_tok, fg, bg, mods)) = self.color_edit.borrow().as_ref().map(|ce| {
            let fg = if ce.fg_nil { Color::Reset } else { Color::Rgb(ce.fg_rgb.0, ce.fg_rgb.1, ce.fg_rgb.2) };
            let bg = if ce.bg_nil { Color::Reset } else { Color::Rgb(ce.bg_rgb.0, ce.bg_rgb.1, ce.bg_rgb.2) };
            (ce.theme_name.clone(), ce.role_tok.clone(), fg, bg, ce.mods)
        }) else {
            return vec![];
        };
        self.apply_role(&theme_name, &role_tok, fg, bg, mods)
    }

    /// Adjust the focused control of the live `ColorEdit` by `delta`: steps
    /// the focused fg or bg R/G/B channel (clamped to `0..255`), or flips a
    /// nil toggle or modifier (any nonzero `delta` toggles it — there is no
    /// continuous value for a checkbox). A no-op when no Color-in-panel
    /// editor is open.
    fn step_color(&self, delta: i32) {
        if let Some(ce) = self.color_edit.borrow_mut().as_mut() {
            match ce.channel {
                FG_NIL_CHANNEL => ce.fg_nil = !ce.fg_nil,
                FG_CHANNEL_BASE => ce.fg_rgb.0 = (ce.fg_rgb.0 as i32 + delta).clamp(0, 255) as u8,
                2 => ce.fg_rgb.1 = (ce.fg_rgb.1 as i32 + delta).clamp(0, 255) as u8,
                3 => ce.fg_rgb.2 = (ce.fg_rgb.2 as i32 + delta).clamp(0, 255) as u8,
                BG_NIL_CHANNEL => ce.bg_nil = !ce.bg_nil,
                BG_CHANNEL_BASE => ce.bg_rgb.0 = (ce.bg_rgb.0 as i32 + delta).clamp(0, 255) as u8,
                9 => ce.bg_rgb.1 = (ce.bg_rgb.1 as i32 + delta).clamp(0, 255) as u8,
                10 => ce.bg_rgb.2 = (ce.bg_rgb.2 as i32 + delta).clamp(0, 255) as u8,
                c => ce.mods[c - MOD_CHANNEL_BASE] = !ce.mods[c - MOD_CHANNEL_BASE],
            }
        }
    }

    /// Render the Color-in-panel editor: an fg BarChart (R/G/B), an "FG nil"
    /// toggle, a bg BarChart, a "BG nil" toggle, a Bold/Italic/Underline
    /// toggle row, and a live swatch of the role's actual resolved look —
    /// the same visual this used to draw as a `Mode::InputForm` popup, now
    /// drawn directly into the function panel's inner rect via the
    /// `render_form` seam. Degrades gracefully in a short panel: each section
    /// only draws if there's room, falling back to just the title.
    fn render_color_form(&self, frame: &mut Frame, inner: Rect, theme: &crate::core::Theme) -> Option<String> {
        let ce = self.color_edit.borrow();
        let ce = ce.as_ref()?;
        let fg_desc = if ce.fg_nil { "nil".to_string() } else { format!("#{:02x}{:02x}{:02x}", ce.fg_rgb.0, ce.fg_rgb.1, ce.fg_rgb.2) };
        let bg_desc = if ce.bg_nil { "nil".to_string() } else { format!("#{:02x}{:02x}{:02x}", ce.bg_rgb.0, ce.bg_rgb.1, ce.bg_rgb.2) };
        let set_mods: Vec<&str> = MOD_NAMES.iter().zip(ce.mods).filter(|(_, on)| *on).map(|(n, _)| *n).collect();
        let title = format!(
            "{} — {} (fg:{fg_desc} bg:{bg_desc}{})",
            ce.role_tok, ce.theme_name,
            if set_mods.is_empty() { String::new() } else { format!(", {}", set_mods.join("+").to_lowercase()) }
        );
        if inner.height == 0 {
            return Some(title);
        }

        let channel_style = |i: usize, base: Color| {
            let mut s = Style::default().fg(base);
            if i == ce.channel {
                s = s.add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
            }
            s
        };
        let bar = |label: &'static str, value: u8, i: usize, base: Color| {
            Bar::default()
                .label(Line::from(label))
                .value(value as u64)
                .style(channel_style(i, base))
                // Fixed-width (right-aligned to 3 digits, matching u8's max
                // "255"): a shorter "12" or "20" needs less width than "127"
                // to draw its text_value, so BarChart starts each bar's fill
                // at a different x offset unless every value claims the same
                // width up front.
                .text_value(format!("{value:>3}"))
        };
        let rgb_chart = |rgb: (u8, u8, u8), base_channel: usize| {
            BarChart::default()
                .direction(Direction::Horizontal)
                .bar_width(1)
                .bar_gap(0)
                .max(255)
                .data(BarGroup::default().bars(&[
                    bar("R", rgb.0, base_channel, Color::Red),
                    bar("G", rgb.1, base_channel + 1, Color::Green),
                    bar("B", rgb.2, base_channel + 2, Color::Blue),
                ]))
        };
        let nil_toggle = |frame: &mut Frame, area: Rect, label: &str, on: bool, channel: usize| {
            let text = format!("{label:<10}[{}]", if on { "x" } else { " " });
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(text, channel_style(channel, theme.role(Role::Base))))),
                area,
            );
        };

        // Top-to-bottom, matching the channel order above: FG nil, fg R/G/B,
        // Bold/Italic/Underline, a blank separator, BG nil, bg R/G/B, swatch,
        // hint. Each section only draws if there's room, falling back to just
        // the title in a short panel.
        let [fg_nil_row, rest] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .areas(inner);
        nil_toggle(frame, fg_nil_row, "FG nil", ce.fg_nil, FG_NIL_CHANNEL);
        if rest.height == 0 {
            return Some(title);
        }

        let bars_h = 3.min(rest.height);
        let [fg_bars, rest2] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(bars_h), Constraint::Min(0)])
            .areas(rest);
        frame.render_widget(rgb_chart(ce.fg_rgb, FG_CHANNEL_BASE), fg_bars);
        if rest2.height == 0 {
            return Some(title);
        }

        let [mods_area, rest3] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(MOD_NAMES.len() as u16), Constraint::Min(0)])
            .areas(rest2);
        // One row per modifier (not packed onto one line) so the `[ ]`
        // checkboxes line up in a column regardless of label length.
        let mod_rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints(vec![Constraint::Length(1); MOD_NAMES.len()])
            .split(mods_area);
        for (i, name) in MOD_NAMES.iter().enumerate() {
            nil_toggle(frame, mod_rows[i], name, ce.mods[i], MOD_CHANNEL_BASE + i);
        }
        if rest3.height == 0 {
            return Some(title);
        }

        let [_gap, rest4] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .areas(rest3);
        if rest4.height == 0 {
            return Some(title);
        }

        let [bg_nil_row, rest5] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .areas(rest4);
        nil_toggle(frame, bg_nil_row, "BG nil", ce.bg_nil, BG_NIL_CHANNEL);
        if rest5.height == 0 {
            return Some(title);
        }

        let bg_bars_h = 3.min(rest5.height);
        let [bg_bars, rest6] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(bg_bars_h), Constraint::Min(0)])
            .areas(rest5);
        frame.render_widget(rgb_chart(ce.bg_rgb, BG_CHANNEL_BASE), bg_bars);
        if rest6.height == 0 {
            return Some(title);
        }

        let [swatch, rest7] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .areas(rest6);
        // A live sample of the role's actual resolved look, not a blank
        // color block: fg, bg, and every modifier together, exactly what
        // wherever this role is used will render.
        let fg_color = if ce.fg_nil { Color::Reset } else { Color::Rgb(ce.fg_rgb.0, ce.fg_rgb.1, ce.fg_rgb.2) };
        let bg_color = if ce.bg_nil { Color::Reset } else { Color::Rgb(ce.bg_rgb.0, ce.bg_rgb.1, ce.bg_rgb.2) };
        let mut swatch_style = Style::default().fg(fg_color).bg(bg_color);
        if ce.mods[0] { swatch_style = swatch_style.add_modifier(Modifier::BOLD); }
        if ce.mods[1] { swatch_style = swatch_style.add_modifier(Modifier::ITALIC); }
        if ce.mods[2] { swatch_style = swatch_style.add_modifier(Modifier::UNDERLINED); }
        frame.render_widget(Paragraph::new(Span::styled(" Sample Aa ", swatch_style)), swatch);
        if rest7.height == 0 {
            return Some(title);
        }

        let hint = "j/k channel  h/l \u{b1}1  H/L \u{b1}10 (writes through immediately)";
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(hint, Style::default().fg(theme.role(Role::Muted))))),
            Rect { x: rest7.x, y: rest7.y, width: rest7.width, height: 1 },
        );
        Some(title)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_presets() -> Vec<NamedTheme> {
        registry::parse_presets(PRESETS_SCHEME)
    }

    fn theme_options() -> Arc<OptionStore> {
        Arc::new(OptionStore::new(crate::app::defaults::extension_options(&ThemeExtension::new(Arc::new(RwLock::new(ThemeRegistry::bundled()))))))
    }

    /// What the kernel does with a returned `Configure`: the store accepts it
    /// and the projection (the OptionStore) follows.
    fn apply(ext: &ThemeExtension, intents: Vec<Intent>) {
        let store = ext.state.options.get().unwrap();
        for i in intents {
            if let Intent::Configure(ConfigWrite::SetOption { key, value }) = i {
                assert!(store.set(&key, value), "the theme options accept {key}");
            }
        }
    }

    #[test]
    fn every_bundled_preset_declares_its_own_role_bold_entries() {
        let presets = parse_presets();
        for name in ["nord", "gruvbox", "light"] {
            let nt = presets.iter().find(|t| t.name == name).unwrap_or_else(|| panic!("{name} preset missing"));
            for role in registry::DEFAULT_BOLD_ROLES {
                let bold = nt.overrides.iter().any(|(r, s)| r == role && s.bold);
                assert!(bold, "{name} preset missing a bold override for {}", role.token());
            }
        }
    }

    #[test]
    fn presets_scm_bold_reflects_through_resolve() {
        let presets = parse_presets();
        let reg = ThemeRegistry::with_presets(presets);
        for name in ["default", "nord", "gruvbox", "light"] {
            let resolved = reg.resolve(name);
            assert!(resolved.bold(Role::EntryDir), "{name}: entry-dir should be bold");
            assert!(!resolved.bold(Role::Title), "{name}: title should not be bold by default");
        }
    }

    fn ext_with_custom(name: &str, source: &str) -> ThemeExtension {
        let ext = ThemeExtension::new(Arc::new(RwLock::new(ThemeRegistry::bundled())));
        ext.bind_store(&theme_options());
        let intent = ExtensionIntent {
            extension: "theme".into(),
            intent: "duplicate-apply".into(),
            data: ExtensionValue::Nil
                .with("source", ExtensionValue::String(source.into()))
                .with(KEY_ITEM, ExtensionValue::String(name.into())),
        };
        let state = AppState::new(std::env::temp_dir());
        let out = ext.handle_intent(&intent, &state);
        apply(&ext, out);
        ext
    }

    fn edit(ext: &ThemeExtension, theme_name: &str, role_tok: &str) {
        let intent = ExtensionIntent {
            extension: "theme".into(),
            intent: "edit-role".into(),
            data: ExtensionValue::Nil
                .with("theme", ExtensionValue::String(theme_name.into()))
                .with("role", ExtensionValue::String(role_tok.into())),
        };
        let state = AppState::new(std::env::temp_dir());
        ext.handle_intent(&intent, &state);
    }

    #[test]
    fn edit_role_seeds_bold_from_the_resolved_theme_not_just_the_color() {
        // gruvbox's entry-dir is bold (DEFAULT_BOLD_ROLES); a duplicate must
        // seed the color-in-panel editor's Bold control from that, not a
        // separate/stale toggle-bold path.
        let ext = ext_with_custom("mycopy", "gruvbox");
        edit(&ext, "mycopy", "entry-dir");
        let ce = ext.color_edit.borrow();
        let ce = ce.as_ref().expect("edit-role opens the color editor");
        assert_eq!(ce.role_tok, "entry-dir");
        assert!(ce.mods[0], "entry-dir is bold in gruvbox, so the seeded editor state must be bold too");
        assert_eq!(ce.fg_rgb, crate::core::color_to_rgb(ratatui::style::Color::Rgb(0x83, 0xa5, 0x98)));
        assert!(ce.bg_nil, "entry-dir carries no bg override in gruvbox");
    }

    #[test]
    fn edit_role_seeds_bold_false_for_a_non_bold_role() {
        let ext = ext_with_custom("mycopy", "gruvbox");
        edit(&ext, "mycopy", "entry-symlink");
        let ce = ext.color_edit.borrow();
        let ce = ce.as_ref().expect("edit-role opens the color editor");
        assert_eq!(ce.role_tok, "entry-symlink");
        assert!(!ce.mods[0], "entry-symlink carries no bold override in gruvbox");
    }

    #[test]
    fn channel_next_reaches_every_control_and_h_l_toggles_them() {
        let ext = ext_with_custom("mycopy", "gruvbox");
        edit(&ext, "mycopy", "entry-symlink"); // starts with no modifiers set, no bg override
        let state = AppState::new(std::env::temp_dir());
        let go = |intent: &str| {
            let out = ext.handle_intent(
                &ExtensionIntent { extension: "theme".into(), intent: intent.into(), data: ExtensionValue::Nil },
                &state,
            );
            apply(&ext, out);
        };
        for _ in 0..MOD_CHANNEL_BASE {
            go("channel-next");
        }
        assert_eq!(ext.color_edit.borrow().as_ref().unwrap().channel, MOD_CHANNEL_BASE, "reaches the Bold control");
        let underline_channel = MOD_CHANNEL_BASE + MOD_NAMES.len() - 1;
        for _ in MOD_CHANNEL_BASE..underline_channel {
            go("channel-next");
        }
        assert_eq!(ext.color_edit.borrow().as_ref().unwrap().channel, underline_channel, "reaches the Underline control");

        go("color-dec"); // h: toggles Underline (no continuous value on a checkbox)
        assert!(ext.color_edit.borrow().as_ref().unwrap().mods[2]);
        go("color-inc"); // l: toggles back
        assert!(!ext.color_edit.borrow().as_ref().unwrap().mods[2]);
        go("color-inc-10"); // L: also toggles
        assert!(ext.color_edit.borrow().as_ref().unwrap().mods[2]);

        for _ in 0..(MAX_CHANNEL + 5) {
            go("channel-next"); // way past the end
        }
        assert_eq!(ext.color_edit.borrow().as_ref().unwrap().channel, MAX_CHANNEL, "channel-next clamps at the last bg B control");
    }

    #[test]
    fn the_fg_and_bg_nil_toggles_are_reachable_and_flip_independently() {
        let ext = ext_with_custom("mycopy", "gruvbox");
        edit(&ext, "mycopy", "entry-symlink");
        let state = AppState::new(std::env::temp_dir());
        let go = |intent: &str| {
            let out = ext.handle_intent(
                &ExtensionIntent { extension: "theme".into(), intent: intent.into(), data: ExtensionValue::Nil },
                &state,
            );
            apply(&ext, out);
        };
        assert!(!ext.color_edit.borrow().as_ref().unwrap().fg_nil, "entry-symlink starts with a real fg color");
        assert!(ext.color_edit.borrow().as_ref().unwrap().bg_nil, "entry-symlink carries no bg override in gruvbox, so bg starts nil");
        assert_eq!(ext.color_edit.borrow().as_ref().unwrap().channel, FG_NIL_CHANNEL, "the edit starts on the FG-nil control");
        go("color-dec"); // toggle FG nil on
        assert!(ext.color_edit.borrow().as_ref().unwrap().fg_nil);
        assert!(ext.color_edit.borrow().as_ref().unwrap().bg_nil, "toggling fg-nil must not touch bg-nil");

        for _ in FG_NIL_CHANNEL..BG_NIL_CHANNEL {
            go("channel-next");
        }
        go("color-dec"); // toggle BG nil off (it starts nil, so this turns it on)
        assert!(!ext.color_edit.borrow().as_ref().unwrap().bg_nil);
    }

    #[test]
    fn every_step_writes_through_immediately_no_confirm_needed() {
        let ext = ext_with_custom("mycopy", "gruvbox");
        edit(&ext, "mycopy", "entry-symlink");
        let state = AppState::new(std::env::temp_dir());
        let go = |intent: &str| {
            let out = ext.handle_intent(
                &ExtensionIntent { extension: "theme".into(), intent: intent.into(), data: ExtensionValue::Nil },
                &state,
            );
            apply(&ext, out);
        };
        for _ in 0..MOD_CHANNEL_BASE {
            go("channel-next"); // -> Bold
        }
        go("color-dec"); // toggle Bold on

        // Persisted immediately — no confirm step exists, and the editor
        // stays open (cursor-follow, not a modal popup) so the user can keep
        // adjusting other channels for the same role.
        assert!(ext.color_edit.borrow().is_some(), "the editor stays open — there is nothing to close");
        let reg = ext.state.read().unwrap();
        let resolved = reg.resolve("mycopy");
        assert!(resolved.bold(Role::EntrySymlink), "the Bold toggle must persist as soon as it's pressed");
        // The color itself (untouched here) survives the same write.
        assert_eq!(resolved.role(Role::EntrySymlink), ratatui::style::Color::Rgb(0xd3, 0x86, 0x9b));
    }

    #[test]
    fn italic_and_underline_toggle_and_persist_independently_of_bold() {
        let ext = ext_with_custom("mycopy", "gruvbox");
        edit(&ext, "mycopy", "entry-symlink");
        let state = AppState::new(std::env::temp_dir());
        let go = |intent: &str| {
            let out = ext.handle_intent(
                &ExtensionIntent { extension: "theme".into(), intent: intent.into(), data: ExtensionValue::Nil },
                &state,
            );
            apply(&ext, out);
        };
        for _ in 0..(MOD_CHANNEL_BASE + 1) {
            go("channel-next"); // -> Italic
        }
        go("color-dec"); // toggle Italic on
        go("channel-next"); // -> Underline
        go("color-dec"); // toggle Underline on

        let reg = ext.state.read().unwrap();
        let resolved = reg.resolve("mycopy");
        assert!(resolved.italic(Role::EntrySymlink));
        assert!(resolved.underline(Role::EntrySymlink));
        assert!(!resolved.bold(Role::EntrySymlink), "toggling italic/underline must not flip bold");
    }

    #[test]
    fn apply_role_removes_the_override_once_it_matches_the_default_style_again() {
        // Exercises `apply_role` directly rather than through the RGB-slider
        // channels: those always reconstruct fg/bg as a literal `Color::Rgb`
        // (`color_to_rgb`'s approximation), which can never structurally
        // equal `Theme::default()`'s named-ANSI colors (e.g. `Color::Gray`)
        // even when it's a visually exact match — a known, accepted limit of
        // that widget, not what this test is about.
        let ext = ext_with_custom("mycopy", "gruvbox");
        let default = registry::default_style(Role::Title); // title has no override in gruvbox
        apply(&ext, ext.apply_role("mycopy", "title", default.fg, default.bg, [!default.bold, default.italic, default.underline]));
        assert!(ext.state.read().unwrap().get("mycopy").unwrap().overrides.iter().any(|(r, _)| *r == Role::Title));
        apply(&ext, ext.apply_role("mycopy", "title", default.fg, default.bg, [default.bold, default.italic, default.underline]));
        assert!(
            !ext.state.read().unwrap().get("mycopy").unwrap().overrides.iter().any(|(r, _)| *r == Role::Title),
            "an override identical to the default should be dropped, not stored"
        );
    }

    #[test]
    fn on_cursor_changed_opens_a_custom_roles_editor_and_releases_off_it() {
        let ext = ext_with_custom("mycopy", "gruvbox");
        let themes_root = vfs_root().join("settings").join("Display").join("Themes");
        let mut state = AppState::new(std::env::temp_dir());

        let entry_at = |path: std::path::PathBuf| crate::core::Entry {
            path,
            name: "x".into(),
            is_dir: false,
            is_symlink: false,
            size: 0,
            mode: 0,
            uid: 0,
            gid: 0, owner: "".into(), group: "".into(),
            modified: None,
            created: None,
            accessed: None,
            symlink_target: None,
            activation: None,
        };

        // Landing on a custom theme's role opens the editor.
        state.entries = vec![entry_at(themes_root.join("mycopy").join("entry-dir"))];
        state.cursor = 0;
        let out = ext.on_cursor_changed(&state);
        assert_eq!(out, vec![Intent::HostFunctionForm(Some("theme".into()))]);
        assert_eq!(ext.color_edit.borrow().as_ref().unwrap().role_tok, "entry-dir");

        // Moving to a non-role leaf releases it.
        state.entries = vec![entry_at(themes_root.join("mycopy").join("rename"))];
        let out = ext.on_cursor_changed(&state);
        assert_eq!(out, vec![Intent::HostFunctionForm(None)]);
        assert!(ext.color_edit.borrow().is_none());

        // Releasing again (nothing open) is a silent no-op.
        assert_eq!(ext.on_cursor_changed(&state), Vec::<Intent>::new());
    }

    #[test]
    fn on_cursor_changed_ignores_a_presets_role() {
        // gruvbox is still a preset (untouched) — landing on its own role
        // entries must not open a live editor (presets are immutable).
        let ext = ext_with_custom("mycopy", "gruvbox");
        let themes_root = vfs_root().join("settings").join("Display").join("Themes");
        let mut state = AppState::new(std::env::temp_dir());
        state.entries = vec![crate::core::Entry {
            path: themes_root.join("gruvbox").join("entry-dir"),
            name: "x".into(),
            is_dir: false,
            is_symlink: false,
            size: 0,
            mode: 0,
            uid: 0,
            gid: 0, owner: "".into(), group: "".into(),
            modified: None,
            created: None,
            accessed: None,
            symlink_target: None,
            activation: None,
        }];
        state.cursor = 0;
        assert_eq!(ext.on_cursor_changed(&state), Vec::<Intent>::new());
        assert!(ext.color_edit.borrow().is_none());
    }

    #[test]
    fn the_prelude_defines_and_selects_themes_through_the_store() {
        use crate::script::store::{Layer, Store, CORE_VOCABULARY};
        let session = crate::script::SchemeSession::spawn();
        assert!(session.wait_ready(std::time::Duration::from_secs(30)));
        let store = Store::new(session);
        let options = theme_options();
        let defs: Vec<_> = options.keys().iter().filter_map(|k| options.def(k).cloned()).collect();
        store.declare_options(&defs).unwrap();
        let mut taken = CORE_VOCABULARY.iter().map(|s| s.to_string()).collect();
        let problems = store.load_prelude(ID, PRELUDE_SCHEME, &mut taken);
        assert!(problems.is_empty(), "{problems:?}");
        // The old managed-block form is the new config.scm form: no quoting.
        let (changes, errors) = store.eval_program(
            Layer::Config,
            "(define-theme \"x\" (title \"#112233\" nil #t #f #f))\n(set-theme \"x\")",
        );
        assert!(errors.is_empty(), "{errors:?}");
        assert!(crate::script::projection::apply_options(&changes, &options).is_empty());
        assert_eq!(options.get_str("theme.active"), "x");
        let customs = registry::customs_from_value(&options.value("theme.custom").unwrap());
        assert_eq!(customs.len(), 1);
        let (role, style) = customs[0].overrides[0];
        assert_eq!(role, Role::Title);
        assert_eq!(style.fg, Color::Rgb(0x11, 0x22, 0x33));
        assert_eq!(style.bg, Color::Reset);
        assert!(style.bold && !style.italic);
    }
}
