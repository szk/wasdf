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

    /// Resolve (mode, focused panel, key) to an intent by walking the **mode
    /// stack** top-down: try the topmost mode's scope, and on no match fall
    /// through to the mode below — but only past **overlay** modes. A modal mode
    /// (File / Select / Policy, or a non-overlay extension mode) stops the walk,
    /// capturing the key. This is the single key-input stack: an overlay adds its
    /// own bindings on top while unhandled keys reach the mode it covers.
    pub fn resolve(
        &self,
        conds: &Conditions,
        state: &AppState,
        panel: &str,
        key: Key,
    ) -> Option<Intent> {
        for frame in state.modes.iter().rev() {
            let mode_id = frame.mode.id();
            if let Some(intent) = self.resolve_in_mode(conds, state, &mode_id, panel, key) {
                return Some(intent);
            }
            if frame.mode.captures_keys() {
                break;
            }
        }
        None
    }

    /// Resolve within a single mode's scope. Scope specificity first (longest
    /// mode-prefix, then panel-specific), then layer priority, then whether the
    /// binding is conditioned at all: among same-mode/panel/layer bindings for
    /// the same key, a real `:when` (e.g. an extension's "…-active" gate on
    /// `form_owner`) beats an unconditional one, so a form hosting the function
    /// panel (CmdOp, Theme's color editor) can reclaim a key another
    /// unconditionally-bound extension (Preview) also declared in the same
    /// scope — without this, the first-registered unconditional binding would
    /// win every tie regardless of which one actually applies right now.
    fn resolve_in_mode(
        &self,
        conds: &Conditions,
        state: &AppState,
        mode_id: &str,
        panel: &str,
        key: Key,
    ) -> Option<Intent> {
        let mut best: Option<(&Binding, usize, u8, u8, u8)> = None;
        for b in &self.bindings {
            if b.key != key {
                continue;
            }
            if !mode_scope_matches(&b.mode, mode_id) {
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
            let specificity: u8 = if matches!(b.when, Cond::Always) { 0 } else { 1 };
            let better = match best {
                None => true,
                Some((_, bm, bp, bl, bs)) => {
                    (mode_len, panel_score, layer_rank, specificity) > (bm, bp, bl, bs)
                }
            };
            if better {
                best = Some((b, mode_len, panel_score, layer_rank, specificity));
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
    // Capped so nested `f` (filtering within an already-filtered listing, each
    // a fresh walk — see `commit_file_search`) can't grow the filter stack
    // unboundedly. Deep search needs a real directory to walk; a virtual cwd
    // (@/...) has nothing for it to read, so `f` there means shallow filter
    // instead (SEARCH_RENEW.md).
    core(
        "file", f, ch('f'), select_mode(SelectSpec::file_search()),
        Cond::And(vec![Cond::not(Cond::pred("filter-maxed")), Cond::not(Cond::pred("cwd-virtual"))]),
    );
    core(
        "file", f, ch('f'), Intent::FilterListing,
        Cond::And(vec![Cond::not(Cond::pred("filter-maxed")), Cond::pred("cwd-virtual")]),
    );
    core("file", f, ch('x'), select_mode(SelectSpec::command_palette(Vec::new())), Cond::Always);
    core("file", f, ch('W'), Intent::CursorTop, Cond::Always);
    core("file", f, ch('S'), Intent::CursorBottom, Cond::Always);
    core("file", f, ctrl('a'), Intent::SelectAll, Cond::Always);
    // c / m / R (copy / move / rename) are the CmdOp extension's entry keys now
    // (its Scheme keymap); the Rust fallback keymap leaves them unbound.
    core("file", f, ch('e'), Intent::StartEdit, Cond::Always);
    core("file", f, ch(','), Intent::CycleFunctionPanel, Cond::Always);
    core("file", f, ch('.'), Intent::ToggleDotFiles, Cond::Always);
    core("file", f, ch('v'), Intent::CycleListLayout, Cond::Always);
    // Esc steps back one filter level while one is active (has-filter beats
    // the unconditioned binding below by specificity — commit_file_search
    // lands a filtered listing in this very scope), else clears the selection.
    core("file", f, sp(KeyCode::Esc), Intent::ClearFilter, Cond::pred("has-filter"));
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
    // wasd always targets the file panel, so pressing it from the function panel
    // moves the file cursor and (via `intent_focus`) returns focus to the file.
    core("file", fp, ch('w'), Intent::CursorUp, Cond::Always);
    core("file", fp, ch('s'), Intent::CursorDown, Cond::Always);
    core("file", fp, ch('a'), Intent::CursorLeft, Cond::Always);
    core("file", fp, ch('d'), Intent::CursorRight, Cond::Always);
    core("file", fp, ch('j'), Intent::ScrollDown, Cond::Always);
    core("file", fp, sp(KeyCode::Down), Intent::ScrollDown, Cond::Always);
    core("file", fp, ch('k'), Intent::ScrollUp, Cond::Always);
    core("file", fp, sp(KeyCode::Up), Intent::ScrollUp, Cond::Always);
    core("file", fp, ch('h'), Intent::FuncLeft, Cond::pred("sublayout-content"));
    core("file", fp, sp(KeyCode::Left), Intent::FuncLeft, Cond::pred("sublayout-content"));
    core("file", fp, ch('l'), Intent::FuncRight, Cond::pred("sublayout-content"));
    core("file", fp, sp(KeyCode::Right), Intent::FuncRight, Cond::pred("sublayout-content"));
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
    // Shallow refine: rank the current result view (deep file-search, or an
    // earlier filter/refine) further, in place. See SEARCH_RENEW.md.
    core(
        "select", None, ch('f'), Intent::SelectRefine,
        Cond::And(vec![nav(), Cond::pred("select-has-view")]),
    );
    core("select", None, ch('q'), Intent::Cancel, nav());

    // --- Policy mode ---
    core("policy", None, ch('y'), Intent::Confirm, Cond::Always);
    core("policy", None, sp(KeyCode::Enter), Intent::Confirm, Cond::Always);
    core("policy", None, ch('n'), Intent::Cancel, Cond::Always);
    core("policy", None, sp(KeyCode::Esc), Intent::Cancel, Cond::Always);

    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{AppState, ExtensionValue};

    fn ext_binding(mode: &str, key: Key, intent: Intent) -> Binding {
        Binding { mode: mode.into(), panel: None, key, intent, when: Cond::Always, layer: Layer::Extension }
    }

    fn overlay(id_ext: &str, id_mode: &str) -> Mode {
        Mode::Extension { extension: id_ext.into(), mode: id_mode.into(), state: ExtensionValue::Nil, overlay: true }
    }

    #[test]
    fn esc_clears_a_filter_before_falling_back_to_clear_selection() {
        let mut km = KeymapRegistry::new();
        km.extend(defaults());
        let conds = Conditions::default();
        let mut state = AppState::new(std::env::temp_dir());

        assert_eq!(
            km.resolve(&conds, &state, "file", sp(KeyCode::Esc)),
            Some(Intent::ClearSelection),
            "no filter active: Esc falls back to the unconditioned binding"
        );

        state.filter_stack.push(crate::core::state::FilterFrame::default());
        assert_eq!(
            km.resolve(&conds, &state, "file", sp(KeyCode::Esc)),
            Some(Intent::ClearFilter),
            "a filter is active: the has-filter binding wins by specificity"
        );
    }

    #[test]
    fn file_search_is_unavailable_once_the_filter_stack_is_maxed() {
        let mut km = KeymapRegistry::new();
        km.extend(defaults());
        let conds = Conditions::default();
        let mut state = AppState::new(std::env::temp_dir());

        assert!(km.resolve(&conds, &state, "file", ch('f')).is_some(), "f starts a filter normally");

        for _ in 0..crate::core::state::MAX_FILTER_DEPTH {
            state.filter_stack.push(crate::core::state::FilterFrame::default());
        }
        assert_eq!(
            km.resolve(&conds, &state, "file", ch('f')),
            None,
            "f is gated off once the filter stack is at MAX_FILTER_DEPTH"
        );
    }

    #[test]
    fn f_is_deep_on_a_real_cwd_and_shallow_on_a_virtual_cwd() {
        let mut km = KeymapRegistry::new();
        km.extend(defaults());
        let conds = Conditions::default();
        let mut state = AppState::new(std::env::temp_dir());

        match km.resolve(&conds, &state, "file", ch('f')) {
            Some(Intent::PushMode(mode)) => match *mode {
                Mode::Select(spec) => assert_eq!(spec.id, "file-search", "a real cwd walks deep"),
                other => panic!("expected a Select push, got {other:?}"),
            },
            other => panic!("expected file-search on a real cwd, got {other:?}"),
        }

        state.cwd = crate::vfs::vfs_root().join("settings");
        assert_eq!(
            km.resolve(&conds, &state, "file", ch('f')),
            Some(Intent::FilterListing),
            "a virtual cwd has nothing for the walk to read: f filters the listing instead"
        );
    }

    #[test]
    fn select_refine_f_is_gated_on_an_existing_result_view() {
        use crate::core::{EntryList, SelectPhase, SelectSpec};
        use crate::core::state::SelectState;
        let mut km = KeymapRegistry::new();
        km.extend(defaults());
        let conds = Conditions::default();
        let mut state = AppState::new(std::env::temp_dir());
        state.push_mode(Mode::Select(SelectSpec::file_search()));
        let mut sel = SelectState::new(String::new(), true);
        sel.phase = SelectPhase::Navigate;
        state.select = Some(sel);

        assert_eq!(
            km.resolve(&conds, &state, "select", ch('f')),
            None,
            "no result view yet: f is not select-refine"
        );

        state.select.as_mut().unwrap().view = Some(EntryList { entries: file_entries_stub(1), cursor: 0 });
        assert_eq!(
            km.resolve(&conds, &state, "select", ch('f')),
            Some(Intent::SelectRefine),
            "a non-empty result view: f refines it"
        );
    }

    fn file_entries_stub(n: usize) -> Vec<crate::core::Entry> {
        (0..n)
            .map(|i| crate::core::Entry {
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
            .collect()
    }

    #[test]
    fn overlay_mode_falls_through_to_the_mode_below() {
        let mut km = KeymapRegistry::new();
        km.extend(defaults());
        // An overlay binds `o` in its own scope; `wasd` it does not bind.
        km.add(ext_binding("ext:info", ch('o'), Intent::Noop));

        let mut state = AppState::new(std::env::temp_dir());
        state.push_mode(overlay("ext", "info"));
        let conds = Conditions::default();

        // The overlay's own key resolves in its scope.
        assert_eq!(km.resolve(&conds, &state, "file", ch('o')), Some(Intent::Noop));
        // An unbound key falls through to the File mode beneath (cursor nav).
        assert_eq!(km.resolve(&conds, &state, "file", ch('w')), Some(Intent::CursorUp));
    }

    #[test]
    fn a_conditioned_binding_beats_an_unconditional_one_at_the_same_scope_and_layer() {
        // Mirrors Preview's unconditional "l" in (file function (...)) versus
        // CmdOp's/Theme's own "l", gated by their own "…-active" predicate —
        // registered first (as Preview's scheme_source is, in `bundled()`
        // order), an unconditional binding must not permanently win the tie
        // just because it was added earlier.
        let mut km = KeymapRegistry::new();
        km.add(Binding {
            mode: "file".into(),
            panel: Some("function".into()),
            key: ch('l'),
            intent: Intent::FuncRight,
            when: Cond::Always,
            layer: Layer::Extension,
        });
        km.add(Binding {
            mode: "file".into(),
            panel: Some("function".into()),
            key: ch('l'),
            intent: Intent::Noop,
            when: Cond::pred("form-active"),
            layer: Layer::Extension,
        });

        let mut conds = Conditions::default();
        conds.register("form-active", Box::new(|_| true));
        let state = AppState::new(std::env::temp_dir());

        assert_eq!(
            km.resolve(&conds, &state, "function", ch('l')),
            Some(Intent::Noop),
            "the conditioned binding should win while its predicate holds"
        );
    }

    #[test]
    fn modal_mode_captures_and_does_not_fall_through() {
        let mut km = KeymapRegistry::new();
        km.extend(defaults());

        let mut state = AppState::new(std::env::temp_dir());
        // A modal (non-overlay) extension mode that binds nothing.
        state.push_mode(Mode::Extension {
            extension: "ext".into(),
            mode: "modal".into(),
            state: ExtensionValue::Nil,
            overlay: false,
        });
        let conds = Conditions::default();

        // `w` is bound in File mode, but the modal mode captures — no fall-through.
        assert_eq!(km.resolve(&conds, &state, "file", ch('w')), None);
    }
}
