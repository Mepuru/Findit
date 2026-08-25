import 'package:flutter/material.dart';
import 'package:findit/src/rust/api/model.dart';
import 'package:findit/src/rust/api/units.dart' as api;

import '../errors.dart';
import '../theme.dart';
import '../widgets/common.dart';
import 'boxes_page.dart';
import 'scan_page.dart';

/// 第一级：存储单元列表。
class UnitsPage extends StatefulWidget {
  const UnitsPage({super.key});

  @override
  State<UnitsPage> createState() => _UnitsPageState();
}

class _UnitsPageState extends State<UnitsPage> {
  List<Unit>? _units;
  Object? _loadError;

  @override
  void initState() {
    super.initState();
    _reload();
  }

  Future<void> _reload() async {
    try {
      final units = await api.listUnits();
      if (!mounted) return;
      setState(() {
        _units = units;
        _loadError = null;
      });
    } catch (e) {
      if (!mounted) return;
      setState(() => _loadError = e);
    }
  }

  Future<void> _openCreateSheet() {
    return showNameSheet(
      context: context,
      title: '新建存储单元',
      nameLabel: '名称',
      nameHint: '如：客厅柜子、储藏室货架',
      onSubmit: (name, description) async {
        await api.createUnit(name: name, description: description);
        await _reload();
      },
    );
  }

  Future<void> _openEditSheet(Unit unit) {
    return showNameSheet(
      context: context,
      title: '编辑存储单元',
      nameLabel: '名称',
      initialName: unit.name,
      initialDescription: unit.description,
      onSubmit: (name, description) async {
        await api.updateUnit(
          id: unit.id,
          name: name,
          description: description,
        );
        await _reload();
      },
    );
  }

  Future<void> _delete(Unit unit) async {
    final confirmed = await confirmDelete(
      context,
      title: '删除「${unit.name}」？',
      message: '该存储单元下的所有收纳箱和物品将一并删除，且无法恢复。',
    );
    if (!confirmed) return;
    try {
      await api.deleteUnit(id: unit.id);
      await _reload();
    } catch (e) {
      if (mounted) showErrorSnack(context, e);
    }
  }

  void _openBoxes(Unit unit) {
    Navigator.of(context)
        .push(MaterialPageRoute(builder: (_) => BoxesPage(unitId: unit.id)))
        .then((_) => _reload());
  }

  void _showPlaceholder(String feature) {
    showErrorSnack(context, '$feature功能将在后续版本上线，敬请期待。');
  }

  Future<void> _openScan() async {
    await Navigator.of(context)
        .push(MaterialPageRoute(builder: (_) => const ScanPage()));
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        title: const Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('Findit'),
            Text(
              '家庭收纳档案',
              style: TextStyle(
                fontSize: 11,
                fontWeight: FontWeight.w500,
                letterSpacing: 2,
                color: FinditColors.inkSoft,
              ),
            ),
          ],
        ),
        actions: [
          IconButton(
            tooltip: '搜索',
            icon: const Icon(Icons.search_rounded),
            onPressed: () => _showPlaceholder('搜索'),
          ),
          IconButton(
            tooltip: '扫码',
            icon: const Icon(Icons.qr_code_scanner_rounded),
            onPressed: _openScan,
          ),
          const SizedBox(width: 4),
        ],
      ),
      floatingActionButton: FloatingActionButton.extended(
        onPressed: _openCreateSheet,
        icon: const Icon(Icons.add),
        label: const Text('新建单元'),
      ),
      body: _buildBody(),
    );
  }

  Widget _buildBody() {
    if (_loadError != null) {
      return EmptyState(
        emoji: '⚠️',
        title: '加载失败',
        subtitle: friendlyErrorMessage(_loadError!),
        actionLabel: '重试',
        onAction: _reload,
      );
    }
    final units = _units;
    if (units == null) {
      return const Center(child: CircularProgressIndicator());
    }
    if (units.isEmpty) {
      return EmptyState(
        emoji: '🗄️',
        title: '还没有存储单元',
        subtitle: '存储单元是收纳的起点，比如「客厅柜子」「卧室床底」。先建一个吧。',
        actionLabel: '新建存储单元',
        onAction: _openCreateSheet,
      );
    }
    return ListView.separated(
      padding: const EdgeInsets.fromLTRB(16, 8, 16, 96),
      itemCount: units.length,
      separatorBuilder: (context, index) => const SizedBox(height: 10),
      itemBuilder: (context, index) => _UnitCard(
        unit: units[index],
        onTap: () => _openBoxes(units[index]),
        onEdit: () => _openEditSheet(units[index]),
        onDelete: () => _delete(units[index]),
      ),
    );
  }
}

class _UnitCard extends StatelessWidget {
  const _UnitCard({
    required this.unit,
    required this.onTap,
    required this.onEdit,
    required this.onDelete,
  });

  final Unit unit;
  final VoidCallback onTap;
  final VoidCallback onEdit;
  final VoidCallback onDelete;

  @override
  Widget build(BuildContext context) {
    return Card(
      child: InkWell(
        borderRadius: BorderRadius.circular(14),
        onTap: onTap,
        child: Padding(
          padding: const EdgeInsets.fromLTRB(16, 14, 8, 14),
          child: Row(
            children: [
              Container(
                width: 42,
                height: 42,
                alignment: Alignment.center,
                decoration: BoxDecoration(
                  color: FinditColors.chipFill,
                  borderRadius: BorderRadius.circular(12),
                ),
                child: const Text('🗄️', style: TextStyle(fontSize: 22)),
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      unit.name,
                      style: Theme.of(context).textTheme.titleMedium,
                    ),
                    if (unit.description.isNotEmpty) ...[
                      const SizedBox(height: 2),
                      Text(
                        unit.description,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: Theme.of(context).textTheme.bodySmall!.copyWith(
                              color: FinditColors.inkSoft,
                            ),
                      ),
                    ],
                  ],
                ),
              ),
              CountBadge(count: unit.boxCount.toInt(), unit: '箱'),
              PopupMenuButton<String>(
                tooltip: '更多操作',
                onSelected: (value) {
                  if (value == 'edit') onEdit();
                  if (value == 'delete') onDelete();
                },
                itemBuilder: (context) => const [
                  PopupMenuItem(value: 'edit', child: Text('编辑')),
                  PopupMenuItem(
                    value: 'delete',
                    child: Text(
                      '删除',
                      style: TextStyle(color: FinditColors.danger),
                    ),
                  ),
                ],
              ),
            ],
          ),
        ),
      ),
    );
  }
}
