(quote (
    (file file (
        ("w" cursor-up "cursor") ("a" cursor-left "cursor") ("s" cursor-down "cursor") ("d" cursor-right "cursor")
        ("Enter" activate)
        ("k" func-up)   ("Up" func-up)    ("j" func-down)   ("Down" func-down)
        ("h" func-left) ("Left" func-left) ("l" func-right) ("Right" func-right)
        ("Space" toggle-select "select") ("e" activate) ("z" command-palette "palette")
        ;; f: deep search (recursive walk) on a real directory; shallow filter
        ;; (rank the listing already on screen, no I/O) on a virtual one, where
        ;; there is nothing for the walk to read. See SEARCH_RENEW.md.
        ("f" file-search (and (not filter-maxed) (not cwd-virtual)) "find")
        ("f" filter-list (and (not filter-maxed) cwd-virtual) "filter")
        ("W" cursor-top) ("S" cursor-bottom) ("Ctrl-a" select-all)
        ("E" start-edit "edit full")
        ("@" toggle-vfs-root "vfs")
        ("~" (navigate-to "~"))
        ("," cycle-function-panel "panel") ("." toggle-dotfiles) ("v" cycle-list-layout)
        ;; Confirming a file-search query lands its matches straight in this
        ;; same "file file" scope (see reducer.rs's commit_file_search) — every
        ;; key above already works on them. Esc steps back one filter level
        ;; while one is active (has-filter beats the unconditioned binding
        ;; below by specificity), else clears the selection as always.
        ("Esc" clear-filter has-filter)
        ("Esc" clear-selection) ("r" refresh)
        ("Backspace" delete-selected) ("Delete" delete-selected)
        ("Tab" focus-next-panel) ("Shift-Tab" focus-prev-panel) ("q" quit "quit") ("Ctrl-c" quit)))
    (file function (
        ;; wasd always targets the file panel, so pressing it from the function
        ;; panel moves the file cursor and (via intent_focus) returns focus there.
        ("w" cursor-up "cursor") ("a" cursor-left "cursor") ("s" cursor-down "cursor") ("d" cursor-right "cursor")
        ("j" scroll-down sublayout-content "scroll") ("Down" scroll-down sublayout-content)
        ("k" scroll-up sublayout-content "scroll")   ("Up" scroll-up sublayout-content)
        ("h" func-left sublayout-content "◂▸")   ("Left" func-left sublayout-content)
        ("l" func-right sublayout-content "◂▸")  ("Right" func-right sublayout-content)
        ("g" scroll-top sublayout-content "top/bottom")  ("G" scroll-bottom sublayout-content "top/bottom")
        ("n" toggle-line-numbers sublayout-content "line#")
        ("Enter" open sublayout-content "open")
        ("Enter" set-sublayout-content sublayout-exec)
        ("Ctrl-c" kill-process sublayout-exec)
        ("," cycle-function-panel sublayout-content) ("Esc" hide-function-panel sublayout-content)
        ("q" hide-function-panel sublayout-content "hide")))
        ;; CmdOp's builder keys are at the end of this file (gated by cmdop-active).
    (select #f (
        ("Enter" confirm "confirm/next") ("Esc" cancel "cancel")
        ("Ctrl-n" cursor-down) ("Down" cursor-down) ("Ctrl-p" cursor-up) ("Up" cursor-up)
        ("w" cursor-up    select-phase-navigate "cursor") ("a" cursor-left  select-phase-navigate "cursor")
        ("s" cursor-down  select-phase-navigate "cursor") ("d" cursor-right select-phase-navigate "cursor")
        ("W" cursor-top   select-phase-navigate) ("S" cursor-bottom select-phase-navigate)
        ("v" cycle-list-layout select-phase-navigate)
        ;; Shallow refine: rank the current result view (deep file-search, or
        ;; an earlier filter/refine) further, in place. See SEARCH_RENEW.md.
        ("f" select-refine (and select-phase-navigate select-has-view))
        ("Space" toggle-select select-phase-navigate "mark") ("q" cancel select-phase-navigate)))
    (policy #f (
        ("y" confirm "confirm") ("Enter" confirm "confirm") ("n" cancel "cancel") ("Esc" cancel "cancel")))
    (input-form #f (
        ("Enter" confirm "confirm") ("Esc" cancel "cancel")
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
        ("Backspace" remove-item input-form-list-browsing)))
    ;; CmdOp, the built-in command builder (doc/CMDOP.md).
    
    (file file (
        ("c" (ext "cmdop" "start-copy") "copy")
        ("m" (ext "cmdop" "start-move") "move")
        ("R" (ext "cmdop" "start-rename") "rename")
        ("M" (ext "cmdop" "start-create") "create")))
    ;; While the builder hosts the panel (cmdop-active), the function-panel keys
    ;; drive it. wasd still target the file panel (core bindings) so the live
    ;; sources follow the file cursor / marks.
    (file function (
        ("j" (ext "cmdop" "cursor-down") cmdop-active "cursor")
        ("Down" (ext "cmdop" "cursor-down") cmdop-active)
        ("k" (ext "cmdop" "cursor-up") cmdop-active "cursor")
        ("Up" (ext "cmdop" "cursor-up") cmdop-active)
        ("h" (ext "cmdop" "parent") cmdop-active "◂▸")
        ("Left" (ext "cmdop" "parent") cmdop-active)
        ("l" (ext "cmdop" "descend") cmdop-active "◂▸")
        ("Right" (ext "cmdop" "descend") cmdop-active)
        ("Enter" (ext "cmdop" "confirm") cmdop-active "run")
        ("Space" (ext "cmdop" "space") cmdop-active "mark")
        ("/" (ext "cmdop" "filter-start") cmdop-active)
        (";" (ext "cmdop" "section-toggle") cmdop-active)
        ("," cycle-function-panel cmdop-active)
        ("q" (ext "cmdop" "cancel") cmdop-active)
        ("Esc" (ext "cmdop" "cancel") cmdop-active "cancel")))))
