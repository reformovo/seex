# Rename PulseOn to Seex without compatibility

PulseOn is renamed to Seex, pronounced “six”, with one brand across the SDK,
CLI, and desktop app. Seex restarts at Python version 0.1.0b0 and Cargo version
0.1.0-beta.0; it uses new package, executable, environment, catalog, local
storage, and workbench identities and deliberately provides no migration,
legacy discovery, import alias, or compatibility layer because the pre-1.0
project has not promised those surfaces.

The product-owned Parquet schema remains the data compatibility boundary.
Historical ADRs and release notes retain the PulseOn name, while current code
and documentation use Seex.
