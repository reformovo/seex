# DuckDB 2.0 升级：状态、阻塞项与后续优化

## 现状

候选改动（未落地）：

- `crates/seex/Cargo.toml`：`duckdb ~1.10504.0`（DuckDB 1.5.4）→ `~1.20000.1`（DuckDB 2.0.0 LTS）。
- `Cargo.lock`：随之重新解析，`arrow` 58 → 60。
- `crates/seex/src/storage/sampling.rs`：2.0 的 `Statement::step()` 返回
  `Result<Option<StructArray>>`，两处调用点补 `?`。
- `rust-toolchain.toml`：`1.97.1` → `1.99.1`（解 B3，同时带来两个必须随之修的 lint）。
- `crates/seex/src/engine/reporting.rs`：`Atomic::fetch_update` → `try_update`。
- `crates/seex-app/src/desktop/session/persistence.rs`：去掉多余的 `&`
  （clippy `needless_borrows_for_generic_args`）。

行为与 Python API 形状不变；`try_update` 需要 Rust ≥ 1.99，已由 toolchain
文件固定。

## 已验证（Rust 1.99.1 + DuckDB 2.0.0 / duckdb-rs 1.20000.1）

| 检查 | 命令 | 结果 |
| --- | --- | --- |
| 全工作区编译（debug） | `cargo check --workspace --all-targets` | 通过 |
| clippy | `cargo clippy --workspace --all-targets --all-features -- -D warnings` | 通过 |
| rustfmt | `cargo fmt --all -- --check` | 通过 |
| 工作区测试（release） | `cargo test --release --workspace --all-features` | 全部通过（含 seex-app 214 项） |
| Rust 测试（release） | `cargo test --release -p seex` | 151 单测 + 全部集成测试通过 |
| Python 测试 | `pytest`（strip 后的 release wheel） | 107 通过 |
| Python lint/type | `ruff format --check`、`ruff check`、`pyright` | 通过 |

无需额外构建覆盖：`duckdb-rs 1.20000.1` 已修正 bundled 构建的扩展版本号，
裸 `cargo build` 即可正常 `INSTALL ducklake`。

## 上游问题与决策

均为上游问题，seex 自身逻辑无缺陷。B1、B3 已由上游修复；B2 无法在上游
短期解决，已按决策用 release 测试规避。

### B1：bundled 构建的扩展 source id（已解决）

**上游**：<https://github.com/duckdb/duckdb-rs/issues/917>（bundled 2.0 报开发版
版本）。**修复**：PR <https://github.com/duckdb/duckdb-rs/pull/918>，随
`duckdb-rs 1.20000.1` 发布；另有 #920/#923 修 `bundled`（cc）在 Linux 的链接
错误，#922 在 CI 里校验 DuckDB 版本。

`1.20000.0` 把 `DUCKDB_SOURCE_ID` 编成 commit `d54cbbe3df`，导致 `INSTALL
ducklake` 去官方仓库找不存在的路径（HTTP 404）。升级到 `1.20000.1` 后，
不加任何构建覆盖，release 全量测试与 wheel 均正常。

### B2：debug 构建下 DuckLake teardown abort（已用 release 规避）

**上游**：<https://github.com/duckdb/ducklake/issues/1050>（已关闭，维护者在
main 上未复现），关联内联元数据泄漏 #1088 / #1065。官方扩展 build
`417a9117f5` + `duckdb-rs 1.20000.1` 仍可复现。

DuckLake attach 之后 drop connection 会触发：

```text
Assertion triggered in allocator.cpp: allocation_count == 0
```

已用最小探针隔离：只开连接、加载 httpfs、加载 ducklake、attach 原生 DuckDB
文件都正常；只有 attach DuckLake（且有内联数据）时退出 abort。

**决策：测试统一跑 release。** 已落地：

- `.github/workflows/ci.yml`：Rust job 改为
  `cargo test --release --workspace --all-features`。
- `pyproject.toml`：`[tool.uv]` 不再强制 `--profile=dev`，`uv sync` / `uv run`
  默认构建 release。已验证 dev-profile 的 Python 测试同样 abort（exit 134）。
- 本地需要调试 Rust 时仍可用 `maturin develop --profile dev`，但涉及原生
  store 的 Python 测试必须用 release。

后续：可用最小复现（Rust + bundled debug + 官方扩展）去 #1050 重新开
issue，推动上游真正修复。

### B3：本机 `strip = true` 产生不可加载的 Mach-O

**上游**：<https://github.com/rust-lang/rust/issues/157750>（stripping debuginfo
on macOS produces misaligned dylibs），由 LLVM 更新 PR
<https://github.com/rust-lang/rust/pull/158410> 修复，Rust 1.99 起可用；
当前 stable 已是 1.99.1。

`[profile.release] strip = true` 在 1.97.1 上产出的 cdylib 被 dyld 拒绝：

```text
(mis-aligned LINKEDIT string pool, fileOffset=0x029C45D4)
```

**已解决**：`rust-toolchain.toml` 升到 1.99.1 后，默认 `strip = true` 构建的
wheel 可正常导入，Python 测试通过。不再需要 `CARGO_PROFILE_RELEASE_STRIP=none`。

## 平台覆盖（DuckDB 2.0 限制）

DuckDB 2.0 官方只为部分平台发布 `ducklake` 扩展（探测
`extensions.duckdb.org/v2.0.0/<platform>/ducklake.duckdb_extension.gz`）：

| 平台 | 扩展 | CI wheel |
| --- | --- | --- |
| linux_amd64 / linux_arm64 / 两者 musl | 200 | 保留 |
| osx_amd64 / osx_arm64 | 200 | 保留，部署目标 10.13 |
| windows_amd64 / windows_arm64 | 200 | 保留 |
| linux_ppc64le / linux_s390x / linux_armv7 / linux_i686 | 404 | 移除 |

ppc64le 另外还存在 DuckDB 源码在 GCC 上的编译错误
（`SerializationData` 默认移动构造的 exception-specification 不一致），即使
扩展可用也需要上游修复。macOS x86_64 需要 `MACOSX_DEPLOYMENT_TARGET=10.13`，
否则 DuckDB 2.0 的 aligned new/delete 无法编译。

Linux aarch64 改为在原生 arm64 runner（`ubuntu-24.04-arm`）上构建：manylinux
交叉工具链是 GCC 7.5，无法编译 DuckDB 2.0 的 C++17 源码。

b5 发布过 ppc64le/s390x/armv7/32 位 x86 wheel，b6 起不再提供。

## 落地步骤

1. 升级 `duckdb` 到 `~1.20000.1`（已含 #917/#918 修复），确认无需
   `CXXFLAGS` 覆盖。
2. 测试 profile 已改为 release（CI 与 `uv sync`/`uv run` 一致）。
3. 已更新 `README.md` 的引擎版本与离线扩展说明，并在 `0.1.0b6` 发布说明新增
   “Runtime”一节。存储仍是 SQLite catalog + Parquet，engine 升级不需要迁移。

## 升级后的优化计划

按“收益/风险”排序，均不改变 Parquet 兼容边界。

1. **行组裁剪与分区感知（最高性价比，零 schema 代价）**
   2.0 把 zone map / Parquet bloom 扩展到 `IN`、函数谓词、struct/list/
decimal/UUID，planner 直接利用 lakehouse 分区；DuckLake 1.1 又补了 OR、
`IS NULL`、struct 字段和 join build 侧下推。`metric_points` 已按
`(run_id, metric_key_encoded)` 分区，冷读放大应直接下降。用现有
`bench_minio_metric_query.py` 的替代路径或本地基准量读放大。
2. **DuckLake 1.1 catalog 操作**
   attach/列表与内联提交显著变快（官方 PG catalog 基准 27×/102×）。影响
   `init/open` 冷启动与 flush 提交，先重跑现有基准确认落到自己的路径上。
3. **表级布局选项**
   `CREATE TABLE ... WITH (parquet_compression=...)`、`ALTER TABLE ... SET/
RESET`、`ducklake_merge_adjacent_files(newer_than/older_than)`、
`rewrite_data_files(max_compacted_files)`：按表定压缩，只压冷窗口，避免重写
热文件。
4. **执行器白拿项**
   聚合可溢写、partial aggregate 下推到 join 之下、冗余聚合复用。
5. **metric aggregates 的增量维护**
   触发器在 DuckLake 表上不可用、2.0 也没有物化视图（已实测）。沿用
   “watermark 找脏 key + 按 `(run_id, metric_key)` 重算”的模式，参考
   DuckDB streaming patterns 的 delta processor；聚合语义必须保留
   last-write-wins 去重，不能照搬可加计数。
6. **暂缓**
   异步 I/O 对本地冷读只有约 1.5×，当前存储已移除对象存储面，优先级低；
   EFC 对本地文件默认不生效（`cache_local_files=false`）；Quack server 与
   `VARIANT` 元数据存储留作独立 ADR。

## 未验证

- 完整性能门禁（`scripts/performance_gate.py`）与 A-B-B-A 对比未跑。
- debug-profile 验收不再运行；原生 store 测试统一 release（见 B2）。
- Linux/Windows 构建未验证。
