//! Layout trees (doc/SCHEME.md, Role 3: Layouter): the data a `layout!` writes
//! and `app/layout.scm` declares, read into a native tree and checked against
//! the panel catalog. Laying a tree out on the screen is the UI's
//! (`ui::layout`); this module only knows the shape.

use crate::script::codec::cond_from_datum;
use crate::script::condition::Cond;
use crate::script::sexpr::Datum;

/// Every panel a layout may place. `help` is the only unboxed one: it draws no
/// border, so it shares no edge with its neighbours.
pub const PANELS: &[&str] = &["nav", "prop", "monitor", "file", "function", "help", "select-input", "select-candidates"];

pub fn is_boxed(panel: &str) -> bool {
    panel != "help"
}

/// The panels a core layout cannot do without.
fn required(layout_id: &str) -> &'static [&'static str] {
    match layout_id {
        "file" => &["file"],
        "select" => &["select-input", "select-candidates"],
        _ => &[],
    }
}

/// A number that is a literal or read from an option at layout time.
#[derive(Debug, Clone, PartialEq)]
pub enum Num {
    Lit(u16),
    Option(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Row(Vec<Node>),
    Column(Vec<Node>),
    Panel(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub kind: Kind,
    /// Fixed rows (in a column) or columns (in a row): (Large, Compact).
    pub size: Option<(Num, Num)>,
    pub weight: Option<Num>,
    /// The file : function split, from `FunctionPanelState.ratio`.
    pub ratio: Option<Side>,
    /// The least space the node needs; a row whose minimums do not fit hides
    /// children from its end until they do.
    pub min: u16,
    pub when: Option<Cond>,
}

/// Read `tree` as layout `id`, or say why it is not one.
pub fn parse(id: &str, tree: &Datum) -> Result<Node, String> {
    let root = node(tree)?;
    let mut panels = Vec::new();
    collect(&root, &mut panels);
    for p in required(id) {
        if !panels.contains(p) {
            return Err(format!("layout {id} needs panel {p}"));
        }
    }
    Ok(root)
}

/// The store's check for `layout!`: an empty string when `printed` is a valid
/// layout `id`, else the problem — a write it rejects leaves the previous
/// layout in place.
pub fn problem(id: &str, printed: &str) -> String {
    match crate::script::sexpr::parse(printed).and_then(|d| parse(id, &d)) {
        Ok(_) => String::new(),
        Err(e) => e,
    }
}

fn collect<'a>(n: &'a Node, out: &mut Vec<&'a str>) {
    match &n.kind {
        Kind::Panel(p) => out.push(p),
        Kind::Row(cs) | Kind::Column(cs) => cs.iter().for_each(|c| collect(c, out)),
    }
}

fn node(d: &Datum) -> Result<Node, String> {
    let Some(items) = d.as_list() else { return Err(format!("not a layout node: {}", crate::script::sexpr::render(d))) };
    let head = items.first().and_then(Datum::as_sym).unwrap_or_default();
    let mut n = Node { kind: Kind::Panel(String::new()), size: None, weight: None, ratio: None, min: 0, when: None };
    let rest = match head {
        "panel" => {
            let id = items.get(1).and_then(Datum::text).ok_or("panel: needs an id")?;
            if !PANELS.contains(&id) {
                return Err(format!("unknown panel {id}"));
            }
            n.kind = Kind::Panel(id.to_string());
            &items[2..]
        }
        "row" | "column" => &items[1..],
        other => return Err(format!("unknown layout node {other:?}")),
    };
    let mut children = Vec::new();
    for item in rest {
        let Some(sub) = item.as_list() else { return Err(format!("{head}: unexpected {}", crate::script::sexpr::render(item))) };
        match sub.first().and_then(Datum::as_sym).unwrap_or_default() {
            "row" | "column" | "panel" if head != "panel" => children.push(node(item)?),
            "size" => {
                let large = num(sub.get(1)).ok_or("size: needs a number")?;
                let compact = match sub.get(2) {
                    Some(d) => num(Some(d)).ok_or("size: the compact size must be a number")?,
                    None => large.clone(),
                };
                n.size = Some((large, compact));
            }
            "weight" => n.weight = Some(num(sub.get(1)).ok_or("weight: needs a number")?),
            "min" => n.min = sub.get(1).and_then(lit).ok_or("min: needs a number")?,
            "ratio" => {
                n.ratio = Some(match sub.get(1).and_then(Datum::as_sym) {
                    Some("left") => Side::Left,
                    Some("right") => Side::Right,
                    _ => return Err("ratio: left or right".into()),
                })
            }
            "when" => n.when = Some(cond_from_datum(sub.get(1).ok_or("when: needs a condition")?)),
            other => return Err(format!("{head}: unknown attribute {other:?}")),
        }
    }
    match head {
        "row" => n.kind = Kind::Row(children),
        "column" => n.kind = Kind::Column(children),
        _ => {}
    }
    if matches!(&n.kind, Kind::Row(c) | Kind::Column(c) if c.is_empty()) {
        return Err(format!("{head}: has no children"));
    }
    Ok(n)
}

fn lit(d: &Datum) -> Option<u16> {
    match d {
        Datum::Int(i) => u16::try_from(*i).ok(),
        _ => None,
    }
}

fn num(d: Option<&Datum>) -> Option<Num> {
    let d = d?;
    if let Some(n) = lit(d) {
        return Some(Num::Lit(n));
    }
    match d.as_list()? {
        [head, key] if head.as_sym() == Some("option") => Some(Num::Option(key.text()?.to_string())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    

    #[test]
    fn bad_layouts_are_explained() {
        assert_eq!(problem("file", "(column (panel file) (panel help (size 1)))"), "");
        assert!(problem("file", "(column (panel nav))").contains("needs panel file"));
        assert!(problem("file", "(column (panel file) (panel dock))").contains("unknown panel dock"));
        assert!(problem("file", "(row (panel file (weight x)))").contains("weight"));
        assert!(problem("file", "(row)").contains("no children"));
    }
}
