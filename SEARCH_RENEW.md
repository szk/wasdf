# SEARCH_RENEW — separating shallow filtering from deep search

Implementation plan. Executor: Sonnet 5 working in this repository. Read
CLAUDE.md and doc/ARCHITECTURE.md / doc/UI.md first; every rule there binds
this work (pure sync reducer, closed plan set, zero-edit extensions, disable/
notify/continue, MVP-over-extensibility).

## Requirements (user, verbatim)

> - fおよび/の各パネルの挙動について、現在の動作は維持したまま「表示されている
>   一覧の絞り込み（浅い）」と「サブツリー検索（深い）」のintentを明確に区分せよ。
> - #1のfile-searchは深い検索、#2のfile-search結果には浅い検索を行え。#4は/で
>   file-searchで深い検索を行い、その後は/で浅い検索を行え。
> - #5 はfを実装せよ。その後の検索は浅い検索を行え。

Panel numbers refer to the investigation table:

- **#1** PANEL_FILE, File mode, real directory (`state.entries`)
- **#2** the Select candidates panel showing file-search results (`sel.view`)
- **#4** CmdOp's destination browser inside PANEL_FUNCTION (`Form.entries`,
  extension-private)
- **#5** PANEL_FILE listing a **virtual** directory (@/settings, @/messages,
  @/bookmarks — same `state.entries`, `vfs::is_virtual(state.cwd)`)

## Vocabulary (use these names everywhere: code, docs, commits)

- **Deep search** (サブツリー検索): recursive `fs::walk` from a root +
  matcher ranking. Async, via `Plan::Search`. Today's `f` file-search.
- **Shallow filter** (表示一覧の絞り込み): rank/filter the entries already
  on screen. Synchronous, no I/O, no plan.

## Target behavior matrix

| Surface | Key | First press | Subsequent |
|---|---|---|---|
| #1 file panel, real cwd | `f` | deep (unchanged) | deep (unchanged — each `f` re-walks cwd; commit still lands in File mode via `commit_file_search` + `filter_stack`) |
| #5 file panel, virtual cwd | `f` | **shallow** (new — `f` today silently walks nothing: `fs::walk` on a relative `@/...` path yields zero entries) | shallow (cwd is still virtual) |
| #2 file-search results (Select, Navigate phase) | `f` | **shallow refine** over the current result view (new) | shallow again (refine of refine) |
| #2 file-search input phase | typing | deep re-rank per keystroke (unchanged) | — |
| #4 CmdOp browser | `/` | **deep** search from the browsed dir (changed: today `/` is shallow-only) | **shallow** over the deep results; resets to deep when the browsed dir changes (h/l/parent/descend reload `Form.entries`) |

Everything else — Esc semantics, `filter_stack`, `filter-maxed` gating,
`commit_file_search`, Select cancel, cmdop cancel — keeps its current meaning.

## Current-state anchors (verified)

- `f` binding: `crates/wasdf/src/app/keymap.scm:6` —
  `("f" file-search (not filter-maxed))`; native fallback in
  `crates/wasdf/src/script/keymap.rs`.
- Deep pipeline: `SelectSource::FileWalk` (`core/mode.rs:137`) → `refilter`
  (`core/reducer.rs` ~1506) → `Plan::Search{root,query,show_hidden}`
  (`core/event.rs:46`) → `runtime/task.rs:148` (`fs::walk`, `rank_paths`) →
  `(Purpose::Search, Payload::Entries)` → `sel.view = EntryList`
  (`core/reducer.rs:1605`).
- Commit: `select_confirm` (`core/reducer.rs` ~941) special-cases the
  FileWalk source ahead of the Confirm-shape table →
  `commit_file_search` (~1021) pushes `FilterFrame` onto
  `state.filter_stack`, adopts matches as `state.entries`.
- Sync ranking helper already present: `fuzzy_rank` / `subseq_score`
  (`core/reducer.rs:1544`) — pure, dependency-free; `SelectSource::Static`
  already ranks synchronously in the reducer, so shallow ranking in the
  reducer breaks no purity rule.
- Select mode id scoping: `Mode::mode_id()` = `select:<spec.id>`
  (`core/mode.rs:49`); keymap scope prefix-matching supports per-instance
  bindings.
- `cursor_path()` (`core/state.rs:447`) keys the Select arm on
  `spec.id == "file-search"`.
- CmdOp browser: `crates/wasdf/src/extension/cmdop/extension.rs` —
  `Form.entries` + `filter_query`; `/` → `filter-start` → `FunctionInput`
  purpose `"filter"`, live shallow filter via `active_filter` (~503);
  directory listings arrive through the generic Read
  (`accept_content`, self-gated on the browsed path, ~849).
- Async routing: `Payload::Read{owner,..}` + `Purpose::Content` is the only
  owner-addressed completion; kernel routes it to `ext.accept_content`
  (`app/kernel.rs:344`). `Plan::Search` today has **no owner**.
- Conditions live in `script/condition.rs` (e.g. `has-filter`,
  `filter-maxed`); extensions may also register conditions.

## Design decisions

1. **Two keymap tokens, two reducer paths.** `file-search` keeps meaning
   *deep*. New token `filter-list` means *shallow over the file panel's
   current listing*. The split is visible in configuration (keymap.scm), not
   hidden inside a handler branch.

2. **Shallow filter reuses Select** ("modal UI is unified under Select").
   New `SelectSource::Entries(Vec<Entry>)`: `refilter` ranks the embedded
   entries **synchronously** against the query (subsequence score on
   `Entry::name`; reuse/extract the scoring from `fuzzy_rank`) and writes
   `sel.view = EntryList` directly (the entry grid — same rendering,
   navigation, and marks as file-search results). No plan, no async, works
   on virtual paths.

3. **New intents** (add to the catalog in doc/ARCHITECTURE.md):
   - `Intent::FilterListing` — push a Select with
     `SelectSource::Entries(state.entries.clone())`, id `"filter-list"`,
     input Fuzzy, `on_confirm: Navigate`. The snapshot must be taken in the
     reducer (the keymap codec builds intents statically, without state — a
     parameterless marker intent is the only shape that works).
   - `Intent::SelectRefine` — only meaningful inside a Select whose `view`
     is non-empty: **replace** the current Select mode in place (pop+push,
     same generation discipline as other mode swaps) with id
     `"filter-list"`, `SelectSource::Entries(<current view entries>)`,
     empty query, phase Input. Deep results are frozen; typing now ranks
     shallowly. Esc still cancels the whole Select (no refine-undo stack —
     MVP; document it).
   - `Intent::LoadSearch { owner, root, query }` — the extension-facing
     deep search (decision 6). Mirrors `LoadContent`'s shape.

4. **Commit guard keys on the source family, not the id.** In
   `select_confirm`, replace the FileWalk special-case guard with:
   `on_confirm == Navigate && matches!(source, FileWalk | Entries(_))` →
   `commit_file_search`. Both deep results and shallow filters then commit
   identically into File mode (`filter_stack` push, Esc pops, title shows
   `— filter: q`). `commit_file_search` itself needs no change.

5. **Keymap** (`app/keymap.scm` + native fallback in `script/keymap.rs`,
   both must stay in sync; update the `(bind …)` template comments in the
   generated config too if applicable):
   ```scheme
   ;; file panel: deep on real dirs (unchanged behavior), shallow on VFS (#5)
   ("f" file-search (and (not filter-maxed) (not cwd-virtual)))
   ("f" filter-list (and (not filter-maxed) cwd-virtual))
   ;; select scope: shallow refine of any result view (#2)
   ("f" select-refine (and select-phase-navigate select-has-view))
   ```
   New native conditions in `script/condition.rs`:
   - `cwd-virtual` → `vfs::is_virtual(&s.cwd)`
   - `select-has-view` → `s.select.as_ref().and_then(|s| s.view.as_ref()).is_some_and(|v| !v.entries.is_empty())`

6. **CmdOp deep search (#4) — owner-addressed `Plan::Search`.** Extend the
   plan: `Search { root, query, show_hidden, owner: Option<String> }`.
   - `owner: None` → exactly today's completion
     (`Purpose::Search`, `Payload::Entries`). All existing call sites pass
     `None`; behavior byte-identical.
   - `owner: Some(id)` → complete as `Purpose::Content`,
     `Payload::Read { owner, path: root, result: ReadResult::Dir { entries: ranked } }`.
     Zero new kernel routing: the existing Content branch already delivers
     to `accept_content`, and CmdOp already self-gates on the browsed path.
     This is a documented amendment to the async contract (the plan set
     stays closed; one plan gains an addressing field that `Read` already
     has). Update ARCHITECTURE.md's plan table and Read→accept_content
     section accordingly.

7. **CmdOp `/` flow** (extension-internal; core untouched — zero-edit):
   - Add `Form.deep_results: bool` (default false). Cleared whenever the
     browsed dir changes (`parent`, `descend`, and the `accept_content`
     arm that reloads a plain dir listing).
   - `"filter-start"` handler branches: if `!deep_results` → open
     `FunctionInput` with purpose `"search-deep"` (submit-only; deep per
     keystroke would spam walks for no benefit). On submit with non-empty
     text → return `Intent::LoadSearch { owner: "cmdop", root: <browsed dir>, query }`.
     If `deep_results` → today's purpose `"filter"` live shallow filter,
     unchanged code path (`active_filter`).
   - `accept_content` distinguishing deep results from plain listings: a
     plain reload self-gates on `path == browsed dir` already; the deep
     completion arrives with the same path. Disambiguate with a pending
     flag (`Form.awaiting_deep: bool` set when LoadSearch is emitted,
     cleared on the next accepted Dir; when set, store entries and set
     `deep_results = true`). Keep it to these two booleans — no enum state
     machine unless the tests force one.
   - Render: while `deep_results`, show the query in the listing header
     (there is an existing title/summary line) so the mode is visible.
   - Reset also on form close/cancel (existing form teardown already drops
     the whole `Form`).

8. **`cursor_path()`**: change the Select arm to key on “has a view” (any
   Select with `view: Some`) instead of `spec.id == "file-search"`, so
   cursor-follow (theme editor, preview) works identically inside a
   `filter-list` Select. Verify no other site string-matches the id.

9. **Out of scope** (do not build): refine-undo inside Select; filtering
   preview's directory PanelView (not an entry grid); a generic
   filterable-listing Extension seam (single consumer — CmdOp — handles it
   internally); rebinding cmdop `/` to `f`.

## Implementation order

Work in four phases; run `cargo test -p wasdf` green after each.

### Phase 1 — core shallow filter (#5 + plumbing)
1. `core/mode.rs`: add `SelectSource::Entries(Vec<Entry>)`; constructor
   `SelectSpec::filter_list()` is *not* needed — the reducer builds the spec
   inline in the `FilterListing` handler (it owns the snapshot).
2. `core/intent.rs`: add `FilterListing`, `SelectRefine`,
   `LoadSearch { owner, root, query }`. `script/codec.rs`: tokens
   `filter-list`, `select-refine` (LoadSearch is extension-emitted only —
   no keymap token).
3. `core/reducer.rs`:
   - `FilterListing` handler (guard: File mode, non-empty entries).
   - `refilter`: `Entries(pool)` arm — synchronous rank, set `sel.view`,
     cursor 0. Empty query = full pool.
   - Commit guard change (decision 4).
   - `LoadSearch` → `Effects::plan(Plan::Search { …, owner: Some(owner) })`.
4. `script/condition.rs`: `cwd-virtual`, `select-has-view`.
5. Keymap: scm + native fallback (decision 5).
6. `core/state.rs`: `cursor_path()` view-keyed (decision 8).
7. Tests (reducer): filter_listing_pushes_a_shallow_select_over_the_current_listing;
   shallow_refilter_ranks_synchronously_without_a_plan;
   shallow_confirm_commits_via_filter_stack_like_file_search;
   filter_listing_works_on_a_virtual_listing (entries with `@/...` paths);
   keymap tests: f_is_deep_on_a_real_cwd_and_shallow_on_a_virtual_cwd.

### Phase 2 — Select refine (#2)
1. `SelectRefine` handler in the reducer (decision 3).
2. Keymap select-scope binding + conditions already from Phase 1.
3. Tests: refine_freezes_deep_results_and_ranks_them_shallowly;
   refine_confirm_still_commits_into_file_mode;
   refine_is_gated_off_without_a_result_view.

### Phase 3 — owner-addressed Search + CmdOp (#4)
1. `core/event.rs`: `owner` field on `Plan::Search`; `runtime/task.rs`
   completion branch (decision 6); existing spawn site passes `None`.
2. CmdOp: decision 7. Tests (cmdop):
   slash_opens_a_deep_search_input_when_no_deep_results_are_active;
   deep_submit_emits_load_search_for_the_browsed_dir;
   accepting_deep_results_flips_slash_to_the_shallow_filter;
   changing_the_browse_dir_resets_to_deep.
   Plus a task-level test: an_owner_addressed_search_completes_as_content_read.
3. Note: `Plan::Search`'s uniqueness key is its purpose; owner-addressed
   searches complete as `Content`, which is also unique-keyed — acceptable
   (a newer content read supersedes a pending deep search; document in the
   test if observable).

### Phase 4 — docs + live verification
1. doc/UI.md: “Filtering the file panel” gains the shallow/deep vocabulary
   and the matrix above; key table rows for `f` (two conditions) and the
   select-scope `f`; CmdOp `/` two-stage note in the CmdOp section.
2. doc/ARCHITECTURE.md: intent catalog additions; `Plan::Search` owner
   amendment in the async contract; note that shallow filtering is
   reducer-synchronous by design.
3. doc/EXTENSION.md: CmdOp section (deep-then-shallow `/`); `LoadSearch`
   alongside `LoadContent` in the generic-mechanisms table.
4. tmux verification (build, then `WASDF_CONFIG_DIR=<scratch> …` against a
   scratch playground; see previous sessions’ pattern):
   - real dir: `f` → type → Enter → filtered File mode; Esc pops.
   - `@/settings`: `f` now filters the listing; Enter commits; Esc pops;
     `f` again refilters shallowly.
   - real dir: `f`, type a broad query, Tab/phase to Navigate, `f` again →
     refine input over frozen results; type; Enter commits.
   - `c` on a file → `/` → query → Enter → listing becomes ranked walk
     results (header shows query) → `/` again → live shallow narrowing →
     `h` (parent) resets → `/` is deep again.
   - `@/messages`: `f` filters diagnostics (bonus check for #5).

## Preservation checklist (regression gates)

- Real-dir `f` end-to-end unchanged (existing reducer/keymap tests must not
  be rewritten to pass — only extended).
- `filter-maxed` still gates both `f` bindings; `MAX_FILTER_DEPTH` shared.
- Esc ladder unchanged: clear-filter(has-filter) → clear-selection.
- Select cancel/confirm shapes unchanged for palette/command selects
  (Static/Commands sources untouched).
- CmdOp: wasd still drives the file panel; Space/Enter/h/l semantics in the
  browser unchanged; `q`/Esc cancel unchanged.
- `load_entries` still clears `filter_stack` on any real (re)load.
- No `eprintln!`; failures go to `services::messages::log(text, true)`.
