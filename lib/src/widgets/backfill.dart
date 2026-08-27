import 'dart:async';

import 'package:findit/src/rust/api/ai.dart' as ai;

Timer? _backfillTimer;

/// 写入触发的向量回填「防抖合并」入口。
///
/// 物品新增/修改后会触发语义向量补齐；连续多次写入时只应在最后一次写入
/// 结束并静默 [delay] 后执行一次批量回填，避免每次写入都立即发起网络调用。
/// 与 AI 设置页的手动「补齐」按钮（立即执行）互不影响。
void scheduleBackfill({Duration delay = const Duration(seconds: 3)}) {
  _backfillTimer?.cancel();
  _backfillTimer = Timer(delay, () {
    // 未配置/失败静默忽略（启动/手动补齐兜底）。
    // ignore: unawaited_futures
    ai.backfillPendingEmbeddings().catchError((_) => 0);
  });
}
