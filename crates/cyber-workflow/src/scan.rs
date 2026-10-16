//! A small JavaScript tokenizer: enough lexical structure to extract a pure-literal `meta`
//! and find forbidden imports without a full parser. Strings, template literals (with
//! nested substitutions), regular expressions and comments are skipped correctly, so a
//! `require(` inside a string is not reported and one inside `${...}` is.

use crate::diagnostic::{Diagnostic, Position};

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Ident(String),
    Punct(char),
    Str(String),
    /// A template literal; `None` when it has substitutions.
    Template(Option<String>),
    Number(String),
    Regex,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub tok: Tok,
    pub pos: Position,
    /// Line of the token's last character.
    pub end_line: u32,
}

/// Keywords after which `/` starts a regular expression rather than a division.
const REGEX_AFTER: &[&str] = &[
    "return",
    "typeof",
    "case",
    "do",
    "else",
    "in",
    "instanceof",
    "new",
    "delete",
    "void",
    "throw",
    "yield",
    "await",
    "of",
];

struct Lexer {
    chars: Vec<char>,
    i: usize,
    line: u32,
    column: u32,
    tokens: Vec<Token>,
    /// Open `${` substitutions, each with its inner brace depth.
    templates: Vec<u32>,
}

/// Tokenize a script, failing with a `ParseError` on unterminated literals and comments.
pub fn tokenize(source: &str) -> Result<Vec<Token>, Diagnostic> {
    let mut lexer = Lexer {
        chars: source.chars().collect(),
        i: 0,
        line: 1,
        column: 1,
        tokens: Vec::new(),
        templates: Vec::new(),
    };
    while lexer.skip_trivia()? {
        lexer.token()?;
    }
    if !lexer.templates.is_empty() {
        return Err(Diagnostic::parse(
            "unterminated template literal",
            lexer.pos(),
        ));
    }
    Ok(lexer.tokens)
}

fn ident_start(c: char) -> bool {
    c == '_' || c == '$' || c.is_alphabetic()
}

fn ident_part(c: char) -> bool {
    ident_start(c) || c.is_alphanumeric() || c == '\u{200c}' || c == '\u{200d}'
}

impl Lexer {
    fn peek(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.i + ahead).copied()
    }

    fn pos(&self) -> Position {
        Position {
            line: self.line,
            column: self.column,
        }
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek(0)?;
        self.i += 1;
        if c == '\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        Some(c)
    }

    fn push(&mut self, tok: Tok, pos: Position) {
        let end_line = self.line;
        self.tokens.push(Token { tok, pos, end_line });
    }

    /// Skip whitespace and comments; false at end of input.
    fn skip_trivia(&mut self) -> Result<bool, Diagnostic> {
        loop {
            match (self.peek(0), self.peek(1)) {
                (None, _) => return Ok(false),
                (Some('/'), Some('/')) => {
                    while self.peek(0).is_some_and(|c| c != '\n') {
                        self.bump();
                    }
                }
                (Some('/'), Some('*')) => self.block_comment()?,
                (Some(c), _) if c.is_whitespace() => {
                    self.bump();
                }
                _ => return Ok(true),
            }
        }
    }

    fn block_comment(&mut self) -> Result<(), Diagnostic> {
        let start = self.pos();
        self.bump();
        self.bump();
        loop {
            match self.bump() {
                None => return Err(Diagnostic::parse("unterminated comment", start)),
                Some('*') if self.peek(0) == Some('/') => {
                    self.bump();
                    return Ok(());
                }
                _ => {}
            }
        }
    }

    fn token(&mut self) -> Result<(), Diagnostic> {
        let pos = self.pos();
        let c = self.peek(0).unwrap_or_default();
        match c {
            '"' | '\'' => {
                let value = self.string(c)?;
                self.push(Tok::Str(value), pos);
            }
            '`' => {
                self.bump();
                self.template(pos)?;
            }
            '/' if self.regex_allowed() => self.regex(pos)?,
            c if c.is_ascii_digit()
                || (c == '.' && self.peek(1).is_some_and(|d| d.is_ascii_digit())) =>
            {
                self.number(pos)
            }
            c if ident_start(c) || c == '\\' => self.ident(pos),
            _ => self.punct(c, pos)?,
        }
        Ok(())
    }

    fn punct(&mut self, c: char, pos: Position) -> Result<(), Diagnostic> {
        self.bump();
        match c {
            '{' => {
                if let Some(depth) = self.templates.last_mut() {
                    *depth += 1;
                }
            }
            '}' => match self.templates.last_mut() {
                Some(0) => {
                    self.templates.pop();
                    return self.template_rest(pos);
                }
                Some(depth) => *depth -= 1,
                None => {}
            },
            _ => {}
        }
        self.push(Tok::Punct(c), pos);
        Ok(())
    }

    fn regex_allowed(&self) -> bool {
        match self.tokens.last().map(|t| &t.tok) {
            None => true,
            Some(Tok::Punct(c)) => !matches!(c, ')' | ']'),
            Some(Tok::Ident(word)) => REGEX_AFTER.contains(&word.as_str()),
            Some(_) => false,
        }
    }

    fn regex(&mut self, pos: Position) -> Result<(), Diagnostic> {
        self.bump();
        let mut class = false;
        loop {
            match self.bump() {
                None | Some('\n') => {
                    return Err(Diagnostic::parse("unterminated regular expression", pos));
                }
                Some('\\') => {
                    self.bump();
                }
                Some('[') => class = true,
                Some(']') => class = false,
                Some('/') if !class => break,
                Some(_) => {}
            }
        }
        while self.peek(0).is_some_and(ident_part) {
            self.bump();
        }
        self.push(Tok::Regex, pos);
        Ok(())
    }

    fn number(&mut self, pos: Position) {
        let mut text = String::new();
        while let Some(c) = self.peek(0) {
            let exponent_sign = matches!(c, '+' | '-')
                && matches!(text.chars().last(), Some('e' | 'E'))
                && !text.starts_with("0x")
                && !text.starts_with("0X");
            if !(c.is_ascii_alphanumeric() || c == '.' || c == '_' || exponent_sign) {
                break;
            }
            text.push(c);
            self.bump();
        }
        self.push(Tok::Number(text), pos);
    }

    fn ident(&mut self, pos: Position) {
        let mut text = String::new();
        while let Some(c) = self.peek(0) {
            if !(ident_part(c) || c == '\\') {
                break;
            }
            text.push(c);
            self.bump();
        }
        self.push(Tok::Ident(text), pos);
    }

    fn string(&mut self, quote: char) -> Result<String, Diagnostic> {
        let start = self.pos();
        self.bump();
        let mut value = String::new();
        loop {
            match self.bump() {
                None | Some('\n') => {
                    return Err(Diagnostic::parse("unterminated string literal", start));
                }
                Some('\\') => self.escape(&mut value, start)?,
                Some(c) if c == quote => return Ok(value),
                Some(c) => value.push(c),
            }
        }
    }

    /// Template text after the opening backtick or a closing `}` of a substitution.
    fn template(&mut self, pos: Position) -> Result<(), Diagnostic> {
        let mut value = String::new();
        loop {
            match self.bump() {
                None => return Err(Diagnostic::parse("unterminated template literal", pos)),
                Some('\\') => self.escape(&mut value, pos)?,
                Some('`') => {
                    self.push(Tok::Template(Some(value)), pos);
                    return Ok(());
                }
                Some('$') if self.peek(0) == Some('{') => {
                    self.bump();
                    self.push(Tok::Template(None), pos);
                    self.templates.push(0);
                    return Ok(());
                }
                Some(c) => value.push(c),
            }
        }
    }

    /// The remainder of a template after a substitution; it belongs to the same literal.
    fn template_rest(&mut self, pos: Position) -> Result<(), Diagnostic> {
        loop {
            match self.bump() {
                None => return Err(Diagnostic::parse("unterminated template literal", pos)),
                Some('\\') => {
                    self.bump();
                }
                Some('`') => return Ok(()),
                Some('$') if self.peek(0) == Some('{') => {
                    self.bump();
                    self.templates.push(0);
                    return Ok(());
                }
                Some(_) => {}
            }
        }
    }

    fn escape(&mut self, value: &mut String, start: Position) -> Result<(), Diagnostic> {
        let invalid = || Diagnostic::parse("invalid escape sequence", start);
        let c = self.bump().ok_or_else(invalid)?;
        let decoded = match c {
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            'b' => '\u{8}',
            'f' => '\u{c}',
            'v' => '\u{b}',
            '0' if !self.peek(0).is_some_and(|d| d.is_ascii_digit()) => '\0',
            '\n' => return Ok(()),
            '\r' => {
                if self.peek(0) == Some('\n') {
                    self.bump();
                }
                return Ok(());
            }
            'x' => self.hex(2).ok_or_else(invalid)?,
            'u' => self.unicode().ok_or_else(invalid)?,
            c if c.is_ascii_digit() => return Err(invalid()),
            c => c,
        };
        value.push(decoded);
        Ok(())
    }

    fn hex(&mut self, digits: usize) -> Option<char> {
        let mut code = 0u32;
        for _ in 0..digits {
            code = code * 16 + self.bump()?.to_digit(16)?;
        }
        char::from_u32(code)
    }

    fn unicode(&mut self) -> Option<char> {
        if self.peek(0) != Some('{') {
            return self.hex(4);
        }
        self.bump();
        let mut code = 0u32;
        loop {
            match self.bump()? {
                '}' => return char::from_u32(code),
                c => code = code.checked_mul(16)?.checked_add(c.to_digit(16)?)?,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<Tok> {
        tokenize(source)
            .unwrap()
            .into_iter()
            .map(|t| t.tok)
            .collect()
    }

    #[test]
    fn strings_comments_and_regexes_hide_their_contents() {
        let toks = kinds("// require('a')\n/* import('b') */ x = '\\'require(' + /re\"q/g;");
        assert!(toks.contains(&Tok::Str("'require(".into())));
        assert!(toks.contains(&Tok::Regex));
        assert!(!toks.contains(&Tok::Ident("require".into())));
    }

    #[test]
    fn template_substitutions_are_tokenized() {
        let toks = kinds("`a${ {b: require('fs')} }c${d}e` + `plain`");
        assert!(toks.contains(&Tok::Ident("require".into())));
        assert!(toks.contains(&Tok::Template(None)));
        assert!(toks.contains(&Tok::Template(Some("plain".into()))));
        assert_eq!(toks.last(), Some(&Tok::Template(Some("plain".into()))));
    }

    #[test]
    fn division_is_not_a_regex_and_positions_are_one_based() {
        let tokens = tokenize("a / b / c\n  x").unwrap();
        assert!(!tokens.iter().any(|t| t.tok == Tok::Regex));
        let last = tokens.last().unwrap();
        assert_eq!((last.pos.line, last.pos.column), (2, 3));
    }

    #[test]
    fn unterminated_literals_fail_with_their_start() {
        let error = tokenize("x = 1;\n  'abc").unwrap_err();
        assert_eq!(
            error.to_string(),
            "ParseError: unterminated string literal (2:3)"
        );
        assert!(tokenize("`abc${").is_err());
        assert!(tokenize("/* x").is_err());
    }

    #[test]
    fn escapes_decode() {
        assert_eq!(
            kinds(r#""a\n\x41B\u{43}\"""#),
            vec![Tok::Str("a\nABC\"".into())]
        );
    }
}
