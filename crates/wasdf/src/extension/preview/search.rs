//! The less-style search algorithm for text/hex previews. Pure: the kernel's
//! reducer owns the search *state* (query, current match, the view jump) and
//! calls this to compute the match set — the matching logic is the extension's.

/// Every `(line, byte start, byte end)` match of `query` in `lines`, in document
/// order. Matching is ASCII case-insensitive and non-overlapping. An empty query
/// yields no matches. Byte ranges index the original lines (so the renderer can
/// highlight them directly).
pub fn find_matches(lines: &[String], query: &str) -> Vec<(usize, usize, usize)> {
    let needle = query.to_ascii_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (li, line) in lines.iter().enumerate() {
        for (s, e) in find_in_line(line, &needle) {
            out.push((li, s, e));
        }
    }
    out
}

/// ASCII case-insensitive, non-overlapping substring search returning byte
/// ranges within `line`. `needle` must already be lowercased.
fn find_in_line(line: &str, needle: &str) -> Vec<(usize, usize)> {
    let (hay, nee) = (line.as_bytes(), needle.as_bytes());
    if nee.is_empty() || hay.len() < nee.len() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i + nee.len() <= hay.len() {
        if hay[i..i + nee.len()].iter().zip(nee).all(|(a, b)| a.to_ascii_lowercase() == *b) {
            out.push((i, i + nee.len()));
            i += nee.len();
        } else {
            i += 1;
        }
    }
    out
}
