import 'package:flutter/material.dart';
import 'package:path_provider/path_provider.dart';
import 'package:findit/src/rust/api/ai.dart' as ai_api;
import 'package:findit/src/rust/api/db.dart' as rust_db;
import 'package:findit/src/rust/frb_generated.dart';

import 'src/pages/units_page.dart';
import 'src/theme.dart';

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
      // 启动完成后后台轻量回填语义向量；未配置/失败静默忽略。
      // ignore: unawaited_futures
      ai_api.backfillPendingEmbeddings().catchError((_) => 0);
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

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'Findit',
      debugShowCheckedModeBanner: false,
      theme: finditTheme,
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
