;; The core layouts (doc/SCHEME.md, Role 3: Layouter): (id tree). `layout!`
;; in config.scm, custom.scm, or at the REPL replaces one. The top row is 7
;; rows tall, 5 in the Compact size class (under 25 rows); its panels give way
;; from the right when their minimum widths do not fit.
(quote (
  (file
    (column
      (row (size 7 5)
        (panel nav     (weight (option "top.nav-weight"))     (min 22))
        (panel prop    (weight (option "top.prop-weight"))    (min 18) (when (option "top.prop-visible")))
        (panel monitor (weight (option "top.monitor-weight")) (min 16) (when (option "top.monitor-visible"))))
      (row
        (panel file     (ratio left))
        (panel function (ratio right) (when function-visible)))
      (panel help (size 1))))
  (select
    (column
      (row (size 7 5)
        (panel nav     (weight (option "top.nav-weight"))     (min 22))
        (panel prop    (weight (option "top.prop-weight"))    (min 18) (when (option "top.prop-visible")))
        (panel monitor (weight (option "top.monitor-weight")) (min 16) (when (option "top.monitor-visible"))))
      (row
        (column (ratio left)
          (panel select-input (size 3) (when select-has-input))
          (panel select-candidates))
        (panel function (ratio right) (when select-has-function)))
      (panel help (size 1))))))
