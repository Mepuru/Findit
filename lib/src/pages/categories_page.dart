import 'package:flutter/material.dart';
import 'package:findit/src/rust/api/categories.dart' as api;
import 'package:findit/src/rust/api/model.dart';

import '../errors.dart';
import '../theme.dart';
import '../widgets/common.dart';

/// 分类管理页：重命名 / 删除分类。
///
/// 删除分类只会解除该分类与物品的关联（物品本身保留）；
/// 分类的创建仍可在登记/编辑物品时行内完成。
class CategoriesPage extends StatefulWidget {
  const CategoriesPage({super.key});

  @override
  State<CategoriesPage> createState() => _CategoriesPageState();
}

class _CategoriesPageState extends State<CategoriesPage> {
  List<Category>? _categories;
  Object? _loadError;

  @override
  void initState() {
    super.initState();
    _reload();
  }

  Future<void> _reload() async {
    try {
      final categories = await api.listCategories();
      if (!mounted) return;
      setState(() {
        _categories = categories;
        _loadError = null;
      });
    } catch (e) {
      if (!mounted) return;
      setState(() => _loadError = e);
    }
  }

  Future<void> _rename(Category category) async {
    final controller = TextEditingController(text: category.name);
    final input = await showDialog<String>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('重命名分类'),
        content: TextField(
          controller: controller,
          autofocus: true,
          decoration: const InputDecoration(
            labelText: '分类名称',
            hintText: '如：电子、衣物、证件',
          ),
          onSubmitted: (v) => Navigator.of(context).pop(v),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(),
            child: const Text('取消'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(context).pop(controller.text),
            child: const Text('保存'),
          ),
        ],
      ),
    );
    controller.dispose();
    final name = input?.trim() ?? '';
    if (name.isEmpty || name == category.name) return;
    try {
      await api.renameCategory(id: category.id, newName: name);
      await _reload();
    } catch (e) {
      if (mounted) showErrorSnack(context, e);
    }
  }

  Future<void> _delete(Category category) async {
    final confirmed = await confirmDelete(
      context,
      title: '删除分类「${category.name}」？',
      message: '该分类会从所有物品上移除，物品本身不会被删除。',
    );
    if (!confirmed) return;
    try {
      await api.deleteCategory(id: category.id);
      await _reload();
    } catch (e) {
      if (mounted) showErrorSnack(context, e);
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('分类管理')),
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
    final categories = _categories;
    if (categories == null) {
      return const Center(child: CircularProgressIndicator());
    }
    if (categories.isEmpty) {
      return const EmptyState(
        emoji: '🏷️',
        title: '还没有分类',
        subtitle: '在登记或编辑物品时可以直接新建分类。',
      );
    }
    return ListView.separated(
      padding: const EdgeInsets.fromLTRB(16, 12, 16, 48),
      itemCount: categories.length,
      separatorBuilder: (context, index) => const SizedBox(height: 10),
      itemBuilder: (context, index) {
        final category = categories[index];
        return Card(
          child: ListTile(
            leading: Container(
              width: 40,
              height: 40,
              alignment: Alignment.center,
              decoration: BoxDecoration(
                color: context.palette.chipFill,
                borderRadius: BorderRadius.circular(12),
              ),
              child: const Text('🏷️', style: TextStyle(fontSize: 20)),
            ),
            title: Text(category.name),
            trailing: PopupMenuButton<String>(
              tooltip: '更多操作',
              onSelected: (value) {
                if (value == 'rename') _rename(category);
                if (value == 'delete') _delete(category);
              },
              itemBuilder: (context) => [
                const PopupMenuItem(value: 'rename', child: Text('重命名')),
                PopupMenuItem(
                  value: 'delete',
                  child: Text(
                    '删除',
                    style: TextStyle(color: context.palette.danger),
                  ),
                ),
              ],
            ),
          ),
        );
      },
    );
  }
}
