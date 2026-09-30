(quote (
    ;; `x` reuses the shell it last opened, `X` always makes a new one. `e`
    ;; overrides core's suspend-the-whole-TUI editor with one hosted in the
    ;; panel; the Extension layer outranks Core, and a user binding still
    ;; outranks both. Gated so turning the option off restores the core
    ;; behaviour rather than leaving `e` dead.
    (file file (
        ("x" (ext "pty" "shell") "shell")
        ("X" (ext "pty" "new-shell") "new shell")
        ("e" (ext "pty" "edit") (option "pty.edit-host") "edit")))
    ;; The same two keys from the function panel, so a released session can be
    ;; resumed or a second one opened without first stepping back to the file
    ;; panel. h/l are reclaimed there as well: horizontally scrolling a terminal
    ;; is meaningless, whereas stepping through the open sessions is not. Core's
    ;; own j/k already scroll the panel and w/a/s/d still drive the file cursor.
    (file function (
        ("x" (ext "pty" "shell") "shell")
        ("X" (ext "pty" "new-shell") "new shell")
        ("h" (ext "pty" "prev") pty-released "session")
        ("Left" (ext "pty" "prev") pty-released)
        ("l" (ext "pty" "next") pty-released "session")
        ("Right" (ext "pty" "next") pty-released)))))
