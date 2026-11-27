//! The layout solver: a layout tree (`script::layout`) and the frame area in,
//! one rect per visible panel out. Pure arithmetic over AppState, the option
//! projection, and the conditions — no I/O, so render may call it.
//!
//! Boxed neighbours share their border line: a split gives the first child
//! `len` cells and starts the rest on its last one, so the two borders coincide
//! (the connected frame, doc/UI.md). An unboxed panel (`help`) shares nothing.

use ratatui::layout::Rect;

use crate::core::{AppState, OptionStore};
use crate::script::condition::Conditions;
use crate::script::layout::{Kind, Node, Num, Side, is_boxed};

/// Below this many rows the frame is in the Compact size class.
const COMPACT_BELOW: u16 = 25;

/// The layouts in force: the core ones, each replaced by the user's where
/// the store holds one.
pub struct Layouts {
    defaults: Vec<(String, Node)>,
    user: Vec<(String, Node)>,
}

impl Layouts {
    pub fn new(defaults: Vec<(String, Node)>) -> Self {
        Layouts { defaults, user: Vec::new() }
    }

    pub fn set_user(&mut self, user: Vec<(String, Node)>) {
        self.user = user;
    }

    pub fn get(&self, id: &str) -> Option<&Node> {
        self.user.iter().chain(&self.defaults).find(|(i, _)| i == id).map(|(_, n)| n)
    }
}

pub struct Env<'a> {
    pub state: &'a AppState,
    pub options: &'a OptionStore,
    pub conds: &'a Conditions,
}

/// Lay `root` out over `area`: `(panel id, rect)` for every visible panel.
pub fn solve(root: &Node, area: Rect, env: &Env) -> Vec<(String, Rect)> {
    let mut out = Vec::new();
    place(root, area, area.height < COMPACT_BELOW, env, &mut out);
    out
}

fn place(node: &Node, r: Rect, compact: bool, env: &Env, out: &mut Vec<(String, Rect)>) {
    let (children, across) = match &node.kind {
        Kind::Panel(id) => return out.push((id.clone(), r)),
        Kind::Row(cs) => (cs, true),
        Kind::Column(cs) => (cs, false),
    };
    let dim = if across { r.width } else { r.height };
    let mut shown: Vec<&Node> =
        children.iter().filter(|c| c.when.as_ref().is_none_or(|w| env.conds.eval(w, env.state))).collect();
    // Minimums that do not fit hide children from the end; the first stays.
    while shown.len() > 1 && shown.iter().map(|c| c.min).sum::<u16>() > dim {
        shown.pop();
    }
    let fixed = |c: &Node| c.size.as_ref().map(|(large, small)| num(if compact { small } else { large }, env));
    let shares = |a: &Node, b: &Node| boxed(a) && boxed(b);

    let mut rest = r;
    for (i, child) in shown.iter().enumerate() {
        let Some(next) = shown.get(i + 1) else {
            place(child, rest, compact, env, out);
            break;
        };
        let avail = if across { rest.width } else { rest.height };
        let later = &shown[i + 1..];
        let len = match fixed(child) {
            Some(n) => n,
            None => {
                // Room the fixed children after this one keep (less the line
                // each shares with its predecessor).
                let reserved: u16 = later
                    .iter()
                    .enumerate()
                    .filter_map(|(k, c)| {
                        let prev = if k == 0 { child } else { &later[k - 1] };
                        fixed(c).map(|n| n.saturating_sub(shares(prev, c) as u16))
                    })
                    .sum();
                let room = avail.saturating_sub(reserved);
                let flex_after: Vec<&&Node> = later.iter().filter(|c| fixed(c).is_none()).collect();
                if flex_after.is_empty() {
                    room
                } else {
                    let w = weight(child, env);
                    let total = w + flex_after.iter().map(|c| weight(c, env)).sum::<u32>();
                    let share = (room as u32 * w / total.max(1)) as u16;
                    share.max(child.min).min(room.saturating_sub(flex_after.iter().map(|c| c.min).sum()))
                }
            }
        };
        let (head, tail) = split(rest, len, across, shares(child, next));
        place(child, head, compact, env, out);
        rest = tail;
    }
}

fn boxed(n: &Node) -> bool {
    match &n.kind {
        Kind::Panel(id) => is_boxed(id),
        _ => true,
    }
}

fn num(n: &Num, env: &Env) -> u16 {
    match n {
        Num::Lit(v) => *v,
        Num::Option(key) => u16::try_from(env.options.get_u32(key)).unwrap_or(u16::MAX),
    }
}

fn weight(n: &Node, env: &Env) -> u32 {
    match (n.ratio, &n.weight) {
        (Some(side), _) => {
            let (l, r) = env.state.function.ratio.weights();
            if side == Side::Left { l } else { r }
        }
        (None, Some(w)) => num(w, env).max(1) as u32,
        (None, None) => 1,
    }
}

/// Split `r` after `len` cells. A shared line is clamped the way the frame
/// always has been (each side keeps at least 2 cells for its border).
fn split(r: Rect, len: u16, across: bool, shared: bool) -> (Rect, Rect) {
    let dim = if across { r.width } else { r.height };
    let len = if shared { len.clamp(2, dim.saturating_sub(2).max(2)) } else { len.min(dim) };
    let next = len - shared as u16;
    if across {
        (Rect { width: len, ..r }, Rect { x: r.x + next, width: r.width - next, ..r })
    } else {
        (Rect { height: len, ..r }, Rect { y: r.y + next, height: r.height - next, ..r })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Mode, OptionValue, SelectInput};
    /// Split a rect into left and right sharing the divider column.
    fn split_h(r: Rect, left_w: u16) -> (Rect, Rect) {
        let left_w = left_w.clamp(2, r.width.saturating_sub(2).max(2));
        let sx = r.x + left_w - 1;
        let left = Rect { x: r.x, y: r.y, width: sx - r.x + 1, height: r.height };
        let right = Rect { x: sx, y: r.y, width: r.x + r.width - sx, height: r.height };
        (left, right)
    }

    /// Split a rect into top and bottom sharing the divider row.
    fn split_v(r: Rect, top_h: u16) -> (Rect, Rect) {
        let top_h = top_h.clamp(2, r.height.saturating_sub(2).max(2));
        let sy = r.y + top_h - 1;
        let top = Rect { x: r.x, y: r.y, width: r.width, height: sy - r.y + 1 };
        let bot = Rect { x: r.x, y: sy, width: r.width, height: r.y + r.height - sy };
        (top, bot)
    }

    fn ratio_left(width: u16, lw: u32, rw: u32) -> u16 {
        ((width as u32 * lw) / (lw + rw)) as u16
    }


    /// A hand-written reference split of the top row, kept here so the
    /// default layout is pinned to it.
    struct ReferenceTop {
        nav_weight: u32,
        prop_visible: bool,
        prop_weight: u32,
        monitor_visible: bool,
        monitor_weight: u32,
    }

    /// Minimum content widths (excluding chrome borders) for each panel.
    const MIN_NAV: u16 = 22;
    const MIN_PROP: u16 = 18;
    const MIN_MON: u16 = 16;

    /// Compute which panels are shown and their rects given the available area.
    /// Optional panels collapse gracefully when the terminal is too narrow:
    /// monitor drops first, then prop.
    fn layout_panels(area: Rect, opts: &ReferenceTop) -> (Rect, Option<Rect>, Option<Rect>) {
        let w = area.width;

        let (show_prop, show_mon) = if opts.prop_visible && opts.monitor_visible {
            if w >= MIN_NAV + MIN_PROP + MIN_MON { (true, true) }
            else if w >= MIN_NAV + MIN_PROP { (true, false) }
            else { (false, false) }
        } else if opts.prop_visible {
            (w >= MIN_NAV + MIN_PROP, false)
        } else if opts.monitor_visible {
            (false, w >= MIN_NAV + MIN_MON)
        } else {
            (false, false)
        };

        if !show_prop && !show_mon {
            return (area, None, None);
        }

        let nw = opts.nav_weight.max(1);
        let pw = opts.prop_weight.max(1);
        let mw = opts.monitor_weight.max(1);

        if show_prop && show_mon {
            let tw = nw + pw + mw;
            let nav_px = ((w as u32 * nw / tw) as u16)
                .max(MIN_NAV)
                .min(w.saturating_sub(MIN_PROP + MIN_MON));
            // Remaining area after nav (split_h overlaps by 1 for shared border).
            let rest_w = w.saturating_sub(nav_px) + 1;
            let prop_px = ((rest_w as u32 * pw / (pw + mw)) as u16)
                .max(MIN_PROP)
                .min(rest_w.saturating_sub(MIN_MON));
            let (nav_r, rest) = split_h(area, nav_px);
            let (prop_r, mon_r) = split_h(rest, prop_px);
            (nav_r, Some(prop_r), Some(mon_r))
        } else if show_prop {
            let tw = nw + pw;
            let nav_px = ((w as u32 * nw / tw) as u16)
                .max(MIN_NAV)
                .min(w.saturating_sub(MIN_PROP));
            let (nav_r, prop_r) = split_h(area, nav_px);
            (nav_r, Some(prop_r), None)
        } else {
            // show_mon only
            let tw = nw + mw;
            let nav_px = ((w as u32 * nw / tw) as u16)
                .max(MIN_NAV)
                .min(w.saturating_sub(MIN_MON));
            let (nav_r, mon_r) = split_h(area, nav_px);
            (nav_r, None, Some(mon_r))
        }
    }

    fn layout(id: &str) -> Node {
        crate::app::defaults::layouts().into_iter().find(|(i, _)| i == id).unwrap().1
    }

    fn options() -> std::sync::Arc<OptionStore> {
        std::sync::Arc::new(OptionStore::new(crate::app::defaults::core_options()))
    }

    fn top_options(o: &OptionStore) -> ReferenceTop {
        ReferenceTop {
            nav_weight: o.get_u32("top.nav-weight"),
            prop_visible: o.get_bool("top.prop-visible"),
            prop_weight: o.get_u32("top.prop-weight"),
            monitor_visible: o.get_bool("top.monitor-visible"),
            monitor_weight: o.get_u32("top.monitor-weight"),
        }
    }

    /// The expected frame, rect for rect, computed by hand.
    fn reference(state: &AppState, o: &OptionStore, area: Rect) -> Vec<(String, Rect)> {
        let tr = if area.height >= 25 { 7 } else { 5 };
        let top = Rect { height: tr, ..area };
        let main = Rect { y: area.y + tr - 1, height: area.height - tr, ..area };
        let help = Rect { y: area.y + area.height - 1, height: 1, ..area };
        let (nav, prop, mon) = layout_panels(top, &top_options(o));
        let mut out = vec![("nav".to_string(), nav)];
        out.extend(prop.map(|r| ("prop".to_string(), r)));
        out.extend(mon.map(|r| ("monitor".to_string(), r)));
        let (lw, rw) = state.function.ratio.weights();
        match state.mode() {
            Mode::Select(spec) => {
                let (left, right) = if spec.function_hint.is_some() {
                    let (l, r) = split_h(main, ratio_left(main.width, lw, rw));
                    (l, Some(r))
                } else {
                    (main, None)
                };
                if matches!(spec.input, SelectInput::None) {
                    out.push(("select-candidates".into(), left));
                } else {
                    let (input, cands) = split_v(left, 3);
                    out.push(("select-input".into(), input));
                    out.push(("select-candidates".into(), cands));
                }
                out.extend(right.map(|r| ("function".to_string(), r)));
            }
            _ if state.function.visible => {
                let (file, func) = split_h(main, ratio_left(main.width, lw, rw));
                out.push(("file".into(), file));
                out.push(("function".into(), func));
            }
            _ => out.push(("file".into(), main)),
        }
        out.push(("help".into(), help));
        out
    }

    fn check(state: &AppState, o: &std::sync::Arc<OptionStore>, id: &str) {
        let mut conds = Conditions::default();
        conds.bind_options(o.clone());
        let env = Env { state, options: o, conds: &conds };
        for (w, h) in [(40, 10), (60, 24), (80, 25), (100, 30), (120, 40), (200, 60), (55, 30), (57, 30), (39, 30)] {
            let area = Rect { x: 0, y: 0, width: w, height: h };
            assert_eq!(solve(&layout(id), area, &env), reference(state, o, area), "{id} at {w}x{h}");
        }
    }

    #[test]
    fn the_file_layout_draws_what_the_frame_always_has() {
        let o = options();
        let mut state = AppState::new(std::env::temp_dir());
        for (prop, mon, func) in [(true, true, false), (true, true, true), (false, true, true), (true, false, false), (false, false, true)] {
            o.set("top.prop-visible", OptionValue::Bool(prop));
            o.set("top.monitor-visible", OptionValue::Bool(mon));
            state.function.visible = func;
            check(&state, &o, "file");
        }
        o.set("top.nav-weight", OptionValue::Int(9));
        check(&state, &o, "file");
    }

}
