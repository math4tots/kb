use super::ast::*;
use super::lexer::*;
use super::BasicError;
use super::Binop;
use super::Mark;
use super::RcStr;
use super::Unop;
use std::collections::HashSet;
use std::rc::Rc;

type Prec = i64;
const CMP_PREC: Prec = 100;
const ADD_PREC: Prec = 120;
const MUL_PREC: Prec = 160;
const UNARY_PREC: Prec = 200;
const POSTFIX_PREC: Prec = 1000;

const KEYWORDS: &[&'static str] = &[
    "fn", "import", "var", "if", "elif", "else", "end", "is", "not", "and", "or", "in",

    // legacy names
    "PRINT", "GOTO",
];

pub fn parse(source: &Rc<Source>) -> Result<File, BasicError> {
    let toks = lex(source)?;
    let keywords: HashSet<&'static str> = KEYWORDS.iter().map(|s| *s).collect();
    let mut parser = Parser {
        source: source.clone(),
        toks,
        i: 0,
        keywords,
    };
    let file = parser.file()?;
    Ok(file)
}

struct Parser<'a> {
    source: Rc<Source>,
    toks: Vec<(Token<'a>, Mark)>,
    i: usize,
    keywords: HashSet<&'static str>,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> &Token<'a> {
        &self.toks[self.i].0
    }
    fn lookahead(&self, n: usize) -> Option<&Token<'a>> {
        self.toks.get(self.i + n).map(|pair| &pair.0)
    }
    fn mark(&self) -> Mark {
        self.toks[self.i].1.clone()
    }
    fn at<P: Into<Pat<'a>>>(&self, p: P) -> bool {
        let p = p.into();
        p.matches(self.peek())
    }
    fn gettok(&mut self) -> Token<'a> {
        self.i += 1;
        self.toks[self.i - 1].0.clone()
    }
    fn expect<'b, P: Into<Pat<'b>>>(&mut self, p: P) -> Result<Token<'a>, BasicError> {
        let p = p.into();
        if p.matches(self.peek()) {
            Ok(self.gettok())
        } else {
            Err(BasicError {
                marks: vec![self.mark()],
                message: format!("Expected {:?} but got {:?}", p, self.peek()),
            })
        }
    }
    fn expect_name(&mut self) -> Result<RcStr, BasicError> {
        let mark = self.mark();
        let token = self.expect(Pat::Name)?;
        let name = token.name_or_keyword().unwrap();
        if self.keywords.contains(&name) {
            Err(BasicError {
                marks: vec![mark],
                message: format!("Expected name but got keyword"),
            })
        } else {
            Ok(name.to_owned().into())
        }
    }
    fn expect_label(&mut self) -> Result<RcStr, BasicError> {
        match self.peek() {
            Token::Number(x) => {
                let x = *x;
                self.gettok();
                Ok(format!("{}", x).into())
            }
            Token::Name(_) => {
                let name = self.expect_name()?;
                Ok(name)
            }
            _ => Err(BasicError {
                marks: vec![self.mark()],
                message: format!("Expected label but got {:?}", self.peek()),
            }),
        }
    }
    fn consume<P: Into<Pat<'a>>>(&mut self, p: P) -> bool {
        if self.at(p) {
            self.gettok();
            true
        } else {
            false
        }
    }
}

impl<'a> Parser<'a> {
    fn file(&mut self) -> Result<File, BasicError> {
        let mark = self.mark();
        let mut imports = Vec::new();
        let mut funcs = Vec::new();
        let mut stmts = Vec::new();
        self.consume_delim();
        while !self.at(Token::EOF) {
            match self.peek() {
                Token::Name("import") => imports.push(self.import_()?),
                Token::Name("fn") => funcs.push(self.func()?),
                _ => stmts.extend(self.maybe_labeled_stmt()?),
            }
            self.delim()?;
        }
        Ok(File {
            source: self.source.clone(),
            imports,
            funcs,
            body: Stmt {
                mark,
                desc: StmtDesc::Block(stmts),
            },
            vars: vec![],
        })
    }
    fn import_(&mut self) -> Result<Import, BasicError> {
        let mark = self.mark();
        self.expect(Token::Name("import"))?;
        let mut module_name = self.expect_name()?;
        let mut last_part = module_name.clone();
        while self.consume(Token::Dot) {
            last_part = self.expect_name()?;
            module_name = format!("{}.{}", module_name, last_part).into();
        }
        if self.consume(Token::Name("as")) {
            last_part = self.expect_name()?;
        }
        Ok(Import {
            mark,
            module_name,
            alias: last_part,
            unique_name: "".to_owned().into(),
        })
    }
    fn func(&mut self) -> Result<FuncDisplay, BasicError> {
        let mark = self.mark();
        self.expect(Token::Name("fn"))?;
        let name = self.expect_name()?;
        let params = {
            let mut params = Vec::new();
            self.expect(Token::LParen)?;
            while !self.consume(Token::RParen) {
                params.push(self.expect_name()?);
                if !self.consume(Token::Comma) {
                    self.expect(Token::RParen)?;
                    break;
                }
            }
            params
        };
        let body = self.block()?;
        Ok(FuncDisplay {
            mark,
            short_name: name,
            params,
            body,

            vars: vec![],
            as_var: None,
        })
    }
    fn block(&mut self) -> Result<Stmt, BasicError> {
        let block = self.block_body()?;
        self.expect(Token::Name("end"))?;
        Ok(block)
    }
    fn block_body(&mut self) -> Result<Stmt, BasicError> {
        let mark = self.mark();
        let mut stmts = Vec::new();
        self.consume_delim();
        while !self.at(Token::Name("end"))
            && !self.at(Token::Name("elif"))
            && !self.at(Token::Name("else"))
        {
            let new_stmts = self.maybe_labeled_stmt()?;
            self.delim()?;
            stmts.extend(new_stmts);
        }
        Ok(Stmt {
            mark,
            desc: StmtDesc::Block(stmts),
        })
    }
    fn maybe_labeled_stmt(&mut self) -> Result<Vec<Stmt>, BasicError> {
        let mut ret = Vec::new();

        // check for
        //   <label-name> :
        // style labels
        match self.peek() {
            Token::Name(name)
                if !self.keywords.contains(name) && self.lookahead(1) == Some(&Token::Colon) =>
            {
                let mark = self.mark();
                let label = self.expect_label()?;
                self.expect(Token::Colon)?;
                self.consume_delim();
                ret.push(Stmt {
                    mark,
                    desc: StmtDesc::Label(label),
                })
            }
            _ => {}
        }

        // check for line number labels
        // we check to see if a Number is immediately followed
        // by a Name (keyword or otherwise) or open parentheses
        // if this is not the case, we assume there is no line number
        if self.peek().number().is_some()
            && self
                .lookahead(1)
                .map(|t| t.name_or_keyword().is_some() || t == &Token::LParen)
                .unwrap_or(false)
        {
            let mark = self.mark();
            let label = self.expect_label()?;
            ret.push(Stmt {
                mark,
                desc: StmtDesc::Label(label),
            });
        }

        ret.push(self.stmt()?);

        Ok(ret)
    }
    fn stmt(&mut self) -> Result<Stmt, BasicError> {
        let mark = self.mark();
        match self.peek() {
            Token::Name("PRINT") => {
                self.gettok();
                let arg = self.expr(0)?;
                Ok(Stmt {
                    mark,
                    desc: StmtDesc::Print(arg.into()),
                })
            }
            Token::Name("GOTO") => {
                self.gettok();
                let label = self.expect_label()?;
                Ok(Stmt {
                    mark,
                    desc: StmtDesc::Goto(label),
                })
            }
            Token::Name("var") => {
                self.gettok();
                let name = self.expect_name()?;
                self.expect(Token::Eq)?;
                let setexpr = self.expr(0)?;
                Ok(Stmt {
                    mark,
                    desc: StmtDesc::DeclVar(name, setexpr),
                })
            }
            Token::Name("if") => {
                self.gettok();
                let mut pairs = Vec::new();
                let mut other = None;
                loop {
                    let cond = self.expr(0)?;
                    self.delim()?;
                    let body = self.block_body()?;
                    pairs.push((cond, body));
                    if !self.consume(Token::Name("elif")) {
                        if self.consume(Token::Name("else")) {
                            self.delim()?;
                            other = Some(self.block_body()?.into());
                        }
                        self.expect(Token::Name("end"))?;
                        break;
                    }
                }
                Ok(Stmt {
                    mark,
                    desc: StmtDesc::If(pairs, other),
                })
            }
            Token::Name("while") => {
                self.gettok();
                let cond = self.expr(0)?;
                self.delim()?;
                let body = self.block()?;
                Ok(Stmt {
                    mark,
                    desc: StmtDesc::While(cond, body.into()),
                })
            }
            Token::Name(name)
                if !self.keywords.contains(name) && self.lookahead(1) == Some(&Token::Colon) =>
            {
                let label = self.expect_label()?;
                self.expect(Token::Colon)?;
                Ok(Stmt {
                    mark,
                    desc: StmtDesc::Label(label),
                })
            }
            _ => {
                let expr = self.expr(0)?;
                Ok(Stmt {
                    mark,
                    desc: StmtDesc::Expr(expr),
                })
            }
        }
    }
    fn expr(&mut self, prec: Prec) -> Result<Expr, BasicError> {
        let mut e = self.atom()?;
        while precof(self.peek()) >= prec {
            e = self.infix(e)?;
        }
        Ok(e)
    }
    fn infix(&mut self, e: Expr) -> Result<Expr, BasicError> {
        let mark = self.mark();
        let token = self.gettok();
        match token {
            Token::Dot => {
                let attr = self.expect_name()?;
                Ok(Expr {
                    mark,
                    desc: ExprDesc::GetAttr(e.into(), attr),
                })
            }
            Token::LParen => {
                let mut args = Vec::new();
                while !self.consume(Token::RParen) {
                    args.push(self.expr(0)?);
                    if !self.consume(Token::Comma) {
                        self.expect(Token::RParen)?;
                        break;
                    }
                }
                Ok(Expr {
                    mark,
                    desc: ExprDesc::CallFunc(e.into(), args),
                })
            }
            Token::Plus
            | Token::Minus
            | Token::Star
            | Token::Slash
            | Token::Slash2
            | Token::Percent
            | Token::LessThan
            | Token::LessThanOrEqual
            | Token::GreaterThan
            | Token::GreaterThanOrEqual => {
                let op = match token {
                    Token::Plus => Binop::Add,
                    Token::Minus => Binop::Subtract,
                    Token::Star => Binop::Multiply,
                    Token::Slash => Binop::Divide,
                    Token::Slash2 => Binop::TruncDivide,
                    Token::Percent => Binop::Remainder,
                    Token::LessThan => Binop::LessThan,
                    Token::LessThanOrEqual => Binop::LessThanOrEqual,
                    Token::GreaterThan => Binop::GreaterThan,
                    Token::GreaterThanOrEqual => Binop::GreaterThanOrEqual,
                    _ => panic!("binop {:?}", token),
                };
                let prec = precof(&token);
                let rhs = self.expr(prec + 1)?;
                Ok(Expr {
                    mark,
                    desc: ExprDesc::Binop(op, e.into(), rhs.into()),
                })
            }
            Token::Eq => match &e.desc {
                ExprDesc::GetVar(name) => {
                    let rhs = self.expr(0)?;
                    Ok(Expr {
                        mark,
                        desc: ExprDesc::SetVar(name.clone(), rhs.into()),
                    })
                }
                _ => Err(BasicError {
                    marks: vec![e.mark.clone()],
                    message: format!("The left hand side is not assignable"),
                }),
            },
            _ => Err(BasicError {
                marks: vec![mark],
                message: format!("Expected infix operator"),
            }),
        }
    }
    fn atom(&mut self) -> Result<Expr, BasicError> {
        let mark = self.mark();
        match self.peek() {
            Token::LParen => {
                self.gettok();
                let expr = self.expr(0)?;
                self.expect(Token::RParen)?;
                Ok(expr)
            }
            Token::Number(x) => {
                let x = *x;
                self.gettok();
                Ok(Expr {
                    mark,
                    desc: ExprDesc::Number(x),
                })
            }
            Token::String(_) => {
                let s = self.gettok().string().unwrap();
                Ok(Expr {
                    mark,
                    desc: ExprDesc::String(s.into()),
                })
            }
            Token::RawString(s) => Ok(Expr {
                mark,
                desc: ExprDesc::String((*s).into()),
            }),
            Token::Minus | Token::Plus => {
                let tok = self.gettok();
                let op = match tok {
                    Token::Minus => Unop::Negative,
                    Token::Plus => Unop::Positive,
                    _ => panic!("parse unop: {:?}", tok),
                };
                let expr = self.expr(UNARY_PREC)?;
                Ok(Expr {
                    mark,
                    desc: ExprDesc::Unop(op, expr.into()),
                })
            }
            Token::Name("nil") => {
                self.gettok();
                Ok(Expr {
                    mark,
                    desc: ExprDesc::Nil,
                })
            }
            Token::Name(name) if !self.keywords.contains(name) => {
                let name = self.expect_name()?;
                Ok(Expr {
                    mark,
                    desc: ExprDesc::GetVar(name),
                })
            }
            Token::Name(name) if self.keywords.contains(name) => Err(BasicError {
                marks: vec![mark],
                message: format!("Expected expression but got keyword {:?}", name),
            }),
            _ => Err(BasicError {
                marks: vec![mark],
                message: format!("Expected expression but got {:?}", self.peek()),
            }),
        }
    }
    fn consume_delim(&mut self) {
        while self.at(Token::Newline) || self.at(Token::Semicolon) {
            self.gettok();
        }
    }
    fn at_delim(&self) -> bool {
        match self.peek() {
            Token::Name("end") | Token::EOF | Token::Semicolon | Token::Newline => true,
            _ => false,
        }
    }
    fn delim(&mut self) -> Result<(), BasicError> {
        if self.at_delim() {
            self.consume_delim();
            Ok(())
        } else {
            Err(BasicError {
                marks: vec![self.mark()],
                message: format!("Expected delimiter but got {:?}", self.peek()),
            })
        }
    }
}

fn precof<'a>(tok: &Token<'a>) -> Prec {
    match tok {
        Token::Name("is")
        | Token::Eq
        | Token::LessThan
        | Token::LessThanOrEqual
        | Token::GreaterThan
        | Token::GreaterThanOrEqual => CMP_PREC,
        Token::Minus | Token::Plus => ADD_PREC,
        Token::Star | Token::Slash | Token::Slash2 | Token::Percent => MUL_PREC,
        Token::LParen | Token::Dot => POSTFIX_PREC,
        _ => -1,
    }
}

#[derive(Debug, Clone)]
enum Pat<'a> {
    Exact(Token<'a>),
    Keyword(&'a str),
    Name,
}

impl<'a> Pat<'a> {
    fn matches<'b>(&self, tok: &Token<'b>) -> bool {
        match self {
            Pat::Exact(t) => t == tok,
            Pat::Keyword(t) => match tok {
                Token::Name(name) => t == name,
                _ => false,
            },
            Pat::Name => match tok {
                Token::Name(_) => true,
                _ => false,
            },
        }
    }
}

impl<'a> From<Token<'a>> for Pat<'a> {
    fn from(t: Token<'a>) -> Pat<'a> {
        Pat::Exact(t)
    }
}

impl<'a> From<&'a str> for Pat<'a> {
    fn from(s: &'a str) -> Pat<'a> {
        Pat::Keyword(s)
    }
}
