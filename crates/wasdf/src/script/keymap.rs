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
    /// editorial choice. What is not editorial is the key, and that always
    /// comes from the binding that would really fire (see doc/HELP.md).
    pub help: Option<String>,
    /// The extension that declared it (Extension layer only): its rank orders
    /// the layer, so a collision inside it goes to the higher-priority one.
    pub owner: Option<String>,
}

/// The help row's lead when the store sets no `help` order (doc/HELP.md).
const DEFAULT_HELP_ORDER: &[&str] = &["cursor", "select", "edit", "find", "move", "delete", "shell", "quit", "help"];

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
    /// this is what makes the higher-priority extension win a collision.
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
        // A release never fires a binding (with the Kitty protocol every
        // keystroke also arrives as one). A held key's repeat fires it again,
        // as the repeated presses of a terminal without the protocol do — left
        // unmatched it would fall through to text input instead.
        if key.kind == KeyKind::Release {
            return None;
        }
        let key = Key { kind: KeyKind::Press, ..key };
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
    /// display order (or [`DEFAULT_HELP_ORDER`]), then alphabetically.
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
        // Labels the `help` order lists come first, in its order; the rest
        // follow alphabetically.
        let listed = |label: &str| match self.help_order.is_empty() {
            true => DEFAULT_HELP_ORDER.iter().position(|l| *l == label),
            false => self.help_order.iter().position(|l| l == label),
        };
        groups.sort_by_key(|(label, _)| match listed(label) {
            Some(i) => (i, String::new()),
            None => (usize::MAX, label.to_lowercase()),
        });
        groups.into_iter().map(|(label, keys)| (chip(&keys), label)).collect()
    }

    /// Every binding that would fire in `state` and `panel`, one per key, in
    /// registration order — what the key list (`?`) shows.
    pub fn bindings_in_force(&self, conds: &Conditions, state: &AppState, panel: &str) -> Vec<&Binding> {
        let mut seen: Vec<Key> = Vec::new();
        let mut out = Vec::new();
        for b in &self.bindings {
            if seen.contains(&b.key) {
                continue;
            }
            seen.push(b.key);
            if let Some(winner) = self.resolve_binding(conds, state, panel, b.key) {
                out.push(winner);
            }
        }
        out
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

    fn overlay(id_ext: &str, id_mode: &str) -> Mode {
        Mode::Extension { extension: id_ext.into(), mode: id_mode.into(), state: ExtensionValue::Nil, overlay: true }
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
