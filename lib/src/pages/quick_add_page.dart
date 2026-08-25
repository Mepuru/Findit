import 'package:flutter/material.dart';
import 'package:findit/src/rust/api/ai.dart' as ai;
import 'package:findit/src/rust/core/ai/parse.dart';

import '../errors.dart';
import '../theme.dart';

/// AI 快速录入：一句话 → 解析预览（可编辑修正）→ 确认后写库。
///
/// 建档（create_item）与修订（modify_item）复用同一页面模式，
/// 意图类型可在预览卡片中手动切换。
class QuickAddPage extends StatefulWidget {
  const QuickAddPage({super.key});

  @override
  State<QuickAddPage> createState() => _QuickAddPageState();
}

enum _Phase { input, parsing, preview, applying, done }

class _QuickAddPageState extends State<QuickAddPage> {
  final TextEditingController _inputController = TextEditingController();

  final TextEditingController _unitName = TextEditingController();
  final TextEditingController _boxName = TextEditingController();
  final TextEditingController _itemName = TextEditingController();
  final TextEditingController _itemDescription = TextEditingController();
  final TextEditingController _quantity = TextEditingController();

  final TextEditingController _targetQuery = TextEditingController();
  final TextEditingController _newUnitName = TextEditingController();
  final TextEditingController _newBoxName = TextEditingController();
  final TextEditingController _newItemName = TextEditingController();
  final TextEditingController _newDescription = TextEditingController();
  final TextEditingController _newQuantity = TextEditingController();

  _Phase _phase = _Phase.input;
  IntentKind _kind = IntentKind.createItem;
  Object? _error;
  String _summary = '';

  @override
  void dispose() {
    _inputController.dispose();
    _unitName.dispose();
    _boxName.dispose();
    _itemName.dispose();
    _itemDescription.dispose();
    _quantity.dispose();
    _targetQuery.dispose();
    _newUnitName.dispose();
    _newBoxName.dispose();
    _newItemName.dispose();
    _newDescription.dispose();
    _newQuantity.dispose();
    super.dispose();
  }

  Future<void> _parse() async {
    final text = _inputController.text.trim();
    if (text.isEmpty) return;
    setState(() {
      _phase = _Phase.parsing;
      _error = null;
    });
    try {
      final intent = await ai.parseQuickAdd(text: text);
      if (!mounted) return;
      _fillFrom(intent);
      setState(() {
        _kind = intent.intent;
        _phase = _Phase.preview;
      });
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _phase = _Phase.input;
        _error = e;
      });
    }
  }

  void _fillFrom(ParsedIntent intent) {
    _unitName.text = intent.unitName ?? '';
    _boxName.text = intent.boxName ?? '';
    _itemName.text = intent.itemName ?? '';
    _itemDescription.text = intent.itemDescription ?? '';
    _quantity.text = intent.quantity == null ? '' : '${intent.quantity}';
    _targetQuery.text = intent.targetQuery ?? '';
    _newUnitName.text = intent.newUnitName ?? '';
    _newBoxName.text = intent.newBoxName ?? '';
    _newItemName.text = intent.newItemName ?? '';
    _newDescription.text = intent.newDescription ?? '';
    _newQuantity.text = intent.newQuantity == null ? '' : '${intent.newQuantity}';
  }

  ParsedIntent _buildIntent() {
    int? parseQuantity(String raw) {
      final v = raw.trim();
      return v.isEmpty ? null : int.tryParse(v);
    }

    return ParsedIntent(
      intent: _kind,
      unitName: _unitName.text.trim().isEmpty ? null : _unitName.text.trim(),
      boxName: _boxName.text.trim().isEmpty ? null : _boxName.text.trim(),
      itemName: _itemName.text.trim().isEmpty ? null : _itemName.text.trim(),
      itemDescription: _itemDescription.text.trim().isEmpty
          ? null
          : _itemDescription.text.trim(),
      quantity: parseQuantity(_quantity.text),
      targetQuery:
          _targetQuery.text.trim().isEmpty ? null : _targetQuery.text.trim(),
      newUnitName:
          _newUnitName.text.trim().isEmpty ? null : _newUnitName.text.trim(),
      newBoxName:
          _newBoxName.text.trim().isEmpty ? null : _newBoxName.text.trim(),
      newItemName:
          _newItemName.text.trim().isEmpty ? null : _newItemName.text.trim(),
      newDescription: _newDescription.text.trim().isEmpty
          ? null
          : _newDescription.text.trim(),
      newQuantity: parseQuantity(_newQuantity.text),
    );
  }

  Future<void> _confirm() async {
    setState(() {
      _phase = _Phase.applying;
      _error = null;
    });
    final intent = _buildIntent();
    try {
      if (_kind == IntentKind.createItem) {
        final result = await ai.applyQuickAdd(intent: intent);
        final unitTag = result.unit.created ? '新建' : '复用';
        final boxTag = result.storageBox.created ? '新建' : '复用';
        _summary = '✅ 已入档「${result.item.name}」×${result.item.quantity}\n'
            '存储单元「${result.unit.name}」（$unitTag）→ '
            '收纳箱「${result.storageBox.name}」（$boxTag）';
      } else {
        final result = await ai.applyAiModify(intent: intent);
        final changes = result.changes.isEmpty ? '无变更' : result.changes.join('；');
        _summary = '✅ 已更新「${result.item.name}」\n$changes';
      }
      if (!mounted) return;
      setState(() => _phase = _Phase.done);
      // 写库成功后异步补齐向量，失败静默忽略（下次启动/手动补齐兜底）。
      // ignore: unawaited_futures
      ai.backfillPendingEmbeddings().catchError((_) => 0);
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _phase = _Phase.preview;
        _error = e;
      });
    }
  }

  void _restart() {
    for (final c in [
      _inputController,
      _unitName,
      _boxName,
      _itemName,
      _itemDescription,
      _quantity,
      _targetQuery,
      _newUnitName,
      _newBoxName,
      _newItemName,
      _newDescription,
      _newQuantity,
    ]) {
      c.clear();
    }
    setState(() {
      _phase = _Phase.input;
      _error = null;
      _summary = '';
    });
  }

  @override
  Widget build(BuildContext context) {
    final palette = context.palette;
    return Scaffold(
      appBar: AppBar(title: const Text('AI 快速录入')),
      body: ListView(
        padding: const EdgeInsets.fromLTRB(16, 12, 16, 48),
        children: [
          const Text(
            '说一句话，帮你整理入档',
            style: TextStyle(fontSize: 17, fontWeight: FontWeight.w800),
          ),
          const SizedBox(height: 6),
          Text(
            '如「把电钻放进车库的蓝色箱子」「把扳手移到杂物箱，数量改成 2」。'
            '也可以把语音转文字的结果粘贴进来。',
            style: TextStyle(fontSize: 13, color: palette.inkSoft),
          ),
          const SizedBox(height: 16),
          _buildPhaseBody(),
        ],
      ),
    );
  }

  Widget _buildPhaseBody() {
    switch (_phase) {
      case _Phase.input:
      case _Phase.parsing:
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            TextField(
              controller: _inputController,
              maxLines: 3,
              enabled: _phase == _Phase.input,
              decoration: InputDecoration(
                hintText: '输入一句话，如：把电钻放进车库的蓝色箱子',
                filled: true,
                fillColor: context.palette.cardFace,
              ),
              onSubmitted: (_) {
                if (_phase == _Phase.input) _parse();
              },
            ),
            const SizedBox(height: 12),
            FilledButton.icon(
              onPressed: _phase == _Phase.parsing ? null : _parse,
              icon: _phase == _Phase.parsing
                  ? const SizedBox(
                      width: 16,
                      height: 16,
                      child: CircularProgressIndicator(strokeWidth: 2),
                    )
                  : const Icon(Icons.auto_awesome),
              label: Text(_phase == _Phase.parsing ? '解析中…' : '解析'),
            ),
            if (_error != null) ...[
              const SizedBox(height: 12),
              _ErrorCard(error: _error!, onRetry: _parse),
            ],
          ],
        );
      case _Phase.preview:
      case _Phase.applying:
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            _PreviewCard(
              kind: _kind,
              busy: _phase == _Phase.applying,
              onKindChanged: (k) => setState(() => _kind = k),
              createFields: [
                _field(_unitName, '存储单元（可空）', '如：车库、阳台'),
                _field(_boxName, '收纳箱 *', '如：蓝色箱子'),
                _field(_itemName, '物品名称 *', '如：电钻'),
                _field(_itemDescription, '备注（可空）', '如：冲击钻，含充电器'),
                _field(_quantity, '数量（可空，默认 1）', '如：2',
                    keyboard: TextInputType.number),
              ],
              modifyFields: [
                _field(_targetQuery, '目标物品 *', '如：扳手'),
                _field(_newBoxName, '移入收纳箱（可空）', '如：杂物箱'),
                _field(_newUnitName, '移入存储单元（可空）', '如：阳台'),
                _field(_newItemName, '新名称（可空）', ''),
                _field(_newDescription, '新备注（可空）', ''),
                _field(_newQuantity, '新数量（可空）', '',
                    keyboard: TextInputType.number),
              ],
            ),
            const SizedBox(height: 12),
            if (_error != null) ...[
              _ErrorCard(error: _error!, onRetry: _confirm),
              const SizedBox(height: 12),
            ],
            FilledButton.icon(
              onPressed: _phase == _Phase.applying ? null : _confirm,
              icon: _phase == _Phase.applying
                  ? const SizedBox(
                      width: 16,
                      height: 16,
                      child: CircularProgressIndicator(strokeWidth: 2),
                    )
                  : const Icon(Icons.check_rounded),
              label: Text(_phase == _Phase.applying ? '保存中…' : '确认并保存'),
            ),
            const SizedBox(height: 8),
            OutlinedButton(
              onPressed:
                  _phase == _Phase.applying ? null : () => setState(() {
                _phase = _Phase.input;
                _error = null;
              }),
              child: const Text('返回修改描述'),
            ),
          ],
        );
      case _Phase.done:
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Card(
              child: Padding(
                padding: const EdgeInsets.all(16),
                child: Text(
                  _summary,
                  style: const TextStyle(fontSize: 14, height: 1.6),
                ),
              ),
            ),
            const SizedBox(height: 12),
            FilledButton.icon(
              onPressed: _restart,
              icon: const Icon(Icons.add),
              label: const Text('继续录入'),
            ),
            const SizedBox(height: 8),
            OutlinedButton(
              onPressed: () => Navigator.of(context).pop(),
              child: const Text('完成'),
            ),
          ],
        );
    }
  }

  Widget _field(
    TextEditingController controller,
    String label,
    String hint, {
    TextInputType keyboard = TextInputType.text,
  }) {
    return Padding(
      padding: const EdgeInsets.only(bottom: 10),
      child: TextField(
        controller: controller,
        keyboardType: keyboard,
        decoration: InputDecoration(
          labelText: label,
          hintText: hint.isEmpty ? null : hint,
          isDense: true,
          filled: true,
          fillColor: context.palette.cardFace,
        ),
      ),
    );
  }
}

/// 解析预览卡片：意图类型可切换，全部字段可编辑修正。
class _PreviewCard extends StatelessWidget {
  const _PreviewCard({
    required this.kind,
    required this.busy,
    required this.onKindChanged,
    required this.createFields,
    required this.modifyFields,
  });

  final IntentKind kind;
  final bool busy;
  final ValueChanged<IntentKind> onKindChanged;
  final List<Widget> createFields;
  final List<Widget> modifyFields;

  @override
  Widget build(BuildContext context) {
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              '解析结果预览',
              style: Theme.of(context).textTheme.titleMedium,
            ),
            const SizedBox(height: 4),
            Text(
              '请核对以下字段，可直接修改后再保存。带 * 的必填。',
              style: TextStyle(
                  fontSize: 12, color: context.palette.inkSoft),
            ),
            const SizedBox(height: 12),
            SegmentedButton<IntentKind>(
              segments: const [
                ButtonSegment(
                  value: IntentKind.createItem,
                  label: Text('新建物品'),
                  icon: Icon(Icons.add_box_outlined),
                ),
                ButtonSegment(
                  value: IntentKind.modifyItem,
                  label: Text('修改物品'),
                  icon: Icon(Icons.edit_outlined),
                ),
              ],
              selected: {kind},
              onSelectionChanged: busy
                  ? null
                  : (selection) => onKindChanged(selection.first),
            ),
            const SizedBox(height: 14),
            if (kind == IntentKind.createItem)
              ...createFields
            else
              ...modifyFields,
          ],
        ),
      ),
    );
  }
}

/// 错误提示卡片（含重试按钮）。
class _ErrorCard extends StatelessWidget {
  const _ErrorCard({required this.error, required this.onRetry});

  final Object error;
  final VoidCallback onRetry;

  @override
  Widget build(BuildContext context) {
    final palette = context.palette;
    return Container(
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: palette.danger.withValues(alpha: 0.1),
        borderRadius: BorderRadius.circular(10),
        border: Border.all(color: palette.danger.withValues(alpha: 0.4)),
      ),
      child: Row(
        children: [
          Icon(Icons.error_outline, color: palette.danger),
          const SizedBox(width: 10),
          Expanded(
            child: Text(
              friendlyErrorMessage(error),
              style: const TextStyle(fontSize: 13),
            ),
          ),
          TextButton(
            onPressed: onRetry,
            child: const Text('重试'),
          ),
        ],
      ),
    );
  }
}
