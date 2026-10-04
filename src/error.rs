use std::fmt;

/// Every failure is explicit: the tool never guesses past syntax it cannot validate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The frame ends before a syntax element.
    Truncated(String),
    /// Syntax that ETSI TS 102 366 allows but this parser does not model or validate.
    Unsupported(String),
    /// A value the specification forbids, or a failed consistency check.
    Invalid(String),
    /// An edit plan this version cannot apply.
    NotSupportedYet(String),
    /// The edit would invalidate EMDF protection and the caller did not allow it.
    Protected(String),
    Io(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Truncated(s) => write!(f, "truncated: {s}"),
            Error::Unsupported(s) => write!(f, "unsupported syntax: {s}"),
            Error::Invalid(s) => write!(f, "invalid stream: {s}"),
            Error::NotSupportedYet(s) => write!(f, "not supported yet: {s}"),
            Error::Protected(s) => write!(f, "EMDF protection: {s}"),
            Error::Io(s) => write!(f, "I/O: {s}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

pub fn unsupported<T>(s: impl Into<String>) -> Result<T> {
    Err(Error::Unsupported(s.into()))
}

pub fn invalid<T>(s: impl Into<String>) -> Result<T> {
    Err(Error::Invalid(s.into()))
}
