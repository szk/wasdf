;; exampleext — glue manifest for the sibling library (declarations only, data).
;;
;; Deploy this file and the built library (libexampleext.so / .dylib) into the
;; extension's own folder: ~/.config/wasdf/extensions/exampleext/init.scm
;; (+ libexampleext.…). The kernel discovers init.scm, reads the declarations,
;; and loads the (lib …) library for the intent handlers.
;;
;; Behavior: `o` in the file panel TOGGLES a cursor-info feature on the function
;; panel. ON pushes an overlay mode (wasd still navigates the file list beneath)
;; and shows the cursor entry's info (name / path / type / size / perms) as a
;; logical document core lays out, following the cursor; OFF pops that mode and
;; the previewer reclaims the panel (the pre-ON state). Every toggle notifies
;; `exampleext.greeting` with a `: ON` / `: OFF` suffix. All of it lives in the
;; library (src/lib.rs), not the kernel: it records the cursor path and (while ON)
;; pushes info from wasdf_on_cursor_changed, and flips its ON/OFF flag in
;; wasdf_handle_intent("show-info"), which is handed this extension's live option
;; values (its greeting) over the ABI.
;;
;; The (options …) settings appear editable + persisted under
;; @/settings/Extensions/exampleext, equivalent to a bundled extension's — so
;; editing `greeting` there changes the notified text.
;; `o` is bound on both the file and function panels: turning the feature ON
;; focuses the function panel, so the next `o` must toggle it off from there too.
(extension "exampleext"
  (lib "exampleext")
  (keymaps ((file file     (("o" (ext "exampleext" "show-info"))))
            (file function (("o" (ext "exampleext" "show-info"))))))
  (options ((option "exampleext.greeting" "exampleext" text "hello" "greeting")
            (option "exampleext.verbose"  "exampleext" bool #f     "verbose"))))
