;; Theme prelude: friendlier procedures over the theme.* options for config.scm
;; and the REPL. Every write goes through opt-set!, so it is validated and lands
;; in the caller's layer. (Forms load one at a time, so helpers come first.)

;; Replace (or append) `key`'s entry in an association list.
(define (theme--put key value pairs)
  (cond ((null? pairs) (list (list key value)))
        ((equal? (car (car pairs)) key) (cons (list key value) (cdr pairs)))
        (else (cons (car pairs) (theme--put key value (cdr pairs))))))

(define (theme--color c) (if (symbol? c) (symbol->string c) c))

;; (role fg bg bold italic underline) -> ("role" (("fg" …) ("bg" …) …))
(define (theme--style r)
  (list (symbol->string (car r))
        (list (list "fg" (theme--color (list-ref r 1)))
              (list "bg" (theme--color (list-ref r 2)))
              (list "bold" (list-ref r 3))
              (list "italic" (list-ref r 4))
              (list "underline" (list-ref r 5)))))

(define (theme--styles roles)
  (if (null? roles)
      '()
      (cons (theme--style (car roles)) (theme--styles (cdr roles)))))

(define (theme--define name roles)
  (opt-set! 'theme.custom (theme--put name (theme--styles roles) (opt 'theme.custom))))

;; (set-theme "name") — make a theme active.
(define (set-theme name) (opt-set! 'theme.active name))

;; (define-theme "name" (role fg bg bold italic underline) ...) — set a custom
;; theme's overrides; fg / bg are a color token string ("#83a598", an ANSI
;; name) or the bare symbol nil for no color.
(define-syntax define-theme
  (syntax-rules ()
    ((_ name role ...) (theme--define name '(role ...)))))
