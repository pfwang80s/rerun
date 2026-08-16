# Chrome Web Viewer 远程 MCAP 流式播放与 seek 实施计划

本文把 [`_mcap_stream_seek.md`](_mcap_stream_seek.md) 中与 Chrome Web Viewer 远程 MCAP 流式播放直接相关的设计拆分为可追踪、可独立审查、可单独回退的 Git 提交。
本文是项目进度的唯一任务清单；`_mcap_stream_seek.md` 是设计语义真源，两份文档共同冻结remote-MCAP-only产品范围，不再保留相互冲突的whole-Web实现要求。

## 0.1 Web-only 产品范围

2026-08-05 产品将本项目限定为“在 Chrome Web Browser 中流式播放和 seek 远程 MCAP”。
本项目不为 native Viewer 实现远程 MCAP 网络播放，不改变 native Viewer 的运行时状态机、UI 行为、Store 查询语义或数据源路由。
共享 crate 仅允许增加 Web 目标所需的 additive、capability-gated 或 `cfg(target_arch = "wasm32")` 路径；不可避免的纯提取必须由 native differential tests 证明既有输入输出和 side effects 不变，不得迁移 native callsite 到新产品状态机。
既有 compatibility `open/start` 中的 RRD、legacy HTTP、gRPC/message proxy 和 Redap 继续走当前路径；本项目只在识别为远程 MCAP 时进入新路径，不重写、暂停、重连或终止其他 transport。
新 strict API 只支持 HTTP(S) MCAP，并允许对无扩展名 URL 执行 bounded sniff；sniff 确认为非 MCAP 时返回结构化 `UnsupportedFormat`，不转交 legacy importer。
compatibility `open/start` 不提供无扩展名远程 MCAP 流式能力；这类 URL 始终进入现有 dispatcher，不执行新 Range probe，不改变既有 HTTP 请求、CORS、认证、错误或服务端 side effect。
页面 visibility、pagehide 和 freeze 只管理本项目新增的 remote-MCAP owners/work，不暂停全局 Viewer frame driver、不拦截全部 JS API，也不改变 LogChannel、gRPC 或 Redap 的现有行为。
因此 legacy adapter、Web LogChannel 重构、全局 compatibility ingress/reducer、gRPC/Redap hidden policy 和 detached Redap publication 不在本计划内。

## 1. 使用方法

每个 `MCAP-XXX` 项恰好对应一个 Git commit。
提交必须同时包含该项所需的实现、单元测试、错误处理、资源记账和必要的局部文档，不能把“实现”和“补测试”拆成两个不可独立验收的提交。
除专门冻结既有行为的测试提交外，每个提交在合入时都必须可编译、可运行相关测试，并且默认关闭尚未接通的公开能力。
若一项过大，必须先在本文中拆项并更新依赖，再开始编码；若一项过小，应与相邻项合并，避免只移动类型而没有可验证行为的提交。
任务编号表示建议的集成顺序，不表示所有任务都必须串行；依赖满足后可以在不同分支并行开发。
当前显式编号例外是产品于2026-08-13批准的`MCAP-023/025 → MCAP-025A → MCAP-032 → MCAP-031 → MCAP-033`，因为025A必须先闭合ambiguous-zero evidence与physical authority的唯一ownership，032再签发031消费的stable partition/root identity authority。
跨 crate 协议优先采用 Web-only additive path，既有 native、非 Web 和非 MCAP compatibility path 在本项目结束时仍保持原入口、原所有权和原行为。

状态约定如下：

- `[ ]`：尚未开始。
- `[-]`：正在进行。
- `[x]`：已完成并通过该项验收。
- `[!]`：阻塞，必须在“备注”中记录原因和解除条件。
- `[~]`：因 Web-only 范围决策移出本项目，不实施、不产生提交，仅保留编号和原始意图供追溯。

每项开始和完成时更新以下追踪信息：

- `负责人`：实际负责人或结对成员。
- `提交`：完成后填写 commit SHA。
- `备注`：记录偏离计划、已接受 tradeoff、后续迁移或阻塞原因。

## 2. 提交与验收规则

所有 Rust 改动完成后运行 `pixi run rs-fmt`，并对受影响 crate 运行 `cargo clippy -p <crate> --all-features` 和 `cargo nextest run --all-features --no-fail-fast -p <crate>`。
所有 TypeScript/Wasm 边界改动至少运行对应 JS 测试、`pixi run rerun-build-web` 和受影响的真实 Chrome contract test。
所有 Markdown 改动遵守一行一句，并运行 `pixi run lint-rerun <file>`；若本地缺少 `pixi`，必须在提交备注中记录未运行项并由 CI 补验。
性能或容量相关提交必须使用固定 fixture 记录阈值、输入上限和失败位置，不能只记录平均耗时。
安全边界提交必须证明失败发生在 Fetch、Wasm 字符串物化、interner、Store mutation 或 public effect 之前的正确边界。
异步所有权提交必须覆盖 success、failure、close、stop、stale callback 和 token/generation reuse。
compatibility 行为除设计明确接受的变更外必须由差分测试冻结；accepted regression 必须同时更新 TypeDoc、changelog、migration note 和宿主 contract test。
任何日志、事件、指标、错误和测试快照都不得包含原始 URL、query、ETag、Topic 全名、EntityPath 或内部 token。

## 3. 里程碑与完成定义

每次合入任务后同步更新本表的“进度”和“状态”，里程碑负责人及目标日期记录在对应首项的备注中。

| 里程碑 | 任务范围 | 进度 | 状态 | 完成定义 |
| --- | --- | --- | --- | --- |
| M0 | MCAP-001…005 | 5/5 | 已完成 | 既有公开行为、fixture、Chrome origin、确定性调度和脱敏断言可复用 |
| M1 | MCAP-006…012 | 7/7 | 已完成 | URL、validator、时间、wire、Store generation 和 remote runtime interner 边界冻结 |
| M2 | MCAP-013…033（含 MCAP-025A、MCAP-030A、MCAP-030B） | 24/24 | 已完成 | 不接 TimeControl 即可安全完成 metadata opening、bounded decoder initialization、局部 Chunk 验证和 terminal batch 派生 |
| M3 | MCAP-034…042 | 5/6 | 进行中 | Web remote-MCAP partition、root、coverage、reclaim 和 existing-identifier 能力可用 |
| M4 | MCAP-043…057 | 7/15 | 进行中 | compatibility 和 strict lane 的身份、状态、回放、dispose 与 teardown 闭合 |
| M5 | MCAP-058…066 | 0/0 | 已移出 | legacy HTTP adapter 保留现有 Web 路径，不进入 remote-MCAP 项目 |
| M6 | MCAP-067…080 | 0/5 | 未开始 | remote-MCAP page execution、hidden suspension、安全字符串和 teardown 闭合 |
| M7 | MCAP-081…087 | 0/2 | 未开始 | remote-MCAP identifier census 与 module-lifetime intern admission 闭合 |
| GA | MCAP-088 | 0/1 | 未开始 | release-Wasm测量满足后封印canonical V1 profile、安装唯一private production capability，并由第21.1节自动化门证明 |
| M8 | MCAP-089…105 | 0/17 | 未开始 | foreground-only window playback、seek、query isolation、mutation arbitration 和 reload 闭合 |
| M9 | MCAP-106…114 | 0/9 | 未开始 | two-phase runner、strict startup、UI/API 文档和桌面 Chrome E2E 全部通过 |

有效工作项总数为 91，当前进度为 48/91；26 个 `[~]` 项不计入分母且不产生提交。
关键路径为 `M0 → M1 → M2/M3 → M4/M6/M7 → GA → M8 → M9`。
M2、M3、M4 的不相交部分可以并行，但任何 public route 在 GA 前保持 feature-disabled。

## 4. M0 — 基线与测试设施

### [x] MCAP-001 — 冻结现有 Web Viewer 公开行为

- 建议提交：`Freeze existing Web Viewer open and control contracts`。
- 依赖：无。
- 变更：为 `open/start(string|string[])`、hidden startup URL、gRPC/Redap routes、LogChannel、raw recording-ID control、`recording_open` 和同步 raw-event 行为增加 characterization tests，不修改生产语义。
- 验收：覆盖 malformed URL、数组第二项失败、反序 recording completion、同 source 多 Store、panel close、Viewer stop 和 stopped-wrapper 错误路径，并明确哪些行为后续保持兼容、哪些是设计已接受的变更。
- 负责人：kola；提交：`675412d7ba8d63ab576ad003d56e25f5610ff3af`；备注：Dafee 三轮 review 后无中级及以上问题，Node 11/11、Viewer contracts 6/6、startup 2/2及定向 clippy/format通过；完整native/wasm门限受本机缺少libudev/clang和Git LFS资产限制。

### [x] MCAP-002 — 建立可组合的 MCAP 对抗 fixture builder

- 建议提交：`Add adversarial MCAP fixture builder`。
- 依赖：MCAP-001。
- 变更：实现设计第 19.1 节要求的 Chunk、Summary、CRC、Schema/Channel、MessageIndex、raw time、nested collection、compression、decoder overlap、partition root 和 physical overlap 控制项。
- 验收：每类开关至少有一个自校验 fixture，fixture 可以组合生成合法边界值和只违反单一约束的非法文件，并能与 upstream Summary/indexed reader 做差分。
- 负责人：kola；提交：`3f4b6c80d86a139dca0dbcd16ba0a6a123410b72`；备注：Dafee 四轮 review 后无中级及以上问题，adversarial fixture 18/18及check/clippy/rustfmt/diff通过；全crate 74/75，唯一失败为未拉取的129-byte Git LFS attachments.mcap pointer。

### [x] MCAP-003 — 建立受控 Chrome Range/CORS 测试源

- 建议提交：`Add controlled Chrome Range test origin`。
- 依赖：MCAP-001。
- 变更：增加 same-origin/cross-origin object origin、service worker、CSP、redirect、BYOB 分片、无 Content-Length、无限 body 和 ETag 变体的可编程测试服务器与页面。
- 验收：Chrome stable 中可精确观察 request headers、response headers、application-visible bytes、reader cancel/abort、heartbeat 和是否调用 `arrayBuffer()`，日志自动脱敏。
- 负责人：kola；提交：`8d1a0cbee720f57fdd3a9e6a61709eb1039290a5`；备注：Dafee 三轮 review 后无中级及以上问题，native runner 9/9、native/Wasm check、clippy、format、JS syntax及diff通过；真实Chrome stable验收已接入强制CI lane，本机因缺Chrome/driver未执行。

### [x] MCAP-004 — 建立确定性异步所有权与帧调度 harness

- 建议提交：`Add deterministic Web ownership scheduler harness`。
- 依赖：MCAP-001。
- 变更：提供可控制 Fetch completion、browser task、RAF、frame allowance、visibility、stop、token reuse、JS ack 和 Store safe point 的 local-future harness。
- 验收：测试可以停在 prepare、disarmed commit、release、activation、body backpressure、apply ack、CommitLocked、GC 和 teardown 后迟到 callback 的每个边界，并能断言 owner/permit/queue 高水位。
- 负责人：kola；提交：`eda02fa76023aaec0b0dea886adb4f5fb84fd576`；备注：Dafee 三轮 review 后无中级及以上问题且无设计偏离，`re_web_tests` native lib 26/26 与 bin 3/3、native clippy、Wasm Chrome fixture check、Rust format、JS syntax、cargo metadata 及 diff check 通过；真实 Chrome 已接入强制 Chrome stable CI lane，本机因缺 Chrome/driver/wasm-pack 未执行。

### [x] MCAP-005 — 建立统一资源与脱敏断言工具

- 建议提交：`Add bounded-resource and redaction test assertions`。
- 依赖：MCAP-001。
- 变更：增加 checked count/byte reservation、逐位 rollback、registry snapshot、zeroize、无 public effect、无 interner delta 和 bounded label 的测试辅助工具。
- 验收：辅助工具能用于 Rust、Wasm 和 TypeScript 测试，并对 URL、query、ETag、Topic、EntityPath、StoreId 和内部 token 的泄漏给出确定失败。
- 负责人：kola；提交：`d18d888b3e2c552492210c2d17e4de07a43126b2`；备注：Dafee 两轮 review 后无中级及以上问题且无设计偏离，native all-features/all-targets 42/42、Node/TypeScript 21/21、native all-target clippy、Wasm check/clippy、JS syntax、package format、metadata 及 diff check 通过；本机因缺 Chrome 未执行真实 Chrome。

## 5. M1 — 安全身份与 Web remote-MCAP 资源基础

### [x] MCAP-006 — 冻结 Web 远程能力的版本化 limits profile

- 建议提交：`Define versioned Web remote resource limits`。
- 依赖：MCAP-005。
- 变更：集中定义 remote-MCAP Summary、nested collection、Chunk、decoder、registration、remote open/lifecycle、Range/cache、presentation、GC 和 runtime intern budget 的 count/byte/time 上限及 checked accounting API。
- 范围：删除 legacy、LogChannel、Redap、全局 compatibility ingress 和其他 transport 的 key、scope、formula、accountant 与验收；profile 只能由 Web remote-MCAP capability 消费，native target 不可构造生产 profile。
- 验收：所有上限都冻结单位、作用域、数值来源和 checked arithmetic 测试；设计已给定或可静态推导的字段提供版本化默认值，必须等待 release-Wasm Chrome 或对抗 harness 测量的字段显式保持生产不可构造的 `Unfrozen`，不得猜测占位数值。
- 验收：`Unfrozen` 字段记录唯一测量阶段与冻结证据类型，后续工作项只能以带证据的 profile transition 安装数值；在 profile 完整冻结前，任何生产路径都不能取得或使用该 profile。
- 验收：reservation rollback 保持原始计数逐位不变，profile 在 Web remote-MCAP feature 接入前不改变任何现有运行路径；native 和非 MCAP Web 路径不能取得 accountant root。
- 决策记录：2026-08-05 产品批准先提交 `Unfrozen` schema/accounting，待设计指定的 release-Wasm Chrome 与对抗测量完成后再冻结 V1 数值；该决策不授权为测量型字段填入临时默认值。
- 负责人：kola；提交：`400e17d4c41c71ee8c466eb10d0b09b33cd8e36a`；备注：Web-only范围与依赖经Dafee复核，四轮实现review后无中级及以上问题；最终冻结261-key exact allowlist、70 requirements、19 typed formulas、sealed Wasm capability、presentation/parked owner与accounting scratch；stable Rust 45个unit、native API compile-fail、Wasm 15个适用compile-fail、native/Wasm check与clippy、rustdoc、format及diff check通过；本机缺pixi、cargo-nextest和仓库指定1.95.0，未冒充完整项目门限。

### [x] MCAP-007 — 实现 secret-safe URL 与 route canonicalization

- 建议提交：`Add secret-safe Web URL normalization`。
- 依赖：MCAP-005、MCAP-006。
- 变更：在 Web-only 模块增加 `SecretUrl`、route-canonical bytes、query-bearing nested URL 拒绝和不经过 domain interner 的 bounded raw fragment parser，不替换 `re_uri` 的 native/compatibility parser。
- 验收：scheme/host case、default port与path中RFC 3986等价percent encoding得到相同canonical bytes；path只解码unreserved、保留encoded reserved并在解码后消除dot segments，query的原始bytes、顺序、重复、percent大小写和显式空query全部保留，fragment不参与route identity。
- 验收：raw URL不进入Debug、Display、日志、history、错误或持久化state；canonical bytes不能替代实际Fetch URL；raw fragment只做literal `&`/首个`=`的field/count/byte/percent-triplet preflight并保持opaque，不进入domain interner；native和非MCAP Web parser没有production diff。
- 负责人：kola；提交：`4213e72c284df4885a1d38e56b2fc2a2481122d8`；备注：仅实现Wasm remote-MCAP additive parser/canonicalization，不替换native或compatibility parser；Dafee确认path/query/fragment规范属于安全实现澄清，五轮review后无中级及以上问题；stable Rust native 64/64、native/Wasm doctest、check与clippy、Wasm docs、rustfmt及diff check通过；本机缺pixi、cargo-nextest、cargo-shear并使用stable 1.96.0。

### [x] MCAP-008 — 实现 typed ETag 与 representation consistency 真值表

- 建议提交：`Add typed remote object validator parsing`。
- 依赖：MCAP-006。
- 变更：实现strong/weak/absent/invalid ETag parser、`RepresentationConsistencyPolicy`、actual consistency和 `If-Match` 生成规则；内部保存RFC 9110 opaque wire bytes，不使用UTF-8 `str`近似表示合法 `obs-text`。
- 变更：增加Chrome Header ByteString isomorphic ingress/egress codec，把JS code unit `0x00..0xFF`与wire byte一一映射；`If-Match`只经该adapter重发，不走普通Rust `str`，并对JS/Wasm双向copy、临时overlap和retained owner执行checked credit。
- 验收：覆盖大小写敏感opaque tag、quoted comma、weak前缀、重复/list header、malformed quote、strong/weak/absent转换和默认 `RequireStrongValidator`，weak tag永不生成 `If-Match`；`0x80..0xFF` obs-text在unit/Wasm与真实Chrome中byte-for-byte roundtrip，超出ByteString的code unit在copy前拒绝且不产生partial owner。
- 决策记录：2026-08-05产品选择完整支持RFC 9110 obs-text，不接受ASCII-only部署限制；后续设计中任何 `Arc<str>`/`&str` ETag示意均由本项的opaque wire-byte与ByteString adapter契约覆盖。
- 负责人：kola；提交：`c14f6f793a48d2948d8984917ee71664008c7df3`；备注：产品选择完整RFC 9110 obs-text，内部使用opaque wire bytes与Chrome ByteString isomorphic codec；Dafee多轮深审后无中级及以上问题；stable Rust native 74/74、Wasm tests编译链接、native/Wasm clippy与doctest、Wasm docs、rustfmt及diff check通过；3个run-in-browser测试因本机无Chrome/runner仅完成Wasm编译，留待Chrome CI执行。

### [x] MCAP-009 — 实现唯一 raw MCAP 时间转换入口

- 建议提交：`Add checked remote MCAP time canonicalization`。
- 依赖：MCAP-006。
- 变更：增加 `RawMcapTime` 到 `TimeInt` 的 checked helper，冻结 `TimestampNs/DurationNs`，固定零 offset，并排除 `Sequence`、`i64::MIN` sentinel 和 `u64 > i64::MAX`。
- 验收：ChunkIndex、MessageIndex 和实际 Message 共用同一 helper，`i64::MAX` 保持原值，publish/log timeline 类型一致，boot-relative fixture 不触发 epoch 启发式或饱和转换。
- 负责人：kola；提交：`3914ef46679d452c8b111ca1a5b6ff6f3cf87ac8`；备注：只增加Wasm production可见、host-test-private的remote-MCAP checked time canonicalization helper与测试，未迁移native/local MCAP或Viewer时间路径；Dafee深审后无中级及以上问题；专项6/6、其余可运行lib测试80/80、check、clippy、native doctest、rustfmt及diff check通过；完整套件唯一失败为本机缺Git LFS导致attachments fixture仍是pointer，Wasm全crate受本机缺clang阻断于既有lz4/zstd构建脚本，本机亦无pixi/nextest。

### [x] MCAP-010 — 实现 StrictOpenWireV1 基础 codec

- 建议提交：`Add StrictOpenWireV1 identity and error codec`。
- 依赖：MCAP-006。
- 变更：实现 branded opaque identity、canonical decimal `u64`、V1 success/error/ack envelope 和唯一五类 error mapping，不接入公开 API。
- 验收：错误 brand/instance、未知 version、前导零、负数、溢出、非 ASCII 数字、JavaScript number 和未知 error code 均落入规定类别，只有 `FatalAbi` 决定 Viewer teardown。
- 负责人：kola；提交：`63af42bf30b7ee7e4f7a4426a02c8ea89df62383`；备注：按Web-only范围实现未接入公开API的V1基础codec，采用instance/domain隔离opaque identity、canonical decimal u64、sealed shape parser输入、独立batch/attachment/ack caps、固定脱敏message与唯一五类error policy；Dafee三轮深审共闭合4个High与4个Medium后无剩余中级及以上问题；专项20/20、re_web lib94/94、native/Wasm clippy、native/Wasm doctest、Wasm test编译链接/docs、rustfmt及diff check通过；本机无pixi/nextest，使用stable cargo定向门限。

### [x] MCAP-011 — 引入 StorePublicationIdentity 与单调 StoreGeneration

- 建议提交：`Add generation-aware Store publication identity`。
- 依赖：MCAP-006。
- 变更：增加 Web remote-MCAP 内部 `(StoreId, StoreGeneration)` 身份和跨 Viewer restart 不复用的 Wasm-module-lifetime generation allocator，不改变现有 Store lookup 或 native Store identity。
- 验收：allocator 溢出安全失败，旧 generation 的 token/result 不能解析到新 remote Store，native 与非 remote-MCAP 调用方没有 production diff。
- 负责人：kola；提交：`50b53944239063c7d1b1855b54be366b9d85a0e8`；备注：按Web-only范围增加未接入StoreHub/native lookup的generation-aware publication/fixed-target identity与Wasm-module-lifetime checked allocator，`u64::MAX`恰好分配一次后永久exhausted，native normal依赖树不新增re_log_types；Dafee两轮深审闭合2个Medium后无剩余中级及以上问题；专项8/8、re_web lib102/102、native/Wasm clippy、Wasm test链接、native doctest5/5、Wasm doctest24/24与6项ignored、Wasm docs、rustfmt及diff check通过；本机无pixi/nextest/Chrome runner。

### [x] MCAP-012 — 建立 remote module-lifetime runtime intern budget

- 建议提交：`Add bounded remote MCAP runtime interning capability`。
- 依赖：MCAP-006。
- 变更：在 `re_string_interner` 增加 additive `bounded_runtime_intern` capability、Wasm-module-singleton remote budget、legacy-map/remote-side-map共享协调revision、borrowed missing-set lookup、revision-checked atomic batch primitive和remote string/entry/side-map-capacity永久burn记账。
- 范围：本项只实现底层remote side-map、budget、封闭bounded census输入协议和test primitive，不接Summary/Chunk decoder调用点；existing `InternedString/newtype/serde` constructors保持原API与行为。
- 验收：remote新增只进入bounded side-map且普通existing lookup可见；legacy实际新增只推进协调revision、不消耗remote budget；remote hit legacy/side-map不重复burn；side-map growth只复制bounded remote entries或一次性固定预分配，绝不复制、计费或受unbounded legacy map容量拖累。
- 验收：所有fallible census/budget/revision/allocation与candidate-peak检查先于leak/insert/burn，失败保持legacy map、side-map、budget、revision逐位不变；Viewer stop/restart后burned bytes不退款。
- 验收：feature-off native、legacy、LogChannel、gRPC、Redap和其他Wasm产品不进入新path，旧constructor compile/API/serde差分通过；API test只约束当前production dependency/callsite，不声称Rust识别caller crate身份。
- 负责人：kola；提交：`8f02ec107a082b016634500d5e2930498fd3ea24`；备注：按remote-MCAP-only范围增加additive fixed-preallocated side-map、module-lifetime不可退款budget、共享legacy协调revision、bounded census与zero-partial batch primitive，未接decoder或迁移native/nonremote callsite；Dafee三轮深审闭合2个High与3个Medium后无剩余中级及以上问题且无设计缺陷；feature-on 30/30、feature-off 9/9、re_web 104/104、native/Wasm clippy、Wasm link/docs、doctest、diff/callsite/dependency-tree检查通过；本机无pixi、cargo-nextest、Chrome runner和仓库指定Rust 1.95，未冒充完整项目门限。

## 6. M2 — Chrome Range、受限 MCAP parser 与局部 decode

### [x] MCAP-013 — 实现 exact-length BYOB body pump

- 建议提交：`Add bounded exact-length Chrome BYOB pump`。
- 依赖：MCAP-003、MCAP-006。
- 变更：在 `cfg(target_arch = "wasm32")` remote-MCAP transport 模块增加专用 `206` body pump，使用固定 scratch、每次 read 后重建 view、填满后额外读取一字节验证 EOF，并在 slice 间 yield browser task。
- 验收：exact、early EOF、overlong、infinite、zero-progress、timeout、missing body、无 BYOB reader 和 detached-view fixture 均在 reservation 内收敛，普通路径不调用 `Response.arrayBuffer()`。
- 负责人：kola；提交：`f02ae88fffd5578c7076ec703e5df5e436ceba8f`；备注：实现Wasm-only exact-length Chrome BYOB pump、四维原子reservation、fixed-layout output owner、mandatory per-slice pre-read gate、returned-view/backing/detach校验、逐slice macrotask yield、1-byte EOF probe和typed cleanup，并接入MCAP-003 controlled-origin production-pump Chrome CI lane；Dafee三轮深审闭合2个High与3个Medium后无剩余中级及以上问题且无设计缺陷；native re_web 109/109、re_web_tests 39/39、native/Wasm all-feature all-target clippy、Wasm test链接/docs、fmt/diff通过；本机无pixi、wasm-pack、Chrome和chromedriver，真实Chrome未执行且已明确留给强制CI lane。

### [x] MCAP-014 — 实现严格 Range request/response 验证

- 建议提交：`Add strict Chrome Range response validation`。
- 依赖：MCAP-008、MCAP-013。
- 变更：实现 request policy、CORS/exposed-header 检查、`206/Content-Range/Content-Length/Content-Encoding` 验证、status mapping、redirect/CSP/opaque rejection 和 typed bound validator；本项只返回 typed status/transport validation，不决定 retryability。
- 验收：所有 Range 使用 `credentials: omit`、`cache: no-store`、`redirect: error`、`no-referrer`，普通 `200` 永不回退 full download，401/403、404/410、412、416、429、500、502、503、504、其他 4xx/5xx 和不可见 CORS failure 均保留精确 typed 分类，且 transport API 不携带宽泛的 `is_retryable()` 判定。
- 负责人：kola；提交：`fe1729f51c12a82edcb656cdb7e88afe874ca78d`；备注：实现Wasm-only严格Range Request policy、typed checked-range preflight、status/header/Content-Range/Length/Encoding/ETag probe bind与bound revalidation、policy-free transport错误分层，并保证前置失败零body read且matching abort/permit cleanup；Dafee三轮深审闭合5个Medium，期间由用户冻结exact retry allowlist后无剩余中级及以上问题且无其他设计缺陷；native re_web 115/115、re_web_tests 39/39、native/Wasm all-feature all-target clippy、Wasm test链接/docs、file-specific rustfmt、JS syntax与diff通过；本机缺仓库Rust 1.95、cargo-nextest与Chrome工具链，真实Chrome留给强制CI lane。

### [x] MCAP-015 — 实现 tokenized retry、取消与 object-change 传播

- 建议提交：`Add tokenized Range retry and object-change handling`。
- 依赖：MCAP-014。
- 范围：只实现production-disarmed、metadata-opening-oriented的retry foundation与对抗harness；不接seek、prefetch、stable-root/GC refetch或backfill的production phase owner、clock和failure policy。
- 变更：增加live source/representation下一条exact Range operation的token、current attempt、`AbortController`、validator/length revalidation、`RetryPending`和不受demand-generation stale filter吞掉的matching `SessionFatal(ObjectChanged)`通路。
- 变更：在limits schema新增phase-A `Unfrozen` 的 `RangeRetryAttemptsPerOperation`，冻结为 `Transport × Count × Operation × ScalarObservation × HighWatermark`并只通过sealed typed projection构造non-cloneable attempt owner；metadata累计Range与active-visible deadline分别复用source-scoped `MetadataOpeningRangeRequests`和 `MetadataOpeningVisibleDeadlineMillis`及其non-cloneable typed owner，数值不得猜测。
- 变更：initial Fetch为attempt 1；Request/Headers/validator/range/capacity全部零burn preflight成功后，在调用 `window.fetch`前原子且不可退款地消费operation attempt与caller phase累计Range count，settlement failure不退款。
- 变更：只允许fetch-level `BrowserFetchUnavailable`、matching active-visible timeout和 `429`/`500`/`502`/`503`/`504` 进入 `RetryPending`；BYOB `ExactLengthByobPumpError::ReadFailed`、missing body/reader、其他HTTP状态与deterministic local/validation failure全部nonretry。
- 变更：retry最早在下一eligible `VisibleRunning` remote control turn启动且每turn至多一个attempt；不增加fixed/exponential backoff、不读取 `Retry-After`；hidden只暂停abstract active-visible owner，production page epoch/fresh-baseline rebind由MCAP-067接入。
- 验收：local preflight产生composite prepared burn，失败或Drop时零Fetch且attempt/Range counters逐位不变；matching commit后Fetch rejection/timeout/allowlisted status保留两个已burn counters；上一attempt的Response/body reader/output/retained permits释放或成功转移前不能启动下一attempt。
- 验收：retained response bytes只表示可回收并发peak；总网络上界由attempt cap、metadata source Range cap和per-response requested bytes checked导出，不新增伪装成retained bytes的累计流量ledger。
- 验收：extensionless sniff与handoff后的metadata opening move同一source Range/deadline owners，不能通过route transition重置budget；known `.mcap`直接从opening首个Range消费同型owner。
- 验收：callback不能递归Fetch，hidden不burn，resume harness只对fresh epoch/baseline rebind一次，pagehide/freeze终结；`Retry-After`对trace无影响，BYOB `ReadFailed`不产生第二次Fetch。
- 验收：same live source/session/representation的旧demand generation object change仍终结metadata opening，旧source/session/attempt的迟到结果只释放owner；metadata exhaustion只终结matching opening source。
- 验收：production profile仍不可构造，native、local MCAP、legacy、LogChannel、gRPC、Redap和其他Wasm route的API、请求、错误与teardown trace逐位不变。
- 负责人：kola；提交：`ab85c40a6dac0d446ad0bae1b78255f5da3ea4df`；备注：按用户冻结的最小保守方案实现production-disarmed metadata-opening retry foundation，包括Unfrozen attempt key、one-shot typed owners、attempt与phase Range count原子burn、authoritative Range/controller/accounting ancestry、pollable pending/settled task ownership、next-visible-turn gate、active-visible timeout、exact allowlist、object-change fatal与同步close/page/deadline teardown；Dafee完成一次完整冻结基线和后续blob增量深审，全部中级及以上问题闭合且无设计缺陷；retry 35/35、re_web 154/154、re_web_tests 42/42、native/Wasm clippy、Wasm check/link、doc tests 8/8、Wasm docs、fmt与diff-check通过；本机缺wasm-pack，真实Chrome未执行并保留强制CI说明。

### [x] MCAP-016 — 实现 8-byte extensionless format sniffer

- 建议提交：`Add bounded extensionless Web format sniffer`。
- 依赖：MCAP-007、MCAP-013、MCAP-014、MCAP-015。
- 变更：为strict API增加独立token、固定8-byte application buffer、`206` probe ownership transfer和 `200` cancel/abort语义，暂不接Viewer registry；sniffer使用MCAP-015 operation并把source-scoped metadata Range/deadline owners随MCAP handoff一起move，不创建legacy owner，也不被compatibility dispatcher调用。
- 验收：`206` MCAP可把已验证的8 bytes及未重置的metadata owners转交remote-MCAP parser；`206/200` non-MCAP在取消原reader后返回 `UnsupportedFormat`；`200` MCAP返回 `RangeUnsupported`；任何路径不保留超过8 bytes；compatibility无扩展名URL的请求trace与MCAP-001逐位一致且没有额外probe。
- 负责人：kola；提交：`4c9517ce829ee4206d4df1c35dff8ceac90f5420`；备注：实现production-disarmed strict-only 8-byte format sniffer、200 fixed-capacity BYOB cancel/abort、206严格exact Range与validator验证、不可复用token/attempt epoch、同一metadata retry owner与SecretUrl/source spec线性handoff，以及compatibility extensionless/known/native零入口；Dafee完成一次冻结基线和一次blob增量深审，闭合1个High与1个Medium后0H/0M且无设计缺陷；native/Wasm re_web clippy、re_web154/154、doc8/8、re_web_tests42/42、Wasm check/link/docs、fmt与diff-check通过；本机缺wasm-pack/Chrome，真实Chrome保留强制CI门限。

### [x] MCAP-017 — 解析 Header、Footer、DataEnd 与 CRC 边界

- 建议提交：`Add bounded MCAP fixed-layout validation`。
- 依赖：MCAP-002、MCAP-009。
- 变更：实现固定 Header/Footer、`summary_start - 13` DataEnd record、Summary CRC 和 Chunk CRC 的 `NotProvided/Verified` 状态。
- 验收：opcode、body length、position、coverage range、zero CRC 和 nonzero mismatch 均有测试，CRC 验证发生在 scanner 和任何 batch publication 之前。
- 负责人：kola；提交：`12f1696a77648e35985c019d569b676125f75e39`；备注：在re_mcap增加remote-only additive fixed-layout validator，严格验证Header/Footer/DataEnd、Summary CRC精确coverage、partial data CRC状态与exact-output Chunk CRC，并以lifetime-bound sealed summary slice保证后续parser消费同一已验证bytes；Dafee完成冻结基线与blob增量深审，闭合1个High与1个Medium后0H/0M且无设计缺陷；focused13/13、native check/all-target all-feature clippy、doc3/3、tree/metadata/diff通过，完整套仅既有Git LFS pointer fixture失败；Wasm因本机缺clang阻断在既有lz4/zstd构建。

### [x] MCAP-018 — 封印 allocation-free Header/Summary 顶层 census

- 建议提交：`Add allocation-free MCAP summary census`。
- 依赖：MCAP-006、MCAP-017。
- 变更：在 `re_mcap` 中消费 MCAP-017 lifetime-bound validated Summary slice，并为 exact Header body read 增加同样 lifetime-bound 的 evidence；使用 checked arithmetic 零分配扫描 Header、Summary 和 SummaryOffset envelope，验证 length、range、exact exhaustion 与 zero progress，对包括 Unknown 在内的每条 Summary record 计数，并在任何 `Vec`、map、`String` 或 owned `Cow` 构造前检查 Summary、Schema、Channel 和 ChunkIndex caps。
- 变更：第一阶段拒绝非法 opcode `0x00`；Summary 主 section 只接受规范允许的 `Schema`、`Channel`、`ChunkIndex`、`AttachmentIndex`、`Statistics`、`MetadataIndex` standard records与future-reserved `0x10..=0x7F`/private `0x80..=0xFF` Unknown records，并拒绝 `Header`、`Footer`、`Message`、`Chunk`、`MessageIndex`、`Attachment`、`Metadata`、`SummaryOffset`和`DataEnd`等context-forbidden standard opcode。
- 变更：Summary 主 section 中每个精确 opcode 必须只形成一个连续 group；validator只使用固定栈上的256-bit bitset或`[bool; 256]`等价状态，不分配heap、保存per-record collection、读取Unknown body或解析/复制SummaryOffset group mapping。
- 变更：只产生 crate-private、sealed、non-`Clone` 的 `PreparedSummaryRecords<'a>`，持有本次已验证的 exact Header/Summary slices 与 scalar census；不产生 final/public Summary result，不调用 `mcap::parse_record`、`SummaryReader` 或 `into_owned`，也不保存 per-record `Vec`。
- 验收：Header length-prefix、record envelope、checked offset、truncation、zero progress、Unknown record 和各顶层 count 的边界由 zero-allocation instrumentation 覆盖；合法grouped records、同opcode连续多record、standard/future/private group任意顺序成功，standard或Unknown opcode离组后重现失败。
- 验收：opcode `0x00`、每一种context-forbidden standard record与grouped future/private Unknown都有矩阵测试，future/private至少覆盖`0x10`、`0x7F`、`0x80`和`0xFF`边界；上述合法/非法opcode和group路径的heap allocation、reserve、push、map insertion、`String`、owned `Cow`与Unknown body copy计数均为零。
- 验收：`summary_offset_start == 0` 明确合法且不允许 SummaryOffset opcode出现在Summary stream；非零时对应 section 必须只由完整 SummaryOffset records exact consume，缺失、错误 opcode、trailing bytes 和越界均在构造 `PreparedSummaryRecords` 前失败。
- 验收：pointer/lifetime 测试证明 Header 与 Summary 证据只能暴露本次 validated input，same-range 另一 buffer 不能替换；合法最大密度的 envelope、opcode 和 scalar census 与独立 fixture oracle 一致，本项不执行 materialization、upstream Summary differential 或 duplicate-definition 语义。
- 范围：本项只增加envelope、context opcode、连续group shape与scalar census验证，不执行owned-field/nested preflight、materialization、duplicate-definition/reference、ChunkIndex canonicalization或physical-layout；这些职责仍属于MCAP-019/020/021及后续validators，SummaryOffset group mapping则明确不在MVP实现。
- 范围：全部 API 保持 Web remote-MCAP-only、production-disarmed；native、local MCAP和nonremote Web parser的default Cargo feature/dependency graph、API resolution与运行 trace 不变。
- 负责人：kola；提交：`b3283ecbbaa40a89ece18958bdc4efd853266bb8`；备注：2026-08-07 完成allocation-free exact Header/Summary envelope与scalar census，以固定栈256-opcode状态拒绝0x00、context-forbidden standard records和离散group，同时保留future/private Unknown并不解析SummaryOffset mapping；Dafee首轮因规范缺口暂停，产品冻结group/opcode语义后完成增量深审，结论0 High/0 Medium且无设计偏离；默认与all-features focused各11/11、all-target all-feature clippy、native feature-off check、doc1/1与compile-fail3/3、fmt和diff-check通过，完整re_mcap为104/105且唯一失败是既有Git LFS pointer，Wasm仍因本机缺clang阻断于lz4/zstd依赖构建，本机无pixi/cargo-nextest。

### [x] MCAP-019 — 预检全部 allocation-bearing 字段并物化 bounded Summary

- 建议提交：`Preflight and materialize bounded MCAP summary records`。
- 依赖：MCAP-018。
- 变更：消费 sealed `PreparedSummaryRecords<'a>`，对完整 Header 与 Summary 输入执行第二次 allocation-free pass，预检所有 owned length-prefixed fields、`Channel.metadata`、`ChunkIndex.message_index_offsets`、`Statistics.channel_message_counts`，并提供 `MessageIndex.records` 的共用 fixed-width preflight；所有重复 records 都计入 checked aggregate count、encoded bytes 和 retained bytes，不能在 budget 前去重。
- 变更：只有 whole-input preflight 成功后才能签发 private materialization token并开始任何 reserve/push/map insertion、`String` 或 owned `Cow` 构造；known records 此后可以调用 upstream `parse_record`，Unknown 必须按 validated envelope borrowed-skip，禁止调用 `into_owned` 或复制 body。
- 变更：输出 bounded parsed representation，保留 duplicate multiplicity、source order、exact schema bytes 与完整 Channel metadata；输出必须先由 MCAP-020 消费并保留，再经 MCAP-020 的 sealed result 传递给 MCAP-021，不得由 MCAP-021 直接旁路 definition/reference closure；本项不执行同 ID conflict/reference 语义、ChunkIndex canonicalization 或 physical-layout validation。
- 验收：Channel metadata、两个 `u16 + u64` integer maps、`u64 + u64` MessageIndex vector、Header/known-record owned strings 的 entry/length/UTF-8、`% 10`/`% 16`、early EOF、exact nested exhaustion、per-record 与 aggregate retained caps 全部在第一次 materialization allocation 前失败。
- 验收：后项 nested 或 aggregate failure 时，前项 records 的 owned allocation 计数仍为零；重复 definitions 不能绕过 aggregate budget，Unknown body copy 始终为零；最大合法输入的 bounded parsed result与 upstream `SummaryReader` 在共同合法语义上差分一致。
- 范围：全部 API 保持 Web remote-MCAP-only、production-disarmed；native、local MCAP和nonremote Web parser的default Cargo feature/dependency graph、API resolution与运行 trace 不变。
- 负责人：kola；提交：`40ca10479f607450e8561e45da91861238c5bde5`；备注：2026-08-07 完成whole-input allocation-free owned-field/nested preflight、完整census绑定的真实non-Clone reservation、token-gated upstream known-record materialization与Unknown borrowed-skip；最终bounded representation保留原Prepared fixed-layout/CRC/slice evidence、source order、duplicate multiplicity、borrowed exact Schema data和完整Channel metadata，并提供同一fixed-width MessageIndex preflight而不承担MCAP-020/021语义；Dafee首轮深审发现2个High与1个Medium，补齐evidence链、Arc-backed profile budget/permit move-and-Drop ownership及全部known/owned-field差分后增量复审为0 High/0 Medium且无设计偏离；默认与all-features focused各22/22、all-target all-feature clippy、native feature-off check、doc1/1与compile-fail3/3、fmt和diff-check通过，完整re_mcap为115/116且唯一失败是既有Git LFS pointer，Wasm仍因本机缺clang阻断于lz4/zstd依赖构建，本机无pixi/cargo-nextest。2026-08-13追溯补充：025A设计审计要求此已完成inner result在集成时被包入从首次bound probe签发的same-object enclosing evidence owner，lower binding/profile/registry/budget issuer必须随019→023原样move；不重开本项算法实现，025A负责新增sealed接线和compile/API证明。

### [x] MCAP-020 — 冻结 Summary/Chunk definition 一致性规则

- 建议提交：`Validate referenced MCAP definitions consistently`。
- 依赖：MCAP-019。
- 变更：独占 Summary/Chunk definition 的 semantic canonicalization，消费 MCAP-019 保留 multiplicity 与 source order 的 bounded parsed representation，实现 schema-less Channel、同 ID exact comparison、reference-driven unknown definition 处理和未选 Channel 也必须验证的规则。
- 变更：成功输出crate-private、sealed、non-`Clone`的`ValidatedSummaryDefinitions<'a>`，按ownership保留MCAP-019的Materialized/Prepared/fixed-layout/CRC/slice evidence与materialization reservation，作为MCAP-021唯一合法输入。
- 验收：完全相同的重复 Schema/Channel 可 canonicalize但仍已计入原始 parse budget；任一同 ID name、encoding、exact data、schema/topic/message-encoding 或完整 metadata 冲突失败；`schema_id == 0` 合法，未知且未引用 definition 被忽略，未知但被 Message 引用或 Chunk 尾部冲突不能在已选 batch 发布后才发现。
- 负责人：kola；提交：`846ef06c10dc282a023c94f703404c6ae85a4012`；备注：2026-08-07 完成allocation-free Summary Schema/Channel exact canonicalization、conditional Channel→Schema closure和逐条原始ChunkIndex message-index Channel closure，同时保留MCAP-019的source order、duplicate multiplicity与non-Clone reservation；Chunk侧只提供single typed-event semantic accumulator与first-error poison，prefix只能产生明确非授权的semantic summary，source identity和exact exhaustion仍由MCAP-025组合后才能形成publication authority；Dafee首轮深审发现2个High，补齐ChunkIndex reference closure并删除可伪造的自报census/validated Chunk capability后增量复审为0 High/0 Medium且无设计偏离；默认与all-features focused各25/25、all-target all-feature clippy、native feature-off check、doc1/1与compile-fail3/3、rustfmt和diff-check通过，完整re_mcap为118/119且唯一失败是既有129-byte Git LFS pointer，Wasm全特性检查仍因本机缺clang阻断于既有lz4/zstd构建脚本，本机无pixi/cargo-nextest。2026-08-13追溯补充：020 transition在025A集成中只能替换same-object enclosing owner的inner typestate，不能接受或重签lower binding/profile/registry/budget authority；算法与既有提交保持不变。

### [x] MCAP-021 — 验证 Chunk 与 MessageIndex physical regions

- 建议提交：`Validate MCAP physical region ownership`。
- 依赖：MCAP-019、MCAP-020。
- 变更：以move-only方式消费MCAP-020的sealed `ValidatedSummaryDefinitions<'a>`，canonicalize duplicate ChunkIndex，checked-compute Chunk/owning MessageIndex region，验证data-section bounds、声明邻接、唯一offset、全局不重叠和cross-Chunk alias。
- 变更：输出sealed、non-`Clone`的`ValidatedPhysicalRegions<'a>`，按ownership保留`ValidatedSummaryDefinitions<'a>`、其Materialized/Prepared/fixed-layout/CRC/slice evidence、materialization reservation、canonical descriptors与bounded owning-region metadata；本项不产生validated per-Channel MessageIndex view。
- 验收：本项是descriptor-only preflight，不Fetch且不读取owning MessageIndex region；完全相同descriptor去重但仍计原始记录，字段冲突、Chunk/MessageIndex/DataEnd/Summary overlap、overflow、越界、重复offset、offset指向所属Chunk bytes、另一Chunk/MessageIndex region、region外gap或cross-Chunk alias都在构造interval index前失败，indexed units之间的合法data-section gap被接受。
- 验收：只凭descriptors不能判定owning region内的offset是否对齐MessageIndex record envelope起点；指向同一owning region内length prefix、record body、record middle或其他非envelope-start字节的fixture在本项可通过physical ownership验证，必须由MCAP-022拒绝。
- 负责人：kola；提交：`633ec6164f7285d01a5c283597465622a82e383f`；备注：2026-08-07 完成move-only消费MCAP-020 evidence的descriptor-only physical ownership validator，以完整字段exact duplicate canonicalization、checked Chunk/紧邻owning MessageIndex/data-section range、map presence/offset owner与全局physical-unit overlap验证签发sealed non-`Clone` `ValidatedPhysicalRegions`，并保留MCAP-019/020 evidence与两层真实reservation；本项不Fetch、不读取MessageIndex bytes、不产生per-Channel或record-alignment authority；Dafee首次完整深审为0 High/0 Medium/0 Low且无设计偏离；默认与all-features focused各30/30、all-target all-feature clippy、native feature-off check、doc1/1与compile-fail3/3、targeted rustfmt和diff-check通过，完整re_mcap为123/124且唯一失败是既有129-byte Git LFS pointer，Wasm全特性检查仍因本机缺clang阻断于既有lz4/zstd构建脚本，本机无pixi/cargo-nextest。2026-08-13追溯补充：021输出在025A集成中必须继续位于same-object enclosing owner内，不能从raw inner regions、独立binding或裸budget构造后续authority；本项physical算法不变。

### [x] MCAP-022 — 实现完整 MessageIndex region work unit

- 建议提交：`Add bounded MessageIndex region parse work unit`。
- 依赖：MCAP-021，并通过`ValidatedPhysicalRegions<'a>`的ownership链传递取得MCAP-019/020 evidence。
- 变更：Fetch完整owning MessageIndex region，只将bounded raw bytes入队，再实现region起点开始的顺序exact record-sequence/map双向验证，并把解析封装为单个可由frame driver调用的CPU work unit。
- 验收：每个map value必须等于其Channel对应MessageIndex record envelope的绝对起始file offset；非法opcode、gap、trailing bytes、record越界、offset指向length prefix/body/record middle或其他非envelope-start字节、map key/Channel ID不一致、错误Channel、缺失或额外record/map entry均失败；多个已完成region一帧最多解析一个。
- 负责人：kola；提交：`da4a9c2214401485f6a37c566bb924c7c1dadb04`；备注：2026-08-07 完成production-disarmed的单owning-region CPU work unit，以move-only方式消费MCAP-021 sealed evidence，在whole-region allocation-free preflight后签发绑定同一budget profile与exact census的non-Clone result materialization token，并构造bounded immutable per-Channel MessageIndex view；record envelope从region绝对起点顺序exact consume，descriptor map与实际Channel/record absolute start双向一一对应，非法opcode、length/body/middle offset、wrong/missing/extra/duplicate record或map、gap/trailing/cross-boundary及全部per-record/aggregate/retained caps均fail-closed；raw安装入口最终收窄为独占exact-sized `Box<[u8]>`，移除任意`AsRef` owner、附属backing和mutable-view旁路，安装零内部copy且success/failure/Drop均按backing后permit顺序精确归还；结果保留MCAP-019/020/021 evidence与真实parsed-result reservation，不携带Fetch、frame、planner、time或ambiguous-zero authority，未来exact Range body由MCAP-033适配；Dafee首轮深审发现1个High，完成Box-only类型边界修复后增量复审为0 High/0 Medium/0 Low且无设计偏离；默认与all-features focused各35/35、all-target all-feature clippy、native feature-off check、doc1/1与compile-fail3/3、targeted rustfmt和diff-check通过，完整re_mcap为128/129且唯一失败是既有129-byte Git LFS pointer，Wasm全特性检查仍因本机缺clang阻断于既有lz4/zstd构建脚本，本机无pixi/cargo-nextest。
  2026-08-13追溯补充：022 work-unit result在025A集成中也只能由same-object enclosing evidence owner派生并回填，不能以裸region result、另一binding/profile/registry lease或独立budget permit推进023；既有record-alignment算法不变。

### [x] MCAP-023 — 实现 ambiguous-zero 两阶段 preflight

- 建议提交：`Resolve ambiguous zero-time MCAP chunks safely`。
- 依赖：MCAP-022。
- 变更：先聚合验证必要 MessageIndex regions，再只为 unresolved Chunks 预检 body 请求、compressed/uncompressed bytes 和扫描预算。
- 验收：非空 time-zero index 不读取大 Chunk body，index aggregate 或第二阶段 aggregate 超限时在相应 Fetch 前失败，空 `[0,0]` 与真实 time-zero `NonEmpty([0,0])` 明确区分。
- 负责人：kola；提交：`1240e51ebb6cd0f1b092c34364c8d5711a2fbdc7`；备注：2026-08-07 完成production-disarmed的ambiguous-zero两阶段aggregate admission，以move-only方式延续MCAP-019/020/021/022 evidence并把同一MessageIndex budget profile绑定到全部顺序region turns；stage1在首个MessageIndex raw/request claim前allocation-free汇总并原子预留ambiguous Chunk数、完整owning-region bytes、`Σ(region_bytes / 16)`保守entry upper、Range count与classification retained bytes，reservation跨逐region parsed-result释放保留，故最终missing/extra/map失败shape和逐Chunk释放都不能低估或绕过总cap；每个turn只执行一个MCAP-022 work unit，非空index要求全部raw log time为零并产生显式raw `NonEmpty(0..=0)` evidence，empty/absent index保持unresolved且绝不伪造`KnownEmpty`；stage2只对unresolved Chunks在任何body claim前汇总并原子预留完整Chunk Range bytes、compressed/uncompressed bytes、Range count、scan bytes与`ceil(uncompressed/9)` record-attempt upper，只输出无Fetch/decompress/scan/final-manifest authority的sealed unresolved descriptors；全部aggregate arithmetic、allocation、contention、failure与Drop路径fail-closed，canonical ordinal lookup最终统一为O(1) `Vec::get`，单region work不再绑定全局descriptor数；Dafee首轮深审发现1个Medium，修复两处线性ordinal lookup后增量复审为0 High/0 Medium/0 Low且无设计缺陷；默认与all-features focused各41/41、all-target all-feature clippy、native feature-off check、doc1/1与compile-fail3/3、targeted rustfmt和diff-check通过，完整re_mcap为134/135且唯一失败是既有129-byte Git LFS pointer，Wasm全特性检查仍因本机缺clang阻断于既有lz4/zstd构建脚本，本机无pixi/cargo-nextest。
  2026-08-13追溯补充：023 final plan在025A集成中是same-object enclosing evidence owner的inner state，并继续持有首次probe安装的binding/profile/registry/budget issuer；025A不能接收裸plan或另配budget，本项既有分类算法不变。

### [x] MCAP-024 — 实现 exact-output bounded decompressor

- 建议提交：`Add exact-output Web MCAP decompression`。
- 依赖：MCAP-006、MCAP-017。
- 变更：为 release Wasm allowlist 增加 declared-size non-growing sink、1-byte overflow detection、single-frame EOF、compressed-input exhaustion 和 CRC-before-scan。
- 验收：zstd/LZ4 exact single frame 成功；overflow、short output、truncated input、zero progress、concatenated frame、trailing payload 和 output-full-before-EOF 均不超 reservation 且不发布 batch。
- 负责人：kola；提交：`7c9862f0307644cffe19d24a6d9d71a471fa5037`；备注：2026-08-07 完成production-disarmed的none/zstd/LZ4 exact-output bounded decompressor，以single-frame EOF、compressed-input exhaustion、zero-progress检查和计入reservation的1-byte overflow probe拒绝short/long/truncated/concatenated/trailing输出，并在任何scanner authority前完成可选CRC；zstd使用锁定1.5.7的documented `ZSTD_estimateDStreamSize`在output allocation和decoder construction前预留完整working set，LZ4手写frame parser并在block decoder调用前把destination截断到BD上限，拒绝legacy/skippable/dictionary/reserved与全部checksum/content-size异常；compressed input、work/scratch/zstd working和retained output统一绑定同一source/profile `Arc` state，sealed non-Clone physical-read handoff一次move identity、compression、sizes、CRC、exact `Box<[u8]>`和permit，production无caller-scalar constructor或public install，output只borrow identity且cross-profile拼接不可表达；Dafee三轮深审依次闭合1个High/2个Medium/1个Low及后续1个High/1个Medium，最终0 High/0 Medium且无设计缺陷；focused22/22、all-target all-feature clippy、native feature-off check、doc1/1与compile-fail4/4、targeted rustfmt和diff-check通过，完整re_mcap为156/157且唯一失败是既有129-byte Git LFS `attachments.mcap` pointer，Wasm检查仍因本机缺clang阻断于既有lz4-sys/zstd-sys构建脚本，本机无pixi/cargo-nextest。

### [x] MCAP-025 — 扫描全部 Chunk records 并重建真实 extent

- 建议提交：`Validate complete MCAP chunk record streams`。
- 依赖：MCAP-009、MCAP-020、MCAP-021、MCAP-024。
- 变更：增加source-scoped、non-`Clone`的`PhysicalChunkSourceAuthority`，一次性消费并持续拥有MCAP-021→020→019 sealed evidence、source/session generation和同一decompression budget profile；它只按canonical ordinal签发move-only、one-shot `PhysicalChunkReadLease`，调用方不能用裸ordinal、借用descriptor或自报metadata重新构造可信physical read。
- 变更：每个`PhysicalChunkReadLease<PendingHeaderValidation>`原子绑定canonical ordinal、完整Chunk record range、完整canonical ChunkIndex descriptor、expected raw extent、expected compression、expected compressed/uncompressed sizes、固定`ChunkChecksumPolicy::ValidateIfProvided`、fresh per-read generation与decompression/source profile；authority不得声称已知或接受caller提供的`declared_uncompressed_crc`。
- 变更：只有MCAP-025 sealed header validator可以消费matching pending lease与exact full-record body；它验证Chunk opcode、record envelope、exact body length和full range，解析`ChunkHeader`，把actual extent/compression/sizes与descriptor逐字段比较，并从header bytes派生`declared_uncompressed_crc`与private compressed-payload subrange，再move同一identity生成`HeaderValidatedPhysicalChunkRead`；adapter只能借用exact payload view并调用one-shot owner install，不能取得裸range/length或重组prepared MCAP-024 input。
- 变更：只有header-validated state可以进入MCAP-024；CRC零记录`NotProvided`，非零只对exact output验证，lease identity从header validation、decompression/CRC、完整scanner一直贯穿到semantic result或decompressed/validation cache；identity与绑定metadata只能borrow或由sealed transition移动，不能`Clone`/`Copy`、导出opaque scalar或在阶段之间重新传入裸字段。
- 变更：同一ordinal同时最多一个live lease，slot总数由MCAP-021 canonical Chunk count约束，in-flight与retained子集复用既有Range/raw-cache/decompressed-cache caps，Drop/abort只在generation匹配时释放live claim，释放后才可用fresh generation refetch。
- 变更：scanner 处理全部 Schema、Channel 和 Message，限制 record/message/selected-dispatch count，对全部 Message 转换 log time并重建count/min/max；本项保持production-disarmed，只实现authority、exact-owner test harness与scanner，不接真实`re_web::ExactLengthRangeBody`或公开route。
- 验收：duplicate live lease在任何Fetch/body owner前失败；lease Drop/abort后只能以更大的fresh generation refetch，cache entry仍有consumer handle时eviction进入pending reclaim并延迟同ordinal refetch，stale callback/result和旧source generation不能释放新claim、进入cache或发布semantic result。
- 验收：wrong ordinal/full-record range/profile、truncated或forged header、oversized compression field、payload bounds/trailing bytes和index↔header extent/compression/size mismatch都由allocation-free borrowed header preflight在prepared MCAP-024 input前fail-closed；zero CRC为`NotProvided`，正确nonzero CRC为`Verified`，错误nonzero CRC在scanner前失败，同一descriptor搭配不同header CRC只能得到各自body派生结果而不能在lease签发时预知。
- 验收：多Chunk顺序签发和受限并发不互相阻塞，释放某ordinal不改变其他ordinal，authority或source关闭后所有迟到结果只释放ownership；header前、payload copy后与MCAP-024后stale injection都不能进入下一state、cache或semantic result。
- 验收：空/非空状态、Chunk header、ChunkIndex descriptor和实际min/max必须精确一致，未选Message的非法时间也失败，最大密度fixture在处理第一个超限元素前停止；compile/API测试证明caller/MCAP-033不能传CRC或metadata，scanner与cache也无法脱离同一lease identity接收任意同长度`Box<[u8]>`。
- 负责人：kola；提交：`a7615316266ae96bfb575a7d8cb7a1b5280d1c52`；备注：2026-08-08 完成production-disarmed方案A，以source-scoped physical authority和per-canonical-ordinal move-only typed lease独占MCAP-021→020→019 evidence，并以allocation-free完整Chunk header validator从exact body派生CRC、payload和MCAP-024 identity/decompression链；完整semantic scanner验证全部records、未选Channel和raw/canonical extent，使用固定u16 domain O(1) Summary projection、bounded result/cache/reclaim、stale source/read generation与backing→permit→lease Drop顺序。设计修订补齐projection budget/lifetime、metadata key/value单字段cap，并移除metadata duplicate quadratic prefix rescan：borrowed whole-Chunk pass只做单次线性structure/UTF-8/cap/scalar验证，取得scan/result reservation后才由parser duplicate rejection或encoded-entry/map-cardinality差异和projection exact-set闭合；Dafee多轮深审后无中级及以上问题且无设计缺陷。真实门限为scanner 29/29、all-features tests check、all-features/tests clippy `-D warnings`、no-default-features all-targets、rustfmt与diff-check通过；完整re_mcap为186/187，唯一失败是既有129-byte Git LFS `attachments.mcap` pointer，doc1/1与compile-fail4/4通过；本机无pixi/cargo-nextest，Wasm仍因缺clang阻断于既有lz4-sys/zstd-sys构建脚本，真实Chrome Range owner适配仍保留给MCAP-033。

### [x] MCAP-025A — 闭合 remote object、ambiguous-zero 与 physical authority ownership

- 建议提交：`Bind remote MCAP physical source resolution ownership`。
- 依赖：MCAP-009、MCAP-015、MCAP-023、MCAP-025。
- 变更：增加fresh-per-open、sealed、non-`Clone`的`BoundRemoteObjectCapability`，在Wasm-only Web Viewer opening adapter中以不可导出的session identity把`re_web::BoundChromeRangeObject` validator owner与one-shot lower-preparation authority封成不可分离owner；首次bound Header/Footer/Summary probe进入fixed-layout前，该authority原子安装`re_mcap`只能比较不能构造的opaque physical-object binding，URL、query、Authorization、Cookie和ETag原文不得进入session identity、manifest identity、lower binding、日志或可序列化字段，`re_web`与`re_mcap`互不新增直接依赖。
- 变更：同一次one-shot transition还冻结source/decompression profile、instance-registry lease、aggregate budget root与子reservation issuer，并构造enclosing `BoundRemotePhysicalEvidence`；lower binding及这些authority必须从MCAP-019→020→021→023随每个inner typestate原样move，所有transition都不能接受裸binding/profile/lease/budget或caller scalar补装。
- 变更：增加move-only `PreparedPhysicalSourceResolution`，只允许以上层capability为enclosing owner并一次性消费携带matching lower binding/profile/registry/budget root的完整MCAP-023 `BoundRemotePhysicalEvidence<PreparedAmbiguousZeroBodyPlan>`，再从plan内部持有的MCAP-021→020→019 evidence构造且独占MCAP-025 `PhysicalChunkSourceAuthority`；`re_mcap` inner owner不持有validator wire bytes，不得分别复制、重建或双消费evidence/binding，也不得组合独立budget permits。
- 变更：coordinator冻结unresolved canonical ordinal的唯一升序序列，只能为当前next ordinal从内部authority签发one-shot lease，并只接受同一lease经MCAP-025完整header/decompression/scan链返回的ambiguous extent resolution；matching result提交classification后必须先Drop scan/decompressed backing与live lease并恢复该ordinal slot，才能推进next ordinal或finalize，duplicate、out-of-order、cross-source、wrong-object、wrong-generation和stale result在改变classification前fail-closed。
- 变更：在025A首次allocation、slot构造、Fetch或lease签发前完成checked census，并由chain-carried issuer原子预留完整simultaneous peak：既有019→023 retained evidence/reservations、authority slot table、classification、canonical conversion、sort/prefix-max-end或等价index scratch、final retained layout、container capacity及old/new transition overlap必须同时计费；任一失败保持allocation/work为零，不允许阶段性裸budget组合或allocation后补reserve。
- 变更：所有frozen ordinals恰好resolution一次后，`finalize`只能成功一次并在已预留容量内move生成私有拥有`ResolvedCanonicalPhysicalLayout`的`ResolvedPhysicalSourceAuthority` inner owner，layout不能被拆出或比authority活得更久，且不能在成功prefix后进行未预留的fallible allocation。
- 变更：enclosing final capability继续持有Web validator owner，inner owner持有matching lower binding、完整physical/definition evidence、profile、registry lease、MCAP-025 authority及全部live reservations；唯一final API是由enclosing owner借出生命周期绑定的`ResolvedRemotePhysicalSourceRef`，不存在take/clone/serialize inner owner或任一authority的API，MCAP-032必须move整个enclosing final capability并在内部borrow，不能只复制identity或独立接收inner authority。
- 变更：未完成或失败的coordinator `Drop`先关闭authority并使全部迟到lease/result失效，再按backing/evidence/reservation顺序释放；不得发布partial layout、partial classification、manifest identity或Store effect，也不得让caller从已解析scalar重建final owner。
- 变更：本项保持production-disarmed，不依赖MCAP-043，也不创建source status、operation或public opening lifecycle；test issuer仅在`cfg(test)`存在，release路径仅允许MCAP-088模块内专属sealed artifact bridge构造one-shot capability，现有ROS 2/protobuf artifact cfg、一般verifier或feature cfg均不可构造；真正public opening由MCAP-043以后使用matching opening token接通，因此不存在025A↔043依赖循环。
- 验收：覆盖fresh open identity、same URL reopen identity不同、URL/ETag不进入identity、strong/deployment-assumed validator绑定、wrong content length/object capability，以及capability、plan、profile、registry lease和budget root的cross-source组合全部在首次allocation、authority或Fetch前失败；compile/API测试证明019→023任一inner state都不能脱离same-binding enclosing owner构造或转移。
- 验收：覆盖零、一个和多个unresolved ordinal，严格升序、duplicate、skip/out-of-order、wrong lease、stale source/read generation、scanner failure、result仍持有claim时推进/finalize、coordinator early Drop和finalize twice；失败时classification/layout不可观察，authority关闭且全部reservation逐位归零，成功finalize时不存在resolution遗留live claim。
- 验收：合法final owner给出完整canonical `NoIndexedMessages | Known` extent、保留重叠canonical time intervals且查询不漏更早开始的长区间，并能继续签发matching MCAP-025 leases；physical byte regions仍必须满足MCAP-021全局不重叠，compile/API测试证明raw plan、borrowed regions、裸ordinal、scalar extent、session token或独立validator不能构造final owner。
- 验收：first-allocation instrumentation和budget快照覆盖零/最大Chunk、reservation contention、checked overflow、slot/classification/index/final-layout allocation failure与finalize transition，证明完整同时峰值无遗漏、失败零partial allocation/classification/claim且registry lease及全部budget逐位恢复。
- 验收：dependency/API测试证明`re_mcap`没有`re_web`依赖、`re_web`没有`re_mcap`依赖且inner code没有HTTP validator wire access，Wasm-only upper capability与lower binding不能分离、替换或cross-object组合；symbol/cfg测试证明test issuer仅在`cfg(test)`，MCAP-088专属sealed bridge之外的ROS/protobuf artifact、一般feature、native、nonremote Web和server均不可构造；existing native/local MCAP与所有nonremote Web行为不变。
- 负责人：kola；提交：本提交；备注：2026-08-13 完成production-disarmed的resolved remote physical source ownership：fresh-per-open object capability与lower binding从首次bound probe沿019→020→021→022→023单一non-Clone outer typestate贯穿，025A不接受裸plan/profile/budget或scalar identity；BoundRemoteObjectRead由upper owner签发并覆盖Header/Summary/MessageIndex读取，PhysicalChunkSourceBinding携带同一object Arc/generation。FullResolutionAdmissionCensus在任何backing前计入019→023 nested materialization/strings/schema bytes、owner/container/Arc/Mutex、authority slots/projection、classification/interval/prefix/unresolved和transition overlap，AggregateResolutionTransaction先原子root+child claim后才分配，失败rollback逐位恢复；post-claim只做预留内infallible fill/move，safe transition guard保证lower→object→validator Drop顺序。final outer只借出same-binding per-ordinal metadata/definitions projection与matching lease，032可move消费完整owner但不能拆分或重建。Dafee最终review 0个中级及以上问题且无设计偏离；`remote_physical_resolution` 19/19、all-features check、library Clippy `-D warnings`与diff-check通过，nextest/pixi/Wasm clang受环境限制，production route、native/local API、Viewer行为和Rerun服务端不变。

### [x] MCAP-026 — 建立 bounded ROS 2 reflection initializer

- 建议提交：`Add bounded remote ROS2 schema initialization`。
- 依赖：MCAP-020。
- 变更：只为Web remote路径增加crate-private initializer，先allocation-free冻结allowlist/assignment/executable profile并从同一source的sealed `ValidatedSummaryDefinitions`签发non-`Clone` same-source/policy的same-topic signature evidence，再由ROS 2 initializer消费并move该evidence；native/local decoder trait、公开API、初始化路径和运行语义保持不变。
- 变更：在首次分配、字符串物化或intern之前，对全部候选ROS 2 `.msg`主定义和依赖定义执行allocation-free whole-schema grammar census，并按versioned strict supported subset检查record count、definition bytes、field count、array bound、nesting/dependency depth、标识符/默认值/字符串字面量长度、复杂类型引用和总同时存活峰值。
- 变更：把ROS 2 census/working-set limits作为remote-only typed `Unfrozen` keys接入canonical limits schema，MCAP-088前只允许test/measurement profile构造且production route保持disarmed；grammar profile在本项固定，不能由GA测量改变语义。
- 变更：以固定数量的contiguous fallible arenas和borrowed source spans承载parsed result，预留公式覆盖layout/alignment、locked Wasm allocator rounding及temporary/final overlap；dependency/duplicate resolution逐步消耗typed initializer-step budget，不允许per-field allocation、schema string复制或未计数quadratic scan。
- 变更：same-topic decoder-relevant signature preflight必须先于initializer；`wstring`、subset外语法、未解析或循环依赖、超限输入和无法证明allocation bound的schema返回typed `UnsupportedForRemote`，不得降级Raw、不得留下partial intern、Store、policy或decoder side effect。
- 变更：admission一次预留materialization scratch、partial builder、parsed representation、dependency resolution和recognition result的完整同时峰值；后续row/Arrow output由MCAP-030另行预留；成功只返回sealed、move-only、source/policy-bound result和reservation，后续remote decode必须直接消费该结果，禁止预检后再次调用既有无界reflection initializer。
- 验收：覆盖malformed grammar、local initializer parse error、字段/依赖/nesting/default/string/array边界、complex type resolution、`wstring`、same-topic冲突、ineligible malformed schema、allocation failure和所有rollback路径；invalid/unsupported/limit/allocation分类稳定且脱敏，失败前allocation/intern/Store/policy side effect均为零且budget snapshot逐位恢复。
- 验收：对strict admitted subset与既有local reflection路径做owner、解析schema和合法payload输出差分；compile/API测试证明raw Summary、另一source定义、裸schema bytes或已释放reservation不能构造或重绑remote initializer result。
- 验收：unknown/reordered/duplicate policy identity先于signature和initializer失败；same-topic evidence覆盖全部canonical Channel及ROS 2/protobuf两类signature，冲突先于任一initializer counter变化；compile/API测试证明caller不能用bool、revision、裸definitions、另一source或另一policy伪造evidence。
- 负责人：kola；提交：`5ce10a20fc460d9e274e2cd9bc8c19aa36a04944`；备注：2026-08-11 完成production-disarmed的Web remote bounded ROS 2 initializer，以allocation-free exact census/dry-run、same-topic全字段signature、独立signature/materialization/projection/recognition step owner、fixed arenas、exact peak reservation和sealed source/policy/profile/generation ownership闭合strict subset、rollback、recognition与026→027 opaque post-EOF projection边界，native/local decoder trait、公开API和ordinary Web product行为保持不变；canonical Linux-x64使用独立verifier artifact、consumer-side generated capability、locked Rust 1.95 compiler/sysroot/rust-lld digest、真实stack/allocator/stage call-graph证明，product artifact不链接probe且普通Wasm不进入locked gate，MCAP-088负责最终production capability；Dafee九轮增量深审后无中级及以上问题且无设计缺陷，真实门限为ROS 2 focused31/31、protobuf boundary1/1、re_build_tools13/13、re_dev_tools4/4、三相关crate all-features/all-targets Clippy `-D warnings`、re_mcap no-default check、targeted rustfmt和diff-check通过，完整re_mcap为218/219且唯一失败是既有129-byte Git LFS `attachments.mcap` pointer，doctest1/1与compile-fail7/7通过；本机无pixi/cargo-nextest，ordinary Web和已越过双consumer attestation的release verifier均因缺clang阻断于既有ring/lz4/zstd native构建，native re_viewer门限因缺libudev.pc阻断；MCAP-027保持pending，未启动。

### [x] MCAP-027 — 建立 bounded protobuf descriptor initializer

- 建议提交：`Add bounded remote protobuf descriptor initialization`。
- 依赖：MCAP-020、MCAP-026。
- 变更：只为Web remote路径增加crate-private initializer，且只消费MCAP-026返回的same-source sealed signature+ROS result并把它move进combined initializer result；native/local protobuf decoder trait、公开API、`DescriptorPool`路径和运行语义保持不变。
- 变更：在首次分配、字符串物化或intern之前，对全部候选`FileDescriptorSet`执行allocation-free protobuf wire census，并按versioned strict supported subset检查wire type、truncation/overflow、unknown-field policy、file/message/field/enum/service/options cardinality、nesting/dependency/import depth、duplicate file/symbol、named-message存在性和全部字符串/default/options bytes。
- 变更：把protobuf census/working-set limits作为remote-only typed `Unfrozen` keys接入canonical limits schema，MCAP-088前只允许test/measurement profile构造且production route保持disarmed；descriptor profile在本项固定，不能由GA测量改变语义。
- 变更：descriptor graph复用MCAP-026 fixed-arena/step-budget ownership模式，以borrowed string spans、sorted symbol/dependency projections和checked offsets构建，不使用per-symbol heap node、隐式growing map或未计数quadratic resolution。
- 变更：same-topic decoder-relevant signature preflight必须先于initializer；malformed或unknown wire field、missing import/type/message、duplicate、subset外options或无法证明allocation bound的descriptor返回typed `UnsupportedForRemote`，不得降级Raw、不得留下partial descriptor graph、intern、Store、policy或decoder side effect。
- 变更：admission一次预留materialization scratch、partial builder、bounded descriptor graph和recognition result的完整同时峰值；后续row/Arrow output由MCAP-030另行预留；成功只返回sealed、move-only、source/policy-bound result和reservation，后续remote decode必须直接消费该graph，禁止预检后再次调用既有无界`DescriptorPool::decode`。
- 验收：覆盖malformed/truncated/overflow wire、unknown fields、descriptor graph cardinality/nesting、duplicate file/symbol、missing import/type/named message、options/default/string边界、allocation failure和所有rollback路径；invalid/unsupported/limit/allocation分类稳定且脱敏，失败前allocation/intern/Store/policy side effect均为零且budget snapshot逐位恢复。
- 验收：对strict admitted subset与既有local protobuf路径做owner、descriptor resolution和合法payload输出差分；compile/API测试证明raw Summary、另一source定义、裸descriptor bytes或已释放reservation不能构造或重绑remote initializer result。
- 负责人：kola；提交：`5a6d601dd8017f0d77bc723b655d4394171aa59b`；备注：2026-08-11 完成production-disarmed的Web remote bounded protobuf descriptor initializer，通过MCAP-026 one-shot bounded projection与opaque post-EOF authority保持same-source/policy/profile/generation ownership，以allocation-free严格wire census、absolute-only type resolution、标准descriptor defaults、完整namespace/oneof/map_entry校验、独立exact step/byte/arena/peak reservation和13个fallible contiguous arenas生成sealed ROS+protobuf combined graph，禁止raw definitions、裸DescriptorPool或二次无界初始化，native/local protobuf API、DescriptorPool路径和服务端行为不变；release verifier要求protobuf probe同时覆盖ROS与protobuf全部stage、locked allocator direct-call和独立call-graph/stack证明，product artifact排除全部internal symbols；Dafee首轮1个High与7个Medium及后续proto2 namespace Medium全部闭合，最终无中级及以上问题且无设计缺陷，真实门限为protobuf focused25/25、ROS regression31/31、artifact4/4、re_build_tools13/13、相关Clippy `-D warnings`、no-default、targeted rustfmt与diff-check通过，完整re_mcap为243/244且唯一失败是既有Git LFS `attachments.mcap` pointer，doctest1/1与compile-fail8/8通过；真实release Web仍因缺clang阻断于既有ring/lz4/zstd native构建；合法payload输出差分属于MCAP-030/032实际消费bounded graph时的验收，本项不冒充已覆盖，MCAP-028可继续消费combined initializer ownership。

### [x] MCAP-028 — 建立 Web remote decoder assignment adapter

- 建议提交：`Add deterministic Web decoder assignment adapter`。
- 依赖：MCAP-020、MCAP-026、MCAP-027。
- 变更：Web remote adapter只消费MCAP-027返回的same-source/policy combined initializer result及同一definitions派生的`KnownNonEmpty | Unknown` eligibility，复用既有priority/fallback core构造remote assignment；它不能重新解析policy、重复same-topic preflight或初始化decoder，也不修改native/local `ExecutionPlan`的empty pruning、diagnostics、initialization、owner、runner或output语义。
- 变更：assignment result以lifetime或move-only ownership保留same-source definitions、resolved allowlist/policy/config/table/fallback identity、MCAP-026/027的matching bounded initializer result和reservation，MCAP-029只能消费该result构造sealed immutable Channel-group owner；projection、recognition scratch和minimal result在首次allocation前完成checked census，不能重跑ROS 2或protobuf initializer，完整manifest由MCAP-032独占finalize。
- 变更：V1 semantic ROS 2使用exact-safe parser/config table而非整个`McapRos2Decoder`；当前审计中static-producing parser全部排除，其余parser因尚缺provenance、deterministic ordinal和registration-bound完整证明也保守排除，因此V1 semantic table暂为空，local semantic命中结构化返回`SemanticParserNotAllowlisted`且不得降级。
- 验收：通过same-topic preflight的ROS 2 reflection、protobuf、Raw overlap fixtures与现有local `ExecutionPlan`得到相同唯一owner，独立oracle和有效payload证明native output/side effect不变，Statistics/MessageIndex伪造zero、allowlist reorder和schema-less Channel不会让remote静默丢消息。
- 验收：同topic valid ROS 2↔`wstring`双顺序、不同protobuf schema、ineligible冲突均在Store创建前返回`ConflictingTopicDecoderSignature`；完全相同semantic signature的duplicate可通过且顺序无关；所有static-producing、unknown和尚未certify的semantic parser返回`SemanticParserNotAllowlisted`且不降级。
- 验收：compile/API tests证明raw `mcap::Summary`、裸owner map、cross-source/cross-policy重绑和evidence drop后使用不可表达；projection/result exact bytes、initializer proven working set、aggregate contention及所有失败/Drop路径恢复budget snapshot；production remote route保持disarmed且native/public API不扩大。
- 负责人：kola；提交：本提交；备注：2026-08-12 完成production-disarmed的Web remote decoder assignment adapter，只消费MCAP-027的same-source/policy combined initializer ownership和物理Chunk message evidence，冻结canonical policy与source semantic config后以deterministic priority/fallback core生成move-only assignment；V1 semantic allowlist保守保持为空，所有semantic候选结构化返回`SemanticParserNotAllowlisted`且不降级Raw，native/local `ExecutionPlan`、公开decoder API和服务端运行行为不变。artifact verifier的definitions capability与source state复用同一`PhysicalChunkSourceBindingV1`，独立binding仍进入fatal control-plane，闭合locked release-Wasm probe的authority一致性。Dafee最终增量review无中级及以上问题且无设计缺陷；门限为artifact probe 2/2、decoder assignment定向21/21、`re_mcap` all-features library check及Clippy `-D warnings`通过，目标Rust文件已格式化且diff-check通过；本机无pixi/cargo-nextest，全workspace fmt受既有缺失`docs/snippets/src/snippets.rs`阻断，真实release Web构建仍受既有clang依赖缺失限制。

### [x] MCAP-029 — 构建 immutable Channel-group 与稳定 source identity

- 建议提交：`Add immutable remote MCAP channel groups`。
- 依赖：MCAP-028。
- 变更：消费MCAP-028 move-only assignment并继续拥有MCAP-026/027 sealed initializer results及matching reservation，生成decoder/config hash/canonical membership/registration bounds、唯一 `ChannelId → group_id`、dense partition key、canonical source order、deterministic ordinal、stable RowId。
- 验收：identical group 去重，跨owner/source/policy initializer重绑和membership overlap拒绝，相同输入在seek、GC reload和allowlist serialization reorder后产生相同identity，group descriptor不能退化为裸schema bytes后重新初始化。
- 负责人：kola；提交：本提交；备注：2026-08-12 完成production-disarmed的immutable remote Channel-group与稳定source identity：move消费MCAP-028 assignment并持续持有MCAP-026/027 sealed initializer ownership及reservation，以bounded materialization阶段冻结的ROS 2/protobuf canonical executable-config digest、decoder固有registration contract、canonical membership和dense group identity生成唯一`ChannelId → group_id`投影；protobuf identity显式覆盖materialized graph、依赖/resolution及完整named-message identity，ROS 2使用显式versioned parsed-type编码，028/029不回读裸schema。group resolution为non-Copy lifetime-bound handle，禁止跨source/policy重绑；canonical source order与derived ordinal生成stable RowId。029以每个真实allocation的locked-Wasm footprint核算working、retained及同时峰值，真实assignment-owner combined-minus-one在首次group allocation前拒绝并恢复026/027/028/029全部预算。Dafee多轮增量review后0个中级及以上问题且无设计缺陷；门限为`re_mcap` all-features/all-targets check与Clippy `-D warnings`、Channel-group定向4/4、真实owner/Drop与combined-minus-one测试、protobuf local-pool差分、doctest及diff-check通过；production route仍disarmed，native/local decoder API、Viewer运行行为和Rerun服务端不变。

### [x] MCAP-030 — 实现 PhysicalChunkValidationCount

- 建议提交：`Add remote chunk validation and count pass`。
- 依赖：MCAP-025、MCAP-029。
- 变更：第一遍从实际Message headers和manifest持有的MCAP-026/027 bounded initializer results构造exact per-Channel count、payload bytes、selected dispatch和versioned decoder resource-bound plan，不创建parser或重新解析schema。
- 验收：MessageIndex count零、一或严重低估都不影响exact plan，resource admission前parser/builder allocation为零，plan/decompressed/initializer ownership跨帧受retained budget和stale cancellation约束，compile/API测试阻止调用local unbounded initializer。
- 负责人：kola；提交：本提交；备注：2026-08-12 完成production-disarmed的`PhysicalChunkValidationCount`首遍工作单元，只消费MCAP-025 sealed physical message evidence与MCAP-029 immutable group owner，从实际Chunk Message headers生成exact per-Channel count、payload bytes、selected group和versioned decoder resource-bound plan，不读取Statistics、MessageIndex或`msg_offsets`，不创建parser/builder也不调用local initializer。plan在分配前后校验physical evidence与group owner的same-source binding/current generation，并move持有decompressed/scan evidence、026/027 initializer及029 group ownership；retained budget以locked-Wasm footprint计入plan、physical backing和group/initializer reservation，stale/cross-source失败与Drop均恢复预算且physical claim保持至plan释放。真实integration覆盖025→026/027→028→029→validation/count、实际header exact count/payload/group、claim lifetime、combined exact/minus-one与预算归零；production call graph和adversarial assignment fixture证明错误/缺失Statistics与MessageIndex不影响结果。Dafee最终review为0个中级及以上问题且无设计缺陷；`re_mcap` all-features check、Clippy `-D warnings`、focused validation tests 3/3与diff-check通过，production route仍disarmed，native/local decoder API、Viewer运行行为和Rerun服务端不变。

### [x] MCAP-030A — 提供 sealed bounded executable decoder adapter

- 建议提交：`Add sealed remote executable decoder adapters`。
- 依赖：MCAP-026、MCAP-027、MCAP-030。
- 变更：在 bounded ROS 2/protobuf initializer 所属模块内生成 source/policy/config-bound、one-shot executable adapter；adapter 直接消费已 materialized 的 parsed arenas/graph，封装 exact `num_rows`、payload、step、scratch、builder 与 output reservation，不暴露 raw schema、graph 或可重绑 parser identity，不调用 `MessageSchema::parse` 或 `DescriptorPool::decode`。
- 变更：ROS 2 仅允许已认证 reflection subset；semantic decoder 继续结构化返回 `SemanticParserNotAllowlisted`。protobuf adapter 直接使用 bounded descriptor graph 完成 admitted wire decode/Arrow construction，并绑定 source generation、policy versions、schema handle、canonical config digest。
- 验收：adapter 只能从 matching MCAP-026/027 sealed owner 借用，cross-source/cross-policy/stale/drop 后使用均失败；所有 parser/builder/temporary allocations 在 reservation 内并可回收；合法 admitted ROS 2/protobuf payload 与 local normalized output 差分一致；raw schema、裸 descriptor、local unbounded initializer 不能构造 adapter；native/local API、Viewer 行为和 Rerun 服务端不变。
- 负责人：kola；提交：本提交；备注：2026-08-13 完成用户批准的方案1：在026/027 bounded initializer内部增加production-disarmed、source/policy/config/generation-bound、one-shot executable adapter；MCAP-030提供不可Clone的validated per-message envelope lease，adapter直接消费私有materialized ROS2/protobuf arenas/graph生成opaque bounded normalized IR，禁止raw schema、裸graph、`MessageSchema::parse`和`DescriptorPool::decode`。ROS2仅认证reflection subset，semantic与未证明protobuf features在admission前结构化拒绝；protobuf wire reader按sealed field/type/resolution执行。adapter每条/每批在每个safe point重验binding，失败即poison且不可重放；reservation覆盖payload、metadata、IR fields/bytes/rows、scratch、builder/output及同时峰值，BudgetRoot冻结实例级global cap并共享实际reserve/drop。normalized span验证checked bounds、tree interval ownership和overflow；真实ROS/protobuf chain、差分、optional/unknown/truncated、cross-source/stale、double-execute/poison、预算exact-minus-one和Drop测试通过。Dafee多轮增量review最终0个中级及以上问题且无设计缺陷；`re_mcap` all-target check、Clippy `-D warnings`、focused executable tests及diff-check通过，production route仍disarmed，native/local API、Viewer行为和Rerun服务端不变。

### [x] MCAP-030B — 提供 source-bound typed output-shape projection 与 Arrow builder capability

- 建议提交：`Add source-bound remote typed output projection`。
- 依赖：MCAP-026、MCAP-027、MCAP-030A、MCAP-032。
- 顺序决策：2026-08-14 经产品批准，作为 MCAP-031 的前置工作项；原因是现有 026/027 adapter 仅能暴露 bounded normalized IR，无法安全构造 local-equivalent Arrow/Chunk output。
- 变更：在 026/027 sealed initializer owner 内提供不可伪造、source/policy/generation-bound 的 typed output-shape projection，包含字段 ordinal、字段名、Arrow datatype、entity/component contract、timeline contract 与 deterministic row-id contract；不得暴露裸 schema/graph或允许scalar重建。
- 变更：提供 source-bound、预算受控的 typed Arrow/Chunk builder capability，所有 builder/array/row/output allocation 在首次分配前完成 checked reservation；失败必须零 partial publication。Root `ChunkId` 只能由 MCAP-032 matching partition authority 按 output ordinal 签发，builder不得接受 caller identity。
- 验收：ROS field ordinal 与实际 decoder 对 constants、nested、多 specification 完全一致；合法 scalar fixture 与 local RecordBatch/Chunk 差分一致，复杂/未认证语义结构化拒绝；entity path、component、canonical log/publish timeline、source order和deterministic RowId均来自 sealed contract。
- 验收：cross-source/session/generation/policy、stale authority、伪造 root、错误 timeline/component、超预算和任意 partial failure 均在 publication 前失败；native/local API、Viewer行为和Rerun服务端不变。
- 负责人：kola；提交：本提交；备注：2026-08-14 完成production-disarmed实现：026/027 live executable factory签发不可伪造的source/policy/config/generation-bound typed output descriptor，冻结ROS field ordinal/name/exact scalar Arrow datatype、entity path、component/archetype与time type；typed batch持有matching physical binding与config digest。builder只消费030 exact plan和matching 032 temporal partition authority，reservation前拒绝evidence/partition ordinal错配，root `ChunkId`由authority签发，RowId使用sealed message absolute/local offset保持canonical source order，caller不能注入identity；所有Arrow/Chunk staging由plan-owned locked-Wasm output reservation覆盖并以sealed handoff保持retained lease。真实full-chain differential从025A finalized owner贯穿026/027→028→029→032→030→030A→typed builder，并与同一MCAP字节经local ROS decoder产生的Chunk逐项比较entity、component/archetype、Arrow datatype/nullability、value和canonical timeline；差分修复field nullability、entity/archetype反置与adapter field capacity低估。预算census逐项覆盖Arrow/Struct owners、临时concat/collect、buffers/maps、final Chunk和metadata overlap；零分配pointer registry以production同一locked footprint测量真实高水位，1-row Int32及3-row Int32/Bool/U64均小于census，exact/minus-one通过。Dafee最终复审0个中级及以上问题且无设计偏离；focused 6/6、all-features library check、Clippy `-D warnings`与diff-check通过。完整313项lib test的14项失败均归因于既有LFS fixture或此前staged classification/source-shape问题，非030B增量；production route继续disarmed，native/local API、Viewer行为和Rerun服务端不变。
- 复审修复：RowId不再hash session/partition/row ordinal，而是只从matching sealed message envelope携带的top-level record absolute offset与record-local offset构造`CanonicalSourceOrderKeyV1`，V1 derived ordinal固定为0；ordering、跨record interleaving与48-bit local-offset边界测试通过。
- 复审修复：validation plan在任何reservation前强制比较physical evidence canonical ordinal与MCAP-032 temporal partition authority source-unit ordinal，cross-ordinal与ordinal conversion overflow均返回`ManifestMismatch`。
- 复审修复：typed output peak删除固定magic byte allowance，改为versioned `Layout` census并逐项应用locked Wasm allocator footprint，覆盖同时存活的`ChunkBuilder`/`Chunk` owners、map slots、row IDs、两条timeline staging与final buffers、per-row scalar/Struct owners、list offsets/validity和sealed descriptor metadata双owner overlap；plan既有combined reservation继续同时保留physical evidence/lease与manifest owner。exact/one-byte-short、metadata growth、overflow和完整full-chain测试通过。

### [x] MCAP-031 — 实现 PhysicalChunkDispatchDecode 与 terminal partition contract

- 建议提交：`Add admitted remote chunk dispatch and decode`。
- 依赖：MCAP-026、MCAP-027、MCAP-030、MCAP-030A、MCAP-032。
- 顺序决策：2026-08-13 经产品批准，先完成MCAP-032，再返回本项；原因是stable partition、root descriptor和root `ChunkId`只能由immutable manifest签发，031不得建立第二identity真源。
- 变更：admission成功后只从matching manifest partition/root descriptor和同一sealed bounded initializer result以exact `num_rows`构造parser，生成deterministic derived chunks，并把结果限制为完整`Complete/CompleteEmpty`或零partial publication的`Failed`；禁止从schema bytes重新调用`MessageSchema::parse`或`DescriptorPool::decode`。
- 变更：每个output ordinal必须交由MCAP-032签发的opaque root issuer生成root descriptor与`ChunkId`；031不能自行hash、随机生成、从row/output bytes派生或复制local identity，且cross-session/cross-generation/cross-partition/stale issuer在构造`DerivedChunk`前失败。
- 变更：protobuf必须实现profile内完整bounded Arrow语义，包括proto2/proto3 default/null、explicit/implicit presence、oneof、enum、singular/repeated、packed/unpacked、map-entry、nested message和unknown application field policy；不得因依赖重排缩窄产品语义。
- 验收：actual append/dispatch与plan不一致失败；`OpeningStatic`只含static，temporal partition必须含canonical log timeline且不能产生static，消息派生static decoder不在allowlist；stale/cross-source/cross-policy initializer result不能decode，合法admitted ROS 2 reflection/protobuf subset与local normalized output差分一致，semantic fixtures继续结构化拒绝。
- 验收：protobuf default/null、presence、oneof、enum、repeated、packed和map fixtures逐项与local normalized output差分，全部builder/temporary/output allocations受首次allocation前reservation约束；同一manifest partition+ordinal重复签发稳定identity，不同identity domain严格分离，任一失败零partial terminal publication。
- 当前中间态：已由MCAP-030B与本项闭合并提交，不再存在normalized terminal或临时禁用测试。
- 负责人：kola；提交：本提交；备注：2026-08-14 完成production-disarmed的原子partition dispatch：validation plan只冻结matching 032 temporal group，terminal按canonical group membership一次消费所有channel descriptor/adapter，逐channel exact rows/payload并以canonical membership ordinal签发唯一root；所有Chunk/reservation先进入sealed partition staging，任一失败Drop全部且零partial，zero-row member仍验证/消费但不emit并保留ordinal hole，全部zero仅在所有owner source/config/policy校验后`CompleteEmpty`。029修复为按decoder/config/registration合并canonical membership并checked聚合registration/external-origin bounds。protobuf recursive typed path直接消费027 sealed graph与030A bounded spans，覆盖proto2/proto3 implicit/explicit presence与defaults、nested singular merge、repeated packed/unpacked、real oneof跨variantlast-one-wins、enum name/value、map排序与duplicate-key last-wins、string/bytes C-escape、unknown application field ignore；不重解schema。真实full-chain local differentials覆盖nested/repeated/presence、proto2 oneof/enum/default、map/unknown/duplicate及同组/混组terminal；recursive output census按descriptor shape/payload与locked allocator高水位验证，exact/minus-one和aggregate handoff capacity通过。所有临时`cfg(any())`已清理。Dafee多轮复审最终0个中级及以上问题且无设计偏离；focused 33/33及后续terminal/zero-row/assignment/group门限通过，all-features library check、fmt、diff-check及增量Clippy `-D warnings`通过。production route继续disarmed，native/local API、Viewer行为和Rerun服务端不变。

### [x] MCAP-032 — 构建 immutable manifest 与全 session metadata preflight

- 建议提交：`Build bounded immutable remote MCAP manifest`。
- 依赖：MCAP-009、MCAP-025A、MCAP-026、MCAP-027、MCAP-029、MCAP-030、MCAP-030A。
- 顺序决策：本项不依赖完整MCAP-031，并在031之前实施；只消费MCAP-025A finalized owner、immutable Channel group、validation/resource contract和sealed executable output-shape contract，不执行payload decode或构造Arrow rows。
- 变更：physical input只能move消费MCAP-025A的完整enclosing final capability，并在其内部borrow`ResolvedRemotePhysicalSourceRef`取得`ResolvedCanonicalPhysicalLayout` projection；不得独立接受`ResolvedPhysicalSourceAuthority`、MCAP-023 plan、MCAP-025 authority、raw classification、extent vector或session scalar，也不得再次执行ambiguous-zero resolution或提前释放upper validator owner。
- 变更：构建canonical indexed extent、overlap-safe interval index、Channel-group table，并以sealed ownership保留MCAP-026/027 bounded initializer results，再checked-compute全文件partition、empty entry、root descriptor、external-origin和registration metadata headroom。
- 变更：本项独占接管MCAP-029的non-`Clone` group owner，并把MCAP-030当前直接move该owner的pre-integration入口收窄为从完整manifest借用same-lifetime group projection、initializer/factory capability和partition/root descriptor；不能复制owner或让030/032各自消费一次。
- 变更：immutable manifest是`DerivationPartitionKey`、stable partition descriptor、root descriptor namespace和root `ChunkId`的唯一authority；它为每个canonical source unit/group签发opaque partition descriptor及versioned root issuer，031、registration、GC和refetch只能消费该同一authority。
- 变更：root issuer在decode前冻结stable identity fields、domain separation、deterministic output ordinal规则和allowed descriptor shape；decode后只允许补入已预留且不改变identity的bounded actual metadata。
- 验收：所有乘法在集合构造前checked，合法最大manifest完整遍历不耗尽预留，不合法输入在Store创建前失败，`NoIndexedMessages`仍保留`OpeningStatic`且不声称source empty；manifest Drop后initializer reservation恰好释放一次。
- 验收：compile/API测试证明validation plan、030A factory、031 output builder、caller scalar/hash和另一session/generation不能构造partition key、root descriptor或`ChunkId`；same partition+ordinal稳定，cross-domain不同，超过registration bound在decode/publication前失败。
- 负责人：kola；提交：本提交；备注：2026-08-13 完成production-disarmed的immutable manifest authority：消费MCAP-025A resolved physical source ref与MCAP-029 immutable groups，在任何allocation前校验stale source并checked preflight partition、retained metadata、registration和external-origin headroom；session-level `OpeningStatic` 在零indexed units时仍保留。Root issuer使用固定domain/version与little-endian字段的BLAKE3-128，绑定fresh session、physical generation、source ordinal、partition kind/group和output ordinal；caller scalar/hash、cross-session/generation/kind及超界ordinal均fail-closed。Dafee多轮审查后无中级及以上问题且无设计偏离；focused manifest测试2/2、all-features library check、Clippy `-D warnings`与diff-check通过。完整NoIndexed/max-manifest fixture留待后续集成验收；生产Web route继续disarmed，native Viewer/API和Rerun服务端行为不变。

### [x] MCAP-033 — 建立 Fetch completion queue、CPU driver 与 Phase A 性能基线

- 建议提交：`Add bounded remote MCAP CPU work driver`。
- 依赖：MCAP-013、MCAP-015、MCAP-022、MCAP-024、MCAP-025、MCAP-030、MCAP-031、MCAP-032。
- 变更：Fetch callback只入队raw bytes或matching retry completion并请求repaint，local harness的`drive_cpu`每次最多执行一个retry admission/opening/index/validation/decode work unit，并记录release-Wasm duration/bytes/count。
- 变更：实现唯一production body-owner adapter，把matching full-record `re_web::ExactLengthRangeBody`与MCAP-025已签发的pending lease一起交给MCAP-025 sealed header validator，再通过header-validated owner的borrowed exact payload view与one-shot install零拷贝转移为exact-sized `Box<[u8]>`，或在无法转移时先预留source body与destination同时存活的overlap peak后执行一次明确copy；adapter不能取得裸payload range/length、解析后重报CRC/codec metadata，也不能签发、复制或重新生成physical read identity。
- 变更：adapter在header前、copy/transfer后、MCAP-024后以及入ready queue、scanner完成和cache/result安装前校验source/read generation、canonical ordinal、expected full range、attempt token与budget profile；任一stale/mismatch都使每个bytes backing先于其accounting permit释放且lease最后释放，不进入下一sealed state或publication。
- 验收：retry completion只能产生`RetryPending`且下一driver turn至多启动一个attempt；同一Chunk两阶段不在同一allowance串联，合法查询与upstream indexed reader差分一致，BYOB、opening parse、validation、dispatch、MessageIndex五类路径在待封印hard limits内满足主线程阈值；不满足时阻断GA并记录Worker RFC候选。
- 验收：zero-copy与copy策略必须在canonical Phase A profile中冻结一种；copy路径报告并限制full response、destination和permit的同时峰值；header前wrong/stale token、generation、ordinal、range或profile使destination allocation为零，copy后stale revalidation使MCAP-024、scanner和cache publication为零并释放destination，caller CRC/codec metadata入口在compile/API边界不存在。
- 负责人：kola；提交：本提交；备注：2026-08-14 完成production-disarmed的Web remote-MCAP Fetch completion与单work-unit CPU driver：Range completion由同源move-only attempt authority签发，per-operation settlement-once、retry与迟到completion不会影响其他ordinal/range；input/output permit以RAII覆盖执行、ready和真实result owner生命周期，typed budget profile与全部safe point精确绑定，ready容量不足时原位保序park。唯一上层adapter位于re_viewer，re_mcap与re_web不新增直接依赖；validation ready通过one-shot phase token在后续独立turn执行dispatch。Phase A证明使用独立attested cfg，在真实web-release+wasm-opt artifact上经controlled Range、production BYOB、Viewer adapter/driver及025A→030→031 typed pipeline执行五阶段，固定2次warmup与5次sample并报告max；byte-only instantaneous ledger记录实际同时存活峰值，evidence绑定JS/Wasm/fixture SHA-256、git commit与Chrome版本，server严格校验、有界单次接收并由CI上传完整证明产物。Dafee三轮审查闭合全部High/Medium后最终为0 High/0 Medium/0 Low且无设计偏离；driver9/9、Phase A chain3/3、Web evidence/server focused tests、proof all-target Clippy `-D warnings`、rustfmt与diff-check通过。本机release-Wasm/Chrome因缺clang、Chrome/Chromedriver未执行，native re_viewer门限因既有libudev.pc缺失被环境阻断；production route继续disarmed，native Viewer/API/runtime与Rerun服务端行为不变。

## 7. M3 — Store、residency 与 subscriber 基础

### [x] MCAP-034 — 持久化 external-refetchable root origin

- 建议提交：`Add persistent refetchable root descriptors`。
- 依赖：MCAP-006、MCAP-029。
- 变更：增加只由 Web remote-MCAP Store config/capability 可构造的稳定 root origin、exact root existence query、refetch capability 和 descriptor identity。
- 验收：root 在插入、GC 删除、重新 Fetch 和重载后保持相同 identity，representation terminal 后 capability 可先撤销，native 与普通 volatile/manifest root 语义不变。
- 负责人：kola；提交：本提交；备注：2026-08-14 完成production-disarmed的Web remote-MCAP external-refetchable root origin：Store-scoped move-only capability签发稳定descriptor identity、exact Resident/Unloaded query与one-shot refetch permit；origin跨deep GC保留，显式revoke或capability Drop撤销所有未消费permit但不删除origin。same StoreId跨实例、clone authority、ordinary insert绕过permit和RRD manifest identity collision均在mutation/event前fail-closed；普通Web/RRD路径只增加O(1) token判断，native production通过cfg完全隔离。Dafee三轮最终0 High/0 Medium/0 Low且无设计偏离；focused6/6、re_chunk_store lib64/64、all-features lib/tests Clippy `-D warnings`、check、rustfmt与diff-check通过，production route继续disarmed。

### [x] MCAP-035 — 实现 partition/root 原子 registration 与 residency

- 建议提交：`Add atomic remote partition registration`。
- 依赖：MCAP-032、MCAP-034。
- 变更：为 Web remote-MCAP 专用 Store 增加 capability-gated `PartitionResidency`、session counters、`CompleteEmpty`、root descriptor 和 external-origin bytes 的整批 preflight/commit，既有 `EntityDb` constructor 和 native mutation API 不变。
- 验收：任一 cap 或 identity conflict 时 registry 逐位不变；合法 manifest 全量 registration 不在中途失败；Store event 是 root resident/nonresident 的唯一真源。
- 负责人：kola；提交：本提交；备注：2026-08-14 完成production-disarmed的Web remote-MCAP partition/root原子registration与residency：sealed OpeningStatic/CompleteEmpty authority、四项hard caps、index/ChunkStore lineage/temporary delta统一reservation及Drop退款；整批preflight完成后才执行Store reserve与无失败commit，任一cap、identity、kind、排序或origin冲突均保持registry、lineage、counters、bytes与events逐位不变。Index绑定opaque Store instance；同StoreId foreign event只能触发bound Store exact existence reconciliation，不能污染状态；delta maps与sorted root binary search保证线性期望复杂度。Dafee四轮最终0 High/0 Medium/0 Low且无设计偏离；residency11/11、manifest3/3、dispatch3/3、re_chunk_store64/64、两crate all-features Clippy `-D warnings`/check、cargo fmt与diff-check通过；本机无cargo-nextest，使用cargo test兜底，production route继续disarmed。

### [x] MCAP-036 — 增量维护 indexed extent 与 loaded coverage

- 建议提交：`Track remote indexed and loaded time coverage`。
- 依赖：MCAP-032、MCAP-035。
- 变更：分离 immutable indexed extent、source-unit satisfied counters、loaded interval coverage 和完整 indexed-extent coverage helper，不修改 `EntityDb::time_range_for`。
- 验收：重叠 roots、CompleteEmpty、GC 删除、reload 和 NoIndexedMessages 均产生正确 coverage，查询不会每帧重建 Chunk × group cross-product。
- 负责人：kola；提交：本提交；备注：2026-08-14 完成production-disarmed的incremental indexed/loaded coverage：immutable `CanonicalIndexedExtent`区分Known与NoIndexedMessages，closed-endpoint coverage cells、per-source selected-group satisfied counters与cached loaded ranges/Complete helper增量维护overlap、gap、CompleteEmpty、GC删除和reload。Coverage retained/temporary逐allocation复用锁定release-Wasm dlmalloc footprint，并在首次allocation前从同一budget组合reserve、RAII退款；Store event按ChunkId只reconcile affected partitions，每batch最多重建一次，getter只读缓存，不修改`EntityDb::time_range_for`。Dafee二轮最终0 High/0 Medium/1 Low且无设计偏离；唯一Low为尚缺真实resolved-source production-builder fixture。coverage8/8、residency16/16、manifest3/3、dispatch3/3、re_chunk_store64/64、两crate all-features Clippy `-D warnings`/check、cargo fmt与diff-check通过；完整re_mcap的5个既有失败位于未修改区域，production route继续disarmed。

### [x] MCAP-037 — 冻结 remote Store 的 deterministic insertion 配置

- 建议提交：`Add bounded compaction-free remote insertion path`。
- 依赖：MCAP-031、MCAP-035。
- 变更：只有 Web remote-MCAP Store 使用 `ChunkStoreConfig::COMPACTION_DISABLED`，decoder 在 insertion 前确定性预分片，并在写入前检查 root/row/output physical bytes 上限；native Store config 不变。
- 验收：实际 insertion 不运行 compaction candidate election；空 Store、cap-state、相同 start time 和 unsorted timeline fixture 的同步 add/index/event/cache propagation 都在阈值内。
- 负责人：kola；提交：本提交；备注：2026-08-14 完成production-disarmed的deterministic remote insertion contract：sealed/versioned derived-chunk profile由immutable channel-group/manifest authority冻结，profile digest进入partition/root identity；Web remote Store固定`COMPACTION_DISABLED`，stable output ordinal/root ID与root/row/output/component/timeline hard caps在写入前校验。Production只接受已满足frozen root limits的prebuilt Chunk；仍需deep-copy split时在任何复制、发布或Store mutation前返回typed `DeepSplitUnavailable`，不使用无法证明的OOM probe。Capability-bound EntityDb add复用既有同步Store event/index/query-cache传播；真实fixture覆盖dispatch→registration→EntityDb latest-at E2E、same-start与unsorted timeline，native Store config不变。Dafee三轮最终0 High/0 Medium/0 Low且无设计偏离；deterministic6/6、dispatch3/3、residency16/16、manifest3/3、terminal3/3、full-chain E2E、EntityDb测试、re_chunk_store65/65、三crate Clippy `-D warnings`与default/all-features check、diff-check通过。裸wasm check受仓库getrandom wasm_js环境配置阻断，本机无pixi/taplo；production route继续disarmed。

### [~] MCAP-038 — 抽取 checked DataSourceMessage apply helper

- 范围调整：本项仅服务 legacy/LogChannel 通用 apply，不实施；以下原始拆分仅供追溯。

- 建议提交：`Add checked Viewer data-source message application`。
- 依赖：MCAP-004。
- 变更：从现有 `receive_messages` 抽取普通 receiver 与 Web typed path 共用的 `apply_data_source_message`，把 Store mutation 和同步 Viewer bookkeeping 的完整结果返回调用方。
- 验收：成功路径对 Store、UI、route、cache 和 recording behavior 与旧路径差分一致；任一失败不再 log-and-continue，也不会在调用方确认 outcome 前构造后续 query context。
- 负责人：TBD；提交：TBD；备注：TBD。

### [x] MCAP-039 — 引入 PurgeOutcome 与 pending remote reclaim

- 建议提交：`Represent asynchronous remote Store reclaim`。
- 依赖：MCAP-004、MCAP-006。
- 变更：在 Wasm remote-MCAP memory controller 增加 `freed_now` 与 `PendingRemoteReclaim::{Gc,Close}`、request ID、去重、GC-to-Close supersede 和完成后重采样；不改变 native StoreHub purge 签名或运行时语义。
- 验收：pending GC/close 预计量不计入同步释放，pending 期间 remote growth 暂停，重复提交不创建新 work，只有 matching completion 和新采样可以恢复 admission。
- 负责人：William；提交：本提交；备注：2026-08-16 完成production-disarmed的Wasm remote-MCAP memory-pressure reclaim controller：增加`PurgeOutcomeV1`、`PendingRemoteReclaimV1::{Gc,Close}`、单调request ID、同ticket去重、GC-to-Close supersede、stale/duplicate/mismatch completion过滤、完成后必须等待更新的全进程memory sample才恢复remote growth admission。App仅在Wasm memory purge采样点更新disarmed controller，不改变native `StoreHub::purge_fraction_of_ram`签名或运行时语义，也不拦截现有Redap/普通Web prefetch。Dafee增量review未发现中级及以上问题或设计偏离；focused `web_remote_mcap_memory` 8/8通过，rustfmt通过；本机无pixi/cargo-nextest，使用`cargo test`兜底。裸wasm check仍被已提交`web_remote_mcap_cpu.rs`未使用lifetime错误阻断，非本项增量；production route继续disarmed，native Viewer/API/runtime、既有Web compatibility行为和Rerun服务端行为不变。

### [~] MCAP-040 — 实现真正 detached ChunkStore/EntityDb

- 范围调整：本项为 detached Redap publication 基础，不实施；remote MCAP 使用 presentation gate 与专用 Store 路径。

- 建议提交：`Add detached Store subscriber mode`。
- 依赖：MCAP-011。
- 变更：增加 detached constructor/seal capability，detached mutation 和 Drop 都不注册、通知或清理 global/per-store subscriber，也不暴露 query/storage handle。
- 验收：`enable_changelog=false` 不被当作 detached；同 StoreId 的 attached old Store 存活时创建、修改和 Drop detached Store，旧 topology/query subscriber state 逐位不变。
- 负责人：TBD；提交：TBD；备注：TBD。

### [~] MCAP-041 — 让 subscribers 支持 generation-aware bootstrap 与 swap

- 范围调整：本项为 detached Redap swap 服务，不实施；既有 native/global subscriber 语义不变。

- 建议提交：`Make ChunkStore subscribers generation-aware`。
- 依赖：MCAP-011、MCAP-040。
- 变更：`re_query`、`re_view_spatial` 和其他 subscribers 按 `StorePublicationIdentity` 分区，支持从 sealed snapshot 准备有界 bootstrap、不可失败 attach/remove-generation 和 authority revoke。
- 验收：same-StoreId 新 generation attach 前无 public effect，swap 后旧 query/cache/subscriber result stale-drop，普通 attached Store 的 mutation/Drop 行为由差分测试保持不变。
- 负责人：TBD；提交：TBD；备注：TBD。

### [x] MCAP-042 — 建立 Web existing-identifier lookup index

- 建议提交：`Add bounded Web existing identifier catalogs`。
- 依赖：MCAP-006、MCAP-011。
- 变更：为 Web `recording_id` 兼容控制增加 bounded lookup index，key 只引用已存在的 recording identity，并在 store add/remove/close 时同步维护；native query API 和兼容 raw-ID control 不变。
- 验收：相同 `recording_id` 的多 Store 仍有稳定且可回收的兼容查找；删除后回落到仍存活的同名 Store；native query API 和 compatibility raw-ID control 不变。
- 负责人：William；提交：`Add web remote MCAP reclaim controller` 之后的增量；备注：根据产品选择 1，MCAP-042 以 recording_id compatibility MVP 收口，timeline/entity-path/component catalog 留待后续设计/取舍。

## 8. M4 — Open ownership、strict handoff 与 public lifecycle

### [x] MCAP-043 — 实现 source status owner、terminal registry 与 first-cause latch

- 建议提交：`Add bounded open-source terminal ownership`。
- 依赖：MCAP-004、MCAP-006、MCAP-025A。
- 变更：增加 `OpenSourceToken`、`OpenSourceStatusOwner`、固定容量/字节 terminal registry、oldest eviction、bounded diagnostics、terminal revision 和 first-terminal-cause-wins latch。
- 变更：为matching remote opening token增加one-shot sealed object-capability issuer authority；它只能在后续probe产生完整typed `BoundRemoteObject`后签发MCAP-025A capability，不能接受分离的session/content-length/consistency/validator scalars，也不能由native、nonremote route或Rerun服务端取得。
- 验收：pre-claim、opening、active failure 和 administrative cleanup 都恰好终态化一次；fatal cause 不被 explicit/pressure/background close 覆盖；高水位和 eviction 可观测且脱敏。
- 验收：wrong/stale/reused opening token、重复签发、字段拆分重组和status owner终态化后签发均失败；025A仍不反向依赖043，dependency/API测试证明不存在cycle且production-disarmed issuer在本项接通前不可达。
- 负责人：William；提交：`0f69654e9c`；备注：Web-only foundation 已落地，未改 native viewer 和现有 compatibility open/close。

### [x] MCAP-044 — 实现 remote slot、client registry 与 close-once

- 建议提交：`Add token-checked remote MCAP client registry`。
- 依赖：MCAP-043。
- 变更：实现 `Vacant → Opening → Active → Closing → Vacant`、opening handle、client guard、registry owner token、owned client entry 和 `RemoteCloseOnce`，Active 不自持 guard。
- 验收：claim 不跨 await，stale activation/close 不影响新 token，guard Drop 可触发 close，registry-driven cleanup 在 `Vacant` 前终结 owner并删除 client entry，stop 不依赖 frame-driven Closing。
- 负责人：William；提交：本地已提交；备注：Web-only registry foundation 已落地，未改 native viewer 和现有 compatibility open/close。

### [x] MCAP-045 — 实现 secret fingerprint multimap 与 semantic reuse

- 建议提交：`Add secret-safe source reuse classification`。
- 依赖：MCAP-007、MCAP-008、MCAP-043。
- 变更：用 instance-keyed HMAC fingerprint加 exact canonical match建立 bounded multimap，并为 remote 比较 Topic filter、decoder/assignment versions、time type、requested policy 和 actual consistency。
- 验收：相同 URL兼容配置 alias，字段任一不同返回 `ExistingSourceOptionsConflict`，different URL 走 slot limit，规范化等价、不同 query、extensionless pending、strong-versus-assumed 和注入 digest collision 均有确定结果；canonical secret removal 后 zeroize。
- 负责人：William；提交：本地已提交；备注：Web-only 分类与复用基础件已落地，未改 native viewer 或现有 compatibility open 行为。

### [x] MCAP-046 — 把 extensionless sniff 接入 bounded pending registry

- 建议提交：`Add cancellable pending format-sniff registry`。
- 依赖：MCAP-016、MCAP-043、MCAP-044、MCAP-045。
- 变更：只为 strict operation 实现 prepared/sniffing/confirmed-remote/unsupported states、独立 cap、status owner move、close(url)/guard/stop cancellation 和不跨 await 的 remote ownership transfer；compatibility 无扩展名 URL 不登记 pending sniff。
- 验收：header 前、magic 前和 remote claim 前 close 都收敛；stale completion不能占slot；strict非MCAP结果在零importer/receiver ownership下返回 `UnsupportedFormat`；compatibility无扩展名URL始终直接进入原dispatcher，不创建pending entry或额外Fetch。
- 负责人：William；提交：本地已提交；备注：保守实现为 strict-only pending registry foundation，未接入 compatibility open 或 native viewer。

### [x] MCAP-047 — 分离 source、operation 与 exact recording registries

- 建议提交：`Add separate source operation and recording lifecycles`。
- 依赖：MCAP-006、MCAP-011、MCAP-043。
- 变更：增加 `OpenOperationToken/PublicOpenRequestId`、`PublicRecordingHandleId`、source fanout、operation-local recording subscriptions、exact Store reverse mapping 和 bounded removed tombstones。
- 验收：每次 open 都有独立 operation；compatible alias只共享 source；相同 RecordingId 的 Stores得到不同 public identity；最后一个显式 subscriber dispose 后确定删除 tombstone，不依赖 GC。
- 负责人：William；提交：本地已提交；备注：Web-only registry foundation 已落地，未接入 native viewer 或现有 compatibility open 行为。

### [x] MCAP-048 — 实现唯一 public lifecycle sequencer

- 建议提交：`Implement normative public open lifecycle sequencing`。
- 依赖：MCAP-047。
- 变更：按设计唯一表实现 accepted、recording_activated、behavior effect、request_ready、presentation_ready、terminal 和 recording_removed 的有序 transition及 typed reason。
- 验收：remote new/Active/terminal alias replay、pre-Store failure、post-activation failure、各种 close、pressure/background 和 stop-no-removed 都只有唯一合法 trace；wrapper installation 先于 ready；既有 non-MCAP `recording_open` trace 不变。
- 负责人：William；提交：本地已提交；备注：Web-only sequencer foundation 已落地，未接入 TypeScript dispatcher 或现有 non-MCAP `recording_open` 路径。

### [x] MCAP-049 — 实现 Rust lifecycle delivery permits

- 建议提交：`Add bounded lifecycle delivery permits and acknowledgements`。
- 依赖：MCAP-006、MCAP-048。
- 变更：增加 `RustQueued/DeliveredToTypeScript` ownership、count/byte permit、delivery ack/cancel、每项至多一个 aggregate listener-error credit 和 stop settlement hook。
- 验收：TypeScript真实 dispatch 或 cancel ack 前 Rust 不归还容量；queue full不丢/coalesce transition；dispose、hidden、stop和迟到 ack 都按 token 收敛。
- 负责人：William；提交：本地已提交；备注：Web-only delivery permit foundation 已落地，未接入 TypeScript dispatcher 或现有 Viewer event dispatcher。

### [ ] MCAP-050 — 实现 HTTP-only strict batch prepare transaction

- 建议提交：`Prepare atomic strict HTTP open batches`。
- 依赖：MCAP-006、MCAP-043、MCAP-044、MCAP-046、MCAP-047。
- 变更：一次 registry transaction 解析/验证全部 typed remote-MCAP HTTP specs，prepare known-remote/sniff source、status、operation、wrapper/Promise reservation 和 deferred terminal future claim，但不启动 work。
- 验收：任一 invalid shape/options/format/route/cap/slot/setup failure 整批零 owner、零 Fetch、零 connection且 terminal registry snapshot不变；non-HTTP、gRPC、Redap 或确认非 MCAP 的 strict 输入返回 typed unsupported error，不转交现有 route。
- 负责人：TBD；提交：TBD；备注：TBD。

### [ ] MCAP-051 — 实现 ordinary strict disarmed handoff release/abort

- 建议提交：`Commit and release disarmed strict open handoffs`。
- 依赖：MCAP-010、MCAP-049、MCAP-050。
- 变更：infallible commit只安装 disarmed ownership；matching release验证完整 installation acks、registry revision/victims和page idle后，同一 mutation执行terminal eviction、claim conversion、accepted publish和route arm；abort撤销新 owner或只detach existing alias。
- 验收：wrapper throw、invalid/duplicate/missing ack、wrong/repeated token、release前stop和state change保持历史registry逐位不变；release前零 event/replay/work；abort绝不关闭共享 source。
- 负责人：TBD；提交：TBD；备注：TBD。

### [ ] MCAP-052 — 实现 TypeScript operation/recording wrapper cache

- 建议提交：`Add strict open operation and recording wrappers`。
- 依赖：MCAP-010、MCAP-047、MCAP-051。
- 变更：实现 wrapper construction、preexisting recording attachment、complete installation ack、internal activation bridge和operation-local稳定对象 cache，不执行用户 callback。
- 验收：重复读取 `recordings` 不增加 wrapper/subscriber/Promise且 object identity 稳定；constructor throw先 matching abort并dispose临时对象；Active/completed alias replay在public dispatch前已装好wrapper。
- 负责人：TBD；提交：TBD；备注：TBD。

### [ ] MCAP-053 — 实现 instance-owned TypeScript dispatcher

- 建议提交：`Add bounded single-task Web Viewer event dispatcher`。
- 依赖：MCAP-049、MCAP-052。
- 变更：为新 strict remote-MCAP lifecycle/Promise/event-error 建立最多挂一个 browser task 的 bounded FIFO dispatcher；不迁移 existing `recording_open`、notebook/Gradio raw event 或其他 compatibility event bridge。
- 验收：public method return/start resolve前不执行 remote host callback；不让出event loop的open/dispose循环仍有界；普通listener错误最多一个预留通知；error hook抛错只reportError不递归；stop取消新队列，既有raw-event同步时序不变。
- 负责人：TBD；提交：TBD；备注：TBD。

### [ ] MCAP-054 — 在 compatibility open/start 中接入 remote-MCAP 分支

- 建议提交：`Preserve compatibility open route semantics`。
- 依赖：MCAP-007、MCAP-044、MCAP-046、MCAP-048。
- 变更：`open/start(string|string[])`继续逐项、non-throwing warning/continue；只有由明确受支持的 `.mcap` URL route 识别的 remote MCAP HTTP item 进入 remote singleton helper；无扩展名URL和其余HTTP、RRD、gRPC、Redap不经过新registry并继续原 `ViewerOpenUrl` dispatcher。
- 验收：第二项malformed不回滚第一项也不stop；明确 `.mcap` URL竞争按逐项slot语义；无扩展名和其他non-MCAP routes的request/receiver/connection/selection/error side effects与MCAP-001完全一致；不存在compatibility format probe或统一全局ingress queue。
- 负责人：TBD；提交：TBD；备注：TBD。

### [ ] MCAP-055 — 发布 strict openRequest/openBatch API

- 建议提交：`Expose strict HTTP open request APIs`。
- 依赖：MCAP-051、MCAP-052、MCAP-053。
- 变更：在 Rust/Wasm/TypeScript 发布 remote-MCAP-only `openRequest/openBatch`、typed options、structured request-local errors和输入顺序稳定的 operation handles。
- 验收：request-local错误不调用 `#fail/stop`；atomic batch只在matching release后返回全部handles；async opening failure通过handle lifecycle；任何payload均脱敏且不暴露 StoreId/RecordingId/token。
- 负责人：TBD；提交：TBD；备注：TBD。

### [ ] MCAP-056 — 实现 exact recording control、close 与 dispose

- 建议提交：`Add exact public recording controls and disposal`。
- 依赖：MCAP-047、MCAP-048、MCAP-052、MCAP-055。
- 变更：新增基于 `PublicRecordingHandleId` 的 select/seek/play/close和structured result，operation/recording `dispose()`与close正交，FinalizationRegistry只调用同一best-effort token。
- 验收：相同 RecordingId 不歧义；removed/stopped/disposed handle不重定向；recording close只控制exact Store，operation close控制source范围；dispose释放subscriptions/Promise/cache/tombstone且不关闭资源。
- 负责人：TBD；提交：TBD；备注：TBD。

### [ ] MCAP-057 — 实现同步 Viewer instance teardown

- 建议提交：`Add synchronous Web Viewer ownership teardown`。
- 依赖：MCAP-043、MCAP-044、MCAP-046、MCAP-049、MCAP-053。
- 变更：`stop/destroy`先使instance token失效，再同步取消新 remote dispatcher、Fetch/future/callback、pending sniff、handoff、client、remote Store、secret和reservation，不接管非 MCAP receiver/connection 的现有 teardown。
- 验收：Opening、active Fetch、CommitLocked、GC lease-drain、Closing 和 release前handoff均不等待下一帧；未settle Promise得到viewer-stopped；不发布removed、不调用listener；迟到JS completion安全丢弃。
- 负责人：TBD；提交：TBD；备注：TBD。

## 9. M5 — Cancellable legacy HTTP importer

### [~] MCAP-058 — 增加 slot-generation legacy data/control envelopes

- 范围调整：MCAP-058 至 MCAP-066 的 legacy adapter 全部移出，既有 legacy HTTP importer 保持原路径。

- 建议提交：`Add sequenced legacy receiver envelopes`。
- 依赖：MCAP-006、MCAP-038。
- 变更：在 `re_log_channel` 增加 prepared slot ID/generation、所有 cloned senders共享sequencer/capacity gate、bounded data envelope、reserved EOF/ProducerFailed control channel和matching ack sink。
- 验收：clone sender序列唯一单调，两个相同 LogSource label可区分，queue full返回capacity pending而非failure，EOF不被data占满阻止，slot reuse/stale envelope/ack不影响新generation。
- 负责人：TBD；提交：TBD；备注：TBD。

### [~] MCAP-059 — 实现 Web-only pull/resumable legacy decoder

- 范围调整：不实施；不改写 legacy decoder ABI 或背压语义。

- 建议提交：`Add resumable Web legacy decoder cursor`。
- 依赖：MCAP-058。
- 变更：`re_log_encoding/re_sorbet`增加一次最多产出一个 message 的 pull cursor，支持 NeedInput/NeedOutputCapacity/NeedInternAdmission/Finished并保留current input offset/state。
- 验收：单HTTP chunk大量小message、queue capacity=1、长时间无frame和capacity wait abort都不丢位置、不重复输出、不继续读body；native同步decoder ABI不变。
- 负责人：TBD；提交：TBD；备注：TBD。

### [~] MCAP-060 — 实现 prepare-zero-work legacy HTTP adapter

- 范围调整：不实施；strict API 的非 MCAP sniff 结果返回 `UnsupportedFormat`。

- 建议提交：`Add deferred cancellable legacy HTTP adapter`。
- 依赖：MCAP-015、MCAP-043、MCAP-058、MCAP-059。
- 变更：在 `re_data_source` 增加预创建channel/status/AbortController/task owner、deferred `start()`、body-reader backpressure和completion-once callback，prepare/rollback不发Fetch。
- 验收：strict release或compatibility publish后才启动；close/stop能唤醒body read/capacity wait并abort；network/importer success/failure/close/teardown只终态化一次；非Web importer语义不变。
- 负责人：TBD；提交：TBD；备注：TBD。

### [~] MCAP-061 — 建立 active legacy ownership 与 frame budgets

- 范围调整：不实施；legacy ownership 不纳入 remote-MCAP frame driver。

- 建议提交：`Drive bounded legacy import ownership in Viewer`。
- 依赖：MCAP-004、MCAP-058、MCAP-060。
- 变更：Viewer registry不可拆分地持有receiver/ack/drain/producer，按每帧message/byte/time allowance poll producer与apply，并将slot映射到source token。
- 验收：hidden/close/stop取消可收敛，data queue满只背压，control EOF可冻结watermark，交错sources的ack不会串线，active ownership Drop前必须显式 completion。
- 负责人：TBD；提交：TBD；备注：TBD。

### [~] MCAP-062 — 实现 LegacyApplyAck 与 EOF drain barrier

- 范围调整：不实施；不修改 legacy message/apply 协议。

- 建议提交：`Add typed legacy apply acknowledgements`。
- 依赖：MCAP-038、MCAP-058、MCAP-061。
- 变更：每个message apply返回matching slot/source/sequence的success或typed mutation/bookkeeping failure，只有连续successful watermark达到EOF frozen sequence才完成。
- 验收：SetStoreInfo→valid Arrow→failing Arrow→EOF不completed；跳号/duplicate/stale ack不能推进；apply failure在同帧任何query context前first-wins terminalize并触发quarantine。
- 负责人：TBD；提交：TBD；备注：TBD。

### [~] MCAP-063 — 实现 legacy 多 Store/public recording binding

- 范围调整：不实施；不新增 legacy source/public handle 绑定。

- 建议提交：`Track legacy source recordings independently`。
- 依赖：MCAP-047、MCAP-048、MCAP-061、MCAP-062。
- 变更：SetStoreInfo按exact Store发布activation，每operation独立兑现behavior，EOF drain后逐Store发布presentation-ready，再发布source completed；task owner与bounded Store binding分离。
- 验收：零/单/多Store、相同RecordingId、首Store关闭后控制第二Store、completed alias replay和连续成功导入超过terminal registry cap都闭合；ready不错误代表data ready。
- 负责人：TBD；提交：TBD；备注：TBD。

### [~] MCAP-064 — 统一 legacy Store removal cause 与 close scope

- 范围调整：不实施；既有 recording-panel legacy close 语义不变。

- 建议提交：`Route legacy Store removals through source ownership`。
- 依赖：MCAP-057、MCAP-060、MCAP-063。
- 变更：recording panel、operation handle、recording handle、URL、memory pressure、background和internal eviction携带typed cause；live source close先completion-once abort/disconnect，再按范围关闭Stores。
- 验收：panel/operation/URL/pressure live close关闭同source全部Stores，recording handle在task terminal后只关exact Store，内部一致的eviction只更新binding；迟到SetStoreInfo/Arrow不能重新安装Store。
- 负责人：TBD；提交：TBD；备注：TBD。

### [~] MCAP-065 — 在 apply failure 后 quarantine 并移除全部 legacy Stores

- 范围调整：不实施；本项目不接管 legacy failure/quarantine。

- 建议提交：`Quarantine failed legacy imports before queries`。
- 依赖：MCAP-062、MCAP-063、MCAP-064。
- 变更：任一 `LegacyApplyAck::Err`撤销route/selection/query/control/reuse authority，发布failure terminal并在有限帧内自动移除shared source全部Stores，保留仅供dispose的removed tombstone。
- 验收：mutation或bookkeeping部分执行后query invocation为零，Store有限帧移除，新alias不重放失败Store，绝不completed，task/drain/binding资源全部释放。
- 负责人：TBD；提交：TBD；备注：TBD。

### [~] MCAP-066 — 完成 legacy adapter 端到端 contract suite

- 范围调整：不实施；只保留 MCAP-001 的现有 legacy characterization 防回归。

- 建议提交：`Add legacy HTTP lifecycle contract tests`。
- 依赖：MCAP-058 至 MCAP-065。
- 变更：补齐known legacy与extensionless handoff的atomic prepare、backpressure、EOF readiness、multi-Store、各种close、apply failure和迟到callback集成测试，并把adapter接入compatibility/strict HTTP routes。
- 验收：remote+legacy batch后项prepare failure零Fetch；capacity=1/无frame/full-data-queue EOF有限收敛；所有 terminal race恰好一次；existing importer/native behavior差分不变。
- 负责人：TBD；提交：TBD；备注：TBD。

## 10. M6 — Remote-MCAP page execution 与 Web teardown

### [ ] MCAP-067 — 建立 ChromePageExecutionState 与 lifecycle listener

- 建议提交：`Add Chrome page execution state machine`。
- 依赖：MCAP-004、MCAP-015、MCAP-057。
- 变更：在remote-MCAP manager内实现VisibleRunning、HiddenSuspended、VisibleRevalidating、execution epoch、source-scoped active-visible deadline和generation-checked visibility/pagehide/pageshow/freeze/resume signal，并把MCAP-015 abstract metadata retry owner绑定到fresh page epoch/baseline；不停止Viewer frame driver或其他data source。
- 验收：初始状态可注入测试；hidden暂停remote Fetch admission、`RetryPending` eligibility、CPU deadline和playback dt且不burnattempt/Range；resume首帧丢弃旧dt、one-shot rebind retry baseline并请求repaint；重复/迟到signal不重复transition；LogChannel/gRPC/Redap仍由既有系统驱动。
- 负责人：TBD；提交：TBD；备注：TBD。

### [ ] MCAP-068 — 把跨帧 insertion/GC 移入 page-hidden suspended ownership

- 建议提交：`Preserve mutation ownership across page suspension`。
- 依赖：MCAP-067。
- 变更：定义 remote-MCAP insertion/GC safe points、frozen commit set、facade snapshot、pins/reservation/effects和one-shot resume nonce，hidden时move ownership而非stale-drop，不改变 native 或普通 Store mutation scheduler。
- 验收：所有safe point hide/resume都只由page-control rebind一次；部分物理写入/删除后revalidation失败进入poison/cleanup，不回滚、不重开facade、不重复ack。
- 负责人：TBD；提交：TBD；备注：TBD。

### [ ] MCAP-069 — 实现 WebExternalStringIngressLimitsV1

- 建议提交：`Bound external Web strings before Wasm materialization`。
- 依赖：MCAP-005、MCAP-006、MCAP-042。
- 变更：对 strict remote-MCAP options、Topic filter、decoder selector、exact-recording timeline/control 字符串执行 TypeScript UTF-16 cap、raw Wasm opaque JS-string brand/length检查、combined copy permit和Rust exact UTF-8/object-graph reservation。
- 验收：stopped/closed路径不观察或格式化caller field；huge/coercion-object输入在wasm-bindgen copy前拒绝；失败不构造TimelineName、Topic/map key或remote command；现有 compatibility recording control、LogChannel 和非 MCAP raw ABI 不修改。
- 负责人：TBD；提交：TBD；备注：TBD。

### [~] MCAP-070 — 实现 per-key bounded compatibility recording target index

- 范围调整：不实施；remote MCAP 控制只使用 strict exact handle，既有 raw-ID compatibility 查找不变。

- 建议提交：`Add isolated compatibility recording target index`。
- 依赖：MCAP-011、MCAP-006。
- 变更：按raw recording ID维护per-key Complete/Unavailable、validated revision和separate new-key coverage seal，把成功解析冻结为exact Store generation。
- 验收：candidate overflow只隔离matching key；new-key capacity failure只封闭未来新key；既有complete keys继续更新；缺失sealed key不fallback StoreHub扫描；strict exact handles不受影响。
- 负责人：TBD；提交：TBD；备注：TBD。

### [~] MCAP-071 — 实现 compatibility command exhaustive normalizer

- 范围调整：不实施；不替换全局 compatibility command channel 或代数语义。

- 建议提交：`Normalize suspended compatibility commands algebraically`。
- 依赖：MCAP-069、MCAP-070。
- 变更：对全部system/UI/recording command variant穷举分类，absolute latest、toggle parity/effective state、relative checked accumulate、timeline+seek atomic bundle和external side-effect reject，并先保持raw bounded identifiers。
- 验收：双toggle、连续step/move、SetActiveTimeline+SetTime顺序和新增enum variant编译期穷举；compose/reject/no-op不增加interner，不跨target或语义barrier。
- 负责人：TBD；提交：TBD；备注：TBD。

### [~] MCAP-072 — 建立 instance-wide BrowserIngressSequence 与单一 ordered queue

- 范围调整：不实施；既有 JS API 不进入新的全局 ingress queue。

- 建议提交：`Serialize compatibility ingress causally`。
- 依赖：MCAP-054、MCAP-067、MCAP-071。
- 变更：compatibility open、navigation、system/UI/recording command共享checked sequence、count/byte-capped FIFO和唯一owner；idle visible可用同一apply helper fast path，有backlog/latch时只能append。
- 验收：hidden open A/B、visible时open C严格A/B/C；navigate→open与open→navigate冻结不同且正确的UserNavigationRevision；strict在backlog期间typed retry，sequence overflow安全拒绝。
- 负责人：TBD；提交：TBD；备注：TBD。

### [ ] MCAP-073 — 实现 remote-MCAP resume revalidation 与 steady-state slice

- 建议提交：`Revalidate remote MCAP work after page resume`。
- 依赖：MCAP-067、MCAP-068。
- 变更：resume时用execution generation重验remote slot/token、validator、body-reader、`RetryPending` operation、CPU queue、suspended mutation ownership和reservation，每帧只重启有限remote work，并为既有Viewer work保留steady-state slice。
- 验收：resume前再次hidden/close不重启remote work；旧timer/body/callback/dt全部丢弃；matching retry只用fresh baseline/epoch rebind且每turn至多一个attempt；remote backlog在有限帧恢复且不饿死现有Viewer source/query/UI；没有全局compatibility ingress prefix或command reducer。
- 负责人：TBD；提交：TBD；备注：TBD。

### [~] MCAP-074 — 把 open_channel 接入 token/generation 与 pre-copy admission

- 范围调整：MCAP-074 至 MCAP-078 的 Web LogChannel 重构全部移出，现有 LogChannel 路径不变。

- 建议提交：`Add bounded Web LogChannel ownership`。
- 依赖：MCAP-006、MCAP-038、MCAP-069。
- 变更：open同时校验id/name和完整metadata graph，返回opaque channel token/generation，预分配input/output permits与scheduler node；close同步cleanup且不依赖frame。
- 验收：duplicate id、huge id/name、metadata cap和constructor failure逐位rollback；成功前零LogSource/map/scheduler mutation；close/stop使旧token后续send确定失败。
- 负责人：TBD；提交：TBD；备注：TBD。

### [~] MCAP-075 — 为同步 LogChannel send 增加 bounded pre-copy rejection

- 范围调整：不实施；不改变现有同步 `send_rrd/send_table` 契约。

- 建议提交：`Bound legacy synchronous Web LogChannel sends`。
- 依赖：MCAP-067、MCAP-074。
- 变更：现有 `send_rrd/send_table`在copy/parse前取得page-state、input/output和copy-peak credit；hidden或无credit沿用void warning/reject，不进入over-quota channel。
- 验收：hidden flood零payload copy/decode/queue growth；超credit不会增加Wasm memory；close仍同步；该accepted safety behavior由TypeDoc/changelog占位测试标记，visible有credit行为差分不变。
- 负责人：TBD；提交：TBD；备注：TBD。

### [~] MCAP-076 — 实现消费式 fixed ArrayBuffer async send

- 范围调整：不实施；本项目不新增 LogChannel transfer API。

- 建议提交：`Add transferable Web LogChannel async sends`。
- 依赖：MCAP-074、MCAP-075。
- 变更：新增 `send_rrd_async/send_table_async`，只接受完整fixed、non-shared、attached `ArrayBuffer`，按真实backing length与JS→Wasm双allocation峰值preflight，prepared owner成功后同步transfer/detach。
- 验收：tiny-view/huge-buffer、subview、SAB、resizable/detached buffer、alias mutation和transfer failure不能绕过cap；失败不detach且source保持attached；成功立即detach所有aliases并保持sequence；stop/close settle Promise并归还permit。
- 负责人：TBD；提交：TBD；备注：TBD。

### [~] MCAP-077 — 实现 bounded Web LogChannel pull parser 与 output ack

- 范围调整：不实施；不改写 LogChannel parser/apply 路径。

- 建议提交：`Drive bounded Web LogChannel parsers and outputs`。
- 依赖：MCAP-038、MCAP-059、MCAP-076。
- 变更：RRD/Arrow不再完整collect，每poll产生有界输出；input parent permit保持到parser finished且全部child output以channel generation/input sequence/output ordinal得到checked apply outcome。
- 验收：copy、parse、query前apply各一次bounded quantum；channel-local FIFO不变；apply failure只终结matching input/channel策略；close/stop/stale output不重复apply或泄漏parent permit。
- 负责人：TBD；提交：TBD；备注：TBD。

### [~] MCAP-078 — 实现 generation-aware round-robin scheduler

- 范围调整：不实施；本项目不引入多 LogChannel 调度器。

- 建议提交：`Schedule Web LogChannels fairly`。
- 依赖：MCAP-074、MCAP-077。
- 变更：BTreeMap只存identity，预分配single-membership ready deque持久round-robin copy/parser/apply stages，blocked channel有probe cap且零progress cycle不busy-wake。
- 验收：N个continuously runnable channels中每个至多等待N−1个其他quantum；hot最小token不能饿死其他channel；hidden/close/slot reuse不产生duplicate/stale node；不承诺跨channel数据顺序。
- 负责人：TBD；提交：TBD；备注：TBD。

### [~] MCAP-079 — 冻结各 compatibility transport 的 hidden 策略

- 范围调整：不实施；gRPC/message proxy 和 Redap 的 hidden/reconnect 行为保持现状。

- 建议提交：`Define route-specific Web transport suspension policies`。
- 依赖：MCAP-043、MCAP-067。
- 变更：message proxy与Redap dataset在connect前冻结terminal-on-hidden；catalog/entry/folder只允许generation-checked幂等metadata refetch；不存在generic reconnect fallback。
- 验收：proxy hidden abort后保留已commit Store为read-only terminal并要求reopen；server history eviction不会触发无cursor reconnect；每类route有可观察typed status，native/non-Web不变。
- 负责人：TBD；提交：TBD；备注：TBD。

### [ ] MCAP-080 — 实现 pagehide/freeze 的 remote-MCAP 同步 teardown

- 建议提交：`Tear down remote MCAP owners on pagehide and freeze`。
- 依赖：MCAP-057、MCAP-067、MCAP-068、MCAP-073。
- 变更：`pagehide` 和 Chrome freeze 同步终结全部 remote-MCAP token/Fetch/CPU/mutation/query owners，pageshow/resume不复活旧 remote token；不销毁整个 Viewer、canvas、非 MCAP receiver 或它们的 handler。
- 验收：BFCache stale remote callback 全部拒绝，remote Store/facade/secret/reservation 无残留；Viewer 和既有 non-MCAP recordings 保持当前产品行为；恢复后调用方可显式重新 open remote MCAP。
- 负责人：TBD；提交：TBD；备注：TBD。

## 11. M7 — Remote-MCAP runtime identifiers

### [ ] MCAP-081 — 实现 remote bounded raw identifier census 与 domain token

- 建议提交：`Add raw identifier census before remote MCAP decoding`。
- 依赖：MCAP-012。
- 变更：为remote-MCAP Summary/Chunk decoder定义bounded raw timeline/entity/component/path census、canonical去重、retained-byte ownership和只能由MCAP-012 transaction兑现的domain construction token。
- 验收：codec在census前不能构造domain identifier；count/byte/canonicalization failure零intern与Store mutation；field-private census不能伪造超limit内容，任何codec不能拿到半批domain identifiers。
- 负责人：TBD；提交：TBD；备注：TBD。

### [~] MCAP-082 — 把 intern barrier 接入 LogChannel 与 legacy

- 范围调整：产品明确排除；runtime intern budget只约束remote-MCAP transaction，LogChannel与legacy继续使用existing global constructors并且不因remote budget exhaustion失败。

- 建议提交：`Budget runtime identifiers in Web file decoders`。
- 依赖：MCAP-059、MCAP-077、MCAP-081。
- 变更：RRD/Arrow schema和legacy message先输出raw census，NeedInternAdmission可暂停cursor，admission后才调用TimelineName/EntityPath/Component构造。
- 验收：连续apply/close唯一identifier小消息只能增长到module cap；失败只拒绝matching LogChannel input或终结legacy source，零partial Store mutation；native decoder仍走原ABI。
- 负责人：TBD；提交：TBD；备注：TBD。

### [ ] MCAP-083 — 把 intern barrier 接入 remote MCAP

- 建议提交：`Budget runtime identifiers in remote MCAP decoding`。
- 依赖：MCAP-025、MCAP-031、MCAP-081。
- 变更：remote Summary/Chunk decode产出MCAP-081 census，在manifest activation、parser construction和Store mutation前调用MCAP-012 side-map transaction；proxy、legacy、LogChannel和Redap不进入该barrier。
- 验收：remote连续独特identifier跨Viewer restart受同一Wasm-module永久budget；legacy-map existing命中零burn，prepare/commit间legacy insertion触发锁内recompute，side-map capacity growth零global-map copy；无法延迟constructor的remote codec在首次domain construction前typed reject。
- 验收：opening exhaustion零Store，active新增identifier exhaustion进入matching SessionFatal并关闭remote source，已合法burn但后续Store failure不退款；local/native MCAP与全部nonremote Web route行为差分不变。
- 负责人：TBD；提交：TBD；备注：TBD。

### [~] MCAP-084 — 构建 Redap detached PreSnapshot graph

- 范围调整：MCAP-084 至 MCAP-087 的 Redap publication 重构全部移出，现有 Redap 路径不变。

- 建议提交：`Stage Redap snapshots in detached Stores`。
- 依赖：MCAP-040、MCAP-081。
- 变更：Web dataset截获SetStoreInfo、blueprint、UI fragment、manifest parts和complete，使用stable per-attempt Store graph、canonical accumulator和effect ledger，publication前不进入普通receiver。
- 验收：PreSnapshot mutation/Drop的global subscriber callback为零；manifest只在detached graph append；blueprint StoreId在attempt内稳定；legacy no-manifest Web dataset在public mutation前typed reject。
- 负责人：TBD；提交：TBD；备注：TBD。

### [~] MCAP-085 — 实现 Redap prepared publication 与 generation swap

- 范围调整：不实施；不新增 Redap Store generation swap。

- 建议提交：`Publish Redap snapshots atomically by generation`。
- 依赖：MCAP-041、MCAP-084。
- 变更：完整manifest后seal graph，preflight subscriber/cache bootstrap和effect reservations，以fresh generation一次attach；reopen先保留old authority，成功commit时先revoke old再不可失败swap。
- 验收：staging failure/Drop保持same-StoreId old graph逐位不变；成功只执行一次manifest complete/fragment/activation；旧generation所有迟到结果stale-drop；不存在orphan Store。
- 负责人：TBD；提交：TBD；备注：TBD。

### [~] MCAP-086 — 接入 Redap hidden、read-only 与 metadata refetch 策略

- 范围调整：不实施；不改变 Redap hidden/read-only/refetch 行为。

- 建议提交：`Enforce Redap Web hidden publication policy`。
- 依赖：MCAP-079、MCAP-085。
- 变更：PreSnapshot hidden丢弃detached staging并terminal；SnapshotCommitted hidden撤销provider、保留read-only完整graph并terminal；catalog/entry/folder使用generation-checked metadata descriptor重发。
- 验收：blueprint前/中、SetStoreInfo后、manifest parts间、complete后和Chunk load中hidden都不重放prefix、不自动restart；missing Chunk query明确失败；显式Reopen产生新attempt/generation。
- 负责人：TBD；提交：TBD；备注：TBD。

### [~] MCAP-087 — 完成 Redap/intern 隔离与 native 差分测试

- 范围调整：产品明确排除Redap publication与intern迁移；Redap继续existing global constructors和现有hidden/lifecycle，native/Redap只由MCAP-001 characterization及MCAP-088/112 differential门保护不回归。

- 建议提交：`Add Redap publication and runtime intern contract tests`。
- 依赖：MCAP-082 至 MCAP-086。
- 变更：增加LogChannel、legacy、proxy、Redap和remote MCAP跨open/close/restart唯一identifier测试，以及old graph存活时staging failure、manifest repartition和hidden safe-point矩阵。
- 验收：module budget单调不退款、零partial intern；detached mutation/Drop零subscriber effect；native/non-Web Redap streaming/fallback和普通attached Store subscriber语义差分不变。
- 负责人：TBD；提交：TBD；备注：TBD。

## 12. GA — Phase A 阻断门

### [ ] MCAP-088 — 固化 Phase A release-Wasm 集成门

- 建议提交：`Add Phase A remote MCAP implementation gate`。
- 依赖：MCAP-012、MCAP-015、MCAP-026、MCAP-027、MCAP-033、MCAP-037、MCAP-039、MCAP-042、MCAP-057、MCAP-067、MCAP-073、MCAP-080、MCAP-081、MCAP-083。
- 变更：把设计第21.1节中仅与Web remote-MCAP直接相关的退出条件整理为自动化suite和CI target，包含remote retry/open/lifecycle/page/intern、bounded ROS 2/protobuf initializer、physical source authority/read lease、exact-body transfer/copy overlap、七类主线程work unit、Store insertion/GC临时harness、资源高水位报告和release artifact能力审计。
- 变更：在全部required release-Wasm与对抗evidence满足后，生成并checked-in唯一canonical V1 profile artifact，复核schema fingerprint/units/scopes/accounting/provenance，并在`re_web`的MCAP-088模块内安装唯一private production constructor与专属sealed object-binding bridge，以签发one-shot Wasm remote-MCAP production capability及首次bound-probe lower preparation authority；tooling draft、ROS 2/protobuf measurement artifact、一般validated artifact、feature cfg和test profile保持disarmed。
- 验收：release Wasm controlled Chrome中所有remote-MCAP hard limits、retry attempt/range/deadline/retained-peak、原子性、零副作用、secret、stale callback和hidden/resume断言通过；canonical artifact任何缺项或fingerprint mismatch都不能构造production capability。
- 验收：artifact symbol/dependency/API gate证明只有Web remote-MCAP root能取得one-shot production capability、MCAP-088 sealed object-binding bridge和bounded intern transaction；普通test issuer只存在于`cfg(test)`，现有ROS 2/protobuf artifact cfg、一般artifact verifier、native及全部nonremote Web routes均不能构造lower binding或取得accounting root，旧constructor仍存在且未被迁移。
- 验收：remote side-map growth的instrumentation证明legacy global map scan/copy始终为零；compatibility extensionless URL的网络trace证明没有新增8-byte probe；RRD、legacy HTTP、LogChannel、gRPC/message proxy、Redap和local/native MCAP的route、error、selection、close、hidden、identifier construction与side-effect differential全部通过。
- 验收：release artifact只能由MCAP-025 authority为matching source签发per-ordinal pending lease，只有MCAP-025 header validator能从body派生CRC/payload并生成024 input，MCAP-033 adapter没有identity或CRC/codec metadata constructor；canonical profile冻结body transfer/copy策略、simultaneous overlap peak、active lease cap和backing-before-permit Drop顺序，duplicate-live、header/descriptor、CRC与stale-generation矩阵全部通过。
- 验收：release artifact中ROS 2/protobuf assignment、manifest和decode只引用MCAP-026/027 sealed bounded results，不包含`MessageSchema::parse`、`DescriptorPool::decode`或等价remote重初始化call edge；strict subset、峰值预算、rollback和admitted-subset differential矩阵全部通过。
- 验收：此处GC只验收Phase-A临时harness与pending reclaim accounting，不替代MCAP-101最终GC集成；任何同步路径超阈值、codec无法预检、remote registry无上界、排除路由差分或release artifact能力审计失败都阻断进入M8。
- 负责人：TBD；提交：TBD；备注：TBD。

## 13. M8 — Window playback、presentation 与 GC

### [ ] MCAP-089 — 原子安装 remote manifest、Store projection 与 use state

- 建议提交：`Activate remote MCAP sessions atomically`。
- 依赖：MCAP-032、MCAP-035、MCAP-044、MCAP-088。
- 变更：`OpenedRemoteMcap`携带同一manifest `Arc`、Store projection、canonical navigation、initial gate/controller和 `Foreground/CatalogOnly/Inactive`，token-checked transition一次安装Active bundle。
- 验收：activation前不创建public Store；任何可观察Active均有完整manifest/controller identity；slot Active不等于foreground；stale activation不安装部分bundle或泄漏reservation。
- 负责人：TBD；提交：TBD；备注：TBD。

### [ ] MCAP-090 — 实现三层窗口需求与 bounded planner

- 建议提交：`Plan bounded remote MCAP window demands`。
- 依赖：MCAP-015、MCAP-032、MCAP-036、MCAP-089。
- 变更：实现presentation-required、minimum-buffer、desired-prefetch三层不相交需求，interval hits、selected groups、checked cross-product、priority和promotion规则，并为每类demand冻结独立caller phase identity及累计Range/deadline policy输入，不复用metadata-opening owner。
- 验收：重叠Chunk不漏选；satisfied partitions先剔除；desired prefetch只在matching per-ordinal lease下缓存exact full-record body；构造集合前所有count/乘法受限；尚未安装matching phase owner的demand不能启动retry；planner结果与upstream indexed selection差分一致。
- 负责人：TBD；提交：TBD；备注：TBD。

### [ ] MCAP-091 — 接入 generation partition decode、registration 与 insertion

- 建议提交：`Insert complete remote MCAP partitions`。
- 依赖：MCAP-015、MCAP-031、MCAP-035、MCAP-037、MCAP-090。
- 变更：以generation/job key驱动Fetch→validation→dispatch→staging，把matching presentation/minimum/prefetch phase owner显式交给MCAP-015 operation foundation，完整terminal batch先原子registration再由bounded insertion写Store并回执residency。
- 验收：CompleteEmpty持久化；attempt/Range/deadline exhaustion返回matching phase failure而不借用metadata counters；失败不发布partial；同partition不重复派生；stale work不能发布/ack但已发生Store event仍更新物理residency；root/session cap防御性检查不在半批失败。
- 负责人：TBD；提交：TBD；备注：TBD。

### [ ] MCAP-092 — 实现 stable root reload 与 refetch capability

- 建议提交：`Reload evicted remote roots deterministically`。
- 依赖：MCAP-015、MCAP-034、MCAP-036、MCAP-091。
- 变更：使用canonical source order、stable RowId、root descriptor、refetch-owned Range/deadline policy和dynamic refetch capability重新Fetch/decode/insert被GC的完整partition。
- 验收：相同时间tie和Chunk重叠在reload后结果不变；refetch retry不复用metadata-opening或current-seek counters；terminal representation先撤销refetch；root existence与Store event一致；reload不重复registration metadata。
- 负责人：TBD；提交：TBD；备注：TBD。

### [ ] MCAP-093 — 分离 requested/committed time 与 candidate clock

- 建议提交：`Separate remote requested and committed time`。
- 依赖：MCAP-089、MCAP-090。
- 变更：增加 `CommittedPresentationTime`、pending navigation intent、stable demand key、`Paused/Playing`和显式clock-hold input，requested generation/buffering/mutation/GC期间丢弃wall-clock dt。
- 验收：generic Following和非正speed结构化拒绝；hold期间仅离散命令替换intent；presentation commit后下一帧才恢复正向积分；inactive/hidden恢复不消费后台dt。
- 负责人：TBD；提交：TBD；备注：TBD。

### [ ] MCAP-094 — 实现 Supersedable/CommitLocked seek 状态机

- 建议提交：`Add remote seek commit state machine`。
- 依赖：MCAP-091、MCAP-093。
- 变更：Fetch/decode阶段可supersede，commit-required batches进入staging；第一次add前进入CommitLocked，后续导航只更新latest pending intent；区分pre-mutation current-seek rollback、initial failure和post-write poison。
- 验收：旧presentation在pre-mutation工作期间继续；CurrentSeekFailedBeforeMutation释放staging/pins/reservation并暂停旧cursor；InitialPresentationFailed保持零query清理；第一次写后任何失败不可重开facade。
- 负责人：TBD；提交：TBD；备注：TBD。

### [ ] MCAP-095 — 实现 GatedRecordingQueryFacade 与完整 revision lease

- 建议提交：`Add revisioned remote presentation facade`。
- 依赖：MCAP-011、MCAP-036、MCAP-094。
- 变更：在 Wasm remote-MCAP recording context 实现 InitialPresentationGated/Ready/ClosedForMutation/TerminalGated、不可拆分 `(facade_instance, StoreId, epoch)` revision、RAII lease和committed timeline/cursor snapshot，native/local context 不取得此状态机。
- 验收：mutation只在旧lease全部退出后非阻塞开始；stale/duplicate Drop不改计数；last lease请求repaint；A session同epoch迟到结果不能进入B；incomplete extent的range lease零Store query并返回统一incomplete。
- 负责人：TBD；提交：TBD；备注：TBD。

### [ ] MCAP-096 — 增加 Web remote consumer 与 privileged storage context

- 建议提交：`Separate Viewer query and storage capabilities`。
- 依赖：MCAP-095。
- 变更：从query-sensitive共享callsite抽取sealed窄consumer capability；native/local通过passthrough adapter继续访问原 `ViewerContext`/`EntityDb`，remote adapter只持presentation lease、committed snapshot和complete-range capability；privileged storage capability仅交给remote frame driver、installer、arbiter与cleanup。
- 验收：禁止在接收完整 `ViewerContext` 的运行时分支中假装隔离；Wasm compile-fail/API-surface test证明remote View/query/cache reachable graph不含AppContext storage、StoreHub、StoreBundle、EntityDb或storage engine；native公开API、查询结果、cache和side-effect trace与基线一致。
- 负责人：TBD；提交：TBD；备注：TBD。

### [ ] MCAP-097 — 接入 Web remote TimeControl、TimePanel 与 navigation ranges

- 建议提交：`Route remote time controls through presentation leases`。
- 依赖：MCAP-036、MCAP-093、MCAP-096。
- 变更：让TimeControl/TimePanel的query-sensitive部分消费MCAP-096窄capability；remote adapter只暴露manifest canonical timeline，并分开requested marker、committed cursor、indexed/loaded/loading extent、buffering/hold和NoTemporalData；native/local passthrough adapter继续原path。
- 验收：非canonical timeline、Following和错误time type不进入remote planner；Web remote TimePanel不能取得完整 `ViewerContext` 或直接查询EntityDb；partial loaded range不伪装完整density；native/local/RRD时间控件API、结果和side-effect trace差分不变。
- 负责人：TBD；提交：TBD；备注：TBD。

### [ ] MCAP-098 — 接入 Web remote View、Dataframe 与数据 UI 查询

- 建议提交：`Migrate Viewer recording queries to sealed facades`。
- 依赖：MCAP-096、MCAP-097。
- 变更：把View systems、selection/data UI、Dataframe、TextLog和StateTimeline的query-sensitive入口迁移到MCAP-096窄capability；remote adapter提供lease-derived committed time及complete-range capability并禁止privileged fallback，native/local passthrough adapter调用原查询实现。
- 验收：InitialPresentationGated、ClosedForMutation、TerminalGated和nonforeground时remote query invocation为零；latest-at正确；默认EVERYTHING range在coverage不完整时显示incomplete而不查询resident子集；native公开API、查询结果、执行顺序和side-effect trace不变。
- 负责人：TBD；提交：TBD；备注：TBD。

### [ ] MCAP-099 — 验证 Web remote memoizer、cache、video 与外部 query

- 建议提交：`Validate asynchronous query results by presentation revision`。
- 依赖：MCAP-095、MCAP-096、MCAP-098。
- 变更：memoizer、transform/view cache、video range cache和external recording query通过MCAP-096窄capability取得snapshot；remote cache key、insert/hit/publication使用完整revision且不接受裸epoch，native/local passthrough adapter保留原cache key与执行路径。
- 验收：A→close→B同epoch的View/cache/video/transform/Web query迟到结果全部拒绝；facade closed期间不能用旧cache hit绕过；compile-fail test阻止remote adapter取得privileged storage；native/local cache API、key、hit/miss及publication trace不变。
- 负责人：TBD；提交：TBD；备注：TBD。

### [ ] MCAP-100 — 实现 Store-scoped RemoteStoreMutationArbiter

- 建议提交：`Serialize remote Store mutations`。
- 依赖：MCAP-068、MCAP-091、MCAP-095。
- 变更：Wasm remote-MCAP insertion、GC、terminal cleanup统一持typed turn ownership；关闭facade并drain lease后执行；close在同步Store调用后的safe point抢占并把ownership直接转给cleanup；native mutation scheduler 不变。
- 验收：insertion/GC不并行；CommitLocked close不继续ack/commit/reopen；hidden suspended turn只one-shot rebind；late/stale turn不修改Store或facade；cleanup有限帧完成。
- 负责人：TBD；提交：TBD；备注：TBD。

### [ ] MCAP-101 — 接入 remote GC、pressure Close 与 memory arbitration

- 建议提交：`Integrate remote GC with Viewer memory pressure`。
- 依赖：MCAP-036、MCAP-039、MCAP-092、MCAP-100。
- 变更：只在持有 Web remote-MCAP memory capability 的 Store 上，按resident root cap与cursor距离选择完整roots，保护current closure/staging/pins；实际deletion才推进epoch；no-op使用Store/protection revision backoff；inactive pressure Close可supersede该remote session的pins/suspended turn。
- 验收：remote cap-state unsorted Store在work threshold内；GC event后coverage精确；pending bytes不伪报freed；Inactive+CommitLocked/GC pressure在有限帧Vacant且不继续ack/commit；普通foreground remote保护不被错误绕过；native、local及非MCAP Store不进入该arbiter，purge/close trace与MCAP-001基线一致。
- 负责人：TBD；提交：TBD；备注：TBD。

### [ ] MCAP-102 — 实现 best-effort predecessor backfill

- 建议提交：`Add bounded predecessor backfill`。
- 依赖：MCAP-015、MCAP-022、MCAP-029、MCAP-090、MCAP-094。
- 变更：只对allowlist声明单消息替代状态的Channel执行lazy reverse MessageIndex lookup，共享每seek request/bytes/entries/deadline预算，使用backfill-owned caller phase Range policy接入retry foundation，并遵守decoder state policy。
- 验收：找到predecessor时并入同一commit closure；找不到、attempt或backfill预算耗尽按best-effort可观察状态收敛且不终结source；非法index仍失败；不支持stateful decoder明确拒绝而不伪造状态。
- 负责人：TBD；提交：TBD；备注：TBD。

### [ ] MCAP-103 — 统一 RemoteMcapFailure 与 terminal cleanup

- 建议提交：`Classify remote failures and terminal cleanup`。
- 依赖：MCAP-015、MCAP-043、MCAP-094、MCAP-100。
- 变更：冻结OptionalWork/CurrentSeek/SessionFatal与各phase exhaustion/retryability，在产生边界分类；prefetch object change不降级；terminal latch先关闭query/update/refetch，再触发tokenized cleanup。
- 验收：metadata exhaustion仍只终结opening source；phase-B网络timeout与attempt/Range/deadline exhaustion按demand owner分类，validator/length/412永远SessionFatal；pre-mutation seek failure不自动创建新retry operation；post-insertion failure为PresentationCommitPoisoned；后续explicit/pressure close只记录cleanup trigger。
- 负责人：TBD；提交：TBD；备注：TBD。

### [ ] MCAP-104 — 把 remote CPU 与 controller 接入 Viewer frame driver

- 建议提交：`Drive remote MCAP work from Viewer frames`。
- 依赖：MCAP-033、MCAP-073、MCAP-089 至 MCAP-103。
- 变更：在Wasm Viewer frame中以一个有界remote-MCAP slice顺序处理page state、`RetryPending` admission、controller、planner、Fetch completion、单CPU work、mutation和query presentation；既有receiver/LogChannel/gRPC/Redap frame ordering不变。
- 验收：Fetch/browser callback不直接retry、parse或mutation；下一eligible visible control turn每帧至多启动一个retry attempt且不挤占现有Viewer work；CatalogOnly/Inactive零temporal Fetch/decode/mutation/query/cursor delta；completion会请求repaint；native frame trace不变。
- 负责人：TBD；提交：TBD；备注：TBD。

### [ ] MCAP-105 — 分离 UserNavigationRevision 与 programmatic selection

- 建议提交：`Preserve compatible recording selection semantics`。
- 依赖：MCAP-048、MCAP-054、MCAP-089、MCAP-104。
- 变更：在现有 user navigation revision 上增加 remote-MCAP programmatic intent；compatibility remote operation 在原同步调用顺序中冻结authority且activation不推进user revision；strict batch使用最后OpenAndSelect的batch-local winner，不引入全局 BrowserIngressSequence。
- 验收：既有non-MCAP compatibility URL反序完成仍last-completion-wins；remote user navigation使旧intent失效；strict batch winner稳定；native/legacy/gRPC/Redap selection trace 不变。
- 负责人：TBD；提交：TBD；备注：TBD。

## 14. M9 — Startup、产品面、可观测性与最终验收

### [ ] MCAP-106 — 锁定并接入 eframe two-phase WebRunner fork

- 建议提交：`Patch eframe with two-phase Web runner startup`。
- 依赖：MCAP-004、MCAP-088。
- 变更：workspace以`[patch.crates-io]`锁定基于0.35.0的最小fork和immutable revision/checksum，只在 `cfg(target_arch = "wasm32")` WebRunner公开 `prepare_app → PreparedWebRunner::{activate,abort}`，记录upstream与rebase责任；该workspace patch是项目控制的Web发布构建输入，不承诺脱离workspace的下游Cargo构建自动继承patch。
- 验收：prepare不安装ResizeObserver、DOM handlers、RAF或repaint callback；activate成功后恰好一次安装；部分activate failure可abort无残留；Web发布产物的build metadata与smoke test证明使用锁定fork及two-phase ABI；native依赖source provenance允许因workspace patch变化，但native feature resolution、公开API、构建、startup测试和运行行为差分必须通过。
- 负责人：TBD；提交：TBD；备注：TBD。

### [ ] MCAP-107 — 实现 startup visibility 双快照与 bootstrap listener

- 建议提交：`Bootstrap Web Viewer page visibility safely`。
- 依赖：MCAP-067、MCAP-106。
- 变更：strict remote-MCAP bootstrap前读visibility、安装唯一不能访问App的bounded listener、再读并reconcile，runner activation和terminal eviction前final read；listener ownership随prepared runner abort。
- 验收：页面初始hidden、bootstrap中切hidden、重复signal和listener后无新event都得到正确state；strict remote 路径在publish/work前拒绝并回到Stopped；现有 compatibility startup 状态机不改成全局HiddenSuspended。
- 负责人：TBD；提交：TBD；备注：TBD。

### [ ] MCAP-108 — 在现有 startup sources 中接入 remote MCAP

- 建议提交：`Open remote MCAP from existing Web startup sources`。
- 依赖：MCAP-054、MCAP-067、MCAP-107。
- 变更：保留 `create_app` 和现有 compatibility startup loop、hidden `AppOptions.url`、direct `start` 顺序；只在单项确认为 remote MCAP 后安装remote source，非 MCAP item 继续原 dispatcher。
- 验收：mixed合法/非法URL保持warning/continue并让Viewer Running；数组和两种startup source顺序与MCAP-001一致；nested query安全来源不丢；初始hidden时remote work 有界park，不延迟或重排其他 startup item。
- 负责人：TBD；提交：TBD；备注：TBD。

### [ ] MCAP-109 — 实现 strict startWithRequests bootstrap handoff

- 建议提交：`Add atomic strict Web Viewer startup`。
- 依赖：MCAP-051、MCAP-053、MCAP-106、MCAP-107。
- 变更：在app-creator内创建唯一disarmed App、执行共享remote-MCAP-only HTTP transaction、用one-shot cell交付descriptors/handles/acks/release token，fallible runner activation先于terminal eviction和route arm。
- 验收：invalid batch、wrapper throw、wrong token、hidden race和activate failure均在ready/RAF/observer/handler/work前同步 `FailingStart → Stopped`；terminal registry不变；同wrapper可用新instance重试；成功`StartResult`只代表accepted。
- 负责人：TBD；提交：TBD；备注：TBD。

### [ ] MCAP-110 — 接入 remote recording panel、catalog card 与 open options UI

- 建议提交：`Expose remote MCAP opening and catalog UI`。
- 依赖：MCAP-089、MCAP-097、MCAP-105。
- 变更：OpenAndSelect/Open安装panel行为，Background安装可发现可关闭但不等价StoreHub preview的metadata catalog card；UI可选time type、Topic/decoder config和advanced consistency policy并展示actual warning/status。
- 验收：明确 `.mcap` compatibility与known/extensionless strict MCAP × 三种behavior矩阵可观察；compatibility无扩展名URL保持原dispatcher，strict非MCAP返回unsupported；metadata control plane对非foreground仍有限完成；非foreground零temporal work；重新选择从committed cursor恢复；默认不启用DeploymentAssumed。
- 负责人：TBD；提交：TBD；备注：TBD。

### [ ] MCAP-111 — 接入资源指标与脱敏诊断

- 建议提交：`Instrument remote MCAP resource and lifecycle metrics`。
- 依赖：MCAP-005、MCAP-104、MCAP-109、MCAP-110。
- 变更：实现设计第20节中 Web remote-MCAP 相关的metrics、high-water marks、work-unit durations、remote registry snapshots、page suspension、intern burn、presentation/GC和failure distributions，不增加LogChannel、Redap或全局ingress指标。
- 验收：label不含高基数secret；count/bytes与实际ownership可对账；performance.memory不可用时不伪装精确heap；debug输出也经过redaction断言。
- 负责人：TBD；提交：TBD；备注：TBD。

### [ ] MCAP-112 — 发布 TypeDoc、changelog 与迁移说明

- 建议提交：`Document remote MCAP Web Viewer contracts`。
- 依赖：MCAP-054 至 MCAP-057、MCAP-067、MCAP-069、MCAP-073、MCAP-080、MCAP-108 至 MCAP-111。
- 变更：记录remote-MCAP compatibility 分支与strict API、handles/lifecycles/dispose、semantic dedup、catalog-only Background、strict string/intern limits、remote hidden/pagehide 行为和 startup 契约；明确 legacy、LogChannel、gRPC、Redap、raw-event 与 native Viewer 不在改动范围。
- 验收：所有remote-MCAP accepted regression都有before/after、迁移示例和host contract test链接；文档不承诺data-ready于start resolve；对排除路由只声明保持现状，不发布无关breaking change。
- 负责人：TBD；提交：TBD；备注：TBD。

### [ ] MCAP-113 — 完成桌面 Chrome 网络与 correctness E2E

- 建议提交：`Add remote MCAP Chrome correctness E2E suite`。
- 依赖：MCAP-003、MCAP-104、MCAP-109、MCAP-110。
- 变更：覆盖same/cross-origin Range、CORS/CSP、validator变化、NoIndexedMessages、overlap、seek supersede、backfill、GC reload、remote-MCAP strict/compatibility API、hidden/resume/pagehide和query gate，并对排除路由运行一组无行为变化差分用例。
- 验收：设计第19.7、23.1、23.2、23.4节全部可自动判定；每个failure有脱敏public status；不允许full-object fallback、partial presentation、stale result或跨recording污染。
- 负责人：TBD；提交：TBD；备注：TBD。

### [ ] MCAP-114 — 完成内存、主线程与 release gate

- 建议提交：`Enforce remote MCAP Chrome release gates`。
- 依赖：MCAP-088、MCAP-101、MCAP-104、MCAP-111、MCAP-113。
- 变更：在release web build运行remote-MCAP七类不可抢占路径、长时window playback、反复seek/GC/reload、remote hidden/visible flood、registry churn、runtime intern exhaustion和Wasm memory pressure测试；LogChannel、Redap、gRPC及其他nonremote routes只进入差分门，不进入新的remote资源或page gate。
- 验收：所有remote hard cap、main-thread阈值、finite-frame cleanup、resume revalidation上界和Wasm/module-lifetime budget通过；release artifact symbol/dependency/API审计证明side-map transaction没有扩散到native或nonremote callsite，side-map candidate/growth的legacy global map scan/copy为零。
- 验收：compatibility extensionless网络trace保持零新probe；`pixi run rerun-build-web`、Web发布产物metadata/two-phase ABI smoke、相关clippy/nextest、Chrome stable E2E和lint全部绿色；native公开API、feature resolution、build、startup/runtime差分，以及RRD、legacy HTTP、LogChannel、gRPC/message proxy、Redap、local/native MCAP的route/error/lifecycle/hidden/intern trace无回归后才允许启用MVP feature。
- 负责人：TBD；提交：TBD；备注：TBD。

## 15. 设计覆盖索引

| 设计章节或能力 | 主要任务 |
| --- | --- |
| Web-only 范围、现有兼容边界与公开行为 | MCAP-001、MCAP-054、MCAP-105、MCAP-108、MCAP-112 |
| Secret URL、validator、time type 与安全输入 | MCAP-007 至 MCAP-009、MCAP-014 至 MCAP-016、MCAP-045、MCAP-069 |
| Chrome Range、retry、Summary、index 与 Chunk validation | MCAP-013 至 MCAP-025A、MCAP-033、MCAP-067、MCAP-088 |
| Header/Summary 三阶段 allocation barrier、Summary opcode/group 语法与 definition canonicalization | MCAP-017 至 MCAP-020 |
| Descriptor-only physical ownership 与 full MessageIndex record alignment | MCAP-021、MCAP-022 |
| Bound remote object、ambiguous-zero resolution、Physical Chunk source authority、pending-header typestate、per-ordinal read lease 与 exact-body owner handoff | MCAP-009、MCAP-015、MCAP-021、MCAP-023 至 MCAP-025A、MCAP-033、MCAP-043、MCAP-088 |
| ROS 2/protobuf bounded initializer、decoder assignment、manifest identity authority与两阶段decode | MCAP-026 至 MCAP-033；实施顺序为025A与030A后先032再031 |
| Web remote Store roots、registration 与 coverage | MCAP-034 至 MCAP-037、MCAP-039、MCAP-042、MCAP-092 |
| Open slot、source/operation/recording lifecycle | MCAP-043 至 MCAP-057 |
| Remote-MCAP page execution | MCAP-067、MCAP-068、MCAP-073、MCAP-080 |
| Remote strict input 与 runtime intern budget | MCAP-012、MCAP-069、MCAP-081、MCAP-083 |
| Window planner、seek、refetch与backfill phase retry owner | MCAP-090 至 MCAP-094、MCAP-102 至 MCAP-104 |
| Web remote query facade与consumer capability隔离 | MCAP-095 至 MCAP-099 |
| Remote mutation arbiter、GC与memory pressure | MCAP-039、MCAP-068、MCAP-100、MCAP-101、MCAP-103 |
| Viewer frame integration与selection | MCAP-104、MCAP-105 |
| eframe two-phase startup | MCAP-106 至 MCAP-109 |
| 产品UI、指标、文档与release验收 | MCAP-110 至 MCAP-114 |
| Phase A和最终阶段门 | MCAP-088、MCAP-114 |

## 16. 最终项目完成检查表

- [ ] 90 个有效工作项均填写负责人、commit SHA和验收结果，26 个 `[~]` 项保留范围决策记录且没有实现提交。
- [ ] 每个提交都可在其依赖点独立构建和回退，没有只靠后续提交修复的已知不通过测试。
- [ ] compatibility API只有文档明确列出的accepted regression，其余characterization tests保持通过。
- [ ] strict remote-MCAP API、compatibility MCAP 分支、page execution和remote session都覆盖success/failure/close/stop/stale callback，排除路由由差分测试证明不变。
- [ ] 所有remote-MCAP不可信输入在对应Fetch、copy、allocation、intern、Store mutation或public effect之前完成容量和一致性检查；排除路由继续既有边界且由差分测试保护。
- [ ] ROS 2/protobuf remote initializer只消费same-source sealed definitions，在首次allocation前完成strict whole-schema census和峰值预留；后续assignment/manifest/decode复用同一sealed result且release artifact不存在无界重初始化call edge。
- [ ] immutable manifest是`DerivationPartitionKey`、stable partition/root descriptor和root `ChunkId`的唯一authority；MCAP-032先于031完成，decode只消费matching issuer且任何失败零partial publication。
- [ ] fresh-per-open `BoundRemoteObjectCapability`、MCAP-023 plan和MCAP-025 authority只能经MCAP-025A exactly-once resolution owner合并；032只消费final owner，partial/stale/out-of-order resolution不能产生canonical layout或留下reservation。
- [ ] admitted protobuf decode覆盖default/null、presence、oneof、enum、repeated、packed、map、nested message和unknown-field policy的完整bounded Arrow语义，并与local normalized output逐项差分。
- [ ] 每个physical Chunk只能沿021→020→019 evidence owner、source authority、one-shot pending lease、MCAP-025 header-validated transition、MCAP-024 output和scanner/cache链流动；caller无法传CRC/codec metadata，duplicate-live、stale generation、index/header mismatch、CRC与body copy overlap门通过。
- [ ] remote recording的所有查询只能通过完整presentation revision的sealed facade，编译期边界测试通过。
- [ ] terminal status、public lifecycle、Store removal和Viewer stop的顺序只实现设计中的唯一normative table。
- [ ] Remote-MCAP 在 Chrome hidden、pagehide/freeze和resume revalidation中不存在无界内存或永久饥饿，且不暂停或重排其他 Viewer work。
- [ ] GC、inactive pressure close、CommitLocked、page-hidden suspended turn和terminal cleanup均在有限帧内收敛且不重开错误facade。
- [ ] URL、query、ETag、Topic、EntityPath、raw StoreId和内部token不进入公开事件、错误、日志、指标或持久化状态。
- [ ] release Wasm的Range、parser、decode、insertion和GC阈值均由固定fixture冻结，未通过项不会以debug/native结果代替。
- [ ] Native Viewer 没有新的 remote-MCAP 网络入口、状态机、UI 或 Store/query 行为；共享 crate 的 additive 修改均有 native differential 证据。
- [ ] TypeDoc、changelog、migration note和宿主contract tests与最终产品行为一致。

## 17. 暂不在本计划中的工作

以下内容维持设计中的非目标，若需要必须另立RFC和工作计划：

- Native Viewer 的远程 MCAP 网络播放、native-specific UI 或 native Store/query 迁移。
- Legacy HTTP importer 重构、strict-to-legacy handoff、Web LogChannel 背压/transfer/scheduler、全局 compatibility ingress/command reducer、gRPC/message-proxy hidden policy 和 Redap detached publication。
- 其他浏览器支持、Web Worker、Wasm threads和cross-origin isolation。
- reverse continuous playback、通用Following和完整View-driven range demand。
- 未索引顶层MCAP消息的完整性证明或producer-trusted snapshot capability。
- 通用`IndexedSource`抽象、跨session raw cache、TF/video状态恢复和平台SSRF connector。
- gRPC message proxy的resume协议；它需要版本化stream sequence、retention boundary和ack/resume token。
- Store-scoped可回收字符串arena以及whole-Web/process-global interner治理；MVP的不退款module-lifetime budget只覆盖remote-MCAP新增identifier，非remote路径保持旧行为。
- 移除eframe fork；只有upstream发布等价two-phase hook并通过本计划contract tests后才能执行。
