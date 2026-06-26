;; wasdf example extension — the glue manifest (declarations only, read as data).
;;
;; Drop this file AND the built library (libwasdf_example_ext.so / .dylib) into
;; the extensions directory ($WASDF_EXTENSIONS_DIR or ~/.config/wasdf/extensions).
;; The kernel discovers this .scm, reads the declarations below, and loads the
;; library named by (lib …) for the intent handlers. Edit the keys/commands here
;; without recompiling the library.
;;
;; The `(ext "example" "…")` intents are dispatched to the library's
;; wasdf_handle_intent over the C ABI (the manifest itself runs nothing).
(extension "example"
  (lib "wasdf_example_ext")
  (commands
    ((example-greet "example: greet via the extension's handle_intent" (ext "example" "greet"))))
  (resolvers
    ((example:echo #f ((native "echo" "hello-from-extension")))))
  (keymaps
    ((file file
      (("g" (ext "example" "greet"))
       ("t" (ext "example" "step")))))))
