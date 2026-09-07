# `_items_refact_20260828.md`

## 目标

依据 `_refact_20260828.md`，以最小、可独立验证的工作项完成 MCAP-114 Web remote-MCAP 四 profile 重构。

本计划只覆盖以下边界：

- `re_mcap`、`re_web` 与 `re_viewer` 的 Web remote-MCAP ownership、opaque contract、profile cfg、warning 和验证边界；
- ordinary product Wasm、Phase A proof、locked verifier、host tests 四种 profile；
- Chrome strict Range transport、受控 file-backed fixture 和 `/data/demo` 证据边界；
- API、compile-fail、ownership、revalidation、Drop 顺序和 source-shape 测试；
- 每个工作项由 William 单独实现，Dafee 只读 review，Blocker/High/Medium 修复并复审后，才创建该工作项独立 commit。

明确不改变：

- native/local MCAP；
- RRD、legacy HTTP、LogChannel、gRPC/message proxy、Redap；
- 非 remote route；
- production capability，必须继续保持 `production_disarmed`；
- `/data/demo` 文件及 Git LFS fixture；
- `_refact_20260828.md`。

---

## 当前上下文与已验证边界

### 已读取的设计和既有计划

已完整读取：

- `_refact_20260828.md`
- `_items_mcap114_four_profile_split.md`
- `_items_mcap114_opaque_contract.md`
- `_items_mcap114_trusted_opaque_seam.md`

既有计划显示：

- Phase A support、opaque contract 和 trusted seam 尚未完成真实跨 crate producer/consumer 迁移；
- `web_body_handoff` 与 Viewer 之间仍存在 concrete owner 边界；
- release/Phase A 仍有 dormant graph warning；
- host clippy 曾因既有 warning 失败；
- 旧计划中的 `[x]` 只表示历史工作项曾完成，不能作为本次任何 gate 已通过的证据。

### 当前源代码证据

#### `crates/store/re_mcap/src/lib.rs`

当前模块声明包含：

- `web_body_handoff`：`#[cfg(any(target_arch = "wasm32", test))] pub mod`
- `remote_physical_resolution`：Wasm 下公开，host test 下 crate-private
- `remote_chunk_scan`：`#[cfg(any(target_arch = "wasm32", test))]`
- 完整 decoder、manifest、dispatch、runtime intern 等模块已经使用 locked cfg，但 remote physical 层仍比最终设计需要的边界更宽。
- 当前 `remote_physical_resolution` 的 Wasm 可见性不是 `all(target_arch = "wasm32", rerun_mcap_phase_a_proof_v1)` 或 locked 专用 conjunction。

#### `crates/store/re_mcap/src/remote_physical_resolution.rs`

当前模块：

- 直接依赖 `remote_chunk_scan`、`remote_decompression`、`remote_summary` 和 `remote_time`；
- 拥有具体的 physical object、registry、lease、budget、reservation 和生命周期 owner；
- `RemotePhysicalObjectBindingV1` 为 public 类型；
- 构造入口通过 `#[cfg(any(test, rerun_mcap_phase_a_proof_v1))]` 存在；
- 该实现同时承载 Phase A measurement 和完整 physical owner graph，尚未按设计拆分。

#### `crates/store/re_mcap/src/web_body_handoff.rs`

当前模块：

- 公开 `WebPhysicalBodyStrategyV1`、`WebPhysicalBodyProfileStatusV1`、`WebPhysicalBodyPhaseAProfileV1`、`WebPhysicalCopyOverlapBudgetV1`、`WebPendingPhysicalChunkReadV1`、`WebPhysicalPendingIdentityV1`、`WebBorrowedPhysicalChunkBodyV1` 等类型；
- `WebPendingPhysicalChunkReadV1::identity_v1` 暴露 source generation、read generation、ordinal、range 和 budget profile；
- `bind_borrowed_exact_body_v1` 接收裸 `&[u8]` 和 public profile 值；
- `WebPhysicalPendingIdentityV1` 提供多个 public scalar accessor；
- `WebPhysicalScanCacheEntryV1` 持有 concrete `PhysicalChunkScanCacheEntry`；
- 该 API 仍允许跨模块消费者看到 concrete physical handoff 类型和可观察的 scalar identity，尚未达到设计要求的 opaque、non-forgeable cross-crate seam。

#### `crates/store/re_mcap/src/remote_chunk_scan.rs`

当前模块顶部存在：

```rust
#![allow(dead_code)]
```

该模块仍包含完整 physical Chunk authority、header validation、semantic scan、budget、definition projection 等图。

当前 cfg 是 `any(target_arch = "wasm32", test)`，因此 ordinary Wasm 仍可能编译 dormant/full graph。移除 suppression 前必须先完成模块抽取和精确 cfg 收口。

#### `crates/viewer/re_viewer/src/lib.rs`

当前 `web_remote_mcap_cpu` 及多个 Web remote-MCAP 模块使用：

```rust
#[cfg(any(target_arch = "wasm32", test))]
```

因此 ordinary Wasm 和 host tests 共用同一模块入口，尚未体现 common opaque contract、Phase A adapter、locked adapter 和 test-only constructor 的最终隔离。

#### `crates/viewer/re_viewer/src/web_remote_mcap_cpu.rs`

当前模块顶部存在：

```rust
#![allow(dead_code)]
```

并且当前 Wasm adapter 直接命名：

- `re_mcap::web_body_handoff::WebPendingPhysicalChunkReadV1`
- `re_mcap::web_body_handoff::WebPhysicalCompletionBindErrorV1`
- `re_mcap::web_body_handoff::WebPhysicalBodyHandoffErrorV1`
- `re_mcap::web_body_handoff::WebPhysicalCopyOverlapBudgetV1`
- `re_mcap::web_body_handoff::WebPhysicalScanCacheEntryV1`

当前 `WasmPhysicalFetchCompletionOwnerV1` 同时持有 Web `ExactLengthRangeBody` 与 re_mcap concrete pending lease。

当前 Phase A 代码还定义 `RemoteCpuWorkKindV1::PhysicalChunkDispatchDecode`，因此需要证明该变体和对应执行路径不会进入 Phase A profile，且 Phase A 不得引入 dispatch/decode graph。

#### `crates/utils/re_web/src/chrome_range.rs`

当前 strict Range API 已具备：

- non-empty checked request range；
- `Content-Range`、`Content-Length`、ETag 和 content encoding 校验；
- HTTP status 映射；
- strong validator 和 representation consistency；
- `BoundChromeRangeObject` 与 `BoundChromeRangeRequest`。

需要在重构中保持这些 transport owner 和 validator 语义，不将 MCAP physical authority、decoder、manifest、Store 或 secret-bearing 类型引入 `re_web`。

特别需要继续验证 `BoundChromeRangeObject::validator` 等 API 只用于 transport 层 revalidation，不成为下游伪造 MCAP identity 的入口。

### 当前 Git status 边界

本次上下文中未执行 Git 命令，因此不把任何旧日志、旧计划状态或目录中可见的临时文件列表当作当前 Git status 证据。

William 开始每个工作项前必须实际记录：

```text
git status --short
git diff --name-only
git diff --stat
```

并将结果写入该工作项的验证记录。

当前计划不声称任何 gate 已通过。

---

## 四 profile 精确 cfg 矩阵

所有实现和验证必须以显式 conjunction 为准，不得使用 Phase A cfg 作为 locked capability 的别名。

| Profile | target / cfg | pure opaque contract | `re_web` transport | Viewer common scheduler | Phase A physical adapter | full physical owner | decoder / manifest / dispatch / runtime intern | verifier anchor / probe | capability |
|---|---|---|---|---|---|---|---|---|---|
| ordinary product Wasm | `wasm32`，无 Phase A、无 locked cfg | 编译 | 编译 | 编译 | 不编译 | 不编译 | 不编译 | 不编译 | `production_disarmed` |
| Phase A proof | `wasm32` + `--cfg rerun_mcap_phase_a_proof_v1` | 编译 | 编译 | 编译 | 编译 | 不编译 | 不编译 | 不编译 | `production_disarmed` |
| locked verifier | `wasm32` + `--cfg re_mcap_locked_remote_wasm_allocator_v1` | 编译 | 编译 | 编译 | 不编译，除非某个明确 locked probe 需要且设计批准 | 编译 | 编译 | 仅 locked conjunction | 不自动 armed |
| host tests | native host + `cfg(test)` | 编译 | 仅测试所需 | 编译测试复用 | 编译测试所需 | 编译测试所需 | 编译测试所需 | 不导出 verifier anchor | `production_disarmed` |

实现约束：

```rust
#[cfg(all(target_arch = "wasm32", rerun_mcap_phase_a_proof_v1))]
```

和：

```rust
#[cfg(all(target_arch = "wasm32", re_mcap_locked_remote_wasm_allocator_v1))]
```

必须分别使用。

不得使用：

```rust
#[cfg(any(target_arch = "wasm32", rerun_mcap_phase_a_proof_v1))]
```

来表达 Phase A 专属模块。

不得让 `cfg(test)` constructor、identity accessor、test fixture 或 host-only API 泄漏到 ordinary product Wasm。

---

## 固定执行协议

每个工作项都必须严格遵循以下顺序：

1. William 在干净的本工作项隔离工作树中实现一个最小可验证批次。
2. William 更新本计划中的对应状态和证据字段，但不得修改其他工作项状态。
3. William 运行 focused tests、affected clippy、custom lint、fmt、diff check 和适用的四 profile compile。
4. Dafee 对当前工作项做只读 review。
5. Dafee 只报告有证据的 Blocker、High、Medium、Low。
6. Blocker、High、Medium 必须修复。
7. 修复后重新运行受影响验证并由 Dafee 复审。
8. 只有该工作项 review 无 Blocker/High/Medium 且证据完整，才创建该工作项 isolated commit。
9. 记录 commit SHA、commit manifest、命令、结果、日志位置和残余风险。
10. 确认工作树只包含该工作项批准文件后，才开始下一个工作项。

任何工作项不得：

- 与其他 William worker 同时写同一文件；
- 在一个 commit 中混入其他 worker 的 dirty/untracked 文件；
- 清理、覆盖或提交无关 dirty 文件；
- 通过 `git add -A` 或 wildcard staging 包含未批准文件；
- 在 review 或 hard gates 前 commit；
- 在本地证据不足时声称通过。

---

# 工作项 1 — 冻结 Git 边界、模块图和四 profile warning baseline

## 目的

建立本次实施唯一可信的起点，避免把既有计划中的历史状态误认为当前通过状态。

## William 实施内容

不修改源代码，只记录：

- 当前 `git status --short`；
- 当前 `git diff --name-only`；
- 当前 `git diff --stat`；
- 目标模块和直接调用关系；
- 四 profile 的实际 cfg、target、模块入口；
- 每个 profile 的 warning/error baseline；
- 当前 `production_disarmed` 和 locked attestation 现状。

目标文件边界至少包括：

```text
crates/store/re_mcap/src/lib.rs
crates/store/re_mcap/src/remote_physical_resolution.rs
crates/store/re_mcap/src/web_body_handoff.rs
crates/store/re_mcap/src/remote_chunk_scan.rs
crates/viewer/re_viewer/src/lib.rs
crates/viewer/re_viewer/src/web_remote_mcap_cpu.rs
crates/utils/re_web/src/chrome_range.rs
```

允许同时记录测试、lint 和证据路径，但不得扩大源代码修改范围。

## 必须验证

- ordinary product Wasm 当前实际是否可编译；
- Phase A 当前实际是否可编译；
- locked verifier 当前实际是否可编译；
- host tests 当前实际 warning/error；
- `remote_chunk_scan.rs` 和 `web_remote_mcap_cpu.rs` 的 module-level suppression；
- `re_mcap` 到 Viewer 的 concrete type 引用清单。

## 交付证据

- baseline 文档或 worker progress 记录；
- 四 profile 命令和完整结果；
- 当前 dirty/untracked manifest；
- 明确区分“本次运行结果”和“历史记录”。

## Commit manifest

工作项 1 不修改源代码，可以不创建 commit。

---

# 工作项 2 — 抽取并冻结纯 opaque cross-crate contract

## 目的

建立不携带 HTTP、MCAP parser、decoder、manifest、dispatch、runtime intern、Store 或 secret-bearing 类型的跨层 contract。

## William 实施内容

根据现有目录结构新增或调整最小 contract 模块，推荐：

```text
crates/store/re_mcap/src/remote_physical_contract.rs
crates/viewer/re_viewer/src/web_remote_mcap_operation.rs
crates/viewer/re_viewer/src/web_remote_mcap_cpu_contract.rs
```

实际命名可遵循现有结构，但每个 canonical type 只能有一个定义。

contract 必须：

- 使用 private fields；
- move-only；
- 不实现 `Clone` 或 `Copy`；
- 不提供 public scalar constructor；
- 不提供 source generation、read generation、ordinal、profile、attempt、range、URL、query、ETag、body、lease、cache、raw token 或 identity accessor；
- 不成为 fieldless result；
- 只表达 operation、result/terminal outcome、scheduler state 和 redacted error category；
- 只由 trusted producer 创建；
- ordinary/native 下不暴露不必要的 remote namespace。

如果 contract 跨 crate 暴露，返回类型必须只能从 producer 入口获得，下游不能直接构造。

## 必须添加测试

- native non-test consumer 不能导入 remote contract module；
- downstream 不能使用 scalar 构造 operation/result；
- operation/result 不能 `Clone` 或 `Copy`；
- contract source shape 不包含 HTTP、URL、query、ETag、Authorization、decoder、manifest、dispatch、runtime intern、Store 或 secret-bearing 类型；
- contract 不泄漏裸 body、lease、cache 或 range。

compile-fail 测试不得依赖删除生产代码，也不得通过放宽 stderr 匹配掩盖错误。

## 必须验证

- ordinary Wasm；
- Phase A；
- locked verifier；
- host focused API tests；
- `cargo fmt --check` 或仓库规定的 `pixi run rs-fmt`；
- affected clippy，使用 `-D warnings`；
- compile-fail 测试实际被执行。

## Commit manifest

只允许：

- 新增/修改 contract 文件；
- 对应 `lib.rs` module/export 声明；
- 对应 API/compile-fail/source-shape 测试；
- 该工作项必要的计划证据。

---

# 工作项 3 — 新增 `re_mcap_web_adapter` 并实现 trusted producer 与 non-forgeable token

## 目的

`re_mcap_web_adapter` 是唯一同时依赖 `re_web` 与 `re_mcap` 的 Web remote-MCAP 跨层组合层；`re_viewer`只消费该crate返回的opaque operation/result。`re_viewer`为其他既有非 remote功能保留的直接crate依赖不属于本路径，必须在实现记录中单独标识，不能用于remote owner组合。

## William 实施内容

trusted producer 必须位于 `re_mcap_web_adapter`。

该工作项必须同时：

- 新增workspace crate和Cargo依赖；
- adapter crate登记为workspace member并更新`ARCHITECTURE.md` crate表；同时必须按仓库指导完成FigJam架构图更新、PNG上传和生成HTML回写，相关命令和凭据/网络限制写入handoff；
- remote-MCAP路径的依赖必须严格为 `re_viewer → re_mcap_web_adapter → {re_web, re_mcap}`，而 `re_web ↛ re_mcap`、`re_mcap ↛ re_web`；任何其他`re_viewer`直接依赖须标记为unrelated existing dependency；
- adapter的Cargo必须使用显式profile features和optional target dependencies：默认`consumer_contract`不启用`re_web`/`re_mcap`，`phase_a`仅在`all(target_arch = "wasm32", feature = "phase_a")`启用，`locked`仅在`all(target_arch = "wasm32", feature = "locked", re_mcap_locked_remote_wasm_allocator_v1)`启用；locked cfg只能由外部attestation/build workflow提供；必须以`cargo metadata`和三份`cargo tree`实际输出验证consumer/Phase A负向依赖，不能以Rust cfg alone声称排除full graph；
  - **direction-b reconciliation**：adapter 的 `phase_a` 不再 gate 在 `rerun_mcap_phase_a_proof_v1` 上（该 cfg 是 `re_mcap/build.rs` 发出的 per-crate cfg，不传播到 adapter crate；否则 producer 永远不编译）。Phase-A proof attestation 由 proof consumer（`test_mcap_phase_a_chrome`）的 `#![cfg(all(target_arch = "wasm32", rerun_mcap_phase_a_proof_v1))]` 强制。
- WI-03必须使用带`--target wasm32-unknown-unknown`的`cargo tree -e features`并执行fail-closed断言：consumer tree不得出现`re_web`、`re_mcap`、`feature "phase_a"`或`feature "locked"`；Phase-A tree必须出现`re_web`、`re_mcap`和`feature "phase_a"`且不得出现`feature "locked"`、decoder assignment、manifest、dispatch、runtime-intern或`PhysicalChunkDispatchDecode`；locked tree只有在`RERUN_LOCKED_ATTESTATION_V1=1`外部attestation环境执行，必须出现`feature "locked"`和完整implementation依赖，未设置attestation的locked命令及Phase-A解析locked feature均必须非零退出。

- Phase A只使用`all(target_arch = "wasm32", feature = "phase_a")`（direction-b reconciliation：proof attestation 由 proof consumer 强制），不编译或调用dispatch/decode/runtime-intern，且该cfg不能启用locked cfg；
- locked adapter/full graph只使用`all(target_arch = "wasm32", feature = "locked", re_mcap_locked_remote_wasm_allocator_v1)`，该cfg只能由外部attestation/build workflow提供；
- host tests使用`cfg(test)`完整图，test-only constructor不得泄漏到product Wasm；
- 让 `re_viewer` 依赖adapter的opaque contract，而不是两侧concrete owner；ordinary product依赖必须显式`default-features = false, features = ["consumer_contract"]`；Phase A和locked必须分别使用`features = ["phase_a"]`与`features = ["locked"]`并保存`cargo metadata`/`cargo tree`负向依赖审计；

producer 必须：

- 接收 `re_web` 签发的 exact body/transport owner；
- 接收 `re_mcap` 签发的 physical authority 或 validated MCAP owner；
- 不接收裸 range、source generation、read generation、ordinal、profile、attempt 或 caller scalar；
- 生成 one-shot opaque operation；
- 持有 registry/slot 和必要的 private owner；
- 在 issue 和 consume 前校验 source identity、read/operation generation、canonical ordinal、profile、attempt、current state、body validity 和 reservation availability；
- 保持 operation/result move-only；
- 未消费 operation 的 Drop 执行 matching cancellation 和 release；
- consume 成功时明确转移必要 owner；
- 任意失败时释放 backing、permit、cache、body 和 lease，禁止 partial Store/registry/budget effect；
- 不使用 `'static` coercion、`leak`、`transmute`、`forget` 绕过 Drop。

## 需要重点重构的当前 seam

当前 `web_body_handoff.rs` 中：

- `WebPendingPhysicalChunkReadV1::identity_v1`；
- `bind_borrowed_exact_body_v1`；
- `WebPhysicalPendingIdentityV1` scalar accessors；
- `WebPhysicalScanCacheEntryV1` concrete inner；

不能继续作为 Viewer ordinary API 的公共跨 crate seam。

最小修复方向是：

- 将 identity 校验留在 trusted producer 或 private owner；
- ordinary Viewer 只收到 opaque operation/result/status/error；
- concrete lease/cache 仅由 producer 及其 private implementation 持有；
- 若 Phase A 测量必须观察数值，只在精确 Phase A cfg 下提供测试/measurement-only API；
- 不复制一份 structurally similar type 来绕过 type identity 或 lifetime 问题。

## WI-03/WI-05边界与前置条件

WI-03必须在本工作项内完成adapter producer及其profile-separated API，并提供compile/source-shape断言证明Phase-A-facing adapter不能导入或调用decoder、dispatch、manifest或runtime-intern graph；Phase A cfg不能别名locked cfg，`PhysicalChunkDispatchDecode`不能进入Phase A executable path。
WI-03的安全性不得依赖WI-05未来迁移才能成立。
- 完成 WI-03 后，WI-05 只能迁移 `re_viewer` consumer-side scheduling/translation adapter 和 Viewer 模块入口；不得补足或重新定义 WI-03 的 producer 安全前置条件。
- `re_viewer` 中的 Phase-A/locked adapter 仅是 consumer-side scheduling/translation adapter；不得依赖 `re_web`/`re_mcap` concrete owner API，不得定义或持有 body、lease、cache、reservation、registry、slot、decoder、manifest、dispatch 或 runtime-intern owner。
- 所有跨层 producer、owner binding、Drop、rollback、revalidation 和 registry/slot 逻辑只能位于 `re_mcap_web_adapter`。
- WI-05 的 owner/Drop 测试只能通过 producer-issued opaque operation/result 验证，不得在 Viewer 重新实现 owner 生命周期。


- stale read generation 在 consume 前失败；
- stale operation generation 在 consume 前失败；
- wrong ordinal 失败；
- wrong profile 失败；
- wrong attempt 失败；
- cross-source 组合失败；
- cross-session 组合失败；
- duplicate consume/replay 失败；
- unconsumed Drop 释放全部 owner 和 reservation；
- successful consume/result Drop 保持精确 release 顺序；
- body invalidation 前 revalidation 失败且无 partial effect；
- cache publication 前 revalidation 失败且无 partial effect；
- Store mutation 前 revalidation 失败且无 partial effect；
- allocation failure 后 aggregate claim、lower reservation 和 owner 全部回滚。

## 必须验证

- ordinary Wasm 不再看到 concrete physical owner；
- Phase A 使用正确的 measurement-only seam；
- locked verifier 保留完整 owner；
- host tests 能验证 Drop 和 fault injection；
- affected clippy `-D warnings`；
- source-shape lint；
- fmt。

## Commit manifest

Work Item 03的独立commit允许包含：

- 新增 `re_mcap_web_adapter` crate的Cargo和源文件；
- `Cargo.toml` workspace member和dependency更新；
- `ARCHITECTURE.md` crate表更新；
- 按 `ARCHITECTURE.md` 指导完成FigJam架构图人工更新、PNG上传和生成HTML回写；执行`pixi run upload-image --name architecture_diagram`，若需要credentials/network必须在handoff中记录，未执行时不得标记该项完成；
- adapter的Cargo/profile dependency audit证据：`cargo metadata --no-deps --format-version 1`、consumer/Phase A/locked三份`cargo tree`及jq负向/正向检查；
- producer/operation/ownership/revalidation/Drop/rollback测试；
- 本工作项计划状态和必要handoff文档。

WI-03实现不能在本修订计划未经Dafee复审通过前开始；当前状态必须保持design correction complete — review pending，所有实现项保持pending。

必须排除 `/data/demo`、LFS fixture、生成物、raw logs、target/session/temp文件和无关dirty/untracked路径。

---

# 工作项 4 — `re_mcap` physical graph 按 profile 拆分

## 目的

将 Phase A physical support 与完整 physical owner、decoder graph 分离。

## William 实施内容

推荐拆分为：

```text
crates/store/re_mcap/src/remote_physical_contract.rs
crates/store/re_mcap/src/remote_physical_owner.rs
crates/store/re_mcap/src/remote_physical_phase_a.rs
crates/store/re_mcap/src/web_body_physical_owner.rs
```

具体拆分必须满足：

### ordinary product Wasm

不编译：

- full physical owner；
- `remote_chunk_scan` 完整 scan graph；
- decoder；
- manifest；
- dispatch；
- runtime intern；
- Phase A adapter，除非存在真实 ordinary 调用且已获明确设计批准。

### Phase A proof

只编译：

- opening parse；
- MessageIndex parse；
- physical validation；
- 必要的 `byob_copy`；
- 必要的 physical evidence；
- bounded measurement。

不得编译或调用：

- decoder assignment；
- manifest；
- dispatch decode；
- runtime intern；
- Store mutation；
- full semantic decoder graph。

### locked verifier

在：

```rust
#[cfg(all(target_arch = "wasm32", re_mcap_locked_remote_wasm_allocator_v1))]
```

下编译完整 physical owner 和 verifier 所需 graph。

该 cfg 只能由外部 attestation/build workflow 启用，不能由 Phase A 隐式启用。

### host tests

编译测试所需完整 owner 和 fixtures，但不得把 test-only constructor/export 泄漏到 product Wasm。

## 必须验证

- `remote_physical_resolution.rs` 不再以过宽 Wasm cfg 加载完整图；
- `remote_chunk_scan.rs` 的 import/module graph 与 profile 矩阵一致；
- `web_body_handoff.rs` 中 concrete owner 和 ordinary shell 分离；
- lifetime、Drop 顺序和 reservation accounting 保持不变；
- native/local MCAP 行为无变化。

## Commit manifest

只允许：

- physical 模块新增、拆分、移动；
- `re_mcap/src/lib.rs` cfg/module 声明；
- 对应 physical focused tests；
- 必要的 test fixture 调整。

不得同时修改 Viewer scheduler 或无关 Store 逻辑。

---

# 工作项 5 — `re_viewer` ordinary CPU contract 与 Phase A/locked adapter 隔离

## 目的

消除 `web_remote_mcap_cpu.rs` 对 `re_mcap::web_body_handoff` concrete 类型的 ordinary 依赖。

## William 实施内容

- `re_viewer` 内的 Phase-A/locked **consumer-side scheduling/translation adapter** 只消费 `re_mcap_web_adapter` 签发的 opaque operation/result/status/error；它不是 owner adapter。
- 该 consumer-side adapter 不得依赖 `re_web` 或 `re_mcap` 的 concrete owner API。
- 它不得定义或持有 body、lease、cache、reservation、registry、slot、decoder、manifest、dispatch 或 runtime-intern owner。
- 所有跨层 producer、owner binding、Drop、rollback、revalidation 和 registry/slot 逻辑只能位于 `re_mcap_web_adapter`。
- `re_viewer` common CPU 不直接引用 `web_body_handoff`、`remote_chunk_scan` 或 phase measurement concrete owner。
- Phase-A consumer-side scheduling/translation adapter 只能调度 opening、MessageIndex、physical validation 和必要 BYOB copy 的 opaque results；`PhysicalChunkDispatchDecode` 不得进入 Phase-A executable path。
- locked consumer-side scheduling/translation adapter 只能消费 locked opaque results，不持有完整 graph。
- host tests只能通过 producer-issued opaque operation/result验证owner/Drop行为，不得在Viewer重新实现owner生命周期。

在 `crates/viewer/re_viewer/src/lib.rs` 中：

- ordinary common scheduler 使用 opaque operation/result/status/error contract；
- Phase-A consumer-side adapter 使用：
  ```rust
  #[cfg(all(target_arch = "wasm32", rerun_mcap_phase_a_proof_v1))]
  ```
- locked consumer-side adapter 使用：
  ```rust
  #[cfg(all(target_arch = "wasm32", re_mcap_locked_remote_wasm_allocator_v1))]
  ```
- host tests 使用精确的 `cfg(test)` 入口；
- 不以 `any(target_arch = "wasm32", test)` 让 ordinary、Phase-A、locked 和 host 共用完整 concrete graph。



## 必须测试

- ordinary scheduler 可在无 concrete physical owner 时编译；
- Phase A scheduler 不包含 dispatch/decode；
- locked adapter 只在 locked cfg 下编译；
- host test constructor 不进入 ordinary Wasm；
- operation 只能由 trusted producer 签发；
- retry、cancel、GC、resume、visibility transition 不绕过 revalidation；
- duplicate completion、stale completion 和 cross-source completion 被拒绝；
- backing body 在 pending lease 之前释放；
- owner、permit、cache 和 lease Drop 顺序精确。

## 必须验证

- Viewer ordinary Wasm compile；
- Phase A proof compile；
- locked verifier compile；
- host focused tests；
- affected clippy；
- fmt；
- custom scope-S lint。

## Commit manifest

只允许：

- `re_viewer/src/lib.rs` cfg/module 调整；
- `web_remote_mcap_cpu.rs` 和拆分出的 operation/adapter 模块；
- Viewer focused tests；
- 必要 contract 调整。

---

# 工作项 6 — `re_web` Chrome Range transport contract 与测试边界

## 目的

保持 `re_web` 为 Web transport owner，不让 MCAP physical authority 反向进入 transport crate。

## William 实施内容

审查并在必要时最小调整：

```text
crates/utils/re_web/src/chrome_range.rs
```

必须保留：

- non-empty range preflight；
- half-open 到 inclusive Range 转换；
- object length bound；
- strict `206`；
- `200` 不得 fallback 为 full-object；
- `Content-Range`；
- `Content-Length`；
- strong validator；
- `identity` content encoding；
- CORS/CSP/authorization 错误的 redacted category；
- AbortController matching；
- exact response body owner；
- representation revalidation。

`re_web` 不得：

- 依赖 `re_mcap`；
- 持有 MCAP physical authority；
- 持有 decoder、manifest、Store、runtime intern；
- 暴露 URL/query/Authorization/ETag 作为下游 MCAP identity；
- 返回 MCAP-specific concrete owner。

## 必须添加或确认测试

- 空 range 失败；
- `start >= end` 失败；
- `u64` overflow 失败；
- range 超出 frozen object length 失败；
- HTTP 200 映射为 `RangeUnsupported`；
- 401/403、404/410、412、416 正确映射；
- 缺少 `Content-Range` 失败；
- 缺少 ETag/strong validator 失败；
- 非 identity content encoding 失败；
- Content-Range 起止和总长度不一致失败；
- body 长度不匹配失败；
- changed representation 失败；
- single-range-only fixture 拒绝 multi-range；
- full-object fallback 不可达。

## Chrome 证据边界

本地 Chrome evidence 只能来自：

- Chrome stable；
- 固定 loopback page origin；
- 固定 object origin；
- synthetic Range/CORS/CSP fixture；
- 固定根目录；
- 文件白名单；
- 固定 SHA；
- 严格单 Range response；
- 真实浏览器 Fetch/ReadableStream/BYOB 路径。

不得将浏览器测试通过状态推断为：

- Wasm warning-free；
- locked verifier 完整安全证明；
- production E2E；
- 长时 window playback；
- memory pressure 或 runtime intern exhaustion 已通过。

失败必须保留原始日志、浏览器版本、fixture SHA、请求范围和响应 headers。

## Commit manifest

只允许：

- `chrome_range.rs` 必要实现；
- transport unit/API tests；
- 受控 Chrome fixture/runner；
- 必要证据引用。

不得提交 `/data/demo` 大文件、Chrome 生成的临时 artifact 或 session artifact。

---

# 工作项 7 — runtime intern、manifest、dispatch、token allocator 和 strict startup cfg 收口

## 目的

完成剩余 dormant graph 的精确 profile 隔离。

## William 实施内容

逐模块确认：

- runtime intern；
- manifest；
- decoder assignment；
- dispatch；
- `re_chunk_store` remote token allocator；
- strict startup `Armed`；
- `CapabilityUnavailable`；
- verifier anchor/probe。

要求：

- ordinary 和 Phase A 不加载 full decoder/manifest/dispatch/runtime intern；
- locked verifier 只在 locked conjunction 下加载完整 graph；
- token allocator 仅在真实消费者 profile 下编译；
- `Armed` 不能由 Phase A 或普通 product cfg 构造；
- production capability 继续 `production_disarmed`；
- attestation 不能由 runtime scalar 或 caller input 伪造。

## 必须验证

- four profile module graph；
- source-shape/API boundary；
- locked anchor/probe 只在 locked cfg；
- ordinary Wasm 无 unresolved import；
- Phase A 不包含 dispatch/decode；
- native/local nonremote route diff 无变化。

## Commit manifest

只允许：

- runtime/manifest/dispatch/token allocator/strict startup 相关文件；
- 对应 cfg tests 和 compile-fail tests；
- 必要的 `lib.rs` 声明。

不得混入 Chrome fixture 或无关 warning 修复。

---

# 工作项 8 — 删除 module-level `dead_code` suppression 并清零 warning

## 目的

在调用图和 cfg 正确之后，移除 suppression，而不是以 suppression 隐藏 dormant graph。

## William 实施内容

至少处理：

```text
crates/store/re_mcap/src/remote_chunk_scan.rs
crates/viewer/re_viewer/src/web_remote_mcap_cpu.rs
```

以及本次迁移涉及的其他目标 remote 模块中的：

```rust
#![allow(dead_code)]
```

每个 warning 必须通过以下方式之一解决：

1. 精确 cfg；
2. 真实调用；
3. 模块或类型抽取；
4. 删除已证明不可达代码；
5. 正确 import、pattern、Debug 或 error propagation。

禁止：

- module-level `allow`；
- 批量 `expect(dead_code)`；
- 修改 workspace lint 配置；
- 隐藏 compiler output；
- skip 测试；
- 弱化断言。

## 必须验证

四 profile 都必须以 warning-deny 方式运行，且分别保存输出：

- ordinary product Wasm：0 warning、0 error；
- Phase A proof：0 warning、0 error；
- locked verifier：0 warning、0 error；
- host tests/clippy：0 warning、0 error。

static allocator、stack audit 只能作为 diagnostic-only evidence，不得标记为 warning-free 或完整安全证明。

## Commit manifest

只允许：

- warning 来源模块；
- 必要 cfg/import/调用图修复；
- warning regression test 或 lint 配置（不得放宽 lint）。

---

# 工作项 9 — API、compile-fail、source-shape 和 ownership 完整验证

## 目的

将设计中的不可伪造和跨层隔离要求固化为可重复测试。

## 必须覆盖

### API/compile-fail

- native non-test consumer 不能导入 remote contract；
- ordinary product Wasm 不能导入 full physical owner；
- Phase A 不能导入 decoder/manifest/dispatch/runtime intern；
- locked anchor 在非 locked cfg 下不可见；
- downstream 不能 scalar construct operation/result；
- downstream 不能构造 physical authority、lease、cache entry；
- operation/result 不能 Clone/Copy；
- test-only constructor 不可见于 product profile；
- `re_web` 不能导入 `re_mcap`；
- `re_mcap` 不能导入 `re_web`；
- Viewer 才能绑定两个 subsystem owner。

### Ownership/revalidation

- stale source/read/operation generation；
- wrong ordinal/profile/attempt；
- cross-source/session；
- duplicate consume/replay；
- unconsumed Drop；
- successful consume Drop；
- body invalidation；
- cache publication；
- Store mutation；
- cancellation；
- retry；
- GC；
- visibility hidden/visible；
- reload/resume；
- allocation failure rollback。

### Source shape

contract 及 ordinary scheduler source 不得包含：

- HTTP；
- URL；
- query；
- ETag；
- Authorization；
- raw body；
- physical lease；
- cache;
- decoder；
- manifest；
- dispatch；
- runtime intern；
- Store；
- secret-bearing owner。

---

# 工作项 10 — focused tests、affected clippy、fmt、custom lint 和 profile compile

## William 必须运行的验证类别

具体 package/target 参数必须根据工作项实际 changed files 收窄，不得用无关全仓扫描替代 focused gate。

至少包括：

```text
pixi run rs-fmt
cargo fmt --all -- --check
cargo clippy -p re_mcap --all-targets --all-features -- -D warnings
cargo clippy -p re_viewer --all-targets --all-features -- -D warnings
cargo clippy -p re_web --all-targets --all-features -- -D warnings
cargo nextest run --all-features --no-fail-fast -p re_mcap
cargo nextest run --all-features --no-fail-fast -p re_viewer
cargo nextest run --all-features --no-fail-fast -p re_web
```

若仓库现有 package 名称、feature 或 wasm task 不同，必须记录实际替代命令和原因，不得静默省略。

四 profile compile 必须明确记录：

```text
ordinary:
  wasm32 target
  无 rerun_mcap_phase_a_proof_v1
  无 re_mcap_locked_remote_wasm_allocator_v1

phase_a:
  wasm32 target
  --cfg rerun_mcap_phase_a_proof_v1
  不启用 locked cfg

locked:
  wasm32 target
  --cfg re_mcap_locked_remote_wasm_allocator_v1
  由受控 attestation/build workflow 提供
  不把普通 Phase A 结果当作 locked 结果

host:
  native target
  cfg(test)
  focused tests/clippy
```

每个命令必须记录：

- 完整命令；
- target；
- cfg/feature；
- exit code；
- warning/error 摘要；
- 日志路径；
- 是否为本次运行；
- 若未运行，写明原因。

---

# 工作项 11 — `/data/demo` 与 native differential 证据

## 目的

使用真实 demo 数据验证 native baseline，但不夸大证据范围。

## William 实施内容

允许使用 `/data/demo/` 下真实 MCAP 进行：

- native doctor/info；
- native indexed；
- native full decoder；
- 受控 file-backed 测试；
- native/nonremote differential。

必须保持：

- 固定根目录；
- 文件白名单；
- 固定 SHA；
- 不允许任意路径读取；
- 不允许 full-object fallback；
- Chrome fixture 使用严格单 Range。

不得：

- 修改 `/data/demo`；
- 提交 `/data/demo` 大文件；
- 将 native RSS 作为 Wasm memory pressure 证据；
- 将 native indexed/full decoder 结果作为 Chrome remote Range 或 production E2E 证据；
- 使用旧的“28 个 demo 文件通过”记录代替本次实际复测。

## 证据边界

`/data/demo` 只能证明本次明确执行的 native/file-backed 行为。

它不能证明：

- ordinary product Wasm warning-free；
- Phase A proof warning-free；
- locked verifier warning-free；
- Chrome stable CI；
- production remote-MCAP E2E；
- 长时 window playback；
- seek/GC/reload/resume revalidation；
- hidden/visible flood；
- Wasm memory pressure；
- runtime intern exhaustion；
- production capability 已 armed。

---

# 工作项 12 — Chrome focused gate 和 release artifact audit

## 必须验证

### Chrome

- Chrome stable；
- loopback page/object origin；
- strict Range；
- CORS/CSP；
- ETag/strong validator；
- single-range；
- malformed/adversarial responses；
- exact body length；
- abort/cancel；
- representation change；
- retry/stale completion；
- visibility hidden/visible；
- reload/resume；
- repeated seek；
- bounded memory behavior。

### Artifact

- ordinary Wasm WAT；
- exports；
- imports；
- dependency graph；
- verifier anchors/probes；
- locked-only symbols；
- absence of full decoder graph from ordinary/Phase A；
- no accidental production capability；
- no generated Wasm checked into Git。

static allocator/stack audit必须标注为 diagnostic-only。

Chrome 或 artifact gate 未执行时，状态必须保持 pending；失败时必须进入后续修复工作项，不能标记通过。

---

# 工作项 13 — Dafee 只读 review、修复和复审

## Review 范围

Dafee 只读检查：

- diff 与本工作项 manifest；
- profile cfg；
- dependency direction；
- ordinary/Phase A/locked/host module graph；
- opaque contract 是否可伪造；
- trusted producer 位置；
- owner/Drop 顺序；
- stale/revalidation/rollback；
- compile-fail/API/source-shape tests；
- warning-free 证据；
- Chrome 和 `/data/demo` 证据边界；
- production-disarmed 与 attestation；
- native/nonremote differential；
- dirty/untracked 文件隔离。

## 严重级别规则

- Blocker：违反架构方向、依赖方向、capability/attestation、ownership safety、production-disarmed、compile boundary，或存在确定性数据/安全损坏。
- High：release 或目标 profile 功能明显不正确、关键 revalidation/Drop/rollback 缺失、ordinary/Phase A 泄漏 full graph。
- Medium：缺少必要 API/compile-fail/source-shape 测试、warning gate 不完整、证据边界不清、commit manifest 不可审计。
- Low：仅报告性可维护性问题，不阻塞本工作项 commit。

Blocker、High、Medium 必须全部修复并复审。

Dafee 不得仅凭旧计划中的 `[x]` 或旧日志判定通过。

---

# 工作项 14 — 单工作项 isolated commit

## 创建 commit 的前置条件

只有同时满足以下条件才允许 William 创建该工作项 commit：

- Dafee review 无 Blocker、High、Medium；
- 该工作项 focused tests 通过；
- affected clippy `-D warnings` 通过；
- fmt 通过；
- 适用的四 profile compile 通过；
- compile-fail/API/source-shape tests 通过；
- 当前 dirty tree 只包含该工作项批准文件；
- 无未解释的 warning；
- 证据日志已记录；
- `/data/demo`、临时日志、secret、session artifact、生成 Wasm 未进入 manifest。

## Commit 之前必须运行

```text
git status --short
git diff --name-only
git diff --check
git diff --cached --quiet
```

随后只 stage 批准 manifest 中的文件，禁止：

```text
git add -A
git add .
```

commit message 必须明确对应单一工作项，不得将多个工作项合并。

记录：

- commit SHA；
- commit message；
- staged manifest；
- `git show --stat --oneline --decorate HEAD`；
- 该工作项全部验证命令和结果；
- 残余风险；
- Dafee review 结论。

---

# 工作项 15 — 下一工作项启动和 dirty-worktree 隔离

下一个工作项开始前必须确认：

- 上一个工作项已有 isolated commit；
- 上一个工作项 review 和复审已完成；
- 当前工作树 status 与上一个 commit 一致；
- 不存在未归属文件；
- 新 worker 只拥有下一个工作项的文件锁；
- 不得让两个 William worker 同时修改同一文件；
- 不得将前一工作项未完成变更带入下一工作项；
- 若发现上一个工作项仍有未完成项，继续该工作项，不得提前启动下一个工作项。

---

# 工作项 16 — 最终 release/CI/push 边界

## 本地最终条件

在所有工作项 isolated commit 完成后，才允许准备最终发布 commit。

最终发布前必须重新运行：

```text
git status --short
git diff --name-only
git diff --check
git diff --cached --quiet
```

并确认：

- ordinary product Wasm warning-free；
- Phase A proof warning-free；
- locked verifier warning-free；
- host tests/clippy 通过；
- scope-S custom lint 为 0；
- full release artifact/API/dependency/ABI evidence 通过；
- Chrome stable local gate 通过或已明确记录失败；
- production capability 仍为 `production_disarmed`；
- `/data/demo` 和临时 artifact 未 staged；
- commit manifest 只包含批准范围；
- Dafee 最终 review 无 Blocker、High、Medium。

## Push/CI 规则

未得到明确 push 授权前不得 push。

获得授权后：

1. push 前重新运行上述 Git checks；
2. push 后检查 Chrome stable CI；
3. 检查 Phase A evidence；
4. 检查 release artifact；
5. 检查全部 CI jobs 和 unresolved comments；
6. CI 失败必须修复并产生后续 commit；
7. 在 Chrome CI、长时播放、重复 seek、GC/reload/resume、visibility flood、malformed/adversarial MCAP、Wasm memory pressure、runtime intern exhaustion、native/nonremote differential 和 capability/attestation acceptance 全部通过前，不得将 MCAP-114 标记完成；
8. M9 保持阻塞；
9. production capability 保持 `production_disarmed`。

---

## 状态表

所有工作项从 pending 开始。

| 工作项 | 内容 | 状态 | commit | Dafee review | 证据 |
|---|---|---|---|---|---|
| 1 | Git 边界、模块图、四 profile baseline | baseline complete — review required; gates failed as recorded | — | FAIL — corrections recorded | `/data/tools/refact-20260828-work-item-01/`; `handoff/william-refact-work-item-01-baseline.md`; `handoff/william-refact-work-item-01-baseline-fix.md` |
| 2 | 纯 opaque cross-crate contract | implemented — bounded contract validations pass; locked verifier baseline and host clippy remain failed on pre-existing `re_string_interner`/dormant-graph issues; review required | — | pending | `/data/tools/mcap114-work-item-02-final/`; `handoff/william-refact-work-item-02-lint-validation-fix.md` |
| 3 | trusted producer 与 non-forgeable token | design correction complete — locked wrapper guard order, consumer positive feature assertion, consumer-only Viewer adapter boundary, and mandatory ARCHITECTURE diagram work recorded; Dafee re-review pending; implementation pending | — | re-review pending | `_refact_20260828.md`; `handoff/william-refact-wi03-plan-medium-fix.md` |
| 4 | re_mcap physical graph profile 拆分 | pending | — | pending | — |
| 5 | Viewer common/Phase A/locked adapter 隔离 | pending | — | pending | — |
| 6 | re_web Chrome Range transport contract | pending | — | pending | — |
| 7 | runtime/manifest/dispatch/token/startup cfg 收口 | pending | — | pending | — |
| 8 | 删除 module-level dead_code suppression | pending | — | pending | — |
| 9 | API/compile-fail/source-shape/ownership 验证 | pending | — | pending | — |
| 10 | focused tests、clippy、fmt、custom lint、profile compile | pending | — | pending | — |
| 11 | `/data/demo` 与 native differential evidence | pending | — | pending | — |
| 12 | Chrome focused gate 与 release artifact audit | pending | — | pending | — |
| 13 | Dafee review、Blocker/High/Medium 修复和复审 | pending | — | pending | — |
| 14 | 单工作项 isolated commit | pending | — | pending | — |
| 15 | 下一工作项 dirty-worktree isolation | pending | — | pending | — |
| 16 | 最终 release、push 和 CI follow-up | pending | — | pending | — |

---

## 当前设计 findings

### Blocker

1. **当前 concrete cross-crate seam 尚未满足设计要求。**
   证据：`crates/viewer/re_viewer/src/web_remote_mcap_cpu.rs:1363-1531` 直接命名并持有 `re_mcap::web_body_handoff` 的 pending lease、bind error、handoff error、overlap budget 和 scan cache entry。
   最小修复：先完成 `re_mcap_web_adapter` trusted producer 与 opaque operation/result seam，再迁移 Viewer ordinary CPU；不得以 public scalar identity/accessor 替代。

2. **`re_mcap` physical module 的 Wasm cfg 过宽，ordinary product 可能加载 dormant/full physical graph。**
   证据：`crates/store/re_mcap/src/lib.rs:121-129` 对 `remote_chunk_scan` 使用 `any(target_arch = "wasm32", test)`，`remote_physical_resolution` 在 Wasm 下直接 public；`remote_physical_resolution.rs` 顶部直接依赖完整 `remote_chunk_scan`、decompression、summary graph。
   最小修复：拆出 Phase A support、ordinary-visible shell 和 locked/full owner，并使用显式 `all(target_arch = "wasm32", ...)` conjunction。

3. **当前 `web_body_handoff` 暴露了可观察 scalar identity，不符合 opaque/non-forgeable contract。**
   证据：`crates/store/re_mcap/src/web_body_handoff.rs:177-255` 提供 `identity_v1`、`WebPhysicalPendingIdentityV1` 及 source/read generation、ordinal、range、profile accessor；`bind_borrowed_exact_body_v1` 接收裸 body 与 profile。
   最小修复：将这些校验留在 trusted producer/private owner；ordinary 跨 crate API 只返回 private-field、move-only opaque contract。

### High

1. **module-level `#![allow(dead_code)]` 仍掩盖 profile-specific dormant graph。**
   证据：
   - `crates/store/re_mcap/src/remote_chunk_scan.rs:1`
   - `crates/viewer/re_viewer/src/web_remote_mcap_cpu.rs:1`

   最小修复：先按 profile 抽取/收口调用图，再删除 suppression，逐 profile 使用 warning-deny 验证。

2. **Viewer module 入口使用 `any(target_arch = "wasm32", test)`，无法证明 ordinary、Phase A、locked 和 host graph 隔离。**
   证据：`crates/viewer/re_viewer/src/lib.rs:49-76`，以及 `web_remote_mcap_cpu.rs` 多处 `cfg(test)`、Phase A 和 Wasm 分支混合。
   最小修复：建立 common contract、精确 Phase A adapter、locked adapter、host test adapter 四层入口。

### Medium

1. **当前 Phase A CPU work kind 仍包含 `PhysicalChunkDispatchDecode`，需要 source-shape 或 compile test 证明该路径不会进入 Phase A。**
   证据：`crates/viewer/re_viewer/src/web_remote_mcap_cpu.rs:126-137`。
   最小修复：将 dispatch/decode 类型和执行函数移入 locked-only 模块，或提供精确的 Phase A compile-fail/source-shape 测试。

2. **`re_web` transport API 需要补充与 opaque seam 的边界测试。**
   证据：`crates/utils/re_web/src/chrome_range.rs:287-379` 已有 object/validator/range API，但当前 bounded context 未证明下游无法把 validator/range 重新解释为 MCAP physical identity。
   最小修复：增加 source-shape/API 测试，确保 `re_web` 只提供 transport owner/revalidation，不提供 MCAP-specific owner 或 scalar reconstruction。

3. **当前没有本次运行的 Git status、profile gate、Chrome 或 `/data/demo` 证据。**
   最小修复：由工作项 1 和后续各工作项实际执行并记录，不能引用旧计划状态替代。

---

## 残余风险

- 现有 `remote_physical_resolution.rs` 规模较大，physical owner、Phase A measurement 和完整 decoder-adjacent graph 的抽取可能暴露 lifetime、visibility 或 reservation rollback 问题。
- Viewer 与 `re_mcap` 的当前 concrete type identity 依赖较深，不能通过定义 structurally similar duplicate type 规避。
- ordinary product Wasm unresolved imports 需要在每次迁移后立即验证，不能等到全量工作项结束。
- Chrome local evidence 不能替代 stable CI、production remote-MCAP E2E、长时播放和 Wasm memory pressure。
- `/data/demo` native 结果不能替代 browser Range、BYOB、representation revalidation 或 runtime intern 证据。
- locked verifier 的 warning-free 和 artifact audit 依赖真实 locked cfg/attestation；Phase A 结果不能推导 locked 结果。
- 在所有 profile 和外部 CI gate 完成前，MCAP-114 必须保持未完成、M9 保持阻塞、production capability 保持 `production_disarmed`。

---

# Concise handoff — Dafee

已生成 `_items_refact_20260828.md` 计划，内容包括：

- 16 个按依赖顺序排列的独立工作项；
- William 单工作项实现、focused validation、Dafee 只读 review、Blocker/High/Medium 修复复审、isolated commit、再启动下一项的固定流程；
- ordinary product Wasm、Phase A proof、locked verifier、host tests 的精确 cfg 矩阵；
- opaque API、compile-fail、source-shape、revalidation、Drop、rollback 测试要求；
- warning-deny、clippy、fmt、custom lint、Chrome、release artifact 和 `/data/demo` 证据边界；
- dirty-worktree isolation、commit manifest、push/CI 规则；
- 初始状态表，所有工作项均为 pending。

## Dafee findings

- **Blocker：** `web_remote_mcap_cpu.rs` 仍直接持有 `re_mcap::web_body_handoff` concrete lease/cache/error 类型，trusted producer/opaque seam 尚未真正迁移。
- **Blocker：** `re_mcap` physical 模块的 Wasm cfg 过宽，ordinary Wasm 与 Phase A/full graph 的编译边界尚未满足设计。
- **Blocker：** `web_body_handoff.rs` 暴露 source/read generation、ordinal、range、profile 等 scalar identity/accessor，仍可被下游观察和组合。
- **High：** `remote_chunk_scan.rs` 和 `web_remote_mcap_cpu.rs` 仍有 module-level `#![allow(dead_code)]`。
- **High：** Viewer 入口仍广泛使用 `any(target_arch = "wasm32", test)`，无法证明四 profile 图隔离。
- **Medium：** Phase A CPU work kind 仍包含 dispatch/decode 变体，需要 locked-only 拆分或 source-shape/compile-fail 证明。
- **Medium：** 当前没有本次运行的 Git status、profile、Chrome 或 `/data/demo` gate 证据，旧计划状态不应被视为当前通过。
## Dafee BLOCK remediation — implementation prerequisite (design only)

- [x] 新增独立 workspace crate `crates/store/re_mcap_web_contract`（contract-only阶段已实现；见 `handoff/william-mcap114-contract-implementation.md`）。
- [ ] contract实现后请求独立 Dafee contract review；无 Blocker/High/Medium前不得继续 `re_web`/`re_mcap` producer seam。
- [x] 冻结唯一依赖图：`re_web → re_mcap_web_contract`、`re_mcap → re_mcap_web_contract`、`re_mcap_web_adapter → {re_mcap_web_contract,re_web,re_mcap}`、`re_viewer → re_mcap_web_adapter`，且两 owning crates互不依赖。
- [x] 冻结 contract禁止依赖/类型：Web owner、HTTP、URL、Response、stream、body、Abort、ETag、MCAP parser、lease、cache、reservation、Store、decoder、manifest、dispatch、runtime intern、Viewer、secret-bearing或可序列化身份。
- [x] 冻结 `CorrelationPairV1`、`WebCorrelationPermitV1`、`McapCorrelationPermitV1`、correlation material及non-authority `MatchedCorrelationV1`；private fields、non-Clone/non-Copy、无scalar constructor/projection/serialization；matching不提供producer authority。
- [!] 旧producer-witness/join-proof authority设计已被C+B v2 supersede；active rule见本文件末尾correction。
- [x] 明确 contract不宣称仅凭类型完成cross-source检查；contract仅负责non-authority correlation matching；两侧producer负责private-owner receipt与runtime revalidation；adapter消费两份真实receipt。
- [x] 冻结 Prepared→BodyBound→HeaderValidated→CacheReservationValidated→Consumed→ResultPublished→Released状态、owner、允许操作、revalidation、rollback及Drop顺序；同步callback不可逃逸，跨async只能owned transfer。
- [x] 冻结 Phase A唯一四阶段：`byob_copy` → `opening_parse` → `message_index_parse` → `physical_validation`；明确输入/输出、允许依赖/副作用及禁止dispatch/decode/decoder/manifest/Store/locked路径。
- [x] 修正 locked audit语义：环境变量仅fail-closed preflight marker；真实接受必须有build.rs/re_build_tools external-attestation artifact及source/target/profile/commit binding；记录missing/invalid/mismatch及forbidden-hit negative tests。
- [x] 修正 `assert_absent`为显式if/return非零，禁止 `grep ... && exit 1 || true`。
- [x] 扩展 ARCHITECTURE artifact checklist：crate table、graph、无reverse edge、PNG/FigJam版本、upload output、HTML round-trip和上传/写回失败fail-closed。
- [x] 设计修正完成后已获 Dafee contract-substage authorization（`dafee-mcap114-contract-authorization-final-review.md` PASS）；contract-only crate已实现，独立 Dafee contract review待执行。

## C+B hybrid authority correction v2 — contract + re_web receipt implemented; re_mcap receipt implemented, review pending

- [x] 旧 contract-only placeholder/public issuance/always-Err join已回退；当前 workspace不含 `re_mcap_web_contract`。
- [x] contract-only crate已实现：dependency-neutral `CorrelationFactoryV1`/`CorrelationPairV1`/move-only `WebCorrelationPermitV1`/`McapCorrelationPermitV1`/material/`MatchedCorrelationV1`/`CorrelationMismatchV1`，仅非authority matching；未实现producer receipt issuance。
- [x] public factory可创建unused pair，但不授予authority；所有字段private，禁止Clone/Copy/Debug/Hash/serialization、scalar constructor/projection/token/raw bytes。
- [x] consuming match只证明same-pair material并拒绝mismatch/replay/duplicate/missing；`MatchedCorrelationV1`不能独立授权operation。
- [x] producer authenticity仅来自 `re_web`/`re_mcap`各自private-owner receipt；contract material不能构造receipt。
- [x] semantic revalidation由owning producers在issue/consume/safe points执行。
- [x] adapter必须消费两份authentic receipt并执行non-authority matching；保持body/lease/cache/reservation/revalidation/Drop/rollback及`re_web ↛ re_mcap`、`re_mcap ↛ re_web`。
- [x] clean Dafee design review → contract non-authority crate（已实现）→ contract review → re_web receipt → review → re_mcap receipt → review → adapter → Viewer/Phase-A migration。
- [x] contract implementation Dafee review通过（PASS）；contract-only commit `ef9cecb679`。
- [x] `re_web` transport receipt substage已实现：`transport_receipt.rs`（wasm32 pub + host test private）、`RemoteTransportReceiptV1<B: TransportBodyOwner>`、sealed `TransportBodyOwner`、opaque `TransportReceiptErrorV1`、`pub(crate)` issuer、one-shot non-escape `consume_transport_v1`；见 `handoff/william-re_web-receipt-substage.md`。
- [x] `re_web` receipt Dafee review通过（PASS）；re_web receipt commit `63df0709ba`。
- [x] `re_mcap` physical receipt substage已实现：`WebPhysicalReceiptV1` 增加 `material: Option<McapCorrelationMaterialV1>`；`issue_from_lease_v1`/`from_lease_v1` 消费 `McapCorrelationPermitV1`；`bind_exact_body_v1` 返回 `(WebBorrowedPhysicalChunkBodyV1, McapCorrelationMaterialV1)`；legacy `bind_borrowed_exact_body_v1` 适配；Phase-A/测试调用点传递 permit；见 `handoff/william-re_mcap-receipt-substage.md`。
- [ ] `re_mcap` receipt Dafee review通过前不得实现adapter或Viewer/Phase-A migration，也不得commit。R1 visibility partial，R2 partial，R3 blocked，`production_disarmed`。
