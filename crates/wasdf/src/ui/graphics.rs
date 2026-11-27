//! Terminal graphics for image previews and pty children: the `image-protocol`
//! option (`off` | `sixel` | `kitty`, under `@/settings/Display`).
//!
//! Two halves, split along the render contract:
//!
//! * **Render side** (pure). Lowering prepares each image once for the active
//!   protocol ([`prepare`]). Render places an image only when it is wholly
//!   visible and nothing overlays the panels: those cells are blanked and
//!   marked `skip`, and a [`Placement`] is recorded. Cells a placement left
//!   since the last frame are marked `AlwaysUpdate`, so the backend repaints
//!   what the picture covered. A pty child's kitty commands arrive through
//!   [`passthrough`], already positioned by the pty extension's render.
//! * **Output side** ([`Output`], driven by the event loop — the I/O
//!   boundary). It diffs the frame's placements against what is on screen and
//!   emits only control strings ([`Cmd`]). Kitty pixels travel out of band in
//!   POSIX shared memory (`t=s`): an image is transmitted once by id, then
//!   placed, moved and deleted with short commands. Over ssh, where the
//!   terminal cannot see our shm, it falls back to inline base64 (`t=d`).

use std::collections::HashSet;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ratatui::layout::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImageProtocol {
    #[default]
    Off,
    Sixel,
    Kitty,
}

impl ImageProtocol {
    /// From the `image-protocol` option value; anything unknown is `Off`.
    pub fn from_option(s: &str) -> Self {
        match s {
            "sixel" => ImageProtocol::Sixel,
            "kitty" => ImageProtocol::Kitty,
            _ => ImageProtocol::Off,
        }
    }
}

impl ImageProtocol {
    pub fn name(self) -> &'static str {
        match self {
            ImageProtocol::Off => "off",
            ImageProtocol::Sixel => "sixel",
            ImageProtocol::Kitty => "kitty",
        }
    }
}

/// Which protocols the display side can draw — probed from the terminal by
/// upstream (`app::terminal::graphics_support`), declared by a backend that
/// draws itself (cz-wasdf's framebuffer: kitty only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Support {
    pub kitty: bool,
    pub sixel: bool,
}

impl Support {
    pub fn has(self, p: ImageProtocol) -> bool {
        match p {
            ImageProtocol::Off => true,
            ImageProtocol::Sixel => self.sixel,
            ImageProtocol::Kitty => self.kitty,
        }
    }
}

/// What lowering needs: the protocol, what the display supports, and the
/// terminal's cell size in pixels. `None` cell size (the terminal does not
/// report it) disables graphics — there is no way to size the picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GraphicsConfig {
    pub protocol: ImageProtocol,
    pub support: Support,
    pub cell_px: Option<(u16, u16)>,
}

impl GraphicsConfig {
    pub fn active(&self) -> Option<(ImageProtocol, (u16, u16))> {
        if self.unavailable().is_some() {
            return None;
        }
        match (self.protocol, self.cell_px) {
            (ImageProtocol::Off, _) => None,
            (p, Some(px)) => Some((p, px)),
            (_, None) => None,
        }
    }

    /// Why the chosen protocol cannot draw here, as the line the image area
    /// shows instead (above the symbol-cell fallback). `None` for `off`.
    pub fn unavailable(&self) -> Option<String> {
        let p = self.protocol;
        if p == ImageProtocol::Off {
            return None;
        }
        if !self.support.has(p) {
            return Some(format!("image-protocol: {} not available", p.name()));
        }
        match self.cell_px {
            Some((w, h)) if w > 0 && h > 0 => None,
            _ => Some(format!("image-protocol: {} not available (no cell size)", p.name())),
        }
    }
}

/// The cell size the event loop last measured, for code that sizes things in
/// pixels outside the render tree (the pty tells its child, so a child such
/// as `mpv --vo=kitty` can size its frames). `(0, 0)` = unknown.
static CELL_PX: AtomicU32 = AtomicU32::new(0);

pub fn set_cell_px(px: Option<(u16, u16)>) {
    let (w, h) = px.unwrap_or((0, 0));
    CELL_PX.store((u32::from(w) << 16) | u32::from(h), Ordering::Relaxed);
}

pub fn cell_px() -> (u16, u16) {
    let v = CELL_PX.load(Ordering::Relaxed);
    ((v >> 16) as u16, v as u16)
}

#[derive(Debug)]
pub enum GraphicData {
    /// A complete sixel DCS sequence.
    Sixel(String),
    /// RGB8 pixels, `px.0 * px.1 * 3` bytes, shipped by the output side.
    Rgb(Vec<u8>),
}

/// One prepared picture. `id` is unique per preparation, so the output side
/// can tell a new picture from one already on screen.
#[derive(Debug)]
pub struct Graphic {
    pub id: u64,
    /// Pixel size of the picture.
    pub px: (u32, u32),
    /// Cells it covers from its top-left cell.
    pub cols: u16,
    pub rows: u16,
    pub data: GraphicData,
}

impl Graphic {
    /// The kitty image id: nonzero, in a range a pty child is unlikely to use.
    pub fn kitty_id(&self) -> u32 {
        ((self.id as u32) & 0x3fff_ffff) | 0x4000_0000
    }
}

/// A graphic placed at a cell this frame.
#[derive(Debug, Clone)]
pub struct Placement {
    pub x: u16,
    pub y: u16,
    pub graphic: Arc<Graphic>,
}

impl Placement {
    pub fn rect(&self) -> Rect {
        Rect { x: self.x, y: self.y, width: self.graphic.cols, height: self.graphic.rows }
    }
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Prepare `rgb` (`width`×`height`, RGB8) to fit inside `cols`×`rows` cells:
/// scaled — up or down, aspect kept — until its longer side (relative to the
/// box) meets the box. `None` for an empty image or box.
pub fn prepare(protocol: ImageProtocol, cell_px: (u16, u16), width: u32, height: u32, rgb: &[u8], cols: u16, rows: u16) -> Option<Arc<Graphic>> {
    if width == 0 || height == 0 || cols == 0 || rows == 0 || rgb.len() < (width * height * 3) as usize {
        return None;
    }
    let (cw, ch) = (u32::from(cell_px.0), u32::from(cell_px.1));
    let (bw, bh) = (u32::from(cols) * cw, u32::from(rows) * ch);
    let scale = f64::min(bw as f64 / width as f64, bh as f64 / height as f64);
    let (w, h) = (((width as f64 * scale) as u32).clamp(1, bw), ((height as f64 * scale) as u32).clamp(1, bh));
    let pix = resize(rgb, width, height, w, h);
    let data = match protocol {
        ImageProtocol::Sixel => GraphicData::Sixel(sixel(&pix, w, h)),
        ImageProtocol::Kitty => GraphicData::Rgb(pix),
        ImageProtocol::Off => return None,
    };
    Some(Arc::new(Graphic {
        id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        px: (w, h),
        cols: w.div_ceil(cw).min(u32::from(cols)) as u16,
        rows: h.div_ceil(ch).min(u32::from(rows)) as u16,
        data,
    }))
}

// ---------------------------------------------------------------- passthrough

/// A control string to write at a cell (`None`: wherever the cursor is).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cmd {
    pub at: Option<(u16, u16)>,
    pub data: String,
}

/// What pty children contributed this frame: their kitty commands at screen
/// cells, and the panel rects they draw into.
#[derive(Debug, Default)]
pub struct Passthrough {
    pub cmds: Vec<Cmd>,
    pub regions: Vec<Rect>,
}

static PASSTHROUGH: Mutex<Passthrough> = Mutex::new(Passthrough { cmds: Vec::new(), regions: Vec::new() });

/// Render side: a pty panel hands over its child's graphics for this frame.
/// Not I/O — the commands are queued for the event loop to write.
pub fn passthrough(region: Rect, cmds: impl IntoIterator<Item = Cmd>) {
    if let Ok(mut p) = PASSTHROUGH.lock() {
        p.regions.push(region);
        p.cmds.extend(cmds);
    }
}

pub fn take_passthrough() -> Passthrough {
    PASSTHROUGH.lock().map(|mut p| std::mem::take(&mut *p)).unwrap_or_default()
}

// ---------------------------------------------------------------- output side

/// Kitty: delete every visible placement and free the images.
pub const KITTY_DELETE_ALL: &str = "\x1b_Ga=d,d=A,q=2\x1b\\";

/// The event loop's view of what is on screen, and the diff to the next frame.
#[derive(Debug, Default)]
pub struct Output {
    /// `(id, x, y)` of the placements on screen.
    shown: Vec<(u64, u16, u16)>,
    /// Kitty image ids the terminal holds.
    transmitted: HashSet<u64>,
    /// Pty regions that had child graphics on screen.
    regions: Vec<Rect>,
    /// The terminal cannot read our shared memory (ssh): send pixels inline.
    pub remote: bool,
}

impl Output {
    pub fn new(remote: bool) -> Self {
        Output { remote, ..Default::default() }
    }

    /// The screen was wiped (resize, suspend): nothing of ours is on it.
    pub fn invalidate(&mut self) {
        self.shown.clear();
        self.transmitted.clear();
        self.regions.clear();
    }

    /// The control strings that turn the screen into this frame's graphics.
    /// Empty when nothing changed — the common case costs a comparison.
    pub fn frame(&mut self, placements: &[Placement], pass: Passthrough) -> Vec<Cmd> {
        let mut out = Vec::new();
        // A pty region that went away (or moved) leaves its child's images
        // behind; kitty can only drop them all, ours included.
        let lost = self.regions.iter().any(|r| !pass.regions.contains(r));
        let child_cleared = pass.cmds.iter().any(|c| kitty_action(&c.data) == Some('d'));
        if lost {
            out.push(Cmd { at: None, data: KITTY_DELETE_ALL.into() });
        }
        out.extend(pass.cmds);
        self.regions = pass.regions;
        if lost || child_cleared {
            // Our placements went with them; put them back below.
            self.shown.clear();
            if lost {
                self.transmitted.clear();
            }
        }

        let now: Vec<(u64, u16, u16)> = placements.iter().map(|p| (p.graphic.id, p.x, p.y)).collect();
        if now == self.shown {
            return out;
        }
        let live: HashSet<u64> = now.iter().map(|k| k.0).collect();
        // Take down kitty placements that moved or went away; free an image
        // only when no placement of it remains. (Sixel needs nothing here:
        // render repainted the cells it covered.)
        for &(id, ..) in self.shown.iter().filter(|k| !now.contains(k)) {
            if !self.transmitted.contains(&id) {
                continue;
            }
            let kid = kitty_id(id);
            if live.contains(&id) {
                out.push(Cmd { at: None, data: format!("\x1b_Ga=d,d=i,i={kid},q=2\x1b\\") });
            } else {
                out.push(Cmd { at: None, data: format!("\x1b_Ga=d,d=I,i={kid},q=2\x1b\\") });
                self.transmitted.remove(&id);
            }
        }
        for p in placements.iter().filter(|p| !self.shown.contains(&(p.graphic.id, p.x, p.y))) {
            let at = Some((p.x, p.y));
            match &p.graphic.data {
                GraphicData::Sixel(s) => out.push(Cmd { at, data: s.clone() }),
                GraphicData::Rgb(rgb) => {
                    let kid = p.graphic.kitty_id();
                    if !self.transmitted.contains(&p.graphic.id) {
                        let (w, h) = p.graphic.px;
                        let data = if self.remote { None } else { shm::publish(kid, rgb).ok() };
                        out.push(Cmd {
                            at: None,
                            data: match data {
                                Some(name) => format!("\x1b_Ga=t,t=s,f=24,s={w},v={h},i={kid},q=2;{}\x1b\\", base64(name.as_bytes())),
                                None => kitty_inline(&format!("a=t,f=24,s={w},v={h},i={kid},q=2"), rgb),
                            },
                        });
                        self.transmitted.insert(p.graphic.id);
                    }
                    out.push(Cmd { at, data: format!("\x1b_Ga=p,i={kid},C=1,q=2\x1b\\") });
                }
            }
        }
        self.shown = now;
        out
    }
}

fn kitty_id(id: u64) -> u32 {
    ((id as u32) & 0x3fff_ffff) | 0x4000_0000
}

/// The `a=` key of a kitty command (`t` when absent, as the protocol says).
pub fn kitty_action(cmd: &str) -> Option<char> {
    let body = cmd.strip_prefix("\x1b_G")?;
    let keys = body.split(';').next().unwrap_or("");
    Some(keys.split(',').find_map(|kv| kv.strip_prefix("a=")).and_then(|v| v.chars().next()).unwrap_or('t'))
}

/// Inline (`t=d`) transmission: base64 in chunks of at most 4096 bytes.
fn kitty_inline(keys: &str, rgb: &[u8]) -> String {
    let b64 = base64(rgb);
    let chunks: Vec<&[u8]> = b64.as_bytes().chunks(4096).collect();
    let mut o = String::with_capacity(b64.len() + chunks.len() * 16 + keys.len() + 8);
    for (i, c) in chunks.iter().enumerate() {
        let more = u8::from(i + 1 < chunks.len());
        if i == 0 {
            o.push_str(&format!("\x1b_G{keys},m={more};"));
        } else {
            o.push_str(&format!("\x1b_Gm={more};"));
        }
        o.push_str(std::str::from_utf8(c).unwrap_or_default());
        o.push_str("\x1b\\");
    }
    o
}

/// POSIX shared memory for `t=s`. The terminal maps and unlinks the object
/// after reading it, so each transmission publishes a fresh one.
mod shm {
    use std::ffi::CString;
    use std::io;

    pub fn publish(kitty_id: u32, bytes: &[u8]) -> io::Result<String> {
        let name = format!("/wasdf-{}-{kitty_id:x}", std::process::id());
        let cname = CString::new(name.clone()).map_err(io::Error::other)?;
        // SAFETY: plain libc calls on an fd we own; the mapping is written
        // within its length and unmapped before return.
        unsafe {
            let fd = libc::shm_open(cname.as_ptr(), libc::O_CREAT | libc::O_RDWR | libc::O_TRUNC, 0o600);
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            let fail = |fd| {
                let e = io::Error::last_os_error();
                libc::close(fd);
                libc::shm_unlink(cname.as_ptr());
                Err(e)
            };
            if libc::ftruncate(fd, bytes.len() as libc::off_t) != 0 {
                return fail(fd);
            }
            let p = libc::mmap(std::ptr::null_mut(), bytes.len(), libc::PROT_WRITE, libc::MAP_SHARED, fd, 0);
            if p == libc::MAP_FAILED {
                return fail(fd);
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), p as *mut u8, bytes.len());
            libc::munmap(p, bytes.len());
            libc::close(fd);
        }
        Ok(name)
    }
}

// ---------------------------------------------------------------- encoders

/// Resample: area average when shrinking, nearest neighbour when growing; a
/// straight copy when the size holds.
fn resize(rgb: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    if (sw, sh) == (dw, dh) {
        return rgb[..(sw * sh * 3) as usize].to_vec();
    }
    let mut out = Vec::with_capacity((dw * dh * 3) as usize);
    for y in 0..dh {
        let (y0, y1) = (y * sh / dh, ((y + 1) * sh / dh).max(y * sh / dh + 1).min(sh));
        for x in 0..dw {
            let (x0, x1) = (x * sw / dw, ((x + 1) * sw / dw).max(x * sw / dw + 1).min(sw));
            let mut acc = [0u32; 3];
            for yy in y0..y1 {
                for xx in x0..x1 {
                    let p = ((yy * sw + xx) * 3) as usize;
                    for c in 0..3 {
                        acc[c] += u32::from(rgb[p + c]);
                    }
                }
            }
            let n = (y1 - y0) * (x1 - x0);
            out.extend(acc.iter().map(|v| (v / n) as u8));
        }
    }
    out
}

/// Sixel with a popularity palette: colors are bucketed to 4 bits a channel,
/// the 256 most frequent buckets become the palette, and every bucket maps to
/// its nearest palette entry.
fn sixel(rgb: &[u8], w: u32, h: u32) -> String {
    let bucket = |p: usize| (usize::from(rgb[p] >> 4) << 8) | (usize::from(rgb[p + 1] >> 4) << 4) | usize::from(rgb[p + 2] >> 4);
    let n = (w * h) as usize;
    let mut count = vec![0u32; 4096];
    for i in 0..n {
        count[bucket(i * 3)] += 1;
    }
    let mut used: Vec<usize> = (0..4096).filter(|&b| count[b] > 0).collect();
    used.sort_by_key(|&b| std::cmp::Reverse(count[b]));
    used.truncate(256);
    let level = |b: usize, shift: u32| (((b >> shift) & 15) * 17) as i32;
    let pal: Vec<(i32, i32, i32)> = used.iter().map(|&b| (level(b, 8), level(b, 4), level(b, 0))).collect();
    let mut map = vec![0u8; 4096];
    for b in (0..4096).filter(|&b| count[b] > 0) {
        let (r, g, bl) = (level(b, 8), level(b, 4), level(b, 0));
        let best = pal
            .iter()
            .enumerate()
            .min_by_key(|(_, (pr, pg, pb))| (pr - r).pow(2) + (pg - g).pow(2) + (pb - bl).pow(2))
            .map_or(0, |(i, _)| i);
        map[b] = best as u8;
    }
    let idx: Vec<u8> = (0..n).map(|i| map[bucket(i * 3)]).collect();

    let mut o = String::with_capacity(n);
    // P2=1: pixels left at 0 keep the background.
    o.push_str(&format!("\x1bP0;1;0q\"1;1;{w};{h}"));
    for (i, (r, g, b)) in pal.iter().enumerate() {
        o.push_str(&format!("#{i};2;{};{};{}", r * 100 / 255, g * 100 / 255, b * 100 / 255));
    }
    let (w, h) = (w as usize, h as usize);
    let mut present = vec![false; pal.len()];
    for band in (0..h).step_by(6) {
        let bh = (h - band).min(6);
        present.iter_mut().for_each(|p| *p = false);
        for y in band..band + bh {
            for x in 0..w {
                present[idx[y * w + x] as usize] = true;
            }
        }
        let mut first = true;
        for (c, _) in present.iter().enumerate().filter(|(_, p)| **p) {
            if !first {
                o.push('$');
            }
            first = false;
            o.push_str(&format!("#{c}"));
            let (mut run_ch, mut run_n) = (0u8, 0usize);
            for x in 0..w {
                let mut bits = 0u8;
                for k in 0..bh {
                    if idx[(band + k) * w + x] as usize == c {
                        bits |= 1 << k;
                    }
                }
                let ch = 0x3f + bits;
                if ch == run_ch {
                    run_n += 1;
                } else {
                    push_run(&mut o, run_ch, run_n);
                    (run_ch, run_n) = (ch, 1);
                }
            }
            push_run(&mut o, run_ch, run_n);
        }
        o.push('-');
    }
    o.push_str("\x1b\\");
    o
}

fn push_run(o: &mut String, ch: u8, n: usize) {
    match n {
        0 => {}
        1..=3 => (0..n).for_each(|_| o.push(ch as char)),
        _ => o.push_str(&format!("!{n}{}", ch as char)),
    }
}

pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut o = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        o.push(T[(n >> 18) as usize & 63] as char);
        o.push(T[(n >> 12) as usize & 63] as char);
        o.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        o.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A kitty picture whose box is exactly its own size (no resampling).
    fn kitty(w: u32, h: u32) -> Arc<Graphic> {
        let (cols, rows) = (w.div_ceil(8) as u16, h.div_ceil(16) as u16);
        prepare(ImageProtocol::Kitty, (8, 16), w, h, &vec![7; (w * h * 3) as usize], cols, rows).unwrap()
    }

    #[test]
    fn kitty_transmits_once_then_only_places_and_deletes() {
        let g = kitty(16, 16);
        let kid = g.kitty_id();
        let mut out = Output::new(true);
        let at = |x, y| vec![Placement { x, y, graphic: g.clone() }];

        let first = out.frame(&at(1, 1), Passthrough::default());
        assert_eq!(first.len(), 2);
        assert!(first[0].data.starts_with(&format!("\x1b_Ga=t,f=24,s=16,v=16,i={kid}")));
        assert_eq!(first[1], Cmd { at: Some((1, 1)), data: format!("\x1b_Ga=p,i={kid},C=1,q=2\x1b\\") });

        assert!(out.frame(&at(1, 1), Passthrough::default()).is_empty(), "unchanged frame writes nothing");

        let moved = out.frame(&at(1, 3), Passthrough::default());
        assert_eq!(moved[0].data, format!("\x1b_Ga=d,d=i,i={kid},q=2\x1b\\"), "keeps the data");
        assert_eq!(moved[1].at, Some((1, 3)));
        assert_eq!(moved.len(), 2, "no retransmission");

        let gone = out.frame(&[], Passthrough::default());
        assert_eq!(gone, vec![Cmd { at: None, data: format!("\x1b_Ga=d,d=I,i={kid},q=2\x1b\\") }]);
    }

}
