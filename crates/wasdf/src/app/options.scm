;; Core option declarations for wasdf. Each form:
;;   (option <key> <category> <kind> <default> <label>)
;; Defaults must be byte-identical to Options::default() / MatcherConfig::default().
(quote (
  ;; Display
  (option "border-style"        "Display" (choice "single" "rounded" "double" "ascii") "single" "border-style")
  (option "content-auto-update" "Display" bool "#f" "content-auto-update")
  (option "file-row-size-graph" "Display" bool "#t" "file-row-size-graph")
  ;; Scrollbar glyphs: thumb (the moving block), rail (the track when scrollable),
  ;; and the char shown when scrolling is unnecessary (the panel fully fits).
  (option "scrollbar-thumb"     "Display" text "▋" "scrollbar-thumb")
  (option "scrollbar-rail"      "Display" text "│" "scrollbar-rail")
  (option "scrollbar-disabled"  "Display" text "│" "scrollbar-disabled")

  ;; Layout
  (option "top.nav-weight"      "Layout" (int 1 99) "5"  "nav-weight")
  (option "top.prop-visible"    "Layout" bool "#t" "prop-visible")
  (option "top.prop-weight"     "Layout" (int 1 99) "2"  "prop-weight")
  (option "top.monitor-visible" "Layout" bool "#t" "monitor-visible")
  (option "top.monitor-weight"  "Layout" (int 1 99) "2"  "monitor-weight")

  ;; Layout / Monitor — per-metric customization (served under Layout/Monitor/<Metric>).
  ;; bar-kind: bar (horizontal fill) | blocks (block sparkline) | braille (braille sparkline).
  (option "monitor.cpu.visible"   "Layout" bool "#t" "visible")
  (option "monitor.cpu.bar"       "Layout" bool "#t" "bar")
  (option "monitor.cpu.label"     "Layout" bool "#t" "label")
  (option "monitor.cpu.label-max" "Layout" (int 0 64) "5" "label-max")
  (option "monitor.cpu.graph-window" "Layout" (int 1 3600) "120" "graph-window-secs")
  (option "monitor.cpu.refresh"   "Layout" (int 1 60) "5"  "refresh-secs")
  (option "monitor.cpu.bar-kind"  "Layout" (choice "bar" "blocks" "braille") "braille" "bar-kind")
  (option "monitor.mem.visible"   "Layout" bool "#t" "visible")
  (option "monitor.mem.bar"       "Layout" bool "#t" "bar")
  (option "monitor.mem.label"     "Layout" bool "#t" "label")
  (option "monitor.mem.label-max" "Layout" (int 0 64) "5" "label-max")
  (option "monitor.mem.graph-window" "Layout" (int 1 3600) "120" "graph-window-secs")
  (option "monitor.mem.refresh"   "Layout" (int 1 60) "5"  "refresh-secs")
  (option "monitor.mem.bar-kind"  "Layout" (choice "bar" "blocks" "braille") "braille" "bar-kind")
  (option "monitor.dsk.visible"   "Layout" bool "#t" "visible")
  (option "monitor.dsk.bar"       "Layout" bool "#t" "bar")
  (option "monitor.dsk.label"     "Layout" bool "#t" "label")
  (option "monitor.dsk.label-max" "Layout" (int 0 64) "5" "label-max")
  (option "monitor.dsk.graph-window" "Layout" (int 1 3600) "120" "graph-window-secs")
  (option "monitor.dsk.refresh"   "Layout" (int 1 60) "5"  "refresh-secs")
  (option "monitor.dsk.bar-kind"  "Layout" (choice "bar" "blocks" "braille") "blocks" "bar-kind")
  (option "monitor.net.visible"   "Layout" bool "#t" "visible")
  (option "monitor.net.bar"       "Layout" bool "#t" "bar")
  (option "monitor.net.label"     "Layout" bool "#t" "label")
  (option "monitor.net.label-max" "Layout" (int 0 64) "5" "label-max")
  (option "monitor.net.graph-window" "Layout" (int 1 3600) "120" "graph-window-secs")
  (option "monitor.net.refresh"   "Layout" (int 1 60) "5"  "refresh-secs")
  (option "monitor.net.bar-kind"  "Layout" (choice "bar" "blocks" "braille") "braille" "bar-kind")

  ;; System
  ;; Retention cap for the @/messages diagnostic log, in lines.
  ;; Default must stay byte-identical to services::messages::DEFAULT_MAX_LINES.
  (option "max-message-line"      "System" (int 1 100000) "200" "max-message-line")

  ;; Matcher
  ;; Cap on raw entries a deep-search walk collects before ranking.
  ;; Default must stay byte-identical to services::matcher::MatcherConfig::default.
  (option "matcher.max-results"   "Matcher" (int 1 1000000) "65535" "max-results")
  (option "matcher.tail-bonus"    "Matcher" (int -999999 999999) "16"  "tail-bonus")
  (option "matcher.depth-penalty" "Matcher" (int -999999 999999) "4"   "depth-penalty")
  (option "matcher.length-penalty" "Matcher" (int -999999 999999) "1"  "length-penalty")
  (option "matcher.exec-bonus"    "Matcher" (int -999999 999999) "16"  "exec-bonus")
  (option "matcher.preferred-ext" "Matcher" list "rs,scm,toml,md,txt,sh,py,js,ts,json,c,h,go" "preferred-ext")
  (option "matcher.noise-ext"     "Matcher" list "lock,tmp,cache,log,o,d,rlib"                "noise-ext")
  (option "matcher.noise-dirs"    "Matcher" list "node_modules,.git,target,Caches,Library"    "noise-dirs")))
