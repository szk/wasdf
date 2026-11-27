//! The kernel: holds all services and AppState, and runs dispatch_intent — the
//! single pipeline stage that performs extension dispatch, the confirmation
//! gate, and plan issuance + reduction. The event loop calls into it; it
//! contains no intent-specific branches beyond the two structural gates the
//! spec names (extension resolver key, destructive → Policy).

use std::sync::{Arc, RwLock};
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

use crate::core::{
    self, AppEvent, AppState, AsyncResult, AsyncStatus, ExtensionIntent, ExtensionValue, InputCapture,
    Intent, Key, KeyCode, KeyKind, Mode, OptionDef, OptionStore, Payload, Plan, Purpose, ResolverRequest,
    KEY_RESOLVER,
};
use super::defaults;
use crate::extension::{self, Extension, ExtensionRegistry};
use crate::runtime::TaskManager;
use crate::script::codec;
use crate::script::condition::Conditions;
use crate::script::keymap::KeymapRegistry;
use crate::services::{CommandRegistry, ResolverChain, SkimMatcher};
use crate::ui::UiManager;

const REDISPATCH_CAP: u32 = 32;

pub struct App {
    pub state: AppState,
    pub keymap: KeymapRegistry,
    pub conditions: Conditions,
    pub commands: CommandRegistry,
    pub resolver: Arc<ResolverChain>,
    pub extensions: ExtensionRegistry,
    pub tasks: TaskManager,
    pub ui: UiManager,
    pub scheme: Arc<crate::script::SchemeSession>,
    /// The unified option store: all options (core + extensions) keyed by name —
    /// the projection of the settings store's options.
    pub store: Arc<OptionStore>,
    /// The settings store (in the Scheme session): the source of truth the
    /// projections follow (doc/SCHEME.md).
    pub settings: Arc<crate::script::store::Store>,
    /// The user's bindings and commands as the store holds them; the keymap's
    /// User layer and the user palette commands are built from it.
    pub user_layer: crate::script::projection::UserLayer,
    /// The core and extension palette commands; the user's are added on top
    /// whenever the store's commands change.
    base_commands: CommandRegistry,
    /// Writes custom.scm when settings change.
    persister: crate::services::persister::Persister,
    /// The extensions (not CmdOp) in registration order — the base the
    /// `extensions` display order ranks against.
    registered: Vec<String>,
    /// The extension list the settings screens show, kept in priority order.
    ext_infos: extension::settings::ExtInfos,
    /// The layouts render lays the frame out by: the core ones, replaced by
    /// the user's.
    pub layouts: crate::ui::layout::Layouts,
    /// System metrics, sampled off the main thread for the top panels.
    pub monitor: crate::services::monitor::Monitor,
    pub vfs: Arc<crate::vfs::Vfs>,
    pub rx: Receiver<AppEvent>,
    pub tx: Sender<AppEvent>,
    /// The shared theme registry. `event_loop` reads `resolve_active()` once per
    /// frame; `ThemeExtension` holds a clone for authoring.
    pub theme_registry: Arc<RwLock<crate::extension::theme::ThemeRegistry>>,
    /// The active theme for the frame, read through the theme projection so it
    /// is never behind the store (see `ThemeExtension::source`).
    pub theme_source: crate::extension::theme::ThemeSource,
    pending_suspend: Option<Vec<String>>,
    /// The last cursor identity broadcast to extensions (target path + panel
    /// visibility), so the cursor-changed event fires only on an actual change.
    last_cursor: (Option<std::path::PathBuf>, bool, usize, String),
    /// The message-log cursor: everything up to this seq has already been
    /// surfaced as a notice by `surface_new_messages` (the only reader).
    /// Captured at boot *start* so boot diagnostics themselves surface.
    msg_seen: u64,
    /// Memo of per-command options parsed from `--help`/`man`.
    option_cache: OptionCache,
}

impl App {
    /// Boot the kernel: build registries, load embedded defaults, register
    /// bundled extensions, and issue the initial directory read.
    pub fn boot(cwd: std::path::PathBuf) -> Self {
        // Before any producer runs, so every diagnostic this boot logs is
        // "new" to surface_new_messages — but nothing from an earlier App in
        // the same process (tests) is.
        let msg_seen = crate::services::messages::global().seq();
        let (tx, rx) = std::sync::mpsc::channel::<AppEvent>();

        // Spawn the resident Scheme session first (pillar #3). Wait briefly for
        // it to be ready: the steel engine loads its stdlib on the dedicated
        // thread and signals readiness. If it is not ready in time, the user's
        // settings take the fallback path so boot never freezes.
        let scheme = crate::script::SchemeSession::spawn();
        // custom.scm is data as well as a store program: two of its values are
        // needed before anything else — how long to wait for the session, and
        // which extensions not to load — so it is read here with the reader.
        let custom_text = read_config_file("custom.scm");
        let early = EarlySettings::from_custom(custom_text.as_deref());
        let scheme_ready = scheme.wait_ready(Duration::from_millis(early.boot_timeout_ms));

        // The embedded defaults are data, read without the session.
        let mut keymap = KeymapRegistry::new();
        keymap.extend(defaults::keymap());

        let mut conditions = Conditions::default();

        let mut commands = CommandRegistry::new();
        commands.extend(defaults::commands());

        let mut resolver = ResolverChain::new();
        resolver.extend(defaults::resolvers());

        // Build the VFS: the embedded filesystem default, then extensions (each
        // contributes its own mount via `vfs_providers()`). The settings provider
        // is contributed by the settings extension below.
        let mut vfs = crate::vfs::Vfs::default();
        vfs.extend(crate::vfs::defaults());

        // ── OptionStore ────────────────────────────────────────────────────────
        // Collect option defs from core + all extensions before registering, so
        // the store is built once with all defs (first-wins on duplicate keys).
        let bundled_exts = early.without_disabled(extension::bundled(config_dir()));
        let optional_exts = early.without_disabled(load_optional_extensions());

        // The theme registry is shared with the frame, so the theme extension is
        // built here rather than in `bundled()`; its options are collected with
        // everyone else's below.
        let theme_registry: Arc<RwLock<extension::theme::ThemeRegistry>> =
            Arc::new(RwLock::new(extension::theme::ThemeRegistry::bundled()));
        let theme_ext = extension::theme::ThemeExtension::new(theme_registry.clone());
        let theme_source = theme_ext.source();

        let mut all_option_defs = defaults::core_options();
        all_option_defs.extend(defaults::extension_options(&theme_ext));
        for ext in bundled_exts.iter().chain(&optional_exts) {
            all_option_defs.extend(defaults::extension_options(ext.as_ref()));
        }
        let store: Arc<OptionStore> = Arc::new(OptionStore::new(all_option_defs));
        conditions.bind_options(store.clone());



        // Collect SettingsNode contributions from theme_ext and every bundled
        // extension (e.g. lscolors' Palette, grafted onto its own screen) before
        // building settings_ext (plan §11 choice (b): no interior mutability
        // needed). `&bundled_exts` since the vec is still needed below to
        // register the extensions themselves.
        let settings_nodes: Vec<std::sync::Arc<dyn extension::settings::SettingsNode>> = {
            let mut nodes = theme_ext.settings_nodes();
            for ext in &bundled_exts {
                nodes.extend(ext.settings_nodes());
            }
            nodes
        };

        // The full extension list (id + About) is known only after every extension
        // is registered, so the settings provider reads it through this shared
        // OnceLock, which the kernel fills at the end of construction.
        let ext_infos: extension::settings::ExtInfos = Arc::default();

        let settings_ext = extension::settings::SettingsExtension::new(
            store.clone(),
            settings_nodes,
            ext_infos.clone(),
        );

        // CmdOp is built in: registered first and never disabled. Its keys and
        // commands are in the core sources; it learns the registered extension
        // ids at the end of construction (for `cmdop.runner`).
        let extension_ids: Arc<std::sync::OnceLock<Vec<String>>> = Arc::default();
        let mut extensions = ExtensionRegistry::new();
        extensions.register(Box::new(super::cmdop::CmdOp::new(extension_ids.clone())));

        // Register bundled extensions: keymaps, commands, resolver entries,
        // conditions. The only place bundled extensions are named is the facade.
        for ext in bundled_exts {
            keymap.extend_owned(ext.id(), ext.keymaps());
            keymap.extend_owned(ext.id(), defaults::extension_keymap(ext.as_ref()));
            commands.extend(ext.commands());
            resolver.extend(ext.resolver_entries());
            vfs.extend(ext.vfs_providers());
            ext.register_conditions(&mut conditions);
            extensions.register(ext);
        }
        // Settings and theme registered separately (they need shared Arcs built
        // above). Settings is registered before optional extensions so its ID
        // wins any collision from an optional extension with the same name.
        // Theme still needs the same keymap/command/resolver/condition wiring
        // the bundled_exts loop above gives every other bundled extension.
        vfs.extend(settings_ext.vfs_providers());
        // @/messages: the diagnostic log every messages::log call above and
        // below has been writing into (a kernel service, not an extension —
        // its producers include the loader and this very boot sequence).
        vfs.extend([Box::new(crate::services::messages::MessagesProvider::new(
            crate::services::messages::global().clone(),
            store.clone(),
        )) as Box<dyn crate::vfs::VfsProvider>]);
        extensions.register(Box::new(settings_ext));
        keymap.extend_owned(theme_ext.id(), theme_ext.keymaps());
        keymap.extend_owned(theme_ext.id(), defaults::extension_keymap(&theme_ext));
        commands.extend(theme_ext.commands());
        resolver.extend(theme_ext.resolver_entries());
        theme_ext.register_conditions(&mut conditions);
        extensions.register(Box::new(theme_ext));

        // Optional (dynamically loaded) extensions: in file-name order; an id
        // collision disables the later one.
        let mut ext_ids: std::collections::HashSet<String> =
            extensions.iter().map(|e| e.id().to_string()).collect();
        for ext in optional_exts {
            if !ext_ids.insert(ext.id().to_string()) {
                crate::services::messages::log(format!("optional extension id collides ({}); disabled", ext.id()), true);
                continue;
            }
            keymap.extend_owned(ext.id(), ext.keymaps());
            keymap.extend_owned(ext.id(), defaults::extension_keymap(ext.as_ref()));
            commands.extend(ext.commands());
            resolver.extend(ext.resolver_entries());
            vfs.extend(ext.vfs_providers());
            ext.register_conditions(&mut conditions);
            extensions.register(ext);
        }

        // Give every extension the store so they can read their own options,
        // and a way to wake the loop for those that own an I/O source of their
        // own (a PTY reader thread has nobody else to announce it).
        for ext in extensions.iter() {
            ext.bind_store(&store);
            ext.bind_events(tx.clone());
        }

        // Publish the full extension list (id + About) to the settings provider so
        // @/settings/Extensions can list every extension and its About card.
        // CmdOp rides the extension registry for its form seam, but it is part
        // of the kernel: not listed as an extension, not a runner candidate.
        let real_extensions = || extensions.iter().filter(|e| e.id() != "cmdop");
        let registered: Vec<String> = real_extensions().map(|e| e.id().to_string()).collect();
        let _ = extension_ids.set(registered.clone());

        // The user's settings last: build the settings store (config.scm, then
        // custom.scm) and project it — options into the OptionStore, bindings
        // and commands into the User layer.
        let settings = Arc::new(crate::script::store::Store::new(scheme.clone()));
        let mut user_layer = crate::script::projection::UserLayer::default();
        let config_text = read_or_create_config(&store);
        let preludes: Vec<(String, String)> =
            extensions.iter().filter_map(|e| e.prelude().map(|p| (e.id().to_string(), p))).collect();
        let custom_loaded = load_user_settings(
            &settings,
            scheme_ready,
            &store,
            &preludes,
            config_text.as_deref(),
            custom_text.as_deref(),
            &mut user_layer,
        );
        let (user_binds, problems) = user_layer.bindings();
        keymap.extend(user_binds);
        let base_commands = commands.clone();
        let (user_commands, more) = user_layer.commands();
        commands.extend(user_commands);
        for p in problems.into_iter().chain(more) {
            crate::services::messages::log(p, true);
        }
        // A custom.scm that did not load cleanly is never overwritten this
        // session: the user's file is what they have to fix.
        let custom_path = if custom_loaded { config_dir().map(|d| d.join("custom.scm")) } else { None };
        if !custom_loaded {
            crate::services::messages::log("custom.scm had errors; settings changed now are not saved this session", true);
        }
        let persister = crate::services::persister::Persister::spawn(
            settings.clone(),
            if cfg!(test) { None } else { custom_path },
            custom_text.clone(),
        );
        // An old-format config.scm has just been loaded through the bridge;
        // migrate it once, so the next start reads a program and a custom.scm.
        if !cfg!(test)
            && let Some(text) = config_text.as_deref()
            && crate::script::user_config::is_legacy(text)
            && let Some(dir) = config_dir()
        {
            match crate::script::user_config::migrate(&dir, text, &store, defaults::KEYMAP_SCHEME) {
                Ok(kept) => {
                    crate::services::messages::log(
                        format!("config.scm migrated to the new format; the old file is kept as {kept}"),
                        false,
                    );
                    persister.wake(); // the old managed block, now in the custom layer
                }
                Err(e) => crate::services::messages::log(format!("config.scm migration failed ({e}); it stays in the old format"), true),
            }
        }
        // Apply the message-log cap now that the user's value (if any) is in
        // the store; MessagesProvider re-reads it on every @/messages listing,
        // so later edits under @/settings/System apply live too.
        crate::services::messages::global()
            .set_max_lines(store.get_u32("max-message-line") as usize);

        for w in keymap.warnings() {
            crate::services::messages::log(format!("warning: {w}"), true);
        }

        let resolver = Arc::new(resolver);
        // Hand the resolver chain to extensions that build external command lines
        // (the CmdOp builder) now that it is finalized and shareable.
        for ext in extensions.iter() {
            ext.bind_services(&resolver);
        }
        let vfs = Arc::new(vfs);
        let matcher: Arc<dyn crate::services::MatcherBackend> = Arc::new(SkimMatcher::default());
        let tasks = TaskManager::new(
            tx.clone(),
            resolver.clone(),
            matcher,
            Some(settings.clone()),
            vfs.clone(),
            store.clone(),
        );

        let state = AppState::new(cwd.clone());
        let ui = UiManager::new();

        let mut app = App {
            state,
            keymap,
            conditions,
            commands,
            resolver,
            extensions,
            tasks,
            ui,
            scheme,
            store,
            settings,
            user_layer,
            base_commands,
            persister,
            registered,
            ext_infos,
            layouts: crate::ui::layout::Layouts::new(defaults::layouts()),
            monitor: crate::services::monitor::Monitor::spawn((!cfg!(test)).then(|| tx.clone())),
            vfs,
            rx,
            tx,
            theme_registry,
            theme_source,
            pending_suspend: None,
            last_cursor: (None, false, 0, String::new()),
            msg_seen,
            option_cache: std::cell::RefCell::new(std::collections::HashMap::new()),
        };
        app.apply_orders();
        app.apply_layouts();
        // Initial listing.
        app.spawn_plan(Plan::ReadDir { path: cwd, show_hidden: false });
        app
    }

    /// Handle one key: resolve to an intent, or deliver a RawKeyEvent in
    /// Select / InputForm (the popup text-entry dialog).
    pub fn handle_key(&mut self, key: Key) {
        if let Some(cap) = self.state.input_capture.clone() {
            self.handle_captured_key(&cap, key);
            self.broadcast_cursor_changed();
            return;
        }
        // Only a captured child (pty) wants releases. Everywhere else a release
        // reaching a text input as a RawKeyEvent would type its character twice.
        if key.kind == KeyKind::Release {
            return;
        }
        // While the function-panel search input is open, keys edit the query — except
        // Enter (submit) and Esc (cancel), which still resolve via the keymap.
        if self.state.function.search.input_active
            && !matches!(key.code, KeyCode::Enter | KeyCode::Esc)
        {
            self.dispatch(Intent::RawKeyEvent(key));
            self.broadcast_cursor_changed();
            return;
        }
        // While a generic function-panel input is open (any form extension), or a
        // CmdOp text input (filter / prompt / builder edit) is active, all keys are
        // fed as raw events so text editing works. The reducer routes Enter/Esc.
        if self.state.function.input.is_some() {
            self.dispatch(Intent::RawKeyEvent(key));
            self.broadcast_cursor_changed();
            return;
        }
        let panel = self.state.focused_panel.clone();
        match self.keymap.resolve(&self.conditions, &self.state, &panel, key) {
            Some(intent) => self.dispatch(intent),
            None => {
                if matches!(self.state.mode(), Mode::Select(_) | Mode::InputForm(_)) {
                    self.dispatch(Intent::RawKeyEvent(key));
                }
            }
        }
        self.broadcast_cursor_changed();
    }

    /// One key while an [`InputCapture`] is installed. The keymap is bypassed
    /// for everything except the two escapes core itself owns, so a terminal
    /// (or anything else driving a child keystroke-by-keystroke) sees `q`,
    /// `Ctrl-C` and the arrows unchanged.
    fn handle_captured_key(&mut self, cap: &InputCapture, key: Key) {
        // The hard release, first and unconditional: this is the only thing
        // standing between a wedged extension and a user who cannot quit, so
        // it must not depend on the extension answering the "prefix" that
        // follows — or on it being reachable at all.
        if key == cap.release {
            self.dispatch(Intent::SetInputCapture(None));
            self.dispatch(Intent::Extension(ExtensionIntent {
                extension: cap.owner.clone(),
                intent: "prefix".into(),
                data: ExtensionValue::Nil,
            }));
            return;
        }
        // Intent escapes: resolve exactly as an uncaptured key would, and if
        // the user pressed something that *means* "move the cursor", release
        // and let it mean that. Releasing and dispatching here — rather than
        // handing the extension the job of replaying the key — is what keeps
        // the two in the right order without any dispatch-ordering subtlety.
        if !cap.escape.is_empty() {
            let panel = self.state.focused_panel.clone();
            if let Some(intent) = self.keymap.resolve(&self.conditions, &self.state, &panel, key)
                && cap.escape.contains(&intent)
            {
                self.dispatch(Intent::SetInputCapture(None));
                self.dispatch(intent);
                return;
            }
        }
        self.dispatch(Intent::Extension(ExtensionIntent {
            extension: cap.owner.clone(),
            intent: "key".into(),
            data: codec::encode_key(key),
        }));
    }

    /// Handle an async completion: drop stale (mode-bound) results, then reduce.
    pub fn handle_async(&mut self, result: AsyncResult) {
        // A settings-store evaluation (a Configure, a REPL line) — successful or
        // not — may have changed entries: follow them in the projections first.
        if let Payload::Scheme { changes, owner, value } = &result.payload {
            let changes = changes.clone();
            self.apply_settings_changes(&changes);
            if let Some(ext) = owner.as_deref().and_then(|o| self.extensions.find(o)) {
                ext.accept_content(std::path::Path::new(""), &core::Content::Value(value.clone()));
            }
        }
        // Content reads are handed to the owning extension (which decodes +
        // stashes); they do not flow through the pure reducer.
        if result.purpose == Purpose::Content {
            if result.status == AsyncStatus::Ok
                && let Payload::Read { owner, path, result: read } = result.payload {
                    // A form owner's explicit read (e.g. the CmdOp listing dir) is
                    // not cursor-follow: it bypasses the cursor-staleness gate and
                    // self-gates in accept_content, and does not reset the view.
                    let is_form = self.state.function.form_owner.as_deref() == Some(owner.as_str());
                    // Drop a cursor-follow read whose path is no longer under the
                    // cursor: the user navigated away while the read was in flight.
                    // Without this gate the late read would re-populate the preview
                    // the cursor-changed clear just dropped.
                    if !is_form && self.cursor_signature().0.as_deref() != Some(path.as_path()) {
                        return;
                    }
                    if let Some(ext) = self.extensions.find(&owner) {
                        ext.accept_content(&path, &read);
                    }
                    if !is_form {
                        // New content: reset the view (scroll / search) like a new file.
                        self.dispatch(Intent::ResetContentView);
                    }
                }
            return;
        }
        if result.purpose == Purpose::Search {
            let bound = result.mode_generation == self.state.mode_generation()
                && matches!(self.state.mode(), Mode::Select(_));
            if !bound {
                return;
            }
        }
        let effects = core::apply_result(&mut self.state, result);
        self.process_effects(effects, 0);
        // A reload may move the cursor, and fresh search results change the
        // selection; re-broadcast the cursor-changed event either way.
        self.broadcast_cursor_changed();
    }

    /// Fold a store change set into the projections: options into the
    /// OptionStore, bindings and commands into the User layer (rebuilding the
    /// keymap layer and the palette when they changed), then let the
    /// Persister write custom.scm. A virtual listing (the settings tree shows
    /// option values) is refreshed so it shows the new values.
    fn apply_settings_changes(&mut self, printed: &str) {
        use crate::script::store::Section;
        let changes = match crate::script::store::parse_changes(printed) {
            Ok(c) => c,
            Err(e) => {
                crate::services::messages::log(format!("settings store: change set unreadable ({e})"), true);
                return;
            }
        };
        if changes.is_empty() {
            return;
        }
        for p in crate::script::projection::apply_options(&changes, &self.store) {
            crate::services::messages::log(p, true);
        }
        self.user_layer.absorb(&changes);
        if changes.iter().any(|c| c.section == Section::Binding) {
            let (binds, problems) = self.user_layer.bindings();
            for w in self.keymap.replace_user_layer(binds) {
                crate::services::messages::log(format!("warning: {w}"), true);
            }
            for p in problems {
                crate::services::messages::log(p, true);
            }
        }
        if changes.iter().any(|c| c.section == Section::Command) {
            let (user_commands, problems) = self.user_layer.commands();
            let mut commands = self.base_commands.clone();
            commands.extend(user_commands);
            self.commands = commands;
            for p in problems {
                crate::services::messages::log(p, true);
            }
        }
        if changes.iter().any(|c| c.section == Section::Order) {
            self.apply_orders();
        }
        if changes.iter().any(|c| c.section == Section::Layout) {
            self.apply_layouts();
        }
        self.persister.wake();
        if crate::vfs::is_virtual(&self.state.cwd) {
            self.dispatch(Intent::Refresh);
        }
    }

    fn apply_layouts(&mut self) {
        let (user, problems) = self.user_layer.layouts();
        self.layouts.set_user(user);
        for p in problems {
            crate::services::messages::log(p, true);
        }
    }

    /// Fold what the draw measured back into the state and services: render is
    /// the single source of geometry, and the pure reducer cannot see it.
    pub fn after_draw(&mut self) {
        // The reducer advances function scroll/hscroll blindly; bind folds in
        // the measured bound and clamps the offset, so over-scroll is discarded
        // and reversing direction moves immediately.
        self.state.function.scroll.bind(self.ui.function_max_scroll());
        self.state.function.hscroll.bind(self.ui.function_hmax_scroll());
        if let Some(row) = self.ui.take_revealed() {
            self.state.function.scroll.set(row);
            self.state.function.search.reveal = false;
        }
        // The file-list geometry, so 2-D cursor movement matches the screen.
        self.state.list_geom = self.ui.list_geom();
        self.monitor.set_active(self.ui.shows("monitor"));
    }

    /// Project the store's display orders: extension priority (the registry,
    /// the Extension keymap layer, the settings list) and the help-row order.
    fn apply_orders(&mut self) {
        let ranked = extension::registry::rank(&self.registered, &self.user_layer.order("extensions"));
        self.extensions.set_order(&ranked);
        self.keymap.set_extension_order(&ranked);
        self.keymap.set_help_order(self.user_layer.order("help"));
        if let Ok(mut infos) = self.ext_infos.write() {
            *infos = ranked
                .iter()
                .filter_map(|id| self.extensions.find(id))
                .map(|e| extension::settings::ExtInfo { id: e.id().to_string(), about: e.about() })
                .collect();
        }
    }

    /// The cursor identity for the broadcast gate: what's under the cursor (the
    /// File-mode entry, or the file-search candidate) and whether the function
    /// panel is open. Generic — it carries no extension-specific notion.
    fn cursor_signature(&self) -> (Option<std::path::PathBuf>, bool, usize, String) {
        let path = self.state.cursor_path();
        // Scroll is part of the signature so paging down re-broadcasts and the
        // content provider can fetch the next chunk as the viewport advances. The
        // mode id is part of it too, so pushing/popping an overlay extension mode
        // re-broadcasts — letting the ambient provider reclaim the panel on pop.
        (
            path,
            self.state.function.visible,
            self.state.function.scroll.offset(),
            self.state.mode_id(),
        )
    }

    /// Broadcast the generic **cursor-changed** event: when the cursor identity or
    /// panel visibility changes, call every registered extension's
    /// `on_cursor_changed` and re-dispatch the intents they return. The kernel
    /// knows nothing about any content extension here — the content provider is just one
    /// subscriber that returns `LoadContent` ([EXTENSION.md]).
    fn broadcast_cursor_changed(&mut self) {
        let sig = self.cursor_signature();
        if self.last_cursor == sig {
            return;
        }
        self.last_cursor = sig;
        let had_form = self.state.function.form_owner.is_some();
        self.notify_cursor_subscribers();
        // A subscriber may have handed the function panel back — a hosted
        // terminal stepping aside when the user moves the file cursor. Everyone
        // else already answered while it still owned the panel, and a content
        // provider declines to follow the cursor into a panel it does not own,
        // so they get one more pass. Exactly one: the panel cannot be handed
        // back twice, so this cannot recurse.
        if had_form && self.state.function.form_owner.is_none() {
            self.notify_cursor_subscribers();
        }
    }

    fn notify_cursor_subscribers(&mut self) {
        let intents: Vec<Intent> = {
            let state = &self.state;
            // Highest priority last, so its intents are the ones that stick.
            let exts: Vec<_> = self.extensions.iter().collect();
            exts.into_iter().rev().flat_map(|e| e.on_cursor_changed(state)).collect()
        };
        for i in intents {
            self.dispatch_depth(i, false, 0);
        }
    }

    /// Give every extension a chance to turn out-of-band state into intents,
    /// once per event-loop pass. Unlike `broadcast_cursor_changed` this is not
    /// deduplicated — that is the whole point, since what it exists to catch
    /// (a child exiting on its own thread) leaves no trace in `AppState`.
    pub fn tick(&mut self) {
        let intents: Vec<Intent> = {
            let state = &self.state;
            self.extensions.iter().flat_map(|e| e.on_tick(state)).collect()
        };
        for i in intents {
            self.dispatch_depth(i, false, 0);
        }
    }

    /// Whether a Suspended execution is pending (the loop runs it).
    pub fn take_suspend(&mut self) -> Option<Vec<String>> {
        self.pending_suspend.take()
    }

    pub fn should_quit(&self) -> bool {
        self.state.quit
    }

    pub fn cwd(&self) -> std::path::PathBuf {
        self.state.cwd.clone()
    }

    /// After a suspend returns, drop back to the file list (the edit is over,
    /// regardless of outcome) and refresh. Refresh re-reads both the directory
    /// listing and the visible function-panel content, so an edit made in the
    /// suspended child (the editor) shows on return without moving the cursor.
    pub fn after_suspend(&mut self) {
        self.state.reset_to_file();
        self.dispatch(Intent::Refresh);
    }

    fn dispatch(&mut self, intent: Intent) {
        self.dispatch_depth(intent, false, 0);
    }

    fn dispatch_depth(&mut self, intent: Intent, confirmed: bool, depth: u32) {
        if depth > REDISPATCH_CAP {
            return;
        }

        // Policy mode is modal: only Confirm (allow) and Cancel (deny) act.
        let policy_pending = match self.state.mode() {
            Mode::Policy(p) => Some((**p).clone()),
            _ => None,
        };
        if let Some(pending) = policy_pending {
            match intent {
                Intent::Confirm => {
                    self.state.pop_mode();
                    self.dispatch_depth(pending, true, depth + 1);
                }
                Intent::Cancel => {
                    self.state.pop_mode();
                }
                _ => {}
            }
            return;
        }

        // Extension dispatch and the reserved resolver key.
        if let Intent::Extension(ext) = &intent {
            if ext.data.get(KEY_RESOLVER).is_some() {
                if let Some(req) = resolver_request(&ext.data) {
                    if self.needs_confirmation(&req) && !confirmed {
                        let i = intent.clone();
                        self.state.push_mode(Mode::Policy(Box::new(i)));
                        return;
                    }
                    self.spawn_plan(Plan::ResolveAndRun { request: req });
                } else {
                    crate::services::messages::log("malformed resolver intent", true);
                }
                return;
            }
            let ext = ext.clone();
            for i in self.extensions.dispatch(&ext, &self.state) {
                self.dispatch_depth(i, confirmed, depth + 1);
            }
            return;
        }

        // Confirmation gate: a destructive resolver op (e.g. delete), or one
        // that would silently overwrite an existing path (cp/mv's `-i`
        // condition) → Policy.
        if let Intent::RunResolver(req) = &intent
            && self.needs_confirmation(req) && !confirmed {
                self.state.push_mode(Mode::Policy(Box::new(intent)));
                return;
            }

        // KillProcess stops the streaming Execute child (the reducer then marks
        // the Exec frame finished).
        if matches!(intent, Intent::KillProcess) {
            self.tasks.kill_execute();
        }

        let effects = {
            let ctx = CmdCtx {
                commands: &self.commands,
                resolver: self.resolver.as_ref(),
                options: &self.option_cache,
                keymap: &self.keymap,
                conditions: &self.conditions,
            };
            core::apply_intent(&mut self.state, intent, &ctx)
        };
        self.process_effects(effects, depth);
    }

    /// Whether `req` needs a Policy confirmation before running: a
    /// statically destructive op (delete), or one that would silently
    /// overwrite an existing path — the dynamic `cp`/`mv -i` condition,
    /// which depends on the actual filesystem at dispatch time and so can't
    /// be a fixed property of the resolver entry the way `destructive` is.
    fn needs_confirmation(&self, req: &ResolverRequest) -> bool {
        self.resolver.is_destructive(&req.op)
            || crate::services::resolver::would_overwrite(req, &self.state.cwd)
    }

    fn process_effects(&mut self, effects: core::Effects, depth: u32) {
        for n in effects.notices {
            crate::services::messages::log(n.text, n.error);
        }
        for p in effects.plans {
            self.spawn_plan(p);
        }
        for i in effects.intents {
            self.dispatch_depth(i, false, depth + 1);
        }
    }

    /// The single seam between the message log and the on-screen notice: the
    /// newest line the user hasn't been shown yet — whether it came from the
    /// reducer's `Effects.notices` (drained into the log above) or straight
    /// from a boot/loader diagnostic — becomes the notice. Called every
    /// event-loop pass *before* the draw, so a notice logged while handling
    /// an event shows in that event's own frame, and a config mistake is
    /// visible at launch. Only the newest unseen line: the notice UI is a
    /// single 4-second slot; the full history is `@/messages`.
    pub fn surface_new_messages(&mut self) {
        if let Some((text, error, seq)) = crate::services::messages::global().latest_since(self.msg_seen) {
            self.msg_seen = seq;
            self.ui.notify(if error { core::Notice::error(text) } else { core::Notice::info(text) });
        }
    }

    fn spawn_plan(&mut self, plan: Plan) {
        if let Plan::Suspend { argv } = plan {
            self.pending_suspend = Some(argv);
            return;
        }
        let generation = self.state.mode_generation();
        let cwd = self.state.cwd.clone();
        self.tasks.spawn(plan, generation, cwd);
    }
}

/// A per-command memo of options parsed from `--help`/`man`, so the extraction
/// subprocess runs at most once per command.
type OptionCache = std::cell::RefCell<std::collections::HashMap<String, Vec<(String, String)>>>;

/// The reducer's command/resolver lookup: palette commands from the registry,
/// command-Select options from the resolver chain (the declared options, or those
/// parsed from the resolved command's `--help`/`man`). Combines the services
/// behind the single `CommandLookup` seam the pure reducer uses.
struct CmdCtx<'a> {
    commands: &'a CommandRegistry,
    resolver: &'a ResolverChain,
    options: &'a OptionCache,
    keymap: &'a KeymapRegistry,
    conditions: &'a Conditions,
}

impl core::CommandLookup for CmdCtx<'_> {
    fn intent_of(&self, name: &str) -> Option<Intent> {
        self.commands.intent_of(name)
    }
    fn command_candidates(&self) -> Vec<core::Candidate> {
        self.commands.candidates()
    }
    fn key_candidates(&self, state: &core::AppState) -> Vec<core::Candidate> {
        let mut rows: Vec<(String, String)> = self
            .keymap
            .bindings_in_force(self.conditions, state, &state.focused_panel)
            .into_iter()
            .map(|b| (codec::key_name(b.key), b))
            // Left out on request: Q and Ctrl-c are bound only in the file
            // panel's scope, so the list would promise them where they do nothing.
            .filter(|(name, _)| name != "Q" && name != "Ctrl-c")
            .map(|(name, b)| {
                let what = codec::intent_text(&b.intent);
                let text = match &b.help {
                    Some(label) => format!("{label} ({what})"),
                    None => what,
                };
                (name, text)
            })
            .collect();
        // Special keys (named, or a modifier chord) first, then single keys,
        // each alphabetically.
        rows.sort_by_key(|(name, _)| (name.chars().count() == 1, name.to_lowercase(), name.clone()));
        let key_w = rows.iter().map(|(name, _)| name.chars().count()).max().unwrap_or(0);
        rows.into_iter()
            .map(|(name, text)| core::Candidate::new(format!("{name:<key_w$}  {text}"), core::ExtensionValue::Nil))
            .collect()
    }
    fn resolver_options(&self, op: &str) -> Vec<(String, String)> {
        let declared: Vec<(String, String)> =
            self.resolver.options(op).into_iter().map(|o| (o.token, o.label)).collect();
        // Parse the real command's options once (cached); fall back to declared.
        let Some(cmd) = self.resolver.command_of(op) else { return declared };
        let cached = self.options.borrow().get(&cmd).cloned();
        let extracted = match cached {
            Some(c) => c,
            None => {
                let parsed = crate::exec::extract_options(&cmd);
                self.options.borrow_mut().insert(cmd, parsed.clone());
                parsed
            }
        };
        if extracted.is_empty() { declared } else { extracted }
    }
    fn resolver_command(&self, op: &str) -> Vec<core::CmdToken> {
        use crate::services::resolver::{ArgEl, Slot};
        self.resolver
            .command_template(op)
            .into_iter()
            .map(|el| match el {
                ArgEl::Lit(s) => core::CmdToken::Lit(s),
                ArgEl::Ph(Slot::Opts) => core::CmdToken::Opts,
                ArgEl::Ph(Slot::Src) => core::CmdToken::Src,
                ArgEl::Ph(Slot::Paths) => core::CmdToken::Paths,
                ArgEl::Ph(Slot::Dst) => core::CmdToken::Dst,
                ArgEl::Ph(Slot::Path) => core::CmdToken::Path,
            })
            .collect()
    }
}

/// Build a ResolverRequest from an extension intent's reserved resolver data.
fn resolver_request(data: &ExtensionValue) -> Option<ResolverRequest> {
    let r = data.get(KEY_RESOLVER)?;
    let op = r.get("op")?.as_str()?.to_string();
    let src = r.get("src").and_then(|v| v.as_path()).cloned();
    let dst = r.get("dst").and_then(|v| v.as_str()).map(str::to_string);
    let path = r.get("path").and_then(|v| match v {
        ExtensionValue::Path(p) => Some(p.to_string_lossy().into_owned()),
        ExtensionValue::String(s) => Some(s.clone()),
        _ => None,
    });
    let paths = r
        .get("paths")
        .and_then(|v| v.as_list())
        .map(|l| l.iter().filter_map(|x| x.as_path().cloned()).collect())
        .unwrap_or_default();
    let opts = r
        .get("opts")
        .and_then(|v| v.as_list())
        .map(|l| l.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let label = r.get("label").and_then(|v| v.as_str()).unwrap_or(&op).to_string();
    Some(ResolverRequest { op, src, dst, path, paths, opts, label })
}

/// Load optional extensions from the extensions directory. Skipped in tests
/// (the loader's glue parsing is covered directly).
fn load_optional_extensions() -> Vec<Box<dyn crate::extension::Extension>> {
    if cfg!(test) {
        return Vec::new();
    }
    let dir = std::env::var_os("WASDF_EXTENSIONS_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| config_dir().map(|d| d.join("extensions")));
    match dir {
        Some(d) => crate::extension::loader::load_optional(&d),
        None => Vec::new(),
    }
}

/// The user config directory ($WASDF_CONFIG_DIR, else XDG config, else
/// ~/.config), created on demand. None only if no home is discoverable.
fn config_dir() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("WASDF_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))?;
    let dir = if std::env::var_os("WASDF_CONFIG_DIR").is_some() {
        base
    } else {
        base.join("wasdf")
    };
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// The two settings boot needs before the session is ready, read from
/// custom.scm's literal store calls.
struct EarlySettings {
    boot_timeout_ms: u64,
    disabled: Vec<String>,
}

impl EarlySettings {
    fn from_custom(text: Option<&str>) -> Self {
        let mut early = EarlySettings { boot_timeout_ms: 3000, disabled: Vec::new() };
        let changes = text.map(crate::script::store::changes_from_literal_program).unwrap_or(Ok(Vec::new()));
        for c in changes.unwrap_or_default() {
            match (c.id.text(), &c.value) {
                (Some("scheme.boot-timeout-ms"), Some(crate::script::sexpr::Datum::Int(ms))) => {
                    early.boot_timeout_ms = (*ms).max(0) as u64;
                }
                (Some("extensions.disabled"), Some(crate::script::sexpr::Datum::List(ids))) => {
                    early.disabled = ids.iter().filter_map(|d| d.text().map(str::to_string)).collect();
                }
                _ => {}
            }
        }
        early
    }

    /// Drop the extensions the user disabled, noting each in @/messages.
    fn without_disabled(
        &self,
        exts: Vec<Box<dyn crate::extension::Extension>>,
    ) -> Vec<Box<dyn crate::extension::Extension>> {
        exts.into_iter()
            .filter(|e| {
                let off = self.disabled.iter().any(|d| d == e.id());
                if off {
                    crate::services::messages::log(format!("extension {} is disabled (extensions.disabled)", e.id()), false);
                }
                !off
            })
            .collect()
    }
}

/// Read a file from the user config directory. Tests never touch the real
/// user configuration.
fn read_config_file(name: &str) -> Option<String> {
    if cfg!(test) {
        return None;
    }
    std::fs::read_to_string(config_dir()?.join(name)).ok()
}

/// Read config.scm, writing the template first when it does not exist.
fn read_or_create_config(options: &OptionStore) -> Option<String> {
    if cfg!(test) {
        return None;
    }
    let path = config_dir()?.join("config.scm");
    if !path.exists() {
        let _ = std::fs::write(&path, crate::script::user_config::config_template(options, defaults::KEYMAP_SCHEME));
    }
    std::fs::read_to_string(path).ok()
}

/// Build the settings store from the user's files and fold the result into the
/// projections: declare every option, evaluate config.scm into the `config`
/// layer (an old-format file through the bridge: its own forms → `config`, its
/// managed block → `custom`), then custom.scm into `custom`. Without the
/// session, config.scm is skipped and custom.scm's literal calls apply
/// directly.
fn load_user_settings(
    settings: &crate::script::store::Store,
    ready: bool,
    options: &OptionStore,
    preludes: &[(String, String)],
    config_text: Option<&str>,
    custom_text: Option<&str>,
    user: &mut crate::script::projection::UserLayer,
) -> bool {
    use crate::script::store::{changes_from_literal_program, Layer};
    use crate::script::user_config;
    let log = |m: String| crate::services::messages::log(m, true);
    let mut changes = Vec::new();
    // Whether every form ran: `false` when this file had errors.
    let mut run = |layer: Layer, text: &str, file: &str| -> bool {
        let (c, errors) = settings.eval_program(layer, text);
        changes.extend(c);
        let clean = errors.is_empty();
        for e in errors {
            log(format!("{file}:{}: {}", e.line, e.message));
        }
        clean
    };
    let mut custom_ok = true;
    if ready {
        let defs: Vec<OptionDef> = options.keys().iter().filter_map(|k| options.def(k).cloned()).collect();
        if let Err(e) = settings.declare_options(&defs) {
            log(format!("settings store: declaring options failed ({e})"));
        }
        let mut taken: std::collections::HashSet<String> =
            crate::script::store::CORE_VOCABULARY.iter().map(|s| s.to_string()).collect();
        for (owner, src) in preludes {
            for p in settings.load_prelude(owner, src, &mut taken) {
                log(p);
            }
        }
        if let Some(text) = config_text {
            if user_config::is_legacy(text) {
                let p = user_config::legacy_programs(text, options);
                run(Layer::Config, &p.config, "config.scm");
                run(Layer::Custom, &p.custom, "config.scm (managed block)");
            } else {
                run(Layer::Config, text, "config.scm");
            }
        }
        if let Some(text) = custom_text {
            custom_ok = run(Layer::Custom, text, "custom.scm");
        }
    } else {
        if config_text.is_some() {
            log("scheme session not ready: config.scm skipped".into());
        }
        match custom_text.map(changes_from_literal_program) {
            Some(Ok(c)) => changes.extend(c),
            Some(Err(e)) => {
                custom_ok = false;
                log(e);
            }
            None => {}
        }
    }
    for p in crate::script::projection::apply_options(&changes, options) {
        log(p);
    }
    user.absorb(&changes);
    custom_ok
}

#[cfg(test)]
impl App {
    /// Drive an intent through the full pipeline, then broadcast cursor-changed
    /// (as handle_key does), so tests see the auto content-follow (test seam).
    pub fn dispatch_for_test(&mut self, intent: Intent) {
        self.dispatch(intent);
        self.broadcast_cursor_changed();
    }

    /// Block until one async result arrives and reduce it (test seam).
    pub fn pump_one(&mut self) {
        if let Ok(AppEvent::Async(r)) = self.rx.recv() {
            self.handle_async(r);
        }
    }

    /// The active content provider's logical row count, if it produced a view.
    pub fn function_content_rows(&self) -> Option<usize> {
        self.provider_view().map(|v| match &*v {
            crate::core::PanelView::Document(blocks) => blocks.len(),
            crate::core::PanelView::Image { .. } => 1,
        })
    }

    /// The function-panel title for the active provider's view (the cursor file
    /// name + any search position), if the provider produced a view.
    pub fn function_title(&self) -> Option<String> {
        self.provider_view().map(|_| {
            crate::ui::content::title(self.cursor_signature().0.as_deref(), &self.state.function.search)
        })
    }

    fn provider_view(&self) -> Option<Arc<crate::core::PanelView>> {
        let theme = crate::core::Theme::default();
        let sources = self.state.targets();
        let ctx = crate::extension::FunctionRenderCtx {
            width: 80,
            height: 24,
            focused: true,
            func: &self.state.function,
            sources: &sources,
            theme: &theme,
        };
        self.extensions.provider().and_then(|p| p.view(&ctx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Mods;

    pub(super) fn temp_dir_with_files() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let mut dir = std::env::temp_dir();
        dir.push(format!("wasdf-test-{}-{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("alpha.txt"), b"hello\nworld\n").unwrap();
        std::fs::write(dir.join("beta.txt"), b"second file").unwrap();
        std::fs::create_dir_all(dir.join("subdir")).unwrap();
        dir
    }

    /// An extension that records the captured keys it is handed and otherwise
    /// does nothing at all — in particular it never releases the capture. The
    /// lockout guarantee only means something if it holds against an extension
    /// that refuses to cooperate, so that is what these tests use.
    #[derive(Default)]
    struct InertCapturer {
        got: std::sync::Mutex<Vec<String>>,
    }

    impl Extension for InertCapturer {
        fn id(&self) -> &str {
            "capturer"
        }
        fn handle_intent(&self, i: &ExtensionIntent, _s: &AppState) -> Vec<Intent> {
            let label = match i.intent.as_str() {
                "key" => codec::decode_key(&i.data).map(codec::key_name).unwrap_or_default(),
                other => format!("<{other}>"),
            };
            self.got.lock().unwrap().push(label);
            Vec::new()
        }
    }

    /// Boot an app with `InertCapturer` registered and a capture already
    /// installed: released by `Ctrl-b`, escaping on `escape`.
    fn app_capturing(escape: Vec<Intent>) -> (App, std::sync::Arc<InertCapturer>) {
        // The registry takes a Box, so keep a parallel Arc purely to read the
        // recorded keys back out; both point at the same recording Mutex only
        // if we register the Arc itself.
        let ext = std::sync::Arc::new(InertCapturer::default());
        struct Shim(std::sync::Arc<InertCapturer>);
        impl Extension for Shim {
            fn id(&self) -> &str {
                self.0.id()
            }
            fn handle_intent(&self, i: &ExtensionIntent, s: &AppState) -> Vec<Intent> {
                self.0.handle_intent(i, s)
            }
        }
        let dir = temp_dir_with_files();
        let mut app = App::boot(dir);
        app.pump_one();
        app.extensions.register(Box::new(Shim(ext.clone())));
        app.dispatch_for_test(Intent::SetInputCapture(Some(Box::new(InputCapture {
            owner: "capturer".into(),
            release: Key::new(KeyCode::Char('b'), Mods::CTRL),
            escape,
        }))));
        (app, ext)
    }

    #[test]
    fn a_captured_bound_key_reaches_the_extension_instead_of_acting() {
        let (mut app, ext) = app_capturing(Vec::new());
        // `q` quits in File mode. Under capture it must not.
        app.handle_key(Key::plain(KeyCode::Char('q')));
        assert!(!app.state.quit, "q must not quit while captured");
        assert_eq!(ext.got.lock().unwrap().as_slice(), ["q"]);
    }

    #[test]
    fn the_release_key_frees_the_user_even_when_the_extension_does_nothing() {
        let (mut app, ext) = app_capturing(Vec::new());
        app.handle_key(Key::new(KeyCode::Char('b'), Mods::CTRL));
        assert!(
            app.state.input_capture.is_none(),
            "the release must not depend on the extension answering"
        );
        assert_eq!(ext.got.lock().unwrap().as_slice(), ["<prefix>"]);
    }

    #[test]
    fn the_help_row_leads_with_the_fixed_order_and_question_mark_lists_the_keys() {
        let mut app = App::boot(temp_dir_with_files());
        app.pump_one();
        let labels: Vec<String> = app
            .keymap
            .help_hints(&app.conditions, &app.state, "file")
            .into_iter()
            .map(|(_, label)| label)
            .collect();
        let lead = ["cursor", "select", "edit", "find", "move", "delete", "shell", "quit", "help"];
        let present: Vec<&str> = lead.iter().copied().filter(|l| labels.iter().any(|x| x == l)).collect();
        assert_eq!(&labels[..present.len()], present.as_slice(), "{labels:?}");
        let rest: Vec<String> = labels[present.len()..].iter().map(|l| l.to_lowercase()).collect();
        assert!(rest.windows(2).all(|w| w[0] <= w[1]), "the rest is alphabetical: {rest:?}");

        app.handle_key(Key::plain(KeyCode::Char('?')));
        let Mode::Select(spec) = app.state.mode() else { panic!("? opens the key list") };
        assert_eq!(spec.id, "key-list");
        let rows: Vec<String> = app.state.select.as_ref().unwrap().results.iter().map(|c| c.label.clone()).collect();
        assert!(rows.iter().any(|r| r.starts_with("w ") && r.contains("cursor-up")), "{rows:?}");
        assert!(rows.iter().all(|r| !r.starts_with("Q ") && !r.starts_with("Ctrl-c ")), "{rows:?}");
        let first_single = rows.iter().position(|r| r.chars().nth(1) == Some(' ')).unwrap();
        assert!(rows[..first_single].iter().all(|r| r.chars().nth(1) != Some(' ')), "special keys lead: {rows:?}");
        // Held under the Kitty protocol, `s` arrives as repeats: they scroll
        // like presses and never fall through to the (absent) text input.
        let held = Key { kind: KeyKind::Repeat, ..Key::plain(KeyCode::Char('s')) };
        app.handle_key(Key::plain(KeyCode::Char('s')));
        app.handle_key(held);
        app.handle_key(held);
        let sel = app.state.select.as_ref().unwrap();
        assert_eq!((sel.selected, sel.phase, sel.query.as_str()), (3, crate::core::SelectPhase::Navigate, ""));
        app.handle_key(Key::plain(KeyCode::Char('x')));
        assert!(app.state.select.as_ref().unwrap().query.is_empty(), "nothing to type into");
        app.handle_key(Key::plain(KeyCode::Char('q')));
        assert!(matches!(app.state.mode(), Mode::File));
    }

    #[test]
    fn the_arrows_drive_the_file_cursor_like_wasd_and_keep_focus() {
        let mut app = App::boot(temp_dir_with_files());
        app.pump_one();
        let key = |c| Key::plain(c);
        let resolve = |app: &App, k| app.keymap.resolve(&app.conditions, &app.state, "file", k);
        for (arrow, wasd) in [(KeyCode::Up, 'w'), (KeyCode::Left, 'a'), (KeyCode::Down, 's'), (KeyCode::Right, 'd')] {
            assert_eq!(resolve(&app, key(arrow)), resolve(&app, key(KeyCode::Char(wasd))), "{arrow:?}");
        }
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.state.cursor, 1);
        assert_eq!(app.state.focused_panel, crate::core::PANEL_FILE);
        // k/j still scroll the function panel; hidden, it does not take focus.
        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(app.state.focused_panel, crate::core::PANEL_FILE);
    }

    #[test]
    fn a_key_release_types_nothing_in_the_file_search_box() {
        let mut app = App::boot(temp_dir_with_files());
        app.pump_one();
        app.handle_key(Key::plain(KeyCode::Char('f')));
        app.handle_key(Key { kind: KeyKind::Release, ..Key::plain(KeyCode::Char('f')) });
        for c in ['a', 'b'] {
            let down = Key::plain(KeyCode::Char(c));
            app.handle_key(down);
            app.handle_key(Key { kind: KeyKind::Release, ..down });
        }
        let Some(sel) = app.state.select.as_ref() else { panic!("f opens the search Select") };
        assert_eq!(sel.query, "ab");
    }

    #[test]
    fn content_read_off_the_cursor_is_dropped() {
        // Reproduces the empty-directory race: a content read that was in flight
        // when the cursor left its target (navigating into an empty directory)
        // must be dropped on arrival, so it does not re-populate the preview that
        // the cursor-changed clear already dropped.
        let dir = temp_dir_with_files();
        let mut app = App::boot(dir.clone());
        app.pump_one(); // initial ReadDir

        // Show the function panel on a file and load its content.
        app.dispatch_for_test(Intent::CursorDown); // off the directory, onto a file
        app.dispatch_for_test(Intent::CursorRight); // reveal + follow
        app.pump_one(); // the content read is accepted (cursor still on it)
        assert!(app.function_content_rows().is_some(), "preview loaded for the file");

        // Simulate entering an empty directory: no entries, so nothing is under the
        // cursor. The cursor-changed clear drops the stale preview.
        app.state.entries.clear();
        app.state.cursor = 0;
        app.broadcast_cursor_changed();
        assert!(app.function_content_rows().is_none(), "preview cleared on empty dir");

        // A late read for the file we navigated away from arrives — it must be
        // dropped, not re-stashed.
        let stale = AsyncResult {
            request_id: 0,
            purpose: Purpose::Content,
            mode_generation: app.state.mode_generation(),
            status: AsyncStatus::Ok,
            payload: Payload::Read {
                owner: "preview".into(),
                path: dir.join("alpha.txt"),
                result: crate::core::Content::Bytes {
                    offset: 0,
                    bytes: b"STALE\n".to_vec(),
                    eof: true,
                },
            },
        };
        app.handle_async(stale);
        assert!(
            app.function_content_rows().is_none(),
            "a content read off the current cursor must not repopulate the preview",
        );
    }

    #[test]
    fn delete_pushes_policy_then_confirms() {
        let dir = temp_dir_with_files();
        let target = dir.join("beta.txt");
        let mut app = App::boot(dir);
        app.pump_one();
        // Select beta.txt and request delete.
        app.state.cursor = app
            .state
            .entries
            .iter()
            .position(|e| e.path == target)
            .unwrap();
        app.dispatch_for_test(Intent::DeleteSelected);
        assert!(matches!(app.state.mode(), Mode::Policy(_)), "delete gates on Policy");
        app.dispatch_for_test(Intent::Confirm); // allow
        assert!(matches!(app.state.mode(), Mode::File));
        app.pump_one(); // OpDone → triggers refresh
        app.pump_one(); // refresh entries
        assert!(!target.exists(), "beta.txt deleted");
    }

    #[test]
    fn cmdop_deep_search_submit_lands_through_the_real_kernel_pipeline() {
        // End-to-end: start-copy, "/" for
        // deep search, type + Enter through the real handle_key path (not
        // dispatch_for_test, which bypasses the function.input raw-key gate and
        // the real async round trip) — the exact layer the reducer-only and
        // extension-only unit tests never exercise.
        use crate::core::ExtensionIntent;
        let dir = temp_dir_with_files();
        let mut app = App::boot(dir.clone());
        app.pump_one(); // initial ReadDir
        app.dispatch_for_test(Intent::CursorDown); // off subdir, onto alpha.txt
        assert_eq!(app.state.current_entry().unwrap().name, "alpha.txt");

        app.dispatch_for_test(Intent::Extension(ExtensionIntent {
            extension: "cmdop".into(),
            intent: "start-copy".into(),
            data: ExtensionValue::Nil,
        }));
        assert_eq!(app.state.function.form_owner.as_deref(), Some("cmdop"));
        app.pump_one(); // the browse dir's own LoadContent listing lands

        app.dispatch_for_test(Intent::Extension(ExtensionIntent {
            extension: "cmdop".into(),
            intent: "filter-start".into(),
            data: ExtensionValue::Nil,
        }));
        assert!(app.state.function.input.is_some(), "search-deep input is open");
        assert_eq!(app.state.function.input.as_ref().unwrap().purpose, "search-deep");

        for c in "alpha".chars() {
            app.handle_key(Key::plain(KeyCode::Char(c)));
        }
        assert_eq!(
            app.state.function.input.as_ref().unwrap().query, "alpha",
            "each character reached the query buffer, none were hijacked as file-cursor keymap bindings",
        );
        app.handle_key(Key::plain(KeyCode::Enter));
        assert!(app.state.function.input.is_none(), "Enter closed the input box");
        assert_eq!(
            app.state.function.form_owner.as_deref(), Some("cmdop"),
            "the builder is still open — Enter must not have fallen through to a stray keymap binding",
        );

        app.pump_one(); // the owner-addressed Plan::Search completes as a content Read
        assert_eq!(app.state.function.form_owner.as_deref(), Some("cmdop"), "still open after the search lands");
    }

    #[test]
    fn file_search_loads_selected_candidate() {
        let dir = temp_dir_with_files();
        let mut app = App::boot(dir);
        app.pump_one(); // initial ReadDir
        app.dispatch_for_test(Intent::PushMode(Box::new(Mode::Select(
            crate::core::SelectSpec::file_search(),
        ))));
        app.pump_one(); // Search result → populates the entry view + spawns the dir content read
        assert!(!app.state.select.as_ref().unwrap().view.as_ref().unwrap().is_empty());
        app.pump_one(); // content read for the highlighted candidate (dir listing)
        assert!(app.function_content_rows().is_some(), "file-search follows the dir content");
    }

    fn core_options() -> OptionStore {
        OptionStore::new(defaults::core_options())
    }

    fn ready_settings() -> crate::script::store::Store {
        let session = crate::script::SchemeSession::spawn();
        assert!(session.wait_ready(Duration::from_secs(600)));
        crate::script::store::Store::new(session)
    }

    #[test]
    fn boot_builds_the_projections_from_config_and_custom() {
        use crate::core::{Key, KeyCode};
        let options = core_options();
        let mut user = crate::script::projection::UserLayer::default();
        let config = "(define macos? (equal? (os) (os)))\n\
                      (when macos? (opt-set! 'border-style \"rounded\"))\n\
                      (opt-set! 'border-style \"wavy\")\n\
                      (bind! 'file 'file \"z\" 'cursor-bottom \"bottom\")\n\
                      (opt-set! 'content-auto-update #t)\n";
        let custom = "(opt-set! 'content-auto-update #f)\n(unbind! 'file #f \"Q\")\n";
        load_user_settings(&ready_settings(), true, &options, &[], Some(config), Some(custom), &mut user);
        assert_eq!(options.get_str("border-style"), "rounded", "the failing form was skipped");
        assert!(!options.get_bool("content-auto-update"), "custom.scm overrides config.scm");
        let (binds, problems) = user.bindings();
        assert!(problems.is_empty(), "{problems:?}");
        let z = binds.iter().find(|b| b.key == Key::plain(KeyCode::Char('z'))).unwrap();
        assert_eq!(z.intent, Intent::CursorBottom);
        assert_eq!(z.layer, crate::script::keymap::Layer::User);
        let q = binds.iter().find(|b| b.key == Key::plain(KeyCode::Char('Q'))).unwrap();
        assert_eq!(q.intent, Intent::Noop, "unbind! is a tombstone");
    }

    #[test]
    fn without_the_session_custom_scm_still_applies() {
        let options = core_options();
        let mut user = crate::script::projection::UserLayer::default();
        let session = crate::script::SchemeSession::spawn();
        let settings = crate::script::store::Store::new(session);
        let custom = "(opt-set! 'border-style \"double\")\n(bind! 'file 'file \"z\" 'quit)\n";
        load_user_settings(&settings, false, &options, &[], Some("(opt-set! 'border-style \"ascii\")"), Some(custom), &mut user);
        assert_eq!(options.get_str("border-style"), "double", "config.scm is skipped, custom.scm applies");
        assert_eq!(user.bindings().0.len(), 1);
    }

    #[test]
    fn execute_streams_output() {
        use crate::core::SubLayout;
        let dir = temp_dir_with_files();
        let mut app = App::boot(dir);
        app.pump_one(); // initial ReadDir
        // Execute a command that emits three lines; the reducer opens the Exec
        // frame and the executor streams output line-by-line.
        app.dispatch_for_test(Intent::Execute {
            argv: vec!["sh".into(), "-c".into(), "echo a; echo b; echo c".into()],
        });
        assert_eq!(app.state.function.sublayout, SubLayout::Exec);
        assert!(!app.state.function.exec.finished);
        let mut guard = 0;
        while !app.state.function.exec.finished && guard < 100 {
            app.pump_one();
            guard += 1;
        }
        assert!(app.state.function.exec.finished, "exec did not finish");
        let lines = &app.state.function.exec.lines;
        assert!(
            lines.contains(&"a".to_string()) && lines.contains(&"c".to_string()),
            "streamed lines: {lines:?}"
        );
        assert_eq!(app.state.function.exec.exit, Some(0));
    }

}

#[cfg(test)]
mod frame_dump {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    pub fn draw(app: &mut App, w: u16, h: u16) -> (String, ratatui::buffer::Buffer) {
        let s = &app.store;
        let top = crate::ui::top::TopOptions {
            border_style: s.get_str("border-style"),
            monitor: crate::ui::top::MonitorOptions::from_store(s),
            scrollbar: crate::ui::chrome::ScrollbarChars {
                thumb: s.get_str("scrollbar-thumb"),
                rail: s.get_str("scrollbar-rail"),
                disabled: s.get_str("scrollbar-disabled"),
            },
            file_row_size_graph: s.get_bool("file-row-size-graph"),
        };
        let theme = app.theme_source.resolve_active();
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        let snapshot = app.monitor.snapshot();
        let view = crate::ui::render::View {
            opts: &top,
            layouts: &app.layouts,
            options: &app.store,
            keymap: &app.keymap,
            conds: &app.conditions,
            monitor: &snapshot,
        };
        term.draw(|f| app.ui.render(f, &app.state, &app.extensions, &view, &theme)).unwrap();
        let buf = term.backend().buffer().clone();
        app.after_draw();
        let text = (0..h).map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>() + "\n").collect();
        (text, buf)
    }

    pub fn dump(app: &mut App, w: u16, h: u16) -> String {
        draw(app, w, h).0
    }

    fn rows(frame: &str) -> Vec<&str> {
        frame.lines().collect()
    }

    #[test]
    fn frames_draw_from_the_layouts() {
        let mut app = App::boot(super::tests::temp_dir_with_files());
        app.pump_one();
        let cwd = app.state.cwd.display().to_string();

        // file layout, Compact (14 rows → a 5-row top): the path sits on the
        // line the top row shares with the file panel, the help row is last.
        let f = dump(&mut app, 100, 14);
        let r = rows(&f);
        assert!(r[4].starts_with("└─[ ") && r[4].contains(&cwd[..20]), "{f}");
        assert!(r[5].starts_with("  "), "the file panel has no left border:\n{f}");
        assert!(r[13].contains("quit"), "{f}");
        assert!(app.ui.shows("monitor"), "the monitor is on screen, so it is sampled");

        // With the function panel, its border crosses the shared line.
        app.state.function.visible = true;
        let f = dump(&mut app, 100, 14);
        assert!(rows(&f)[6].contains('│'), "{f}");

        // select layout: the input row, then the candidates below its line.
        app.state.function.visible = false;
        app.dispatch_for_test(Intent::PushMode(Box::new(Mode::Select(crate::core::SelectSpec::file_search()))));
        let f = dump(&mut app, 100, 14);
        assert!(rows(&f)[5].starts_with(" >"), "{f}");
        assert!(rows(&f)[6].starts_with("────"), "{f}");
        app.dispatch_for_test(Intent::PopMode);

        // A user layout: the function panel under the file panel keeps its own
        // title on its own border, and help still has the last row.
        app.state.function.visible = true;
        let tree = crate::script::sexpr::parse(
            "(column (row (size 5) (panel nav) (panel monitor (min 16))) (panel file) \
             (panel function (size 5) (when function-visible)) (panel help (size 1)))",
        )
        .unwrap();
        app.layouts.set_user(vec![("file".into(), crate::script::layout::parse("file", &tree).unwrap())]);
        let f = dump(&mut app, 100, 16);
        let r = rows(&f);
        assert!(r[10].starts_with("──[ "), "{f}");
        assert!(r[15].contains("quit"), "{f}");
    }
}
