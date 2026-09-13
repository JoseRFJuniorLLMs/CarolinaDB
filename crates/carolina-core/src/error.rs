//! Stable error codes shared by all crates (SPEC-003 §13, SPEC-004 §16, SPEC-012 §4).

use std::fmt;

/// Stable, machine-readable error code. New codes are appended; existing codes never change meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum ErrorCode {
    // --- frontend / IR (SPEC-003 §13)
    ParseError,
    DuplicateIdentity,
    TypeMismatch,
    MissingRecord,
    NumericOverflow,
    InvalidDecimal,
    UnknownNormalizer,
    UnsupportedEffect,
    UnboundedFootprint,
    UnevaluableInvariant,
    InvalidContract,
    UnsupportedIrVersion,
    NonCanonicalEncoding,
    ResourceLimit,
    IdentityMismatch,
    UnsupportedSessionScope,
    // --- compiler (SPEC-004 §16)
    InvalidIr,
    IncompleteContract,
    InvalidSequentialContract,
    UnenforceableInvariant,
    IncompleteFootprint,
    UnsupportedComposition,
    UnsatisfiableDurability,
    UnmetObservationContract,
    MissingRuntimeCapability,
    InvalidEvidence,
    AnalysisLimit,
    NoSafePlan,
    // --- runtime / storage (SPEC-002 §110, SPEC-004 §16, SPEC-012 §4)
    Io,
    DiskFull,
    Corruption,
    ChecksumMismatch,
    UnsupportedFormat,
    InvalidManifest,
    KeyTooLarge,
    ValueTooLarge,
    BatchTooLarge,
    Conflict,
    StaleEpoch,
    StalePlan,
    StaleSchema,
    TxnAlreadyCommitted,
    TxnAlreadyAborted,
    TxnInDoubt,
    ReadOnly,
    NotReady,
    RequestIdentityMismatch,
    IdentityConflict,
    IdentityExpired,
    ResultExpired,
    OutcomeUnknown,
    AuthorityUnavailable,
    MissingDependency,
    InvariantRejected,
    PreconditionRejected,
    PostconditionRejected,
    ArithmeticError,
    Unavailable,
    ProtocolError,
    UnsupportedCodec,
    IncompatiblePeer,
    NamespaceRetired,
    Internal,
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

/// Error with a stable code, a bounded explanation and an optional IR/source path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreError {
    pub code: ErrorCode,
    pub message: String,
    pub path: Option<String>,
}

impl CoreError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            path: None,
        }
    }
    pub fn at(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.path {
            Some(p) => write!(f, "{}: {} (at {})", self.code, self.message, p),
            None => write!(f, "{}: {}", self.code, self.message),
        }
    }
}

impl std::error::Error for CoreError {}

impl From<std::io::Error> for CoreError {
    fn from(e: std::io::Error) -> Self {
        let code = match e.kind() {
            std::io::ErrorKind::StorageFull => ErrorCode::DiskFull,
            _ => ErrorCode::Io,
        };
        CoreError::new(code, e.to_string())
    }
}

pub type CoreResult<T> = Result<T, CoreError>;

#[macro_export]
macro_rules! bail {
    ($code:expr, $($arg:tt)*) => {
        return Err($crate::error::CoreError::new($code, format!($($arg)*)))
    };
}
