;; The wasdf settings store (doc/SCHEME.md, The Settings Store).
;;
;; Loaded into the resident session when it starts. It is the single source of
;; truth for options, bindings, commands, display orders, and layouts. Every
;; entry holds one value per layer; the effective value is the one in the
;; highest non-empty layer. Writes go to the current layer, are validated
;; (options, against their declared kind), and are logged when they change an
;; effective value; the kernel collects that log as a change set after each
;; evaluation and applies it to its native projections.
;;
;; Internal names start with `store--`. The user vocabulary is: opt, opt-set!,
;; bind!, unbind!, command!, order!, layout!, customize!, reset!.

(define *store-layers* (list "session" "custom" "config" "default"))
(define *store-layer* "config")
(define *store-tables* (hash))   ; "section/layer" -> hash: id -> value
(define *store-kinds* (hash))    ; option key -> declared kind
(define *store-changes* '())     ; (section id) of changed entries, newest first

;; ── Tables ──────────────────────────────────────────────────────────────────

(define (store--table-name section layer) (string-append section "/" layer))

(define (store--table section layer)
  (let ((t (hash-try-get *store-tables* (store--table-name section layer))))
    (if t t (hash))))

(define (store--put! section layer id value)
  (set! *store-tables*
        (hash-insert *store-tables* (store--table-name section layer)
                     (hash-insert (store--table section layer) id value))))

(define (store--remove! section layer id)
  (let ((t (store--table section layer)))
    (if (hash-contains? t id)
        (set! *store-tables*
              (hash-insert *store-tables* (store--table-name section layer) (hash-remove t id)))
        #f)))

;; The effective value as a one-element list, or #f when no layer holds one.
(define (store--lookup section id layers)
  (cond ((null? layers) #f)
        ((hash-contains? (store--table section (car layers)) id)
         (list (hash-ref (store--table section (car layers)) id)))
        (else (store--lookup section id (cdr layers)))))

(define (store--effective section id) (store--lookup section id *store-layers*))

;; ── Change tracking ─────────────────────────────────────────────────────────

(define (store--note! section id before)
  (if (equal? before (store--effective section id))
      #f
      (set! *store-changes* (cons (list section id) *store-changes*))))

(define (store--write! section id value)
  (let ((before (store--effective section id)))
    (store--put! section *store-layer* id value)
    (store--note! section id before)))

(define (store--erase! section layer id)
  (let ((before (store--effective section id)))
    (store--remove! section layer id)
    (store--note! section id before)))

(define (store--unique xs seen)
  (cond ((null? xs) (reverse seen))
        ((member (car xs) seen) (store--unique (cdr xs) seen))
        (else (store--unique (cdr xs) (cons (car xs) seen)))))

;; The change set since the last call, oldest first, each entry once:
;; (section id #t value) when the entry has an effective value, else (section id #f).
(define (store-take-changes)
  (let ((ids (store--unique (reverse *store-changes*) '())))
    (set! *store-changes* '())
    (map (lambda (c)
           (let ((e (store--effective (car c) (car (cdr c)))))
             (if e
                 (list (car c) (car (cdr c)) #t (car e))
                 (list (car c) (car (cdr c)) #f))))
         ids)))

;; Set by the kernel around each file it loads. `*store-loading-custom*` is
;; true only while custom.scm itself loads — not for `customize!` at the REPL.
(define *store-loading-custom* #f)
(define (store-set-layer! layer)
  (set! *store-layer* layer)
  (set! *store-loading-custom* (equal? layer "custom"))
  layer)

;; Every entry of one layer, as (section id value), for the custom.scm dump.
(define (store-layer-entries layer)
  (let loop ((sections (list "option" "binding" "command" "order" "layout")) (out '()))
    (if (null? sections)
        (reverse out)
        (loop (cdr sections)
              (append (reverse
                       (map (lambda (id)
                              (list (car sections) id
                                    (hash-ref (store--table (car sections) layer) id)))
                            (hash-keys->list (store--table (car sections) layer))))
                      out)))))

;; ── Kinds ───────────────────────────────────────────────────────────────────

(define (store--name x) (if (symbol? x) (symbol->string x) x))

(define (store--all? pred xs)
  (cond ((null? xs) #t)
        ((pred (car xs)) (store--all? pred (cdr xs)))
        (else #f)))

(define (store--pair? p) (and (list? p) (= (length p) 2)))

;; Recursion over nested values goes through the named helpers below, never
;; through a lambda that calls its enclosing procedure: steel treats such a
;; call as the lambda's own tail self-call and fails with an arity error.

;; Record / map keys may be written as symbols; they are stored as strings.
(define (store--normalize kind v)
  (cond ((and (pair? kind) (list? v) (eq? (car kind) 'map))
         (store--normalize-pairs (lambda (name) (car (cdr kind))) v))
        ((and (pair? kind) (list? v) (eq? (car kind) 'record))
         (store--normalize-pairs (lambda (name) (store--field-kind (cdr kind) name)) v))
        ((and (pair? kind) (list? v) (eq? (car kind) 'list))
         (store--normalize-items (car (cdr kind)) v))
        (else v)))

(define (store--normalize-items kind xs)
  (if (null? xs)
      '()
      (cons (store--normalize kind (car xs)) (store--normalize-items kind (cdr xs)))))

(define (store--normalize-pairs kind-of pairs)
  (if (null? pairs)
      '()
      (cons (store--normalize-pair kind-of (car pairs))
            (store--normalize-pairs kind-of (cdr pairs)))))

(define (store--normalize-pair kind-of p)
  (if (store--pair? p)
      (let* ((name (store--name (car p)))
             (k (kind-of name)))
        (list name (if k (store--normalize k (car (cdr p))) (car (cdr p)))))
      p))

(define (store--field-kind fields name)
  (cond ((null? fields) #f)
        ((equal? (store--name (car (car fields))) name) (car (cdr (car fields))))
        (else (store--field-kind (cdr fields) name))))

(define (store--accepts? kind v)
  (cond ((eq? kind 'bool) (boolean? v))
        ((eq? kind 'text) (string? v))
        ((not (pair? kind)) #f)
        ((eq? (car kind) 'int)
         (and (integer? v) (>= v (car (cdr kind))) (<= v (car (cdr (cdr kind))))))
        ((eq? (car kind) 'choice) (and (string? v) (if (member v (cdr kind)) #t #f)))
        ((eq? (car kind) 'list) (and (list? v) (store--items-ok? (car (cdr kind)) v)))
        ((eq? (car kind) 'map) (and (list? v) (store--map-ok? (car (cdr kind)) v)))
        ((eq? (car kind) 'record)
         (and (list? v) (= (length v) (length (cdr kind))) (store--record-ok? (cdr kind) v)))
        (else #f)))

(define (store--items-ok? kind xs)
  (cond ((null? xs) #t)
        ((store--accepts? kind (car xs)) (store--items-ok? kind (cdr xs)))
        (else #f)))

(define (store--map-ok? kind pairs)
  (cond ((null? pairs) #t)
        ((and (store--pair? (car pairs))
              (string? (car (car pairs)))
              (store--accepts? kind (car (cdr (car pairs)))))
         (store--map-ok? kind (cdr pairs)))
        (else #f)))

(define (store--record-ok? fields pairs)
  (cond ((null? pairs) #t)
        ((not (store--pair? (car pairs))) #f)
        (else
         (let ((fk (store--field-kind fields (car (car pairs)))))
           (if (and fk (store--accepts? fk (car (cdr (car pairs)))))
               (store--record-ok? fields (cdr pairs))
               #f)))))

;; Declare an option: its kind and its default-layer value. Not a change.
(define (store-declare-option! key kind default)
  (set! *store-kinds* (hash-insert *store-kinds* key kind))
  (store--put! "option" "default" key (store--normalize kind default))
  key)

;; ── The user vocabulary ─────────────────────────────────────────────────────

(define (opt key)
  (let ((e (store--effective "option" (store--name key))))
    (if e (car e) (error (string-append "opt: undeclared option " (store--name key))))))

(define (opt-set! key value)
  (let* ((id (store--name key))
         (kind (hash-try-get *store-kinds* id)))
    (cond
      ;; custom.scm may name an option nobody declares this session (its
      ;; extension is disabled or gone): keep it as written, unvalidated and
      ;; without effect, so the dump writes it back and it returns with its
      ;; extension. Anywhere else an undeclared key is a mistake.
      ((and (not kind) *store-loading-custom*)
       (store--put! "option" "custom" id value))
      ((not kind)
        (error (string-append "opt-set!: undeclared option " id)))
      (else
        (let ((v (store--normalize kind value)))
          (if (store--accepts? kind v)
              (store--write! "option" id v)
              (error (string-append "opt-set!: " id ": value does not fit its kind " (to-string kind)))))))))

(define (store--binding-id mode panel key)
  (if (string? key)
      (list (store--name mode) (if panel (store--name panel) #f) key)
      (error "bind!: the key must be a string, e.g. \"Ctrl-a\"")))

(define (store--find pred xs)
  (cond ((null? xs) #f)
        ((pred (car xs)) (car xs))
        (else (store--find pred (cdr xs)))))

;; (bind! mode panel "key" intent [when] ["label"]) — after the intent, a string
;; is the help label and anything else is the :when condition.
(define (bind! mode panel key intent . rest)
  (store--write! "binding" (store--binding-id mode panel key)
                 (list intent
                       (store--find (lambda (x) (not (string? x))) rest)
                       (store--find string? rest))))

;; A tombstone: hides the binding in the layers below.
(define (unbind! mode panel key)
  (store--write! "binding" (store--binding-id mode panel key) 'unbound))

(define (command! name description intent)
  (if (string? description)
      (store--write! "command" (store--name name) (list description intent))
      (error "command!: the description must be a string")))

(define (order! name items)
  (if (list? items)
      (store--write! "order" (store--name name) (map store--name items))
      (error "order!: the items must be a list")))

;; Checked natively against the panel catalog (`layout-problem`, registered
;; by the kernel); a rejected tree leaves the previous layout in place.
(define (layout! id tree)
  (let ((problem (layout-problem (store--name id) (store--write tree))))
    (if (equal? problem "")
        (store--write! "layout" (store--name id) tree)
        (error (string-append "layout!: " problem)))))

(define (store--with-layer layer thunk)
  (let ((saved *store-layer*))
    (set! *store-layer* layer)
    (let ((r (thunk)))
      (set! *store-layer* saved)
      r)))

;; (customize! expr …) — evaluate with the persisted `custom` layer current.
(define-syntax customize!
  (syntax-rules ()
    ((_ e ...) (store--with-layer "custom" (lambda () e ...)))))

;; (reset! 'option-key) or (reset! 'mode 'panel "key"): clear the custom value.
(define (reset! . args)
  (cond ((= (length args) 1) (store--erase! "option" "custom" (store--name (car args))))
        ((= (length args) 3)
         (store--erase! "binding" "custom"
                        (store--binding-id (car args) (car (cdr args)) (car (cdr (cdr args))))))
        (else (error "reset!: expected (reset! 'key) or (reset! 'mode 'panel \"key\")"))))

;; ── custom.scm ──────────────────────────────────────────────────────────────
;;
;; The `custom` layer written out as literal store calls, one per entry, sorted
;; so the file only changes where a setting did. The result is both a store
;; program (loaded with `custom` current) and plain data for the Rust reader,
;; which parses it before the session is ready and as the fallback.

(define (store--escape-chars cs)
  (cond ((null? cs) '())
        ((char=? (car cs) #\") (cons #\\ (cons #\" (store--escape-chars (cdr cs)))))
        ((char=? (car cs) #\\) (cons #\\ (cons #\\ (store--escape-chars (cdr cs)))))
        ((char=? (car cs) #\newline) (cons #\\ (cons #\n (store--escape-chars (cdr cs)))))
        ((char=? (car cs) #\tab) (cons #\\ (cons #\t (store--escape-chars (cdr cs)))))
        (else (cons (car cs) (store--escape-chars (cdr cs))))))

(define (store--write-string s)
  (string-append "\"" (list->string (store--escape-chars (string->list s))) "\""))

;; A datum as source text.
(define (store--write v)
  (cond ((string? v) (store--write-string v))
        ((boolean? v) (if v "#t" "#f"))
        ((integer? v) (number->string v))
        ((symbol? v) (symbol->string v))
        ((null? v) "()")
        ((list? v) (string-append "(" (store--write-items v) ")"))
        (else (to-string v))))

(define (store--write-items xs)
  (cond ((null? xs) "")
        ((null? (cdr xs)) (store--write (car xs)))
        (else (string-append (store--write (car xs)) " " (store--write-items (cdr xs))))))

;; A datum as a literal argument: symbols and lists are quoted.
(define (store--literal v)
  (if (or (symbol? v) (pair? v) (null? v))
      (string-append "'" (store--write v))
      (store--write v)))

(define (store--binding-head id)
  (string-append "'" (car id) " "
                 (if (car (cdr id)) (string-append "'" (car (cdr id))) "#f") " "
                 (store--write-string (car (cdr (cdr id))))))

(define (store--binding-call id value)
  (if (eq? value 'unbound)
      (string-append "(unbind! " (store--binding-head id) ")")
      (string-append "(bind! " (store--binding-head id) " " (store--literal (car value))
                     (if (car (cdr value)) (string-append " " (store--literal (car (cdr value)))) "")
                     (if (car (cdr (cdr value))) (string-append " " (store--write-string (car (cdr (cdr value))))) "")
                     ")")))

(define (store--entry-call section id value)
  (cond ((equal? section "option") (string-append "(opt-set! '" id " " (store--literal value) ")"))
        ((equal? section "binding") (store--binding-call id value))
        ((equal? section "command")
         (string-append "(command! '" id " " (store--write-string (car value)) " "
                        (store--literal (car (cdr value))) ")"))
        ((equal? section "order") (string-append "(order! '" id " " (store--literal value) ")"))
        ((equal? section "layout") (string-append "(layout! '" id " " (store--literal value) ")"))
        (else "")))

(define (store--insert-sorted s xs)
  (cond ((null? xs) (list s))
        ((string<? s (car xs)) (cons s xs))
        (else (cons (car xs) (store--insert-sorted s (cdr xs))))))

(define (store--sort-strings xs)
  (if (null? xs) '() (store--insert-sorted (car xs) (store--sort-strings (cdr xs)))))

(define (store--entry-calls entries)
  (if (null? entries)
      '()
      (let ((e (car entries)))
        (cons (store--entry-call (car e) (car (cdr e)) (car (cdr (cdr e))))
              (store--entry-calls (cdr entries))))))

(define (store--join-lines xs)
  (if (null? xs) "" (string-append (car xs) "\n" (store--join-lines (cdr xs)))))

(define (store-dump-custom)
  (string-append
   ";; custom.scm — the settings you changed in wasdf (the settings UI, editors,\n"
   ";; and (customize! …) at the REPL). Generated by wasdf; your own configuration\n"
   ";; belongs in config.scm, and anything set here overrides it.\n"
   (store--join-lines (store--sort-strings (store--entry-calls (store-layer-entries "custom"))))))
