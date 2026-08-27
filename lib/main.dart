import 'package:flutter/material.dart';
import 'package:path_provider/path_provider.dart';
import 'package:findit/src/rust/api/ai.dart' as ai_api;
import 'package:findit/src/rust/api/db.dart' as rust_db;
import 'package:findit/src/rust/api/settings.dart' as settings_api;
import 'package:findit/src/rust/frb_generated.dart';

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

  @override
  void initState() {
    super.initState();
    _bootstrap();
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
