;; Theme extension options: the active theme, and the user's own themes
;; (theme name -> role token -> style). Presets live in presets.scm.
(quote (
  (option "theme.active" "Theme" text "default" "active theme")
  (option "theme.custom" "Theme"
          (map (map (record (fg text) (bg text) (bold bool) (italic bool) (underline bool))))
          () "custom themes")))
