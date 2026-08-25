# Findit 构建环境与已知约束

> 本文档对应实施计划的风险缓解项「安装清单文档化」。记录本机（Windows）实际验证过的
> 工具链版本、必需环境变量、常用命令与已知构建约束（技术债及解除条件）。
> 所有版本均按仓库当前可构建状态核实，升级任何一项前请先阅读「已知构建约束」。

## 1. 工具链清单与版本

| 组件 | 版本 | 说明 |
| --- | --- | --- |
| Flutter | 3.47.1 stable（Dart 3.13.1） | SDK 位于 `C:\Users\KuriKana\flutter` |
| rustc / cargo | 1.97.1 | rustup 管理 |
| flutter_rust_bridge | **三件套锁定 2.11.0** | 见下方锁定说明 |
| cargo-ndk | 4.1.2 | cargokit 经它交叉编译 Android 目标 |
| LLVM | `C:\Program Files\LLVM` | codegen 依赖 libclang，经 `LIBCLANG_PATH=C:\Program Files\LLVM\bin\libclang.dll` 指向 |
| JDK | Microsoft OpenJDK 21（jdk-21.0.12.101-hotspot） | `C:\Program Files\Microsoft\jdk-21.0.12.101-hotspot` |
| Android cmdline-tools | latest | `%LOCALAPPDATA%\Android\Sdk\cmdline-tools` |
| Android platforms | android-35、android-36（另有遗留 android-33） | 同上 SDK 目录 |
| Android build-tools | 35.0.0、36.0.0 | 同上 |
| Android NDK | **25.2.9519653** 与 **28.2.13676358** 并存 | 28.2 由 AGP 侧 `flutter.ndkVersion` 自动装出，近期成功构建均以它作为 `ANDROID_NDK_HOME`；25.2 为计划预留的固定版本 |

### flutter_rust_bridge 三件套锁定 2.11.0

三个组件版本必须严格一致，当前均锁定 2.11.0：

- **codegen 可执行文件**：`flutter_rust_bridge_codegen`（`~/.cargo/bin`，`cargo install flutter_rust_bridge_codegen`）
- **Rust crate**：`rust/Cargo.toml` 中 `flutter_rust_bridge = "=2.11.0"`
- **Dart 包**：`pubspec.yaml` 中 `flutter_rust_bridge: 2.11.0`

升级必须三者同升，升级后立即重跑桥接门禁：`flutter_rust_bridge_codegen generate` →
`cargo test` → `flutter analyze lib test` → `flutter build apk --release`。

### rustup Android targets

交叉编译四目标均已安装（`rustup target list --installed`）：

- `aarch64-linux-android`
- `armv7-linux-androideabi`
- `x86_64-linux-android`
- `i686-linux-android`

缺失时：`rustup target add <target>`。

## 2. 必需环境变量

新终端会话若缺失以下变量需手动设置（示例为本机实际路径）：

```powershell
$env:JAVA_HOME         = 'C:\Program Files\Microsoft\jdk-21.0.12.101-hotspot'
$env:ANDROID_HOME      = "$env:LOCALAPPDATA\Android\Sdk"
# 指向已安装的 NDK；近期成功构建使用 28.2（SDK 中另有 25.2 可替换）
$env:ANDROID_NDK_HOME  = "$env:ANDROID_HOME\ndk\28.2.13676358"
```

另需 `LIBCLANG_PATH` 指向 LLVM 的 libclang（见上表），仅运行 `flutter_rust_bridge_codegen` 时需要。

## 3. 常用命令

| 命令 | 何时使用 |
| --- | --- |
| `flutter_rust_bridge_codegen generate` | 修改 `rust/src/api/**` 对外函数（签名/注释）后重新生成桥接代码 |
| `cargo test`（在 `rust/` 下，或 `cargo test --manifest-path rust/Cargo.toml`） | 核心逻辑全部位于 `rust/src/core`，可纯主机测试，无需设备 |
| `flutter analyze lib test` | 提交前静态检查门禁 |
| `flutter build apk --release` | 产物位于 `build/app/outputs/flutter-apk/app-release.apk`（约 83.5MB） |

## 4. 已知构建约束（技术债与解除条件）

1. **Gradle 8.14.3 + AGP 8.13.0（有意降级）**
   cargokit 与 Gradle 9 不兼容（报 `Could not find method exec()`），故锁定该组合；
   Flutter 已在构建时警告该组合即将被弃用。**解除条件**：cargokit 上游适配 Gradle 9 后
   同步升级 Gradle wrapper 与 AGP，并重跑全部构建门禁。

2. **`android/gradle.properties` 中 `kotlin.build.useFallbacks=true` + `kotlin.incremental=false`**
   规避 Windows 上 Kotlin Build Tools API 增量缓存关闭失败的问题（非性能优化，勿随意删除）。
   **解除条件**：Kotlin/AGP 工具链升级后验证缓存故障消失，即可移除并恢复增量编译。

3. **Gradle wrapper 使用腾讯云镜像且无校验和**
   `gradle-wrapper.properties` 的 `distributionUrl` 指向
   `https://mirrors.cloud.tencent.com/gradle/gradle-8.14.3-all.zip`，未配置
   `distributionSha256Sum`（镜像不保证与官方校验和一致）。
   `pluginManagement` 仍走官方源（google / mavenCentral / gradlePluginPortal）。
   **解除条件**：网络环境可直连官方分发源后，换回官方 URL 并补校验和。

4. **compileSdk 36**
   `compileSdk = flutter.compileSdkVersion`（Flutter 3.47 的默认值为 36）；
   部分 androidx 依赖要求 compileSdk ≥ 34，下调前须确认全部依赖兼容。

5. **正式包权限与明文流量声明勿移除**
   `AndroidManifest.xml` 已声明 `INTERNET`（AI 服务调用）、`CAMERA`（扫码）、
   `RECORD_AUDIO`（语音输入）及 `android:usesCleartextTraffic="true"`
   （局域网 Ollama 走明文 http）。移除任一项都会直接破坏对应功能。

## 5. 架构速览

- **Rust 核心**：`rust/src/core/**` —— 纯业务逻辑，零 FRB 依赖，全部可主机单测。
- **FRB 桥接薄壳**：`rust/src/api/**` —— 只做转发与全局状态编排；对外签名变化后必须重跑 codegen。
- **Flutter UI**：`lib/src/pages/**` 等，经 `lib/src/rust/` 调用 Rust。
- **`lib/src/rust/` 为 codegen 生成代码，禁止手改**；发现其与源码不同步时重跑
  `flutter_rust_bridge_codegen generate` 并提交生成产物。
- 锁边界铁律：全局数据库单连接（`with_conn`），一切网络调用绝不持锁。
