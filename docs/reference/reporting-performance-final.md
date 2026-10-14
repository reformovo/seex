# Seex 上报性能阶段收尾报告

## 结论与配置

本阶段收尾，不继续扩大参数矩阵。当前本地配置：worker 收集阈值 **16,384**、
DuckLake inline limit **8,192**、默认队列 **65,536**、收集等待上限 **10 ms**。
公共 `log` 的单 mapping 上限仍为 **8,192**；不要与 worker 阈值混淆。
整包 mapping 不拆分，收集阈值不是严格点数或字节上限。

保留 Native v5 纯追加：重复 Step 全部保留，摘要统计所有观测。
DuckLake 仍拥有事实、快照与文件清单；SQLite WAL 保存 catalog。
没有修改外部依赖源码、引入连接缓存、Rollup 或生产压实/布局切换。

分析与收尾源码基线为 `0a7d6dfa3ffd389bdab5abc034621c2b980f927e`，本报告所在
提交额外包含可选 Reader 阶段诊断。历史样本按其各自版本解释，而非全部来自当前 HEAD。
完整过程、原始结果路径见 [证据记录](native-v5-performance-baseline.md)。

## 上报速度：必须分开看

| 边界 | 可报告的结果 | 限制 |
| --- | --- | --- |
| Rust admission | 已观察约 5–6M reports/s | 多数计时不足 25 ms；不是合格性能基线或持久化速度 |
| 单写者 admission→持久化 | 16K 候选约 291–293k reports/s | 每样本新 store、单 Run/key、32768 条；不是长期稳态 |
| 同负载 finish | 16K 中位 216–218 ms | 与 drain 分开；不是持续写入吞吐 |
| 低速单 key | 10 次 enqueue→persisted 中位约 55 ms | 单次 release 诊断，无前后性能分类 |
| 低速 32 keys | inline8K/1K 中位约 79.6/71.0 ms | 顺序诊断，不能宣称 1K 更快 |

`1000 / drain时间` 仅是已入队后的排空速率估计；不能当 steady-state throughput。
`1000 / finish时间` 不应称为持久化吞吐。Rust release 与 Python development SDK
也不能直接比较。尚未建立 Python release admission 或长时间过载吞吐基线。

## 8K→16K 的证据与收益来源

相同队列、每样本全新 store、32768 条观测，A-B-B-A 原始记录：

| 中位数 | 8K A1 | 16K B1 | 16K B2 | 8K A2 |
| --- | ---: | ---: | ---: | ---: |
| admission→持久化，reports/s | 19663.8 | 292691.5 | 291321.7 | 19268.6 |
| 同阶段经过时间，ms | 1666.4 | 112.0 | 112.5 | 1700.6 |
| finish，ms | 275.8 | 216.4 | 218.1 | 279.0 |

Gate 返回 **PASS**：persistence 主指标，finish 保护指标。吞吐约 **15×**，
固定工作量的持久化经过时间约减少 **93.3%**；finish 约改善 **21.7%**。
这不是所有上报阶段都快 15×。Admission 未受保护，RSS/并发也未纳入该 gate。

校准限制：此探针固定32768条，仅检查所有选中样本≥25ms，没有内部自适应校准。
所以软件 gate PASS 有效，但项目完整测量契约尚未证明满足；保留为局部候选证据，
不将其当作无条件发布资格或 P7 达标。完整 gate 由120秒共同 deadline约束；
没有保留精确整体 wall time。历史 provenance 是 `9cc1cba` 加工作区候选补丁。

路径矩阵已经核实：batch8192/inline8192 将事实写入 inline；batch16384/inline8192
直接产生 Parquet；把 inline 提高到16384又进入 inline 并恢复昂贵 append。
因此大收益主要有 **写入路径切换** 证据，不只是 attach 次数减半。
具体 SQLite 序列化/事务/fsync 成本未逐项剖析，不能伪称已确定全部内部根因。

## 为什么不接受 inline1024

Inline 是已持久化、可查询的 catalog 数据，不是内存排队。它避免小提交产生小文件。
降低阈值减少 catalog 压力，但 paced 四 writer、多 key 诊断呈现明确反例：

| 指标 | Inline8192 | Inline1024 |
| --- | ---: | ---: |
| Parquet 文件 | 192 | 2048 |
| Parquet 字节 | 2,571,724 | 4,504,106 |
| 最终 catalog 字节 | 12,894,208 | 1,257,472 |
| 每子进程峰值 RSS | 96–164 MiB | 113–116 MiB |
| finish 范围 | 0.247–5.730 s | 0.440–2.405 s |

这是一次各四进程诊断，非正式速度/尾分位结论。1K 文件数增加约10.7倍；
不以更低锁等待单独决定默认值。并发收益仍未分类，不能外推单写者15×。
历史 burst 四 writer 的 finish 数字已纠正为0.370–1.268秒，旧1.27–3.70秒是单位错误。

## 收尾补测：WAL 占用和 Reader 阶段

相同 paced 四 writer（每进程32768观测），每5ms请求一次文件长度采样，
inline8192/1024 的 **观测 WAL 峰值** 为12,644,312 / 1,104,192字节
（12.06 / 1.05 MiB）；catalog峰值12,476,416 / 1,257,472字节。
结束时两者WAL均为0，不代表写入期间无WAL。这些是采样占用下界估计；
不测累计WAL流量、写入放大、checkpoint写回或设备写入字节。
原始序列：`.tmp/v5-final-wal-samples.json`，子进程均成功退出。

完整 Reader 查询的可选阶段诊断（相同保留 store）：

| 阶段 | Inline8K | Inline1K |
| --- | ---: | ---: |
| 热请求连接打开中位 | 28.44 ms | 28.34 ms |
| 热请求 metadata 中位 | 0.887 ms | 0.881 ms |
| 首次序列诊断 | 16.75 ms | 32.00 ms |
| 热请求序列诊断 | 缓存命中，约0.0005 ms | 缓存命中，约0.0006 ms |
| 热请求点查询中位 | 17.97 ms | 30.27 ms |

阶段有残余工作和日志开销，不能把独立中位数相加冒充请求中位数。
所选 `writer-0/key-0` 是1/16文件、1/16个 `ReadRowGroup` 日志事件；旧4/64
文件数误合计了四个Run。日志事件不等于磁盘I/O或解码行数。
Reader正式 gate仍为 **INCONCLUSIVE**（1K relative MAD超过2%），不能改称正式回退。
但文件/RG与阶段诊断一致支持“写入成本转移到读取端”，因此保留8K inline。

## 验证与交付边界

本轮通过 `cargo test -p seex`：库167通过/6忽略，client22、facade1、
多进程6、测量契约2、doc-test5通过；Rust核心check、格式、diff检查通过。
`maturin develop --uv`、`maturin build --out dist`通过；Python111测试通过，
全量Ruff格式/lint和Pyright通过。既有6项dead-code warning未清理。

精确 workspace `cargo check` / `cargo test` 都受缺失Apple MetalToolchain阻挡；
Viewer端到端、真实网络LTTB验收和P7未通过本轮验证。没有放宽配置或安装系统工具。

暂不继续调参。剩余工作按独立任务处理：合规自适应性能契约、四writer稳态/公平性/
queue-full、长key和长Run内存/文件增长、WAL流量与设备层写放大。
在这些证据补齐前，不将有限观测吞吐作为产品持续吞吐承诺，也不继续调到32K。
