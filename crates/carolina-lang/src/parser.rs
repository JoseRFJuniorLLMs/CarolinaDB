//! Recursive-descent parser for the restricted DSL (SPEC-003 §3).
//!
//! Grammar (keywords are case-sensitive uppercase identifiers):
//!
//! ```text
//! module     := item*
//! item       := record | enum | index | resource | invariant | operation
//! record     := RECORD name [IMMUTABLE] { field (, field)* }
//! field      := name : type [PRIMARY KEY]
//! enum       := ENUM name { name (, name)* }
//! index      := INDEX name ON record ( name (, name)* )
//! resource   := RESOURCE name ON record ( available: field, reserved: field, reservation: record )
//! invariant  := INVARIANT name { [KIND kind] body }
//! body       := FORALL v IN record [WHERE expr] : expr
//!             | UNIQUE v IN record [WHERE expr] : expr
//!             | REFERENCE v IN record : expr -> record
//!             | AGGREGATE (SUM|COUNT) ( expr ) FOR v IN record [WHERE expr] [GROUP BY expr] cmp expr
//!             | TRANSITION v IN record : v.field FROM A TO B (, A TO B)*
//!             | REQUIRES op ( params ) NEEDS fact ( params )
//! operation  := OPERATION name ( params ) VERSION int { REQUIRE expr READ { reads } EFFECT { effects } ENSURE expr RETURN expr CONTRACT { fields } }
//! read       := name = record[expr] | name = OPTIONAL record[expr] | name = EXISTS record[expr] | name = SCAN v IN record WHERE expr
//! effect     := [WHEN expr :] effect_kind
//! ```
//!
//! Every operation section is mandatory and explicit (`REQUIRE true`, `READ {}`, `ENSURE true`,
//! `RETURN Unit`): the parser never defaults a contract or observation.

use crate::ast::*;
use crate::lexer::{lex, Tok, Token};
use carolina_core::error::{CoreError, CoreResult, ErrorCode};
use carolina_core::limits::Limits;

pub fn parse_module(src: &str, limits: &Limits) -> CoreResult<Module> {
    if src.len() > limits.max_source_bytes {
        return Err(CoreError::new(
            ErrorCode::ResourceLimit,
            "source exceeds max_source_bytes",
        ));
    }
    let tokens = lex(src)?;
    let mut p = Parser {
        toks: tokens,
        pos: 0,
        depth: 0,
        limits: limits.clone(),
    };
    let mut items = Vec::new();
    while !p.at_eof() {
        items.push(p.item()?);
        if items.len() > limits.max_declarations {
            return Err(CoreError::new(
                ErrorCode::ResourceLimit,
                "too many declarations",
            ));
        }
    }
    Ok(Module { items })
}

struct Parser {
    toks: Vec<Token>,
    pos: usize,
    depth: usize,
    limits: Limits,
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }
    fn span(&self) -> Span {
        self.toks[self.pos].span
    }
    fn at_eof(&self) -> bool {
        matches!(self.peek(), Tok::Eof)
    }
    fn bump(&mut self) -> Token {
        let t = self.toks[self.pos].clone();
        if !self.at_eof() {
            self.pos += 1;
        }
        t
    }
    fn err<T>(&self, msg: &str) -> CoreResult<T> {
        let s = self.span();
        Err(CoreError::new(
            ErrorCode::ParseError,
            format!("{msg}, found {:?}", self.peek()),
        )
        .at(s.to_string()))
    }
    fn expect(&mut self, tok: Tok) -> CoreResult<()> {
        if *self.peek() == tok {
            self.bump();
            Ok(())
        } else {
            self.err(&format!("expected {tok:?}"))
        }
    }
    fn is_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Tok::Ident(s) if s == kw)
    }
    fn kw(&mut self, kw: &str) -> CoreResult<()> {
        if self.is_kw(kw) {
            self.bump();
            Ok(())
        } else {
            self.err(&format!("expected keyword {kw}"))
        }
    }
    fn eat_kw(&mut self, kw: &str) -> bool {
        if self.is_kw(kw) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn eat(&mut self, tok: Tok) -> bool {
        if *self.peek() == tok {
            self.bump();
            true
        } else {
            false
        }
    }
    fn ident(&mut self) -> CoreResult<Ident> {
        let span = self.span();
        match self.bump().tok {
            Tok::Ident(name) => Ok(Ident { name, span }),
            _ => {
                self.pos -= 1;
                self.err("expected identifier")
            }
        }
    }
    fn int(&mut self) -> CoreResult<i128> {
        match self.bump().tok {
            Tok::Int(v) => Ok(v),
            _ => {
                self.pos -= 1;
                self.err("expected integer")
            }
        }
    }

    // ---- items ---------------------------------------------------------
    fn item(&mut self) -> CoreResult<Item> {
        if self.is_kw("RECORD") {
            self.bump();
            let name = self.ident()?;
            let immutable = self.eat_kw("IMMUTABLE");
            self.expect(Tok::LBrace)?;
            let mut fields = Vec::new();
            loop {
                if self.eat(Tok::RBrace) {
                    break;
                }
                let fname = self.ident()?;
                self.expect(Tok::Colon)?;
                let ty = self.type_expr()?;
                let mut pk = false;
                if self.eat_kw("PRIMARY") {
                    self.kw("KEY")?;
                    pk = true;
                }
                fields.push(FieldDecl {
                    name: fname,
                    ty,
                    primary_key: pk,
                });
                if !self.eat(Tok::Comma) {
                    self.expect(Tok::RBrace)?;
                    break;
                }
            }
            return Ok(Item::Record(RecordDecl {
                name,
                immutable,
                fields,
            }));
        }
        if self.is_kw("ENUM") {
            self.bump();
            let name = self.ident()?;
            self.expect(Tok::LBrace)?;
            let mut variants = Vec::new();
            loop {
                variants.push(self.ident()?);
                if !self.eat(Tok::Comma) {
                    self.expect(Tok::RBrace)?;
                    break;
                }
                if self.eat(Tok::RBrace) {
                    break;
                }
            }
            return Ok(Item::Enum(EnumDecl { name, variants }));
        }
        if self.is_kw("INDEX") {
            self.bump();
            let name = self.ident()?;
            self.kw("ON")?;
            let record = self.ident()?;
            self.expect(Tok::LParen)?;
            let mut fields = Vec::new();
            loop {
                fields.push(self.ident()?);
                if !self.eat(Tok::Comma) {
                    self.expect(Tok::RParen)?;
                    break;
                }
            }
            return Ok(Item::Index(IndexDecl {
                name,
                record,
                fields,
            }));
        }
        if self.is_kw("RESOURCE") {
            self.bump();
            let name = self.ident()?;
            self.kw("ON")?;
            let record = self.ident()?;
            self.expect(Tok::LParen)?;
            self.kw("available")?;
            self.expect(Tok::Colon)?;
            let available = self.ident()?;
            self.expect(Tok::Comma)?;
            self.kw("reserved")?;
            self.expect(Tok::Colon)?;
            let reserved = self.ident()?;
            self.expect(Tok::Comma)?;
            self.kw("reservation")?;
            self.expect(Tok::Colon)?;
            let reservation_record = self.ident()?;
            self.expect(Tok::RParen)?;
            return Ok(Item::Resource(ResourceDecl {
                name,
                record,
                available,
                reserved,
                reservation_record,
            }));
        }
        if self.is_kw("INVARIANT") {
            return Ok(Item::Invariant(self.invariant()?));
        }
        if self.is_kw("OPERATION") {
            return Ok(Item::Operation(self.operation()?));
        }
        self.err("expected RECORD, ENUM, INDEX, RESOURCE, INVARIANT or OPERATION")
    }

    fn type_expr(&mut self) -> CoreResult<TypeExpr> {
        let id = self.ident()?;
        Ok(match id.name.as_str() {
            "Bool" => TypeExpr::Bool,
            "I64" => TypeExpr::I64,
            "U64" => TypeExpr::U64,
            "Uuid" => TypeExpr::Uuid,
            "Bytes" => {
                self.expect(Tok::LParen)?;
                let n = self.int()?;
                self.expect(Tok::RParen)?;
                TypeExpr::Bytes(
                    u32::try_from(n)
                        .map_err(|_| CoreError::new(ErrorCode::ParseError, "bad Bytes length"))?,
                )
            }
            "String" => {
                self.expect(Tok::LParen)?;
                let n = self.int()?;
                self.expect(Tok::RParen)?;
                TypeExpr::String(
                    u32::try_from(n)
                        .map_err(|_| CoreError::new(ErrorCode::ParseError, "bad String length"))?,
                )
            }
            "Decimal" => {
                self.expect(Tok::LParen)?;
                let p = self.int()?;
                self.expect(Tok::Comma)?;
                let s = self.int()?;
                self.expect(Tok::RParen)?;
                let p = u8::try_from(p).map_err(|_| {
                    CoreError::new(ErrorCode::InvalidDecimal, "precision out of range")
                })?;
                let s = u8::try_from(s)
                    .map_err(|_| CoreError::new(ErrorCode::InvalidDecimal, "scale out of range"))?;
                TypeExpr::Decimal(p, s)
            }
            "Option" => {
                self.expect(Tok::Lt)?;
                let inner = self.type_expr()?;
                self.expect(Tok::Gt)?;
                TypeExpr::Option(Box::new(inner))
            }
            "Set" => {
                self.expect(Tok::Lt)?;
                let inner = self.type_expr()?;
                self.expect(Tok::Gt)?;
                TypeExpr::Set(Box::new(inner))
            }
            "Tuple" => {
                self.expect(Tok::LParen)?;
                let mut items = Vec::new();
                loop {
                    items.push(self.type_expr()?);
                    if !self.eat(Tok::Comma) {
                        self.expect(Tok::RParen)?;
                        break;
                    }
                }
                TypeExpr::Tuple(items)
            }
            _ => TypeExpr::Named(id),
        })
    }

    fn invariant(&mut self) -> CoreResult<InvariantDecl> {
        self.kw("INVARIANT")?;
        let name = self.ident()?;
        self.expect(Tok::LBrace)?;
        let kind_hint = if self.eat_kw("KIND") {
            Some(self.ident()?)
        } else {
            None
        };
        let body = if self.eat_kw("FORALL") {
            let var = self.ident()?;
            self.kw("IN")?;
            let record = self.ident()?;
            let filter = if self.eat_kw("WHERE") {
                Some(self.expr()?)
            } else {
                None
            };
            self.expect(Tok::Colon)?;
            let predicate = self.expr()?;
            InvariantBody::ForAll {
                var,
                record,
                filter,
                predicate,
            }
        } else if self.eat_kw("UNIQUE") {
            let var = self.ident()?;
            self.kw("IN")?;
            let record = self.ident()?;
            let filter = if self.eat_kw("WHERE") {
                Some(self.expr()?)
            } else {
                None
            };
            self.expect(Tok::Colon)?;
            let key = self.expr()?;
            InvariantBody::Unique {
                var,
                record,
                filter,
                key,
            }
        } else if self.eat_kw("REFERENCE") {
            let var = self.ident()?;
            self.kw("IN")?;
            let record = self.ident()?;
            self.expect(Tok::Colon)?;
            let parent_key = self.expr()?;
            self.expect(Tok::Arrow)?;
            let parent = self.ident()?;
            InvariantBody::Reference {
                var,
                record,
                parent_key,
                parent,
            }
        } else if self.eat_kw("AGGREGATE") {
            let op = if self.eat_kw("SUM") {
                AggregateOp::Sum
            } else if self.eat_kw("COUNT") {
                AggregateOp::Count
            } else {
                return self.err("expected SUM or COUNT");
            };
            self.expect(Tok::LParen)?;
            let value = self.expr()?;
            self.expect(Tok::RParen)?;
            self.kw("FOR")?;
            let var = self.ident()?;
            self.kw("IN")?;
            let record = self.ident()?;
            let filter = if self.eat_kw("WHERE") {
                Some(self.expr()?)
            } else {
                None
            };
            let group_by = if self.eat_kw("GROUP") {
                self.kw("BY")?;
                Some(self.expr()?)
            } else {
                None
            };
            let comparison = self.cmp_op()?;
            let bound = self.expr()?;
            InvariantBody::Aggregate {
                op,
                value,
                var,
                record,
                filter,
                group_by,
                comparison,
                bound,
            }
        } else if self.eat_kw("TRANSITION") {
            let var = self.ident()?;
            self.kw("IN")?;
            let record = self.ident()?;
            self.expect(Tok::Colon)?;
            let v2 = self.ident()?;
            if v2.name != var.name {
                return self.err("transition path must use the bound row variable");
            }
            self.expect(Tok::Dot)?;
            let field = self.ident()?;
            let mut edges = Vec::new();
            loop {
                self.kw("FROM")?;
                let a = self.ident()?;
                self.kw("TO")?;
                let b = self.ident()?;
                edges.push((a, b));
                if !self.eat(Tok::Comma) {
                    break;
                }
            }
            InvariantBody::Transition {
                var,
                record,
                field,
                edges,
            }
        } else if self.eat_kw("REQUIRES") {
            let operation = self.ident()?;
            let op_params = self.ident_list_parens()?;
            self.kw("NEEDS")?;
            let fact = self.ident()?;
            let fact_params = self.ident_list_parens()?;
            InvariantBody::Requires {
                operation,
                op_params,
                fact,
                fact_params,
            }
        } else {
            return self.err(
                "expected invariant body (FORALL/UNIQUE/REFERENCE/AGGREGATE/TRANSITION/REQUIRES)",
            );
        };
        self.expect(Tok::RBrace)?;
        Ok(InvariantDecl {
            name,
            kind_hint,
            body,
        })
    }

    fn ident_list_parens(&mut self) -> CoreResult<Vec<Ident>> {
        self.expect(Tok::LParen)?;
        let mut v = Vec::new();
        if self.eat(Tok::RParen) {
            return Ok(v);
        }
        loop {
            v.push(self.ident()?);
            if !self.eat(Tok::Comma) {
                self.expect(Tok::RParen)?;
                break;
            }
        }
        Ok(v)
    }

    fn cmp_op(&mut self) -> CoreResult<CmpOp> {
        let op = match self.peek() {
            Tok::EqEq => CmpOp::Eq,
            Tok::Ne => CmpOp::Ne,
            Tok::Lt => CmpOp::Lt,
            Tok::Le => CmpOp::Le,
            Tok::Gt => CmpOp::Gt,
            Tok::Ge => CmpOp::Ge,
            _ => return self.err("expected comparison operator"),
        };
        self.bump();
        Ok(op)
    }

    fn operation(&mut self) -> CoreResult<OperationDecl> {
        self.kw("OPERATION")?;
        let name = self.ident()?;
        self.expect(Tok::LParen)?;
        let mut params = Vec::new();
        if !self.eat(Tok::RParen) {
            loop {
                let pname = self.ident()?;
                self.expect(Tok::Colon)?;
                let ty = self.type_expr()?;
                params.push(Param { name: pname, ty });
                if !self.eat(Tok::Comma) {
                    self.expect(Tok::RParen)?;
                    break;
                }
            }
        }
        self.kw("VERSION")?;
        let version = self.int()?;
        let version = u32::try_from(version)
            .map_err(|_| CoreError::new(ErrorCode::ParseError, "bad version"))?;
        if version == 0 {
            return self.err("operation version must be >= 1");
        }
        self.expect(Tok::LBrace)?;
        self.kw("REQUIRE")?;
        let require = self.expr()?;
        self.kw("READ")?;
        self.expect(Tok::LBrace)?;
        let mut reads = Vec::new();
        while !self.eat(Tok::RBrace) {
            let binding = self.ident()?;
            self.expect(Tok::Assign)?;
            let expr = if self.eat_kw("OPTIONAL") {
                let record = self.ident()?;
                let key = self.bracket_expr()?;
                ReadExpr::OptionalRow { record, key }
            } else if self.eat_kw("EXISTS") {
                let record = self.ident()?;
                let key = self.bracket_expr()?;
                ReadExpr::Exists { record, key }
            } else if self.eat_kw("SCAN") {
                let var = self.ident()?;
                self.kw("IN")?;
                let record = self.ident()?;
                self.kw("WHERE")?;
                let filter = self.expr()?;
                ReadExpr::Scan {
                    var,
                    record,
                    filter,
                }
            } else {
                let record = self.ident()?;
                let key = self.bracket_expr()?;
                ReadExpr::Row { record, key }
            };
            reads.push(ReadDecl { binding, expr });
            self.eat(Tok::Comma);
        }
        self.kw("EFFECT")?;
        self.expect(Tok::LBrace)?;
        let mut effects = Vec::new();
        while !self.eat(Tok::RBrace) {
            effects.push(self.effect()?);
        }
        self.kw("ENSURE")?;
        let ensure = self.expr()?;
        self.kw("RETURN")?;
        let result = self.expr()?;
        self.kw("CONTRACT")?;
        let contract = self.contract()?;
        self.expect(Tok::RBrace)?;
        Ok(OperationDecl {
            name,
            params,
            version,
            require,
            reads,
            effects,
            ensure,
            result,
            contract,
        })
    }

    fn bracket_expr(&mut self) -> CoreResult<Expr> {
        self.expect(Tok::LBracket)?;
        let e = self.expr()?;
        self.expect(Tok::RBracket)?;
        Ok(e)
    }

    fn path(&mut self) -> CoreResult<PathExpr> {
        let record = self.ident()?;
        let key = self.bracket_expr()?;
        self.expect(Tok::Dot)?;
        let field = self.ident()?;
        Ok(PathExpr { record, key, field })
    }

    fn field_inits(&mut self) -> CoreResult<Vec<(Ident, Expr)>> {
        self.expect(Tok::LBrace)?;
        let mut fields = Vec::new();
        while !self.eat(Tok::RBrace) {
            let f = self.ident()?;
            self.expect(Tok::Colon)?;
            let e = self.expr()?;
            fields.push((f, e));
            if !self.eat(Tok::Comma) {
                self.expect(Tok::RBrace)?;
                break;
            }
        }
        Ok(fields)
    }

    fn effect(&mut self) -> CoreResult<Effect> {
        let span = self.span();
        let guard = if self.eat_kw("WHEN") {
            let g = self.expr()?;
            self.expect(Tok::Colon)?;
            Some(g)
        } else {
            None
        };
        let kind = if self.eat_kw("ASSIGN") {
            let path = self.path()?;
            self.expect(Tok::Assign)?;
            let value = self.expr()?;
            EffectKindAst::Assign { path, value }
        } else if self.eat_kw("INCREMENT") {
            let path = self.path()?;
            self.kw("BY")?;
            let amount = self.expr()?;
            EffectKindAst::Increment { path, amount }
        } else if self.eat_kw("DECREMENT") {
            let path = self.path()?;
            self.kw("BY")?;
            let amount = self.expr()?;
            EffectKindAst::Decrement { path, amount }
        } else if self.eat_kw("INSERT") {
            let record = self.ident()?;
            let fields = self.field_inits()?;
            EffectKindAst::Insert { record, fields }
        } else if self.eat_kw("DELETE") {
            let record = self.ident()?;
            let key = self.bracket_expr()?;
            EffectKindAst::Delete { record, key }
        } else if self.eat_kw("ADD") {
            let value = self.expr()?;
            self.kw("TO")?;
            let path = self.path()?;
            EffectKindAst::AddToSet { path, value }
        } else if self.eat_kw("REMOVE") {
            let value = self.expr()?;
            self.kw("FROM")?;
            let path = self.path()?;
            EffectKindAst::RemoveFromSet { path, value }
        } else if self.eat_kw("CAS") {
            let path = self.path()?;
            self.kw("FROM")?;
            let expected = self.expr()?;
            self.kw("TO")?;
            let value = self.expr()?;
            EffectKindAst::CompareAndSwap {
                path,
                expected,
                value,
            }
        } else if self.eat_kw("RESERVE") {
            let resource = self.ident()?;
            let key = self.bracket_expr()?;
            let amount = self.expr()?;
            self.kw("AS")?;
            let reservation_id = self.expr()?;
            EffectKindAst::Reserve {
                resource,
                key,
                amount,
                reservation_id,
            }
        } else if self.eat_kw("RELEASE") {
            let resource = self.ident()?;
            let key = self.bracket_expr()?;
            let amount = self.expr()?;
            self.kw("AS")?;
            let reservation_id = self.expr()?;
            EffectKindAst::Release {
                resource,
                key,
                amount,
                reservation_id,
            }
        } else if self.eat_kw("TRANSFER") {
            let amount = self.expr()?;
            self.kw("FROM")?;
            let source = self.path()?;
            self.kw("TO")?;
            let destination = self.path()?;
            EffectKindAst::Transfer {
                amount,
                source,
                destination,
            }
        } else if self.eat_kw("ADVANCE") {
            let path = self.path()?;
            self.kw("FROM")?;
            let expected = self.ident()?;
            self.kw("TO")?;
            let next = self.ident()?;
            EffectKindAst::AdvanceState {
                path,
                expected,
                next,
            }
        } else if self.eat_kw("EMIT") {
            let record = self.ident()?;
            let fields = self.field_inits()?;
            EffectKindAst::Emit { record, fields }
        } else {
            return self.err("expected effect keyword");
        };
        self.eat(Tok::Comma);
        Ok(Effect { span, guard, kind })
    }

    fn contract(&mut self) -> CoreResult<ContractDecl> {
        let span = self.span();
        self.expect(Tok::LBrace)?;
        let mut fields = Vec::new();
        while !self.eat(Tok::RBrace) {
            let name = self.ident()?;
            self.expect(Tok::Colon)?;
            let v = self.contract_value()?;
            fields.push((name, v));
            self.eat(Tok::Comma);
        }
        Ok(ContractDecl { span, fields })
    }

    fn contract_value(&mut self) -> CoreResult<ContractValue> {
        match self.peek().clone() {
            Tok::Str(s) => {
                self.bump();
                Ok(ContractValue::Str(s))
            }
            Tok::Int(v) => {
                self.bump();
                Ok(ContractValue::Int(u64::try_from(v).map_err(|_| {
                    CoreError::new(ErrorCode::ParseError, "bad contract int")
                })?))
            }
            Tok::LBrace => {
                self.bump();
                let mut items = Vec::new();
                while !self.eat(Tok::RBrace) {
                    items.push(self.contract_value()?);
                    if !self.eat(Tok::Comma) {
                        self.expect(Tok::RBrace)?;
                        break;
                    }
                }
                Ok(ContractValue::Set(items))
            }
            Tok::LBracket => {
                self.bump();
                let mut items = Vec::new();
                while !self.eat(Tok::RBracket) {
                    items.push(self.contract_value()?);
                    if !self.eat(Tok::Comma) {
                        self.expect(Tok::RBracket)?;
                        break;
                    }
                }
                Ok(ContractValue::List(items))
            }
            Tok::Ident(_) => {
                let name = self.ident()?;
                if *self.peek() == Tok::LBracket {
                    // Record[param]
                    self.bump();
                    let param = self.ident()?;
                    self.expect(Tok::RBracket)?;
                    return Ok(ContractValue::KeyRef {
                        record: name,
                        param,
                    });
                }
                let mut args = Vec::new();
                if self.eat(Tok::LParen) {
                    while !self.eat(Tok::RParen) {
                        args.push(self.contract_value()?);
                        if !self.eat(Tok::Comma) {
                            self.expect(Tok::RParen)?;
                            break;
                        }
                    }
                }
                Ok(ContractValue::Ctor { name, args })
            }
            _ => self.err("expected contract value"),
        }
    }

    // ---- expressions --------------------------------------------------
    pub(crate) fn expr(&mut self) -> CoreResult<Expr> {
        self.depth += 1;
        if self.depth > self.limits.max_expr_depth {
            return Err(CoreError::new(
                ErrorCode::ResourceLimit,
                "expression depth exceeds limit",
            ));
        }
        let r = self.or_expr();
        self.depth -= 1;
        r
    }

    fn or_expr(&mut self) -> CoreResult<Expr> {
        let mut lhs = self.and_expr()?;
        while self.eat(Tok::OrOr) {
            let rhs = self.and_expr()?;
            lhs = bin(BinOp::Or, lhs, rhs);
        }
        Ok(lhs)
    }
    fn and_expr(&mut self) -> CoreResult<Expr> {
        let mut lhs = self.not_expr()?;
        while self.eat(Tok::AndAnd) {
            let rhs = self.not_expr()?;
            lhs = bin(BinOp::And, lhs, rhs);
        }
        Ok(lhs)
    }
    fn not_expr(&mut self) -> CoreResult<Expr> {
        let span = self.span();
        if self.eat(Tok::Bang) {
            let e = self.not_expr()?;
            return Ok(Expr {
                span,
                node: ExprNode::Not(Box::new(e)),
            });
        }
        self.cmp_expr()
    }
    fn cmp_expr(&mut self) -> CoreResult<Expr> {
        let lhs = self.add_expr()?;
        let op = match self.peek() {
            Tok::EqEq => Some(CmpOp::Eq),
            Tok::Ne => Some(CmpOp::Ne),
            Tok::Lt => Some(CmpOp::Lt),
            Tok::Le => Some(CmpOp::Le),
            Tok::Gt => Some(CmpOp::Gt),
            Tok::Ge => Some(CmpOp::Ge),
            _ => None,
        };
        if let Some(op) = op {
            self.bump();
            let rhs = self.add_expr()?;
            return Ok(bin(BinOp::Cmp(op), lhs, rhs));
        }
        if self.is_kw("IN") {
            self.bump();
            let rhs = self.add_expr()?;
            return Ok(bin(BinOp::In, lhs, rhs));
        }
        if self.is_kw("IS") {
            self.bump();
            let span = lhs.span;
            if self.eat_kw("NONE") {
                return Ok(Expr {
                    span,
                    node: ExprNode::IsNone(Box::new(lhs)),
                });
            }
            if self.eat_kw("SOME") {
                return Ok(Expr {
                    span,
                    node: ExprNode::IsSome(Box::new(lhs)),
                });
            }
            return self.err("expected NONE or SOME after IS");
        }
        Ok(lhs)
    }
    fn add_expr(&mut self) -> CoreResult<Expr> {
        let mut lhs = self.mul_expr()?;
        loop {
            if self.eat(Tok::Plus) {
                let rhs = self.mul_expr()?;
                lhs = bin(BinOp::Add, lhs, rhs);
            } else if self.eat(Tok::Minus) {
                let rhs = self.mul_expr()?;
                lhs = bin(BinOp::Sub, lhs, rhs);
            } else if self.eat(Tok::QuestionQuestion) {
                let rhs = self.mul_expr()?;
                let span = lhs.span;
                lhs = Expr {
                    span,
                    node: ExprNode::UnwrapOr {
                        value: Box::new(lhs),
                        default: Box::new(rhs),
                    },
                };
            } else {
                break;
            }
        }
        Ok(lhs)
    }
    fn mul_expr(&mut self) -> CoreResult<Expr> {
        let mut lhs = self.unary()?;
        while self.eat(Tok::Star) {
            let rhs = self.unary()?;
            lhs = bin(BinOp::Mul, lhs, rhs);
        }
        Ok(lhs)
    }
    fn unary(&mut self) -> CoreResult<Expr> {
        let span = self.span();
        if self.eat(Tok::Minus) {
            let e = self.unary()?;
            return Ok(Expr {
                span,
                node: ExprNode::Neg(Box::new(e)),
            });
        }
        self.postfix()
    }
    fn postfix(&mut self) -> CoreResult<Expr> {
        let mut e = self.primary()?;
        loop {
            if self.eat(Tok::Dot) {
                let field = self.ident()?;
                let span = e.span;
                e = Expr {
                    span,
                    node: ExprNode::Field {
                        base: Box::new(e),
                        field,
                    },
                };
            } else {
                break;
            }
        }
        Ok(e)
    }
    fn primary(&mut self) -> CoreResult<Expr> {
        let span = self.span();
        let node = match self.peek().clone() {
            Tok::Int(v) => {
                self.bump();
                ExprNode::IntLit(v)
            }
            Tok::Dec(d) => {
                self.bump();
                ExprNode::DecimalLit(d)
            }
            Tok::Str(s) => {
                self.bump();
                ExprNode::StrLit(s)
            }
            Tok::Uuid(u) => {
                self.bump();
                ExprNode::UuidLit(u)
            }
            Tok::Bytes(b) => {
                self.bump();
                ExprNode::BytesLit(b)
            }
            Tok::LParen => {
                self.bump();
                let first = self.expr()?;
                if self.eat(Tok::RParen) {
                    return Ok(first);
                }
                let mut items = vec![first];
                while self.eat(Tok::Comma) {
                    items.push(self.expr()?);
                }
                self.expect(Tok::RParen)?;
                ExprNode::Tuple(items)
            }
            Tok::LBrace => {
                let fields = self.field_inits()?;
                ExprNode::Struct(fields)
            }
            Tok::Hash => {
                self.bump();
                self.expect(Tok::LBrace)?;
                let mut items = Vec::new();
                while !self.eat(Tok::RBrace) {
                    items.push(self.expr()?);
                    if !self.eat(Tok::Comma) {
                        self.expect(Tok::RBrace)?;
                        break;
                    }
                }
                ExprNode::SetLit(items)
            }
            Tok::Ident(name) => match name.as_str() {
                "true" => {
                    self.bump();
                    ExprNode::BoolLit(true)
                }
                "false" => {
                    self.bump();
                    ExprNode::BoolLit(false)
                }
                "Unit" => {
                    self.bump();
                    ExprNode::Unit
                }
                "None" => {
                    self.bump();
                    ExprNode::None
                }
                "Some" => {
                    self.bump();
                    self.expect(Tok::LParen)?;
                    let e = self.expr()?;
                    self.expect(Tok::RParen)?;
                    ExprNode::Some(Box::new(e))
                }
                "SIZE" => {
                    self.bump();
                    self.expect(Tok::LParen)?;
                    let e = self.expr()?;
                    self.expect(Tok::RParen)?;
                    ExprNode::Size(Box::new(e))
                }
                "SUM" => {
                    self.bump();
                    self.expect(Tok::LParen)?;
                    let value = self.expr()?;
                    self.kw("FOR")?;
                    let var = self.ident()?;
                    self.kw("IN")?;
                    let set = self.expr()?;
                    self.expect(Tok::RParen)?;
                    ExprNode::SumOver {
                        var,
                        set: Box::new(set),
                        value: Box::new(value),
                    }
                }
                "EXISTS" => {
                    self.bump();
                    let record = self.ident()?;
                    let key = self.bracket_expr()?;
                    ExprNode::Exists {
                        record,
                        key: Box::new(key),
                    }
                }
                _ => {
                    let id = self.ident()?;
                    if *self.peek() == Tok::DoubleColon {
                        self.bump();
                        let variant = self.ident()?;
                        ExprNode::EnumVariant {
                            enum_name: id,
                            variant,
                        }
                    } else if *self.peek() == Tok::LBracket && self.is_record_lookup(&id) {
                        let key = self.bracket_expr()?;
                        ExprNode::RowLookup {
                            record: id,
                            key: Box::new(key),
                        }
                    } else {
                        ExprNode::Var(id)
                    }
                }
            },
            _ => return self.err("expected expression"),
        };
        Ok(Expr { span, node })
    }

    /// `Name[...]` is a record lookup when `Name` starts with an uppercase letter (records are
    /// declared capitalized by convention); lowercase identifiers are bindings/parameters.
    fn is_record_lookup(&self, id: &Ident) -> bool {
        id.name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_uppercase())
    }
}

fn bin(op: BinOp, lhs: Expr, rhs: Expr) -> Expr {
    let span = lhs.span;
    Expr {
        span,
        node: ExprNode::Bin {
            op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_module() {
        let src = r#"
RECORD Product { id: Uuid PRIMARY KEY, stock: I64 }
INVARIANT stock_nonneg { KIND LowerBound FORALL p IN Product : p.stock >= 0 }
OPERATION sell(item: Uuid, q: I64) VERSION 1 {
  REQUIRE q > 0
  READ { p = Product[item] }
  EFFECT { DECREMENT Product[item].stock BY q }
  ENSURE Product[item].stock >= 0
  RETURN { item: item, q: q }
  CONTRACT {
    atomicity: WholeInvocation,
    input_visibility: SerialScope,
    result_semantics: Receipt,
    result_scope: PerKey(Product[item]),
    session: {},
    session_scope: None,
    durability: LocalStable,
    partition_outcomes: { Unavailable },
    refusal_semantics: BusinessPredicate,
    commitment: FinalWhenDurable,
    request_namespace: "inventory"
  }
}
"#;
        let m = parse_module(src, &Limits::v1()).unwrap();
        assert_eq!(m.items.len(), 3);
        match &m.items[2] {
            Item::Operation(op) => {
                assert_eq!(op.name.name, "sell");
                assert_eq!(op.effects.len(), 1);
                assert_eq!(op.contract.fields.len(), 11);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn rejects_missing_sections() {
        let src = "OPERATION x() VERSION 1 { REQUIRE true EFFECT {} ENSURE true RETURN Unit CONTRACT {} }";
        assert!(parse_module(src, &Limits::v1()).is_err());
    }
}
