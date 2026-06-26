//! ArchiveExtension (bundled): self-contained pack and unpack. Its glue declares
//! entry bindings into File mode (pack when a selection exists, unpack when the
//! cursor entry is an archive) and matching palette commands. The flow uses
//! Select confirmed through Emit; the final intent carries the reserved resolver
//! key and executes as a plan through the chain. No custom mode or panel.

use crate::core::{
    AppState, ExtensionIntent, ExtensionValue, FunctionHint, Intent, Key, KeyCode, Mode, Mods,
    SelectSpec, KEY_ITEM, KEY_RESOLVER,
};
use crate::extension::Extension;
use crate::script::condition::{Cond, Conditions};
use crate::script::keymap::{Binding, Layer};
use crate::services::command::CommandDef;
use crate::services::resolver::{ArgEl, Candidate, Kind, ResolverEntry, Slot};

const ID: &str = "archive";

pub struct ArchiveExtension;

fn ext_intent(name: &str, data: ExtensionValue) -> Intent {
    Intent::Extension(ExtensionIntent { extension: ID.into(), intent: name.into(), data })
}

fn is_archive_name(name: &str) -> bool {
    let n = name.to_lowercase();
    [".tar", ".tar.gz", ".tgz", ".tar.bz2", ".tbz", ".tar.xz", ".txz", ".gz", ".bz2", ".xz"]
        .iter()
        .any(|s| n.ends_with(s))
}

impl Extension for ArchiveExtension {
    fn id(&self) -> &str {
        ID
    }

    fn register_conditions(&self, c: &mut Conditions) {
        c.register(
            "cursor-is-archive",
            Box::new(|s| s.current_entry().map(|e| is_archive_name(&e.name)).unwrap_or(false)),
        );
    }

    fn commands(&self) -> Vec<CommandDef> {
        vec![
            CommandDef {
                name: "pack".into(),
                description: "pack the selection into an archive".into(),
                intent: ext_intent("pack", ExtensionValue::Nil),
            },
            CommandDef {
                name: "unpack".into(),
                description: "extract the cursor archive here".into(),
                intent: ext_intent("unpack", ExtensionValue::Nil),
            },
        ]
    }

    fn keymaps(&self) -> Vec<Binding> {
        let bind = |key: Key, intent: Intent, when: Cond| Binding {
            mode: "file".into(),
            panel: Some("file".into()),
            key,
            intent,
            when,
            layer: Layer::Extension,
        };
        vec![
            bind(
                Key { code: KeyCode::Char('p'), mods: Mods::NONE },
                ext_intent("pack", ExtensionValue::Nil),
                Cond::pred("has-selection"),
            ),
            bind(
                Key { code: KeyCode::Char('P'), mods: Mods::NONE },
                ext_intent("unpack", ExtensionValue::Nil),
                Cond::pred("cursor-is-archive"),
            ),
        ]
    }

    fn resolver_entries(&self) -> Vec<ResolverEntry> {
        let lit = |s: &str| ArgEl::Lit(s.to_string());
        vec![
            ResolverEntry {
                key: "archive:pack".into(),
                destructive: false,
                options: vec![],
                candidates: vec![Candidate {
                    kind: Kind::Native,
                    argv: vec![lit("tar"), lit("-czf"), ArgEl::Ph(Slot::Dst), ArgEl::Ph(Slot::Paths)],
                }],
            },
            ResolverEntry {
                key: "archive:unpack".into(),
                destructive: false,
                options: vec![],
                candidates: vec![Candidate {
                    kind: Kind::Native,
                    argv: vec![lit("tar"), lit("-xf"), ArgEl::Ph(Slot::Path)],
                }],
            },
        ]
    }

    fn handle_intent(&self, intent: &ExtensionIntent, state: &AppState) -> Vec<Intent> {
        match intent.intent.as_str() {
            // Step 1: ask for a destination archive name, confirmed via Emit.
            "pack" => {
                let template = ExtensionIntent {
                    extension: ID.into(),
                    intent: "pack-run".into(),
                    data: ExtensionValue::Nil,
                };
                let spec = SelectSpec::emit_path_input(
                    "archive:pack",
                    template,
                    Some("archive.tar.gz".into()),
                    Some(FunctionHint::CommandSummary),
                );
                vec![Intent::PushMode(Box::new(Mode::Select(spec)))]
            }
            // Step 2: the Emit confirm filled `item` with the destination text.
            "pack-run" => {
                let dest = intent.data.get(KEY_ITEM).and_then(|v| v.as_str()).unwrap_or("archive.tar.gz");
                let paths: Vec<ExtensionValue> =
                    state.targets().into_iter().map(ExtensionValue::Path).collect();
                let resolver = ExtensionValue::map([
                    ("op".into(), ExtensionValue::String("archive:pack".into())),
                    ("dst".into(), ExtensionValue::String(dest.to_string())),
                    ("paths".into(), ExtensionValue::List(paths)),
                    ("label".into(), ExtensionValue::String(format!("pack → {dest}"))),
                ]);
                vec![Intent::PopMode, ext_intent("pack", ExtensionValue::Nil.with(KEY_RESOLVER, resolver))]
            }
            // Unpack: build the resolver plan directly from the cursor archive.
            "unpack" => {
                let Some(entry) = state.current_entry() else { return vec![] };
                let resolver = ExtensionValue::map([
                    ("op".into(), ExtensionValue::String("archive:unpack".into())),
                    ("path".into(), ExtensionValue::Path(entry.path.clone())),
                    ("label".into(), ExtensionValue::String(format!("unpack {}", entry.name))),
                ]);
                vec![ext_intent("unpack", ExtensionValue::Nil.with(KEY_RESOLVER, resolver))]
            }
            _ => vec![],
        }
    }
}
