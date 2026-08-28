import 'dart:io';

import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'package:findit/src/rust/api/backup.dart' as backup;
import 'package:findit/src/rust/api/model.dart';
import 'package:findit/src/rust/api/settings.dart' as settings_api;
import 'package:path_provider/path_provider.dart';
import 'package:share_plus/share_plus.dart';

import '../errors.dart';
import '../theme.dart';
import 'ai_settings_page.dart';
import 'categories_page.dart';

/// 统一设置页：数据备份 / 恢复 + AI 设置入口。
class SettingsPage extends StatefulWidget {
  const SettingsPage({super.key});

  @override
  State<SettingsPage> createState() => _SettingsPageState();
}

/// 导出/恢复的实时进度。
class _OpProgress {
  const _OpProgress(this.label, this.done, this.total);

  final String label;
  final int done;
  final int total;

  double? get value => total > 0 ? done / total : null;
  String get text => total > 0 ? '$done / $total' : '';
}

class _SettingsPageState extends State<SettingsPage> {
  bool _busy = false;

  /// F14：距上次成功导出的天数；`null` 表示从未导出过。
  int? _daysSinceLastBackup;

  static const _kLastBackupAtKey = 'last_backup_at';

  @override
  void initState() {
    super.initState();
    _loadBackupReminder();
  }

  /// F14：读取「上次成功导出时间」，超过 7 天未导出则展示轻量提醒条。
  Future<void> _loadBackupReminder() async {
    try {
      final stored = await settings_api.getSetting(key: _kLastBackupAtKey);
      if (stored == null || stored.isEmpty) {
        if (mounted) setState(() => _daysSinceLastBackup = null);
        return;
      }
      final last = DateTime.tryParse(stored);
      if (last == null) {
        if (mounted) setState(() => _daysSinceLastBackup = null);
        return;
      }
      final days = DateTime.now().difference(last).inDays;
      if (mounted) setState(() => _daysSinceLastBackup = days);
    } catch (_) {
      // 读取失败不打扰用户，按无提醒处理。
      if (mounted) setState(() => _daysSinceLastBackup = null);
    }
  }

  static const _exportStageLabels = {
    'snapshot': '正在生成数据库快照…',
    'photos': '正在打包照片…',
    'finalize': '正在写入压缩包…',
    'done': '完成',
  };

  static const _restoreStageLabels = {
    'validate': '正在校验备份…',
    'extract': '正在解压备份…',
    'swap': '正在替换数据…',
    'done': '完成',
  };

  String _timestamp() {
    final now = DateTime.now();
    String two(int v) => v.toString().padLeft(2, '0');
    return '${now.year}${two(now.month)}${two(now.day)}'
        '-${two(now.hour)}${two(now.minute)}';
  }

  Future<Directory> _stagingDir() async {
    final docDir = await getApplicationDocumentsDirectory();
    final dir = Directory('${docDir.path}/backups');
    if (!await dir.exists()) {
      await dir.create(recursive: true);
    }
    return dir;
  }

  /// 弹出一个不可手动关闭的进度对话框，`body` 通过回调更新进度；
  /// `task` 结束后自动关闭对话框。
  Future<T?> _runWithProgress<T>({
    required String title,
    required Future<T> Function(void Function(_OpProgress) report) task,
  }) async {
    final progress = ValueNotifier<_OpProgress>(
      const _OpProgress('准备中…', 0, 0),
    );
    final dialogFuture = showDialog<void>(
      context: context,
      barrierDismissible: false,
      builder: (context) => PopScope(
        canPop: false,
        child: AlertDialog(
          content: ValueListenableBuilder<_OpProgress>(
            valueListenable: progress,
            builder: (context, p, _) => Row(
              children: [
                const SizedBox(
                  width: 28,
                  height: 28,
                  child: CircularProgressIndicator(strokeWidth: 3),
                ),
                const SizedBox(width: 16),
                Expanded(
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(title,
                          style: Theme.of(context).textTheme.titleMedium),
                      const SizedBox(height: 6),
                      Text(p.label,
                          style: TextStyle(
                              fontSize: 13,
                              color: context.palette.inkSoft)),
                      const SizedBox(height: 8),
                      LinearProgressIndicator(value: p.value),
                      if (p.text.isNotEmpty) ...[
                        const SizedBox(height: 4),
                        Text(p.text,
                            style: TextStyle(
                                fontSize: 12,
                                color: context.palette.inkSoft)),
                      ],
                    ],
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
    try {
      return await task((p) => progress.value = p);
    } finally {
      // 先关闭对话框，等其路由真正退出后再释放进度通知器，
      // 避免 ValueListenableBuilder 在 dispose 后仍收到帧回调。
      if (mounted && Navigator.of(context).canPop()) {
        Navigator.of(context).pop(); // 关闭进度对话框
      }
      await dialogFuture;
      progress.dispose();
    }
  }

  void _snack(String message) {
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(content: Text(message), behavior: SnackBarBehavior.floating),
    );
  }

  // -------------------------------------------------------------------------
  // 导出
  // -------------------------------------------------------------------------

  Future<void> _export() async {
    if (_busy) return;
    setState(() => _busy = true);
    File? staging;
    try {
      final dir = await _stagingDir();
      final fileName = 'findit-backup-${_timestamp()}.zip';
      staging = File('${dir.path}/$fileName');

      BackupSummary? summary;
      await _runWithProgress<void>(
        title: '正在导出备份',
        task: (report) async {
          await for (final p in backup.exportBackup(targetPath: staging!.path)) {
            report(_OpProgress(
              _exportStageLabels[p.stage] ?? p.stage,
              p.done,
              p.total,
            ));
            summary = p.summary ?? summary;
          }
        },
      );

      if (!mounted) return;
      // 通过系统分享面板移交文件路径，不把整个 zip 读入内存；
      // 分享成功则清理暂存文件；用户取消分享时保留暂存文件并提示位置（F10），
      // 避免「取消即丢失」的体验（README 已同步修订）。
      final result = await SharePlus.instance.share(
        ShareParams(files: [XFile(staging.path)]),
      );
      if (!mounted) return;
      final s = summary;
      if (result.status == ShareResultStatus.success) {
        try {
          if (await staging.exists()) await staging.delete();
        } catch (_) {}
        // F14：记录上次成功导出时间，用于下次进设置页时的备份提醒。
        try {
          await settings_api.setSetting(
            key: _kLastBackupAtKey,
            value: DateTime.now().toIso8601String(),
          );
        } catch (_) {}
        if (mounted) setState(() => _daysSinceLastBackup = 0);
        _snack(s == null
            ? '备份已保存'
            : '备份已保存：${s.unitsCount} 个单元、${s.boxesCount} 个箱、'
                '${s.itemsCount} 件物品、${s.photosCount} 张照片文件');
      } else {
        // 取消分享：保留暂存文件，提示位置以便稍后手动处理。
        _snack('备份已保留在应用备份目录，可稍后重新分享：\n${staging.path}');
      }
    } catch (e) {
      if (mounted) showErrorSnack(context, e);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  // -------------------------------------------------------------------------
  // 恢复
  // -------------------------------------------------------------------------

  Future<void> _restore() async {
    if (_busy) return;
    final files = await FilePicker.pickFiles(
      dialogTitle: '选择备份文件',
      type: FileType.custom,
      allowedExtensions: const ['zip'],
    );
    if (files.isEmpty) return;
    final picked = files.first;

    final confirmed = await _confirmRestore();
    if (confirmed != true) return;

    setState(() => _busy = true);
    File? staging;
    try {
      // 统一复制到应用暂存路径，规避 SAF content:// URI 无法被原生读取的问题。
      final dir = await _stagingDir();
      staging = File('${dir.path}/restore-${_timestamp()}.zip');
      if (picked.path != null) {
        await File(picked.path!).copy(staging.path);
      } else {
        // SAF content:// URI：流式分块写入暂存文件，避免整包 readAsBytes 入内存（P-M5）。
        final sink = staging.openWrite();
        try {
          await sink.addStream(picked.readAsByteStream());
        } finally {
          await sink.close();
        }
      }

      RestoreSummary? summary;
      await _runWithProgress<void>(
        title: '正在恢复备份',
        task: (report) async {
          await for (final p in backup.restoreBackup(zipPath: staging!.path)) {
            report(_OpProgress(
              _restoreStageLabels[p.stage] ?? p.stage,
              p.done,
              p.total,
            ));
            summary = p.summary ?? summary;
          }
        },
      );

      await staging.delete();
      if (!mounted) return;
      final s = summary;
      _snack(s == null
          ? '备份恢复完成'
          : '恢复完成：${s.unitsCount} 个单元、${s.boxesCount} 个箱、'
              '${s.itemsCount} 件物品。旧数据已保留副本。');
      // 回到首页并触发刷新（首页在 push 时用 then 监听返回）。
      Navigator.of(context).popUntil((route) => route.isFirst);
    } catch (e) {
      if (staging != null && await staging.exists()) {
        try {
          await staging.delete();
        } catch (_) {}
      }
      if (mounted) showErrorSnack(context, e);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<bool?> _confirmRestore() {
    return showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('恢复备份'),
        content: const Text(
          '该操作会用备份文件覆盖当前全部数据'
          '（存储单元、收纳箱、物品、分类与设置）。\n\n'
          '当前数据会自动保留一份副本，可在应用目录旁找到。确定继续吗？',
          style: TextStyle(height: 1.6),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('取消'),
          ),
          FilledButton(
            style: FilledButton.styleFrom(
              backgroundColor: context.palette.danger,
            ),
            onPressed: () => Navigator.of(context).pop(true),
            child: const Text('确认恢复'),
          ),
        ],
      ),
    );
  }

  // -------------------------------------------------------------------------

  @override
  Widget build(BuildContext context) {
    final palette = context.palette;
    return Scaffold(
      appBar: AppBar(title: const Text('设置')),
      body: ListView(
        padding: const EdgeInsets.fromLTRB(16, 12, 16, 48),
        children: [
          // F14：自动备份提醒（轻量）：超过 7 天未成功导出时提示。
          if (_daysSinceLastBackup case final int days? when days >= 7) ...[
            Card(
              color: palette.persimmon.withValues(alpha: 0.08),
              shape: RoundedRectangleBorder(
                borderRadius: BorderRadius.circular(14),
                side: BorderSide(
                  color: palette.persimmon.withValues(alpha: 0.5),
                ),
              ),
              child: Padding(
                padding: const EdgeInsets.fromLTRB(14, 10, 8, 10),
                child: Row(
                  children: [
                    Icon(Icons.backup_outlined, color: palette.persimmon),
                    const SizedBox(width: 10),
                    Expanded(
                      child: Text(
                        '距上次备份已超过 $days 天，'
                        '建议尽快导出备份以防数据丢失。',
                        style: const TextStyle(fontSize: 13, height: 1.5),
                      ),
                    ),
                    TextButton(
                      onPressed: _busy ? null : _export,
                      child: const Text('去备份'),
                    ),
                  ],
                ),
              ),
            ),
            const SizedBox(height: 16),
          ],
          _sectionTitle('数据备份'),
          Card(
            child: Padding(
              padding: const EdgeInsets.all(16),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    '备份包含数据库与全部照片（zip 格式）。恢复会覆盖当前数据，'
                    '旧数据自动保留副本。',
                    style: TextStyle(
                      fontSize: 13,
                      height: 1.6,
                      color: palette.inkSoft,
                    ),
                  ),
                  const SizedBox(height: 6),
                  Row(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Icon(Icons.warning_amber_rounded,
                          size: 15, color: palette.persimmon),
                      const SizedBox(width: 6),
                      Expanded(
                        child: Text(
                          '备份文件未加密，包含物品与照片信息，请妥善保管导出的 zip 文件。',
                          style: TextStyle(
                            fontSize: 12,
                            height: 1.5,
                            color: palette.persimmon,
                          ),
                        ),
                      ),
                    ],
                  ),
                  const SizedBox(height: 14),
                  Row(
                    children: [
                      Expanded(
                        child: FilledButton.icon(
                          onPressed: _busy ? null : _export,
                          icon: const Icon(Icons.file_upload_outlined),
                          label: const Text('导出备份'),
                        ),
                      ),
                      const SizedBox(width: 10),
                      Expanded(
                        child: OutlinedButton.icon(
                          onPressed: _busy ? null : _restore,
                          icon: const Icon(Icons.file_download_outlined),
                          label: const Text('恢复备份'),
                        ),
                      ),
                    ],
                  ),
                ],
              ),
            ),
          ),
          const SizedBox(height: 20),
          _sectionTitle('数据管理'),
          Card(
            child: ListTile(
              leading: Icon(Icons.label_outline_rounded, color: palette.pine),
              title: const Text('分类管理'),
              subtitle: Text(
                '重命名或删除分类标签',
                style: TextStyle(fontSize: 12, color: palette.inkSoft),
              ),
              trailing: const Icon(Icons.chevron_right_rounded),
              shape: RoundedRectangleBorder(
                borderRadius: BorderRadius.circular(14),
              ),
              onTap: () => Navigator.of(context).push(
                MaterialPageRoute(builder: (_) => const CategoriesPage()),
              ),
            ),
          ),
          const SizedBox(height: 20),
          _sectionTitle('智能助手'),
          Card(
            child: ListTile(
              leading: Icon(Icons.auto_awesome_rounded,
                  color: palette.pine),
              title: const Text('AI 设置'),
              subtitle: Text(
                '服务地址、模型与语义向量',
                style:
                    TextStyle(fontSize: 12, color: palette.inkSoft),
              ),
              trailing: const Icon(Icons.chevron_right_rounded),
              shape: RoundedRectangleBorder(
                borderRadius: BorderRadius.circular(14),
              ),
              onTap: () => Navigator.of(context).push(
                MaterialPageRoute(builder: (_) => const AiSettingsPage()),
              ),
            ),
          ),
        ],
      ),
    );
  }

  Widget _sectionTitle(String text) {
    return Padding(
      padding: const EdgeInsets.only(bottom: 8),
      child: Text(
        text,
        style: Theme.of(context).textTheme.labelSmall!.copyWith(
              color: context.palette.pineDeep,
              letterSpacing: 1.6,
            ),
      ),
    );
  }
}
