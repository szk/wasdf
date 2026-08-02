(quote (
    (file file (
        ("w" cursor-up) ("s" cursor-down) ("a" cursor-left) ("d" cursor-right) ("Enter" activate)
        ("k" func-up)   ("Up" func-up)    ("j" func-down)   ("Down" func-down)
        ("h" func-left) ("Left" func-left) ("l" func-right) ("Right" func-right)
        ("Space" toggle-select) ("x" activate) ("e" activate) ("z" command-palette)
        ;; f: deep search (recursive walk) on a real directory; shallow filter
        ;; (rank the listing already on screen, no I/O) on a virtual one, where
        ;; there is nothing for the walk to read. See SEARCH_RENEW.md.
        ("f" file-search (and (not filter-maxed) (not cwd-virtual)))
        ("f" filter-list (and (not filter-maxed) cwd-virtual))
        ("W" cursor-top) ("S" cursor-bottom) ("Ctrl-a" select-all)
        ("E" start-edit)
        ("@" toggle-vfs-root)
        ("~" (navigate-to "~"))
        ("," cycle-function-panel) ("." toggle-dotfiles) ("v" cycle-list-layout)
        ;; Confirming a file-search query lands its matches straight in this
        ;; same "file file" scope (see reducer.rs's commit_file_search) — every
        ;; key above already works on them. Esc steps back one filter level
        ;; while one is active (has-filter beats the unconditioned binding
        ;; below by specificity), else clears the selection as always.
        ("Esc" clear-filter has-filter)
        ("Esc" clear-selection) ("r" refresh)
        ("Backspace" delete-selected) ("Delete" delete-selected)
        ("Tab" focus-next-panel) ("Shift-Tab" focus-prev-panel) ("q" quit) ("Ctrl-c" quit)))
    (file function (
        ;; wasd always targets the file panel, so pressing it from the function
        ;; panel moves the file cursor and (via intent_focus) returns focus there.
        ("w" cursor-up) ("s" cursor-down) ("a" cursor-left) ("d" cursor-right)
        ("j" scroll-down sublayout-content) ("Down" scroll-down sublayout-content)
        ("k" scroll-up sublayout-content)   ("Up" scroll-up sublayout-content)
        ("h" func-left sublayout-content)   ("Left" func-left sublayout-content)
        ("l" func-right sublayout-content)  ("Right" func-right sublayout-content)
        ("g" scroll-top sublayout-content)  ("G" scroll-bottom sublayout-content)
        ("n" toggle-line-numbers sublayout-content)
        ("Enter" open sublayout-content)
        ("Enter" set-sublayout-content sublayout-exec)
        ("Ctrl-c" kill-process sublayout-exec)
        ("," cycle-function-panel sublayout-content) ("Esc" hide-function-panel sublayout-content)
        ("q" hide-function-panel sublayout-content)))
        ;; CmdOp's sublayout-command keys now live in the cmdop extension's keymap.
    (select #f (
        ("Enter" confirm) ("Esc" cancel)
        ("Ctrl-n" cursor-down) ("Down" cursor-down) ("Ctrl-p" cursor-up) ("Up" cursor-up)
        ("w" cursor-up    select-phase-navigate) ("s" cursor-down   select-phase-navigate)
        ("a" cursor-left  select-phase-navigate) ("d" cursor-right  select-phase-navigate)
        ("W" cursor-top   select-phase-navigate) ("S" cursor-bottom select-phase-navigate)
        ("v" cycle-list-layout select-phase-navigate)
        ;; Shallow refine: rank the current result view (deep file-search, or
        ;; an earlier filter/refine) further, in place. See SEARCH_RENEW.md.
        ("f" select-refine (and select-phase-navigate select-has-view))
        ("Space" toggle-select select-phase-navigate) ("q" cancel select-phase-navigate)))
    (policy #f (
        ("y" confirm) ("Enter" confirm) ("n" cancel) ("Esc" cancel)))
    (input-form #f (
        ("Enter" confirm) ("Esc" cancel)
        ;; Structured navigation (Bool/Choice/Int/Color, and List while
        ;; browsing rows) — gated off while typing (Text, or a List row's
        ;; nested edit) so those keys fall through to tui-input instead.
        ("w" cursor-up    (not input-form-typing)) ("Up" cursor-up    (not input-form-typing))
        ("s" cursor-down  (not input-form-typing)) ("Down" cursor-down (not input-form-typing))
        ("Left" cursor-left (not input-form-typing)) ("Right" cursor-right (not input-form-typing))
        ("W" cursor-top (not input-form-typing)) ("S" cursor-bottom (not input-form-typing))
        ("Home" cursor-top (not input-form-typing)) ("End" cursor-bottom (not input-form-typing))
        ("Space" toggle-select (not input-form-typing))
        ("PageUp" page-up (not input-form-typing)) ("PageDown" page-down (not input-form-typing))
        ;; "a"/"d" mean cursor-left/right everywhere except List's row-browsing,
        ;; where they add/remove a row instead (List has no left/right concept).
        ("a" cursor-left  (and (not input-form-typing) (not input-form-list-browsing)))
        ("d" cursor-right (and (not input-form-typing) (not input-form-list-browsing)))
        ("a" add-item input-form-list-browsing)
        ("d" remove-item input-form-list-browsing)
        ("Delete" remove-item input-form-list-browsing)
        ("Backspace" remove-item input-form-list-browsing)))))
