//! The matcher backend: fuzzy ranking. One shipped implementation (SkimMatcher);
//! the trait is a test seam, not an extension point. Used by the async Search
//! plan to rank a recursive walk.

use std::path::{Path, PathBuf};

use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;

/// Configuration for the affinity re-rank layer. All weights are in skim's
/// score units (base char = 16). A zero weight disables that term entirely.
#[derive(Debug, Clone)]
pub struct MatcherConfig {
    /// Extra score applied when the best match is in the basename.
    pub tail_bonus: i64,
    /// Score deducted per directory level of depth.
    pub depth_penalty: i64,
    /// Score deducted per character of the full relative path (sinks UUID paths).
    pub length_penalty: i64,
    /// Bonus for regular files with the executable bit set.
    pub exec_bonus: i64,
    /// Extensions that get +24 (source/document files).
    pub preferred_ext: Vec<String>,
    /// Extensions that get −24 (cache/build artifacts).
    pub noise_ext: Vec<String>,
    /// Directory components that trigger a −48 penalty.
    pub noise_dirs: Vec<String>,
    /// The cap on raw entries a deep-search walk collects before ranking
    /// (`@/settings/Matcher/max-results`). Bounds the recursive `fs::walk`
    /// itself, not the post-rank display truncation.
    pub max_results: usize,
}

impl Default for MatcherConfig {
    fn default() -> Self {
        let csv = |s: &str| s.split(',').map(String::from).collect();
        MatcherConfig {
            tail_bonus: 16,
            depth_penalty: 4,
            length_penalty: 1,
            exec_bonus: 16,
            preferred_ext: csv("rs,toml,md,txt,sh,py,js,ts,json,c,h,go"),
            noise_ext: csv("lock,tmp,cache,log,o,d,rlib"),
            noise_dirs: csv("node_modules,.git,target,Caches,Library"),
            max_results: 65535,
        }
    }
}

impl MatcherConfig {
    /// The weights and cap as the options currently hold them (`matcher.*`).
    /// Built at each Search spawn, so a settings edit takes effect on the next
    /// search with no projection of its own.
    pub fn from_options(o: &crate::core::OptionStore) -> Self {
        MatcherConfig {
            tail_bonus: o.get_i64("matcher.tail-bonus"),
            depth_penalty: o.get_i64("matcher.depth-penalty"),
            length_penalty: o.get_i64("matcher.length-penalty"),
            exec_bonus: o.get_i64("matcher.exec-bonus"),
            preferred_ext: o.get_list("matcher.preferred-ext"),
            noise_ext: o.get_list("matcher.noise-ext"),
            noise_dirs: o.get_list("matcher.noise-dirs"),
            max_results: o.get_u32("matcher.max-results") as usize,
        }
    }
}

/// Coarse file kind used by the affinity layer. Computed once from symlink
/// metadata (no follow) so the caller avoids double-statting.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum FileKind {
    Regular { executable: bool },
    Dir,
    Symlink,
    Other,
}

impl FileKind {
    pub fn from_meta(meta: &std::fs::Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;
        let ft = meta.file_type();
        if ft.is_symlink() {
            FileKind::Symlink
        } else if ft.is_dir() {
            FileKind::Dir
        } else if ft.is_file() {
            #[cfg(unix)]
            let executable = meta.permissions().mode() & 0o111 != 0;
            #[cfg(not(unix))]
            let executable = false;
            FileKind::Regular { executable }
        } else {
            FileKind::Other
        }
    }
}

/// A survivor from the two-stage rank with the metadata already read.
/// Passed back so the Search arm can build an `Entry` without re-statting.
pub struct RankedEntry {
    pub path: PathBuf,
    /// Symlink metadata (no follow). `None` if the stat failed.
    pub meta: Option<std::fs::Metadata>,
}

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

/// Discount applied to a full-path match vs. a basename-only match.
/// Equals one `bonusBoundaryDelimiter` in fzf's table (§3.1 of plan/matcher.md).
const REL_DIR_DISCOUNT: i64 = 9;

/// Maximum survivors to stat and affinity-re-rank. Keeps per-keystroke I/O
/// proportional to survivors, not to the walk size.
const RERANK_N: usize = 800;

/// Preferred / noise extension bonus magnitude. Fixed relative to skim's base
/// char score (16); user tunes the weight fields, not this constant.
const EXT_BONUS: i64 = 24;

/// Noise-directory penalty magnitude.
const NOISE_DIR_PEN: i64 = 48;

/// Tail-biased base score: prefer matches in the filename over matches that
/// span directory components. Returns `(score, is_tail)` where `is_tail` is
/// true when the basename match dominates and the tail_bonus should be applied.
fn base_tail(matcher: &dyn MatcherBackend, rel: &str, query: &str) -> Option<(i64, bool)> {
    let name = Path::new(rel).file_name().and_then(|n| n.to_str()).unwrap_or(rel);
    let full = matcher.score(rel, query);
    let tail = matcher.score(name, query);
    match (tail, full) {
        (Some(ts), Some(fs)) => {
            let disc = fs.saturating_sub(REL_DIR_DISCOUNT);
            if ts >= disc { Some((ts, true)) } else { Some((disc, false)) }
        }
        (Some(ts), None) => Some((ts, true)),
        (None, Some(fs)) => Some((fs.saturating_sub(REL_DIR_DISCOUNT), false)),
        (None, None) => None,
    }
}

/// Pure affinity adjustment on top of the base score. All inputs are already
/// computed; this function performs no I/O and is directly unit-testable.
pub(crate) fn affinity(
    base: i64,
    rel: &str,
    name: &str,
    kind: FileKind,
    cfg: &MatcherConfig,
) -> i64 {
    let mut score = base;

    // Depth penalty: number of path separators in the relative path string.
    let depth = rel.chars().filter(|&c| c == '/').count() as i64;
    score -= cfg.depth_penalty * depth;

    // Length penalty: sinks UUID/hash-heavy paths.
    score -= cfg.length_penalty * rel.len() as i64;

    // Extension bonus/penalty.
    let ext = Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("");
    if !ext.is_empty() {
        if cfg.preferred_ext.iter().any(|p| p == ext) {
            score += EXT_BONUS;
        } else if cfg.noise_ext.iter().any(|n| n == ext) {
            score -= EXT_BONUS;
        }
    }

    // Exec bonus (file-manager specific: surfaces runnable files).
    if matches!(kind, FileKind::Regular { executable: true }) {
        score += cfg.exec_bonus;
    }

    // Noise directory penalty (first hit only).
    for component in Path::new(rel).components() {
        if let Some(s) = component.as_os_str().to_str() {
            if cfg.noise_dirs.iter().any(|d| d == s) {
                score -= NOISE_DIR_PEN;
                break;
            }
        }
    }

    score
}

/// Two-stage rank: base fuzzy pass (no I/O) → affinity re-rank (stat survivors
/// only). An empty query returns paths in lexicographic order.
///
/// Returns `RankedEntry` values so the caller can build `Entry` values without
/// re-statting the survivors (the symlink metadata is already read in stage 2).
pub fn rank_paths(
    matcher: &dyn MatcherBackend,
    root: &Path,
    paths: Vec<PathBuf>,
    query: &str,
    cfg: &MatcherConfig,
) -> Vec<RankedEntry> {
    if query.is_empty() {
        let mut paths = paths;
        paths.sort();
        paths.truncate(cfg.max_results);
        return paths.into_iter().map(|p| RankedEntry { path: p, meta: None }).collect();
    }

    // Stage 1: score every path with tail bias (no I/O). Drop non-matches.
    let mut stage1: Vec<(i64, bool, PathBuf)> = paths
        .into_iter()
        .filter_map(|p| {
            let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy();
            base_tail(matcher, &rel, query).map(|(s, tail)| (s, tail, p))
        })
        .collect();
    // Sort descending by base score; path ascending as tiebreak.
    stage1.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.2.cmp(&b.2)));
    // Never stat fewer survivors than the final cap needs, so a raised
    // max-results actually returns more results — RERANK_N (800) is only
    // the default floor for the common, much smaller cap.
    stage1.truncate(RERANK_N.max(cfg.max_results));

    // Stage 2: stat survivors (one symlink_metadata each), apply affinity.
    let mut stage2: Vec<(i64, PathBuf, Option<std::fs::Metadata>)> = stage1
        .into_iter()
        .map(|(base, is_tail, p)| {
            let meta = std::fs::symlink_metadata(&p).ok();
            let kind = meta.as_ref().map(|m| FileKind::from_meta(m)).unwrap_or(FileKind::Other);
            let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy();
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let tail_adj = if is_tail { cfg.tail_bonus } else { 0 };
            let score = affinity(base + tail_adj, &rel, &name, kind, cfg);
            (score, p, meta)
        })
        .collect();
    stage2.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    stage2.truncate(cfg.max_results);
    stage2.into_iter().map(|(_, p, meta)| RankedEntry { path: p, meta }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> MatcherConfig {
        MatcherConfig::default()
    }

    fn cfg_with_max(max_results: usize) -> MatcherConfig {
        MatcherConfig { max_results, ..MatcherConfig::default() }
    }

    // ── affinity unit tests (pure, no I/O) ───────────────────────────────────

    #[test]
    fn uuid_path_sinks_below_short_basename() {
        let short = affinity(
            100,
            "src/main.rs",
            "main.rs",
            FileKind::Regular { executable: false },
            &cfg(),
        );
        let uuid = affinity(
            100,
            "Library/Caches/A1B2-C3D4-E5F6-G7H8/A1B2C3D4E5F6G7H8-content.cache",
            "A1B2C3D4E5F6G7H8-content.cache",
            FileKind::Regular { executable: false },
            &cfg(),
        );
        assert!(short > uuid, "short={short}, uuid={uuid}");
    }

    #[test]
    fn deep_path_sinks_vs_shallow() {
        let shallow = affinity(
            100,
            "Makefile",
            "Makefile",
            FileKind::Regular { executable: false },
            &cfg(),
        );
        let deep = affinity(
            100,
            "a/b/c/d/e/f/Makefile",
            "Makefile",
            FileKind::Regular { executable: false },
            &cfg(),
        );
        assert!(shallow > deep, "shallow={shallow}, deep={deep}");
    }

    #[test]
    fn exec_file_rises() {
        let non_exec = affinity(
            100,
            "scripts/build",
            "build",
            FileKind::Regular { executable: false },
            &cfg(),
        );
        let exec = affinity(
            100,
            "scripts/build",
            "build",
            FileKind::Regular { executable: true },
            &cfg(),
        );
        assert!(exec > non_exec, "exec={exec}, non_exec={non_exec}");
    }

    #[test]
    fn node_modules_path_sinks() {
        let normal = affinity(
            100,
            "src/index.js",
            "index.js",
            FileKind::Regular { executable: false },
            &cfg(),
        );
        let noise = affinity(
            100,
            "node_modules/lodash/index.js",
            "index.js",
            FileKind::Regular { executable: false },
            &cfg(),
        );
        assert!(normal > noise, "normal={normal}, node_modules={noise}");
    }

    #[test]
    fn preferred_ext_rises_noise_ext_sinks() {
        let base_score = 100i64;
        let preferred = affinity(
            base_score,
            "doc/README.md",
            "README.md",
            FileKind::Regular { executable: false },
            &cfg(),
        );
        let noise = affinity(
            base_score,
            "doc/Cargo.lock",
            "Cargo.lock",
            FileKind::Regular { executable: false },
            &cfg(),
        );
        // Both have same depth and similar length; preferred_ext gives +24, noise_ext gives -24
        assert!(preferred > noise, "preferred={preferred}, noise_ext={noise}");
    }

    // ── rank_paths integration tests ──────────────────────────────────────────

    #[test]
    fn rank_paths_empty_query_sorted_lexicographically() {
        let m = SkimMatcher::default();
        let root = Path::new("/root");
        let paths = vec![PathBuf::from("/root/b.txt"), PathBuf::from("/root/a.txt")];
        let ranked = rank_paths(&m, root, paths, "", &cfg());
        assert_eq!(ranked[0].path, PathBuf::from("/root/a.txt"));
        assert_eq!(ranked[1].path, PathBuf::from("/root/b.txt"));
    }

    #[test]
    fn rank_paths_node_modules_sinks_below_src() {
        let m = SkimMatcher::default();
        let root = Path::new("/r");
        // Both paths contain "src" (matches the query).
        let paths = vec![
            PathBuf::from("/r/node_modules/some/deep/src/index.js"),
            PathBuf::from("/r/src/main.rs"),
        ];
        let ranked = rank_paths(&m, root, paths, "src", &cfg());
        assert!(!ranked.is_empty(), "at least one match expected");
        assert_eq!(ranked[0].path, PathBuf::from("/r/src/main.rs"), "src/main.rs should rank first");
    }

    #[test]
    fn rank_paths_empty_query_result_count_follows_max_results_not_a_hardcoded_500() {
        let m = SkimMatcher::default();
        let root = Path::new("/root");
        let paths: Vec<PathBuf> = (0..10).map(|i| PathBuf::from(format!("/root/f{i}.txt"))).collect();
        let ranked = rank_paths(&m, root, paths, "", &cfg_with_max(3));
        assert_eq!(ranked.len(), 3, "the empty-query branch must respect the configured cap");
    }

    #[test]
    fn rank_paths_query_result_count_follows_max_results_not_a_hardcoded_500() {
        let m = SkimMatcher::default();
        let root = Path::new("/root");
        // 10 candidates all matching "f" so all 10 survive stage 1.
        let paths: Vec<PathBuf> = (0..10).map(|i| PathBuf::from(format!("/root/f{i}.txt"))).collect();
        let ranked = rank_paths(&m, root, paths, "f", &cfg_with_max(3));
        assert_eq!(ranked.len(), 3, "the matched-query branch must respect the configured cap, not truncate(500)");
    }
}
