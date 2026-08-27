# Findit 综合审查报告（三视角汇总）

- **项目名称**：Findit（Flutter UI + flutter_rust_bridge v2 嵌入式 Rust 核心 + SQLite 本地存储的隐私优先收纳应用）
- **审查范围**：`rust/src/**`（db / repo / search / photo / backup / ai / api）、`lib/src/**`（pages / widgets / main）、平台配置（`android/`、`ios/`）、依赖清单（`Cargo.lock`、`pubspec.lock`）、README 等文档
- **审查日期**：2026 年（三视角并行审查；源报告日期标注不一致——性能线 2026-08，安全线/产品线 2026-02，以各分线报告为准）
- **审查方式**：静态代码审查（只读，未修改业务代码）。性能线未做真机基准（量级判断标注「待验证」）；产品线未做真机走查（交互结论基于代码路径推演）
- **汇总人**：报告汇总员（findit-review-team）
- **源报告**：
  - 性能：`docs/review/performance.md`
  - 安全：`docs/review/security_review.md`
  - 产品：`docs/review/product_review.md`

---

## 0. 三线审查结论一览

| 视角 | 一句话结论 | 问题数 |
| --- | --- | --- |
| 性能 | 架构基线健康（备份流式打包、回填三段式锁边界、WAL 配置、批量分类加载无 N+1、列表懒构建），但「照片保存持全局锁」「语义搜索全量扫描」两大热路径问题需优先处理 | 3 高 / 7 中 / 8 低 |
| 安全 | 代码质量基础好（全参数化 SQL 无注入、备份恢复校验链完整、照片重编码自动剥离 EXIF、无遥测无硬编码密钥），但作为隐私优先产品存在签名不可信、密钥明文、备份漏剔三大发布前缺口 | 3 高 / 5 中 / 4 低 / 2 提示 |
| 产品 | 核心功能链完整、工程质量高（空态/错误/降级/备份安全均到位），但「二维码标签扫码直达」与「AI 修改物品」两大核心卖点存在断点 | 2 高 / 5 中 / 7 低 |

**合计 46 项问题：8 高 / 17 中 / 19 低 / 2 提示。**

---

## 1. 总执行摘要

### 1.1 整体健康度评价

Findit 是一个**工程质量良好、功能链完整**的隐私优先本地应用：数据库访问全部参数化（无 SQL 注入），备份恢复实现了完整的 zip-slip / zip-bomb / 完整性校验链，照片重编码自动剥离 EXIF/GPS，无遥测无埋点，AI 网络调用严格在全局数据库锁外执行（三段式锁边界），UI 三态（空态/加载/失败）与中文错误提示齐全。**架构与工程层面属于「可发布但要先补齐关键收尾」的状态。**

主要风险集中在三个方向：

1. **发布与密钥信任链**（安全 H1/H2/H3）：release 用 debug 签名、API Key 明文落盘、备份漏剔向量 Key——这三项直接动摇「隐私优先」的定位承诺；
2. **热路径性能**（性能 H1/H2/H3）：照片入库阻塞全库、AI 客户端无连接复用、语义搜索全量扫描——随数据规模增长会明显劣化体验；
3. **核心卖点断点**（产品 F1/F2）：二维码标签无法被外部扫码唤起 App、AI 修改物品存在改错对象风险——两者都是 README 主打能力与真实体验的落差。

### 1.2 最值得优先处理的跨线高优先级问题（TOP 10，按风险/影响排序）

| # | 问题 | 所属线 | 级别 | 一句话影响 |
| --- | --- | --- | --- | --- |
| 1 | release 构建使用 debug 签名，分发 APK 可被任何人用公开 debug 密钥伪造同签名「更新」覆盖安装 | 安全 H1 | 高危 / P0 | 发布链路无签名信任，可被整体替换 |
| 2 | 备份导出只剔除 `ai_api_key`，漏剔 `ai_embed_api_key`，与 README「API Key 不会写入备份文件」承诺不符 | 安全 H3 | 高危 / P0 | 隐私承诺违约，现有测试漏检 |
| 3 | 照片保存管线在全局数据库锁内执行全分辨率解码 + Lanczos3 双缩放 + 双编码 | 性能 H1 | 高危 / P0 | 拍照入库期间阻塞全 App 所有数据库操作数百毫秒~数秒 |
| 4 | AI API Key（含向量独立 Key）明文存 SQLite，数据库/照片均未加密落盘 | 安全 H2 | 高危 / P1 | 密钥随数据文件在多个层面明文离开设备/暴露给本机主体 |
| 5 | 二维码标签未注册系统级深链（`findit://` 无 intent-filter / CFBundleURLTypes），外部扫码无法唤起 App | 产品 F1 | 高 / P1 | 「打印标签→随手扫码直达」核心卖点闭环断裂 |
| 6 | AI「修改物品」预览不展示目标物品，取关键词搜索第一个命中即修改 | 产品 F2 | 高 / P1 | 同名/相似物品存在改错对象风险，且预览阶段不可见 |
| 7 | 语义搜索每次查询全量读取全部 embedding 并逐条计算余弦相似度，无缓存无索引且持全局锁 | 性能 H3 | 高危 / P1 | 万级物品时单次搜索数十~数百毫秒，随库存增长快速劣化 |
| 8 | AI HTTP 客户端每个请求重建 `reqwest::blocking::Client`，无连接复用，千级物品回填被显著拖慢 | 性能 H2 | 高危 / P1 | 批量回填 = 数百次「新建运行时 + 建连 + TLS 握手」 |
| 9 | AI 数据外发缺少告知与最小化：启动即静默回填语义向量（物品文本自动外发）+ 每次写入无条件触发回填、无防抖无互斥 | 安全 M4 + 性能 M4 | 中 / P1 | 用户无感知的数据外发 + 重复网络请求与流量/电量消耗 |
| 10 | 物品无法在普通编辑界面移动收纳箱，移动能力完全依赖 AI，无 AI 用户无途径移箱 | 产品 F3 | 中 / P1 | 收纳最高频操作之一对无 AI 用户不可用 |

---

## 2. 跨线交叉主题

三线报告在以下主题上存在互相印证的横切发现，建议按主题打包治理，而非逐条孤立修复。

### 2.1 数据安全与备份可靠性

- **核心矛盾**：备份/云同步会把明文密钥和照片带出设备。安全线发现密钥明文存储（S-H2）、备份 zip 不加密（S-M1）、备份漏剔向量 Key（S-H3）、Android Auto Backup / iOS iCloud 默认上云（S-M3）、恢复后旧数据明文副本遗留（S-L1）；性能线发现恢复备份整包读入内存（P-M5）、快照 `VACUUM INTO` 持锁（P-L4）；产品线发现备份导出分享面板 UX 与文档不一致、取消分享即丢失（F10）、无自动备份提醒（F14）。
- **建议**：把「备份内容剔除全部密钥类设置 + 可选加密 + 云备份排除」作为一条发布前硬性收尾链处理，产品侧同步修订 README 表述（F10 与安全 H3 互相印证：文档承诺与实际行为不一致）。

### 2.2 AI 功能对隐私 / 性能 / 体验的连带影响（三线交汇最密集）

- **隐私侧**（安全线）：启动静默回填外发物品文本且无开关无告知（S-M4）、密钥明文存储（S-H2）、语音输入依赖系统服务未披露（S-I2）、AI 服务端错误片段回显给用户（S-L4）。
- **性能侧**（性能线）：HTTP 客户端逐请求重建（P-H2）、语义搜索全量扫描（P-H3）、写入后无条件触发回填且并发重复拉取同一批待办（P-M4）、向量重建为同步长任务占 FRB worker（P-M6）。
- **体验侧**（产品线）：AI 修改预览不展示目标物品（F2）、快速录入解析期无取消、黑洞地址可致分钟级假死（F6）。
- **结论**：**AI 是最大的横切风险面**。三线共同的治理方向是「显式开关 + 告知文案 + 数据最小化 + 性能护栏」成套落地（语义搜索默认关、回填单飞防抖、客户端复用、预览可见、超时可控）。

### 2.3 照片处理链路

- 性能线：照片管线持全局锁（P-H1）、解码无尺寸上限内存峰值高（P-M7）、列表逐条串行 FRB 解析照片路径（P-M3）、大图预览无 `cacheWidth`（P-L6）。
- 产品线：建档时不能直接拍照，录入+留档被拆两步（F7）。
- 安全线（亮点确认）：照片重编码自动剥离 EXIF/GPS、主图限 1600px 缩略图 256px，隐私友好。
- **结论**：照片既是隐私资产又是性能热点。建议「拍照建档一体化（F7）+ 锁外压缩（P-H1）+ 解码降采样（P-M7）+ 路径解析并行化（P-M3）」组合优化，一举改善留存率与性能。

### 2.4 搜索体验与性能

- 性能线：关键词搜索 LIKE 全表扫描无 FTS5（P-M1）、同一查询执行两次搜索（P-M2）、语义全量扫描（P-H3）、结果列表非懒构建（P-L5）。
- 产品线：结果不直达物品、无缩略图、「语义搜索已上线」在未配置 AI 时误导（F5）。
- **结论**：搜索体验与搜索性能强绑定。语义通道当前的全量扫描实现无法支撑规模化，建议「FTS5 + 向量缓存/索引 + 一次查询 + 直达高亮 + 动态提示」整体演进。

### 2.5 平台配置与发布就绪（Android 优先，iOS 未就绪）

- 安全线：debug 签名（S-H1）、全局明文 HTTP（S-M2）、云备份默认开启（S-M3）、iOS 权限描述缺失 + ATS 拦截（S-M5）。
- 产品线：深链缺失（F1）、iOS 权限描述缺失（F12）、manifest 中文注释乱码 + namespace 与 applicationId 不一致（F13）。
- **结论**：Android 发布前需完成「签名、明文流量收敛、云备份排除、深链注册」四项平台配置审计；iOS 当前处于不可用状态（权限描述缺失 + ATS），需明确是否纳入路线图，否则在文档中声明仅支持 Android。

---

## 3. 分线摘要表（修复 Backlog）

> 编号前缀：`P-` 性能线（对应源报告 H/M/L）、`S-` 安全线（对应源报告 H/M/L/I）、`F` 产品线（对应源报告 F1–F14）。证据位置为源报告引用的真实代码位置。

### 3.1 性能线（18 项）

| 编号 | 严重度 | 一句话描述 | 证据位置 | 建议 |
| --- | --- | --- | --- | --- |
| P-H1 | 高 | 照片保存管线在全局 DB 锁内执行全分辨率解码 + Lanczos3 双缩放 + 双 JPEG 编码，一次拍照入库阻塞全 App 数据库操作 | `rust/src/api/photos.rs:10`、`rust/src/core/photo.rs:86-108,54-64`、`rust/src/core/db/mod.rs:104-120` | 拆成「锁外压缩落盘 → 短锁内登记 photo_path」两段（与回填三段式同纪律） |
| P-H2 | 高 | AI HTTP 客户端每次请求新建 `reqwest::blocking::Client`，无连接复用，每次重建运行时并重复 TLS 握手 | `rust/src/core/ai/client.rs:278-281,305-345`、`rust/src/api/ai.rs:86,174,228,250,303` | `HttpAiTransport` 持有单例 Client（OnceLock/static），或改异步 Client |
| P-H3 | 高 | 语义搜索每次查询全量读取全部 embedding 并逐条计算余弦相似度，无缓存无索引，且持全局锁 | `rust/src/core/search/hybrid.rs:74-113`、`semantic.rs:13-29`、`rust/src/api/search.rs:16` | 内存缓存向量 + 预计算范数；评估 sqlite-vec；移出持锁闭包；top-K 截断 |
| P-M1 | 中 | 关键词搜索 `lower() LIKE '%词%'` 全表扫描 + 每物品 EXISTS 子查询，无 FTS5、无结果条数上限 | `rust/src/core/search/keyword.rs:50-100,34-46` | 引入 FTS5（trigram/n-gram）；短期对高频查询做 LRU 缓存 |
| P-M2 | 中 | 搜索页对同一查询执行两次搜索（先无向量、后有向量各一次），关键词扫描翻倍 | `lib/src/pages/search_page.dart:70,80`、`rust/src/core/search/hybrid.rs:24-71` | 只调用一次带向量的混合搜索（向量未就绪用 null 降级） |
| P-M3 | 中 | 物品列表逐物品串行 FRB 调用解析照片路径（100 件 = 200 次串行往返） | `lib/src/pages/items_page.dart:55-65` | `Future.wait` 并行；更优是新增 Rust 批量 API |
| P-M4 | 中 | 每次物品写入后无条件触发全库向量回填，无防抖/无互斥，并发触发重复拉取同一批待办 | `lib/src/pages/items_page.dart:117-118`、`quick_add_page.dart:157-159`、`lib/main.dart:42-44`、`rust/src/api/ai.rs:243-263` | 回填单飞（single-flight）+ 防抖（延迟 2–5s 合并触发）；启动只触发一次 |
| P-M5 | 中 | SAF 恢复备份整体 `readAsBytes()` 读入 Dart 内存（备份上限 1GB），低内存设备可能 OOM | `lib/src/pages/settings_page.dart:218-222`、`rust/src/core/backup/restore.rs:52-56` | content:// 场景流式复制（InputStream 分块写临时文件） |
| P-M6 | 中 | 同步 FRB 长任务（向量重建/备份导出/恢复）占用 normal 线程池 worker，重试退避用 `sleep` 阻塞 | `rust/src/api/ai.rs:288`、`rust/src/api/backup.rs:23,77`、`rust/src/core/ai/client.rs:202-220` | `rebuild_embeddings` 改 async 对齐现有回填路径；退避非阻塞化 |
| P-M7 | 中 | 照片解码无尺寸上限，全分辨率解码峰值内存达原图 RGBA 数倍，低端机 OOM 风险 | `lib/src/pages/items_page.dart:513-517`、`rust/src/core/photo.rs:90-104` | picker 限 `maxWidth`/`imageQuality`；Rust 侧 scale-down 解码 |
| P-L1 | 低 | 向量写回逐条 UPDATE 未包事务，写放大 | `rust/src/core/ai/embed.rs:66-77` | 单事务内批量执行 |
| P-L2 | 低 | 待回填扫描 `WHERE embedding IS NULL` 无索引，全量回填近似 O(N²) 行访问 | `rust/src/core/ai/embed.rs:43-51` | 建部分索引 `items(id) WHERE embedding IS NULL` |
| P-L3 | 低 | 物品写入多语句未事务化，进程中断可能残留孤儿 `item_categories` 行；含冗余 `get_item` | `rust/src/core/repo/items.rs:118-126,89-98,185,226` | 包单事务；去除重复 `get_item` |
| P-L4 | 低 | 备份导出 `VACUUM INTO` 在全局锁内执行，大库时持锁数秒 | `rust/src/api/backup.rs:44`、`rust/src/core/backup/export.rs:56-78` | 快照改用独立连接执行 checkpoint + `VACUUM INTO` |
| P-L5 | 低 | 搜索结果列表一次性构建全部卡片，非懒构建 | `lib/src/pages/search_page.dart:194-214` | `CustomScrollView` + `SliverList` 分段懒构建 |
| P-L6 | 低 | 大图预览未限制解码尺寸 | `lib/src/pages/photo_viewer_page.dart:52-54` | 按屏幕宽度传 `cacheWidth`（1080–1440） |
| P-L7 | 低 | 全局单连接 + Mutex 串行化全部数据库访问，未调优 cache_size，WAL 并发读能力未利用 | `rust/src/core/db/mod.rs:18,104-120,46-51` | 长期拆读写连接（读连接池 + 写连接）；按需 `PRAGMA cache_size` |
| P-L8 | 低 | delete_unit 逐箱循环删除，语句数随箱数线性增长 | `rust/src/core/repo/units.rs:163-171` | `IN (SELECT ...)` 子查询合并为单条 DELETE |

### 3.2 安全线（14 项）

| 编号 | 严重度 | 一句话描述 | 证据位置 | 建议 |
| --- | --- | --- | --- | --- |
| S-H1 | 高 | release 构建使用 debug 签名，分发 APK 可被同签名伪造「更新」覆盖安装并读取原数据目录 | `android/app/build.gradle.kts` | 独立 release keystore + `key.properties`（不入库）；发布前 `apksigner verify --print-certs` 核对 |
| S-H2 | 高 | AI API Key（含向量独立 Key）明文存 SQLite，数据库/照片均未加密落盘 | `rust/src/core/ai/config.rs`、`rust/src/core/db/mod.rs` | 密钥改系统级安全存储（Android Keystore / iOS Keychain）；长期 SQLCipher/整库加密；至少密钥不随普通数据文件导出 |
| S-H3 | 高 | 备份导出只剔除 `ai_api_key`，漏剔 `ai_embed_api_key`，与 README 承诺不符；现有测试只覆盖对话 Key | `rust/src/core/backup/export.rs`（`scrub_api_key`）、`rust/src/core/ai/config.rs`（`KEY_EMBED_API_KEY`） | 白名单剔除所有密钥类设置，新增测试断言两个 Key 均空；manifest 加 `secrets_scrubbed` 标记 |
| S-M1 | 中 | 备份 zip 不加密，获取文件即获取全部数据（完整库 + 照片） | `rust/src/core/backup/export.rs`（`write_zip`）、`lib/src/pages/settings_page.dart` | 可选密码加密（AES-256-GCM + Argon2id 派生）；或至少 UI 提示未加密并先剔除向量 Key |
| S-M2 | 中 | Android 全局 `usesCleartextTraffic="true"` + AI 地址不校验协议，`http://` + Key 会明文发密钥 | `android/app/src/main/AndroidManifest.xml`、`rust/src/core/ai/client.rs` | `network_security_config.xml` 仅放行 localhost/局域网；`save_ai_config` 对 http+key 拒绝或强警告 |
| S-M3 | 中 | Android Auto Backup / iOS iCloud 默认上传含明文密钥与照片的数据目录，用户无感知「自动上云」 | `android/app/src/main/AndroidManifest.xml`、`ios/Runner/Info.plist` | `allowBackup="false"` 或 `dataExtractionRules` 排除；iOS 设置 `NSURLIsExcludedFromBackupKey` |
| S-M4 | 中 | 启动即静默回填语义向量，物品文本（名称/备注/分类）自动外发至 AI 服务，无开关无告知 | `lib/main.dart`、`rust/src/core/ai/embed.rs` | 语义搜索显式开关（默认关）+ 明确告知文案；启动回填仅开关开启时执行 |
| S-M5 | 中 | iOS 缺相机/麦克风/相册权限用途描述（调用即崩溃）；默认 Ollama `http://10.0.2.2:11434` 被 ATS 拦截，AI 功能整体不可用 | `ios/Runner/Info.plist` | 补齐三项 usage description；ATS 例外或引导用户填 https/LAN 地址 |
| S-L1 | 低 | 恢复后旧数据明文副本遗留（`{db_dir}.backup-{ts}`），且参与云备份 | `rust/src/core/backup/restore.rs`（`swap_dirs`/`cleanup_old_backups`） | 恢复完成提示可删除副本，或副本目录排除云备份 |
| S-L2 | 低 | 恢复解压 zip 条目数无上限（可构造海量微小条目 DoS）；Dart 侧整包读入内存（OOM 风险） | `rust/src/core/backup/restore.rs`（`extract_all`）、`lib/src/pages/settings_page.dart` | 增加条目数上限（如 10 万）与文件数预算；`openStream` 分块流式复制 |
| S-L3 | 低 | 依赖版本滞后（reqwest 0.12.4 等早期版本）、无 `cargo audit`/Dependabot/密钥扫描门禁 | `rust/Cargo.lock`、`pubspec.lock` | CI 加 `cargo audit`/`cargo deny` + `flutter pub outdated`；升级 reqwest 最新 0.12.x；pre-commit 密钥扫描（gitleaks） |
| S-L4 | 低 | 错误信息向 UI 透传原始细节（AI 服务端响应片段回显、SQLite/路径细节） | `rust/src/core/error.rs`、`lib/src/errors.dart` | 对外层对 `AiModelOutput` 通用化提示；完整错误仅 debug 日志；路径脱敏 |
| S-I1 | 提示 | 无应用锁与防截屏（`FLAG_SECURE`），最近任务缩略图/截屏可能暴露物品清单 | 全应用 | 可选生物识别锁 + 防截屏开关（注意禁用录屏分享） |
| S-I2 | 提示 | 语音输入依赖系统语音服务（内容离开设备），未在 UI 披露 | `lib/src/pages/quick_add_page.dart`（`speech_to_text`） | 首次使用弹隐私说明（「语音将由系统语音服务识别」） |

### 3.3 产品线（14 项）

| 编号 | 严重度 | 一句话描述 | 证据位置 | 建议 |
| --- | --- | --- | --- | --- |
| F1 | 高 | 二维码标签未注册系统级深链（无 `findit://` intent-filter / CFBundleURLTypes），外部扫码无法唤起 App，「扫码直达」闭环断裂 | `AndroidManifest.xml`、`ios/Runner/Info.plist`、`lib/src/pages/scan_page.dart`、README L7-8 | Android 注册深链直达 ItemsPage；iOS 补 `CFBundleURLTypes`；或 README 明确仅 App 内扫码 |
| F2 | 高 | AI「修改物品」预览不展示目标物品，取关键词搜索第一个命中即修改，存在改错对象风险 | `lib/src/pages/quick_add_page.dart` L349-357、`rust/src/core/ai/apply.rs` L130-134 | 预览先检索并展示命中物品（名称/所在箱/命中数），多候选提供选择 |
| F3 | 中 | 物品无法在普通编辑界面移动收纳箱（`update_item` 支持 `box_id` 但 UI 未用），无 AI 用户无移箱途径 | `lib/src/pages/items_page.dart`（`_ItemFormSheet`）、`rust/src/api/items.rs` | 编辑表单增加「所在收纳箱」选择器；无 AI 时快速录入降级为普通表单 |
| F4 | 中 | 分类缺少管理入口：重命名/删除 API 存在但 UI 不可达 | `rust/src/api/categories.rs`、`lib/src/pages/items_page.dart` L457-471 | 新增分类管理页（列表 + 重命名 + 删除） |
| F5 | 中 | 搜索不直达物品（只开整个箱页）、结果无缩略图、「语义搜索已上线」在未配置 AI 时误导 | `lib/src/pages/search_page.dart` L103-113,362-427,245 | 命中直达 + 高亮定位；渲染缩略图；按 `AiStatus.configured` 动态提示 |
| F6 | 中 | 快速录入解析期无取消/超时兜底（对话超时 60s × 重试），局域网黑洞地址可致分钟级假死 | `rust/src/core/ai/client.rs` L16-17、`rust/src/api/ai.rs` L163-191、`lib/src/pages/quick_add_page.dart` | 解析中提供「取消」；快速录入用短超时（15–20s）；错误卡增加「前往 AI 设置」 |
| F7 | 中 | 建档时不能直接拍照，录入+留档被拆成两步，用户可能忘记补拍 | `lib/src/pages/items_page.dart` L726-733 | 创建表单直接支持拍照/选图，或保存后自动进入编辑态补拍 |
| F8 | 低 | 无删除撤销/回收站，级联删除（整单元）不可挽回 | `lib/src/widgets/common.dart`（`confirmDelete`）、`rust/src/core/repo/units.rs` L132-178 | SnackBar 级「撤销」（延迟删除）或软删除/回收站 |
| F9 | 低 | 物品登记/更新时间从未展示 | `lib/src/rust/api/model.dart` L280-296（`createdAt`/`updatedAt`） | 物品卡片副标题或详情展示时间 |
| F10 | 低 | 备份导出经系统分享面板，取消分享即删除暂存文件，与 README「经系统文件选择器保存」描述不一致 | `lib/src/pages/settings_page.dart` L172-187、README L70-71 | 提供直存路径；取消分享保留暂存并提示位置；同步修订 README |
| F11 | 低 | 品牌残留：默认启动图标、模板 web manifest、模板 pubspec 描述 | `android/app/src/main/res/mipmap-*/ic_launcher.png`、`web/manifest.json`、`pubspec.yaml` | 定制图标/描述/web manifest；或文档明确仅 Android 目标 |
| F12 | 低 | iOS 权限描述缺失（若支持 iOS，调用相机/麦克风/相册会崩溃或被拒审） | `ios/Runner/Info.plist` | 补齐权限描述，或 README 明确当前仅支持 Android |
| F13 | 低 | AndroidManifest 中文注释乱码；`namespace = com.example.findit` 与 `applicationId = com.kurikana.findit` 及源码包路径不一致 | `AndroidManifest.xml`、`android/app/build.gradle.kts` | 修正 manifest 编码；统一 namespace 与源码包路径 |
| F14 | 低 | 无自动备份/备份提醒，数据丢失风险完全依赖用户自觉 | `lib/src/pages/settings_page.dart` | 导出成功后的下次提醒/定期备份提醒，或接入系统备份（配合排除敏感设置） |

---

## 4. 改进路线建议

### 4.1 短期（快速修复，改动小、收益直接，建议下一迭代完成）

**发布前必修（P0）：**

1. 【安全 S-H1】release 改用独立签名：生成 release keystore，`key.properties` 不入库，发布前 `apksigner verify --print-certs` 核对非 debug 证书；
2. 【安全 S-H3】备份剔除全部密钥类设置（`ai_api_key` + `ai_embed_api_key`），并新增测试断言两个 Key 均被剔除；
3. 【性能 P-H1】照片压缩管线移出持锁闭包：「锁外压缩落盘 → 短锁内登记 photo_path」，消除拍照入库期间全 App 数据库阻塞；
4. 【性能 P-H2】`HttpAiTransport` 持有单例 `reqwest::blocking::Client`，消除逐请求重建运行时与连接。

**快速体验修复（P0/P1）：**

5. 【性能 P-M2】搜索页去掉重复查询，只保留一次带向量的混合搜索调用；
6. 【性能 P-M3】照片路径解析 `Future.wait` 并行化（或新增 Rust 批量 API）；
7. 【性能 P-L3】物品写入（`create_item`/`replace_item_categories`）包事务，顺带去除冗余 `get_item`；
8. 【产品 F2】AI 修改预览展示命中目标物品（名称 + 所在箱 + 命中数），多候选提供选择，杜绝盲改；
9. 【产品 F6】快速录入解析期提供「取消」+ 短超时（15–20s），错误卡增加「前往 AI 设置」引导。

### 4.2 中期（结构性改进，v1.1 前后）

**数据安全与隐私（安全线 P1）：**

10. 【安全 S-H2】API Key 迁移至系统级安全存储（Keystore/Keychain），数据库仅存引用；
11. 【安全 S-M2】`network_security_config.xml` 收敛明文流量（仅 localhost/局域网），`save_ai_config` 拒绝或强警告 `http://` + Key；
12. 【安全 S-M3】Android `allowBackup=false` 或 `dataExtractionRules` 排除、iOS 排除 iCloud 备份；
13. 【安全 S-M4 + 性能 P-M4】语义回填改为「显式开关（默认关）+ 告知文案 + 单飞防抖延迟触发」，杜绝静默外发与重复请求。

**性能结构化优化（性能线 P1）：**

14. 【性能 P-H3】语义搜索向量内存缓存 + 预计算范数，评估 sqlite-vec，扫描移出持锁闭包；
15. 【性能 P-M1】关键词搜索引入 FTS5（trigram/n-gram 分词）替代 LIKE 全表扫描；
16. 【性能 P-M5】SAF 恢复备份改流式分块写盘，避免整包入内存；
17. 【性能 P-M7】照片解码降采样（picker 限尺寸 + Rust scale-down 解码）；
18. 【性能 P-M6】`rebuild_embeddings` 改 async 对齐现有回填路径，重试退避非阻塞化。

**产品能力补齐（产品线 P1-P2）：**

19. 【产品 F1】Android 注册 `findit://box/*` 深链直达收纳箱，iOS 同步补 `CFBundleURLTypes`；
20. 【产品 F3】编辑表单增加「所在收纳箱」选择器，无 AI 用户可手动移箱；
21. 【产品 F4】新增分类管理页（重命名/删除）；
22. 【产品 F5】搜索结果直达物品 + 高亮、渲染缩略图、语义提示按配置状态动态显示；
23. 【产品 F7】建档表单直接支持拍照/选图。

### 4.3 长期（数据规模增长后 / v1.2+）

**安全（安全线 P2+）：**

24. 【安全 S-M1】备份可选密码加密（AES-256-GCM + Argon2id 派生密钥）；【安全 S-L1】旧副本清理提示/排除云备份；【安全 S-L2】zip 条目数上限 + 流式复制；【安全 S-L3】`cargo audit`/Dependabot/密钥扫描入 CI；【安全 S-L4】错误信息通用化脱敏；【安全 S-I1】应用锁/防截屏；【安全 S-I2】语音隐私说明；【安全 S-M5】iOS 权限描述与 ATS 处理（或文档声明仅 Android）。

**性能（性能线 P2）：**

25. 【性能 P-L7】读写连接拆分（读连接池 + 写连接）+ `PRAGMA cache_size` 调优；【性能 P-L2】`embedding IS NULL` 部分索引，全量回填 O(N²)→O(N)；【性能 P-L4】备份快照独立连接；【性能 P-L5/L6/L8】搜索结果 SliverList 懒构建、大图 `cacheWidth`、delete_unit 合并删除。

**产品（产品线 P3-P4 + Roadmap）：**

26. 【产品 F8】删除撤销/回收站；【产品 F9】展示登记/更新时间；【产品 F10】备份直存路径 + 修订 README；【产品 F11/F13】品牌定制与工程卫生；【产品 F12】iOS 声明或补齐；【产品 F14】自动备份提醒；另可评估产品线 Roadmap 建议：物品排序/置顶、批量操作、分类浏览视图、多箱二维码批量导出、关于页、无 AI 快速录入降级表单。

### 4.4 验证建议（修复后应补的回执）

- **性能基准**：用「5000 件物品 + 500 张照片 + 768 维向量」合成库做真机基准（列表首屏、拍照入库时其它操作延迟、语义搜索单次延迟、全量回填总时长）；Android Profiler / Dart DevTools 观测照片保存与搜索期间堆内存峰值与帧率（当前量级判断均为「待验证」）。
- **安全回执**：`cargo test` 通过且新增用例覆盖 `ai_embed_api_key` 剔除与「http+key 拒绝/警告」；`apksigner verify --print-certs` 显示正式签名；`cargo audit` 零漏洞告警；设置页存在语义搜索开关与数据外发说明文案。
- **产品走查**：发布前执行一次完整真机走查（新建单元 → 建箱 → 二维码导出 → 外部相机扫描标签 → AI 建档/修订 → 无 AI 降级 → 备份/恢复）；`flutter analyze` 与 `cargo test` 纳入门禁。

---

## 5. 附录：源报告文件路径

| 视角 | 报告文件 | 说明 |
| --- | --- | --- |
| 性能 | `docs/review/performance.md` | 3 高 / 7 中 / 8 低，含 P0-P2 改进路线与验证建议 |
| 安全 | `docs/review/security_review.md` | 3 高 / 5 中 / 4 低 / 2 提示，含修复路线图与验证方式 |
| 产品 | `docs/review/product_review.md` | 2 高 / 5 中 / 7 低 + Roadmap 增强建议，含移交其他视角线索 |

> 注：汇总时按队长确认的实际落盘文件名读取（产品线文件为 `product_review.md` 而非任务描述中的 `product.md`；安全线文件为 `security_review.md` 而非 `security.md`）。三份源报告均完整存在，无缺失项。本汇总报告忠实于三份源报告的发现，未新增源报告之外的问题；性能线问题数（8 低）以源报告正文清单（L1–L8）为准。

---

## 6. 修复进度（2026-08 修复阶段）

### 6.1 验证结果

| 门禁 | 结果 |
| --- | --- |
| `cargo test --manifest-path rust/Cargo.toml` | ✅ 196 passed / 0 failed（含 FTS5、hybrid、backup、photo 全部测试） |
| `dart analyze lib test` | ✅ No issues found（沙箱放开后可用，约 7s） |
| `flutter build apk --debug` | ✅ 构建成功，产物 `build/app/outputs/flutter-apk/app-debug.apk`（209.9MB，四 ABI） |

### 6.2 25 项高/中问题修复状态（范围：8 高 + 17 中）

**性能线 10 项 — 全部 ✅**

| 编号 | 状态 | 落地说明 |
| --- | --- | --- |
| P-H1 | ✅ | 照片保存三阶段锁边界：短锁校验 → 锁外压缩落盘 → 短锁登记（api/photos.rs、photo.rs） |
| P-H2 | ✅ | `blocking::Client` LazyLock 进程级单例（client.rs） |
| P-H3 | ✅ | 语义向量缓存 + 预计算范数 + top-K=100 + 锁外打分（semantic.rs、hybrid.rs、api/search.rs） |
| P-M1 | ✅ | FTS5（unicode61 + `cjk_spaced` 标量函数，bundled SQLite 无 ngram 已注明取舍）+ `needs_like` 兜底（keyword.rs） |
| P-M2 | ✅ | 搜索页单次混合搜索（search_page.dart） |
| P-M3 | ✅ | 照片路径 `Future.wait` 并行解析（items_page.dart） |
| P-M4 | ✅ | Rust 侧 AtomicBool 单飞 + Dart 侧 3s 防抖合并（api/ai.rs、backfill.dart） |
| P-M5 | ✅ | SAF 恢复流式复制，不再整包 readAsBytes（settings_page.dart） |
| P-M6 | ✅ | 热路径异步化 + `Delay` 非阻塞退避；rebuild_embeddings 因 FRB codegen 硬约束保持同步签名并注明 |
| P-M7 | ✅ | jpeg scale-down 降采样（photo.rs）+ picker 限尺寸/质量（items_page.dart） |

**安全线 8 项 — 全部 ✅（含修复阶段补齐）**

| 编号 | 状态 | 落地说明 |
| --- | --- | --- |
| S-H1 | ✅ | release 独立 keystore（key.properties 不入库）；缺文件时 release 构建 fail-fast，禁回退 debug 签名（build.gradle.kts） |
| S-H2 | ✅ | API Key AES-256-GCM 加密落盘（keystore.rs，设备派生密钥 + app 私有目录盐；过渡方案，真 Keystore 迁移留待后续） |
| S-H3 | ✅ | `SECRET_SETTING_KEYS` 白名单剔除两 Key；backup/tests.rs 断言 `ai_api_key` 与 `ai_embed_api_key` 均空 |
| S-M1 | ✅ | 采用"提示"分支：设置页「备份文件未加密」警示；zip 加密留待长期 |
| S-M2 | ✅ | `network_security_config.xml` 默认禁明文，仅放行 localhost/127.0.0.1/10.0.2.2；config.rs 对 http+Key 拒绝/警告 |
| S-M3 | ✅ | `allowBackup=false` + iOS `NSURLIsExcludedFromBackupKey` |
| S-M4 | ✅ | 回填开关默认关 + 启动门控（main.dart）；**修复阶段补齐**：设置页与后端键名统一为 `semantic_backfill_enabled`（原 `ai_semantic_backfill_enabled` 不一致导致开关不生效） |
| S-M5 | ✅ | iOS 相机/麦克风/相册 usage description + ATS 本地网络放行 |

**产品线 7 项 — 5 ✅ + 2 ⚠️ 部分落地**

| 编号 | 状态 | 落地说明 |
| --- | --- | --- |
| F1 | ⚠️ 部分 | 平台深链注册完成（Android intent-filter `findit://box` + iOS CFBundleURLTypes）；App 内扫码解析可用；**外部扫码唤起后直达收纳箱需引入 app_links/uni_links 插件（未做，留待后续）** |
| F2 | ✅ | AI 修改预览展示候选物品 + 300ms 防抖检索 + 选定后以确切名称精确定位（quick_add_page.dart + apply.rs） |
| F3 | ✅ | 编辑表单「所在收纳箱」选择器，支持跨单元（items_page.dart） |
| F4 | ⚠️ 部分 | 分类管理页（重命名/删除）已建（categories_page.dart）；**入口未接入导航，UI 暂不可达** |
| F5 | ✅ | 搜索直达物品并高亮、结果卡缩略图、语义提示按配置状态动态显示（search_page.dart） |
| F6 | ✅ | 解析期取消 + 错误卡「前往 AI 设置」引导 + CHAT_TIMEOUT=20s 可配置 |
| F7 | ✅ | 建档态先选图、保存后立即留档（items_page.dart） |

**修复阶段额外处理**：t7 遗留的 2 个编译错误（quick_add_page `targetQuery` final 赋值 → 改 `_buildIntent(targetQueryOverride:)` 构造；settings_page `pipe(sink)` → `addStream`）+ 5 个 analyze 提示（多余 cast/import、final 字段、mounted 守卫）已由队长修复，`dart analyze` 归零。

### 6.3 已知遗留（本次范围外 / 待后续）

1. **21 项低危与提示问题**（P-L1–L8、S-L1–L4、S-I1–I2、F8–F14）未处理，见 §3 backlog 表；
2. **F1 外部深链直达**：需引入深链插件并注册路由；
3. **F4 分类管理入口**：需在导航/物品页接入 CategoriesPage；
4. **新文件注释乱码**：categories_page.dart、backfill.dart、network_security_config.xml 等部分新增文件注释存在编码损坏（UTF-8 被错误写入），需按 UTF-8 重写清理；
5. **release 构建**：需先创建 release keystore 并配置 `android/key.properties`（构建会 fail-fast 提示）；release APK 体积约 83.5MB（README）；
6. **验证建议**：发布前建议在正常环境补跑 `flutter analyze`（本阶段已用 `dart analyze` 全绿 + debug 构建验证）。
