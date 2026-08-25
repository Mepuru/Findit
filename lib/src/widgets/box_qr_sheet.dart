import 'dart:typed_data';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:path_provider/path_provider.dart';
import 'package:qr_flutter/qr_flutter.dart';
import 'package:share_plus/share_plus.dart';
import 'dart:io';

import '../errors.dart';
import '../theme.dart';
import 'package:findit/src/rust/api/model.dart';

/// 二维码内容格式：`findit://box/{slug}`（唯一支持的格式）。
String finditBoxQrData(String slug) => 'findit://box/$slug';

/// 弹出「箱标签」卡片：二维码 + 箱名，可导出 PNG 并唤起系统分享。
Future<void> showBoxQrSheet(BuildContext context, {required StorageBox box}) {
  return showModalBottomSheet<void>(
    context: context,
    isScrollControlled: true,
    backgroundColor: FinditColors.cardFace,
    shape: const RoundedRectangleBorder(
      borderRadius: BorderRadius.vertical(top: Radius.circular(22)),
      side: BorderSide(color: FinditColors.cardLine),
    ),
    builder: (_) => _BoxQrSheet(box: box),
  );
}

class _BoxQrSheet extends StatefulWidget {
  const _BoxQrSheet({required this.box});

  final StorageBox box;

  @override
  State<_BoxQrSheet> createState() => _BoxQrSheetState();
}

class _BoxQrSheetState extends State<_BoxQrSheet> {
  final GlobalKey _boundaryKey = GlobalKey();
  bool _exporting = false;

  /// 将标签卡截图为约 512px 白底 PNG，写入临时目录并唤起系统分享。
  Future<void> _exportAndShare() async {
    final boundaryCtx = _boundaryKey.currentContext;
    if (boundaryCtx == null || _exporting) return;
    setState(() => _exporting = true);
    try {
      final boundary = boundaryCtx.findRenderObject() as RenderRepaintBoundary;
      final image = await boundary.toImage(pixelRatio: 2.0);
      final byteData = await image.toByteData(format: ui.ImageByteFormat.png);
      if (byteData == null) {
        throw Exception('截图失败');
      }
      final Uint8List png = byteData.buffer.asUint8List();

      final tmpDir = await getTemporaryDirectory();
      final file = File('${tmpDir.path}/findit-box-${widget.box.slug}.png');
      await file.writeAsBytes(png, flush: true);

      await SharePlus.instance.share(
        ShareParams(
          files: [XFile(file.path)],
          text: 'Findit 箱标签：「${widget.box.name}」，打印后贴在箱子上即可扫码直达。',
        ),
      );
    } catch (e) {
      if (mounted) showErrorSnack(context, e);
    } finally {
      if (mounted) setState(() => _exporting = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final box = widget.box;
    return Padding(
      padding: const EdgeInsets.fromLTRB(24, 16, 24, 32),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Center(
            child: Container(
              width: 36,
              height: 4,
              decoration: BoxDecoration(
                color: FinditColors.cardLine,
                borderRadius: BorderRadius.circular(2),
              ),
            ),
          ),
          const SizedBox(height: 16),
          Text('箱标签', style: Theme.of(context).textTheme.titleMedium),
          const SizedBox(height: 4),
          Text(
            '打印或分享这张标签，贴上后一扫即可打开「${box.name}」。',
            style: Theme.of(context).textTheme.bodySmall!.copyWith(
                  color: FinditColors.inkSoft,
                ),
          ),
          const SizedBox(height: 20),
          Center(
            child: RepaintBoundary(
              key: _boundaryKey,
              child: _QrLabel(box: box),
            ),
          ),
          const SizedBox(height: 24),
          FilledButton.icon(
            onPressed: _exporting ? null : _exportAndShare,
            icon: _exporting
                ? const SizedBox(
                    width: 16,
                    height: 16,
                    child: CircularProgressIndicator(strokeWidth: 2),
                  )
                : const Icon(Icons.share_rounded, size: 18),
            label: Text(_exporting ? '正在生成图片…' : '导出图片并分享'),
          ),
        ],
      ),
    );
  }
}

/// 可打印的实体标签卡：白底 + 虚线描边 + 二维码 + 箱名 + 短标签码。
class _QrLabel extends StatelessWidget {
  const _QrLabel({required this.box});

  final StorageBox box;

  @override
  Widget build(BuildContext context) {
    return Container(
      width: 260,
      padding: const EdgeInsets.fromLTRB(20, 20, 20, 16),
      color: Colors.white,
      child: Container(
        padding: const EdgeInsets.fromLTRB(16, 18, 16, 14),
        decoration: BoxDecoration(
          border: Border.all(
            color: FinditColors.pine,
            width: 1.6,
            strokeAlign: BorderSide.strokeAlignInside,
          ),
          borderRadius: BorderRadius.circular(6),
        ),
        child: Column(
          children: [
            QrImageView(
              data: finditBoxQrData(box.slug),
              size: 176,
              padding: EdgeInsets.zero,
              backgroundColor: Colors.white,
            ),
            const SizedBox(height: 12),
            Text(
              box.name,
              textAlign: TextAlign.center,
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: const TextStyle(
                fontSize: 18,
                fontWeight: FontWeight.w800,
                color: FinditColors.ink,
                height: 1.2,
              ),
            ),
            const SizedBox(height: 6),
            Text(
              'FINDIT · 标签码 ${box.slug.substring(0, 8).toUpperCase()}',
              style: const TextStyle(
                fontSize: 10,
                letterSpacing: 1.6,
                fontWeight: FontWeight.w600,
                color: FinditColors.inkSoft,
              ),
            ),
          ],
        ),
      ),
    );
  }
}
