use std::collections::HashMap;

use crate::StorageError;

const REQUIRED_COLUMNS: [(&str, LogicalType); 7] = [
    ("run_id", LogicalType::String),
    ("metric_key", LogicalType::String),
    ("metric_key_encoded", LogicalType::String),
    ("step", LogicalType::Int64),
    ("timestamp", LogicalType::Timestamp),
    ("value_f64", LogicalType::Float64),
    ("ingested_at", LogicalType::Timestamp),
];

#[derive(Clone, Copy)]
enum LogicalType {
    String,
    Int64,
    Timestamp,
    Float64,
}

impl LogicalType {
    const fn name(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Int64 => "int64",
            Self::Timestamp => "timestamp",
            Self::Float64 => "float64",
        }
    }

    fn accepts(self, actual: &str) -> bool {
        let actual = actual.trim().to_ascii_uppercase();
        match self {
            Self::String => actual == "VARCHAR",
            Self::Int64 => actual == "BIGINT",
            Self::Timestamp => actual.starts_with("TIMESTAMP"),
            Self::Float64 => actual == "DOUBLE",
        }
    }
}

/// One column reported by DuckDB for a Parquet relation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ColumnSchema {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
}

impl ColumnSchema {
    pub fn new(name: impl Into<String>, data_type: impl Into<String>, nullable: bool) -> Self {
        Self {
            name: name.into(),
            data_type: data_type.into(),
            nullable,
        }
    }
}

/// Compatible schema details, including ignored additive columns.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaReport {
    pub columns: Vec<ColumnSchema>,
    pub additive_columns: Vec<String>,
}

/// Validates the public `metric_points` Parquet compatibility boundary.
///
/// Column order is irrelevant. Unknown nullable columns are compatible, while
/// missing, renamed, retyped, or required additive columns are rejected.
///
/// # Errors
///
/// Returns [`StorageError`] when the schema violates the contract.
pub fn validate_metric_point_schema(
    columns: Vec<ColumnSchema>,
) -> Result<SchemaReport, StorageError> {
    let mut by_name = HashMap::with_capacity(columns.len());
    for column in &columns {
        if by_name.insert(column.name.as_str(), column).is_some() {
            return Err(StorageError::DuplicateColumn {
                name: column.name.clone(),
            });
        }
    }

    for (name, expected) in REQUIRED_COLUMNS {
        let column = by_name
            .get(name)
            .ok_or(StorageError::MissingColumn { name })?;
        if !expected.accepts(&column.data_type) {
            return Err(StorageError::IncompatibleColumnType {
                name,
                expected: expected.name(),
                actual: column.data_type.clone(),
            });
        }
    }

    let mut additive_columns = Vec::new();
    for column in &columns {
        if REQUIRED_COLUMNS
            .iter()
            .any(|(name, _)| *name == column.name)
        {
            continue;
        }
        if !column.nullable {
            return Err(StorageError::IncompatibleAdditiveColumn {
                name: column.name.clone(),
            });
        }
        additive_columns.push(column.name.clone());
    }
    additive_columns.sort();
    Ok(SchemaReport {
        columns,
        additive_columns,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contract_columns() -> Vec<ColumnSchema> {
        vec![
            ColumnSchema::new("run_id", "VARCHAR", false),
            ColumnSchema::new("metric_key", "VARCHAR", false),
            ColumnSchema::new("metric_key_encoded", "VARCHAR", false),
            ColumnSchema::new("step", "BIGINT", false),
            ColumnSchema::new("timestamp", "TIMESTAMP WITH TIME ZONE", false),
            ColumnSchema::new("value_f64", "DOUBLE", false),
            ColumnSchema::new("ingested_at", "TIMESTAMP", false),
        ]
    }

    #[test]
    fn accepts_additive_nullable_columns() {
        let mut columns = contract_columns();
        columns.push(ColumnSchema::new("worker_name", "VARCHAR", true));

        let report = validate_metric_point_schema(columns).expect("schema should be compatible");

        assert_eq!(report.additive_columns, vec!["worker_name"]);
    }

    #[test]
    fn rejects_missing_or_retyped_contract_columns() {
        let mut missing = contract_columns();
        missing.retain(|column| column.name != "step");
        let mut retyped = contract_columns();
        retyped[3].data_type = "INTEGER".to_owned();

        assert!(matches!(
            validate_metric_point_schema(missing),
            Err(StorageError::MissingColumn { name: "step" })
        ));
        assert!(matches!(
            validate_metric_point_schema(retyped),
            Err(StorageError::IncompatibleColumnType { name: "step", .. })
        ));
    }

    #[test]
    fn rejects_required_additive_columns() {
        let mut columns = contract_columns();
        columns.push(ColumnSchema::new("worker_name", "VARCHAR", false));

        assert!(matches!(
            validate_metric_point_schema(columns),
            Err(StorageError::IncompatibleAdditiveColumn { .. })
        ));
    }
}
