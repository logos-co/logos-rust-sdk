//! LIDL parser — same grammar as logos-lidl's C++ frontend, including
//! keywords being reserved only structurally (valid in name positions).

use crate::ast::*;

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Module, TypeKw, Method, Event, Version, Description, Category, Depends,
    Ident(String), StringLit(String),
    LBrace, RBrace, LParen, RParen, LBracket, RBracket,
    Colon, Comma, Arrow, Question,
    Eof,
}

#[derive(Debug)]
pub struct ParseError {
    pub message: String,
    pub line: usize,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}
impl std::error::Error for ParseError {}

fn lex(src: &str) -> Result<Vec<(Tok, usize)>, ParseError> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let (mut i, mut line) = (0usize, 1usize);
    while i < b.len() {
        let c = b[i] as char;
        match c {
            ' ' | '\t' | '\r' => { i += 1; }
            '\n' => { i += 1; line += 1; }
            ';' => { while i < b.len() && b[i] != b'\n' { i += 1; } }
            '-' if i + 1 < b.len() && b[i + 1] == b'>' => { out.push((Tok::Arrow, line)); i += 2; }
            '{' => { out.push((Tok::LBrace, line)); i += 1; }
            '}' => { out.push((Tok::RBrace, line)); i += 1; }
            '(' => { out.push((Tok::LParen, line)); i += 1; }
            ')' => { out.push((Tok::RParen, line)); i += 1; }
            '[' => { out.push((Tok::LBracket, line)); i += 1; }
            ']' => { out.push((Tok::RBracket, line)); i += 1; }
            ':' => { out.push((Tok::Colon, line)); i += 1; }
            ',' => { out.push((Tok::Comma, line)); i += 1; }
            '?' => { out.push((Tok::Question, line)); i += 1; }
            '"' => {
                i += 1;
                let mut s = String::new();
                loop {
                    if i >= b.len() || b[i] == b'\n' {
                        return Err(ParseError { message: "Unterminated string literal".into(), line });
                    }
                    match b[i] {
                        b'"' => { i += 1; break; }
                        b'\\' if i + 1 < b.len() => {
                            i += 1;
                            s.push(match b[i] {
                                b'n' => '\n', b't' => '\t', other => other as char,
                            });
                            i += 1;
                        }
                        other => { s.push(other as char); i += 1; }
                    }
                }
                out.push((Tok::StringLit(s), line));
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let start = i;
                while i < b.len() && ((b[i] as char).is_ascii_alphanumeric() || b[i] == b'_') { i += 1; }
                let word = &src[start..i];
                let tok = match word {
                    "module" => Tok::Module, "type" => Tok::TypeKw, "method" => Tok::Method,
                    "event" => Tok::Event, "version" => Tok::Version,
                    "description" => Tok::Description, "category" => Tok::Category,
                    "depends" => Tok::Depends,
                    _ => Tok::Ident(word.to_string()),
                };
                out.push((tok, line));
            }
            other => {
                return Err(ParseError { message: format!("Unexpected character '{}'", other), line });
            }
        }
    }
    out.push((Tok::Eof, line));
    Ok(out)
}

const BUILTINS: &[&str] = &["tstr", "bstr", "int", "uint", "float64", "bool", "result", "any"];

struct P {
    toks: Vec<(Tok, usize)>,
    pos: usize,
}

impl P {
    fn cur(&self) -> &Tok { &self.toks[self.pos.min(self.toks.len() - 1)].0 }
    fn line(&self) -> usize { self.toks[self.pos.min(self.toks.len() - 1)].1 }
    fn bump(&mut self) { self.pos += 1; }
    fn err(&self, m: impl Into<String>) -> ParseError {
        ParseError { message: m.into(), line: self.line() }
    }
    fn expect(&mut self, t: Tok, ctx: &str) -> Result<(), ParseError> {
        if std::mem::discriminant(self.cur()) == std::mem::discriminant(&t) {
            self.bump();
            Ok(())
        } else {
            Err(self.err(format!("Expected {:?} in {}", t, ctx)))
        }
    }
    // Keywords are valid in name positions (structurally reserved only).
    fn name(&mut self, what: &str) -> Result<String, ParseError> {
        let n = match self.cur() {
            Tok::Ident(s) => s.clone(),
            Tok::Module => "module".into(), Tok::TypeKw => "type".into(),
            Tok::Method => "method".into(), Tok::Event => "event".into(),
            Tok::Version => "version".into(), Tok::Description => "description".into(),
            Tok::Category => "category".into(), Tok::Depends => "depends".into(),
            _ => return Err(self.err(format!("Expected {}", what))),
        };
        self.bump();
        Ok(n)
    }
    fn string_lit(&mut self, ctx: &str) -> Result<String, ParseError> {
        if let Tok::StringLit(s) = self.cur().clone() {
            self.bump();
            Ok(s)
        } else {
            Err(self.err(format!("Expected string after '{}'", ctx)))
        }
    }

    fn type_expr(&mut self) -> Result<TypeExpr, ParseError> {
        match self.cur().clone() {
            Tok::Question => {
                self.bump();
                Ok(TypeExpr { kind: TypeKind::Optional, name: String::new(), elements: vec![self.type_expr()?] })
            }
            Tok::LBracket => {
                self.bump();
                let inner = self.type_expr()?;
                self.expect(Tok::RBracket, "array type")?;
                Ok(TypeExpr { kind: TypeKind::Array, name: String::new(), elements: vec![inner] })
            }
            Tok::LBrace => {
                self.bump();
                let k = self.type_expr()?;
                self.expect(Tok::Colon, "map type")?;
                let v = self.type_expr()?;
                self.expect(Tok::RBrace, "map type")?;
                Ok(TypeExpr { kind: TypeKind::Map, name: String::new(), elements: vec![k, v] })
            }
            _ => {
                let n = self.name("type expression")?;
                let kind = if BUILTINS.contains(&n.as_str()) { TypeKind::Primitive } else { TypeKind::Named };
                Ok(TypeExpr { kind, name: n, elements: vec![] })
            }
        }
    }

    fn params(&mut self) -> Result<Vec<ParamDecl>, ParseError> {
        let mut out = Vec::new();
        if matches!(self.cur(), Tok::RParen) {
            return Ok(out);
        }
        loop {
            let name = self.name("parameter name")?;
            self.expect(Tok::Colon, "parameter")?;
            out.push(ParamDecl { name, ty: self.type_expr()? });
            if matches!(self.cur(), Tok::Comma) { self.bump(); } else { break; }
        }
        Ok(out)
    }

    fn module(&mut self) -> Result<ModuleDecl, ParseError> {
        self.expect(Tok::Module, "module declaration")?;
        let mut m = ModuleDecl { name: self.name("module name")?, ..Default::default() };
        self.expect(Tok::LBrace, "module declaration")?;
        loop {
            match self.cur().clone() {
                Tok::RBrace => { self.bump(); break; }
                Tok::Version => { self.bump(); m.version = self.string_lit("version")?; }
                Tok::Description => { self.bump(); m.description = self.string_lit("description")?; }
                Tok::Category => { self.bump(); m.category = self.string_lit("category")?; }
                Tok::Depends => {
                    self.bump();
                    self.expect(Tok::LBracket, "depends list")?;
                    if !matches!(self.cur(), Tok::RBracket) {
                        loop {
                            m.depends.push(self.name("dependency name")?);
                            if matches!(self.cur(), Tok::Comma) { self.bump(); } else { break; }
                        }
                    }
                    self.expect(Tok::RBracket, "depends list")?;
                }
                Tok::TypeKw => {
                    self.bump();
                    let mut td = TypeDecl { name: self.name("type name")?, fields: vec![] };
                    self.expect(Tok::LBrace, "type definition")?;
                    while !matches!(self.cur(), Tok::RBrace | Tok::Eof) {
                        let optional = if matches!(self.cur(), Tok::Question) { self.bump(); true } else { false };
                        let fname = self.name("field name")?;
                        self.expect(Tok::Colon, "field definition")?;
                        td.fields.push(FieldDecl { name: fname, ty: self.type_expr()?, optional });
                    }
                    self.expect(Tok::RBrace, "type definition")?;
                    m.types.push(td);
                }
                Tok::Method => {
                    self.bump();
                    let name = self.name("method name")?;
                    self.expect(Tok::LParen, "method parameters")?;
                    let params = self.params()?;
                    self.expect(Tok::RParen, "method parameters")?;
                    self.expect(Tok::Arrow, "method return type")?;
                    let return_type = self.type_expr()?;
                    m.methods.push(MethodDecl { name, params, return_type });
                }
                Tok::Event => {
                    self.bump();
                    let name = self.name("event name")?;
                    self.expect(Tok::LParen, "event parameters")?;
                    let params = self.params()?;
                    self.expect(Tok::RParen, "event parameters")?;
                    m.events.push(EventDecl { name, params });
                }
                Tok::Eof => return Err(self.err("Unexpected end of input in module body")),
                other => return Err(self.err(format!("Unexpected token {:?} in module body", other))),
            }
        }
        if !matches!(self.cur(), Tok::Eof) {
            return Err(self.err("Unexpected content after module closing '}'"));
        }
        Ok(m)
    }
}

pub fn parse(source: &str) -> Result<ModuleDecl, ParseError> {
    let toks = lex(source)?;
    P { toks, pos: 0 }.module()
}
