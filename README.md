# Findit

隐私优先的家庭自托管收纳定位安卓应用：数据全部存手机本地 SQLite，离线可用，无账号、无服务端。

## 功能特性

- **层级收纳**：存储单元 → 收纳箱 → 物品三级结构，逐层浏览与管理
- **二维码标签**：每个收纳箱可生成 `findit://box/{slug}` 二维码，可打印张贴；扫码直达箱内清单
- **AI 快速录入及修订**：语音或文本一句话，经大模型解析后预览确认入库；支持 Ollama 与 OpenAI 兼容接口
- **智能搜索**：关键词 + 语义向量双通道，语义结果按相似度排序并标注百分比；无 AI 时自动降级为关键词搜索
- **照片与分类**：物品拍照留档（自动生成缩略图，支持大图预览），可挂多个分类标签
- **备份/恢复**：数据库 + 照片一键打包为 zip 导出，支持安全恢复（恢复前保留旧数据副本）

全中文界面，支持深色模式。

## 技术架构

Flutter UI + flutter_rust_bridge v2 嵌入式 Rust 核心，纯本地应用，无任何服务进程。

```
rust/src/core/**     纯业务逻辑（零 FRB 依赖，162 个主机单元测试）
rust/src/api/**      FRB 桥接薄壳，只做转发与全局状态编排
lib/src/pages/**     Flutter 页面（单元/箱/物品/快速录入/扫码/搜索/设置等）
lib/src/rust/        codegen 自动生成代码，禁止手改
rust_builder/        cargokit 构建集成（勿手动调整）
```

技术栈：Flutter 3.47.x、flutter_rust_bridge 2.11.0（codegen/crate/Dart 包三件套版本锁定）、
Rust（rusqlite bundled + WAL、reqwest rustls-tls、image、zip）、SQLite。

## 快速开始

环境要求与构建约束详见 [docs/ENVIRONMENT.md](docs/ENVIRONMENT.md)。关键工具：

| 组件 | 版本 |
| --- | --- |
| Flutter | 3.47.1 stable（Dart 3.13.1） |
| rustc / cargo | 1.97.1 |
| flutter_rust_bridge | 2.11.0（三件套锁定） |
| cargo-ndk | 4.1.2 |
| LLVM | 需设置 `LIBCLANG_PATH` 指向 libclang（仅 codegen 需要） |
| JDK | 21 |
| Android NDK | 需设置 `ANDROID_NDK_HOME`（并安装 rustup Android 交叉编译目标） |

构建命令：

```powershell
flutter pub get
flutter_rust_bridge_codegen generate   # 修改了 rust/src/api 对外签名后需要重跑
cargo test --manifest-path rust/Cargo.toml
flutter build apk --release
```

产物位于 `build\app\outputs\flutter-apk\app-release.apk`（约 83.5MB，含
arm64-v8a / armeabi-v7a / x86_64），传到手机安装即可。

## AI 配置

在设置页配置（可选，不配置不影响基础功能）：

- **Ollama**：填写局域网内 Ollama 服务地址（如 `http://192.168.x.x:11434`），建议选择
  指令遵循较好的中小模型；手机需与 Ollama 处于同一局域网，应用已声明明文 http 流量
- **OpenAI 兼容端点**：填写 base URL、API Key 与模型名

无 AI 时的降级表现：快速录入/一句话修订不可用，搜索退化为纯关键词匹配，其余功能不受影响。

## 备份与恢复

- **导出**：设置页一键将数据库与照片打包为 zip，经系统文件选择器保存
- **恢复**：选择备份 zip 恢复；恢复前会将旧数据保留为副本，可事后找回

## 开发与测试

- 核心逻辑全部位于 `rust/src/core`，`cargo test` 可在纯主机环境运行，无需设备
- 提交前静态检查门禁：`flutter analyze lib test`
- 提交规范：`<英文类型>: <中文描述>`（如 `feat: 添加用户登录功能`），详见 [AGENTS.md](AGENTS.md)
- 构建约束与技术债说明详见 [docs/ENVIRONMENT.md](docs/ENVIRONMENT.md)

## 许可证

[MIT](LICENSE)，Copyright © 2026 栗かな
