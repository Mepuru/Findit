import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart'
    show Int64List;
import 'package:findit/src/rust/api/boxes.dart' as boxes_api;
import 'package:findit/src/rust/api/categories.dart' as categories_api;
import 'package:findit/src/rust/api/items.dart' as api;
import 'package:findit/src/rust/api/model.dart';

import '../errors.dart';
import '../theme.dart';
import '../widgets/common.dart';

/// 第三级：某个收纳箱内的物品列表。
class ItemsPage extends StatefulWidget {
  const ItemsPage({super.key, required this.boxId});

  final int boxId;

  @override
  State<ItemsPage> createState() => _ItemsPageState();
}

class _ItemsPageState extends State<ItemsPage> {
  StorageBox? _box;
  List<Item>? _items;
  Object? _loadError;

  @override
  void initState() {
    super.initState();
    _reload();
  }

  Future<void> _reload() async {
    try {
      final results = await Future.wait([
        boxes_api.getBox(id: widget.boxId),
        api.listItems(boxId: widget.boxId),
      ]);
      if (!mounted) return;
      setState(() {
        _box = results[0] as StorageBox;
        _items = results[1] as List<Item>;
        _loadError = null;
      });
    } catch (e) {
      if (!mounted) return;
      setState(() => _loadError = e);
    }
  }

  Future<void> _openCreateSheet() {
    return _showItemSheet(item: null);
  }

  Future<void> _openEditSheet(Item item) {
    return _showItemSheet(item: item);
  }

  Future<void> _showItemSheet({required Item? item}) {
    return showModalBottomSheet<void>(
      context: context,
      isScrollControlled: true,
      backgroundColor: FinditColors.cardFace,
      shape: const RoundedRectangleBorder(
        borderRadius: BorderRadius.vertical(top: Radius.circular(22)),
        side: BorderSide(color: FinditColors.cardLine),
      ),
      builder: (sheetContext) => _ItemFormSheet(
        item: item,
        onSubmit: (name, description, quantity, categoryIds) async {
          if (item == null) {
            await api.createItem(
              boxId: widget.boxId,
              name: name,
              description: description,
              quantity: quantity,
              categoryIds: categoryIds,
            );
          } else {
            await api.updateItem(
              id: item.id,
              name: name,
              description: description,
              quantity: quantity,
              categoryIds: categoryIds,
            );
          }
          await _reload();
        },
      ),
    );
  }

  Future<void> _delete(Item item) async {
    final confirmed = await confirmDelete(
      context,
      title: '删除「${item.name}」？',
      message: '该物品将从档案中移除，且无法恢复。',
    );
    if (!confirmed) return;
    try {
      await api.deleteItem(id: item.id);
      await _reload();
    } catch (e) {
      if (mounted) showErrorSnack(context, e);
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: Text(_box?.name ?? '物品')),
      floatingActionButton: FloatingActionButton.extended(
        onPressed: _openCreateSheet,
        icon: const Icon(Icons.add),
        label: const Text('登记物品'),
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
    final items = _items;
    final box = _box;
    if (items == null || box == null) {
      return const Center(child: CircularProgressIndicator());
    }
    if (items.isEmpty) {
      return EmptyState(
        emoji: '🏷️',
        title: '「${box.name}」里还没有物品',
        subtitle: '登记第一件物品，给它起个名字并选几个分类。',
        actionLabel: '登记物品',
        onAction: _openCreateSheet,
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(18, 12, 18, 6),
          child: SectionHeader(text: '物品 · ${items.length}'),
        ),
        Expanded(
          child: ListView.separated(
            padding: const EdgeInsets.fromLTRB(16, 4, 16, 96),
            itemCount: items.length,
            separatorBuilder: (context, index) => const SizedBox(height: 10),
            itemBuilder: (context, index) => _ItemCard(
              item: items[index],
              onEdit: () => _openEditSheet(items[index]),
              onDelete: () => _delete(items[index]),
            ),
          ),
        ),
      ],
    );
  }
}

class _ItemCard extends StatelessWidget {
  const _ItemCard({
    required this.item,
    required this.onEdit,
    required this.onDelete,
  });

  final Item item;
  final VoidCallback onEdit;
  final VoidCallback onDelete;

  @override
  Widget build(BuildContext context) {
    return Card(
      child: InkWell(
        borderRadius: BorderRadius.circular(14),
        onTap: onEdit,
        child: Padding(
          padding: const EdgeInsets.fromLTRB(16, 14, 8, 14),
          child: Row(
            children: [
              Container(
                width: 42,
                height: 42,
                alignment: Alignment.center,
                decoration: BoxDecoration(
                  color: FinditColors.persimmon.withValues(alpha: 0.14),
                  borderRadius: BorderRadius.circular(12),
                ),
                child: const Text('🏷️', style: TextStyle(fontSize: 22)),
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Row(
                      children: [
                        Flexible(
                          child: Text(
                            item.name,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: Theme.of(context).textTheme.titleMedium,
                          ),
                        ),
                        const SizedBox(width: 8),
                        Text(
                          '× ${item.quantity}',
                          style: Theme.of(context)
                              .textTheme
                              .labelSmall!
                              .copyWith(color: FinditColors.persimmon),
                        ),
                      ],
                    ),
                    const SizedBox(height: 4),
                    if (item.categories.isEmpty)
                      Text(
                        item.description.isNotEmpty
                            ? item.description
                            : '未设置分类',
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: Theme.of(context)
                            .textTheme
                            .bodySmall!
                            .copyWith(color: FinditColors.inkSoft),
                      )
                    else
                      Wrap(
                        spacing: 6,
                        runSpacing: 4,
                        children: [
                          for (final c in item.categories)
                            Container(
                              padding: const EdgeInsets.symmetric(
                                  horizontal: 8, vertical: 2),
                              decoration: BoxDecoration(
                                color: FinditColors.chipFill,
                                borderRadius: BorderRadius.circular(999),
                              ),
                              child: Text(
                                c,
                                style: Theme.of(context)
                                    .textTheme
                                    .labelSmall!
                                    .copyWith(
                                      color: FinditColors.pineDeep,
                                      letterSpacing: 0.2,
                                    ),
                              ),
                            ),
                        ],
                      ),
                  ],
                ),
              ),
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

/// 物品表单：名称、备注、数量、分类多选（支持行内新建分类）。
class _ItemFormSheet extends StatefulWidget {
  const _ItemFormSheet({required this.item, required this.onSubmit});

  final Item? item;
  final Future<void> Function(
    String name,
    String description,
    int quantity,
    Int64List categoryIds,
  ) onSubmit;

  @override
  State<_ItemFormSheet> createState() => _ItemFormSheetState();
}

class _ItemFormSheetState extends State<_ItemFormSheet> {
  late final TextEditingController _nameController;
  late final TextEditingController _descController;
  late final TextEditingController _quantityController;
  final TextEditingController _newCategoryController = TextEditingController();

  List<Category> _categories = [];
  Set<int> _selected = {};
  bool _busy = false;
  Object? _categoriesError;

  @override
  void initState() {
    super.initState();
    final item = widget.item;
    _nameController = TextEditingController(text: item?.name ?? '');
    _descController = TextEditingController(text: item?.description ?? '');
    _quantityController =
        TextEditingController(text: (item?.quantity ?? 1).toString());
    _loadCategories();
  }

  Future<void> _loadCategories() async {
    try {
      final categories = await categories_api.listCategories();
      if (!mounted) return;
      // 编辑态：按分类名回填选中项。
      final names = widget.item?.categories.toSet() ?? const <String>{};
      setState(() {
        _categories = categories;
        _selected = {
          for (final c in categories)
            if (names.contains(c.name)) c.id.toInt(),
        };
        _categoriesError = null;
      });
    } catch (e) {
      if (!mounted) return;
      setState(() => _categoriesError = e);
    }
  }

  Future<void> _createCategoryInline() async {
    final name = _newCategoryController.text.trim();
    if (name.isEmpty) return;
    try {
      final created = await categories_api.createCategory(name: name);
      _newCategoryController.clear();
      if (!mounted) return;
      setState(() {
        _categories = [..._categories, created];
        _selected.add(created.id.toInt());
      });
    } catch (e) {
      if (mounted) showErrorSnack(context, e);
    }
  }

  Future<void> _submit() async {
    final name = _nameController.text.trim();
    if (name.isEmpty) {
      _snack('物品名称不能为空');
      return;
    }
    final quantity = int.tryParse(_quantityController.text.trim()) ?? 0;
    if (quantity < 1) {
      _snack('数量必须是不少于 1 的整数');
      return;
    }
    setState(() => _busy = true);
    try {
      await widget.onSubmit(
        name,
        _descController.text.trim(),
        quantity,
        Int64List.fromList(_selected.toList()..sort()),
      );
      if (mounted) Navigator.of(context).pop();
    } catch (e) {
      if (mounted) showErrorSnack(context, e);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  void _snack(String message) {
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(content: Text(message), behavior: SnackBarBehavior.floating),
    );
  }

  @override
  void dispose() {
    _nameController.dispose();
    _descController.dispose();
    _quantityController.dispose();
    _newCategoryController.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final isEdit = widget.item != null;
    return Padding(
      padding: EdgeInsets.only(
        left: 20,
        right: 20,
        top: 16,
        bottom: MediaQuery.of(context).viewInsets.bottom + 24,
      ),
      child: SingleChildScrollView(
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
            Text(
              isEdit ? '编辑物品' : '登记物品',
              style: Theme.of(context).textTheme.titleMedium,
            ),
            const SizedBox(height: 16),
            TextField(
              controller: _nameController,
              autofocus: !isEdit,
              decoration: const InputDecoration(
                labelText: '物品名称',
                hintText: '如：羽绒服、充电宝',
              ),
            ),
            const SizedBox(height: 12),
            TextField(
              controller: _descController,
              maxLines: 2,
              decoration: const InputDecoration(labelText: '备注（可选）'),
            ),
            const SizedBox(height: 12),
            TextField(
              controller: _quantityController,
              keyboardType: TextInputType.number,
              inputFormatters: [FilteringTextInputFormatter.digitsOnly],
              decoration: const InputDecoration(labelText: '数量'),
            ),
            const SizedBox(height: 18),
            Text(
              '分类（可多选）',
              style: Theme.of(context).textTheme.titleSmall,
            ),
            const SizedBox(height: 8),
            if (_categoriesError != null)
              Text(
                '分类加载失败：${friendlyErrorMessage(_categoriesError!)}',
                style: Theme.of(context).textTheme.bodySmall!.copyWith(
                      color: FinditColors.danger,
                    ),
              )
            else if (_categories.isEmpty)
              Text(
                '还没有分类，在下方输入框新建一个吧。',
                style: Theme.of(context).textTheme.bodySmall!.copyWith(
                      color: FinditColors.inkSoft,
                    ),
              )
            else
              Wrap(
                spacing: 8,
                runSpacing: 8,
                children: [
                  for (final c in _categories)
                    FilterChip(
                      label: Text(c.name),
                      selected: _selected.contains(c.id.toInt()),
                      onSelected: (checked) {
                        setState(() {
                          if (checked) {
                            _selected.add(c.id.toInt());
                          } else {
                            _selected.remove(c.id.toInt());
                          }
                        });
                      },
                      selectedColor: FinditColors.pine.withValues(alpha: 0.16),
                      checkmarkColor: FinditColors.pineDeep,
                    ),
                ],
              ),
            const SizedBox(height: 12),
            Row(
              children: [
                Expanded(
                  child: TextField(
                    controller: _newCategoryController,
                    decoration: const InputDecoration(
                      labelText: '新分类',
                      hintText: '如：电子、衣物、证件',
                    ),
                  ),
                ),
                const SizedBox(width: 8),
                IconButton.filled(
                  tooltip: '新建分类',
                  onPressed: _createCategoryInline,
                  icon: const Icon(Icons.add),
                ),
              ],
            ),
            const SizedBox(height: 20),
            FilledButton(
              onPressed: _busy ? null : _submit,
              child: _busy
                  ? const SizedBox(
                      width: 18,
                      height: 18,
                      child: CircularProgressIndicator(strokeWidth: 2),
                    )
                  : const Text('保存'),
            ),
          ],
        ),
      ),
    );
  }
}
