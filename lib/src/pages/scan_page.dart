import 'package:flutter/material.dart';
import 'package:mobile_scanner/mobile_scanner.dart';
import 'package:findit/src/rust/api/boxes.dart' as boxes_api;

import '../errors.dart';
import '../theme.dart';
import 'items_page.dart';

/// 解析扫码内容。仅支持唯一格式 `findit://box/{slug}`，
/// 匹配成功返回标签码，否则返回 `null`。
String? parseFinditBoxSlug(String raw) {
  final uri = Uri.tryParse(raw.trim());
  if (uri == null || uri.scheme != 'findit' || uri.host != 'box') {
    return null;
  }
  if (uri.pathSegments.length != 1) return null;
  final slug = uri.pathSegments.single;
  return slug.isEmpty ? null : slug;
}

/// 扫码页无论亮/暗主题都保持深绿相机底色（沉浸式取景），固定不随主题变。
const Color _scanBackdrop = Color(0xFF24473D);

/// 扫码页：对准箱标签二维码，匹配 `findit://box/{slug}` 后直达对应收纳箱。
class ScanPage extends StatefulWidget {
  const ScanPage({super.key});

  @override
  State<ScanPage> createState() => _ScanPageState();
}

class _ScanPageState extends State<ScanPage> {
  final MobileScannerController _controller = MobileScannerController();

  /// 正在处理一次扫码（跳转/查询期间屏蔽后续识别）。
  bool _handling = false;
  String? _lastRaw;
  DateTime _lastAt = DateTime.fromMillisecondsSinceEpoch(0);

  Future<void> _onDetect(BarcodeCapture capture) async {
    if (_handling) return;
    final raw = capture.barcodes.firstOrNull?.rawValue;
    if (raw == null || raw.isEmpty) return;

    // 同一内容 3 秒内不重复提示（如对着无效码不动）。
    final now = DateTime.now();
    if (raw == _lastRaw && now.difference(_lastAt).inSeconds < 3) return;
    _lastRaw = raw;
    _lastAt = now;

    final slug = parseFinditBoxSlug(raw);
    if (slug == null) {
      _toast('这不是 Findit 箱标签，请对准收纳箱上的二维码。');
      return;
    }

    _handling = true;
    try {
      await _controller.stop();
    } catch (_) {}
    try {
      final box = await boxes_api.getBoxBySlug(slug: slug);
      if (!mounted) return;
      await Navigator.of(context).push(
        MaterialPageRoute(builder: (_) => ItemsPage(boxId: box.id.toInt())),
      );
    } catch (e) {
      if (mounted) showErrorSnack(context, e);
    } finally {
      _handling = false;
      if (mounted) {
        try {
          await _controller.start();
        } catch (_) {}
      }
    }
  }

  void _toast(String message) {
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(message),
        behavior: SnackBarBehavior.floating,
      ),
    );
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: _scanBackdrop,
      extendBodyBehindAppBar: true,
      appBar: AppBar(
        backgroundColor: Colors.transparent,
        foregroundColor: Colors.white,
        title: const Text(
          '扫描箱标签',
          style: TextStyle(color: Colors.white),
        ),
      ),
      body: Stack(
        fit: StackFit.expand,
        children: [
          MobileScanner(
            controller: _controller,
            onDetect: _onDetect,
            errorBuilder: (context, error) => Center(
              child: Padding(
                padding: const EdgeInsets.all(32),
                child: Text(
                  '无法打开相机：${error.errorDetails?.message ?? error.errorCode}',
                  textAlign: TextAlign.center,
                  style: const TextStyle(color: Colors.white70),
                ),
              ),
            ),
          ),
          const IgnorePointer(child: _ScanFrame()),
          Positioned(
            left: 0,
            right: 0,
            bottom: 36,
            child: Column(
              children: [
                Text(
                  '将二维码对准取景框',
                  style: Theme.of(context).textTheme.bodySmall!.copyWith(
                        color: Colors.white.withValues(alpha: 0.9),
                        letterSpacing: 2,
                      ),
                ),
                const SizedBox(height: 4),
                Text(
                  '仅识别 findit://box/… 格式的箱标签',
                  style: Theme.of(context).textTheme.labelSmall!.copyWith(
                        color: Colors.white.withValues(alpha: 0.55),
                        letterSpacing: 0.6,
                      ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

/// 取景框遮罩：四周压暗 + 柿子橙四角括号，呼应档案标签风格。
class _ScanFrame extends StatelessWidget {
  const _ScanFrame();

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, constraints) {
        const frame = 280.0;
        const corner = 34.0;
        final left = (constraints.maxWidth - frame) / 2;
        final top = (constraints.maxHeight - frame) / 2 - 40;
        return Stack(
          children: [
            ColorFiltered(
              colorFilter: ColorFilter.mode(
                _scanBackdrop.withValues(alpha: 0.55),
                BlendMode.srcOut,
              ),
              child: Stack(
                fit: StackFit.expand,
                children: [
                  Container(
                    decoration: const BoxDecoration(
                      color: Colors.black,
                      backgroundBlendMode: BlendMode.dstOut,
                    ),
                  ),
                  Positioned(
                    left: left,
                    top: top,
                    child: Container(
                      width: frame,
                      height: frame,
                      decoration: BoxDecoration(
                        color: Colors.black,
                        borderRadius: BorderRadius.circular(18),
                      ),
                    ),
                  ),
                ],
              ),
            ),
            _Corner(
              left: left,
              top: top,
              alignment: Alignment.topLeft,
              corner: corner,
            ),
            _Corner(
              left: left + frame - corner,
              top: top,
              alignment: Alignment.topRight,
              corner: corner,
            ),
            _Corner(
              left: left,
              top: top + frame - corner,
              alignment: Alignment.bottomLeft,
              corner: corner,
            ),
            _Corner(
              left: left + frame - corner,
              top: top + frame - corner,
              alignment: Alignment.bottomRight,
              corner: corner,
            ),
          ],
        );
      },
    );
  }
}

class _Corner extends StatelessWidget {
  const _Corner({
    required this.left,
    required this.top,
    required this.alignment,
    required this.corner,
  });

  final double left;
  final double top;
  final Alignment alignment;
  final double corner;

  @override
  Widget build(BuildContext context) {
    final radius = BorderRadius.only(
      topLeft: alignment == Alignment.topLeft ? const Radius.circular(18) : Radius.zero,
      topRight: alignment == Alignment.topRight ? const Radius.circular(18) : Radius.zero,
      bottomLeft: alignment == Alignment.bottomLeft ? const Radius.circular(18) : Radius.zero,
      bottomRight:
          alignment == Alignment.bottomRight ? const Radius.circular(18) : Radius.zero,
    );
    return Positioned(
      left: left,
      top: top,
      child: Container(
        width: corner,
        height: corner,
        decoration: BoxDecoration(
          border: Border.all(color: context.palette.persimmon, width: 4),
          borderRadius: radius,
        ),
      ),
    );
  }
}
