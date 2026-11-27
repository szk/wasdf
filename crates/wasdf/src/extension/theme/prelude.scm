;; Theme prelude: friendlier procedures over the theme.* options for config.scm
;; and the REPL. Every write goes through opt-set!, so it is validated and lands
;; in the caller's layer. (Forms load one at a time, so helpers come first.)

;; Replace (or append) `key`'s entry in an association list.
(define (theme--put key value pairs)
  (cond ((null? pairs) (list (list key value)))
        ((equal? (car (car pairs)) key) (cons (list key value) (cdr pairs)))
        (else (cons (car pairs) (theme--put key value (cdr pairs))))))

;; (role (fg C) (bg C) (bold B) ...) -> ("role" ((fg C) (bg C) (bold B) ...)):
;; the store keeps the fields, any subset, in declaration order.
(define (theme--style r)
  (list (symbol->string (car r)) (cdr r)))

(define (theme--styles roles)
  (if (null? roles)
      '()
      (cons (theme--style (car roles)) (theme--styles (cdr roles)))))

(define (theme--define name roles)
  (opt-set! 'theme.custom (theme--put name (theme--styles roles) (opt 'theme.custom))))

;; (set-theme "name") — make a theme active.
(define (set-theme name) (opt-set! 'theme.active name))

;; (define-theme "name" (role (fg C) (bg C) (bold B) (italic B) (underline B)) ...)
;; — set a custom theme's overrides, written like presets.scm: every field is
;; optional (an omitted color is nil, an omitted modifier off); a color is a
;; token string ("#83a598", an ANSI name).
(define-syntax define-theme
  (syntax-rules ()
    ((_ name role ...) (theme--define name '(role ...)))))
