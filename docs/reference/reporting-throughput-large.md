# 千万级上报吞吐测试报告

## 可审计结论

当前16K batch / 8K inline配置，在下述本机单写者负载中，正式计时观测超过1000万条：
**Rust admission中位537.93万条/s，端到端持久化中位29.27万条/s**。
Admission样本已达到25ms校准要求；持久化relative MAD为2.55%，超过2%规则，
所以不宣称稳定性PASS、无回退或跨机器普遍吞吐。本次是基线测试，不是A-B比较。

## 环境与身份

- Apple M4 Pro，Mac16,8，48GiB RAM，arm64；输出记录macOS27.0.1。
- Rust/Cargo1.97.1，release；DuckDB v1.5.4，DuckLake `d318a545`。
- 本地APFS工作区，运行时可用空间约246GiB；SSD型号/设备带宽未单独测量。
- 源码HEAD `44129320bb46a538333e4150de9b305846fc073d` 加仅诊断测试补丁。
- 测试源码补丁SHA-256 `13264e84c5e5e87570dfb929bb003b1a2ca22249772a55638ab6db6154ed8530`。
- release二进制SHA-256 `54bb00380442ee777fa423f6d0df033006d40ab199e024d20e0115cc237f676a`。

## 工作负载与单位

一个fresh store、一个Run、一个metric key `loss`。Step从0递增，value固定1.0，
Native v5纯追加。worker阈值16384、inline8192、默认队列65536，均未调整。
每段32768条使用真实公共Rust `RunHandle.log_with` 入队，然后等pending清零。
固定值/短key易压缩，不代表随机值、多key、Python、四writer或无限连续生产。

从每样本1048576条开始校准，可倍增到4194304；两项计时均≥25ms才采样。
本次首次pilot即达标。Pilot不进入十个正式样本，但其数据留在同一Run：
校准1048576条 + 正式10485760条 = **11534336条实际持久化观测**。
样本间不重建store，因此结果包含Run从百万条增长到千万条的状态。

Admission为各32768条突发的纯log循环耗时之和，不含drain等待；不是无背压稳态。
Persistence从该样本首次入队计到最后一段pending清零，包含admission和中间drain。
Finish单独测，不包含在上述持久化吞吐中。Setup、编译及独立验证不计入样本。
每个正式样本admission185.4–202.4ms，persistence3462.4–3774.7ms。

## 原始结果摘要

| 指标 | 中位 | 范围 | Relative MAD | 总观测数/总计时 |
| --- | ---: | ---: | ---: | ---: |
| Admission，条/s | 5,379,332 | 5,181,358–5,655,081 | 0.97% | 5,411,776 |
| Persistence，条/s | 292,682 | 277,789–302,846 | 2.55% | 291,071 |

十样本总admission时间1.938s；总persistence时间36.025s。最后finish314.961ms。
带setup、pilot、finish和shutdown的外部wall为40.626s，未超过120s；内部记录39.803s。
未运行performance gate，不将一次基线的样本统计冒充前后优化分类。

## 事实与资源核对

- 独立进程、新只读连接读取DuckLake：count=11534336，sum(value)=11534336。
- min/max Step=0/11534335；终态摘要count=11534336；pending=0、queue-full=0。
- 这是count/sum/范围核对，不是全字段逐行哈希证明或断电durability测试。
- `/usr/bin/time -l`报告writer测试进程峰值124747776字节（118.97MiB）。
- 704个Parquet文件，合计105108146字节（100.24MiB）；文件中位149297字节。
- catalog659456字节；未采集本轮WAL峰值或设备写入流量。
- 每段边界观察到pending最多32768，不等同于连续采样的真实峰值；没有queue-full。
- 约146KiB/文件仍是小文件状态；本测试不证明最终文件布局/增长预算已达标。

## 复现与交付

先构建release测试，再将Cargo artifact中的executable传给 `/usr/bin/time -l`。
设置 `SEEX_LARGE_ROOT` 为不存在的新目录，运行 `reporting_large_baseline --ignored
--exact --nocapture --test-threads=1`，并用外部120s timeout限制整个测试进程。
之后设置 `SEEX_LARGE_EXPECTED=11534336`，运行
`cargo test -p seex --release --lib storage::bootstrap::tests::verify_large_reporting_store
-- --ignored --exact --nocapture`。不同机器若校准倍增，应从输出读取expected，而非固定它。
原始样本与pilot：`.tmp/v5-large-reporting.log`；核对：`.tmp/v5-large-verification.log`；
汇总与身份：`.tmp/v5-large-summary.json`；保留store：`.tmp/v5-large-reporting-store`。

测量契约测试、完整 `cargo test -p seex`、核心check、Rust格式和diff检查通过。
生产逻辑/参数不变；没有修改DuckLake、关闭inline或增加队列来提高数字。
最可信的承诺仅是：本机、此有界单key增长Run负载，约540万条/s入队、29万条/s持久化。
发布级稳态吞吐、长key/多key、四writer公平性、设备写放大和断电恢复仍未验证。
