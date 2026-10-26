# 实现阶段

> 本文只记录**实现路线与进度**，不是使用文档。命令怎么用见 [README](../README.md)。

## 状态规则

- 状态只有两种：**未完成** / **已完成**。
- 只有该阶段的全部验收项都通过，才把状态改为「已完成」。
- 阶段按依赖顺序推进；前一阶段未完成时，不开始后一阶段（`M0` 除外，它无依赖）。

## 总览

| 阶段 | 内容 | 依赖 | 状态 |
|---|---|---|---|
| M0 | `ldp hexdump`：字节区间读取与十六进制渲染 | 无 | 未完成 |
| M1 | `ldp info`：footer、GBO/CMO、schema/列元数据装载 | M0 | 已完成 |
| M2 | `ldp schema`：schema 树渲染 | M1 | 已完成 |
| M3 | `ldp columns`：列摘要（暂缓，`info` 的 CMO 表已覆盖） | M1 | 未完成 |
| M4 | `ldp pages`：`PageLayout` 解码 | M1 | 已完成 |
| M5 | `ldp show`：页字节、chunk 表、rep index | M4 | 已完成 |
| M6 | `ldp chunks`：mini-block chunk 表 | M5 | 未完成 |
| M7 | `ldp check`：结构校验 | M1–M6 | 未完成 |

依赖关系：

```text
M0 hexdump                    （独立原语）

M1 info                       容器层
 ├── M2 schema                GBO[0] 视图
 ├── M3 columns               CMO 视图（暂缓）
 └── M4 pages                 + 解码 PageLayout（只读元数据）
      └── M5 show            + 读页字节、chunk 表、rep index
           └── M6 chunks     mini-block 子集
M7 check                      以上全部 + 边界校验
```

---

## M0 - `ldp hexdump`

**状态：未完成**

> `FileSource::read_at` 已在 M1 中实现；`hexdump` 命令本身尚未实现。

目标：建立最小 I/O 原语和十六进制渲染，后续所有命令都复用它。

范围：

- `read_at(offset, len)`：越界返回清晰错误，不 panic
- `--offset` / `--length` / `--width`
- 输出：偏移、hex、ASCII gutter

验收：

- `ldp hexdump f.lance --offset 0 --length 64` 正常输出
- 越界读取返回错误且退出码非 0
- `--format json` 可用（至少包含 offset/length/bytes）

---

## M1 - `ldp info`

**状态：已完成**

完成于：footer 解析（v1 16B 仅用于识别并拒绝 / v2 40B）、GBO/CMO、
`FileDescriptor`、`ColumnMetadata` 装载，以及 text / json 输出。测试覆盖
v1（拒绝）/ v2.0 / v2.1 / v2.2 样本，并断言装载过程不读取任何 page buffer。

目标：定义“成功打开一个 Lance 文件”的标准。只解析容器层，不读任何页字节。

范围：

- 解析 40 字节 footer：magic、major/minor、三个 offset、两个 count
- 校验区域顺序与 CMO/GBO 表大小
- 解码 `GBO[0]` 为 `FileDescriptor`（schema + 行数）
- 解码每一列的 `ColumnMetadata`（页数、页 buffer 区间、列 buffer）
- 输出：路径、大小、版本、行数、区域表、global buffer 表、列摘要
- legacy v1：直接报 `unsupported` 错误，不尝试解析

验收：

- 在 v2.1、v2.2 的真实文件上输出正确
- footer 损坏 / magic 错误时返回可读错误
- v1 文件返回 `unsupported` 错误（退出码非 0）
- 不读取任何 page buffer（用 I/O 计数或代码审查确认）
- `--format json` 字段稳定

---

## M2 - `ldp schema`

**状态：已完成**

完成于：按 `parent_id` 建树，输出 field id、名称、逻辑类型、nullable、字段
metadata、schema metadata，text / json 两种格式。测试覆盖嵌套 list、字段
metadata、孤儿节点与循环 parent 链。

> 说明：proto 里的 `Field.type`（PARENT/REPEATED/LEAF）在 v2 写入端恒为 0
> （`FieldsWithMeta` 转换时固定 `r#type: 0`），读端也忽略它。因此 schema 输出
> 不展示这个字段，层级关系由 `parent_id` 与 `logical_type` 表达。

目标：渲染 M1 已装载的 schema。

范围：

- 按 `parent_id` 建树
- 输出 field id、名称、逻辑类型、nullable、字段 metadata
- 输出 schema 级 metadata
- parent 链损坏时退化为平铺列表，不失败

验收：

- 嵌套 struct / list / map / fixed-size-list 的文件结构正确
- 带字段 metadata 的文件正确显示
- 与 `info` 中的字段数一致

---

## M3 - `ldp columns`

**状态：未完成**

目标：列级摘要，不读页字节。

范围：

- 每列：页数、页行数之和、列级 buffer 数、字节数、列编码类别、布局种类
- `--column N` 过滤
- 不存在的列索引给出可用范围

验收：

- 列行数与 footer 的 `num_columns` 一致
- `--column N` 输出与全量输出的对应行一致
- 不解码页布局（布局种类由 M4 提供后补齐）

---

## M4 - `ldp pages`

**状态：已完成**

完成于：`Any` → `PageLayout` / legacy `ArrayEncoding` 解码，页级表格输出
（column / page / position / bytes / rows / priority / buffers / layout），
`--column N` 过滤，text / json 两种格式。测试覆盖 v2.0（legacy）、v2.1
（mini-block u16）、v2.2（constant + mini-block u32）。

目标：第一次解码 `Page.encoding`（`Any` → `PageLayout`）。

范围：

- 每页：column、page、position、bytes、rows、priority、buffer 数、布局种类与参数摘要
- 支持 `MiniBlock` / `FullZip` / `Sparse` / `Constant` / `Blob`
- v2.0 legacy `ArrayEncoding` 只做类型摘要
- `--column N` 过滤

验收：

- 未知/无法解码的编码变成 warning，不中断
- 布局摘要与 `show` 命令一致
- 仍不读取页 buffer

---

## M5 - `ldp show`

**状态：已完成**

完成于：页 buffer 读取与角色标注、完整 `PageLayout` 参数输出、mini-block
chunk 表（2B/4B 元数据、chunk header、rep/def/value 区间、repetition index）、
full-zip repetition index（自动推断 1/2/4/8 字节宽度）、`--hex` / `--hex-limit`
/ `--raw` / `--detail`，text / json 两种格式。`--detail` 会解码 mini-block 的
rep/def levels、value 数组和 value buffer sizes（flat / RLE / inline 与
out-of-line bitpacking / fixed-size-list / variable；其它压缩降级为 warning）。
变长值输出为 UTF-8 文本（不可打印时输出十六进制）及其字节数。full-zip 页
通过 control word 解码 rep/def、逐 item 大小和 value 数组。测试覆盖 v2.1 mini-block、v2.2 mini-block+repdef、
v2.2 constant、v2.0 legacy，以及 hex/raw。

目标：单页完整解剖。

范围：

- 解码完整 `PageLayout`：压缩方式、rep/def layers、item 数、字典、rep index 深度
- 页 buffer 及角色：chunk metadata / chunk data / dictionary / rep index / zipped values
- 选择方式：`--page K`，K 是 `ldp pages` 输出的全局 `#`（物理顺序）
- mini-block chunk 表：
  - 2 字节元数据（v2.1）/ 4 字节（v2.2+）
  - chunk offset/size、值数、chunk header、rep/def/value 区间
- mini-block repetition index（每 chunk `depth + 1` 个 u64）
- full-zip repetition index（字节打包宽度 + 哨兵）
- `--hex` / `--hex-limit` / `--raw`

验收：

- 在 v2.1 与 v2.2、且含 list（rep/def）的页上输出自洽：
  - chunk 覆盖长度等于 data buffer 长度
  - chunk 值数之和等于 `num_items`
  - rep index 行数之和等于页行数
- 不一致只报 warning，退出码仍为 0
- `--raw` 输出可解回同一布局

---

## M6 - `ldp chunks`

**状态：未完成**

目标：把 mini-block chunk 表作为独立输出，便于对照格式规范。

范围：

- 只接受 mini-block 页，其它布局给出明确错误
- 输出元数据宽度、chunk 数、item/value 总数
- 每 chunk：offset、bytes、values、log2、levels、rep/def 字节、header、value buffer 大小
- rep index 两列语义化：`rows ending`、`leftover items`

验收：

- 输出与 `show` 内嵌的 chunk 表完全一致
- 非 mini-block 页返回非 0 退出码

---

## M7 - `ldp check`

**状态：未完成**

目标：复用以上解析，做结构校验。

范围：

- magic / 版本
- 区域顺序、CMO/GBO 表大小
- 列元数据与 global buffer 边界
- 页 buffer 64B 对齐与边界
- 所有页布局可解
- 所有 mini-block chunk 表可解

验收：

- 每个检查项有 `ok` / `warn` / `error` 状态
- 有 `error` 时退出码非 0
- 在已知损坏样本（旧版本 test_data）上给出错误而不是 panic
- `--format json` 逐项输出

---

## 开工前待定

以下决策会影响 M1 的实现方式，需先确认：

1. **依赖策略**：复用 `lance-file` / `lance-encoding`，还是独立解析容器与 protobuf。
2. **proto 组织**：vendor 哪些 `.proto`，CI 里如何处理 `protoc`。
3. **`show` 默认深度**：默认完整解剖，还是需要 `--deep` 才展开 chunk 表。
4. **`columns` / `pages`**：保持两个命令，还是合并成 `ldp list`。
