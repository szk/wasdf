(quote (
    (refresh         "reload the directory"      refresh)
    (toggle-dotfiles "show/hide hidden files"    toggle-dotfiles)
    (select-all      "select all entries"        select-all)
    (clear-selection "clear the selection"       clear-selection)
    ;; CmdOp, the built-in command builder; mkdir and touch both open its
    ;; combined create flow (a trailing / picks mkdir).
    (copy            "copy selection/cursor"     (ext cmdop start-copy))
    (move            "move selection/cursor"     (ext cmdop start-move))
    (rename          "rename the cursor entry"   (ext cmdop start-rename))
    (mkdir           "make a directory (or touch a file — M)" (ext cmdop start-create))
    (touch           "create an empty file (or mkdir a directory — M)" (ext cmdop start-create))
    (delete          "delete selection/cursor"   delete-selected)
    (edit            "edit in $EDITOR"           start-edit)
    (home            "go to the VFS root"        (navigate-to "@"))
    (bookmarks       "open bookmarks"            (navigate-to "@/bookmarks"))
    (settings        "open settings"             (navigate-to "@/settings"))
    (add-bookmark    "bookmark current path"     (ext bookmarks add))
    (quit            "quit wasdf"                quit)))
