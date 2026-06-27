;; Core option declarations for wasdf. Each form:
;;   (option <key> <category> <kind> <default> <label>)
;; Defaults must be byte-identical to Options::default() / MatcherConfig::default().
(quote (
  ;; Display
  (option "border-style"        "Display" (choice "single" "rounded" "double" "ascii") "single" "border-style")
  (option "content-auto-update" "Display" bool "#f" "content-auto-update")

  ;; Layout
  (option "top.nav-weight"      "Layout" (int 1 99) "5"  "nav-weight")
  (option "top.prop-visible"    "Layout" bool "#t" "prop-visible")
  (option "top.prop-weight"     "Layout" (int 1 99) "2"  "prop-weight")
  (option "top.monitor-visible" "Layout" bool "#t" "monitor-visible")
  (option "top.monitor-weight"  "Layout" (int 1 99) "2"  "monitor-weight")

  ;; Matcher
  (option "matcher.tail-bonus"    "Matcher" (int -999999 999999) "16"  "tail-bonus")
  (option "matcher.depth-penalty" "Matcher" (int -999999 999999) "4"   "depth-penalty")
  (option "matcher.length-penalty" "Matcher" (int -999999 999999) "1"  "length-penalty")
  (option "matcher.exec-bonus"    "Matcher" (int -999999 999999) "16"  "exec-bonus")
  (option "matcher.preferred-ext" "Matcher" list "rs,toml,md,txt,sh,py,js,ts,json,c,h,go" "preferred-ext")
  (option "matcher.noise-ext"     "Matcher" list "lock,tmp,cache,log,o,d,rlib"             "noise-ext")
  (option "matcher.noise-dirs"    "Matcher" list "node_modules,.git,target,Caches,Library" "noise-dirs")))
