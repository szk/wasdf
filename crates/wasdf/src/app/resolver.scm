(quote (
    (copy   #f ((native "cp" "-R" opts paths dst)) (("-v" "verbose") ("-n" "no-clobber")))
    (move   #f ((native "mv" opts paths dst))      (("-v" "verbose") ("-n" "no-clobber")))
    (delete #t ((native "rm" "-rf" paths)))
    (rename #f ((native "mv" src dst)))
    (mkdir  #f ((native "mkdir" "-p" path)))
    (touch  #f ((native "touch" path)))
    (open   #f ((native-macos "open" path) (native-linux "xdg-open" path)))))
