import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart'
    show Int64List;
import 'package:image_picker/image_picker.dart';
import 'package:findit/src/rust/api/ai.dart' as ai_api;
import 'package:findit/src/rust/api/boxes.dart' as boxes_api;
import 'package:findit/src/rust/api/categories.dart' as categories_api;
import 'package:findit/src/rust/api/items.dart' as api;
import 'package:findit/src/rust/api/model.dart';
import 'package:findit/src/rust/api/photos.dart' as photos_api;

import '../errors.dart';
import '../theme.dart';
import '../widgets/box_qr_sheet.dart';
import '../widgets/common.dart';
import 'photo_viewer_page.dart';

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

  /// item_id → 照片本地路径（列表缩略图 + 大图）。
  final Map<int, ({String thumb, String full})> _photoPaths = {};

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
      final box = results[0] as StorageBox;
      final items = results[1] as List<Item>;

      // 解析照片本地路径；文件缺失时仅不展示。
      final photos = <int, ({String thumb, String full})>{};
      for (final item in items) {
        final fileName = item.photoPath;
        if (fileName == null) continue;
        try {
          photos[item.id] = (
            thumb: await photos_api.getThumbFullPath(fileName: fileName),
            full: await photos_api.getPhotoFullPath(fileName: fileName),
          );
        } catch (_) {}
      }

      if (!mounted) return;
      setState(() {
        _box = box;
        _items = items;
        _photoPaths
          ..clear()
          ..addAll(photos);
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
        onPhotoChanged: _reload,
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
          // 物品写入后异步补齐语义向量；失败静默忽略（启动/手动补齐兜底）。
          // ignore: unawaited_futures
          ai_api.backfillPendingEmbeddings().catchError((_) => 0);
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

  /// 打开物品照片大图预览；返回后若照片已删除则刷新。
  Future<void> _openPhotoViewer(Item item) async {
    final paths = _photoPaths[item.id];
    if (paths == null) return;
    final deleted = await Navigator.of(context).push<bool>(
      MaterialPageRoute(
        builder: (_) => PhotoViewerPage(
          title: item.name,
          photoPath: paths.full,
          onDelete: () async {
            await photos_api.deleteItemPhoto(itemId: item.id);
            return true;
          },
        ),
      ),
    );
    if (deleted == true) await _reload();
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        title: Text(_box?.name ?? '物品'),
        actions: [
          if (_box != null)
            IconButton(
              tooltip: '箱标签二维码',
              icon: const Icon(Icons.qr_code_rounded),
              onPressed: () => showBoxQrSheet(context, box: _box!),
            ),
        ],
      ),
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
              photoThumb: _photoPaths[items[index].id]?.thumb,
              onPhotoTap: () => _openPhotoViewer(items[index]),
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
    required this.photoThumb,
    required this.onPhotoTap,
    required this.onEdit,
    required this.onDelete,
  });

  final Item item;

  /// 缩略图本地路径；无照片时为 `null`。
  final String? photoThumb;
  final VoidCallback onPhotoTap;
  final VoidCallback onEdit;
  final VoidCallback onDelete;

  Widget get _tagIcon => Container(
        width: 42,
        height: 42,
        alignment: Alignment.center,
        decoration: BoxDecoration(
          color: FinditColors.persimmon.withValues(alpha: 0.14),
          borderRadius: BorderRadius.circular(12),
        ),
        child: const Text('🏷️', style: TextStyle(fontSize: 22)),
      );

  @override
  Widget build(BuildContext context) {
    final thumb = photoThumb;
    return Card(
      child: InkWell(
        borderRadius: BorderRadius.circular(14),
        onTap: onEdit,
        child: Padding(
          padding: const EdgeInsets.fromLTRB(16, 14, 8, 14),
          child: Row(
            children: [
              if (thumb != null)
                GestureDetector(
                  onTap: onPhotoTap,
                  child: ClipRRect(
                    borderRadius: BorderRadius.circular(12),
                    child: Image.file(
                      File(thumb),
                      width: 42,
                      height: 42,
                      fit: BoxFit.cover,
                      errorBuilder: (context, error, stackTrace) => _tagIcon,
                    ),
                  ),
                )
              else
                _tagIcon,
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

/// 物品表单：名称、备注、数量、分类多选（支持行内新建分类）；
/// 编辑态另含照片区（拍照/选图/替换/删除/大图预览）。
class _ItemFormSheet extends StatefulWidget {
  const _ItemFormSheet({
    required this.item,
    required this.onSubmit,
    this.onPhotoChanged,
  });

  final Item? item;
  final Future<void> Function(
    String name,
    String description,
    int quantity,
    Int64List categoryIds,
  ) onSubmit;

  /// 照片变更（新增/替换/删除）后回调，用于刷新外层列表。
  final VoidCallback? onPhotoChanged;

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

  /// 当前照片文件名（跟随保存/删除操作更新）。
  String? _photoPath;
  bool _photoBusy = false;

  /// 缩略图路径 Future（缓存，避免重复解析）。
  Future<String>? _thumbFuture;

  @override
  void initState() {
    super.initState();
    final item = widget.item;
    _nameController = TextEditingController(text: item?.name ?? '');
    _descController = TextEditingController(text: item?.description ?? '');
    _quantityController =
        TextEditingController(text: (item?.quantity ?? 1).toString());
    _photoPath = item?.photoPath;
    if (_photoPath != null) {
      _thumbFuture = photos_api.getThumbFullPath(fileName: _photoPath!);
    }
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

  // ---------- 照片 ----------

  /// 拍照或从相册选图 → 交给 Rust 压缩落盘。
  Future<void> _pickAndSavePhoto(ImageSource source) async {
    final item = widget.item;
    if (item == null || _photoBusy) return;
    try {
      final picked = await ImagePicker().pickImage(source: source);
      if (picked == null) return;
      final bytes = await picked.readAsBytes();
      setState(() => _photoBusy = true);
      await photos_api.saveItemPhoto(itemId: item.id, bytes: bytes);
      final refreshed = await api.getItem(id: item.id);
      if (!mounted) return;
      setState(() {
        _photoPath = refreshed.photoPath;
        _thumbFuture = _photoPath == null
            ? null
            : photos_api.getThumbFullPath(fileName: _photoPath!);
      });
      widget.onPhotoChanged?.call();
    } catch (e) {
      if (mounted) showErrorSnack(context, e);
    } finally {
      if (mounted) setState(() => _photoBusy = false);
    }
  }

  Future<void> _deletePhoto() async {
    final item = widget.item;
    if (item == null || _photoPath == null || _photoBusy) return;
    final confirmed = await confirmDelete(
      context,
      title: '删除这张照片？',
      message: '照片文件将被移除，且无法恢复。',
    );
    if (!confirmed) return;
    try {
      setState(() => _photoBusy = true);
      await photos_api.deleteItemPhoto(itemId: item.id);
      if (!mounted) return;
      setState(() {
        _photoPath = null;
        _thumbFuture = null;
      });
      widget.onPhotoChanged?.call();
    } catch (e) {
      if (mounted) showErrorSnack(context, e);
    } finally {
      if (mounted) setState(() => _photoBusy = false);
    }
  }

  /// 打开大图预览。
  Future<void> _viewPhoto() async {
    final item = widget.item;
    final fileName = _photoPath;
    if (item == null || fileName == null) return;
    try {
      final full = await photos_api.getPhotoFullPath(fileName: fileName);
      if (!mounted) return;
      await Navigator.of(context).push(
        MaterialPageRoute(
          builder: (_) =>
              PhotoViewerPage(title: item.name, photoPath: full),
        ),
      );
    } catch (e) {
      if (mounted) showErrorSnack(context, e);
    }
  }

  /// 照片区：左侧缩略图（可点大图）+ 右侧操作按钮。
  Widget _buildPhotoArea() {
    final fileName = _photoPath;
    final thumbFuture = _thumbFuture;
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        GestureDetector(
          onTap: fileName != null ? _viewPhoto : null,
          child: Container(
            width: 88,
            height: 88,
            decoration: BoxDecoration(
              color: FinditColors.chipFill,
              borderRadius: BorderRadius.circular(12),
              border: Border.all(color: FinditColors.cardLine),
            ),
            clipBehavior: Clip.antiAlias,
            child: fileName != null && thumbFuture != null
                ? FutureBuilder<String>(
                    future: thumbFuture,
                    builder: (context, snap) {
                      if (!snap.hasData) {
                        return const Center(
                          child: SizedBox(
                            width: 18,
                            height: 18,
                            child: CircularProgressIndicator(strokeWidth: 2),
                          ),
                        );
                      }
                      return Image.file(
                        File(snap.data!),
                        fit: BoxFit.cover,
                        errorBuilder: (context, error, stackTrace) =>
                            const Icon(
                          Icons.broken_image_outlined,
                          color: FinditColors.inkSoft,
                        ),
                      );
                    },
                  )
                : const Icon(
                    Icons.photo_camera_outlined,
                    color: FinditColors.inkSoft,
                  ),
          ),
        ),
        const SizedBox(width: 12),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Wrap(
                spacing: 8,
                runSpacing: 8,
                children: [
                  OutlinedButton.icon(
                    onPressed: _photoBusy
                        ? null
                        : () => _pickAndSavePhoto(ImageSource.camera),
                    icon: const Icon(Icons.photo_camera, size: 16),
                    label: Text(fileName != null ? '重新拍照' : '拍照'),
                  ),
                  OutlinedButton.icon(
                    onPressed: _photoBusy
                        ? null
                        : () => _pickAndSavePhoto(ImageSource.gallery),
                    icon: const Icon(Icons.photo_library_outlined, size: 16),
                    label: const Text('相册'),
                  ),
                  if (fileName != null)
                    TextButton.icon(
                      onPressed: _photoBusy ? null : _deletePhoto,
                      icon: const Icon(
                        Icons.delete_outline_rounded,
                        size: 16,
                        color: FinditColors.danger,
                      ),
                      label: const Text(
                        '删除',
                        style: TextStyle(color: FinditColors.danger),
                      ),
                    ),
                ],
              ),
              if (_photoBusy) ...[
                const SizedBox(height: 8),
                Text(
                  '正在压缩保存…',
                  style: Theme.of(context).textTheme.labelSmall!.copyWith(
                        color: FinditColors.inkSoft,
                      ),
                ),
              ],
            ],
          ),
        ),
      ],
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
            if (isEdit) ...[
              Text('照片留档', style: Theme.of(context).textTheme.titleSmall),
              const SizedBox(height: 8),
              _buildPhotoArea(),
              const SizedBox(height: 16),
            ] else ...[
              Text(
                '保存物品后可在编辑页拍照留档。',
                style: Theme.of(context).textTheme.bodySmall!.copyWith(
                      color: FinditColors.inkSoft,
                    ),
              ),
              const SizedBox(height: 16),
            ],
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
