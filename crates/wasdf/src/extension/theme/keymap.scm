(quote (
    ;; The Color-in-panel editor (fg BarChart R/G/B + FG-nil toggle, bg
    ;; BarChart R/G/B + BG-nil toggle, Bold/Italic/Underline toggles, and a
    ;; live swatch) is cursor-follow: landing the file cursor on a custom
    ;; theme's role entry auto-opens it (ThemeExtension::on_cursor_changed),
    ;; hosted passively via Intent::HostFunctionForm — the file panel keeps
    ;; focus, so wasd keeps browsing the role list while hjkl edits the entry
    ;; currently under the cursor. No confirm/cancel: every edit writes
    ;; straight through.
    ;;
    ;; Scoped to the FILE panel (not "function"), overriding the core file
    ;; panel's own hjkl (func-up/down/left/right) while theme-color-active
    ;; holds — the function panel has no separate scrollable content to drive
    ;; while it's showing our editor, so repurposing those keys is exactly
    ;; the point (mirrors how FuncUp/Down/Left/Right already let the file
    ;; panel drive the function panel without a focus switch, just writable).
    ;;
    ;; j/k move the focused control through the full channel list (fg R/G/B,
    ;; FG nil, bg R/G/B, BG nil, Bold, Italic, Underline); h/l step the
    ;; focused control by 1 (or flip a toggle); H/L step by 10 (or also flip).
    (file file (
        ("j" (ext "theme" "channel-next") theme-color-active)
        ("Down" (ext "theme" "channel-next") theme-color-active)
        ("k" (ext "theme" "channel-prev") theme-color-active)
        ("Up" (ext "theme" "channel-prev") theme-color-active)
        ("h" (ext "theme" "color-dec") theme-color-active)
        ("Left" (ext "theme" "color-dec") theme-color-active)
        ("l" (ext "theme" "color-inc") theme-color-active)
        ("Right" (ext "theme" "color-inc") theme-color-active)
        ("H" (ext "theme" "color-dec-10") theme-color-active)
        ("L" (ext "theme" "color-inc-10") theme-color-active)))))
