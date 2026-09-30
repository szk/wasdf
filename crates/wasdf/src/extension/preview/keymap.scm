(quote (
    (file function (
        ("h" func-left) ("Left" func-left) ("l" func-right) ("Right" func-right)
        ("/" function-search-start sublayout-content "search")
        ("Enter" function-search-submit function-searching)
        ("Esc"   function-search-cancel function-searching)
        ("n" function-search-next function-has-matches "match")
        ("p" function-search-prev function-has-matches "match")))
    (file file (
        ("/" function-search-start (and function-visible sublayout-content) "search")
        ("n" function-search-next function-has-matches "match")
        ("p" function-search-prev function-has-matches "match")))))
