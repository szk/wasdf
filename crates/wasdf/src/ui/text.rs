//! Display-width text helpers for code that places characters cell by cell
//! (titles, border labels, the nav columns). A CJK character takes two cells,
//! so widths are display columns, never `chars().count()`.

use ratatui::buffer::Buffer;
use ratatui::style::Style;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// `s`'s width in terminal columns.
pub fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// `c`'s width in terminal columns (0 for a control or combining character).
pub fn char_width(c: char) -> usize {
    c.width().unwrap_or(0)
}

/// The longest prefix of `s` that fits in `max` columns. A wide character
/// that would straddle the limit is left out rather than split.
pub fn take_width(s: &str, max: usize) -> String {
    let mut used = 0;
    s.chars()
        .take_while(|&c| {
            used += char_width(c);
            used <= max
        })
        .collect()
}

/// `s` cut to `max` columns with a trailing `…` when it does not fit.
pub fn ellipsize(s: &str, max: usize) -> String {
    if width(s) <= max {
        s.to_string()
    } else {
        format!("{}…", take_width(s, max.saturating_sub(1)))
    }
}

/// Write `ch` at `(x, y)`, its trailing cell reset the way `Buffer::set_stringn`
/// does, so a later character never lands on the second half of a wide one.
/// Draws nothing when it would not fit in `max` columns or the buffer. Returns
/// the columns it takes.
pub fn put_char(buf: &mut Buffer, x: u16, y: u16, ch: char, style: Style, max: u16) -> u16 {
    let w = char_width(ch) as u16;
    if w == 0 || w > max {
        return w;
    }
    if x.saturating_add(w) <= buf.area.right() && buf.area.contains((x, y).into()) {
        buf.set_stringn(x, y, ch.to_string(), w as usize, style);
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    /// The `[楳図かずお]` title: each character advances by its own width, so a
    /// wide one is never overwritten by the next (it used to lose every other).
    #[test]
    fn wide_characters_placed_one_by_one_keep_every_character() {
        let s = "[楳図かずお] わたし";
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 1));
        let mut x = 0;
        for ch in s.chars() {
            x += put_char(&mut buf, x, 0, ch, Style::default(), 30 - x);
        }
        let mut row = String::new();
        let mut i = 0;
        while i < x {
            let sym = buf[(i, 0)].symbol();
            row.push_str(sym);
            i += width(sym).max(1) as u16;
        }
        assert_eq!(row, s);
        assert_eq!(width(s), x as usize);
        assert_eq!(take_width("楳図か", 5), "楳図", "a straddling wide char is left out");
        assert_eq!(ellipsize("楳図かずお", 6), "楳図…");
    }
}
