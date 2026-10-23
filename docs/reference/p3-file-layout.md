# P3 文件布局评估

## 结论与边界

选择**跨短序列共享、按 `(run_id, metric_key, step)` 排序**的布局方向，
不选择现有双键分区或固定 64 个 Run bucket。P3 的布局评估已完成；
本次不修改 Native v3 生产存储、公共 API 或 Parquet schema。
DuckLake 继续拥有文件清单；探针读取其元数据，不建立第二份清单。

选择依据是短序列基数增长不再导致每序列一个文件，且在达到目标文件大小后，
精确 Run/key 和窄 Step 查询实际跳过绝大多数 row group。这个结论只证明
候选布局可行，不证明 P7 规模验收或生产写入预算已通过。

## 可重复负载与测量方法

- 环境：Apple M4 Pro、48 GiB RAM、macOS Darwin 27.0.0、arm64；
  debug 构建，Rust 1.97.1（根 toolchain），DuckDB v1.5.4、DuckLake `d318a545`。
- 固定 seed `0x5EEC_0001`；256 个 Run，每 Run 32 个短 key，每序列 4 点，
  Step 为 0/3/6/9；Run 10 另有一条长序列。每批 16 个 Run，共 16 批。
- 三种布局使用相同事实。双键分区为 `(run_id, metric_key_encoded)`；
  bucket 使用 `bucket(64, run_id)`；shared 不分区。后两者设置 DuckLake
  排序并显式排序 INSERT 输入，不能把声明排序误当作输入已经有序。
- 禁止 data inlining；每次最多合并 10 个文件，最多 64 轮，必须收敛；
  清理旧文件后计稳态文件。物理写入放大 =（初始文件字节 + 合并产生字节）/稳态字节，
  不包含 catalog/WAL 写入，不能当作设备层写入放大。
- 短查询取 Run 10 / metric-02 / Step 0–9，长查询取 metric-long / Step 100–200；
  都读取 value（`count(*), sum(value_f64)`）。全表点数、短查询 4 点和长查询
  101 点是 Acceptance 断言，另检查清理后物理文件数等于活跃文件数。
- 两个查询分别重新建立只读连接，排除 writer 的 DuckDB buffer cache；
  **不清 OS page cache**。JSON profile 记录读取文件与字节；
  `duckdb_logs_parsed('PhysicalOperator')` 的 ParquetReader ReadRowGroup /
  SkipRowGroup 记录该版本、该查询的实际 row-group 行为，不等同于磁盘 I/O
  次数或解码行数，也不保证其他负载的扫描边界。独立 min/max 候选数只是辅助证据。
- DuckLake 的 `operator_rows_scanned` 在不同布局都报告全表点数，不能解释为
  实际解码行数；也不能把返回的 4/101 点当作扫描点数。

## 两千万点长序列：达到 64–256 MiB 文件目标

共有 8,193 条序列、20,032,768 点。大小均为二进制单位；RSS 为整个探针进程峰值，
包含生成、排序、合并和查询，不是单独的 Reader RSS。

| 指标 | 双键分区 | 64 bucket | shared sorted |
|---|---:|---:|---:|
| 目录数（含数据根） | 8,452 | 64 | 3 |
| 初始 → 稳态文件数 | 8,193 → 8,193 | 224 → 61 | 16 → 1 |
| 稳态数据 MiB | 92.98 | 160.51 | 83.89 |
| 文件 min / p50 / p95 KiB | 1.12 / 1.13 / 1.13 | 1.95 / 4.73 / 6.83 | 85,901.77 / 同左 / 同左 |
| 最大文件 MiB | 83.95 | 160.23 | 83.89 |
| catalog MiB | 4.375 | 0.266 | 0.152 |
| 物理写入放大 | 1.00 | 1.53 | 2.00 |
| 单次写入 / 合并 ms | 7,540 / 51 | 15,030 / 6,553 | 14,331 / 5,739 |
| 峰值 RSS MiB | 386.59 | 5,376.30 | 4,581.75 |
| 短查询文件 / read RG / skip RG | 1 / 1 / 0 | 1 / 2 / 161 | 1 / 2 / 162 |
| 长查询文件 / read RG / skip RG | 1 / 1 / 171 | 1 / 2 / 161 | 1 / 2 / 162 |
| 短 / 长查询读取字节 | 1,151 / 142,626 | 165,500 / 173,067 | 148,376 / 155,863 |

shared 中两种查询的 ParquetReader 日志各报告读取 2/164 个 row group，
跳过其余 162 个，说明这个样例没有退化为全 row-group 扫描。
bucket 也能裁剪，但 60 个剩余文件仍只有 KiB 级，且该样例数据字节与 RSS 更高。
双键分区裁剪准确，却留下 8,192 个约 1 KiB 短序列文件，违反文件数目标。

## 短序列基数对照

固定长序列为一百万点，将每 Run 的短 key 从 32 增至 128：
实际序列从 8,193 增至 32,769，事实点数从 1,032,768 增至 1,131,072。

| 指标（32 → 128 keys） | 双键分区 | 64 bucket | shared sorted |
|---|---:|---:|---:|
| 稳态文件数 | 8,193 → 32,769 | 61 → 61 | 1 → 1 |
| 稳态字节 | 13,866,115 → 42,324,163 | 8,707,167 → 9,207,808 | 4,430,862 → 4,525,115 |
| catalog 字节 | 4,587,520 → 18,006,016 | 274,432 → 274,432 | 159,744 → 159,744 |

128-key 样例中，shared 的短/长查询各读 2、跳过 8 个 row group；bucket 各读 2、
跳过 7 个；双键分区各读 1 个。小数据总量本来不足 64 MiB，不要求填充到目标大小。

## 复现与证据

```sh
cargo build -p seex --example p3_layout_probe
mkdir -p .tmp/p3-reproduce
for layout in dual bucket shared; do
  /usr/bin/time -l target/debug/examples/p3_layout_probe \
    ".tmp/p3-reproduce/$layout" "$layout" 256 20000000 32 16 \
    > ".tmp/p3-reproduce/$layout.txt" 2> ".tmp/p3-reproduce/$layout-rss.txt"
done
```

参数为新输出目录、布局、短 Run 数、长点数、每 Run 短 key 数、每批 Run 数；
目录必须不存在，探针不会覆盖旧 store。`time -l` 为 macOS 命令。
将长点数改为 1000000，并分别使用 32/128 keys 可复现基数对照。
原始输出保留在本机 `.tmp/p3-final-20261003/`、`.tmp/p3-cold-20261003/`、
`.tmp/p3-cardinality-20261003/`，每个目录包含三种布局的文本、RSS 和查询 JSON。
这些诊断数据不入 Git；本文件保留关键结果与复现命令。旧 `/tmp/seex-p3*` 不作为本次依据。

## 验证与后续约束

Rust format、workspace all-target check、`cargo test`、探针两个单元测试通过；
三种布局的规模与最终小样例探针 Acceptance 断言、旧目录/无效参数拒绝验证通过；
全量 `uv run pyright` 通过。现有 unused/dead-code 和依赖 future-incompat
警告未在本次清理。项目引用的 Rust skill 目录在当前 worktree 缺失。
没有生产热路径变更，因此不运行性能 A-B-B-A；上面的单次耗时/RSS 是 Profile 诊断，
不是性能提升结论或阻断性 benchmark。

**不能直接照搬探针上线。** 一次 INSERT 排序两千万点和后续合并导致 shared
约 4.47 GiB RSS，远超 P7 的单写者 512 MiB 目标。P4 必须采用有界输入批次、
长序列有限段、有限压实工作集及原子发布/恢复；随后重新测量实际 writer RSS。
还须验证交错写入、多个长序列、超过单个目标文件的数据量与多进程恢复；
这些未测场景不能由当前单长序列样例推断通过，也不能用 Rollup 隐藏布局全扫描。
生产切换另作小变更，更新存储版本、原生边界文档及适用 ADR；在此之前 v3
仍保持原有分区。P4/P5 的 LWW、摘要和扫描边界，P7 的完整规模验收均未完成。
