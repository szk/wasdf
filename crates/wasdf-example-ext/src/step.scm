;; The `step` reply template, included into the library via include_str!. `{n}`
;; is replaced with the current step count before the datum crosses the C ABI.
;; update-function-view stores the count as opaque kernel view state; the lines
;; reflect it back.
((update-function-view {n})
 (show-function-content "example"
  (lines
    ("step: {n}")
    ("Press t again to advance; the view state is stored in the kernel."))))
