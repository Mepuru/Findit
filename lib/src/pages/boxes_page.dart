import 'package:flutter/material.dart';
import 'package:findit/src/rust/api/boxes.dart' as api;
import 'package:findit/src/rust/api/model.dart';
import 'package:findit/src/rust/api/units.dart' as units_api;

import '../errors.dart';
import '../theme.dart';
import '../widgets/common.dart';
import 'items_page.dart';

/// 第二级：某个存储单元下的收纳箱列表。
class BoxesPage extends StatefulWidget {
  const BoxesPage({super.key, required this.unitId});

  final int unitId;

  @override
  State<BoxesPage> createState() => _BoxesPageState();
}

class _BoxesPageState extends State<BoxesPage> {
  Unit? _unit;
  List<StorageBox>? _boxes;
  Object? _loadError;

  @override
  void initState() {
    super.initState();
    _reload();
  }

  Future<void> _reload() async {
    try {
      final results = await Future.wait([
        units_api.getUnit(id: widget.unitId),
        api.listBoxes(unitId: widget.unitId),
      ]);
      if (!mounted) return;
      setState(() {
        _unit = results[0] as Unit;
        _boxes = results[1] as List<StorageBox>;
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
      title: '新建收纳箱',
      nameLabel: '箱名',
      nameHint: '如：冬季衣物箱、药品箱',
      onSubmit: (name, description) async {
        await api.createBox(
          unitId: widget.unitId,
          name: name,
          description: description,
        );
        await _reload();
      },
    );
  }

  Future<void> _openEditSheet(StorageBox box) {
    return showNameSheet(
      context: context,
      title: '编辑收纳箱',
      nameLabel: '箱名',
      initialName: box.name,
      initialDescription: box.description,
      onSubmit: (name, description) async {
        await api.updateBox(
          id: box.id,
          name: name,
          description: description,
        );
        await _reload();
      },
    );
  }

  Future<void> _delete(StorageBox box) async {
    final confirmed = await confirmDelete(
      context,
      title: '删除「${box.name}」？',
      message: '箱内所有物品及分类关联将一并删除，且无法恢复。',
    );
    if (!confirmed) return;
    try {
      await api.deleteBox(id: box.id);
      await _reload();
    } catch (e) {
      if (mounted) showErrorSnack(context, e);
    }
  }

  void _openItems(StorageBox box) {
    Navigator.of(context)
        .push(MaterialPageRoute(builder: (_) => ItemsPage(boxId: box.id)))
        .then((_) => _reload());
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        title: Text(_unit?.name ?? '收纳箱'),
      ),
      floatingActionButton: FloatingActionButton.extended(
        onPressed: _openCreateSheet,
        icon: const Icon(Icons.add),
        label: const Text('新建收纳箱'),
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
    final boxes = _boxes;
    final unit = _unit;
    if (boxes == null || unit == null) {
      return const Center(child: CircularProgressIndicator());
    }
    if (boxes.isEmpty) {
      return EmptyState(
        emoji: '📦',
        title: '「${unit.name}」还是空的',
        subtitle: '把物品分装进收纳箱，每个箱子都有专属二维码标签。',
        actionLabel: '新建收纳箱',
        onAction: _openCreateSheet,
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(18, 12, 18, 6),
          child: SectionHeader(text: '收纳箱 · ${boxes.length}'),
        ),
        Expanded(
          child: ListView.separated(
            padding: const EdgeInsets.fromLTRB(16, 4, 16, 96),
            itemCount: boxes.length,
            separatorBuilder: (context, index) => const SizedBox(height: 10),
            itemBuilder: (context, index) => _BoxCard(
              box: boxes[index],
              onTap: () => _openItems(boxes[index]),
              onEdit: () => _openEditSheet(boxes[index]),
              onDelete: () => _delete(boxes[index]),
            ),
          ),
        ),
      ],
    );
  }
}

class _BoxCard extends StatelessWidget {
  const _BoxCard({
    required this.box,
    required this.onTap,
    required this.onEdit,
    required this.onDelete,
  });

  final StorageBox box;
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
                  color: FinditColors.pine.withValues(alpha: 0.12),
                  borderRadius: BorderRadius.circular(12),
                ),
                child: const Text('📦', style: TextStyle(fontSize: 22)),
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      box.name,
                      style: Theme.of(context).textTheme.titleMedium,
                    ),
                    const SizedBox(height: 2),
                    Text(
                      box.description.isNotEmpty
                          ? box.description
                          : '标签码 ${box.slug.substring(0, 8)}',
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: Theme.of(context).textTheme.bodySmall!.copyWith(
                            color: FinditColors.inkSoft,
                          ),
                    ),
                  ],
                ),
              ),
              CountBadge(count: box.itemCount.toInt(), unit: '件'),
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
