;; LS Colors prelude: friendlier procedures over the lscolors.* options for
;; config.scm and the REPL. (Forms load one at a time, so helpers come first.)

;; Replace (or append) `key`'s entry in an association list.
(define (lscolors--put key value pairs)
  (cond ((null? pairs) (list (list key value)))
        ((equal? (car (car pairs)) key) (cons (list key value) (cdr pairs)))
        (else (cons (car pairs) (lscolors--put key value (cdr pairs))))))

(define (lscolors--define name raw)
  (opt-set! 'lscolors.custom (lscolors--put name raw (opt 'lscolors.custom))))

;; (set-lscolors-palette "name") — make a palette active.
(define (set-lscolors-palette name) (opt-set! 'lscolors.palette name))

;; (define-lscolors-palette "name" (raw "di=01;34:*.rs=00;33:…")) — set a
;; custom palette's LS_COLORS string.
(define-syntax define-lscolors-palette
  (syntax-rules ()
    ((_ name (field raw)) (lscolors--define name raw))))
