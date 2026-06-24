//! A small s-expression reader. The Scheme session evaluates config forms and
//! prints canonical data with `write`; this reader parses that output back into
//! Rust. It belongs to the script layer's codec role (the single owner of the
//! datum string form). It reads canonical `write` output only — no quoting,
//! quasiquote, or dotted pairs are needed.

/// A parsed datum.
#[derive(Debug, Clone, PartialEq)]
pub enum Datum {
    Sym(String),
    Str(String),
    Int(i64),
    Bool(bool),
    List(Vec<Datum>),
}

impl Datum {
    pub fn as_sym(&self) -> Option<&str> {
        match self {
            Datum::Sym(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Datum::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Datum::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn as_list(&self) -> Option<&[Datum]> {
        match self {
            Datum::List(v) => Some(v),
            _ => None,
        }
    }
    /// A string literal or a bare symbol, as text (argv elements may be either).
    pub fn text(&self) -> Option<&str> {
        match self {
            Datum::Str(s) | Datum::Sym(s) => Some(s),
            _ => None,
        }
    }
}

/// Render a datum back to canonical s-expression text (used to generate the
/// commented keybindings template from the embedded keymap source).
pub fn render(d: &Datum) -> String {
    match d {
        Datum::Sym(s) => s.clone(),
        Datum::Str(s) => format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")),
        Datum::Int(i) => i.to_string(),
        Datum::Bool(true) => "#t".into(),
        Datum::Bool(false) => "#f".into(),
        Datum::List(items) => {
            let inner: Vec<String> = items.iter().map(render).collect();
            format!("({})", inner.join(" "))
        }
    }
}

/// Parse a single datum from the input.
pub fn parse(input: &str) -> Result<Datum, String> {
    let mut r = Reader { chars: input.chars().peekable() };
    let d = r.read_datum()?.ok_or("empty input")?;
    Ok(d)
}

struct Reader<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
}

impl Reader<'_> {
    fn skip_ws(&mut self) {
        while let Some(&c) = self.chars.peek() {
            if c == ';' {
                while let Some(&c) = self.chars.peek() {
                    self.chars.next();
                    if c == '\n' {
                        break;
                    }
                }
            } else if c.is_whitespace() {
                self.chars.next();
            } else {
                break;
            }
        }
    }

    fn read_datum(&mut self) -> Result<Option<Datum>, String> {
        self.skip_ws();
        match self.chars.peek().copied() {
            None => Ok(None),
            Some('(') => self.read_list().map(Some),
            Some(')') => Err("unexpected )".into()),
            Some('"') => self.read_string().map(Some),
            Some(_) => self.read_atom().map(Some),
        }
    }

    fn read_list(&mut self) -> Result<Datum, String> {
        self.chars.next(); // consume '('
        let mut items = Vec::new();
        loop {
            self.skip_ws();
            match self.chars.peek().copied() {
                None => return Err("unterminated list".into()),
                Some(')') => {
                    self.chars.next();
                    return Ok(Datum::List(items));
                }
                Some(_) => {
                    if let Some(d) = self.read_datum()? {
                        items.push(d);
                    }
                }
            }
        }
    }

    fn read_string(&mut self) -> Result<Datum, String> {
        self.chars.next(); // consume opening quote
        let mut s = String::new();
        while let Some(c) = self.chars.next() {
            match c {
                '"' => return Ok(Datum::Str(s)),
                '\\' => match self.chars.next() {
                    Some('n') => s.push('\n'),
                    Some('t') => s.push('\t'),
                    Some('\\') => s.push('\\'),
                    Some('"') => s.push('"'),
                    Some(other) => s.push(other),
                    None => return Err("unterminated escape".into()),
                },
                _ => s.push(c),
            }
        }
        Err("unterminated string".into())
    }

    fn read_atom(&mut self) -> Result<Datum, String> {
        let mut s = String::new();
        while let Some(&c) = self.chars.peek() {
            if c.is_whitespace() || c == '(' || c == ')' {
                break;
            }
            s.push(c);
            self.chars.next();
        }
        Ok(match s.as_str() {
            "#t" | "#true" => Datum::Bool(true),
            "#f" | "#false" => Datum::Bool(false),
            _ => match s.parse::<i64>() {
                Ok(i) => Datum::Int(i),
                Err(_) => Datum::Sym(s),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_nested_lists_and_atoms() {
        let d = parse("(copy #f ((native \"cp\" \"-R\" paths dst)))").unwrap();
        let top = d.as_list().unwrap();
        assert_eq!(top[0].as_sym(), Some("copy"));
        assert_eq!(top[1].as_bool(), Some(false));
        let cands = top[2].as_list().unwrap();
        let cand = cands[0].as_list().unwrap();
        assert_eq!(cand[0].as_sym(), Some("native"));
        assert_eq!(cand[1].as_str(), Some("cp"));
        assert_eq!(cand[4].as_sym(), Some("dst"));
    }

    #[test]
    fn skips_comments_and_whitespace() {
        let d = parse("  ; a comment\n  (a 1 #t)\n").unwrap();
        assert_eq!(
            d,
            Datum::List(vec![Datum::Sym("a".into()), Datum::Int(1), Datum::Bool(true)])
        );
    }
}
