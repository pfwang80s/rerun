# Work Item 01 Baseline Review

## Review

**结论：FAIL。**

Work Item 01 的范围是冻结 Git 边界、模块图、四 profile 编译及 warning/error baseline，不修改源代码。当前证据只能证明部分命令曾执行，不能证明完整 baseline 已按计划形成，也不能证明工作树边界、profile graph 或历史证据隔离满足要求。

### Correct

- `_items_refact_20260828.md:215-265` 将 Work Item 01 限定为 baseline 记录，不要求修改生产源代码。
- `_items_refact_20260828.md:1129` 将 Work Item 01 标记为 `complete — review pending`，而 Work Items 02–16 仍为 `pending`。
- 因此，没有发现 implementation item 被错误标记为完成。
- `/data/tools/refact-20260828-work-item-01/commands.tsv` 记录了命令退出状态，并且与日志中可见的结果基本一致：
  - ordinary Wasm：`exit=0`
  - Phase A：`exit=0`
  - locked release：`exit=1`
  - host clippy：`exit=101`
  - host check：`exit=0`
  - scope-s lint：`exit=1`
  - rustfmt：`exit=0`
  - diff check：`exit=0`
- `ordinary-wasm.log` 和 `phase-a.log` 都显示编译完成，但同时产生大量 dead-code warning。
- `locked-release.log` 明确显示构建失败，原因是 `re_mcap` 无法解析 `re_string_interner`。
- `host-clippy.log` 明确显示在 `re_mcap` lib test 上因 20 个 warning-as-error/dead-code 错误失败。
- `host-check.log` 显示 host check 的进程最终完成，但产生 20 个 warning。
- `scope-s-lint.log` 的内容显示 custom lint 报告了 73 个错误，包括 `todo!()` 格式、错误变量命名、双空格、迭代器风格和可读性问题。
- `rustfmt.log` 和 `diff-check.log` 为空，且 `commands.tsv` 分别记录为成功。
- 当前计划中的架构 findings 与已有源代码状态一致：`_items_refact_20260828.md:1129` 之后的现有 findings 仍指出 concrete `web_body_handoff` seam、宽泛 Wasm cfg 和 module-level `dead_code` suppression 尚未修复；这些属于后续 implementation work item，不应在 Work Item 01 中被标记完成。

## Findings

### Blocker 1 — Baseline handoff 文件缺失

**位置：**

- 计划引用：`_items_refact_20260828.md:1129`
- 预期文件：`handoff/william-refact-work-item-01-baseline.md`

**证据：**

- `/data/rerun/handoff/william-refact-work-item-01-baseline.md` 不存在。
- `/data/rerun/handoff/` 中可见的是 `william-demo-baseline-record.md`，这是另一个真实 MCAP demo baseline，不能替代 Work Item 01 baseline。
- `_items_refact_20260828.md:258-261` 明确要求 baseline 文档或 worker progress 记录、四 profile 完整结果、dirty/untracked manifest，并区分本次运行结果与历史记录。

**影响：**

Work Item 01 的核心交付物缺失。现有 `/data/tools/...` 目录不能单独替代 handoff，因为其中缺少多个必需证据文件，且没有完整的 Git 边界记录。

**最小修复：**

恢复或重新生成 `handoff/william-refact-work-item-01-baseline.md`，内容必须只记录本次 Work Item 01 运行，并明确区分历史证据。

---

### Blocker 2 — 必需证据文件缺失，无法验证 Git 边界和 profile graph

**位置：**

- 证据目录：`/data/tools/refact-20260828-work-item-01/`
- 计划要求：`_items_refact_20260828.md:225-231`、`:256-261`

**缺失文件：**

- `git-boundary.log`
- `toolchain.log`
- `profile-graph.log`
- `dead-code.log`

目录中实际只有：

- `commands.tsv`
- `diff-check.log`
- `host-check.log`
- `host-clippy.log`
- `locked-release.log`
- `ordinary-wasm.log`
- `phase-a.log`
- `profile-diagnostics.txt`
- `rustfmt.log`
- `scope-s-lint.log`

**影响：**

无法验证以下关键要求：

1. 当前 `git status --short` 的 dirty/untracked manifest；
2. 当前 `git diff --name-only`；
3. 当前 `git diff --stat`；
4. 没有 staged files；
5. dirty/untracked 文件是否被正确排除；
6. 使用的 Rust/Cargo/toolchain 版本；
7. 四 profile 的实际 module graph、cfg、target 和入口；
8. `remote_chunk_scan.rs`、`web_remote_mcap_cpu.rs` 及其他 suppression 的实际扫描结果；
9. 是否存在旧证据被提升为本次 baseline。

**最小修复：**

重新执行并保存：

```text
git status --short
git diff --name-only
git diff --stat
git diff --cached --quiet
```

同时生成 `toolchain.log`、`profile-graph.log` 和 `dead-code.log`，并在 handoff 中说明每条记录是否为本次运行。

---

### High 1 — locked verifier baseline 明确失败，但 Work Item 01 状态仍写为 complete

**位置：**

- 状态：`_items_refact_20260828.md:1129`
- 失败日志：`/data/tools/refact-20260828-work-item-01/locked-release.log`

**证据：**

`locked-release.log` 报告：

```text
error[E0433]: cannot find module or crate `re_string_interner`
error[E0432]: unresolved import `re_string_interner`
error: could not compile `re_mcap` due to 3 previous errors
```

`commands.tsv` 也记录：

```text
locked-release exit=1
```

**判断：**

Work Item 01 的目标是冻结 baseline，因此失败本身可以作为 baseline 记录，不能要求本工作项修复 implementation 问题。但是状态必须明确表示“baseline recorded / review pending / locked baseline failed”，不能让 `complete` 被误读为四 profile gate 已通过。

计划本身已经规定失败证据不能标记为通过，例如 `_items_refact_20260828.md:976` 要求 gate 未执行或失败时保持 pending。虽然 Work Item 01 是 baseline 记录项而非最终 gate，但 handoff 必须清楚地将 `exit=1` 作为失败 baseline，而非成功验证。

**最小修复：**

在 baseline handoff 和状态说明中明确：

- locked verifier baseline：失败；
- 失败原因：`re_string_interner` unresolved import；
- 该失败未被修复；
- 不得将 Work Item 01 的 `complete` 解释为 locked verifier 已通过。

---

### High 2 — ordinary Wasm 和 Phase A 均非 warning-free，warning counts 没有被完整、结构化冻结

**位置：**

- `/data/tools/refact-20260828-work-item-01/ordinary-wasm.log`
- `/data/tools/refact-20260828-work-item-01/phase-a.log`
- `/data/tools/refact-20260828-work-item-01/profile-diagnostics.txt`

**证据：**

ordinary Wasm：

- `re_mcap (lib) generated 70 warnings`
- `re_viewer (lib) generated 2 warnings`
- 另外包含 `re_web` 的 unused import warning 和 `re_chunk_store` 的 dead-code warning。
- 日志中还出现大量 `remote_physical_resolution.rs` dormant graph warning。

Phase A：

- `re_mcap (lib) generated 27 warnings`
- `re_chunk_store (lib) generated 1 warning`

host check：

- `re_mcap (lib test) generated 20 warnings`

host clippy：

- 因 20 个 warning-as-error/dead-code 错误失败。

**判断：**

这与设计中“warning baseline 先冻结，后续逐模块清零”的要求一致，不能把成功编译误报为 warning-free。但当前证据只是日志 excerpts，没有在 handoff 中形成按 profile、crate、warning/error 数量归类的明确 baseline。

**最小修复：**

在 handoff 中加入结构化表格，至少包含：

| Profile | target/cfg | exit | warning/error baseline | 是否通过 |
|---|---|---:|---|---|
| ordinary Wasm | wasm32，无 Phase A/locked cfg | 0 | 至少 `re_mcap 70`、`re_viewer 2`，另有 `re_web`/`re_chunk_store` warning | 编译成功但非 warning-free |
| Phase A | wasm32 + `rerun_mcap_phase_a_proof_v1` | 0 | `re_mcap 27`、`re_chunk_store 1` | 编译成功但非 warning-free |
| locked verifier | wasm32 + locked cfg | 1 | unresolved `re_string_interner`，并有 warnings | 失败 |
| host tests | native + cfg(test) | check 0 / clippy 101 | 20 warnings/errors | check 完成但 clippy 失败 |

如果某些 warning 总数无法从现有 excerpts 精确计算，应重新运行并保存完整日志，而不是猜测总数。

---

### High 3 — profile graph claims 未被证据支持

**位置：**

- 计划要求：`_items_refact_20260828.md:228-230`
- 缺失证据：`/data/tools/refact-20260828-work-item-01/profile-graph.log`

**证据：**

- `profile-graph.log` 不存在。
- 现有 `profile-diagnostics.txt` 只有 warning/error excerpts，没有实际 module graph、cfg expansion、target 或入口记录。
- 源代码搜索仍显示：
  - `crates/store/re_mcap/src/remote_chunk_scan.rs` 使用 `#![allow(dead_code)]`；
  - `crates/viewer/re_viewer/src/web_remote_mcap_cpu.rs` 使用 `#![allow(dead_code)]`；
  - `crates/viewer/re_viewer/src/web_remote_mcap_cpu.rs:1367-1531` 仍直接引用 `re_mcap::web_body_handoff` concrete types。
- 因此不能仅凭 build exit 0 推断 ordinary Wasm 与 Phase A/locked graph 已隔离。

**影响：**

无法验证 Work Item 01 要求冻结的“四 profile 实际 cfg、target、模块入口”，也无法证明 ordinary product Wasm 没有意外加载 dormant/full graph。

**最小修复：**

重新生成 `profile-graph.log`，逐 profile 记录：

- target；
- 显式 cfg；
- build package/profile；
- module entry points；
- `remote_chunk_scan`、`remote_physical_resolution`、decoder、manifest、dispatch、runtime intern 的 inclusion/exclusion；
- ordinary / Phase A / locked / host 的差异；
- 现存 module-level suppressions。

---

### Medium 1 — dead-code suppression 盘点缺少本次证据

**位置：**

- 计划要求：`_items_refact_20260828.md:253-254`
- 缺失证据：`/data/tools/refact-20260828-work-item-01/dead-code.log`

**证据：**

实际源码搜索显示 suppression 不仅存在于两个重点文件，还存在于多个 `re_mcap` 和 `re_viewer` remote 模块，例如：

- `crates/store/re_mcap/src/remote_chunk_scan.rs:7`
- `crates/viewer/re_viewer/src/web_remote_mcap_cpu.rs:3`
- 以及多个其他 remote 模块。

当前 `dead-code.log` 缺失，因此无法确认 William 是否记录了完整 suppression 清单，或是否把旧扫描结果当作本次 baseline。

**最小修复：**

重新生成完整 suppression 清单，至少标出：

- 文件路径；
- 行号；
- `allow(dead_code)` 类型；
- 所属 profile；
- 是否属于后续 Work Item 08 的删除范围。

---

### Medium 2 — custom lint 失败项尚未在 baseline handoff 中准确归档

**位置：**

- `/data/tools/refact-20260828-work-item-01/scope-s-lint.log`

**证据：**

日志报告：

```text
scripts/lint.py found 73 errors.
```

主要类别包括：

- `crates/store/re_mcap/src/lib.rs:65`：`todo!()` 应带 `$details`；
- 多个 `_error` / `error` 命名；
- 多个 double space；
- `a.zip(b)` 风格问题；
- `Store` title casing；
- 可读性换行建议。

`commands.tsv` 同时记录 `scope-s-lint exit=1`。

**判断：**

这是可接受的 baseline 失败结果，但必须明确标记为失败，不可只保留空的 `scope-s-lint.log` 结论或在总体状态中暗示所有验证已通过。

**最小修复：**

在 handoff 中列出失败命令、错误总数、主要文件范围和“由后续 implementation/lint 工作项处理”的说明。

---

### Low — 证据目录的文件集合与计划引用不一致

**位置：**

- `_items_refact_20260828.md:1129`
- `/data/tools/refact-20260828-work-item-01/`

**证据：**

计划引用：

```text
/data/tools/refact-20260828-work-item-01/;
handoff/william-refact-work-item-01-baseline.md
```

但实际证据目录缺少四个计划要求核验的日志，handoff 文件也缺失。

**最小修复：**

完成证据归档后，在 handoff 中列出完整 manifest，并确认每个文件均为本次运行生成。

## 未发现的问题

- 没有证据表明 `/data/demo` 文件被提交或被错误当作当前 Work Item 01 的四 profile 通过证据。
- `handoff/william-demo-baseline-record.md` 明确限制了 demo baseline 的适用范围，没有把它描述为 Chrome、Wasm、release 或 differential gate。
- 没有证据表明 Work Items 02–16 被错误标记为完成。
- `commands.tsv`、`rustfmt.log` 和 `diff-check.log` 支持 rustfmt/diff-check 命令成功，但 Git staged 状态仍因缺失 `git-boundary.log` 无法独立确认。

## Residual Risks

- Git dirty/untracked 文件可能未被完整记录，无法确认未来 commit 是否会混入无关文件。
- 无法确认当前是否存在 staged files。
- 无法确认 toolchain 版本和环境是否与预期一致。
- 无法确认 ordinary Wasm、Phase A 和 locked verifier 的模块 graph 隔离。
- ordinary Wasm 和 Phase A 当前编译成功，但仍有大量 dormant graph warning。
- locked verifier 当前无法编译，不能作为 locked warning-free 或 artifact safety 证据。
- host clippy 当前失败，host check 仅完成编译检查且保留 20 个 warning。
- custom lint 当前有 73 个错误。
- `remote_chunk_scan.rs` 和 `web_remote_mcap_cpu.rs` 的 module-level `dead_code` suppression 仍存在。
- Viewer 到 `re_mcap::web_body_handoff` 的 concrete owner seam 仍未完成重构；该问题属于后续实现项，不应在 baseline review 中被误判为已修复。
- production capability 是否保持 `production_disarmed` 在已有源代码搜索中可见于 `re_web_tests/release_gate_audit.rs`，但缺少本次 `toolchain`/profile evidence 的完整证明，仍应在 handoff 中明确记录。

## Commit Safety

**Work Item 01 当前不安全提交。**

它可以作为“baseline-only”工作项提交的前提是：

1. 补齐 `handoff/william-refact-work-item-01-baseline.md`；
2. 补齐 `git-boundary.log`、`toolchain.log`、`profile-graph.log`、`dead-code.log`；
3. 在 handoff 中准确记录所有命令及退出码；
4. 明确 ordinary Wasm/Phase A 的 warning baseline；
5. 明确 locked release 和 host clippy 失败是 baseline 结果，而不是通过；
6. 记录 dirty/untracked exclusion、无 staged files 和无旧证据 promotion；
7. 保持 Work Items 02–16 为 pending；
8. 不修改生产源代码、不修改 capability、不创建 implementation commit。

完成上述证据修正并复审通过后，Work Item 01 才可安全作为 **plan/baseline-only** commit 提交。当前 verdict 为 **BLOCK**。