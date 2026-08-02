//! CmdOp panel reducer logic: activating the copy / move / rename / mkdir / touch
//! command-builder panel, navigating its listing, toggling options and marks,
//! confirming into a `RunResolver`, and editing its inline filter / prompt inputs.
//! Pure and synchronous like the rest of the reducer; driven from `apply_intent`.

use std::path::PathBuf;

use crate::core::event::{Plan, ResolverRequest};
use crate::core::intent::{Intent, Key, KeyCode};
use crate::core::mode::{CmdToken, SubLayout};
use crate::core::state::{AppState, CmdOp, CmdOpOption, CmdOpState, CmdPromptInput};

use super::{CommandLookup, Effects, Notice};

/// Space inside a CmdOp panel: toggle the item under the unified cursor. On an
/// option row it flips the checkbox; on a listing row it marks/unmarks that entry
/// in the shared file selection (the same path set the file panel and file-search
/// mark into).
pub(super) fn cmdop_space_toggle(state: &mut AppState) {
    let Some(c) = &mut state.cmdop else { return };
    if c.cursor < c.options.len() {
        c.options[c.cursor].checked = !c.options[c.cursor].checked;
        return;
    }
    let Some(p) = c.entries.get(c.cursor - c.options.len()).map(|e| e.path.clone()) else {
        return;
    };
    if !state.selection.remove(&p) {
        state.selection.insert(p);
    }
}

/// Move the CmdOp unified cursor down by one row (clamped to the last row).
pub(super) fn cmd_cursor_down(c: &mut CmdOpState) {
    let total = c.options.len() + c.entries.len();
    if c.cursor + 1 < total {
        c.cursor += 1;
    }
}

/// Move the CmdOp unified cursor up by one row (clamped to the first row).
pub(super) fn cmd_cursor_up(c: &mut CmdOpState) {
    if c.cursor > 0 {
        c.cursor -= 1;
    }
}

/// Descend into the directory under the cursor; on a file, confirm with it.
/// A no-op while the cursor sits on an option row.
pub(super) fn cmd_descend(state: &mut AppState, ctx: &dyn CommandLookup) -> Effects {
    let Some(c) = &mut state.cmdop else { return Effects::none() };
    if c.cursor < c.options.len() {
        return Effects::none();
    }
    let idx = c.cursor - c.options.len();
    let Some(entry) = c.entries.get(idx) else { return Effects::none() };
    if !entry.is_dir {
        return confirm_cmdop(state, ctx);
    }
    let new_cwd = entry.path.clone();
    let c = state.cmdop.as_mut().unwrap();
    c.list_cwd = new_cwd.clone();
    c.entries = vec![];
    c.cursor = c.options.len();
    c.scroll = 0;
    c.filter = None;
    Effects::plan(Plan::ReadDirCmd { path: new_cwd, show_hidden: state.show_hidden })
}

/// Ascend to the parent of the current listing directory (no-op at the root).
pub(super) fn cmd_parent(state: &mut AppState) -> Effects {
    let Some(c) = &mut state.cmdop else { return Effects::none() };
    let Some(parent) = c.list_cwd.parent().map(|p| p.to_path_buf()) else {
        return Effects::none();
    };
    c.list_cwd = parent.clone();
    c.entries = vec![];
    c.cursor = c.options.len();
    c.scroll = 0;
    c.filter = None;
    Effects::plan(Plan::ReadDirCmd { path: parent, show_hidden: state.show_hidden })
}

/// Open the CmdOp panel for the given operation.
pub(super) fn activate_cmdop(state: &mut AppState, op: CmdOp, ctx: &dyn CommandLookup) -> Effects {
    let srcs = state.targets();
    if srcs.is_empty() && matches!(op, CmdOp::Copy | CmdOp::Move | CmdOp::Rename) {
        return Effects::notice(Notice::error("nothing selected"));
    }
    let key = op.resolver_key();
    let options: Vec<CmdOpOption> = ctx.resolver_options(key)
        .into_iter()
        .map(|(token, label)| CmdOpOption { token, label, checked: false })
        .collect();
    let command_line: Vec<CmdToken> = ctx.resolver_command(key);

    let prompt = match op {
        CmdOp::Rename => {
            let name = srcs.first()
                .and_then(|p| p.file_name())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let caret = name.len();
            Some(CmdPromptInput { text: name, caret })
        }
        CmdOp::Mkdir | CmdOp::Touch => Some(CmdPromptInput { text: String::new(), caret: 0 }),
        _ => None,
    };

    let n_opts = options.len();
    let list_cwd = state.cwd.clone();
    state.cmdop = Some(CmdOpState {
        op,
        srcs,
        command_line,
        options,
        entries: vec![],
        list_cwd: list_cwd.clone(),
        cursor: n_opts,
        scroll: 0,
        filter: None,
        prompt,
    });
    state.function.sublayout = SubLayout::Command;
    state.function.visible = true;

    if !matches!(op, CmdOp::Copy | CmdOp::Move) {
        state.focused_panel = crate::core::PANEL_FUNCTION.into();
    }

    Effects::plan(Plan::ReadDirCmd { path: list_cwd, show_hidden: state.show_hidden })
}

/// Confirm the current CmdOp selection and issue a resolver request.
pub(super) fn confirm_cmdop(state: &mut AppState, ctx: &dyn CommandLookup) -> Effects {
    let Some(c) = state.cmdop.take() else { return Effects::none() };
    state.function.sublayout = SubLayout::Content;

    let dest: PathBuf = if let Some(p) = &c.prompt {
        PathBuf::from(&p.text)
    } else if c.cursor >= c.options.len() {
        c.entries.get(c.cursor - c.options.len())
            .map(|e| e.path.clone())
            .unwrap_or(c.list_cwd.clone())
    } else {
        c.list_cwd.clone()
    };

    let opts: Vec<String> = c.options.iter()
        .filter(|o| o.checked).map(|o| o.token.clone()).collect();
    let dest_str = dest.to_string_lossy().into_owned();

    let req = match c.op {
        CmdOp::Copy | CmdOp::Move => ResolverRequest {
            op: c.op.resolver_key().into(),
            src: None,
            dst: Some(dest_str),
            path: None,
            paths: c.srcs,
            opts,
            label: c.op.resolver_key().into(),
        },
        CmdOp::Rename => ResolverRequest {
            op: "rename".into(),
            src: c.srcs.into_iter().next(),
            dst: Some(dest_str),
            path: None,
            paths: vec![],
            opts,
            label: "rename".into(),
        },
        CmdOp::Mkdir | CmdOp::Touch => ResolverRequest {
            op: c.op.resolver_key().into(),
            src: None,
            dst: None,
            path: Some(dest_str),
            paths: vec![],
            opts,
            label: c.op.resolver_key().into(),
        },
    };

    let _ = ctx; // resolver lookup not needed here — RunResolver goes through the kernel
    Effects::emit(vec![Intent::RunResolver(req)])
}

/// Handle a raw key when the CmdOp panel has an active text input (filter or prompt).
pub(super) fn handle_cmdop_raw_key(state: &mut AppState, key: Key, ctx: &dyn CommandLookup) -> Effects {
    let Some(c) = &mut state.cmdop else { return Effects::none() };

    // Filter input has priority when active.
    if let Some(f) = &mut c.filter {
        if f.active {
            return match key.code {
                KeyCode::Esc => { f.active = false; Effects::none() }
                KeyCode::Enter => { f.active = false; Effects::none() }
                KeyCode::Backspace => {
                    if f.caret > 0 {
                        f.query.remove(f.caret - 1);
                        f.caret -= 1;
                    }
                    Effects::none()
                }
                _ => {
                    if let Some(ch) = key.printable_char() {
                        f.query.insert(f.caret, ch);
                        f.caret += 1;
                    }
                    Effects::none()
                }
            };
        }
    }

    // Prompt input.
    if c.prompt.is_some() {
        return match key.code {
            KeyCode::Esc => {
                state.cmdop.as_mut().unwrap().prompt = None;
                Effects::none()
            }
            KeyCode::Enter => confirm_cmdop(state, ctx),
            KeyCode::Backspace => {
                let p = &mut state.cmdop.as_mut().unwrap().prompt.as_mut().unwrap();
                if p.caret > 0 {
                    p.text.remove(p.caret - 1);
                    p.caret -= 1;
                }
                Effects::none()
            }
            KeyCode::Left => {
                let p = &mut state.cmdop.as_mut().unwrap().prompt.as_mut().unwrap();
                if p.caret > 0 { p.caret -= 1; }
                Effects::none()
            }
            KeyCode::Right => {
                let p = &mut state.cmdop.as_mut().unwrap().prompt.as_mut().unwrap();
                if p.caret < p.text.len() { p.caret += 1; }
                Effects::none()
            }
            _ => {
                if let Some(ch) = key.printable_char() {
                    let p = &mut state.cmdop.as_mut().unwrap().prompt.as_mut().unwrap();
                    p.text.insert(p.caret, ch);
                    p.caret += 1;
                }
                Effects::none()
            }
        };
    }

    Effects::none()
}
