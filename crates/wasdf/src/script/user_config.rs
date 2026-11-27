//! The user's configuration files around the settings store: the `config.scm`
//! template, and reading an old-format `config.scm` (data forms and a managed
//! block) until it is migrated.

use crate::core::{OptionStore, OptionValue};
use crate::script::sexpr::{self, Datum};

/// The first line of the managed block old versions wrote into `config.scm`.
const LEGACY_BLOCK_BEGIN: &str = ";; >>> wasdf settings (managed — edited via the SETTINGS panel; do not hand-edit)";
const LEGACY_BLOCK_END: &str = ";; <<< wasdf settings";

/// Whether `text` is an old-format `config.scm`: it has the managed block, or it
/// reads as plain data whose top-level forms include `bind` / `set-option`.
pub fn is_legacy(text: &str) -> bool {
    if text.lines().any(|l| l.trim() == LEGACY_BLOCK_BEGIN) {
        return true;
    }
    sexpr::parse_all(text)
        .map(|forms| forms.iter().any(|f| matches!(head(f), Some("bind" | "set-option"))))
        .unwrap_or(false)
}

/// An old-format file as two store programs: the forms outside the managed
/// block (the user's own — the `config` layer) and those inside it (UI edits —
/// the `custom` layer). `bind` and `set-option` are converted to store calls;
/// any other form passes through unchanged — an extension's prelude may define
/// it (theme's `define-theme`, for instance), and if none does, it is reported
/// as an unbound procedure like any other failing form.
pub struct LegacyPrograms {
    pub config: String,
    pub custom: String,
}

pub fn legacy_programs(text: &str, options: &OptionStore) -> LegacyPrograms {
    let (mut outside, mut inside) = (String::new(), String::new());
    let mut in_block = false;
    for line in text.lines() {
        match line.trim() {
            l if l == LEGACY_BLOCK_BEGIN => in_block = true,
            l if l == LEGACY_BLOCK_END => in_block = false,
            _ => {
                let part = if in_block { &mut inside } else { &mut outside };
                part.push_str(line);
                part.push('\n');
            }
        }
    }
    LegacyPrograms { config: convert(&outside, options), custom: convert(&inside, options) }
}

fn convert(text: &str, options: &OptionStore) -> String {
    let Ok(forms) = sexpr::parse_all(text) else { return String::new() };
    forms
        .iter()
        .filter_map(|f| {
            let items = f.as_list()?;
            match head(f)? {
                "bind" => {
                    let [_, mode, panel, key, intent, rest @ ..] = items else { return None };
                    let mut call = format!("(bind! {} {} {} {}", quoted(mode), quoted(panel), sexpr::render(key), quoted(intent));
                    for r in rest {
                        call.push(' ');
                        call.push_str(&quoted(r));
                    }
                    Some(call + ")")
                }
                "set-option" => {
                    let [_, key, value] = items else { return None };
                    let (key, value) = (key.text()?, value.text()?);
                    let literal = options
                        .def(key)
                        .and_then(|d| OptionValue::parse_flat(&d.kind, value))
                        .map(|v| v.to_literal())
                        .unwrap_or_else(|| format!("{value:?}"));
                    Some(format!("(opt-set! '{key} {literal})"))
                }
                _ => Some(sexpr::render(f)),
            }
        })
        .map(|call| call + "\n")
        .collect()
}

fn head(d: &Datum) -> Option<&str> {
    d.as_list()?.first()?.as_sym()
}

/// A datum as a store-call argument: symbols and lists are quoted.
fn quoted(d: &Datum) -> String {
    match d {
        Datum::Sym(_) | Datum::List(_) => format!("'{}", sexpr::render(d)),
        _ => sexpr::render(d),
    }
}

/// Migrate an old-format `config.scm` in `dir`, once: the old file is kept as
/// `config.scm.old` (or `config.scm.old.N` when that exists), and the new
/// `config.scm` is the template followed by the old file's own forms as store
/// calls. The managed block's forms are not written here — boot has already
/// loaded them into the `custom` layer, so the Persister writes them to
/// `custom.scm`. Returns the name the old file was kept under.
pub fn migrate(dir: &std::path::Path, old_text: &str, options: &OptionStore, keymap_source: &str) -> std::io::Result<String> {
    let kept = (1..)
        .map(|n| if n == 1 { "config.scm.old".to_string() } else { format!("config.scm.old.{n}") })
        .find(|name| !dir.join(name).exists())
        .expect("an unused name");
    std::fs::rename(dir.join("config.scm"), dir.join(&kept))?;
    let own = legacy_programs(old_text, options).config;
    let mut text = config_template(options, keymap_source);
    if !own.is_empty() {
        text.push_str(&format!("\n;; Migrated from {kept}: the settings you had written there.\n"));
        text.push_str(&own);
    }
    std::fs::write(dir.join("config.scm"), text)?;
    Ok(kept)
}

/// The `config.scm` written when none exists: every option and every default
/// binding as a commented-out store call, plus commented examples of
/// environment-dependent configuration. `keymap_source` is the embedded
/// keymap, `(quote ((mode panel (binding…)) …))`.
pub fn config_template(options: &OptionStore, keymap_source: &str) -> String {
    let mut s = String::from(
        ";; config.scm — your wasdf configuration: a Scheme program evaluated at\n\
         ;; startup, one top-level form at a time (doc/SCHEME.md). Uncomment and\n\
         ;; edit a line to override a default. Settings you change inside wasdf go\n\
         ;; to custom.scm and override this file.\n\
         ;;\n\
         ;; The environment is available, so one file can serve several machines:\n\
         ;; (when (equal? (os) \"macos\") (opt-set! 'border-style \"rounded\"))\n\
         ;; (when (wsl?) (bind! 'file 'file \"o\" '(navigate-to \"/mnt/c\")))\n\
         ;; (when (equal? (hostname) \"work-laptop\") (opt-set! 'top.monitor-visible #f))\n\n\
         ;; Options\n",
    );
    for (cat, defs) in options.categories() {
        s.push_str(&format!(";; {cat}\n"));
        for def in defs {
            let hint = def.kind.hint();
            let call = format!(";; (opt-set! '{} {})", def.key, def.default.to_literal());
            if hint.is_empty() {
                s.push_str(&format!("{call}\n"));
            } else {
                s.push_str(&format!("{call}  ; {hint}\n"));
            }
        }
    }
    s.push_str("\n;; Keybindings: (bind! 'mode 'panel-or-#f \"key\" 'intent ['when] [\"label\"])\n");
    let groups = sexpr::parse_quoted(keymap_source).ok().and_then(|d| d.as_list().map(<[Datum]>::to_vec)).unwrap_or_default();
    for group in &groups {
        let Some([mode, panel, Datum::List(binds)]) = group.as_list() else { continue };
        for b in binds {
            let Some([key, intent, rest @ ..]) = b.as_list() else { continue };
            let mut call = format!(";; (bind! {} {} {} {}", quoted(mode), quoted(panel), sexpr::render(key), quoted(intent));
            for r in rest {
                call.push(' ');
                call.push_str(&quoted(r));
            }
            s.push_str(&call);
            s.push_str(")\n");
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{OptionDef, OptionKind};

    fn options() -> OptionStore {
        let def = |key: &str, kind: OptionKind, default: OptionValue| OptionDef {
            key: key.into(),
            category: "Display".into(),
            label: key.into(),
            kind,
            default,
            owner: None,
        };
        OptionStore::new(vec![
            def("border-style", OptionKind::Choice(vec!["single".into(), "rounded".into()]), OptionValue::Text("single".into())),
            def("auto", OptionKind::Bool, OptionValue::Bool(false)),
            def("exts", OptionKind::text_list(), OptionValue::List(vec![OptionValue::Text("rs".into())])),
        ])
    }

    #[test]
    fn migration_keeps_the_old_file_and_writes_a_program() {
        let dir = std::env::temp_dir().join(format!("wasdf-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let old = format!("(bind file #f \"Q\" quit)\n{LEGACY_BLOCK_BEGIN}\n(set-option \"auto\" \"#t\")\n{LEGACY_BLOCK_END}\n");
        std::fs::write(dir.join("config.scm"), &old).unwrap();
        std::fs::write(dir.join("config.scm.old"), "an earlier backup").unwrap();
        let keymap = r#"(quote ((file file (("w" cursor-up "cursor")))))"#;
        let kept = migrate(&dir, &old, &options(), keymap).unwrap();
        assert_eq!(kept, "config.scm.old.2", "an existing backup is never overwritten");
        assert_eq!(std::fs::read_to_string(dir.join(&kept)).unwrap(), old);
        let new = std::fs::read_to_string(dir.join("config.scm")).unwrap();
        assert!(!is_legacy(&new), "the new file is a program");
        assert!(new.contains("(bind! 'file #f \"Q\" 'quit)"), "the user's own forms carry over");
        assert!(
            !new.lines().any(|l| l.starts_with("(opt-set! 'auto")),
            "the managed block goes to custom.scm, not here"
        );
    }
}
