;; The `cursor-changed` reply template, included into the library via
;; include_str!. `{path}` is replaced (already escaped for Scheme) with the path
;; now under the cursor before the datum crosses the C ABI.
((show-function-content "example"
  (lines
    ("cursor-changed →" (13 80 250 123))
    ("{path}")
    ("This line was pushed by wasdf-example-ext reacting to the")
    ("cursor-changed event over the C ABI (no kernel edits)."))))
