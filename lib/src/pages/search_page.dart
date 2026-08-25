import 'dart:async';
import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:findit/src/rust/api/ai.dart' as ai_api;
import 'package:findit/src/rust/api/model.dart';
import 'package:findit/src/rust/api/search.dart' as api;

import '../errors.dart';
import '../theme.dart';
import '../widgets/common.dart';
import 'items_page.dart';

/// 全局搜索：关键词 + 语义双通道。
///
/// 关键词搜索与查询向量生成并发进行：先展示关键词结果，
/// 向量到达后再补充语义结果重排（向量失败/未配置则保持纯关键词）。
class SearchPage extends StatefulWidget {
  const SearchPage({super.key});

  @override
  State<SearchPage> createState() => _SearchPageState();
}

enum _SearchState { idle, loading, loaded, failed }

class _SearchPageState extends State<SearchPage> {
  final TextEditingController _controller = TextEditingController();
  Timer? _debounce;

  /// 请求序号，丢弃过期响应（防抖期间的旧查询）。
  int _seq = 0;

  _SearchState _state = _SearchState.idle;
  List<SearchResult> _results = const [];
  Object? _error;

  @override
  void dispose() {
    _debounce?.cancel();
    _controller.dispose();
    super.dispose();
  }

  void _onChanged(String value) {
    _debounce?.cancel();
    final query = value.trim();
    if (query.isEmpty) {
      _seq++;
      setState(() {
        _state = _SearchState.idle;
        _results = const [];
        _error = null;
      });
      return;
    }
    _debounce = Timer(const Duration(milliseconds: 300), () => _search(query));
  }

  Future<void> _search(String query) async {
    final seq = ++_seq;
    setState(() {
      _state = _SearchState.loading;
      _error = null;
    });
    try {
      // 向量生成与关键词搜索并发：不等向量就先出关键词结果，
      // AI 不可达时搜索不再被超时阻塞。
      final embeddingFuture = _safeQueryEmbedding(query);
      final keywordResults = await api.searchItems(query: query);
      if (!mounted || seq != _seq) return;
      setState(() {
        _state = _SearchState.loaded;
        _results = keywordResults;
      });
      // 向量到达后补充语义结果（去重后重排展示）。
      final embedding = await embeddingFuture;
      if (!mounted || seq != _seq || embedding == null) return;
      final merged =
          await api.searchItems(query: query, embedding: embedding);
      if (!mounted || seq != _seq) return;
      setState(() {
        _results = merged;
      });
    } catch (e) {
      if (!mounted || seq != _seq) return;
      setState(() {
        _state = _SearchState.failed;
        _error = e;
      });
    }
  }

  /// 生成查询向量：未配置 / 超时 / 失败均返回 null（降级为纯关键词）。
  Future<Float32List?> _safeQueryEmbedding(String query) async {
    try {
      return await ai_api.generateQueryEmbedding(text: query);
    } catch (_) {
      return null;
    }
  }

  void _openResult(SearchResult result) {
    Navigator.of(context)
        .push(MaterialPageRoute(
            builder: (_) => ItemsPage(boxId: result.boxId.toInt())))
        .then((_) {
      if (!mounted) return;
      // 返回后结果可能过期，用当前输入重新检索。
      final query = _controller.text.trim();
      if (query.isNotEmpty) _search(query);
    });
  }

  @override
  Widget build(BuildContext context) {
    final palette = context.palette;
    return Scaffold(
      appBar: AppBar(
        title: const Text('搜索'),
        actions: [
          if (_controller.text.isNotEmpty)
            IconButton(
              tooltip: '清空',
              icon: const Icon(Icons.close_rounded),
              onPressed: () {
                _controller.clear();
                _onChanged('');
              },
            ),
          const SizedBox(width: 4),
        ],
        bottom: PreferredSize(
          preferredSize: const Size.fromHeight(64),
          child: Padding(
            padding: const EdgeInsets.fromLTRB(16, 0, 16, 12),
            child: TextField(
              controller: _controller,
              autofocus: true,
              onChanged: _onChanged,
              textInputAction: TextInputAction.search,
              onSubmitted: (value) {
                _debounce?.cancel();
                final query = value.trim();
                if (query.isNotEmpty) _search(query);
              },
              decoration: InputDecoration(
                hintText: '找什么？如：扳手、冬装、蓝色 箱子',
                prefixIcon:
                    Icon(Icons.search_rounded, color: palette.inkSoft),
                filled: true,
                fillColor: palette.cardFace,
              ),
            ),
          ),
        ),
      ),
      body: _buildBody(),
    );
  }

  Widget _buildBody() {
    switch (_state) {
      case _SearchState.idle:
        return const _IdleHint();
      case _SearchState.loading:
        return const Padding(
          padding: EdgeInsets.only(top: 48),
          child: Center(child: CircularProgressIndicator()),
        );
      case _SearchState.failed:
        return EmptyState(
          emoji: '⚠️',
          title: '搜索失败',
          subtitle: friendlyErrorMessage(_error!),
          actionLabel: '重试',
          onAction: () => _search(_controller.text.trim()),
        );
      case _SearchState.loaded:
        if (_results.isEmpty) {
          return EmptyState(
            emoji: '🔍',
            title: '没有匹配的物品',
            subtitle:
                '换个说法试试，比如用「工具」代替「扳手」。\n多个词用空格隔开，会同时满足所有词。',
          );
        }
        final semantic = _results
            .where((r) => r.matchedBy == MatchedBy.semantic)
            .toList();
        final keyword = _results
            .where((r) => r.matchedBy == MatchedBy.keyword)
            .toList();
        return ListView(
          padding: const EdgeInsets.fromLTRB(16, 12, 16, 48),
          children: [
            if (semantic.isNotEmpty) ...[
              _SectionHeader(label: '语义匹配', count: semantic.length),
              const SizedBox(height: 8),
              for (final r in semantic) ...[
                _ResultCard(result: r, onTap: () => _openResult(r)),
                const SizedBox(height: 10),
              ],
            ],
            if (keyword.isNotEmpty) ...[
              _SectionHeader(label: '关键词匹配', count: keyword.length),
              const SizedBox(height: 8),
              for (final r in keyword) ...[
                _ResultCard(result: r, onTap: () => _openResult(r)),
                const SizedBox(height: 10),
              ],
            ],
          ],
        );
    }
  }
}

/// 空查询引导：档案卡片风格的检索提示。
class _IdleHint extends StatelessWidget {
  const _IdleHint();

  @override
  Widget build(BuildContext context) {
    final palette = context.palette;
    return ListView(
      padding: const EdgeInsets.fromLTRB(16, 20, 16, 48),
      children: [
        const Text(
          '跨全部收纳箱检索',
          style: TextStyle(
            fontSize: 17,
            fontWeight: FontWeight.w800,
          ),
        ),
        const SizedBox(height: 6),
        Text(
          '输入物品名称、描述或分类，空格分隔多个词。',
          style: TextStyle(fontSize: 13, color: palette.inkSoft),
        ),
        const SizedBox(height: 20),
        _hintRow('🧰', '试试搜「工具」', '命中分类名同样有效'),
        _hintRow('🔤', '大小写无所谓', '「drill」也能找到「Power Drill」'),
        _hintRow('🧩', '多词逐条满足', '「蓝色 箱子」两个词都要命中'),
        _hintRow('🧠', '语义搜索已上线', '搜「修水管的」也能找到「扳手」'),
      ],
    );
  }

  Widget _hintRow(String emoji, String title, String subtitle) {
    return Padding(
      padding: const EdgeInsets.only(bottom: 10),
      child: Builder(
        builder: (context) {
          final palette = context.palette;
          return Row(
            children: [
              Container(
                width: 40,
                height: 40,
                alignment: Alignment.center,
                decoration: BoxDecoration(
                  color: palette.chipFill,
                  borderRadius: BorderRadius.circular(12),
                ),
                child: Text(emoji, style: const TextStyle(fontSize: 20)),
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(title,
                        style: const TextStyle(
                            fontSize: 14, fontWeight: FontWeight.w700)),
                    const SizedBox(height: 2),
                    Text(
                      subtitle,
                      style: TextStyle(
                          fontSize: 12, color: palette.inkSoft),
                    ),
                  ],
                ),
              ),
            ],
          );
        },
      ),
    );
  }
}

/// 分区标题：编号式标签 + 虚线延伸，档案卷宗感。
class _SectionHeader extends StatelessWidget {
  const _SectionHeader({required this.label, required this.count});

  final String label;
  final int count;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(top: 8, bottom: 2),
      child: Row(
        children: [
          Text(
            '$label · $count',
            style: Theme.of(context).textTheme.labelSmall!.copyWith(
                  color: context.palette.pineDeep,
                  letterSpacing: 1.6,
                ),
          ),
          const SizedBox(width: 10),
          const Expanded(child: _DashedLine()),
        ],
      ),
    );
  }
}

class _DashedLine extends StatelessWidget {
  const _DashedLine();

  @override
  Widget build(BuildContext context) {
    return CustomPaint(
      size: const Size.fromHeight(1),
      painter: _DashPainter(color: context.palette.cardLine),
    );
  }
}

class _DashPainter extends CustomPainter {
  const _DashPainter({required this.color});

  final Color color;

  @override
  void paint(Canvas canvas, Size size) {
    final paint = Paint()
      ..color = color
      ..strokeWidth = 1.4;
    const dash = 5.0;
    const gap = 4.0;
    var x = 0.0;
    while (x < size.width) {
      canvas.drawLine(
        Offset(x, size.height / 2),
        Offset((x + dash).clamp(0.0, size.width), size.height / 2),
        paint,
      );
      x += dash + gap;
    }
  }

  @override
  bool shouldRepaint(_DashPainter oldDelegate) => oldDelegate.color != color;
}

/// 单条搜索结果卡片：名称 / 描述 / 分类标签 / 所在位置，
/// 语义命中额外带「匹配度 N%」柿子橙徽章。
class _ResultCard extends StatelessWidget {
  const _ResultCard({required this.result, required this.onTap});

  final SearchResult result;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = context.palette;
    final isSemantic = result.matchedBy == MatchedBy.semantic;
    return Card(
      child: InkWell(
        borderRadius: BorderRadius.circular(14),
        onTap: onTap,
        child: Padding(
          padding: const EdgeInsets.fromLTRB(16, 12, 12, 12),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                children: [
                  Expanded(
                    child: Text(
                      result.name,
                      style: Theme.of(context).textTheme.titleMedium,
                    ),
                  ),
                  if (isSemantic && result.similarityPercent != null)
                    _SimilarityBadge(percent: result.similarityPercent!),
                  if (!isSemantic)
                    Icon(Icons.chevron_right_rounded,
                        color: palette.inkSoft.withValues(alpha: 0.6)),
                ],
              ),
              if (result.description.isNotEmpty) ...[
                const SizedBox(height: 3),
                Text(
                  result.description,
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: Theme.of(context).textTheme.bodySmall!.copyWith(
                        color: palette.inkSoft,
                      ),
                ),
              ],
              const SizedBox(height: 8),
              Wrap(
                spacing: 6,
                runSpacing: 6,
                crossAxisAlignment: WrapCrossAlignment.center,
                children: [
                  for (final cat in result.categories) _TagChip(text: cat),
                  _LocationChip(
                    text: '${result.unitName} · ${result.boxName}',
                  ),
                  if (result.quantity > 1)
                    _TagChip(text: '×${result.quantity}'),
                ],
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// 语义匹配度徽章（柿子橙，标签样式）。
class _SimilarityBadge extends StatelessWidget {
  const _SimilarityBadge({required this.percent});

  final int percent;

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 5),
      decoration: BoxDecoration(
        color: context.palette.persimmon,
        borderRadius: BorderRadius.circular(999),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          const Icon(Icons.auto_awesome, size: 13, color: Colors.white),
          const SizedBox(width: 4),
          Text(
            '匹配度 $percent%',
            style: const TextStyle(
              color: Colors.white,
              fontSize: 12,
              fontWeight: FontWeight.w700,
              letterSpacing: 0.3,
            ),
          ),
        ],
      ),
    );
  }
}

/// 分类名标签（松木绿描边）。
class _TagChip extends StatelessWidget {
  const _TagChip({required this.text});

  final String text;

  @override
  Widget build(BuildContext context) {
    final palette = context.palette;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(6),
        border: Border.all(color: palette.pine.withValues(alpha: 0.55)),
      ),
      child: Text(
        text,
        style: TextStyle(
          fontSize: 11,
          fontWeight: FontWeight.w600,
          color: palette.pineDeep,
        ),
      ),
    );
  }
}

/// 所在位置标签（单元 · 箱）。
class _LocationChip extends StatelessWidget {
  const _LocationChip({required this.text});

  final String text;

  @override
  Widget build(BuildContext context) {
    final palette = context.palette;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
      decoration: BoxDecoration(
        color: palette.chipFill,
        borderRadius: BorderRadius.circular(6),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(Icons.place_outlined, size: 12, color: palette.inkSoft),
          const SizedBox(width: 3),
          Flexible(
            child: Text(
              text,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                fontSize: 11,
                fontWeight: FontWeight.w600,
                color: palette.inkSoft,
              ),
            ),
          ),
        ],
      ),
    );
  }
}
