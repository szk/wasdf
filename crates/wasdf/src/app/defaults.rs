//! The embedded defaults — core keymap, palette commands, resolver entries,
//! option declarations, and each extension's declarations. They are data, a
//! `(quote …)` literal per file, so they are read with the s-expression reader:
//! no session is needed, and there is no second copy of them in Rust. What the
//! user writes (`config.scm`, `custom.scm`, the REPL) is what the session
//! evaluates.

use crate::core::OptionDef;
use crate::extension::Extension;
use crate::script::config;
use crate::script::keymap::{Binding, Layer};
use crate::script::sexpr::{self, Datum};
use crate::services::command::CommandDef;
use crate::services::resolver::ResolverEntry;

pub const KEYMAP_SCHEME: &str = include_str!("keymap.scm");
const COMMAND_SCHEME: &str = include_str!("command.scm");
const RESOLVER_SCHEME: &str = include_str!("resolver.scm");
pub const CORE_OPTIONS_SCHEME: &str = include_str!("options.scm");
pub const LAYOUT_SCHEME: &str = include_str!("layout.scm");

/// Read `src` and parse it; on failure log and yield nothing (disable, notify,
/// continue). The core sources are covered by tests, so for them this only
/// ever fails in development.
fn read<T>(src: &str, label: &str, parse: impl FnOnce(&Datum) -> Result<Vec<T>, String>) -> Vec<T> {
    sexpr::parse_quoted(src).and_then(|d| parse(&d)).unwrap_or_else(|e| {
        crate::services::messages::log(format!("{label} failed ({e}); ignoring"), true);
        Vec::new()
    })
}

pub fn keymap() -> Vec<Binding> {
    read(KEYMAP_SCHEME, "core keymap", |d| config::parse_keymap_config(d, Layer::Core))
}

pub fn commands() -> Vec<CommandDef> {
    read(COMMAND_SCHEME, "core commands", config::parse_command_config)
}

pub fn resolvers() -> Vec<ResolverEntry> {
    read(RESOLVER_SCHEME, "core resolver", config::parse_resolver_config)
}

/// The core layouts, `(id, tree)`.
pub fn layouts() -> Vec<(String, crate::script::layout::Node)> {
    read(LAYOUT_SCHEME, "core layouts", |d| {
        d.as_list()
            .ok_or("not a list of layouts")?
            .iter()
            .map(|entry| match entry.as_list() {
                Some([id, tree]) => {
                    let id = id.text().ok_or("a layout id must be a symbol")?;
                    Ok((id.to_string(), crate::script::layout::parse(id, tree)?))
                }
                _ => Err("a layout entry is (id tree)".to_string()),
            })
            .collect()
    })
}

pub fn core_options() -> Vec<OptionDef> {
    read(CORE_OPTIONS_SCHEME, "core options", config::parse_option_config)
}

/// An extension's option declarations, each tagged with its owner so settings
/// groups them under `@/settings/Extensions/<id>`.
pub fn extension_options(ext: &dyn Extension) -> Vec<OptionDef> {
    let Some(src) = ext.option_scheme() else { return Vec::new() };
    let mut defs = read(&src, &format!("extension '{}' options", ext.id()), config::parse_option_config);
    for d in &mut defs {
        d.owner = Some(ext.id().to_string());
    }
    defs
}

/// An extension's declarative keymap, as Extension-layer bindings.
pub fn extension_keymap(ext: &dyn Extension) -> Vec<Binding> {
    let Some(src) = ext.scheme_source() else { return Vec::new() };
    read(&src, &format!("extension '{}' keymap", ext.id()), |d| config::parse_keymap_config(d, Layer::Extension))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_core_source_reads() {
        assert!(keymap().len() > 30);
        assert!(commands().iter().any(|c| c.name == "copy"), "CmdOp's commands are core commands");
        assert!(resolvers().iter().any(|r| r.key == "delete" && r.destructive));
        assert!(core_options().iter().any(|o| o.key == "cmdop.runner"));
        let ids: Vec<String> = layouts().into_iter().map(|(id, _)| id).collect();
        assert_eq!(ids, ["file", "select"]);
    }
}
