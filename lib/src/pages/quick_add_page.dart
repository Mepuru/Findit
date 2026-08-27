import 'dart:async';

import 'package:flutter/material.dart';
import 'package:findit/src/rust/api/ai.dart' as ai;
import 'package:findit/src/rust/api/model.dart';
import 'package:findit/src/rust/api/search.dart' as search_api;
import 'package:findit/src/rust/core/ai/parse.dart';
import 'package:findit/src/rust/core/error.dart';
import 'package:speech_to_text/speech_recognition_result.dart';
import 'package:speech_to_text/speech_to_text.dart';

import '../errors.dart';
import '../theme.dart';
import '../widgets/backfill.dart';
import 'ai_settings_page.dart';

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

  /// 修改意图的候选目标物品（按 `target_query` 关键词检索，供预览选择）。
  List<SearchResult> _candidates = const [];
  int? _selectedCandidateId;
  bool _candidatesLoading = false;
  Object? _candidatesError;
  Timer? _candidatesDebounce;

  /// 解析期间用户点击「取消」标记：pending 的解析结果将被丢弃。
  bool _parseCancelled = false;

  // ------------------ 语音输入 ------------------
  final SpeechToText _speech = SpeechToText();
  bool _listening = false;
  bool _speechReady = false;

  @override
  void initState() {
    super.initState();
    // 目标物品描述变化后（防抖）重新检索候选，保证选择与输入一致。
    _targetQuery.addListener(_onTargetQueryChanged);
  }

  void _onTargetQueryChanged() {
    if (_phase != _Phase.preview || _kind != IntentKind.modifyItem) return;
    _candidatesDebounce?.cancel();
    _candidatesDebounce =
        Timer(const Duration(milliseconds: 300), () => _loadCandidates());
  }

  @override
  void dispose() {
    _targetQuery.removeListener(_onTargetQueryChanged);
    _candidatesDebounce?.cancel();
    // 离开页面时停止未结束的识别会话。
    _speech.stop();
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
    _parseCancelled = false;
    setState(() {
      _phase = _Phase.parsing;
      _error = null;
    });
    try {
      final intent = await ai.parseQuickAdd(text: text);
      if (!mounted || _parseCancelled) return;
      _fillFrom(intent);
      setState(() {
        _kind = intent.intent;
        _phase = _Phase.preview;
      });
      if (_kind == IntentKind.modifyItem) {
        _loadCandidates();
      }
    } catch (e) {
      if (!mounted || _parseCancelled) return;
      setState(() {
        _phase = _Phase.input;
        _error = e;
      });
    }
  }

  /// 解析期间取消：回到输入态；pending 的解析结果到达后被丢弃。
  void _cancelParsing() {
    _parseCancelled = true;
    setState(() {
      _phase = _Phase.input;
      _error = null;
    });
  }

  /// 按 `target_query` 检索修改意图的候选目标物品（纯关键词，不依赖 AI）。
  Future<void> _loadCandidates() async {
    final query = _targetQuery.text.trim();
    if (query.isEmpty) {
      setState(() {
        _candidates = const [];
        _selectedCandidateId = null;
        _candidatesLoading = false;
        _candidatesError = null;
      });
      return;
    }
    setState(() {
      _candidatesLoading = true;
      _candidatesError = null;
    });
    try {
      final results = await search_api.searchItems(query: query);
      if (!mounted || query != _targetQuery.text.trim()) return;
      setState(() {
        _candidates = results;
        _candidatesLoading = false;
        // 默认选中：与目标描述精确同名的候选优先，否则取第一个。
        _selectedCandidateId = _pickDefaultCandidate(results, query);
      });
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _candidatesLoading = false;
        _candidatesError = e;
      });
    }
  }

  int? _pickDefaultCandidate(List<SearchResult> results, String query) {
    for (final r in results) {
      if (r.name == query) return r.id.toInt();
    }
    return results.isEmpty ? null : results.first.id.toInt();
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

  ParsedIntent _buildIntent({String? targetQueryOverride}) {
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
      targetQuery: targetQueryOverride ??
          (_targetQuery.text.trim().isEmpty ? null : _targetQuery.text.trim()),
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
    // 用户在候选列表中选定了目标物品：用其确切名称精确定位，
    // 避免同名/相似物品被关键词排序误改。
    final pickedName = (_kind == IntentKind.modifyItem &&
            _selectedCandidateId != null)
        ? _candidates
            .where((c) => c.id.toInt() == _selectedCandidateId)
            .firstOrNull
            ?.name
        : null;
    final intent = _buildIntent(targetQueryOverride: pickedName);
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
      // 写库后向量回填做防抖合并，避免每次写入立即发起网络调用。
      scheduleBackfill();
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _phase = _Phase.preview;
        _error = e;
      });
    }
  }

  /// 切换语音识别：中文（zh_CN）优先，失败回退系统默认语言；
  /// 识别不可用（无权限/不支持）时提示但不影响文本输入。
  Future<void> _toggleSpeech() async {
    if (_listening) {
      await _speech.stop();
      if (mounted) setState(() => _listening = false);
      return;
    }
    if (!_speechReady) {
      bool ok = false;
      try {
        ok = await _speech.initialize();
      } catch (_) {
        ok = false;
      }
      if (!ok || !mounted) {
        if (mounted) {
          _snack('语音输入不可用：请授予麦克风权限或检查设备支持');
        }
        return;
      }
      _speechReady = true;
    }
    setState(() => _listening = true);

    void onResult(SpeechRecognitionResult r) {
      if (!mounted) return;
      final words = r.recognizedWords.trim();
      if (words.isNotEmpty) {
        _inputController.text = words;
        _inputController.selection = TextSelection.fromPosition(
          TextPosition(offset: _inputController.text.length),
        );
      }
      if (r.finalResult) setState(() => _listening = false);
    }

    try {
      // 优先中文识别。
      await _speech.listen(
        onResult: onResult,
        listenOptions: SpeechListenOptions(
          localeId: 'zh_CN',
          partialResults: true,
        ),
      );
    } catch (_) {
      try {
        // zh_CN 不可用时回退系统默认语言。
        await _speech.listen(
          onResult: onResult,
          listenOptions: SpeechListenOptions(partialResults: true),
        );
      } catch (_) {
        if (mounted) {
          setState(() => _listening = false);
          _snack('启动语音识别失败，请直接输入文字');
        }
      }
    }
  }

  void _snack(String message) {
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(content: Text(message), behavior: SnackBarBehavior.floating),
    );
  }

  /// 错误是否为 AI 相关（未配置 / 不可达 / 模型输出异常）：
  /// 是则错误卡附「前往 AI 设置」引导。
  bool _isAiError(Object error) {
    if (error is! FinditError) return false;
    return error.when(
      dbNotInitialized: () => false,
      db: (_) => false,
      duplicateName: (_, _) => false,
      notFound: (_, _) => false,
      validation: (_) => false,
      io: (_) => false,
      aiNotConfigured: (_) => true,
      aiUnreachable: (_) => true,
      aiModelOutput: (_) => true,
    );
  }

  void _openAiSettings() {
    Navigator.of(context).push(
      MaterialPageRoute(builder: (_) => const AiSettingsPage()),
    );
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
      _candidates = const [];
      _selectedCandidateId = null;
      _candidatesError = null;
      _candidatesLoading = false;
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
                suffixIcon: IconButton(
                  tooltip: _listening ? '停止语音输入' : '语音输入',
                  icon: Icon(
                    _listening ? Icons.stop_circle_rounded : Icons.mic_rounded,
                    color: _listening ? context.palette.danger : null,
                  ),
                  onPressed: _phase == _Phase.input ? _toggleSpeech : null,
                ),
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
            if (_phase == _Phase.parsing) ...[
              const SizedBox(height: 8),
              TextButton(
                onPressed: _cancelParsing,
                child: const Text('取消（放弃本次解析）'),
              ),
            ],
            if (_error != null) ...[
              const SizedBox(height: 12),
              _ErrorCard(
                error: _error!,
                onRetry: _parse,
                onSettings: _isAiError(_error!) ? _openAiSettings : null,
              ),
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
              modifyHeader: _kind == IntentKind.modifyItem
                  ? _CandidateSelector(
                      candidates: _candidates,
                      selectedId: _selectedCandidateId,
                      loading: _candidatesLoading,
                      error: _candidatesError,
                      busy: _phase == _Phase.applying,
                      onSelect: (id) =>
                          setState(() => _selectedCandidateId = id),
                      onRefresh: _loadCandidates,
                    )
                  : null,
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
              _ErrorCard(
                error: _error!,
                onRetry: _confirm,
                onSettings: _isAiError(_error!) ? _openAiSettings : null,
              ),
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

/// 解析预览卡片：意图类型可切换，全部字段可编辑修正；
/// 修改意图时顶部展示「目标物品候选」选择区。
class _PreviewCard extends StatelessWidget {
  const _PreviewCard({
    required this.kind,
    required this.busy,
    required this.onKindChanged,
    required this.createFields,
    required this.modifyFields,
    this.modifyHeader,
  });

  final IntentKind kind;
  final bool busy;
  final ValueChanged<IntentKind> onKindChanged;
  final List<Widget> createFields;
  final List<Widget> modifyFields;

  /// 修改意图的目标候选区（名称/所在箱/命中数 + 多候选切换）。
  final Widget? modifyHeader;

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
            else ...[
              if (modifyHeader != null) ...[
                modifyHeader!,
                const SizedBox(height: 14),
              ],
              ...modifyFields,
            ],
          ],
        ),
      ),
    );
  }
}

/// 修改意图的目标物品候选区：展示命中数、候选（名称/所在位置/数量），
/// 支持切换选择；存在同名物品时提示「将修改最早登记的一件」。
class _CandidateSelector extends StatelessWidget {
  const _CandidateSelector({
    required this.candidates,
    required this.selectedId,
    required this.loading,
    required this.error,
    required this.busy,
    required this.onSelect,
    required this.onRefresh,
  });

  final List<SearchResult> candidates;
  final int? selectedId;
  final bool loading;
  final Object? error;
  final bool busy;
  final ValueChanged<int> onSelect;
  final VoidCallback onRefresh;

  /// 是否存在同名物品（需提示「按最早登记修改」）。
  bool get _hasDuplicateNames {
    final names = <String, int>{};
    for (final c in candidates) {
      names[c.name] = (names[c.name] ?? 0) + 1;
    }
    return names.values.any((n) => n > 1);
  }

  @override
  Widget build(BuildContext context) {
    final palette = context.palette;
    Widget? body;
    if (loading) {
      body = const Row(
        children: [
          SizedBox(
            width: 16,
            height: 16,
            child: CircularProgressIndicator(strokeWidth: 2),
          ),
          SizedBox(width: 10),
          Text('正在定位目标物品…', style: TextStyle(fontSize: 13)),
        ],
      );
    } else if (error != null) {
      body = Row(
        children: [
          Expanded(
            child: Text(
              '候选加载失败：${friendlyErrorMessage(error!)}',
              style: TextStyle(fontSize: 12, color: palette.danger),
            ),
          ),
          TextButton(onPressed: onRefresh, child: const Text('重试')),
        ],
      );
    } else if (candidates.isEmpty) {
      body = Text(
        '未找到匹配「目标物品」的已有物品，请调整描述后再保存。',
        style: TextStyle(fontSize: 12, color: palette.inkSoft),
      );
    } else {
      body = Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            '目标物品（命中 ${candidates.length} 个，选择后按其确切名称修改）',
            style: Theme.of(context).textTheme.labelSmall!.copyWith(
                  color: palette.pineDeep,
                  letterSpacing: 0.4,
                ),
          ),
          if (_hasDuplicateNames) ...[
            const SizedBox(height: 4),
            Text(
              '⚠️ 存在同名物品，将修改最早登记的一件。',
              style: TextStyle(fontSize: 12, color: palette.persimmon),
            ),
          ],
          const SizedBox(height: 8),
          for (final c in candidates)
            _candidateTile(context, c),
        ],
      );
    }
    return Container(
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: palette.cardFace,
        borderRadius: BorderRadius.circular(10),
        border: Border.all(color: palette.cardLine),
      ),
      child: body,
    );
  }

  Widget _candidateTile(BuildContext context, SearchResult c) {
    final palette = context.palette;
    final selected = selectedId == c.id.toInt();
    return Padding(
      padding: const EdgeInsets.only(bottom: 6),
      child: InkWell(
        borderRadius: BorderRadius.circular(8),
        onTap: busy ? null : () => onSelect(c.id.toInt()),
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
          decoration: BoxDecoration(
            color: selected
                ? palette.pine.withValues(alpha: 0.12)
                : palette.paper.withValues(alpha: 0.5),
            borderRadius: BorderRadius.circular(8),
            border: Border.all(
              color: selected ? palette.pine : palette.cardLine,
            ),
          ),
          child: Row(
            children: [
              Icon(
                selected
                    ? Icons.radio_button_checked
                    : Icons.radio_button_off,
                size: 18,
                color: selected ? palette.pine : palette.inkSoft,
              ),
              const SizedBox(width: 8),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      c.name,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: const TextStyle(
                          fontSize: 13, fontWeight: FontWeight.w700),
                    ),
                    const SizedBox(height: 2),
                    Text(
                      '${c.unitName} · ${c.boxName}'
                      '${c.quantity > 1 ? ' ×${c.quantity}' : ''}',
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        fontSize: 11,
                        color: palette.inkSoft,
                      ),
                    ),
                  ],
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// 错误提示卡片（重试按钮 + 可选「前往 AI 设置」引导）。
class _ErrorCard extends StatelessWidget {
  const _ErrorCard({required this.error, required this.onRetry, this.onSettings});

  final Object error;
  final VoidCallback onRetry;

  /// AI 相关错误时的设置引导（未配置/不可达/模型输出异常）。
  final VoidCallback? onSettings;

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
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
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
          if (onSettings != null)
            Align(
              alignment: Alignment.centerRight,
              child: TextButton.icon(
                onPressed: onSettings,
                icon: const Icon(Icons.settings_outlined, size: 16),
                label: const Text('前往 AI 设置'),
              ),
            ),
        ],
      ),
    );
  }
}
