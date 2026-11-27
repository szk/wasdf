;; LS Colors extension options: the coloring source, the active palette, and
;; the user's own palettes (name -> raw LS_COLORS string). Presets live in
;; presets.scm.
(quote (
  (option "lscolors.coloring" "LsColors" (choice "none" "LS_COLORS" "Palette" "Themes") "Themes" "coloring")
  (option "lscolors.palette"  "LsColors" text "dark" "active palette")
  (option "lscolors.custom"   "LsColors" (map text) () "custom palettes")))
