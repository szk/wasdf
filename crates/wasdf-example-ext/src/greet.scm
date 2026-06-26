;; The `greet` reply: styled lines pushed into the function panel over the C ABI.
;; Fully static Scheme data — included into the library via include_str! so the
;; content lives as .scm, not as a Rust string literal. The GREET run carries
;; both fg and a bg (the 7-element run form), so the extension owns its colors
;; fully across the ABI.
((show-function-content "example"
  (lines
    ("GREET" (5 80 250 123 40 42 54))
    ("Rendered by wasdf-example-ext in the function panel.")
    ("This content crossed the C ABI as Scheme data.")
    ("Scroll with j / k. Search with /. Press t to step."))))
