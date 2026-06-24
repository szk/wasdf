//! The KeymapRegistry: the only key-to-Intent path. Layered (Core < Extension
//! < User) with scope specificity (longest mode-id prefix, then panel scope)
//! applied before layer priority. `:when` conditions are evaluated natively.

use crate::core::{AppState, Intent, Key, KeyCode, Mode, Mods, SelectSpec};
use crate::script::condition::{Cond, Conditions};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Layer {
    Core,
    Extension,
    User,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Binding {
    /// Mode-id scope (prefix-matched on segment boundaries).
    pub mode: String,
    /// Optional panel scope; None matches any focused panel.
    pub panel: Option<String>,
    pub key: Key,
    pub intent: Intent,
    pub when: Cond,
    pub layer: Layer,
}

#[derive(Default)]
pub struct KeymapRegistry {
    bindings: Vec<Binding>,
    warnings: Vec<String>,
}

impl KeymapRegistry {
    pub fn new() -> Self {
        KeymapRegistry::default()
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Add a binding, detecting collisions on the exact (mode, panel, key, when)
    /// tuple across layers.
    pub fn add(&mut self, b: Binding) {
        if let Some(existing) = self.bindings.iter().find(|e| {
            e.mode == b.mode && e.panel == b.panel && e.key == b.key && e.when == b.when
        }) {
            if existing.layer != b.layer {
                self.warnings.push(format!(
                    "keymap collision on {:?} in mode '{}': {:?} vs {:?}",
                    b.key, b.mode, existing.layer, b.layer
                ));
            }
        }
        self.bindings.push(b);
    }

    pub fn extend(&mut self, bindings: impl IntoIterator<Item = Binding>) {
        for b in bindings {
            self.add(b);
        }
    }

    /// Resolve (mode, focused panel, key) to an intent. Scope specificity first
    /// (longest mode-prefix, then panel-specific), then layer priority.
    pub fn resolve(
        &self,
        conds: &Conditions,
        state: &AppState,
        panel: &str,
        key: Key,
    ) -> Option<Intent> {
        let mode_id = state.mode_id();
        let mut best: Option<(&Binding, usize, u8, u8)> = None;
        for b in &self.bindings {
            if b.key != key {
                continue;
            }
            if !mode_scope_matches(&b.mode, &mode_id) {
                continue;
            }
            let panel_score = match &b.panel {
                Some(p) if p == panel => 1u8,
                Some(_) => continue,
                None => 0u8,
            };
            if !conds.eval(&b.when, state) {
                continue;
            }
            let mode_len = b.mode.len();
            let layer_rank = b.layer as u8;
            let better = match best {
                None => true,
                Some((_, bm, bp, bl)) => {
                    (mode_len, panel_score, layer_rank) > (bm, bp, bl)
                }
            };
            if better {
                best = Some((b, mode_len, panel_score, layer_rank));
            }
        }
        best.map(|(b, ..)| b.intent.clone())
    }
}

/// A mode scope matches when it equals the mode id or is a segment-boundary
/// prefix of it.
fn mode_scope_matches(scope: &str, mode_id: &str) -> bool {
    mode_id == scope || mode_id.starts_with(&format!("{scope}:"))
}

// --- key constructors -------------------------------------------------------

fn ch(c: char) -> Key {
    Key { code: KeyCode::Char(c), mods: Mods::NONE }
}
fn ctrl(c: char) -> Key {
    Key { code: KeyCode::Char(c), mods: Mods::CTRL }
}
fn sp(code: KeyCode) -> Key {
    Key { code, mods: Mods::NONE }
}

fn select_mode(spec: SelectSpec) -> Intent {
    Intent::PushMode(Box::new(Mode::Select(spec)))
}

/// The embedded default keymap (the Scheme keymap source analogue), all on the
/// Core layer.
pub fn defaults() -> Vec<Binding> {
    let mut v = Vec::new();
    let mut core = |mode: &str, panel: Option<&str>, key: Key, intent: Intent, when: Cond| {
        v.push(Binding {
            mode: mode.into(),
            panel: panel.map(|s| s.to_string()),
            key,
            intent,
            when,
            layer: Layer::Core,
        });
    };

    // --- File mode, file panel ---
    let f = Some("file");
    core("file", f, ch('w'), Intent::CursorUp, Cond::Always);
    core("file", f, ch('s'), Intent::CursorDown, Cond::Always);
    core("file", f, ch('a'), Intent::CursorLeft, Cond::Always);
    core("file", f, ch('d'), Intent::CursorRight, Cond::Always);
    core("file", f, sp(KeyCode::Enter), Intent::Activate, Cond::Always);
    core("file", f, ch('k'), Intent::FuncUp, Cond::Always);
    core("file", f, sp(KeyCode::Up), Intent::FuncUp, Cond::Always);
    core("file", f, ch('j'), Intent::FuncDown, Cond::Always);
    core("file", f, sp(KeyCode::Down), Intent::FuncDown, Cond::Always);
    core("file", f, ch('h'), Intent::FuncLeft, Cond::Always);
    core("file", f, sp(KeyCode::Left), Intent::FuncLeft, Cond::Always);
    core("file", f, ch('l'), Intent::FuncRight, Cond::Always);
    core("file", f, sp(KeyCode::Right), Intent::FuncRight, Cond::Always);
    core("file", f, ch(' '), Intent::ToggleSelect, Cond::Always);
    core("file", f, ch('f'), select_mode(SelectSpec::file_search()), Cond::Always);
    core("file", f, ch('x'), select_mode(SelectSpec::command_palette(Vec::new())), Cond::Always);
    core("file", f, ch('W'), Intent::CursorTop, Cond::Always);
    core("file", f, ch('S'), Intent::CursorBottom, Cond::Always);
    core("file", f, ctrl('a'), Intent::SelectAll, Cond::Always);
    core("file", f, ch('c'), Intent::StartCopy, Cond::Always);
    core("file", f, ch('m'), Intent::StartMove, Cond::Always);
    core("file", f, ch('R'), Intent::StartRename, Cond::Always);
    core("file", f, ch('e'), Intent::StartEdit, Cond::Always);
    core("file", f, ch(','), Intent::CycleFunctionPanel, Cond::Always);
    core("file", f, ch('.'), Intent::ToggleDotFiles, Cond::Always);
    core("file", f, ch('v'), Intent::CycleListLayout, Cond::Always);
    core("file", f, sp(KeyCode::Esc), Intent::ClearSelection, Cond::Always);
    core("file", f, sp(KeyCode::Backspace), Intent::DeleteSelected, Cond::Always);
    core("file", f, sp(KeyCode::Delete), Intent::DeleteSelected, Cond::Always);
    core("file", f, ch('r'), Intent::Refresh, Cond::Always);
    core("file", f, sp(KeyCode::Tab), Intent::FocusNextPanel, Cond::Always);
    core("file", f, sp(KeyCode::BackTab), Intent::FocusPrevPanel, Cond::Always);
    core("file", f, ch('q'), Intent::Quit, Cond::Always);
    core("file", f, ctrl('c'), Intent::Quit, Cond::Always);

    // --- File mode, function panel (panel-scoped) ---
    // Only the truly core bindings live here: vertical scroll (shared with the
    // Exec frame), line numbers, open/back, kill, cycle, hide. The content
    // extension contributes its own function-panel bindings (search, horizontal
    // scroll) at the Extension layer, overriding these by layer priority.
    let fp = Some("function");
    core("file", fp, ch('j'), Intent::ScrollDown, Cond::Always);
    core("file", fp, sp(KeyCode::Down), Intent::ScrollDown, Cond::Always);
    core("file", fp, ch('k'), Intent::ScrollUp, Cond::Always);
    core("file", fp, sp(KeyCode::Up), Intent::ScrollUp, Cond::Always);
    core("file", fp, ch('g'), Intent::ScrollTop, Cond::Always);
    core("file", fp, ch('G'), Intent::ScrollBottom, Cond::Always);
    core("file", fp, ch('n'), Intent::ToggleLineNumbers, Cond::pred("sublayout-content"));
    core(
        "file",
        fp,
        sp(KeyCode::Enter),
        Intent::Open { path: std::path::PathBuf::new() },
        Cond::pred("sublayout-content"),
    );
    core(
        "file",
        fp,
        sp(KeyCode::Enter),
        Intent::SetSubLayout(crate::core::SubLayout::Content),
        Cond::pred("sublayout-exec"),
    );
    core("file", fp, ctrl('c'), Intent::KillProcess, Cond::pred("sublayout-exec"));
    core("file", fp, ch(','), Intent::CycleFunctionPanel, Cond::Always);
    core("file", fp, sp(KeyCode::Esc), Intent::HideFunctionPanel, Cond::Always);
    core("file", fp, ch('q'), Intent::HideFunctionPanel, Cond::Always);

    // --- Select mode (shared by every picker) ---
    core("select", None, sp(KeyCode::Enter), Intent::Confirm, Cond::Always);
    core("select", None, sp(KeyCode::Esc), Intent::Cancel, Cond::Always);
    // Results navigate through the shared Cursor* intents (the same as the file
    // panel), so path results (file-search) get its grid layouts and 2-D moves.
    core("select", None, ctrl('n'), Intent::CursorDown, Cond::Always);
    core("select", None, sp(KeyCode::Down), Intent::CursorDown, Cond::Always);
    core("select", None, ctrl('p'), Intent::CursorUp, Cond::Always);
    core("select", None, sp(KeyCode::Up), Intent::CursorUp, Cond::Always);
    let nav = || Cond::pred("select-phase-navigate");
    core("select", None, ch('w'), Intent::CursorUp, nav());
    core("select", None, ch('s'), Intent::CursorDown, nav());
    core("select", None, ch('a'), Intent::CursorLeft, nav());
    core("select", None, ch('d'), Intent::CursorRight, nav());
    core("select", None, ch('W'), Intent::CursorTop, nav());
    core("select", None, ch('S'), Intent::CursorBottom, nav());
    core("select", None, ch('v'), Intent::CycleListLayout, nav());
    core("select", None, ch(' '), Intent::ToggleSelect, nav());
    core("select", None, ch('q'), Intent::Cancel, nav());

    // --- Policy mode ---
    core("policy", None, ch('y'), Intent::Confirm, Cond::Always);
    core("policy", None, sp(KeyCode::Enter), Intent::Confirm, Cond::Always);
    core("policy", None, ch('n'), Intent::Cancel, Cond::Always);
    core("policy", None, sp(KeyCode::Esc), Intent::Cancel, Cond::Always);

    v
}
