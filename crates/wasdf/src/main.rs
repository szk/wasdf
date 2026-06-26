//! wasdf — a WASD-keyed TUI filer built around a single intent pipeline:
//! key → Intent → plan → kernel async → AsyncResult → reducer → render.

// The intent catalog is a closed set, ExtensionValue is structurally complete,
// the keymap is layered (Core/Extension/User), and the `:when` grammar is whole
// — all per the doc/ specification. Some of that surface is not yet exercised by
// the MVP's embedded defaults (e.g. the User config layer, the EvalScheme plan,
// uutils resolver candidates). These are deliberate spec surface, not dead ends.
#![allow(dead_code)]

mod app;
mod core;
mod exec;
mod extension;
mod fs;
mod runtime;
mod script;
mod services;
mod ui;

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();

    // Headless self-test of the optional-extension loader: load a directory of
    // dynamic libraries and print what each contributes, then exit.
    if let Some(pos) = args.iter().position(|a| a == "--list-extensions") {
        let dir = args.get(pos + 1).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        let state = core::AppState::new(std::env::temp_dir());
        for ext in extension::loader::load_optional(&dir) {
            let cmds: Vec<String> = ext.commands().iter().map(|c| c.name.clone()).collect();
            let res: Vec<String> = ext.resolver_entries().iter().map(|r| r.key.clone()).collect();
            println!("extension {} commands={cmds:?} resolvers={res:?}", ext.id());
            // Probe handle_intent across the ABI with a `greet` intent.
            let probe = core::ExtensionIntent {
                extension: ext.id().to_string(),
                intent: "greet".into(),
                data: core::ExtensionValue::Nil,
            };
            let out = ext.handle_intent(&probe, &state);
            println!("  handle_intent(greet) -> {out:?}");
        }
        return ExitCode::SUCCESS;
    }

    let cwd = args
        .get(1)
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));

    match app::run(cwd) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("wasdf: {e}");
            ExitCode::FAILURE
        }
    }
}
