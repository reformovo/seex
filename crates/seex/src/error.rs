//! Public SDK errors without storage-engine details.

use std::fmt;

/// A result produced by the public Seex SDK.
pub type Result<T> = std::result::Result<T, Error>;

/// Matchable public failure categories.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Error {
    Configuration,
    UnsupportedQuery,
    Storage,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configuration => formatter.write_str("Seex configuration is invalid"),
            Self::UnsupportedQuery => formatter.write_str("Reader query is not yet supported"),
            Self::Storage => formatter.write_str("Seex storage operation failed"),
        }
    }
}

impl std::error::Error for Error {}
