import 'dart:async';

import 'package:app_links/app_links.dart';
import 'package:flutter/material.dart';
import 'package:path_provider/path_provider.dart';
import 'package:findit/src/rust/api/ai.dart' as ai_api;
import 'package:findit/src/rust/api/db.dart' as rust_db;
import 'package:findit/src/rust/api/settings.dart' as settings_api;
import 'package:findit/src/rust/frb_generated.dart';

import 'src/pages/scan_page.dart' show openBoxByDeepLink, parseFinditBoxSlug;
import 'src/pages/units_page.dart';
import 'src/theme.dart';

/// 语义向量自动回填开关的存储键（与 rust/src/core/ai/config.rs 的
/// KEY_SEMANTIC_BACKFILL 保持一致；设置页 UI 读写同一键）。
const _kSemanticBackfillKey = 'semantic_backfill_enabled';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  runApp(const FinditApp());
}

/// 应用根组件：负责 Rust 桥接初始化 → 打开数据库 → 进入主界面。
/// 初始化失败时展示错误页，用户可重试。
class FinditApp extends StatefulWidget {
  const FinditApp({super.key});

  @override
  State<FinditApp> createState() => _FinditAppState();
}

enum _BootState { loading, ready, failed }

class _FinditAppState extends State<FinditApp> {
  _BootState _boot = _BootState.loading;
  String _errorMessage = '';

  /// F1：外部深链路由用的根导航 key（冷启动/恢复时经它推入目标页）。
  final GlobalKey<NavigatorState> _navigatorKey = GlobalKey<NavigatorState>();

  /// F1：深链监听订阅（dispose 时取消）。
  StreamSubscription<Uri>? _linkSub;

  /// F1：启动未完成时先暂存深链，就绪后统一路由。
  String? _pendingDeepLink;

  /// F1：最近一次已处理的深链（去重，避免 getInitialLink 与流重复投递）。
  String? _lastHandledLink;
  DateTime _lastHandledAt = DateTime.fromMillisecondsSinceEpoch(0);

  @override
  void initState() {
    super.initState();
    _bootstrap();
    _initDeepLinks();
  }

  @override
  void dispose() {
    _linkSub?.cancel();
    super.dispose();
  }

  /// F1：注册外部深链监听。
  /// 冷启动由 [AppLinks.getInitialLink] 捕获；运行中/后台恢复由
  /// [AppLinks.uriLinkStream] 捕获。仅处理 `findit://box/{slug}` 箱标签，
  /// 其余内容忽略（与 App 内扫码共用同一解析/路由函数）。
  Future<void> _initDeepLinks() async {
    try {
      _linkSub = AppLinks().uriLinkStream.listen(_handleDeepLink);
      final initial = await AppLinks().getInitialLink();
      if (initial != null) _handleDeepLink(initial);
    } catch (_) {
      // 深链插件不可用时静默降级（不影响启动与 App 内扫码直达）。
    }
  }

  /// F1：解析并路由外部深链；非箱标签格式直接忽略。
  void _handleDeepLink(Uri uri) {
    final raw = uri.toString();
    if (parseFinditBoxSlug(raw) == null) return;
    // 去重：getInitialLink 与 uriLinkStream 可能对同一链接重复投递。
    final now = DateTime.now();
    if (raw == _lastHandledLink &&
        now.difference(_lastHandledAt).inSeconds < 3) {
      return;
    }
    _lastHandledLink = raw;
    _lastHandledAt = now;
    if (_boot != _BootState.ready) {
      _pendingDeepLink = raw;
      return;
    }
    _openDeepLink(raw);
  }

  /// F1：等当前帧结束后经根导航推入目标收纳箱物品页。
  void _openDeepLink(String raw) {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      final ctx = _navigatorKey.currentContext;
      if (ctx == null) return;
      openBoxByDeepLink(ctx, raw);
    });
  }

  /// F1：启动完成后补发启动期间收到的深链。
  void _drainPendingDeepLink() {
    final pending = _pendingDeepLink;
    if (pending == null) return;
    _pendingDeepLink = null;
    _openDeepLink(pending);
  }

  Future<void> _bootstrap() async {
    setState(() => _boot = _BootState.loading);
    try {
      await RustLib.init();
      final docDir = await getApplicationDocumentsDirectory();
      await rust_db.initDb(dbDir: docDir.path);
      // 隐私门控（S-M4）：仅在用户显式开启"语义向量自动回填"时才在启动时
      // 外发物品文本到 AI 服务；默认关闭，任何失败静默降级，不影响启动。
      // ignore: unawaited_futures
      _maybeBackfillSemanticVectors();
      if (!mounted) return;
      setState(() => _boot = _BootState.ready);
      _drainPendingDeepLink();
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _boot = _BootState.failed;
        _errorMessage = e.toString();
      });
    }
  }

  /// 读取语义回填开关（默认关闭），开启时触发后台向量回填。
  Future<void> _maybeBackfillSemanticVectors() async {
    var enabled = false;
    try {
      final stored = await settings_api.getSetting(key: _kSemanticBackfillKey);
      enabled = stored == '1' || stored == 'true';
    } catch (_) {
      // 读取开关失败按关闭处理，不影响启动。
      enabled = false;
    }
    if (enabled) {
      // 后台轻量回填；失败静默忽略（与原行为一致）。
      // ignore: unawaited_futures
      ai_api.backfillPendingEmbeddings().catchError((_) => 0);
    }
  }

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'Findit',
      debugShowCheckedModeBanner: false,
      navigatorKey: _navigatorKey,
      theme: finditTheme,
      darkTheme: finditDarkTheme,
      themeMode: ThemeMode.system,
      home: switch (_boot) {
        _BootState.loading => const _BootPage(),
        _BootState.failed => _BootErrorPage(
            message: _errorMessage,
            onRetry: _bootstrap,
          ),
        _BootState.ready => const UnitsPage(),
      },
    );
  }
}

class _BootPage extends StatelessWidget {
  const _BootPage();

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            const Text('📦', style: TextStyle(fontSize: 56)),
            const SizedBox(height: 16),
            Text(
              'Findit 正在打开收纳档案…',
              style: Theme.of(context).textTheme.titleMedium,
            ),
            const SizedBox(height: 20),
            const SizedBox(
              width: 28,
              height: 28,
              child: CircularProgressIndicator(strokeWidth: 3),
            ),
          ],
        ),
      ),
    );
  }
}

class _BootErrorPage extends StatelessWidget {
  const _BootErrorPage({required this.message, required this.onRetry});

  final String message;
  final VoidCallback onRetry;

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: Center(
        child: Padding(
          padding: const EdgeInsets.all(32),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              const Text('🧾', style: TextStyle(fontSize: 56)),
              const SizedBox(height: 16),
              Text(
                '初始化失败',
                style: Theme.of(context).textTheme.titleLarge,
              ),
              const SizedBox(height: 12),
              Text(
                message,
                textAlign: TextAlign.center,
                style: Theme.of(context).textTheme.bodySmall,
              ),
              const SizedBox(height: 24),
              FilledButton.icon(
                onPressed: onRetry,
                icon: const Icon(Icons.refresh),
                label: const Text('重试'),
              ),
            ],
          ),
        ),
      ),
    );
  }
}
