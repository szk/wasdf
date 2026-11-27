//! ReplExtension: an active function-panel form — the evaluation history over
//! an input line. Each submitted line is an `Eval`; its printed value (or
//! error) lands in `accept_content`. `@/repl` lists the past expressions, and
//! activating one reopens the REPL with it in the input line.

use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::core::{AppState, Entry, ExtensionIntent, ExtensionValue, Intent, Content, Role};
use crate::extension::{Extension, FunctionRenderCtx};
use crate::services::command::CommandDef;
use crate::vfs::{VfsProvider, vfs_root};

const ID: &str = "repl";
const PURPOSE: &str = "expr";
const PROMPT: &str = "> ";

struct Evaluation {
    expr: String,
    /// `None` while the session is still evaluating it.
    value: Option<Result<String, String>>,
}

/// Shared with the `@/repl` provider, which lists it off the main thread.
type History = Arc<Mutex<Vec<Evaluation>>>;

#[derive(Default)]
pub struct ReplExtension {
    history: History,
}

fn open(seed: String) -> Vec<Intent> {
    vec![
        Intent::SetFunctionForm(Some(ID.into())),
        Intent::BeginInput { purpose: PURPOSE.into(), seed, multiline: false },
    ]
}

impl ReplExtension {
    fn submit(&self, data: &ExtensionValue) -> Vec<Intent> {
        let expr = data.get("text").and_then(ExtensionValue::as_str).unwrap_or_default().trim().to_string();
        if expr.is_empty() {
            return open(String::new());
        }
        if let Ok(mut h) = self.history.lock() {
            h.push(Evaluation { expr: expr.clone(), value: None });
        }
        let mut out = vec![Intent::Eval { owner: ID.into(), expr }];
        out.extend(open(String::new()));
        out
    }
}

impl Extension for ReplExtension {
    fn id(&self) -> &str {
        ID
    }

    fn about(&self) -> crate::extension::About {
        crate::extension::About::new("REPL", "Evaluate Scheme in the resident session, at @/repl")
    }

    fn commands(&self) -> Vec<CommandDef> {
        vec![CommandDef {
            name: "repl".into(),
            description: "open the Scheme REPL".into(),
            intent: Intent::Extension(ExtensionIntent { extension: ID.into(), intent: "open".into(), data: ExtensionValue::Nil }),
        }]
    }

    fn vfs_providers(&self) -> Vec<Box<dyn VfsProvider>> {
        vec![Box::new(ReplProvider { history: self.history.clone() })]
    }

    fn handle_intent(&self, ei: &ExtensionIntent, _state: &AppState) -> Vec<Intent> {
        if ei.extension != ID {
            return Vec::new();
        }
        let purpose = ei.data.get("purpose").and_then(ExtensionValue::as_str);
        match ei.intent.as_str() {
            "open" => open(ei.data.as_str().unwrap_or_default().to_string()),
            "input-submit" if purpose == Some(PURPOSE) => self.submit(&ei.data),
            "input-cancel" if purpose == Some(PURPOSE) => vec![Intent::SetFunctionForm(None)],
            _ => Vec::new(),
        }
    }

    fn accept_content(&self, _path: &Path, result: &Content) {
        let Content::Value(value) = result else { return };
        // The session evaluates in submission order, so a value belongs to the
        // oldest evaluation still waiting for one.
        if let Ok(mut h) = self.history.lock()
            && let Some(pending) = h.iter_mut().find(|e| e.value.is_none())
        {
            pending.value = Some(value.clone());
        }
    }

    fn render_form(&self, frame: &mut Frame, inner: Rect, ctx: &FunctionRenderCtx) -> Option<String> {
        let theme = ctx.theme;
        let expr_style = Style::default().fg(theme.role(Role::Base)).add_modifier(Modifier::BOLD);
        let ok_style = Style::default().fg(theme.role(Role::Entry));
        let err_style = Style::default().fg(theme.role(Role::ExecFail));
        let muted = Style::default().fg(theme.role(Role::Muted));

        let mut lines: Vec<Line> = Vec::new();
        if let Ok(h) = self.history.lock() {
            for e in h.iter() {
                lines.push(Line::from(vec![Span::styled(PROMPT, muted), Span::styled(e.expr.clone(), expr_style)]));
                let (text, style) = match &e.value {
                    None => ("…".to_string(), muted),
                    Some(Ok(v)) => (v.clone(), ok_style),
                    Some(Err(err)) => (format!("error: {err}"), err_style),
                };
                lines.extend(text.lines().map(|l| Line::from(Span::styled(l.to_string(), style))));
            }
        }
        // The input line, with its caret, under the newest result.
        let input = ctx.func.input.as_ref().filter(|i| i.purpose == PURPOSE);
        let (query, caret) = input.map(|i| (i.query.as_str(), i.caret)).unwrap_or(("", 0));
        let under = query[caret..].chars().next();
        let after = caret + under.map_or(0, char::len_utf8);
        lines.push(Line::from(vec![
            Span::styled(PROMPT, muted),
            Span::styled(query[..caret].to_string(), expr_style),
            Span::styled(under.map_or(" ".into(), String::from), Style::default().add_modifier(Modifier::REVERSED)),
            Span::styled(query[after..].to_string(), expr_style),
        ]));
        // Keep the newest lines in view: the history scrolls up as it grows.
        let skip = lines.len().saturating_sub(inner.height as usize);
        frame.render_widget(Paragraph::new(lines.split_off(skip)), inner);
        Some("REPL".into())
    }
}

/// `@/repl`: one leaf per past expression, oldest first; activating one
/// reopens the REPL with that expression ready to edit and re-run.
struct ReplProvider {
    history: History,
}

impl VfsProvider for ReplProvider {
    fn mount(&self) -> &str {
        ID
    }

    fn list(&self, _path: &Path) -> io::Result<Vec<Entry>> {
        let h = self.history.lock().map_err(|_| io::Error::other("repl history lock poisoned"))?;
        let base = vfs_root().join(ID);
        Ok(h
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let mut leaf = Entry::virtual_leaf(base.join(i.to_string()), e.expr.clone());
                leaf.activation = Some(ExtensionIntent {
                    extension: ID.into(),
                    intent: "open".into(),
                    data: ExtensionValue::String(e.expr.clone()),
                });
                leaf
            })
            .collect())
    }
}
