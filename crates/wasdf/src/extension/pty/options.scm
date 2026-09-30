;; PTY extension options.
(quote (
  (option "pty.prefix-key"    "Pty" text                                "Ctrl-b" "prefix-key")
  (option "pty.shell"         "Pty" text                                ""       "shell")
  (option "pty.scrollback"    "Pty" (int 0 100000)                      2000     "scrollback")
  (option "pty.max-sessions"  "Pty" (int 1 64)                          8        "max-sessions")
  (option "pty.keep-exited"   "Pty" (int 0 32)                          4        "keep-exited")
  (option "pty.edit-host"     "Pty" bool                                #t      "run the editor on a PTY")
  (option "pty.close-on-exit" "Pty" (choice "never" "success" "always") "never"  "close-on-exit")))
