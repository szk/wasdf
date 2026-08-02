(quote (
    ;; Entry keys (file panel): open the builder.
    (file file (
        ("c" (ext "cmdop" "start-copy"))
        ("m" (ext "cmdop" "start-move"))
        ("R" (ext "cmdop" "start-rename"))
        ("M" (ext "cmdop" "start-create"))))
    ;; While the builder hosts the panel (cmdop-active), the function-panel keys
    ;; drive it. wasd still target the file panel (core bindings) so the live
    ;; sources follow the file cursor / marks.
    (file function (
        ("j" (ext "cmdop" "cursor-down") cmdop-active)
        ("Down" (ext "cmdop" "cursor-down") cmdop-active)
        ("k" (ext "cmdop" "cursor-up") cmdop-active)
        ("Up" (ext "cmdop" "cursor-up") cmdop-active)
        ("h" (ext "cmdop" "parent") cmdop-active)
        ("Left" (ext "cmdop" "parent") cmdop-active)
        ("l" (ext "cmdop" "descend") cmdop-active)
        ("Right" (ext "cmdop" "descend") cmdop-active)
        ("Enter" (ext "cmdop" "confirm") cmdop-active)
        ("Space" (ext "cmdop" "space") cmdop-active)
        ("/" (ext "cmdop" "filter-start") cmdop-active)
        (";" (ext "cmdop" "section-toggle") cmdop-active)
        ("," cycle-function-panel cmdop-active)
        ("q" (ext "cmdop" "cancel") cmdop-active)
        ("Esc" (ext "cmdop" "cancel") cmdop-active)))))
