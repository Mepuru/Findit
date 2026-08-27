# Findit 安全审查报告

- 审查视角：安全（Security）
- 审查人：安全审查员（findit-review-team）
- 审查日期：2026-02（Findit 三视角审查的一部分）
- 审查范围：Flutter UI（`lib/`）、Rust 核心（`rust/src/`）、平台配置（Android/iOS）、备份/恢复、AI 链路、依赖清单
- 审查方法：全量源码走读 + 数据流/信任边界分析 + 依赖版本核对 + 关键场景（备份恢复、AI 外发、密钥存储）专项验证

## 一、总体结论

Findit 的**本地存储架构与代码质量基础较好**：数据库访问全部参数化（无 SQL 注入）、备份恢复实现了完整的 zip-slip / zip-bomb / 完整性校验链、照片重编码自动剥离 EXIF/GPS、无遥测无埋点、仓库内未发现硬编码密钥。

但作为**隐私优先**产品，存在三类系统性缺口，需在对外发布前处理：

1. **密钥与数据静态保护缺失**：AI API Key（含向量独立 Key）明文存于 SQLite；备份 zip 不加密；Android 自动备份/iOS iCloud 备份会原样上传含密钥与照片的数据目录；
2. **发布签名不可信**：release 构建仍使用 debug 签名，分发出去的 APK 任何人都能用公开的 debug 密钥伪造同签名"更新"；
3. **数据外发缺少告知与最小化**：应用启动即自动回填语义向量，物品文本（名称/备注/分类）会被静默发送到所配置的 AI 服务；备份剔除密钥的逻辑遗漏了向量 Key，与 README 承诺不符。

共发现 **3 高 / 5 中 / 4 低 / 2 提示** 类问题（详见下）。

## 二、问题清单（按严重程度）

### 高危（High）

#### H1. release 构建使用 debug 签名，分发链路无签名信任

- 位置：`android/app/build.gradle.kts`（`buildTypes.release.signingConfig = signingConfigs.getByName("debug")`）
- 问题：README 的发布方式为 `flutter build apk --release` 后直接传手机安装。该产物由 **Android SDK 公开的 debug keystore** 签名（默认密码人人皆知，且各开发者机器上的 debug 密钥默认相同）。任何攻击者都能用同一签名密钥伪造一个"官方更新"APK，在已安装用户上无缝覆盖安装（同签名升级路径），进而读取原应用数据目录（含未加密的数据库与照片）。
- 修复建议：生成独立 release keystore，通过 `android/key.properties`（不入库）配置 `signingConfig`；将 debug 签名仅用于开发；发布前用 `apksigner verify --print-certs` 核对签名；可选加固（ProGuard/R8 + `flutter build apk --release --obfuscate --split-per-abi`）。

#### H2. AI API Key 明文存储于 SQLite

- 位置：`rust/src/core/ai/config.rs`（`KEY_API_KEY` / `KEY_EMBED_API_KEY`，经 `save_ai_config` 写入 `app_settings` 表）；`rust/src/core/db/mod.rs`（普通 SQLite，未加密）
- 问题：密钥以明文 TEXT 存于 `findit.db`。结合 H6（Android 自动备份）与 H4（备份 zip），密钥会在多个层面以明文形式离开设备或暴露给本机其他主体（root/ADB/文件拷贝）。数据库整体也未加密（无 SQLCipher），照片文件同样明文落盘。
- 修复建议：密钥改用系统级安全存储（Android Keystore / iOS Keychain，如 `flutter_secure_storage`），数据库仅存引用；核心数据落盘加密（SQLCipher 或整库文件加密）作为长期方向；至少做到"密钥绝不随普通数据文件一起导出/备份"。

#### H3. 备份导出漏剔向量 API Key（`ai_embed_api_key`），与 README 承诺不符

- 位置：`rust/src/core/backup/export.rs` `scrub_api_key()`（仅 `UPDATE ... WHERE key = 'ai_api_key'`）；`rust/src/core/ai/config.rs` 定义了 `KEY_EMBED_API_KEY = "ai_embed_api_key"`
- 问题：导出的备份 zip 中 `ai_embed_api_key` 仍为明文，只有对话 Key 被置空。README 明确承诺"AI API Key 不会写入备份文件"。现有测试 `export_scrubs_api_key_from_snapshot`（`rust/src/core/backup/tests.rs`）只覆盖了 `ai_api_key`，未覆盖向量 Key，因此漏检。
- 修复建议：`scrub_api_key` 改为遍历/白名单剔除所有密钥类设置（至少 `ai_api_key`、`ai_embed_api_key`），并新增测试同时断言两个 Key 均为空；备份 manifest 可加 `secrets_scrubbed: true` 标记供恢复端校验。

### 中危（Medium）

#### M1. 备份 zip 不加密，获取文件即获取全部数据

- 位置：`rust/src/core/backup/export.rs`（`write_zip`，`CompressionMethod::Deflated`，无加密）；`lib/src/pages/settings_page.dart`（导出后经系统分享）
- 问题：备份文件包含完整数据库（剔除对话 Key 后仍有向量 Key）与全部照片，未压缩加密。文件一经分享/存储即可被任意读取，与"隐私优先"定位不符。
- 修复建议：提供可选的密码加密（如 AES-256-GCM，密码经 Argon2id 派生密钥，密钥不落盘）；或至少导出前在 UI 明确提示"备份文件未加密，请妥善保管"，并把向量 Key 剔除（H3）作为硬性前置。

#### M2. Android 全局允许明文 HTTP（`usesCleartextTraffic="true"`）+ 未限制 AI 地址协议

- 位置：`android/app/src/main/AndroidManifest.xml`（`android:usesCleartextTraffic="true"`）；`rust/src/core/ai/client.rs`（`bearer_auth` 不区分协议）
- 问题：全局明文开关使任意流量（不止 Ollama 局域网）都允许明文传输；同时 AI 配置允许任意 base URL——若用户把 OpenAI 兼容地址配成 `http://`，API Key 将以明文经网络发送。默认 Ollama 地址 `http://10.0.2.2:11434` 本身也需明文（可接受，但应限定范围）。
- 修复建议：改用 `network_security_config.xml`，仅对 localhost/10.0.2.2/局域网前缀放行 cleartext，其余强制 TLS；在 `save_ai_config` 校验：当 api_key 非空且 base_url 为 `http://` 时拒绝或强警告。

#### M3. Android Auto Backup / iOS iCloud 备份未处理，敏感数据默认上云

- 位置：`android/app/src/main/AndroidManifest.xml`（无 `android:allowBackup` / `dataExtractionRules`）；`ios/Runner/Info.plist`（无备份排除）
- 问题：Android `allowBackup` 默认 true，`findit.db`（含明文密钥）与 `photos/` 会随系统自动备份上传 Google 云；iOS 应用文档目录默认参与 iCloud 备份。用户无感知地"自动上云"，与"数据全部存手机本地、无服务端"的 README 表述冲突。
- 修复建议：`allowBackup="false"`（或 `dataExtractionRules` 排除 db/photos/settings）；iOS 对数据目录设置 `NSURLIsExcludedFromBackupKey`；文档与产品说明同步更新。

#### M4. 启动时静默回填语义向量：物品文本自动外发至 AI 服务

- 位置：`lib/main.dart`（`_bootstrap` 中 `ai_api.backfillPendingEmbeddings()`）；`rust/src/core/ai/embed.rs`（`pending_item_texts` 取 name/description/categories 全文）
- 问题：配置了 AI 服务后，**每次启动**都会把全部待向量化物品的"名称+备注+分类"文本批量发送到所配置的嵌入服务（若是 OpenAI 等云端服务则为外网）。这是用户未显式触发、也无独立开关的数据外发；快速录入/修订链路同样把用户输入发送到 AI。隐私优先产品应让数据外发可预期、可关闭。
- 修复建议：语义搜索改为显式启用（设置页开关，默认关）；启用时给出明确告知文案（"语义搜索会把物品文本发送到配置的 AI 服务"）；启动回填仅在该开关开启时执行。

#### M5. iOS 权限用途描述缺失 + AI 功能在 iOS 不可用

- 位置：`ios/Runner/Info.plist`（无 `NSCameraUsageDescription` / `NSMicrophoneUsageDescription` / `NSPhotoLibraryUsageDescription`；无 ATS 例外）
- 问题：iOS 上调用相机（mobile_scanner）、麦克风（speech_to_text）、相册（image_picker）会因缺少 usage description 直接崩溃/失败；默认 Ollama 地址 `http://10.0.2.2:11434` 在 iOS ATS 下被拦截，AI 功能整体不可用（功能完整性 + 崩溃面）。
- 修复建议：补齐三项 usage description；如目标平台含 iOS，评估 ATS 例外范围或引导用户填 https/LAN 地址。

### 低危（Low）

#### L1. 恢复后旧数据明文副本遗留

- 位置：`rust/src/core/backup/restore.rs` `swap_dirs` / `cleanup_old_backups`（`{db_dir}.backup-{ts}` 仅保留最近一份，且不参与任何加密/备份排除）
- 问题：每次恢复都会在应用目录留下一份完整的旧数据明文副本（含照片与密钥），下次恢复才清理；这份副本同样会被系统备份带走（叠加 M3）。
- 修复建议：恢复完成后提示用户可删除副本，或将副本目录一并排除出云备份；长期以 H2 的落盘加密兜底。

#### L2. 恢复解压的 zip 条目数无上限 + Dart 侧整包读入内存

- 位置：`rust/src/core/backup/restore.rs` `extract_all`（仅按字节限额，`files` 计数无上限）；`lib/src/pages/settings_page.dart` `_restore`（SAF 场景 `picked.readAsBytes()` 整包进内存，上限 1GB）
- 问题：精心构造的 zip 可用大量微小条目在临时目录创建海量文件（DoS）；1GB 上限的文件在弱内存设备上整包读入可能 OOM。均需用户主动选择恶意文件，风险有限。
- 修复建议：增加条目数上限（如 10 万）与单文件数预算；Dart 侧改用流式复制 `content://` 内容（`openStream` + 分块写入）替代 `readAsBytes`。

#### L3. 依赖版本滞后、无自动化安全审计

- 位置：`rust/Cargo.lock`（reqwest 0.12.4、image 0.25.10、zip 2.4.2、rusqlite 0.37.0、flutter_rust_bridge 2.11.0）；`pubspec.lock`
- 问题：reqwest 0.12.4 属于 0.12 早期版本，后续 0.12.22+ 已合入多项安全修复（[deps.rs 参考](https://deps.rs/crate/reqwest/0.12.26)）；无 `cargo audit` / Dependabot / 密钥扫描门禁。zip 2.4.2 已覆盖历史 zip 相关 CVE（如 [CVE-2025-29786 相关公告](https://github.com/advisories/GHSA-93mq-9ffx-83m2)），无已知高危。
- 修复建议：将 `cargo audit`（或 `cargo deny`）与 `flutter pub outdated` 纳入 CI 门禁；reqwest 升级至最新 0.12.x；新增 pre-commit 密钥扫描（gitleaks/trufflehog）。

#### L4. 错误信息向 UI 透传原始细节

- 位置：`rust/src/core/error.rs`（`Db/Io/AiModelOutput` 原样携带 SQLite 错误、文件路径、服务端响应片段）；`lib/src/errors.dart`（`friendlyErrorMessage` 直接展示）
- 问题：AI 服务端返回内容（最长 200 字符片段）会原样展示给用户，可能回显用户已发送的隐私文本或服务端诊断信息；SQLite/路径细节有助于定位但属信息暴露。
- 修复建议：对外展示层对 `AiModelOutput` 做通用化处理（仅提示"模型输出无法解析"），完整错误仅保留在 debug 日志；路径类 IO 错误做脱敏。

### 提示（Info）

#### I1. 无应用锁与防截屏

- 位置：全应用
- 问题：无 PIN/生物识别锁，无 `FLAG_SECURE` 防截屏；最近任务缩略图与截屏可能暴露物品清单/照片。对存放家庭物品（含敏感物品如证件、贵重物品）的隐私应用而言属可选的隐私加固。
- 建议：提供可选的应用锁（系统生物识别）与防截屏开关（`FLAG_SECURE`，注意会同时禁用录屏分享）。

#### I2. 语音输入依赖系统语音服务，未在 UI 披露

- 位置：`lib/src/pages/quick_add_page.dart`（`speech_to_text`）
- 问题：语音识别文本由系统语音服务（Google/Apple）处理，语音内容会离开设备；README/UI 未披露。另需注意语音输入后直接进入 AI 解析链路（叠加 M4 的外发告知缺失）。
- 建议：首次使用语音时弹出一句隐私说明（"语音将由系统语音服务识别"）。

## 三、做得好的安全实践（予以肯定）

- **SQL 注入面为零**：全库 CRUD/搜索均参数化（`?` 绑定），关键词 LIKE 元字符显式转义（`rust/src/core/search/keyword.rs`）；动态 SQL 仅拼接固定列名。
- **备份恢复校验链完整**：扩展名/大小/zip-slip（`..`、绝对路径、盘符逐组件拒绝 + `starts_with` 双保险）/解压字节预算/压缩比炸弹检测/SQLite `integrity_check`/`user_version` 上限/必需表清单/原子替换与回滚（`rust/src/core/backup/restore.rs`）。
- **照片隐私友好**：重编码管线从像素级再压缩（自动剥离 EXIF/GPS），主图限制 1600px、缩略图 256px（`rust/src/core/photo.rs`）；文件名校验拒绝路径穿越。
- **TLS 与认证**：reqwest 走 rustls（不依赖系统 OpenSSL 配置），https 证书校验默认开启；Bearer 仅对 OpenAI 兼容 provider 附加。
- **二维码安全**：扫码仅接受 `findit://box/{slug}` 单一格式（`lib/src/pages/scan_page.dart`），不打开任意 URL；slug 为 UUID v4 不可枚举。
- **密钥输入 UI 掩码**（`obscureText`）、`AiStatus` 不下发密钥；仓库工作区无硬编码密钥；无分析/崩溃上报 SDK。
- **锁边界设计**：网络调用严格在全局 DB 锁外执行（`rust/src/api/ai.rs` 三段式），避免持锁阻塞与死锁风险。

## 四、优先级与修复路线图

| 优先级 | 问题 | 建议版本 |
| --- | --- | --- |
| P0（发布前必须） | H1 release 签名、H3 备份漏剔向量 Key | v1.0.1 前 |
| P1（发布前必须） | H2 密钥安全存储、M2 明文流量收敛 | v1.1 |
| P1（强烈建议） | M3 云备份处理、M4 语义回填开关与告知 | v1.1 |
| P2 | M1 备份加密、M5 iOS 权限/ATS、L1-L4、I1-I2 | v1.2+ |

**验证方式**（修复后应补的回执）：
1. `cargo test` 通过，且新增用例覆盖 `ai_embed_api_key` 剔除与「http+key 拒绝/警告」；
2. `apksigner verify --print-certs app-release.apk` 显示正式签名而非 `CN=Android Debug`；
3. `cargo audit` 零漏洞告警；
4. 设置页存在语义搜索开关与数据外发说明文案。

## 五、附录：关键文件索引

| 关注点 | 文件 |
| --- | --- |
| 密钥存储 | `rust/src/core/ai/config.rs`、`rust/src/core/db/mod.rs` |
| AI 外发链路 | `rust/src/core/ai/embed.rs`、`rust/src/core/ai/client.rs`、`lib/main.dart` |
| 备份/恢复 | `rust/src/core/backup/export.rs`、`restore.rs`、`lib/src/pages/settings_page.dart` |
| 平台配置 | `android/app/src/main/AndroidManifest.xml`、`android/app/build.gradle.kts`、`ios/Runner/Info.plist` |
| 搜索注入 | `rust/src/core/search/keyword.rs`、`hybrid.rs` |
| 依赖清单 | `rust/Cargo.lock`、`pubspec.lock` |
