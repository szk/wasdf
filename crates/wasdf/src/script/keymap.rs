//! The KeymapRegistry: the only key-to-Intent path. Layered (Core < Extension
//! < User) with scope specificity (longest mode-id prefix, then panel scope)
//! applied before layer priority. `:when` conditions are evaluated natively.

use crate::core::{AppState, Intent, Key, KeyKind};
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
    /// The help-row label for this binding, e.g. `"copy"`. `None` — the
    /// default — keeps it out of the help row.
    ///
    /// Opt-in rather than derived from the intent: the file panel binds about
    /// thirty keys and the row is one line, so *which* to advertise is an
    /// editorial choice. What is not editorial is the key, and that now always
    /// comes from the binding that would really fire (see doc/HELP.md).
    pub help: Option<String>,
    /// The extension that declared it (Extension layer only): its rank orders
    /// the layer, so a collision inside it goes to the higher-priority one.
    pub owner: Option<String>,
}

#[derive(Default)]
pub struct KeymapRegistry {
    bindings: Vec<Binding>,
    warnings: Vec<String>,
    /// The store's `help` display order: labels listed here lead the help row.
    help_order: Vec<String>,
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
        }) && (existing.layer != b.layer || existing.owner != b.owner)
        {
            let side = |b: &Binding| match &b.owner {
                Some(o) => format!("{:?} ({o})", b.layer),
                None => format!("{:?}", b.layer),
            };
            self.warnings.push(format!(
                "keymap collision on {:?} in mode '{}': {} vs {}",
                b.key, b.mode, side(existing), side(&b)
            ));
        }
        self.bindings.push(b);
    }

    pub fn extend(&mut self, bindings: impl IntoIterator<Item = Binding>) {
        for b in bindings {
            self.add(b);
        }
    }

    /// Add an extension's bindings, recording it as their owner.
    pub fn extend_owned(&mut self, owner: &str, bindings: impl IntoIterator<Item = Binding>) {
        self.extend(bindings.into_iter().map(|b| Binding { owner: Some(owner.to_string()), ..b }));
    }

    /// Order the Extension layer by extension priority (`ranked`, highest
    /// first). Resolution keeps the first of equally specific bindings, so
    /// this is what makes the higher-priority extension win a collision; the
    /// help row's registration order follows it too.
    pub fn set_extension_order(&mut self, ranked: &[String]) {
        let rank = |b: &Binding| {
            b.owner.as_ref().map_or(0, |o| ranked.iter().position(|r| r == o).unwrap_or(ranked.len()))
        };
        self.bindings.sort_by_key(|b| (b.layer, rank(b)));
    }

    pub fn set_help_order(&mut self, labels: Vec<String>) {
        self.help_order = labels;
    }

    /// Replace the User layer (a settings change rebuilt it), re-running
    /// collision detection over the whole registry. Returns the warnings that
    /// are new since the previous build.
    pub fn replace_user_layer(&mut self, user: impl IntoIterator<Item = Binding>) -> Vec<String> {
        let before = std::mem::take(&mut self.warnings);
        let kept: Vec<Binding> =
            std::mem::take(&mut self.bindings).into_iter().filter(|b| b.layer != Layer::User).collect();
        self.extend(kept);
        self.extend(user);
        self.warnings.iter().filter(|w| !before.contains(w)).cloned().collect()
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
        self.resolve_binding(conds, state, panel, key).map(|b| b.intent.clone())
    }

    /// The binding [`resolve`](Self::resolve) would fire, rather than just its
    /// intent. The help row needs the whole binding (for its label) and must
    /// agree with what actually happens, so both go through this one walk —
    /// duplicating the precedence rules is how the two would drift apart.
    pub fn resolve_binding(
        &self,
        conds: &Conditions,
        state: &AppState,
        panel: &str,
        key: Key,
    ) -> Option<&Binding> {
        // Bindings fire on the press only. Once the Kitty protocol is enabled
        // every keystroke also arrives as a release (and, held, as repeats);
        // matching those too would fire every binding two or more times.
        if key.kind != KeyKind::Press {
            return None;
        }
        for frame in state.modes.iter().rev() {
            let mode_id = frame.mode.id();
            if let Some(b) = self.resolve_in_mode(conds, state, &mode_id, panel, key) {
                return Some(b);
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
    ) -> Option<&Binding> {
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
        best.map(|(b, ..)| b)
    }
}

impl KeymapRegistry {
    /// Every labelled binding in force right now, as `(chip, label)` pairs ready
    /// for the help row — keys sharing a label already merged, in the `help`
    /// display order, then registration order (Core, then Extension by
    /// priority, then User).
    ///
    /// Each candidate key is put back through [`resolve_binding`](Self::resolve_binding),
    /// so what is advertised is what would actually fire: a key bound twice is
    /// listed once, under whichever binding wins, and a binding whose `:when` is
    /// false is not listed at all.
    pub fn help_hints(
        &self,
        conds: &Conditions,
        state: &AppState,
        panel: &str,
    ) -> Vec<(String, String)> {
        let mut seen_keys: Vec<Key> = Vec::new();
        // (label, keys) in first-appearance order — a Vec, not a map, because
        // the order *is* the layout.
        let mut groups: Vec<(String, Vec<Key>)> = Vec::new();
        for b in &self.bindings {
            if b.help.is_none() || seen_keys.contains(&b.key) {
                continue;
            }
            seen_keys.push(b.key);
            let Some(winner) = self.resolve_binding(conds, state, panel, b.key) else { continue };
            let Some(label) = winner.help.clone() else { continue };
            match groups.iter_mut().find(|(l, _)| *l == label) {
                Some((_, keys)) => keys.push(b.key),
                None => groups.push((label, vec![b.key])),
            }
        }
        // Labels the `help` order lists come first, in its order; the rest keep
        // registration order (a stable sort).
        groups.sort_by_key(|(label, _)| self.help_order.iter().position(|l| l == label).unwrap_or(usize::MAX));
        groups.into_iter().map(|(label, keys)| (chip(&keys), label)).collect()
    }

    /// The keys, if any, that currently reach `intent` in `panel` — how the
    /// captured-keyboard row turns an escape intent back into something to show.
    pub fn keys_for_intent(
        &self,
        conds: &Conditions,
        state: &AppState,
        panel: &str,
        intent: &Intent,
    ) -> Vec<Key> {
        let mut out: Vec<Key> = Vec::new();
        for b in &self.bindings {
            if &b.intent != intent || out.contains(&b.key) {
                continue;
            }
            if self.resolve_binding(conds, state, panel, b.key).map(|w| &w.intent) == Some(intent) {
                out.push(b.key);
            }
        }
        out
    }
}

/// Render merged keys as one chip. All single characters concatenate (`wasd`,
/// `jk`); anything else joins with `/` (`Enter/Esc`). `wasd` is the
/// application's own name for its navigation and reads as one gesture, so the
/// concatenated form is the house style rather than a special case for it.
fn chip(keys: &[Key]) -> String {
    let names: Vec<String> = keys.iter().map(|k| crate::script::codec::key_name(*k)).collect();
    if names.iter().all(|n| n.chars().count() == 1) {
        names.concat()
    } else {
        names.join("/")
    }
}

/// A mode scope matches when it equals the mode id or is a segment-boundary
/// prefix of it.
fn mode_scope_matches(scope: &str, mode_id: &str) -> bool {
    mode_id == scope || mode_id.starts_with(&format!("{scope}:"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{KeyCode, Mode};

    fn ch(c: char) -> Key {
        Key::plain(KeyCode::Char(c))
    }
    use crate::core::{AppState, ExtensionValue};

    fn ext_binding(mode: &str, key: Key, intent: Intent) -> Binding {
        Binding { mode: mode.into(), panel: None, key, intent, when: Cond::Always, layer: Layer::Extension, help: None, owner: None }
    }

    /// A Core-layer binding in the `file`/`file` scope, optionally labelled.
    fn bind(key: Key, intent: Intent, when: Cond, help: Option<&str>) -> Binding {
        Binding {
            mode: "file".into(),
            panel: Some("file".into()),
            key,
            intent,
            when,
            layer: Layer::Core,
            help: help.map(str::to_string),
            owner: None,
        }
    }

    fn file_state() -> AppState {
        AppState::new(std::path::PathBuf::from("/"))
    }

    #[test]
    fn extension_priority_decides_a_collision_inside_the_extension_layer() {
        let mut km = KeymapRegistry::new();
        km.extend_owned("a", [ext_binding("file", ch('x'), Intent::Refresh)]);
        km.extend_owned("b", [ext_binding("file", ch('x'), Intent::Quit)]);
        assert!(km.warnings().iter().any(|w| w.contains("(a)") && w.contains("(b)")), "{:?}", km.warnings());
        let (conds, state) = (Conditions::default(), file_state());
        assert_eq!(km.resolve(&conds, &state, "file", ch('x')), Some(Intent::Refresh), "registration order");
        km.set_extension_order(&["b".into(), "a".into()]);
        assert_eq!(km.resolve(&conds, &state, "file", ch('x')), Some(Intent::Quit), "b now outranks a");
    }

    #[test]
    fn the_help_order_leads_and_the_rest_keep_registration_order() {
        let mut km = KeymapRegistry::new();
        km.add(bind(ch('q'), Intent::Quit, Cond::Always, Some("quit")));
        km.add(bind(ch('r'), Intent::Refresh, Cond::Always, Some("refresh")));
        km.add(bind(ch('z'), Intent::SelectAll, Cond::Always, Some("all")));
        km.set_help_order(vec!["all".into(), "missing".into()]);
        let labels: Vec<String> =
            km.help_hints(&Conditions::default(), &file_state(), "file").into_iter().map(|(_, l)| l).collect();
        assert_eq!(labels, vec!["all", "quit", "refresh"]);
    }

    #[test]
    fn an_unlabelled_keymap_produces_no_hints() {
        let mut km = KeymapRegistry::new();
        km.add(bind(ch('w'), Intent::CursorUp, Cond::Always, None));
        assert!(km.help_hints(&Conditions::default(), &file_state(), "file").is_empty());
    }

    #[test]
    fn keys_sharing_a_label_merge_in_registration_order() {
        let mut km = KeymapRegistry::new();
        for (k, i) in [
            ('w', Intent::CursorUp),
            ('a', Intent::CursorLeft),
            ('s', Intent::CursorDown),
            ('d', Intent::CursorRight),
        ] {
            km.add(bind(ch(k), i, Cond::Always, Some("cursor")));
        }
        let hints = km.help_hints(&Conditions::default(), &file_state(), "file");
        assert_eq!(hints, vec![("wasd".to_string(), "cursor".to_string())]);
    }

    #[test]
    fn a_multi_character_key_forces_the_slash_form() {
        let mut km = KeymapRegistry::new();
        km.add(bind(Key::plain(KeyCode::Enter), Intent::Activate, Cond::Always, Some("go")));
        km.add(bind(ch('o'), Intent::Activate, Cond::Always, Some("go")));
        let hints = km.help_hints(&Conditions::default(), &file_state(), "file");
        assert_eq!(hints, vec![("Enter/o".to_string(), "go".to_string())]);
    }

    #[test]
    fn a_binding_whose_when_is_false_is_not_advertised() {
        let mut conds = Conditions::default();
        conds.register("never", Box::new(|_| false));
        conds.register("always", Box::new(|_| true));
        let mut km = KeymapRegistry::new();
        km.add(bind(ch('f'), Intent::FilterListing, Cond::pred("never"), Some("filter")));
        km.add(bind(ch('g'), Intent::Refresh, Cond::pred("always"), Some("refresh")));
        let hints = km.help_hints(&conds, &file_state(), "file");
        assert_eq!(hints, vec![("g".to_string(), "refresh".to_string())]);
    }

    #[test]
    fn a_key_bound_twice_is_advertised_once_under_the_winning_binding() {
        // The Extension layer outranks Core, so its label is the one shown —
        // and it is shown once, not twice.
        let mut km = KeymapRegistry::new();
        km.add(bind(ch('e'), Intent::StartEdit, Cond::Always, Some("edit")));
        let mut over = bind(ch('e'), Intent::Activate, Cond::Always, Some("open"));
        over.layer = Layer::Extension;
        km.add(over);
        let hints = km.help_hints(&Conditions::default(), &file_state(), "file");
        assert_eq!(hints, vec![("e".to_string(), "open".to_string())]);
    }

    #[test]
    fn keys_for_intent_finds_only_the_bindings_that_would_really_fire() {
        let mut conds = Conditions::default();
        conds.register("never", Box::new(|_| false));
        let mut km = KeymapRegistry::new();
        km.add(bind(ch('w'), Intent::CursorUp, Cond::Always, None));
        km.add(bind(ch('k'), Intent::CursorUp, Cond::pred("never"), None));
        let got = km.keys_for_intent(&conds, &file_state(), "file", &Intent::CursorUp);
        assert_eq!(got, vec![ch('w')], "the gated-off binding is not a way to reach it");
    }

    fn overlay(id_ext: &str, id_mode: &str) -> Mode {
        Mode::Extension { extension: id_ext.into(), mode: id_mode.into(), state: ExtensionValue::Nil, overlay: true }
    }

    #[test]
    fn esc_clears_a_filter_before_falling_back_to_clear_selection() {
        let mut km = KeymapRegistry::new();
        km.extend(crate::app::defaults::keymap());
        let conds = Conditions::default();
        let mut state = AppState::new(std::env::temp_dir());

        assert_eq!(
            km.resolve(&conds, &state, "file", Key::plain(KeyCode::Esc)),
            Some(Intent::ClearSelection),
            "no filter active: Esc falls back to the unconditioned binding"
        );

        state.filter_stack.push(crate::core::state::FilterFrame::default());
        assert_eq!(
            km.resolve(&conds, &state, "file", Key::plain(KeyCode::Esc)),
            Some(Intent::ClearFilter),
            "a filter is active: the has-filter binding wins by specificity"
        );
    }

    #[test]
    fn file_search_is_unavailable_once_the_filter_stack_is_maxed() {
        let mut km = KeymapRegistry::new();
        km.extend(crate::app::defaults::keymap());
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
        km.extend(crate::app::defaults::keymap());
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
        km.extend(crate::app::defaults::keymap());
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
                gid: 0, owner: "".into(), group: "".into(),
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
        km.extend(crate::app::defaults::keymap());
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
            help: None,
            mode: "file".into(),
            panel: Some("function".into()),
            key: ch('l'),
            intent: Intent::FuncRight,
            when: Cond::Always,
            layer: Layer::Extension,
            owner: None,
        });
        km.add(Binding {
            help: None,
            mode: "file".into(),
            panel: Some("function".into()),
            key: ch('l'),
            intent: Intent::Noop,
            when: Cond::pred("form-active"),
            layer: Layer::Extension,
            owner: None,
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
        km.extend(crate::app::defaults::keymap());

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
