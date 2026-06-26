//! The kernel: holds all services and AppState, and runs dispatch_intent — the
//! single pipeline stage that performs extension dispatch, the confirmation
//! gate, and plan issuance + reduction. The event loop calls into it; it
//! contains no intent-specific branches beyond the two structural gates the
//! spec names (extension resolver key, destructive → Policy).

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

use crate::core::{
    self, AppEvent, AppState, AsyncResult, AsyncStatus, ExtensionValue, Intent, Key, KeyCode, Mode,
    Payload, Plan, Purpose, ResolverRequest, KEY_RESOLVER,
};
use crate::extension::{self, ExtensionRegistry};
use crate::runtime::TaskManager;
use crate::script::condition::{Cond, Conditions};
use crate::script::keymap::{Binding, KeymapRegistry, Layer};
use crate::services::command::CommandDef;
use crate::services::resolver::ResolverEntry;
use crate::services::{CommandRegistry, ResolverChain, SkimMatcher};
use crate::ui::UiManager;

const REDISPATCH_CAP: u32 = 32;

/// The flat option store (the Scheme `set-option` analogue). Behavior-affecting
/// flags read by the kernel's hooks. Unknown keys would warn at parse time.
#[derive(Debug, Clone)]
pub struct Options {
    /// ui.content.auto-update — refresh the function-panel content as the file
    /// cursor moves.
    pub content_auto_update: bool,
}

impl Default for Options {
    fn default() -> Self {
        // Default off, per UI.md. Overridable by env until config parsing lands.
        Options {
            content_auto_update: std::env::var("WASDF_CONTENT_AUTO")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false),
        }
    }
}

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
    pub options: Options,
    pub rx: Receiver<AppEvent>,
    pub tx: Sender<AppEvent>,
    pending_suspend: Option<Vec<String>>,
    /// The last cursor identity broadcast to extensions (target path + panel
    /// visibility), so the cursor-changed event fires only on an actual change.
    last_cursor: (Option<std::path::PathBuf>, bool, usize),
    /// Memo of per-command options parsed from `--help`/`man`.
    option_cache: OptionCache,
}

impl App {
    /// Boot the kernel: build registries, load embedded defaults, register
    /// bundled extensions, and issue the initial directory read.
    pub fn boot(cwd: std::path::PathBuf) -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<AppEvent>();

        // Spawn the resident Scheme REPL session first (pillar #3). Wait briefly
        // for it to be ready: on a warm bytecode cache it is ready in
        // milliseconds and parses the embedded config; on a cold cache (first
        // ever launch, multi-minute compile) we fall back to native defaults so
        // boot never freezes — the session warms in the background and caches.
        let scheme = crate::script::SchemeSession::spawn();
        let scheme_ready = scheme.wait_ready(Duration::from_secs(3));

        // Keymaps, commands, and resolvers are sourced from embedded Scheme,
        // evaluated by the session (Role 1), or native defaults if not ready.
        let mut keymap = KeymapRegistry::new();
        keymap.extend(if scheme_ready {
            scheme_keymaps(&scheme)
        } else {
            crate::script::keymap::defaults()
        });

        let mut conditions = Conditions::default();

        let mut commands = CommandRegistry::new();
        commands.extend(if scheme_ready {
            scheme_commands(&scheme)
        } else {
            crate::services::command::defaults()
        });

        let mut resolver = ResolverChain::new();
        resolver.extend(if scheme_ready {
            scheme_resolvers(&scheme)
        } else {
            crate::services::resolver::defaults()
        });

        // Register bundled extensions: keymaps, commands, resolver entries,
        // conditions. The only place bundled extensions are named is the facade.
        let mut extensions = ExtensionRegistry::new();
        for ext in extension::bundled() {
            keymap.extend(ext.keymaps());
            keymap.extend(extension_scheme_keymaps(&scheme, scheme_ready, ext.as_ref()));
            commands.extend(ext.commands());
            resolver.extend(ext.resolver_entries());
            ext.register_conditions(&mut conditions);
            extensions.register(ext);
        }

        // Optional (dynamically loaded) extensions: bundled first, then optional
        // in file-name order; an id collision disables the later one. Registered
        // like bundled so their keymaps/commands/resolvers apply and extension
        // intents route to their handle_intent.
        let mut ext_ids: std::collections::HashSet<String> =
            extensions.iter().map(|e| e.id().to_string()).collect();
        for ext in load_optional_extensions() {
            if !ext_ids.insert(ext.id().to_string()) {
                eprintln!("optional extension id collides ({}); disabled", ext.id());
                continue;
            }
            keymap.extend(ext.keymaps());
            keymap.extend(extension_scheme_keymaps(&scheme, scheme_ready, ext.as_ref()));
            commands.extend(ext.commands());
            resolver.extend(ext.resolver_entries());
            ext.register_conditions(&mut conditions);
            extensions.register(ext);
        }

        // User layer last: highest priority, with collision detection across
        // all layers. Deleting the user file reproduces the embedded defaults.
        if scheme_ready {
            keymap.extend(scheme_user_keymaps(&scheme));
        }

        for w in keymap.warnings() {
            eprintln!("warning: {w}");
        }

        let resolver = Arc::new(resolver);
        let matcher: Arc<dyn crate::services::MatcherBackend> = Arc::new(SkimMatcher::default());
        let tasks =
            TaskManager::new(tx.clone(), resolver.clone(), matcher, Some(scheme.clone()));

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
            options: Options::default(),
            rx,
            tx,
            pending_suspend: None,
            last_cursor: (None, false, 0),
            option_cache: std::cell::RefCell::new(std::collections::HashMap::new()),
        };
        // Initial listing.
        app.spawn_plan(Plan::ReadDir { path: cwd, show_hidden: false });
        app
    }

    /// Handle one key: resolve to an intent, or deliver a RawKeyEvent in Select.
    pub fn handle_key(&mut self, key: Key) {
        // While the function-panel search input is open, keys edit the query — except
        // Enter (submit) and Esc (cancel), which still resolve via the keymap.
        if self.state.function.search.input_active
            && !matches!(key.code, KeyCode::Enter | KeyCode::Esc)
        {
            self.dispatch(Intent::RawKeyEvent(key));
            self.broadcast_cursor_changed();
            return;
        }
        let panel = self.state.focused_panel.clone();
        match self.keymap.resolve(&self.conditions, &self.state, &panel, key) {
            Some(intent) => self.dispatch(intent),
            None => {
                if matches!(self.state.mode(), Mode::Select(_)) {
                    self.dispatch(Intent::RawKeyEvent(key));
                }
            }
        }
        self.broadcast_cursor_changed();
    }

    /// Handle an async completion: drop stale (mode-bound) results, then reduce.
    pub fn handle_async(&mut self, result: AsyncResult) {
        // Content reads are handed to the owning extension (which decodes +
        // stashes); they do not flow through the pure reducer.
        if result.purpose == Purpose::Content {
            if result.status == AsyncStatus::Ok {
                if let Payload::Read { owner, path, result: read } = result.payload {
                    if let Some(ext) = self.extensions.find(&owner) {
                        ext.accept_content(&path, &read);
                    }
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

    /// The cursor identity for the broadcast gate: what's under the cursor (the
    /// File-mode entry, or the file-search candidate) and whether the function
    /// panel is open. Generic — it carries no extension-specific notion.
    fn cursor_signature(&self) -> (Option<std::path::PathBuf>, bool, usize) {
        let path = match self.state.mode() {
            Mode::File => self.state.current_entry().map(|e| e.path.clone()),
            Mode::Select(spec) if spec.id == "file-search" => self
                .state
                .select
                .as_ref()
                .and_then(|s| s.view.as_ref())
                .and_then(|v| v.current())
                .map(|e| e.path.clone()),
            _ => None,
        };
        // Scroll is part of the signature so paging down re-broadcasts and the
        // content provider can fetch the next chunk as the viewport advances.
        (path, self.state.function.visible, self.state.function.scroll)
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
        let intents: Vec<Intent> = {
            let state = &self.state;
            self.extensions.iter().flat_map(|e| e.on_cursor_changed(state)).collect()
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
    /// regardless of outcome) and refresh the listing.
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
                    if self.resolver.is_destructive(&req.op) && !confirmed {
                        let i = intent.clone();
                        self.state.push_mode(Mode::Policy(Box::new(i)));
                        return;
                    }
                    self.spawn_plan(Plan::ResolveAndRun { request: req });
                } else {
                    self.ui.notify(core::Notice::error("malformed resolver intent"));
                }
                return;
            }
            let ext = ext.clone();
            for i in self.extensions.dispatch(&ext, &self.state) {
                self.dispatch_depth(i, confirmed, depth + 1);
            }
            return;
        }

        // Confirmation gate: a destructive resolver op (e.g. delete) → Policy.
        if let Intent::RunResolver(req) = &intent {
            if self.resolver.is_destructive(&req.op) && !confirmed {
                self.state.push_mode(Mode::Policy(Box::new(intent)));
                return;
            }
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
            };
            core::apply_intent(&mut self.state, intent, &ctx)
        };
        self.process_effects(effects, depth);
    }

    fn process_effects(&mut self, effects: core::Effects, depth: u32) {
        for n in effects.notices {
            self.ui.notify(n);
        }
        for p in effects.plans {
            self.spawn_plan(p);
        }
        for i in effects.intents {
            self.dispatch_depth(i, false, depth + 1);
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
}

impl core::CommandLookup for CmdCtx<'_> {
    fn intent_of(&self, name: &str) -> Option<Intent> {
        self.commands.intent_of(name)
    }
    fn command_candidates(&self) -> Vec<core::Candidate> {
        self.commands.candidates()
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

/// The embedded default resolver schema, in Scheme. Evaluated by the session so
/// Scheme is genuinely the source of truth for this configuration.
const RESOLVER_SCHEME: &str = include_str!("resolver.scm");

/// Eval a config Scheme const through the session, parse it into the registry
/// type, and fall back to the native embedded defaults on any eval or parse
/// failure (disable, notify, continue). The single shape shared by every
/// `scheme_*` config source below.
fn scheme_config<T>(
    session: &crate::script::SchemeSession,
    src: &str,
    label: &str,
    parse: impl FnOnce(&crate::script::sexpr::Datum) -> Result<Vec<T>, String>,
    fallback: impl FnOnce() -> Vec<T>,
) -> Vec<T> {
    session
        .eval(src)
        .and_then(|printed| crate::script::sexpr::parse(&printed))
        .and_then(|datum| parse(&datum))
        .unwrap_or_else(|e| {
            eprintln!("scheme {label} config failed ({e}); using embedded defaults");
            fallback()
        })
}

/// Source resolver entries from Scheme via the session; fall back to the native
/// embedded defaults on any eval or parse failure.
fn scheme_resolvers(session: &crate::script::SchemeSession) -> Vec<ResolverEntry> {
    scheme_config(
        session,
        RESOLVER_SCHEME,
        "resolver",
        |d| crate::script::config::parse_resolver_config(d),
        crate::services::resolver::defaults,
    )
}


/// The embedded default command palette, in Scheme: (name description intent).
const COMMAND_SCHEME: &str = include_str!("command.scm");

/// Source palette commands from Scheme via the session; fall back to defaults.
fn scheme_commands(session: &crate::script::SchemeSession) -> Vec<CommandDef> {
    scheme_config(
        session,
        COMMAND_SCHEME,
        "command",
        |d| crate::script::config::parse_command_config(d),
        crate::services::command::defaults,
    )
}

/// The embedded default keymap, in Scheme. Each group is
/// (mode panel-or-#f (binding...)); each binding is (key intent) or
/// (key intent when). This is the core layer only; extensions add their own.
const KEYMAP_SCHEME: &str = include_str!("keymap.scm");

/// Source the core keymap from Scheme via the session; fall back to defaults.
fn scheme_keymaps(session: &crate::script::SchemeSession) -> Vec<Binding> {
    scheme_config(
        session,
        KEYMAP_SCHEME,
        "keymap",
        |d| crate::script::config::parse_keymap_config(d, Layer::Core),
        crate::script::keymap::defaults,
    )
}

/// Evaluate an extension's declarative Scheme source into Extension-layer keymap
/// bindings. Uses the resident REPL when ready (Role 1: the session is the
/// config parser); on a cold cache it parses the quoted literal directly so the
/// bindings still register. The source is `(quote ((mode panel (binding…)) …))`.
fn extension_scheme_keymaps(
    session: &crate::script::SchemeSession,
    ready: bool,
    ext: &dyn crate::extension::Extension,
) -> Vec<Binding> {
    let Some(src) = ext.scheme_source() else {
        return Vec::new();
    };
    let groups = if ready {
        session.eval(&src).and_then(|printed| crate::script::sexpr::parse(&printed))
    } else {
        crate::script::sexpr::parse(&src).and_then(|d| {
            d.as_list()
                .and_then(|l| l.get(1).cloned())
                .ok_or_else(|| "extension scheme source is not a (quote …) form".to_string())
        })
    };
    match groups.and_then(|d| crate::script::config::parse_keymap_config(&d, Layer::Extension)) {
        Ok(binds) => binds,
        Err(e) => {
            eprintln!("extension '{}' scheme keymap failed ({e}); ignoring", ext.id());
            Vec::new()
        }
    }
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

/// Load User-layer keybindings: write the commented template if absent, then
/// parse the uncommented `(bind …)` lines. The User layer wins via priority,
/// and deleting the file reproduces exactly the embedded defaults.
fn scheme_user_keymaps(session: &crate::script::SchemeSession) -> Vec<Binding> {
    // Tests never touch the real user config; the user-config decode path is
    // covered directly by parse_user_binds / keybindings_template tests.
    if cfg!(test) {
        return Vec::new();
    }
    let Some(dir) = config_dir() else { return Vec::new() };
    let path = dir.join("keybindings.scm");
    if !path.exists() {
        let _ = std::fs::write(&path, keybindings_template());
    }
    let Ok(contents) = std::fs::read_to_string(&path) else { return Vec::new() };
    // Read the uncommented (bind …) forms as data through the session, so the
    // Scheme reader handles comment stripping and syntax validation.
    let expr = format!("(quote ({contents}))");
    match session
        .eval(&expr)
        .and_then(|printed| crate::script::sexpr::parse(&printed))
        .and_then(|datum| parse_user_binds(&datum))
    {
        Ok(binds) => binds,
        Err(e) => {
            eprintln!("user keybindings failed ({e}); ignoring user overrides");
            Vec::new()
        }
    }
}

fn parse_user_binds(datum: &crate::script::sexpr::Datum) -> Result<Vec<Binding>, String> {
    use crate::script::sexpr::Datum;
    let forms = datum.as_list().ok_or("user keybindings is not a list")?;
    let mut out = Vec::new();
    for form in forms {
        let p = form.as_list().ok_or("binding is not a list")?;
        if p.first().and_then(|d| d.as_sym()) != Some("bind") {
            return Err("expected a (bind …) form".into());
        }
        let mode = p.get(1).and_then(|d| d.as_sym()).ok_or("bind missing mode")?.to_string();
        let panel = match p.get(2) {
            Some(Datum::Bool(false)) => None,
            Some(d) => d.as_sym().map(str::to_string),
            None => None,
        };
        let key_name = p.get(3).and_then(|d| d.text()).ok_or("bind missing key")?;
        let key = crate::script::codec::parse_key(key_name)
            .ok_or_else(|| format!("unknown key: {key_name}"))?;
        let intent = p
            .get(4)
            .and_then(crate::script::codec::intent_from_datum)
            .ok_or("bind has an unknown intent")?;
        let when = match p.get(5) {
            Some(d) => crate::script::codec::cond_from_datum(d),
            None => Cond::Always,
        };
        out.push(Binding { mode, panel, key, intent, when, layer: Layer::User });
    }
    Ok(out)
}

/// Generate the fully commented-out keybindings template from the embedded
/// keymap source, so every default appears as an editable `(bind …)` line.
fn keybindings_template() -> String {
    let mut s = String::from(
        ";; wasdf keybindings — uncomment and edit a line to override a default.\n\
         ;; Deleting this file restores the built-in defaults exactly.\n\
         ;; Form: (bind <mode> <panel|#f> \"<key>\" <intent> [<when>])\n\n",
    );
    if let Ok(quoted) = crate::script::sexpr::parse(KEYMAP_SCHEME) {
        // KEYMAP_SCHEME parses as (quote (<group> …)); take the groups.
        if let Some(groups) = quoted.as_list().and_then(|l| l.get(1)).and_then(|d| d.as_list()) {
            for group in groups {
                let Some(g) = group.as_list() else { continue };
                let (Some(mode), Some(panel), Some(binds)) =
                    (g.first(), g.get(1), g.get(2).and_then(|d| d.as_list()))
                else {
                    continue;
                };
                for b in binds {
                    let Some(bp) = b.as_list() else { continue };
                    let mut parts = vec![
                        "bind".to_string(),
                        crate::script::sexpr::render(mode),
                        crate::script::sexpr::render(panel),
                    ];
                    for el in bp {
                        parts.push(crate::script::sexpr::render(el));
                    }
                    s.push_str("; (");
                    s.push_str(&parts.join(" "));
                    s.push_str(")\n");
                }
            }
        }
    }
    s
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

    /// Render the active content provider; the row count if it drew anything.
    pub fn function_content_rows(&self) -> Option<usize> {
        self.render_provider().map(|d| d.total)
    }

    /// The active content provider's rendered title, if any.
    pub fn function_title(&self) -> Option<String> {
        self.render_provider().map(|d| d.title)
    }

    fn render_provider(&self) -> Option<crate::extension::FunctionDraw> {
        let ctx = crate::extension::FunctionRenderCtx {
            width: 80,
            height: 24,
            focused: true,
            func: &self.state.function,
        };
        self.extensions.provider().and_then(|p| p.render_function(&ctx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::SubLayout;

    fn temp_dir_with_files() -> std::path::PathBuf {
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

    #[test]
    fn boots_and_lists_directory() {
        let dir = temp_dir_with_files();
        let mut app = App::boot(dir.clone());
        app.pump_one(); // initial ReadDir
        assert_eq!(app.state.entries.len(), 3, "subdir, alpha, beta");
        // Directory sorts first.
        assert!(app.state.entries[0].is_dir);
    }

    #[test]
    fn cursor_and_selection() {
        let dir = temp_dir_with_files();
        let mut app = App::boot(dir);
        app.pump_one();
        assert_eq!(app.state.cursor, 0);
        app.dispatch_for_test(Intent::CursorDown);
        assert_eq!(app.state.cursor, 1);
        app.dispatch_for_test(Intent::ToggleSelect);
        assert_eq!(app.state.selection.len(), 1);
        app.dispatch_for_test(Intent::ClearSelection);
        assert!(app.state.selection.is_empty());
    }

    #[test]
    fn cursor_right_on_file_loads_content() {
        let dir = temp_dir_with_files();
        let mut app = App::boot(dir);
        app.pump_one();
        // Move to a file (skip the directory at index 0).
        app.dispatch_for_test(Intent::CursorDown);
        app.dispatch_for_test(Intent::CursorRight);
        assert!(app.state.function.visible);
        assert_eq!(app.state.function.sublayout, SubLayout::Content);
        app.pump_one(); // content read → handed to the content provider's stash
        assert!(app.function_content_rows().is_some());
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

    #[test]
    fn content_auto_update_follows_file_cursor() {
        use crate::core::{Key, KeyCode, Mods};
        let dir = temp_dir_with_files();
        let mut app = App::boot(dir);
        app.pump_one();
        app.options.content_auto_update = true;
        app.state.function.visible = true;
        app.state.focused_panel = "file".into();
        // Move the file cursor; the on_cursor_move hook reads the file's content.
        app.handle_key(Key { code: KeyCode::Char('s'), mods: Mods::NONE });
        assert_eq!(app.state.cursor, 1);
        app.pump_one(); // the auto-update content read
        assert!(app.function_content_rows().is_some());
    }

    #[test]
    fn resolver_table_sourced_from_scheme() {
        let session = crate::script::SchemeSession::spawn();
        session.wait_ready(Duration::from_secs(600));
        let entries = scheme_resolvers(&session);
        let copy = entries.iter().find(|e| e.key == "copy").expect("copy entry");
        assert!(!copy.destructive);
        let del = entries.iter().find(|e| e.key == "delete").expect("delete entry");
        assert!(del.destructive, "delete is destructive per the Scheme config");
        let open = entries.iter().find(|e| e.key == "open").expect("open entry");
        assert_eq!(open.candidates.len(), 2, "open has macos + linux candidates");
    }

    #[test]
    fn commands_sourced_from_scheme() {
        let session = crate::script::SchemeSession::spawn();
        session.wait_ready(Duration::from_secs(600));
        let cmds = scheme_commands(&session);
        assert!(cmds.iter().any(|c| c.name == "quit"));
        let mkdir = cmds.iter().find(|c| c.name == "mkdir").expect("mkdir command");
        assert!(matches!(mkdir.intent, Intent::PushMode(_)), "mkdir opens a path input");
    }

    #[test]
    fn keymap_sourced_from_scheme() {
        use crate::core::{Key, KeyCode, Mods};
        let session = crate::script::SchemeSession::spawn();
        session.wait_ready(Duration::from_secs(600));
        let binds = scheme_keymaps(&session);
        assert!(binds.len() > 30, "core keymap has many bindings, got {}", binds.len());

        let mut km = KeymapRegistry::new();
        km.extend(binds);
        let conds = Conditions::default();
        let state = AppState::new(std::env::temp_dir());
        // file mode, file panel: 'w' resolves to CursorUp via the Scheme keymap.
        let up = km.resolve(&conds, &state, "file", Key { code: KeyCode::Char('w'), mods: Mods::NONE });
        assert_eq!(up, Some(Intent::CursorUp));
        // Ctrl-a resolves to SelectAll (modifier parsing works).
        let all = km.resolve(&conds, &state, "file", Key { code: KeyCode::Char('a'), mods: Mods::CTRL });
        assert_eq!(all, Some(Intent::SelectAll));
        // Delete fires on both Backspace and Delete.
        for code in [KeyCode::Backspace, KeyCode::Delete] {
            let got = km.resolve(&conds, &state, "file", Key { code, mods: Mods::NONE });
            assert_eq!(got, Some(Intent::DeleteSelected), "{code:?} → delete");
        }
    }

    #[test]
    fn user_keybindings_parsed_at_user_layer() {
        use crate::core::{Key, KeyCode, Mods};
        let session = crate::script::SchemeSession::spawn();
        session.wait_ready(Duration::from_secs(600));
        // Commented lines are ignored; uncommented (bind …) forms become User
        // overrides. Includes a `:when` and a #f (no-panel) scope.
        let printed = session
            .eval(
                "(quote (\
                   ; (bind file file \"w\" cursor-up)\n\
                   (bind file file \"z\" cursor-bottom)\
                   (bind file #f \"Q\" quit)))",
            )
            .unwrap();
        let datum = crate::script::sexpr::parse(&printed).unwrap();
        let binds = parse_user_binds(&datum).unwrap();
        assert_eq!(binds.len(), 2, "the commented line is skipped");
        assert!(binds.iter().all(|b| b.layer == Layer::User));
        let z = &binds[0];
        assert_eq!(z.key, Key { code: KeyCode::Char('z'), mods: Mods::NONE });
        assert_eq!(z.intent, Intent::CursorBottom);
        assert_eq!(binds[1].panel, None);
    }

    #[test]
    fn keybindings_template_is_commented() {
        let t = keybindings_template();
        assert!(t.contains("; (bind file file \"w\" cursor-up)"));
        assert!(t.contains("; (bind select #f \"Enter\" confirm)"));
        // Every binding line is commented out.
        assert!(t.lines().filter(|l| l.contains("(bind ")).all(|l| l.trim_start().starts_with(';')));
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

    #[test]
    fn eval_scheme_runs_through_the_pipeline() {
        let dir = temp_dir_with_files();
        let mut app = App::boot(dir);
        app.scheme.wait_ready(Duration::from_secs(600));
        app.pump_one(); // initial ReadDir
        // Issue an EvalScheme plan; the resident REPL computes it and the
        // result returns as an AsyncResult the reducer turns into a notice.
        let generation = app.state.mode_generation();
        let cwd = app.cwd();
        app.tasks
            .spawn(Plan::EvalScheme { expr: "(+ 40 2)".into() }, generation, cwd);
        app.pump_one(); // Scheme result
        let shown = app.ui.live_notice().map(|n| n.text.clone()).unwrap_or_default();
        assert!(shown.contains("42"), "scheme result surfaced: {shown:?}");
    }

    #[test]
    fn cursor_left_lands_on_origin_directory() {
        let dir = temp_dir_with_files();
        let mut app = App::boot(dir.clone());
        app.pump_one();
        // Enter subdir (index 0, the only directory).
        assert!(app.state.entries[0].is_dir);
        app.dispatch_for_test(Intent::CursorRight);
        app.pump_one(); // listing of subdir
        assert_eq!(app.state.cwd, dir.join("subdir"));
        // Go back to the parent; cursor should land on "subdir".
        app.dispatch_for_test(Intent::CursorLeft);
        app.pump_one(); // listing of parent
        assert_eq!(app.state.cwd, dir);
        assert_eq!(app.state.current_entry().unwrap().path, dir.join("subdir"));
    }

    #[test]
    fn content_title_is_the_file_name() {
        let dir = temp_dir_with_files();
        let mut app = App::boot(dir.clone());
        app.pump_one();
        app.dispatch_for_test(Intent::CursorDown); // alpha.txt (subdir is index 0)
        app.dispatch_for_test(Intent::CursorRight);
        app.pump_one(); // content read → the content provider stashes it + the path
        assert_eq!(app.function_title().as_deref(), Some("alpha.txt"), "title is the file name");
    }

    #[test]
    fn preview_extension_owns_function_panel_search_keys() {
        use crate::core::{Key, KeyCode, Mods};
        // The preview extension registers its function-panel bindings (search,
        // horizontal scroll) via scheme_source at the Extension layer; they
        // resolve through the live keymap and override the core bindings by
        // layer priority while a search is active.
        let dir = temp_dir_with_files();
        let mut app = App::boot(dir);
        app.pump_one();
        app.state.function.visible = true;
        app.state.function.sublayout = SubLayout::Content;
        app.state.focused_panel = "function".into();
        let ch = |c| Key { code: KeyCode::Char(c), mods: Mods::NONE };
        let enter = Key { code: KeyCode::Enter, mods: Mods::NONE };
        let resolve = |app: &App, k| app.keymap.resolve(&app.conditions, &app.state, "function", k);

        // Extension-owned bindings: `/` opens search, h/l scroll horizontally.
        assert_eq!(resolve(&app, ch('/')), Some(Intent::FunctionSearchStart));
        assert_eq!(resolve(&app, ch('h')), Some(Intent::FuncLeft));
        // n toggles line numbers (core) until a search has matches, then the
        // extension's n (Extension layer) wins and walks matches.
        assert_eq!(resolve(&app, ch('n')), Some(Intent::ToggleLineNumbers));
        app.state.function.search.matches = vec![(0, 0, 1)];
        assert_eq!(resolve(&app, ch('n')), Some(Intent::FunctionSearchNext));
        assert_eq!(resolve(&app, ch('p')), Some(Intent::FunctionSearchPrev));
        // Enter opens (core) normally; submits (extension) while input is open.
        assert_eq!(resolve(&app, enter), Some(Intent::Open { path: std::path::PathBuf::new() }));
        app.state.function.search.input_active = true;
        assert_eq!(resolve(&app, enter), Some(Intent::FunctionSearchSubmit));
    }

    #[test]
    fn command_palette_filters_and_runs() {
        let dir = temp_dir_with_files();
        let mut app = App::boot(dir);
        app.pump_one();
        app.dispatch_for_test(Intent::PushMode(Box::new(Mode::Select(
            crate::core::SelectSpec::command_palette(Vec::new()),
        ))));
        // Candidates filled from the registry.
        let n = app.state.select.as_ref().unwrap().results.len();
        assert!(n > 5, "palette has the default commands");
    }
}
