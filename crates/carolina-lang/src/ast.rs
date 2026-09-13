//! Surface AST of the restricted DSL (SPEC-003 §3). Spans are diagnostics only.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub line: u32,
    pub col: u32,
}

impl std::fmt::Display for Span {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.line, self.col)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Record(RecordDecl),
    Enum(EnumDecl),
    Index(IndexDecl),
    Resource(ResourceDecl),
    Invariant(InvariantDecl),
    Operation(OperationDecl),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordDecl {
    pub name: Ident,
    /// `IMMUTABLE` records are insert-only facts (targets of `EMIT`).
    pub immutable: bool,
    pub fields: Vec<FieldDecl>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDecl {
    pub name: Ident,
    pub ty: TypeExpr,
    pub primary_key: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumDecl {
    pub name: Ident,
    pub variants: Vec<Ident>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexDecl {
    pub name: Ident,
    pub record: Ident,
    pub fields: Vec<Ident>,
}

/// `RESOURCE name ON Record (available: f, reserved: f, reservation: ReservationRecord)`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceDecl {
    pub name: Ident,
    pub record: Ident,
    pub available: Ident,
    pub reserved: Ident,
    pub reservation_record: Ident,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeExpr {
    Bool,
    I64,
    U64,
    Uuid,
    Bytes(u32),
    String(u32),
    Decimal(u8, u8),
    Named(Ident),
    Option(Box<TypeExpr>),
    Set(Box<TypeExpr>),
    Tuple(Vec<TypeExpr>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvariantDecl {
    pub name: Ident,
    pub kind_hint: Option<Ident>,
    pub body: InvariantBody,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvariantBody {
    ForAll {
        var: Ident,
        record: Ident,
        filter: Option<Expr>,
        predicate: Expr,
    },
    Unique {
        var: Ident,
        record: Ident,
        filter: Option<Expr>,
        key: Expr,
    },
    Reference {
        var: Ident,
        record: Ident,
        parent_key: Expr,
        parent: Ident,
    },
    Aggregate {
        op: AggregateOp,
        value: Expr,
        var: Ident,
        record: Ident,
        filter: Option<Expr>,
        group_by: Option<Expr>,
        comparison: CmpOp,
        bound: Expr,
    },
    Transition {
        var: Ident,
        record: Ident,
        field: Ident,
        edges: Vec<(Ident, Ident)>,
    },
    /// `REQUIRES op(param) NEEDS FactRecord(param)`
    Requires {
        operation: Ident,
        op_params: Vec<Ident>,
        fact: Ident,
        fact_params: Vec<Ident>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggregateOp {
    Sum,
    Count,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationDecl {
    pub name: Ident,
    pub params: Vec<Param>,
    pub version: u32,
    pub require: Expr,
    pub reads: Vec<ReadDecl>,
    pub effects: Vec<Effect>,
    pub ensure: Expr,
    pub result: Expr,
    pub contract: ContractDecl,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    pub name: Ident,
    pub ty: TypeExpr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadDecl {
    pub binding: Ident,
    pub expr: ReadExpr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadExpr {
    /// `Record[key]` — required row; missing record rejects the candidate.
    Row { record: Ident, key: Expr },
    /// `OPTIONAL Record[key]` — `Option<row>`.
    OptionalRow { record: Ident, key: Expr },
    /// `EXISTS Record[key]` — `Bool`.
    Exists { record: Ident, key: Expr },
    /// `SCAN var IN Record WHERE predicate` — set of rows.
    Scan {
        var: Ident,
        record: Ident,
        filter: Expr,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Effect {
    pub span: Span,
    pub guard: Option<Expr>,
    pub kind: EffectKindAst,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathExpr {
    pub record: Ident,
    pub key: Expr,
    pub field: Ident,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectKindAst {
    Assign {
        path: PathExpr,
        value: Expr,
    },
    Increment {
        path: PathExpr,
        amount: Expr,
    },
    Decrement {
        path: PathExpr,
        amount: Expr,
    },
    Insert {
        record: Ident,
        fields: Vec<(Ident, Expr)>,
    },
    Delete {
        record: Ident,
        key: Expr,
    },
    AddToSet {
        path: PathExpr,
        value: Expr,
    },
    RemoveFromSet {
        path: PathExpr,
        value: Expr,
    },
    CompareAndSwap {
        path: PathExpr,
        expected: Expr,
        value: Expr,
    },
    Reserve {
        resource: Ident,
        key: Expr,
        amount: Expr,
        reservation_id: Expr,
    },
    Release {
        resource: Ident,
        key: Expr,
        amount: Expr,
        reservation_id: Expr,
    },
    Transfer {
        amount: Expr,
        source: PathExpr,
        destination: PathExpr,
    },
    AdvanceState {
        path: PathExpr,
        expected: Ident,
        next: Ident,
    },
    Emit {
        record: Ident,
        fields: Vec<(Ident, Expr)>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractDecl {
    pub span: Span,
    pub fields: Vec<(Ident, ContractValue)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContractValue {
    /// `Name` or `Name(args...)`
    Ctor {
        name: Ident,
        args: Vec<ContractValue>,
    },
    /// `{ A, B }`
    Set(Vec<ContractValue>),
    /// `[ a, b ]`
    List(Vec<ContractValue>),
    Str(String),
    Int(u64),
    /// `Record[param]` inside scope expressions
    KeyRef {
        record: Ident,
        param: Ident,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    And,
    Or,
    Cmp(CmpOp),
    /// set membership `x IN s`
    In,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expr {
    pub span: Span,
    pub node: ExprNode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExprNode {
    IntLit(i128),
    DecimalLit(String),
    StrLit(String),
    BoolLit(bool),
    UuidLit([u8; 16]),
    BytesLit(Vec<u8>),
    Unit,
    None,
    Some(Box<Expr>),
    Var(Ident),
    EnumVariant {
        enum_name: Ident,
        variant: Ident,
    },
    Field {
        base: Box<Expr>,
        field: Ident,
    },
    /// `Record[key]` yields the row (required)
    RowLookup {
        record: Ident,
        key: Box<Expr>,
    },
    /// `EXISTS Record[key]`
    Exists {
        record: Ident,
        key: Box<Expr>,
    },
    Tuple(Vec<Expr>),
    /// `{ a: e, b: e }`
    Struct(Vec<(Ident, Expr)>),
    /// `#{ a, b }`
    SetLit(Vec<Expr>),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    Bin {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    IsNone(Box<Expr>),
    IsSome(Box<Expr>),
    /// `e ?? default`
    UnwrapOr {
        value: Box<Expr>,
        default: Box<Expr>,
    },
    /// `SIZE(set)`
    Size(Box<Expr>),
    /// `SUM(x.f FOR x IN set)`; `COUNT(set)` is `Size`
    SumOver {
        var: Ident,
        set: Box<Expr>,
        value: Box<Expr>,
    },
}
