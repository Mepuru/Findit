import 'package:flutter/material.dart';
import '../errors.dart';
import '../theme.dart';

/// 空态引导：虚线框 + 图标 + 提示文案 + 行动按钮。
class EmptyState extends StatelessWidget {
  const EmptyState({
    super.key,
    required this.emoji,
    required this.title,
    required this.subtitle,
    this.actionLabel,
    this.onAction,
  });

  final String emoji;
  final String title;
  final String subtitle;
  final String? actionLabel;
  final VoidCallback? onAction;

  @override
  Widget build(BuildContext context) {
    final palette = context.palette;
    return Center(
      child: Padding(
        padding: const EdgeInsets.all(28),
        child: Container(
          width: double.infinity,
          padding: const EdgeInsets.symmetric(vertical: 36, horizontal: 24),
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(18),
            border: Border.all(
              color: palette.cardLine,
              width: 1.5,
              strokeAlign: BorderSide.strokeAlignInside,
            ),
            color: palette.cardFace.withValues(alpha: 0.6),
          ),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              Text(emoji, style: const TextStyle(fontSize: 44)),
              const SizedBox(height: 14),
              Text(title, style: Theme.of(context).textTheme.titleMedium),
              const SizedBox(height: 6),
              Text(
                subtitle,
                textAlign: TextAlign.center,
                style: Theme.of(context).textTheme.bodySmall!.copyWith(
                      color: palette.inkSoft,
                    ),
              ),
              if (actionLabel != null && onAction != null) ...[
                const SizedBox(height: 20),
                FilledButton.icon(
                  onPressed: onAction,
                  icon: const Icon(Icons.add, size: 18),
                  label: Text(actionLabel!),
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}

/// 确认删除对话框。返回 `true` 表示用户确认。
Future<bool> confirmDelete(
  BuildContext context, {
  required String title,
  required String message,
}) async {
  final result = await showDialog<bool>(
    context: context,
    builder: (context) => AlertDialog(
      title: Text(title),
      content: Text(message),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(false),
          child: const Text('取消'),
        ),
        FilledButton(
          style: FilledButton.styleFrom(
            backgroundColor: context.palette.danger,
          ),
          onPressed: () => Navigator.of(context).pop(true),
          child: const Text('删除'),
        ),
      ],
    ),
  );
  return result ?? false;
}

/// 名称/描述 两字段底部弹窗表单。
/// [initialName]/[initialDescription] 非空时为编辑态。
Future<void> showNameSheet({
  required BuildContext context,
  required String title,
  required String nameLabel,
  String nameHint = '',
  bool withDescription = true,
  String? initialName,
  String? initialDescription,
  required Future<void> Function(String name, String description) onSubmit,
}) {
  final nameController = TextEditingController(text: initialName ?? '');
  final descController =
      TextEditingController(text: initialDescription ?? '');

  return showModalBottomSheet<void>(
    context: context,
    isScrollControlled: true,
    builder: (sheetContext) {
      return Padding(
        padding: EdgeInsets.only(
          left: 20,
          right: 20,
          top: 16,
          bottom: MediaQuery.of(sheetContext).viewInsets.bottom + 24,
        ),
        child: _NameSheetBody(
          title: title,
          nameLabel: nameLabel,
          nameHint: nameHint,
          withDescription: withDescription,
          nameController: nameController,
          descController: descController,
          onSubmit: onSubmit,
        ),
      );
    },
  );
}

class _NameSheetBody extends StatefulWidget {
  const _NameSheetBody({
    required this.title,
    required this.nameLabel,
    required this.nameHint,
    required this.withDescription,
    required this.nameController,
    required this.descController,
    required this.onSubmit,
  });

  final String title;
  final String nameLabel;
  final String nameHint;
  final bool withDescription;
  final TextEditingController nameController;
  final TextEditingController descController;
  final Future<void> Function(String name, String description) onSubmit;

  @override
  State<_NameSheetBody> createState() => _NameSheetBodyState();
}

class _NameSheetBodyState extends State<_NameSheetBody> {
  bool _busy = false;

  Future<void> _submit() async {
    final name = widget.nameController.text.trim();
    if (name.isEmpty) {
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(
          content: Text('${widget.nameLabel}不能为空'),
          behavior: SnackBarBehavior.floating,
        ),
      );
      return;
    }
    setState(() => _busy = true);
    try {
      await widget.onSubmit(name, widget.descController.text.trim());
      if (mounted) Navigator.of(context).pop();
    } catch (e) {
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(
            content: Text(friendlyErrorMessage(e)),
            behavior: SnackBarBehavior.floating,
          ),
        );
      }
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Center(
          child: Container(
            width: 36,
            height: 4,
            decoration: BoxDecoration(
              color: context.palette.cardLine,
              borderRadius: BorderRadius.circular(2),
            ),
          ),
        ),
        const SizedBox(height: 16),
        Text(widget.title, style: Theme.of(context).textTheme.titleMedium),
        const SizedBox(height: 16),
        TextField(
          controller: widget.nameController,
          autofocus: true,
          textInputAction: TextInputAction.done,
          decoration: InputDecoration(
            labelText: widget.nameLabel,
            hintText: widget.nameHint.isEmpty ? null : widget.nameHint,
          ),
        ),
        if (widget.withDescription) ...[
          const SizedBox(height: 12),
          TextField(
            controller: widget.descController,
            maxLines: 2,
            decoration: const InputDecoration(
              labelText: '备注（可选）',
            ),
          ),
        ],
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
    );
  }
}

/// 卡片右侧的数量徽标（如「3 箱」「12 件」）。
class CountBadge extends StatelessWidget {
  const CountBadge({super.key, required this.count, required this.unit});

  final int count;
  final String unit;

  @override
  Widget build(BuildContext context) {
    final palette = context.palette;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 5),
      decoration: BoxDecoration(
        color: palette.chipFill,
        borderRadius: BorderRadius.circular(999),
      ),
      child: Text(
        '$count $unit',
        style: Theme.of(context).textTheme.labelSmall!.copyWith(
              color: palette.pineDeep,
              letterSpacing: 0.4,
            ),
      ),
    );
  }
}

/// 小节标题行（如「收纳箱 · 4」）。
class SectionHeader extends StatelessWidget {
  const SectionHeader({super.key, required this.text});

  final String text;

  @override
  Widget build(BuildContext context) {
    return Text(
      text.toUpperCase(),
      style: Theme.of(context).textTheme.labelSmall!.copyWith(
            color: context.palette.inkSoft,
          ),
    );
  }
}
