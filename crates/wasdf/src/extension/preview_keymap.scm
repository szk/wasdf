(quote (
    (file function (
        ("h" func-left) ("Left" func-left) ("l" func-right) ("Right" func-right)
        ("/" function-search-start sublayout-content)
        ("Enter" function-search-submit function-searching)
        ("Esc"   function-search-cancel function-searching)
        ("n" function-search-next function-has-matches)
        ("p" function-search-prev function-has-matches)))
    (file file (
        ("/" function-search-start (and function-visible sublayout-content))
        ("n" function-search-next function-has-matches)
        ("p" function-search-prev function-has-matches)))))
