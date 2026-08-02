//! Public native connection configuration.

use std::fmt;

/// Catalog database used by the native store.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CatalogBackend {
    #[default]
    DuckDb,
    Sqlite,
}

impl CatalogBackend {
    pub(crate) const fn as_name(self) -> &'static str {
        match self {
            Self::DuckDb => "duckdb",
            Self::Sqlite => "sqlite",
        }
    }
}

/// Field-level S3 connection overrides for [`crate::ClientBuilder`].
#[derive(Clone, Default)]
pub struct S3Options {
    pub(crate) endpoint: Option<String>,
    pub(crate) access_key_id: Option<String>,
    pub(crate) secret_access_key: Option<String>,
    pub(crate) session_token: Option<String>,
    pub(crate) region: Option<String>,
    pub(crate) path_style: Option<bool>,
    pub(crate) use_ssl: Option<bool>,
}

impl S3Options {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn endpoint(mut self, value: impl Into<String>) -> Self {
        self.endpoint = Some(value.into());
        self
    }

    pub fn access_key_id(mut self, value: impl Into<String>) -> Self {
        self.access_key_id = Some(value.into());
        self
    }

    pub fn secret_access_key(mut self, value: impl Into<String>) -> Self {
        self.secret_access_key = Some(value.into());
        self
    }

    pub fn session_token(mut self, value: impl Into<String>) -> Self {
        self.session_token = Some(value.into());
        self
    }

    pub fn region(mut self, value: impl Into<String>) -> Self {
        self.region = Some(value.into());
        self
    }

    pub const fn path_style(mut self, value: bool) -> Self {
        self.path_style = Some(value);
        self
    }

    pub const fn use_ssl(mut self, value: bool) -> Self {
        self.use_ssl = Some(value);
        self
    }
}

impl fmt::Debug for S3Options {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("S3Options")
            .field("endpoint", &self.endpoint)
            .field(
                "access_key_id",
                &self.access_key_id.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "secret_access_key",
                &self.secret_access_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "session_token",
                &self.session_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("region", &self.region)
            .field("path_style", &self.path_style)
            .field("use_ssl", &self.use_ssl)
            .finish()
    }
}
