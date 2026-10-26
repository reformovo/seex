//! Public native connection configuration.

/// Catalog database used by the native store.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CatalogBackend {
    #[default]
    Sqlite,
    /// Legacy test/storage adapter. Public configuration rejects this backend.
    DuckDb,
}

impl CatalogBackend {
    pub(crate) const fn as_name(self) -> &'static str {
        match self {
            Self::DuckDb => "duckdb",
            Self::Sqlite => "sqlite",
        }
    }
}
