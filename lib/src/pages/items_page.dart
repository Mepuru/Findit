import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart'
    show Int64List;
import 'package:image_picker/image_picker.dart';
import 'package:findit/src/rust/api/boxes.dart' as boxes_api;
import 'package:findit/src/rust/api/categories.dart' as categories_api;
import 'package:findit/src/rust/api/items.dart' as api;
import 'package:findit/src/rust/api/model.dart';
import 'package:findit/src/rust/api/photos.dart' as photos_api;
import 'package:findit/src/rust/api/units.dart' as units_api;

import '../errors.dart';
import '../theme.dart';
import '../widgets/backfill.dart';
import '../widgets/box_qr_sheet.dart';
import '../widgets/common.dart';
import 'categories_page.dart';
import 'photo_viewer_page.dart';

/// 第三级：某个收纳箱内的物品列表。
class ItemsPage extends StatefulWidget {
  const ItemsPage({super.key, required this.boxId, this.highlightItemId});

  final int boxId;

  /// 从搜索结果直达时高亮定位的物品 id；`null` 表示普通进入。
  final int? highlightItemId;

  @override
  State<ItemsPage> createState() => _ItemsPageState();
}

class _ItemsPageState extends State<ItemsPage> {
  StorageBox? _box;
  List<Item>? _items;
  Object? _loadError;

  /// item_id → 照片本地路径（列表缩略图 + 大图）。
  final Map<int, ({String thumb, String full})> _photoPaths = {};

  /// 物品卡片的 GlobalKey（用于搜索结果直达时滚动定位）。
  final Map<int, GlobalKey> _itemKeys = {};
  final ScrollController _scrollController = ScrollController();

  @override
  void initState() {
    super.initState();
    _reload();
  }

  @override
  void dispose() {
    _scrollController.dispose();
    super.dispose();
  }

  Future<void> _reload() async {
    try {
      final results = await Future.wait([
        boxes_api.getBox(id: widget.boxId),
        api.listItems(boxId: widget.boxId),
      ]);
      final box = results[0] as StorageBox;
      final items = results[1] as List<Item>;

      // 解析照片本地路径（并发解析；文件缺失时仅不展示）。
      final photos = <int, ({String thumb, String full})>{};
      await Future.wait([
        for (final item in items)
          if (item.photoPath != null)
            () async {
              try {
                final paths = await Future.wait([
                  photos_api.getThumbFullPath(fileName: item.photoPath!),
                  photos_api.getPhotoFullPath(fileName: item.photoPath!),
                ]);
                photos[item.id] = (
                  thumb: paths[0],
                  full: paths[1],
                );
              } catch (_) {}
            }(),
      ]);

      if (!mounted) return;
      setState(() {
        _box = box;
        _items = items;
        _itemKeys
          ..clear()
          ..addEntries(items.map((i) => MapEntry(i.id, GlobalKey())));
        _photoPaths
          ..clear()
          ..addAll(photos);
        _loadError = null;
      });
      _scrollToHighlight();
    } catch (e) {
      if (!mounted) return;
      setState(() => _loadError = e);
    }
  }

  /// 搜索结果直达：列表构建完成后滚动到目标物品并让其保持高亮。
  void _scrollToHighlight() {
    final targetId = widget.highlightItemId;
    final items = _items;
    if (targetId == null || items == null || !mounted) return;
    final index = items.indexWhere((i) => i.id.toInt() == targetId);
    if (index < 0) return;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      if (index >= 6 && _scrollController.hasClients) {
        _scrollController.jumpTo(
          (index * 76.0)
              .clamp(0.0, _scrollController.position.maxScrollExtent),
        );
      }
      final ctx = _itemKeys[targetId]?.currentContext;
      if (ctx != null) {
        Scrollable.ensureVisible(
          ctx,
          duration: const Duration(milliseconds: 250),
          alignment: 0.3,
        );
      }
    });
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
      builder: (sheetContext) => _ItemFormSheet(
        item: item,
        boxId: widget.boxId,
        onPhotoChanged: _reload,
        onSubmit:
            (name, description, quantity, categoryIds, boxId, photoBytes) async {
          if (item == null) {
            final created = await api.createItem(
              boxId: boxId,
              name: name,
              description: description,
              quantity: quantity,
              categoryIds: categoryIds,
            );
            // 建档时已选照片：物品创建后立即留档（选图时已限制尺寸/质量）。
            if (photoBytes != null) {
              await photos_api.saveItemPhoto(
                itemId: created.id,
                bytes: photoBytes,
              );
            }
          } else {
            await api.updateItem(
              id: item.id,
              name: name,
              description: description,
              quantity: quantity,
              boxId: boxId,
              categoryIds: categoryIds,
            );
          }
          await _reload();
          // 写入触发的向量回填做防抖合并，避免每次写入立即发起网络调用。
          scheduleBackfill();
        },
      ),
    );
  }

  /// F8：物品删除改为「SnackBar 级撤销（延迟删除）」：
  /// 先乐观移除并弹出撤销条，超时未撤销才真正删除；
  /// 点「撤销」立即恢复（重新加载列表）。
  /// 整单元级联删除（units_page/boxes_page）仍保留确认弹窗，不做撤销。
  Future<void> _delete(Item item) async {
    final items = _items;
    if (items == null) return;
    final messenger = ScaffoldMessenger.of(context);
    messenger.hideCurrentSnackBar();
    setState(() {
      _items = items.where((i) => i.id != item.id).toList();
    });

    // 5 秒内可撤销；超时后真正删除。
    // 用 Completer 而非「等待后检查」：用户点撤销立即完成；否则 5s 后完成，
    // 即使期间页面已退出（mounted=false）也照常执行删除，避免物品复活。
    final undoGate = Completer<bool>();
    messenger.showSnackBar(
      SnackBar(
        content: Text('已删除「${item.name}」'),
        duration: const Duration(seconds: 5),
        action: SnackBarAction(
          label: '撤销',
          onPressed: () {
            if (!undoGate.isCompleted) undoGate.complete(true);
          },
        ),
      ),
    );
    Timer(const Duration(seconds: 5), () {
      if (!undoGate.isCompleted) undoGate.complete(false);
    });
    final undone = await undoGate.future;
    if (undone) {
      // 撤销：物品尚未真正删除，重新加载即恢复原状。
      if (mounted) await _reload();
      return;
    }
    try {
      await api.deleteItem(id: item.id);
      if (!mounted) return;
      setState(() {
        _photoPaths.remove(item.id);
        _itemKeys.remove(item.id);
      });
    } catch (e) {
      if (mounted) {
        showErrorSnack(context, e);
        await _reload();
      }
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
            controller: _scrollController,
            padding: const EdgeInsets.fromLTRB(16, 4, 16, 96),
            itemCount: items.length,
            separatorBuilder: (context, index) => const SizedBox(height: 10),
            itemBuilder: (context, index) => _ItemCard(
              key: _itemKeys[items[index].id],
              item: items[index],
              photoThumb: _photoPaths[items[index].id]?.thumb,
              highlighted:
                  items[index].id.toInt() == widget.highlightItemId,
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
    super.key,
    required this.item,
    required this.photoThumb,
    required this.onPhotoTap,
    required this.onEdit,
    required this.onDelete,
    this.highlighted = false,
  });

  final Item item;

  /// 缩略图本地路径；无照片时为 `null`。
  final String? photoThumb;
  final VoidCallback onPhotoTap;
  final VoidCallback onEdit;
  final VoidCallback onDelete;

  /// 搜索结果直达定位的高亮标记（卡片描边 + 底色提亮）。
  final bool highlighted;

  /// F9：把 ISO 8601 UTC 时间转成本地「MM-dd HH:mm」紧凑格式。
  static String _fmtTime(String iso) {
    final dt = DateTime.tryParse(iso)?.toLocal();
    if (dt == null) return '';
    String two(int v) => v.toString().padLeft(2, '0');
    return '${two(dt.month)}-${two(dt.day)} ${two(dt.hour)}:${two(dt.minute)}';
  }

  /// F9：登记/更新时间标签；更新晚于登记时两者都展示，否则只展示登记时间。
  String _itemTimeLabel(Item item) {
    final created = _fmtTime(item.createdAt);
    final updated = _fmtTime(item.updatedAt);
    if (updated.isEmpty) return '登记于 $created';
    if (created == updated || item.updatedAt == item.createdAt) {
      return '登记于 $created';
    }
    return '登记于 $created · 更新于 $updated';
  }

  Widget _tagIcon(BuildContext context) => Container(
        width: 42,
        height: 42,
        alignment: Alignment.center,
        decoration: BoxDecoration(
          color: context.palette.persimmon.withValues(alpha: 0.18),
          borderRadius: BorderRadius.circular(12),
        ),
        child: const Text('🏷️', style: TextStyle(fontSize: 22)),
      );

  @override
  Widget build(BuildContext context) {
    final palette = context.palette;
    final thumb = photoThumb;
    return Card(
      color: highlighted ? palette.persimmon.withValues(alpha: 0.08) : null,
      shape: highlighted
          ? RoundedRectangleBorder(
              borderRadius: BorderRadius.circular(14),
              side: BorderSide(color: palette.persimmon, width: 1.5),
            )
          : null,
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
                      errorBuilder: (context, error, stackTrace) =>
                          _tagIcon(context),
                    ),
                  ),
                )
              else
                _tagIcon(context),
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
                              .copyWith(color: palette.persimmon),
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
                            .copyWith(color: palette.inkSoft),
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
                                color: palette.chipFill,
                                borderRadius: BorderRadius.circular(999),
                              ),
                              child: Text(
                                c,
                                style: Theme.of(context)
                                    .textTheme
                                    .labelSmall!
                                    .copyWith(
                                      color: palette.pineDeep,
                                      letterSpacing: 0.2,
                                    ),
                              ),
                            ),
                        ],
                      ),
                    // F9：登记/更新时间（model 已有 createdAt/updatedAt，ISO UTC）。
                    const SizedBox(height: 4),
                    Text(
                      _itemTimeLabel(item),
                      style: Theme.of(context).textTheme.labelSmall!.copyWith(
                            color: palette.inkSoft.withValues(alpha: 0.85),
                          ),
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
                itemBuilder: (context) => [
                  const PopupMenuItem(value: 'edit', child: Text('编辑')),
                  PopupMenuItem(
                    value: 'delete',
                    child: Text(
                      '删除',
                      style: TextStyle(color: palette.danger),
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

/// 物品表单：名称、备注、数量、所在收纳箱、分类多选（支持行内新建分类）；
/// 编辑态另含照片区（拍照/选图/替换/删除/大图预览），建档态支持先选照片、保存后留档。
class _ItemFormSheet extends StatefulWidget {
  const _ItemFormSheet({
    required this.item,
    required this.boxId,
    required this.onSubmit,
    this.onPhotoChanged,
  });

  final Item? item;

  /// 表单打开时所在的收纳箱 id（作为「所在收纳箱」选择器的初始值）。
  final int boxId;
  final Future<void> Function(
    String name,
    String description,
    int quantity,
    Int64List categoryIds,
    int boxId,
    Uint8List? photoBytes,
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

  /// 「所在收纳箱」选择器：全单元平铺选项（单元名 / 箱名）。
  List<({int id, String label})> _boxOptions = [];
  int _boxId = 0;
  bool _boxesError = false;

  /// 建档态待保存的照片字节（保存物品后立即留档）。
  Uint8List? _pendingPhotoBytes;
  final bool _photoPicking = false;

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
    _boxId = widget.boxId;
    _loadCategories();
    _loadBoxes();
  }

  /// 加载「所在收纳箱」选项：所有单元下全部收纳箱（支持跨单元移动）。
  Future<void> _loadBoxes() async {
    try {
      final units = await units_api.listUnits();
      final options = <({int id, String label})>[];
      for (final unit in units) {
        final boxes = await boxes_api.listBoxes(unitId: unit.id);
        for (final b in boxes) {
          options.add((id: b.id.toInt(), label: '${unit.name} / ${b.name}'));
        }
      }
      if (!mounted) return;
      setState(() {
        _boxOptions = options;
        _boxesError = false;
        // 当前箱不存在（极端情况）时回退到第一个可用箱。
        if (!options.any((o) => o.id == widget.boxId)) {
          _boxId = options.isEmpty ? widget.boxId : options.first.id;
        }
      });
    } catch (e) {
      if (!mounted) return;
      setState(() => _boxesError = true);
    }
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
        _boxId,
        _pendingPhotoBytes,
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
  /// 选图时限制最长边与质量（P-M7），降低原图字节数后再过桥传输。
  Future<void> _pickAndSavePhoto(ImageSource source) async {
    final item = widget.item;
    if (item == null || _photoBusy) return;
    try {
      final picked = await ImagePicker().pickImage(
        source: source,
        maxWidth: 1600,
        imageQuality: 85,
      );
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

  /// 建档态选图：先暂存字节，保存物品后一并留档（F7）。
  /// 选图时限制最长边与质量（P-M7），降低内存占用与过桥传输量。
  Future<void> _pickCreatePhoto(ImageSource source) async {
    if (_photoPicking) return;
    try {
      final picked = await ImagePicker().pickImage(
        source: source,
        maxWidth: 1600,
        imageQuality: 85,
      );
      if (picked == null) return;
      final bytes = await picked.readAsBytes();
      if (!mounted) return;
      setState(() => _pendingPhotoBytes = bytes);
    } catch (e) {
      if (mounted) showErrorSnack(context, e);
    }
  }

  /// 建档态照片区：预览已选图片 + 拍照/相册/移除。
  Widget _buildCreatePhotoArea() {
    final palette = context.palette;
    final bytes = _pendingPhotoBytes;
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Container(
          width: 88,
          height: 88,
          decoration: BoxDecoration(
            color: palette.chipFill,
            borderRadius: BorderRadius.circular(12),
            border: Border.all(color: palette.cardLine),
          ),
          clipBehavior: Clip.antiAlias,
          child: bytes != null
              ? Image.memory(bytes, fit: BoxFit.cover)
              : Icon(
                  Icons.photo_camera_outlined,
                  color: palette.inkSoft,
                ),
        ),
        const SizedBox(width: 12),
        Expanded(
          child: Wrap(
            spacing: 8,
            runSpacing: 8,
            children: [
              OutlinedButton.icon(
                onPressed: _photoPicking
                    ? null
                    : () => _pickCreatePhoto(ImageSource.camera),
                icon: const Icon(Icons.photo_camera, size: 16),
                label: Text(bytes != null ? '重新拍照' : '拍照'),
              ),
              OutlinedButton.icon(
                onPressed: _photoPicking
                    ? null
                    : () => _pickCreatePhoto(ImageSource.gallery),
                icon: const Icon(Icons.photo_library_outlined, size: 16),
                label: const Text('相册'),
              ),
              if (bytes != null)
                TextButton.icon(
                  onPressed: () => setState(() => _pendingPhotoBytes = null),
                  icon: Icon(
                    Icons.delete_outline_rounded,
                    size: 16,
                    color: palette.danger,
                  ),
                  label: Text(
                    '移除',
                    style: TextStyle(color: palette.danger),
                  ),
                ),
            ],
          ),
        ),
      ],
    );
  }

  /// 照片区：左侧缩略图（可点大图）+ 右侧操作按钮。
  Widget _buildPhotoArea() {
    final palette = context.palette;
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
              color: palette.chipFill,
              borderRadius: BorderRadius.circular(12),
              border: Border.all(color: palette.cardLine),
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
                        errorBuilder: (context, error, stackTrace) => Icon(
                          Icons.broken_image_outlined,
                          color: palette.inkSoft,
                        ),
                      );
                    },
                  )
                : Icon(
                    Icons.photo_camera_outlined,
                    color: palette.inkSoft,
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
                      icon: Icon(
                        Icons.delete_outline_rounded,
                        size: 16,
                        color: palette.danger,
                      ),
                      label: Text(
                        '删除',
                        style: TextStyle(color: palette.danger),
                      ),
                    ),
                ],
              ),
              if (_photoBusy) ...[
                const SizedBox(height: 8),
                Text(
                  '正在压缩保存…',
                  style: Theme.of(context).textTheme.labelSmall!.copyWith(
                        color: palette.inkSoft,
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
    final palette = context.palette;
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
                  color: palette.cardLine,
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
            Text(
              isEdit ? '照片留档' : '照片（可选）',
              style: Theme.of(context).textTheme.titleSmall,
            ),
            const SizedBox(height: 8),
            if (isEdit)
              _buildPhotoArea()
            else
              _buildCreatePhotoArea(),
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
            const SizedBox(height: 12),
            if (_boxesError)
              Text(
                '收纳箱列表加载失败，无法移动物品。',
                style: Theme.of(context).textTheme.bodySmall!.copyWith(
                      color: palette.danger,
                    ),
              )
            else if (_boxOptions.isEmpty)
              const SizedBox(
                height: 48,
                child: Center(
                  child: SizedBox(
                    width: 18,
                    height: 18,
                    child: CircularProgressIndicator(strokeWidth: 2),
                  ),
                ),
              )
            else
              DropdownButtonFormField<int>(
                initialValue: _boxId,
                decoration: const InputDecoration(
                  labelText: '所在收纳箱',
                  helperText: '可移动到其它单元/收纳箱（无需 AI）',
                ),
                isExpanded: true,
                items: [
                  for (final o in _boxOptions)
                    DropdownMenuItem(
                      value: o.id,
                      child: Text(
                        o.label,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                      ),
                    ),
                ],
                onChanged: (v) {
                  if (v != null) setState(() => _boxId = v);
                },
              ),
            const SizedBox(height: 18),
            Row(
              children: [
                Text(
                  '分类（可多选）',
                  style: Theme.of(context).textTheme.titleSmall,
                ),
                const Spacer(),
                // F4：分类管理入口（重命名/删除），与设置页「数据管理」入口互补。
                TextButton.icon(
                  onPressed: () => Navigator.of(context).push(
                    MaterialPageRoute(
                      builder: (_) => const CategoriesPage(),
                    ),
                  ),
                  icon: const Icon(Icons.label_outline_rounded, size: 16),
                  label: const Text('管理分类'),
                ),
              ],
            ),
            const SizedBox(height: 8),
            if (_categoriesError != null)
              Text(
                '分类加载失败：${friendlyErrorMessage(_categoriesError!)}',
                style: Theme.of(context).textTheme.bodySmall!.copyWith(
                      color: palette.danger,
                    ),
              )
            else if (_categories.isEmpty)
              Text(
                '还没有分类，在下方输入框新建一个吧。',
                style: Theme.of(context).textTheme.bodySmall!.copyWith(
                      color: palette.inkSoft,
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
                      selectedColor: palette.pine.withValues(alpha: 0.2),
                      checkmarkColor: palette.pineDeep,
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
