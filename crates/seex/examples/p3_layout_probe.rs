//! Compare isolated DuckLake metric layouts with one deterministic distribution.

use std::error::Error;
use std::fs;
use std::path::Path;
use std::time::Instant;

const SEED: u64 = 0x5EEC_0001;

fn sql_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn data_stats(path: &Path) -> Result<(usize, usize, u64, Vec<u64>), Box<dyn Error>> {
    let mut stack = vec![path.to_owned()];
    let mut files = Vec::new();
    let mut directories = 0;
    while let Some(path) = stack.pop() {
        directories += 1;
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path
                .extension()
                .is_some_and(|extension| extension == "parquet")
            {
                files.push(entry.metadata()?.len());
            }
        }
    }
    files.sort_unstable();
    Ok((directories, files.len(), files.iter().sum(), files))
}

fn reader(catalog: &Path, data: &Path) -> Result<duckdb::Connection, Box<dyn Error>> {
    let connection = duckdb::Connection::open_in_memory()?;
    connection.execute_batch(&format!(
        "SET threads=1; LOAD ducklake; ATTACH {} AS dl (READ_ONLY, DATA_PATH {});
         SET enable_logging=true; SET logging_level='DEBUG';",
        sql_string(&format!("ducklake:sqlite:{}", catalog.display())),
        sql_string(&data.to_string_lossy())
    ))?;
    Ok(connection)
}

fn scan_logs(connection: &duckdb::Connection, query: &str) -> Result<(), Box<dyn Error>> {
    connection.execute_batch("SET enable_logging=false;")?;
    let mut statement = connection.prepare(
        "SELECT event, count(*) FROM duckdb_logs_parsed('PhysicalOperator')
         WHERE class = 'ParquetReader' GROUP BY event ORDER BY event",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut read_groups = 0;
    for row in rows {
        let (event, count) = row?;
        println!("SCAN_LOG query={query} event={event} row_groups={count}");
        if event == "ReadRowGroup" {
            read_groups += count;
        }
    }
    if read_groups == 0 {
        return Err("missing Parquet row-group read evidence".into());
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let root = args.next().ok_or("expected output directory")?;
    let layout = args.next().ok_or("expected dual, bucket, or shared")?;
    let short_runs: usize = args.next().unwrap_or_else(|| "16".to_owned()).parse()?;
    let long_points: usize = args.next().unwrap_or_else(|| "10000".to_owned()).parse()?;
    let short_keys: usize = args.next().unwrap_or_else(|| "8".to_owned()).parse()?;
    let batch_runs: usize = args
        .next()
        .unwrap_or_else(|| short_runs.to_string())
        .parse()?;
    if batch_runs == 0 || short_runs <= 10 || short_keys <= 2 || long_points <= 200 {
        return Err(
            "require batch_runs > 0, short_runs > 10, short_keys > 2, long_points > 200".into(),
        );
    }
    if !matches!(layout.as_str(), "dual" | "bucket" | "shared") {
        return Err("layout must be dual, bucket, or shared".into());
    }
    let root = Path::new(&root);
    // Never mix a diagnostic run with an existing store or previous results.
    fs::create_dir(root)?;
    let catalog = root.join("catalog.sqlite");
    let data = root.join("data");
    fs::create_dir_all(&data)?;
    let connection = duckdb::Connection::open_in_memory()?;
    connection.execute_batch("SET threads=1;")?;
    connection.execute_batch(&format!(
        "INSTALL ducklake; LOAD ducklake;
         ATTACH {} AS dl (DATA_PATH {}, METADATA_CATALOG 'layout_meta');
         CREATE TABLE dl.metric_points (
             run_id VARCHAR, metric_key VARCHAR, metric_key_encoded VARCHAR,
             step BIGINT, timestamp TIMESTAMPTZ, value_f64 DOUBLE,
             ingested_at TIMESTAMPTZ
         );
         CALL dl.set_option('data_inlining_row_limit', 0, table_name => 'metric_points');",
        sql_string(&format!("ducklake:sqlite:{}", catalog.display())),
        sql_string(&data.to_string_lossy())
    ))?;
    let versions: (String, String) = connection.query_row(
        "SELECT version(), extension_version FROM duckdb_extensions() WHERE extension_name='ducklake'",
        [], |row| Ok((row.get(0)?, row.get(1)?))
    )?;
    println!("duckdb={} ducklake={}", versions.0, versions.1);
    let partition = match layout.as_str() {
        "dual" => Some("(run_id, metric_key_encoded)"),
        "bucket" => Some("(bucket(64, run_id))"),
        _ => None,
    };
    if let Some(partition) = partition {
        connection.execute_batch(&format!(
            "ALTER TABLE dl.metric_points SET PARTITIONED BY {partition};"
        ))?;
    }
    if layout != "dual" {
        connection.execute_batch(
            "ALTER TABLE dl.metric_points SET SORTED BY (run_id, metric_key, step);",
        )?;
    }
    let order = if layout == "dual" {
        ""
    } else {
        "ORDER BY run_id, metric_key, step"
    };
    let write_started = Instant::now();
    for start in (0..short_runs).step_by(batch_runs) {
        let end = start.saturating_add(batch_runs).min(short_runs);
        connection.execute_batch(&format!(
            "INSERT INTO dl.metric_points
         SELECT run_id, metric_key, metric_key, step, fixed_time,
                ((step * 17 + {SEED}) % 1009)::DOUBLE / 1009, fixed_time
         FROM (
             SELECT printf('run-%06d', run_index) AS run_id,
                    printf('metric-%02d', key_index) AS metric_key,
                    step_index * 3 AS step,
                    '2026-01-01T00:00:00Z'::TIMESTAMPTZ AS fixed_time
             FROM range({start}, {end}) AS runs(run_index),
                  range({short_keys}) AS keys(key_index), range(4) AS steps(step_index)
             UNION ALL
             SELECT 'run-000010', 'metric-long', long_step,
                    '2026-01-01T00:00:00Z'::TIMESTAMPTZ
             FROM range({long_points}) AS points(long_step)
             WHERE {start} <= 10 AND 10 < {end}
         ) {order};"
        ))?;
    }
    let write_ms = write_started.elapsed().as_millis();
    let (_, before_files, before_bytes, _) = data_stats(&data)?;
    let compact_started = Instant::now();
    let mut compacted_files = 0_i64;
    let mut created_files = 0_i64;
    let mut converged = false;
    for _ in 0..64 {
        let mut statement = connection.prepare(
            "CALL ducklake_merge_adjacent_files('dl', 'metric_points', max_compacted_files => 10);",
        )?;
        let merges =
            statement.query_map([], |row| Ok((row.get::<_, i64>(2)?, row.get::<_, i64>(3)?)))?;
        let mut processed_this_pass = 0;
        for merge in merges {
            let (processed, created) = merge?;
            processed_this_pass += processed;
            created_files += created;
        }
        compacted_files += processed_this_pass;
        if processed_this_pass == 0 {
            converged = true;
            break;
        }
    }
    if !converged {
        return Err("compaction did not converge within 64 passes".into());
    }
    let compact_ms = compact_started.elapsed().as_millis();
    let active_files: i64 = connection.query_row(
        "SELECT count(*) FROM layout_meta.ducklake_data_file WHERE end_snapshot IS NULL",
        [],
        |row| row.get(0),
    )?;
    let (_, _, retained_bytes, _) = data_stats(&data)?;
    connection.execute_batch("CALL ducklake_cleanup_old_files('dl', cleanup_all => true);")?;
    let total: i64 = connection.query_row("SELECT count(*) FROM dl.metric_points", [], |row| {
        row.get(0)
    })?;
    assert_eq!(total as usize, short_runs * short_keys * 4 + long_points);
    drop(connection);
    // Fresh DuckDB connections avoid the writer's Parquet buffer cache.
    // The operating-system page cache is deliberately not cleared.
    let connection = reader(&catalog, &data)?;
    let short_profile = root.join("short-query-profile.json");
    connection.execute_batch(&format!(
        "PRAGMA enable_profiling='json'; PRAGMA profiling_output={};",
        sql_string(&short_profile.to_string_lossy())
    ))?;
    let short_count: i64 = connection.query_row(
        "SELECT count(*), sum(value_f64) FROM dl.metric_points
         WHERE run_id = 'run-000010' AND metric_key = 'metric-02' AND step BETWEEN 0 AND 9",
        [],
        |row| row.get(0),
    )?;
    connection.execute_batch("PRAGMA disable_profiling;")?;
    assert_eq!(short_count, 4);
    scan_logs(&connection, "short")?;
    drop(connection);
    let connection = reader(&catalog, &data)?;
    let long_profile = root.join("long-query-profile.json");
    connection.execute_batch(&format!(
        "PRAGMA enable_profiling='json'; PRAGMA profiling_output={};",
        sql_string(&long_profile.to_string_lossy())
    ))?;
    let long_count: i64 = connection.query_row(
        "SELECT count(*), sum(value_f64) FROM dl.metric_points WHERE run_id = 'run-000010'
         AND metric_key = 'metric-long' AND step BETWEEN 100 AND 200",
        [],
        |row| row.get(0),
    )?;
    connection.execute_batch("PRAGMA disable_profiling;")?;
    assert_eq!(long_count, 101);
    scan_logs(&connection, "long")?;
    let plan: String = connection.query_row(
        "EXPLAIN ANALYZE SELECT count(*), sum(value_f64) FROM dl.metric_points
         WHERE run_id = 'run-000010' AND metric_key = 'metric-long' AND step BETWEEN 100 AND 200",
        [],
        |row| row.get(1),
    )?;
    let glob = sql_string(&format!("{}/**/*.parquet", data.display()));
    let mut metadata = connection.prepare(&format!(
        "SELECT file_name, row_group_id, path_in_schema, stats_min_value, stats_max_value
         FROM parquet_metadata({glob})
         WHERE path_in_schema IN ('run_id', 'metric_key', 'step')
         ORDER BY file_name, row_group_id, path_in_schema LIMIT 30"
    ))?;
    let metadata_rows = metadata
        .query_map([], |row| {
            Ok(format!(
                "{} group={} column={} min={:?} max={:?}",
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(metadata);
    // Count row groups whose independent min/max bounds permit the exact key.
    // These are pruning candidates, not a claim about decoded rows or actual I/O.
    for (key, step_min, step_max) in [("metric-02", 0, 9), ("metric-long", 100, 200)] {
        let (groups, candidates): (i64, i64) = connection.query_row(&format!(
            "WITH bounds AS (
                SELECT file_name, row_group_id,
                    max(CASE WHEN path_in_schema = 'run_id' THEN stats_min_value END) AS run_min,
                    max(CASE WHEN path_in_schema = 'run_id' THEN stats_max_value END) AS run_max,
                    max(CASE WHEN path_in_schema = 'metric_key' THEN stats_min_value END) AS key_min,
                    max(CASE WHEN path_in_schema = 'metric_key' THEN stats_max_value END) AS key_max,
                    max(CASE WHEN path_in_schema = 'step' THEN stats_min_value::BIGINT END) AS step_min,
                    max(CASE WHEN path_in_schema = 'step' THEN stats_max_value::BIGINT END) AS step_max
                FROM parquet_metadata({glob}) GROUP BY file_name, row_group_id
             ) SELECT count(*), count(*) FILTER (WHERE run_min <= 'run-000010'
               AND run_max >= 'run-000010' AND key_min <= {key} AND key_max >= {key}
               AND step_min <= {step_max} AND step_max >= {step_min}) FROM bounds",
            key = sql_string(key)
        ), [], |row| Ok((row.get(0)?, row.get(1)?)))?;
        println!("ROW_GROUP_CANDIDATES key={key} total={groups} candidates={candidates}");
    }
    drop(connection);
    let (directories, file_count, data_bytes, file_sizes) = data_stats(&data)?;
    let catalog_bytes = fs::metadata(&catalog)?.len();
    assert_eq!(file_count as i64, active_files);
    println!(
        "layout={layout} seed={SEED} short_runs={short_runs} short_keys={short_keys} long_points={long_points} \
         directories={directories} files={file_count} data_bytes={data_bytes} \
         catalog_bytes={catalog_bytes} write_ms={write_ms} compact_ms={compact_ms} short_count={short_count} \
         batch_runs={batch_runs} before_files={before_files} before_bytes={before_bytes} \
         active_files={active_files} compacted_files={compacted_files} created_files={created_files} \
         bytes_written_by_compaction={}",
        retained_bytes.saturating_sub(before_bytes)
    );
    println!("file_sizes={file_sizes:?}");
    for row in metadata_rows {
        println!("ROW_GROUP {row}");
    }
    println!("PLAN_BEGIN\n{plan}\nPLAN_END");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_sql_paths() {
        assert_eq!(sql_string("a'b"), "'a''b'");
    }

    #[test]
    fn counts_only_parquet_recursively() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("nested")).unwrap();
        fs::write(root.path().join("nested/a.parquet"), [0; 7]).unwrap();
        fs::write(root.path().join("ignored.json"), [0; 20]).unwrap();
        assert_eq!(data_stats(root.path()).unwrap(), (2, 1, 7, vec![7]));
    }
}
