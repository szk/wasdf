(quote (
    (refresh         "reload the directory"      refresh)
    (toggle-dotfiles "show/hide hidden files"    toggle-dotfiles)
    (select-all      "select all entries"        select-all)
    (clear-selection "clear the selection"       clear-selection)
    ;; copy / move / rename / mkdir / touch are contributed by the cmdop
    ;; extension's commands() (mkdir/touch share its combined M builder).
    (delete          "delete selection/cursor"   delete-selected)
    (edit            "edit in $EDITOR"           start-edit)
    (home            "go to the VFS root"        (navigate-to "@"))
    (bookmarks       "open bookmarks"            (navigate-to "@/bookmarks"))
    (settings        "open settings"             (navigate-to "@/settings"))
    (add-bookmark    "bookmark current path"     (ext bookmarks add))
    (quit            "quit wasdf"                quit)))
