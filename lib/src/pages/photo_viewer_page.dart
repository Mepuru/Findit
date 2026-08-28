import 'dart:io';

import 'package:flutter/material.dart';

import '../errors.dart';
import '../widgets/common.dart';

/// 物品照片大图预览：沉浸式深色背景 + 双指缩放。
/// 返回 `true` 表示照片已被删除，调用方应刷新列表。
class PhotoViewerPage extends StatelessWidget {
  const PhotoViewerPage({
    super.key,
    required this.title,
    required this.photoPath,
    this.onDelete,
  });

  /// 物品名，展示在顶栏。
  final String title;

  /// 主图完整本地路径。
  final String photoPath;

  /// 删除回调：由调用方执行删除逻辑，返回是否成功删除。
  final Future<bool> Function()? onDelete;

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: const Color(0xFF171310),
      extendBodyBehindAppBar: true,
      appBar: AppBar(
        backgroundColor: Colors.transparent,
        foregroundColor: Colors.white,
        iconTheme: const IconThemeData(color: Colors.white),
        title: Text(
          title,
          style: const TextStyle(color: Colors.white, fontSize: 16),
        ),
        actions: [
          if (onDelete != null)
            IconButton(
              tooltip: '删除照片',
              icon: const Icon(Icons.delete_outline_rounded),
              onPressed: () => _confirmDelete(context),
            ),
        ],
      ),
      body: Center(
        child: InteractiveViewer(
          maxScale: 4,
          child: Image.file(
            File(photoPath),
            fit: BoxFit.contain,
            // P-L6：按屏幕物理像素宽度限制解码尺寸。主图 1600px 全尺寸
            // 解码在低端机内存/解码开销偏高；cacheWidth 取屏幕宽度 × DPR
            // 即可满足清晰度（约 1080–1440，超过原图宽度时按原图解码）。
            cacheWidth:
                (MediaQuery.sizeOf(context).width *
                        MediaQuery.devicePixelRatioOf(context))
                    .round(),
            errorBuilder: (context, error, stackTrace) => Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                const Text('🖼️', style: TextStyle(fontSize: 48)),
                const SizedBox(height: 12),
                Text(
                  '照片文件加载失败',
                  style: Theme.of(context)
                      .textTheme
                      .bodySmall!
                      .copyWith(color: Colors.white70),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }

  Future<void> _confirmDelete(BuildContext context) async {
    final confirmed = await confirmDelete(
      context,
      title: '删除这张照片？',
      message: '照片文件将被移除，且无法恢复。',
    );
    if (!confirmed) return;
    try {
      final deleted = await onDelete!();
      if (deleted && context.mounted) {
        Navigator.of(context).pop(true);
      }
    } catch (e) {
      if (context.mounted) showErrorSnack(context, e);
    }
  }
}
