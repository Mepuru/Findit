# Findit 性能审查报告

- **审查对象**: Findit（Flutter UI + flutter_rust_bridge v2 嵌入式 Rust 核心 + SQLite 本地存储）
- **审查日期**: 2026-08（三视角并行审查 · 性能线）
- **审查方式**: 静态代码审查（只读，未修改任何业务代码；未进行真机基准测量，涉及量级判断处均标注「待验证」）
- **审查范围**: `rust/src/core/**`（db / repo / search / photo / backup / ai）、`rust/src/api/**`（FRB 桥接）、`lib/src/pages/**`、`lib/src/widgets/**`、`lib/main.dart`

---

## 一、执行摘要

Findit 的架构基线是健康的：备份导出/恢复全程 64KB 流式打包、向量回填采用「锁内取待办 → 锁外网络 → 锁内写回」三段式保证网络调用不持全局数据库锁、WAL + `synchronous=NORMAL` 配置正确、列表页普遍使用 `ListView.separated` 懒构建、分类加载已做批量 `IN` 查询（无 N+1）、搜索页有防抖与请求序号防过期响应。这些设计值得肯定。

但存在 **3 个高危性能问题**，集中在三处热路径：

1. **照片保存管线在全局数据库锁内执行**全分辨率解码 + Lanczos3 双重缩放 + JPEG 双编码（H1）——一次拍照入库可阻塞整个 App 的所有数据库操作数百毫秒至数秒；
2. **AI HTTP 客户端每次请求重建**（H2）——`reqwest::blocking::Client` 每次请求新建，无连接复用；向量回填逐批重复建客户端 + TLS 握手，千级物品回填被显著拖慢；
3. **语义搜索每次查询全量扫描全部 embedding**（H3）——每次搜索都从 SQLite 全量读出所有向量 BLOB 并逐条计算余弦相似度，无缓存、无索引；与搜索页对同一查询执行两次搜索（M2）叠加，大库下每次按键搜索代价很高。

另有 4 个中等与若干低危问题（照片路径逐条串行 FRB 调用、写入后无条件触发全库向量回填、SAF 恢复整包读入内存、同步长任务占用 FRB worker、写入未事务化等）。修复优先级见文末改进路线。

---

## 二、发现清单

### 高（High）

#### H1. 照片保存管线在全局数据库锁内执行重 CPU/内存工作（解码 + 双重缩放 + 双编码）

- **问题描述**: `api::photos::save_item_photo` 把整条照片压缩管线包在 `with_conn` 持锁闭包内执行；`core::photo::save_photo_bytes` 内部对原始图片做**全分辨率解码**（`image::load_from_memory`），再对主图（1600px）与缩略图（256px）**各做一次 Lanczos3 缩放**并分别 **JPEG 编码**。整个过程（解码 + 2×缩放 + 2×编码 + 磁盘写入 + 数据库更新）都在持有全局数据库 `Mutex<Connection>` 期间完成。
- **证据**:
  - `rust/src/api/photos.rs:10` — `with_conn(|conn| photo::save_item_photo(conn, item_id, &bytes))`
  - `rust/src/core/photo.rs:86-108` — `save_photo_bytes`：`image::load_from_memory` → `resize_to_fit` ×2 → `encode_jpeg` ×2（`write_jpeg` 落盘）
  - `rust/src/core/photo.rs:54-64` — `resize_to_fit` 使用 `FilterType::Lanczos3`（最昂贵的滤波）
  - `rust/src/core/db/mod.rs:104-120` — 全局单连接互斥锁，`with_conn` 持锁期间所有其它数据库操作排队
- **影响**: 保存一张 1200 万像素照片通常耗时 0.5–2 秒（低端机更久），期间全局数据库锁被持有：物品列表加载、搜索、录入、设置读取等**所有**数据库操作全部阻塞等待。用户拍照入库时若同时浏览/搜索，会明显感知卡顿；照片越多越大越严重。
- **改进建议**: 把「压缩落盘」移出持锁闭包——先锁外完成解码/缩放/编码与文件写入（`save_photo_bytes` 本身不碰数据库），再以短锁内的一次 `UPDATE items SET photo_path` 收尾；或将 `save_item_photo` 拆成「锁外压缩 → 锁内登记」两段，与回填三段式同样的锁边界纪律。

#### H2. AI HTTP 客户端每个请求新建 `reqwest::blocking::Client`，无连接复用

- **问题描述**: `HttpAiTransport::post_once` 在**每次请求**内部调用 `reqwest::blocking::Client::builder()...build()` 新建客户端。`reqwest` 的 blocking Client 每个实例自带一个内部 tokio 运行时与连接池，逐请求重建意味着：无 keep-alive 连接复用、每次请求重建运行时、OpenAI 场景每次重新 TLS 握手。上层又在每次 API 调用处新建 `HttpAiTransport`，双重重复构建。向量回填按批（24 条/批）串行循环，N 批 = N 次全新客户端。
- **证据**:
  - `rust/src/core/ai/client.rs:278-281` — `post_once` 内 `reqwest::blocking::Client::builder().timeout(timeout).build()`
  - `rust/src/core/ai/client.rs:305-345` — `chat` / `embed` 每轮重试都调用 `post_once`（网络错误重试也各建一次客户端）
  - `rust/src/api/ai.rs:86, 174, 228, 250, 303` — 每次 API 调用 `HttpAiTransport::new()`
  - `rust/src/api/ai.rs:255-263` / `rust/src/core/ai/embed.rs:19` — 回填 `max_rounds = total/24+4` 轮，每轮一次 embed 请求
- **影响**: 千级物品全量回填 = 数百次「新建运行时 + 建连（+TLS 握手）」；局域网 Ollama 每次请求重新 TCP 建连；OpenAI 每次请求重新 TLS 握手，批量回填总耗时可观地被放大。对话链路（快速录入解析、探活）同样逐次重建。
- **改进建议**: `HttpAiTransport` 持有**单个** `reqwest::blocking::Client`（`OnceLock`/`static`，构建一次复用连接池）；或改用 `reqwest::Client`（异步）与 FRB 运行时配合。客户端构建失败仅发生在初始化时，便于统一错误处理。

#### H3. 语义搜索每次查询全量读取全部 embedding 并逐条计算余弦相似度，无缓存无索引

- **问题描述**: `semantic_search` 每次查询执行 `SELECT id, embedding FROM items WHERE embedding IS NOT NULL`，把**全部**向量 BLOB 从 SQLite 读出，逐条 `blob_to_embedding` 解码并计算与查询向量的余弦相似度（含每次重复计算存储向量的范数），再做阈值过滤与排序。全程在 `with_conn` 持锁闭包内（`api/search.rs:16`）。
- **证据**:
  - `rust/src/core/search/hybrid.rs:74-113` — `semantic_search` 全量读取 + 全量计算
  - `rust/src/core/search/semantic.rs:13-29` — `cosine_similarity` 每次重算 `norm_b`
  - `rust/src/api/search.rs:16` — `with_conn(|conn| search::hybrid::search(...))` 持锁执行
- **影响**: 万级物品 × 384/768/1024 维向量 = 每次搜索从磁盘读出数十 MB BLOB 并做 O(N×D) 浮点运算，单次数十至数百毫秒（待验证，依赖设备存储与向量规模）；期间还持有全局锁。搜索页 300ms 防抖后每键触发一次，体验随库存增长快速劣化。
- **改进建议**: ① 内存缓存（item_id → 向量）并在写入/删除时增量维护，预计算并缓存各向量范数；② 引入向量索引（如 sqlite-vec）或至少将语义扫描移出持锁闭包；③ 对语义结果做 top-K 截断。

---

### 中（Medium）

#### M1. 关键词搜索为 `lower() LIKE '%词%'` 全表扫描 + 每物品 EXISTS 子查询，无 FTS

- **问题描述**: `search_keyword` 对每个分词生成 `(lower(i.name) LIKE ? OR lower(i.description) LIKE ? OR EXISTS(分类 LIKE ?))` 子句，词间 AND。`%词%` 前缀无法命中索引，`lower()` 包裹列也禁止索引，实质是全表扫描 items 并对每行做 EXISTS 分类子查询；无结果条数上限。
- **证据**: `rust/src/core/search/keyword.rs:50-100`（`search_keyword` 主查询），`rust/src/core/search/keyword.rs:34-46`（`like_pattern`）
- **影响**: 家庭收纳典型规模（数千件）可接受；但万级以上每次搜索为全表 LIKE 扫描 × 词数，且与 M2 的重复调用叠加，延迟翻倍。
- **改进建议**: 引入 FTS5（中文可用 trigram tokenizer 或字符 n-gram 分词），以 `MATCH` 替代 LIKE；短期内可对高频查询结果做 LRU 缓存。

#### M2. 搜索页对同一查询执行两次搜索（关键词搜索被重复执行）

- **问题描述**: `SearchPage._search` 先以**无向量**调用 `api.searchItems` 出关键词结果；向量返回后再次调用 `api.searchItems`（带向量），而 `hybrid::search` 在语义通道之外**又执行一遍完整的关键词搜索**。即一次用户搜索 = 2 次关键词全表扫描 + 1 次语义全量扫描。
- **证据**:
  - `lib/src/pages/search_page.dart:70` — `await api.searchItems(query: query)`（无向量）
  - `lib/src/pages/search_page.dart:80` — `await api.searchItems(query: query, embedding: embedding)`
  - `rust/src/core/search/hybrid.rs:24-71` — `search()` 先语义后 `keyword::search_keyword` 再合并
- **影响**: 大库下每次搜索的关键词扫描代价翻倍；语义通道本身就是全量扫描（H3），叠加后延迟明显。
- **改进建议**: Dart 侧只调用一次带向量的混合搜索（向量未就绪时可用 `null` 快速降级，但避免「先无向量、后有向量」两次完整搜索）；或在 Rust 侧缓存同一查询的关键词结果。

#### M3. 物品列表加载时逐物品串行 FRB 调用解析照片路径

- **问题描述**: `ItemsPage._reload` 对每个带照片的物品在 `for` 循环内**串行** `await` 两次 FRB 调用（`getThumbFullPath` + `getPhotoFullPath`）。100 件带照片物品 = 200 次串行 FFI 往返。
- **证据**: `lib/src/pages/items_page.dart:55-65`（`for (final item in items) { ... await getThumbFullPath; await getPhotoFullPath; }`）
- **影响**: 照片多的箱子打开时首屏加载明显变慢（每次 FRB 往返有固定开销，串行放大）。
- **改进建议**: `Future.wait` 并行解析；更优是新增 Rust 批量 API（一次调用返回 `photo_path → 全/缩略图路径` 映射），从根上消除逐条往返。

#### M4. 每次物品写入后无条件触发全库向量回填（无防抖/无去重，并发触发重复拉取同一批待办）

- **问题描述**: 物品创建/修改成功、快速录入确认、App 启动三处都会 fire-and-forget 调用 `backfillPendingEmbeddings()`，该函数循环处理**直到没有 pending**。AI 已配置时：① 每次写入都可能发起 1..N 次网络 embedding 调用；② 快速连续录入时多个回填并发执行，两轮可能取到同一批 pending（先读后写，无互斥）→ 重复 embedding 请求；③ 启动时若库内存在大量未回填物品，会长时间占用 FRB 线程并消耗流量/电量。
- **证据**:
  - `lib/src/pages/items_page.dart:117-118`、`lib/src/pages/quick_add_page.dart:157-159`、`lib/main.dart:42-44`
  - `rust/src/api/ai.rs:243-263` — `backfill_pending_embeddings` 循环直到 `n == 0`
  - `rust/src/api/ai.rs:268-283` — 三段式本身不持锁做网络（这点是好的），但无并发互斥
- **影响**: 录入多件物品时产生重复网络请求；回填期间 FRB worker 被占用；未配置 AI 时调用快速失败（代价可忽略，`api/ai.rs:245-249`）。
- **改进建议**: 回填改为单飞（single-flight，`Mutex<Option<JoinHandle>>`）+ 防抖（如最后一次写入后延迟 2–5s 合并触发）；启动时只触发一次且可被手动补齐替代。

#### M5. 恢复备份对无本地路径的 zip（SAF content://）整体 `readAsBytes()` 读入内存

- **问题描述**: `SettingsPage._restore` 在 `picked.path == null` 分支用 `staging.writeAsBytes(await picked.readAsBytes())` 把整个备份 zip 读入 Dart 内存再写盘。备份上限允许 1GB（`restore.rs` 的 `DEFAULT_RESTORE_LIMITS.max_zip_bytes`）。
- **证据**: `lib/src/pages/settings_page.dart:218-222`；`rust/src/core/backup/restore.rs:52-56`
- **影响**: 数百 MB 备份在恢复时 Dart 堆内存峰值飙升，低内存 Android 设备可能 OOM。
- **建议**: content:// 场景用流式复制（`InputStream` 分块写入临时文件），或让 file_picker 返回可流式读取的句柄。

#### M6. 同步 FRB 长任务（向量重建 / 备份导出 / 恢复）占用 normal 线程池 worker

- **问题描述**: `rebuild_embeddings`、`export_backup`、`restore_backup` 均为**同步** FRB 函数（`frb_generated.dart` 走 `executeNormal`）。向量重建全程串行网络调用，且 `retry_on_network` 用 `std::thread::sleep` 阻塞退避（1s/3s）；导出照片数多时打包耗时长。normal 池线程有限，多个此类长任务并发时可能排队。
- **证据**:
  - `rust/src/api/ai.rs:288` — `pub fn rebuild_embeddings(...)`（非 async）
  - `rust/src/api/backup.rs:23, 77` — `export_backup` / `restore_backup`（同步）
  - `rust/src/core/ai/client.rs:202-220` — 重试退避 `std::thread::sleep`
- **影响**: 不阻塞 UI isolate（这点安全），但长任务独占一个 worker；normal 池被占满时其它同步调用（如设置读取）排队。重建千级向量可能持续数分钟。
- **改进建议**: `rebuild_embeddings` 改为 async 并复用 `backfill_one_round`（对齐现有 async 回填路径）；重试退避改非阻塞等待。

#### M7. 照片解码无尺寸上限，大图峰值内存为原图 RGBA 的数倍

- **问题描述**: picker 返回原图（无 `maxWidth`/`imageQuality` 限制），Dart 侧 `readAsBytes()` 整读，Rust 侧 `image::load_from_memory` 全分辨率解码（4800 万像素 ≈ 192MB RGBA），随后两次 Lanczos3 缩放还会产生中间缓冲，峰值内存 ≈ 原图 + 主图 + 缩略图。
- **证据**: `lib/src/pages/items_page.dart:513-517`（`ImagePicker().pickImage` + `readAsBytes`）；`rust/src/core/photo.rs:90-104`
- **影响**: 高像素手机照片入库时内存峰值可达数百 MB（待验证，与像素数线性相关），低端机有 OOM 风险；同时加重 H1 的持锁时长。
- **改进建议**: picker 侧设置 `maxWidth: 4096` 或 `imageQuality`；Rust 侧利用 JPEG 解码器的 scale-down 解码（如 1/2、1/4、1/8）先降采样再处理，或对超大图先快速缩小再 Lanczos 精调。

---

### 低（Low）

#### L1. 向量写回逐条 UPDATE，未包事务

- **证据**: `rust/src/core/ai/embed.rs:66-77` — `write_item_embeddings` 循环 `conn.execute("UPDATE items SET embedding=...")`
- **影响**: 每批 24 条 = 24 个独立 autocommit 提交，写放大；回填总量大时累计明显。
- **建议**: 单事务内批量执行（`unchecked_transaction` + prepared statement）。

#### L2. 待回填扫描 `WHERE embedding IS NULL ORDER BY id LIMIT 24` 无索引，全量回填近似 O(N²) 行访问

- **证据**: `rust/src/core/ai/embed.rs:43-51`
- **影响**: 万级物品全量重建时每轮从头部重扫，扫描开销显著（待验证，分钟级可感知）。
- **建议**: 建部分索引 `CREATE INDEX idx_items_pending_embedding ON items(id) WHERE embedding IS NULL`。

#### L3. 物品写入多语句未事务化（create_item / replace_item_categories / update_item）

- **证据**: `rust/src/core/repo/items.rs:118-126`（`create_item`：INSERT + `replace_item_categories` 的 DELETE + N×INSERT，无显式事务）；`rust/src/core/repo/items.rs:89-98`；`rust/src/core/repo/items.rs:185, 226`（`update_item` 前后各一次 `get_item`）
- **影响**: 分类多的物品一次保存 10+ 次独立提交；进程中断时可能残留孤儿 `item_categories` 行（数据一致性小隐患）。
- **建议**: 包单事务；去除冗余的重复 `get_item`。

#### L4. 导出备份的 `VACUUM INTO` 在全局锁内执行

- **证据**: `rust/src/api/backup.rs:44`（`with_conn(create_snapshot)`）；`rust/src/core/backup/export.rs:56-78`（`wal_checkpoint(TRUNCATE)` + `VACUUM INTO`）
- **影响**: 大库（数百 MB）时快照生成持锁数秒，期间所有数据库操作阻塞。备份频率低，影响有限。
- **建议**: 快照改用独立连接（对 `db_dir/findit.db` 再开一个连接执行 checkpoint + `VACUUM INTO`），避免持全局锁。

#### L5. 搜索结果列表一次性构建全部卡片（非懒构建）

- **证据**: `lib/src/pages/search_page.dart:194-214` — `ListView(children: [...for 语义...for 关键词...])`
- **影响**: 结果数百条时首帧构建全部卡片，滚动与首帧开销随结果数线性增长。
- **建议**: `CustomScrollView` + `SliverList` 分段懒构建。

#### L6. 大图预览未限制解码尺寸

- **证据**: `lib/src/pages/photo_viewer_page.dart:52-54` — `Image.file(File(photoPath))` 未设 `cacheWidth`
- **影响**: 主图 1600px 全尺寸解码；Flutter 图片缓存可缓解重复打开，低端机峰值内存略高。
- **建议**: 按屏幕宽度传 `cacheWidth`（如 1080–1440）。

#### L7. 全局单连接 + Mutex 串行化全部数据库访问，且未调优 cache_size

- **证据**: `rust/src/core/db/mod.rs:18, 104-120`；`rust/src/core/db/mod.rs:46-51`（仅设置 WAL / synchronous / busy_timeout / foreign_keys，无 `cache_size` / `mmap_size`）
- **影响**: WAL 本可支持并发读，但单连接 Mutex 使所有读串行；与 H1/M1 的长持锁叠加放大阻塞。默认页缓存 2MB，语义全量读向量（H3）时缓存抖动。
- **建议**: 长期拆读写连接（读连接池 + 写连接）；按需 `PRAGMA cache_size` 调优。

#### L8. delete_unit 逐箱循环删除

- **证据**: `rust/src/core/repo/units.rs:163-171` — `for box_id in &box_ids { DELETE item_categories ...; DELETE items ...; }`
- **影响**: 语句数随箱数线性增长；事务内执行，删除大单元时略慢。
- **建议**: 用 `IN (SELECT ...)` 子查询合并为单条 DELETE。

---

## 三、按优先级排序的改进路线

### P0 · 短期快速修复（改动小、收益直接）

1. **H1 — 照片压缩移出持锁闭包**：`save_item_photo` 拆成「锁外压缩落盘 → 短锁内登记 photo_path」两段。这是收益最大的一项，直接消除拍照入库期间全 App 数据库阻塞。
2. **H2 — 复用 HTTP 客户端**：`HttpAiTransport` 持有单例 `reqwest::blocking::Client`，消除逐请求重建运行时与连接。
3. **M2 — 去掉搜索页重复查询**：只保留一次带向量的混合搜索调用（或 Rust 侧合并关键词结果），立即减半关键词扫描开销。
4. **M3 — 照片路径解析并行化**：`Future.wait` 或新增 Rust 批量 API，消除逐条串行 FFI。
5. **L3 — 物品写入事务化**：`create_item` / `replace_item_categories` 包事务，顺带去除冗余 `get_item`。

### P1 · 中期优化（结构性改进）

6. **H3 — 语义搜索缓存与索引**：内存缓存向量 + 预计算范数；评估 sqlite-vec；语义扫描移出持锁闭包。
7. **M4 — 回填单飞 + 防抖**：写入触发改为合并/延迟触发，杜绝并发重复拉取同一批待办；启动回填做 pending 数判断。
8. **M5 — 恢复备份流式复制**：content:// 分块写盘，避免整包入内存。
9. **M1 — 引入 FTS5**：关键词搜索从 LIKE 全表扫描升级为 FTS5（trigram/n-gram 分词），并给搜索结果加上限。
10. **M7 — 照片解码降采样**：picker 限尺寸 + Rust 侧 scale-down 解码，控制内存峰值。
11. **M6 — 同步长任务改 async**：`rebuild_embeddings` 对齐 async 回填路径，重试退避非阻塞化。

### P2 · 长期演进（数据规模增长后必做）

12. **L7 — 读写连接拆分 + cache 调优**：读连接池 + 写连接，充分发挥 WAL 并发读能力。
13. **L2 — 部分索引**：`items(id) WHERE embedding IS NULL`，全量回填从近似 O(N²) 降为 O(N)。
14. **L4 — 备份快照独立连接**：`VACUUM INTO` 不再占用全局锁。
15. **L5 / L6 — UI 懒构建与解码尺寸**：搜索结果 SliverList 化、大图 cacheWidth 限制。
16. **L8 — delete_unit 合并删除语句**。

### 验证建议（后续可执行）

- 用「5000 件物品 + 500 张照片 + 768 维向量」的合成库做真机基准：列表首屏时间、拍照入库时其它操作延迟、语义搜索单次延迟、全量回填总时长（当前为静态审查，以上量级判断均属「待验证」）。
- 用 Android Profiler / Dart DevTools 观测照片保存与搜索期间的堆内存峰值与主 isolate 帧率。

---

*本报告基于静态代码审查，所有证据均为真实读取到的代码位置；性能量级判断（毫秒/秒级、内存峰值）未经真机测量，标注「待验证」处建议以基准测试确认。*
