import 'dart:async';

import 'package:flutter/material.dart';
import 'package:findit/src/rust/api/ai.dart' as ai;
import 'package:findit/src/rust/api/model.dart';
import 'package:findit/src/rust/core/ai/config.dart';

import '../errors.dart';
import '../theme.dart';

/// AI 设置：对话模型、向量模型（支持独立服务配置）、语义向量的重建与补齐。
class AiSettingsPage extends StatefulWidget {
  const AiSettingsPage({super.key});

  @override
  State<AiSettingsPage> createState() => _AiSettingsPageState();
}

class _AiSettingsPageState extends State<AiSettingsPage> {
  // ── 对话模型 ──
  final _baseUrl = TextEditingController();
  final _apiKey = TextEditingController();
  final _chatModel = TextEditingController();
  AiProvider _provider = AiProvider.ollama;

  // ── 向量模型 ──
  final _embedModel = TextEditingController();
  final _embedBaseUrl = TextEditingController();
  final _embedApiKey = TextEditingController();
  AiProvider _embedProvider = AiProvider.ollama;
  bool _useSameService = true;

  AiStatus? _status;
  AiTestResult? _testResult;
  bool _loading = true;
  bool _testing = false;
  bool _saving = false;
  bool _backfilling = false;

  /// 重建向量进度；非空表示正在重建。
  EmbedProgress? _rebuildProgress;
  StreamSubscription<EmbedProgress>? _rebuildSub;

  @override
  void initState() {
    super.initState();
    _load();
  }

  @override
  void dispose() {
    _rebuildSub?.cancel();
    _baseUrl.dispose();
    _apiKey.dispose();
    _chatModel.dispose();
    _embedModel.dispose();
    _embedBaseUrl.dispose();
    _embedApiKey.dispose();
    super.dispose();
  }

  Future<void> _load() async {
    try {
      final results = await Future.wait([
        ai.getAiConfig(),
        ai.getAiStatus(),
      ]);
      final config = results[0] as AiConfig;
      final status = results[1] as AiStatus;
      if (!mounted) return;
      setState(() {
        _provider = config.provider;
        _baseUrl.text = config.baseUrl;
        _apiKey.text = config.apiKey;
        _chatModel.text = config.chatModel;
        _embedModel.text = config.embedModel;
        _useSameService = !config.hasSeparateEmbedService;
        _embedProvider = config.embedProvider;
        _embedBaseUrl.text = config.embedBaseUrl;
        _embedApiKey.text = config.embedApiKey;
        _status = status;
        _loading = false;
      });
    } catch (e) {
      if (!mounted) return;
      setState(() => _loading = false);
      showErrorSnack(context, e);
    }
  }

  AiConfig _buildConfig() => AiConfig(
        provider: _provider,
        baseUrl: _baseUrl.text.trim(),
        apiKey: _provider == AiProvider.openAi ? _apiKey.text.trim() : '',
        chatModel: _chatModel.text.trim(),
        embedModel: _embedModel.text.trim(),
        embedProvider: _useSameService ? _provider : _embedProvider,
        embedBaseUrl: _useSameService ? '' : _embedBaseUrl.text.trim(),
        embedApiKey: _useSameService
            ? ''
            : (_embedProvider == AiProvider.openAi
                ? _embedApiKey.text.trim()
                : ''),
      );

  Future<void> _save() async {
    setState(() => _saving = true);
    try {
      await ai.saveAiConfig(config: _buildConfig());
      if (!mounted) return;
      await _load();
      if (!mounted) return;
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(
          content: Text('AI 配置已保存'),
          behavior: SnackBarBehavior.floating,
        ),
      );
    } catch (e) {
      if (mounted) showErrorSnack(context, e);
    } finally {
      if (mounted) setState(() => _saving = false);
    }
  }

  Future<void> _test() async {
    setState(() {
      _testing = true;
      _testResult = null;
    });
    try {
      final result = await ai.testAiConnection();
      if (!mounted) return;
      setState(() => _testResult = result);
      await _load();
    } catch (e) {
      if (mounted) showErrorSnack(context, e);
    } finally {
      if (mounted) setState(() => _testing = false);
    }
  }

  Future<void> _backfill() async {
    setState(() => _backfilling = true);
    try {
      final n = await ai.backfillPendingEmbeddings();
      if (!mounted) return;
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(
          content: Text(n > 0 ? '已补齐 $n 条语义向量' : '没有待补齐的向量'),
          behavior: SnackBarBehavior.floating,
        ),
      );
      await _load();
    } catch (e) {
      if (mounted) showErrorSnack(context, e);
    } finally {
      if (mounted) setState(() => _backfilling = false);
    }
  }

  void _rebuild() {
    final stream = ai.rebuildEmbeddings();
    setState(() => _rebuildProgress = const EmbedProgress(done: 0, total: 0));
    _rebuildSub?.cancel();
    _rebuildSub = stream.listen(
      (p) {
        if (!mounted) return;
        setState(() => _rebuildProgress = p);
      },
      onError: (Object e) {
        if (!mounted) return;
        setState(() => _rebuildProgress = null);
        showErrorSnack(context, e);
      },
      onDone: () {
        if (!mounted) return;
        final p = _rebuildProgress;
        setState(() => _rebuildProgress = null);
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(
            content: Text('向量重建完成：${p?.done ?? 0}/${p?.total ?? 0}'),
            behavior: SnackBarBehavior.floating,
          ),
        );
        _load();
      },
    );
  }

  // ---------------------------------------------------------------------------

  @override
  Widget build(BuildContext context) {
    final palette = context.palette;
    return Scaffold(
      appBar: AppBar(title: const Text('AI 设置')),
      body: _loading
          ? const Center(child: CircularProgressIndicator())
          : ListView(
              padding: const EdgeInsets.fromLTRB(16, 12, 16, 48),
              children: [
                _sectionTitle('对话模型'),
                Card(
                  child: Padding(
                    padding: const EdgeInsets.all(16),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        SegmentedButton<AiProvider>(
                          segments: const [
                            ButtonSegment(
                              value: AiProvider.ollama,
                              label: Text('Ollama'),
                            ),
                            ButtonSegment(
                              value: AiProvider.openAi,
                              label: Text('OpenAI 兼容'),
                            ),
                          ],
                          selected: {_provider},
                          onSelectionChanged: (s) =>
                              setState(() => _provider = s.first),
                        ),
                        const SizedBox(height: 14),
                        TextField(
                          controller: _baseUrl,
                          decoration: InputDecoration(
                            labelText: '服务地址',
                            hintText:
                                'http://10.0.2.2:11434（模拟器）或局域网电脑地址',
                            isDense: true,
                            filled: true,
                            fillColor: palette.cardFace,
                          ),
                          keyboardType: TextInputType.url,
                        ),
                        if (_provider == AiProvider.openAi) ...[
                          const SizedBox(height: 10),
                          TextField(
                            controller: _apiKey,
                            obscureText: true,
                            decoration: InputDecoration(
                              labelText: 'API Key',
                              isDense: true,
                              filled: true,
                              fillColor: palette.cardFace,
                            ),
                          ),
                        ],
                        const SizedBox(height: 10),
                        TextField(
                          controller: _chatModel,
                          decoration: InputDecoration(
                            labelText: '模型名称',
                            hintText: '如：qwen3:4b',
                            isDense: true,
                            filled: true,
                            fillColor: palette.cardFace,
                          ),
                        ),
                        const SizedBox(height: 14),
                        Row(
                          children: [
                            FilledButton.icon(
                              onPressed: _saving ? null : _save,
                              icon: _saving
                                  ? const SizedBox(
                                      width: 14,
                                      height: 14,
                                      child: CircularProgressIndicator(
                                          strokeWidth: 2),
                                    )
                                  : const Icon(Icons.save_outlined),
                              label: Text(_saving ? '保存中…' : '保存配置'),
                            ),
                            const SizedBox(width: 10),
                            OutlinedButton.icon(
                              onPressed: _testing ? null : _test,
                              icon: _testing
                                  ? const SizedBox(
                                      width: 14,
                                      height: 14,
                                      child: CircularProgressIndicator(
                                          strokeWidth: 2),
                                    )
                                  : const Icon(Icons.wifi_tethering),
                              label: Text(_testing ? '测试中…' : '测试连接'),
                            ),
                          ],
                        ),
                        if (_testResult != null) ...[
                          const SizedBox(height: 12),
                          _testRow('对话', _testResult!.chatOk,
                              _testResult!.chatMessage),
                          _testRow('向量', _testResult!.embedOk,
                              _testResult!.embedMessage),
                        ],
                      ],
                    ),
                  ),
                ),
                const SizedBox(height: 20),
                _sectionTitle('向量模型'),
                Card(
                  child: Padding(
                    padding: const EdgeInsets.all(16),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        SwitchListTile(
                          title: const Text('使用与对话相同的服务'),
                          subtitle: Text(
                            _useSameService
                                ? '向量请求将发送到对话模型的服务地址'
                                : '可独立配置向量服务的地址和认证',
                            style: TextStyle(
                                fontSize: 12, color: palette.inkSoft),
                          ),
                          value: _useSameService,
                          contentPadding: EdgeInsets.zero,
                          onChanged: (v) =>
                              setState(() => _useSameService = v),
                        ),
                        if (!_useSameService) ...[
                          const SizedBox(height: 10),
                          SegmentedButton<AiProvider>(
                            segments: const [
                              ButtonSegment(
                                value: AiProvider.ollama,
                                label: Text('Ollama'),
                              ),
                              ButtonSegment(
                                value: AiProvider.openAi,
                                label: Text('OpenAI 兼容'),
                              ),
                            ],
                            selected: {_embedProvider},
                            onSelectionChanged: (s) =>
                                setState(() => _embedProvider = s.first),
                          ),
                          const SizedBox(height: 14),
                          TextField(
                            controller: _embedBaseUrl,
                            decoration: InputDecoration(
                              labelText: '向量服务地址',
                              hintText:
                                  'http://localhost:11434（本地 Ollama）',
                              isDense: true,
                              filled: true,
                              fillColor: palette.cardFace,
                            ),
                            keyboardType: TextInputType.url,
                          ),
                          if (_embedProvider == AiProvider.openAi) ...[
                            const SizedBox(height: 10),
                            TextField(
                              controller: _embedApiKey,
                              obscureText: true,
                              decoration: InputDecoration(
                                labelText: 'API Key',
                                isDense: true,
                                filled: true,
                                fillColor: palette.cardFace,
                              ),
                            ),
                          ],
                        ],
                        const SizedBox(height: 10),
                        TextField(
                          controller: _embedModel,
                          decoration: InputDecoration(
                            labelText: '向量模型',
                            hintText: '如：nomic-embed-text',
                            isDense: true,
                            filled: true,
                            fillColor: palette.cardFace,
                          ),
                        ),
                      ],
                    ),
                  ),
                ),
                const SizedBox(height: 20),
                _sectionTitle('语义向量'),
                Card(
                  child: Padding(
                    padding: const EdgeInsets.all(16),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        _statusLine(),
                        const SizedBox(height: 12),
                        if (_rebuildProgress != null) ...[
                          LinearProgressIndicator(
                            value: _rebuildProgress!.total <= 0
                                ? null
                                : _rebuildProgress!.done /
                                    _rebuildProgress!.total,
                          ),
                          const SizedBox(height: 6),
                          Text(
                            '正在重建：${_rebuildProgress!.done}/${_rebuildProgress!.total}',
                            style: TextStyle(
                              fontSize: 12,
                              color: palette.inkSoft,
                            ),
                          ),
                          const SizedBox(height: 12),
                        ],
                        Wrap(
                          spacing: 10,
                          runSpacing: 8,
                          children: [
                            OutlinedButton.icon(
                              onPressed: _backfilling || _rebuildProgress != null
                                  ? null
                                  : _backfill,
                              icon: _backfilling
                                  ? const SizedBox(
                                      width: 14,
                                      height: 14,
                                      child: CircularProgressIndicator(
                                          strokeWidth: 2),
                                    )
                                  : const Icon(Icons.playlist_add),
                              label: Text(_backfilling ? '补齐中…' : '补齐待处理向量'),
                            ),
                            OutlinedButton.icon(
                              onPressed:
                                  _rebuildProgress != null ? null : _rebuild,
                              icon: const Icon(Icons.refresh),
                              label: const Text('重建全部向量'),
                            ),
                          ],
                        ),
                        const SizedBox(height: 8),
                        Text(
                          '更换向量模型或维度变化时请先重建；补齐只处理尚未生成向量的物品。',
                          style: TextStyle(
                            fontSize: 12,
                            color: palette.inkSoft,
                          ),
                        ),
                      ],
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

  Widget _statusLine() {
    final status = _status;
    if (status == null) {
      return const Text('状态未知', style: TextStyle(fontSize: 13));
    }
    final configured = status.configured ? '已配置' : '未配置服务地址';
    final pending = status.pendingEmbeddings;
    final model = status.embeddedModel ?? '未记录';
    final dim = status.embeddedDim == null ? '—' : '${status.embeddedDim}';
    return Text(
      '状态：$configured · 待补齐向量 $pending 条\n'
      '已用模型：$model（$dim 维）',
      style: const TextStyle(fontSize: 13, height: 1.6),
    );
  }

  Widget _testRow(String label, bool ok, String message) {
    final palette = context.palette;
    final color = ok ? palette.pineDeep : palette.danger;
    return Padding(
      padding: const EdgeInsets.only(top: 4),
      child: Row(
        children: [
          Icon(
            ok ? Icons.check_circle : Icons.cancel,
            size: 16,
            color: color,
          ),
          const SizedBox(width: 6),
          Expanded(
            child: Text(
              '$label：$message',
              style: TextStyle(fontSize: 13, color: color),
            ),
          ),
        ],
      ),
    );
  }
}
