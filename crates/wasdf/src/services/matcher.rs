//! The matcher backend: fuzzy ranking. One shipped implementation (SkimMatcher);
//! the trait is a test seam, not an extension point. Used by the async Search
//! plan to rank a recursive walk.

use std::path::{Path, PathBuf};

use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;

/// The one service trait in the kernel. A test seam, not an extension point.
pub trait MatcherBackend: Send + Sync {
    fn score(&self, haystack: &str, needle: &str) -> Option<i64>;
}

pub struct SkimMatcher {
    inner: SkimMatcherV2,
}

impl Default for SkimMatcher {
    fn default() -> Self {
        SkimMatcher { inner: SkimMatcherV2::default() }
    }
}

impl MatcherBackend for SkimMatcher {
    fn score(&self, haystack: &str, needle: &str) -> Option<i64> {
        self.inner.fuzzy_match(haystack, needle)
    }
}

/// Rank walked paths by their display path (relative to `root`) against the
/// query. An empty query keeps directory-first name order.
pub fn rank_paths(
    matcher: &dyn MatcherBackend,
    root: &Path,
    paths: Vec<PathBuf>,
    query: &str,
) -> Vec<PathBuf> {
    if query.is_empty() {
        let mut paths = paths;
        paths.sort();
        paths.truncate(500);
        return paths;
    }
    let mut scored: Vec<(i64, PathBuf)> = paths
        .into_iter()
        .filter_map(|p| {
            let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().into_owned();
            matcher.score(&rel, query).map(|s| (s, p))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    scored.truncate(500);
    scored.into_iter().map(|(_, p)| p).collect()
}
