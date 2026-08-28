import 'package:flutter/material.dart';
import 'package:findit/src/rust/core/error.dart';

/// 把 FRB 抛出的 [FinditError] 转成用户友好的中文提示。
///
/// S-L4 脱敏约定：`db` / `io` / `aiModelOutput` 只显示通用提示，
/// 不回显 SQLite 细节、文件路径或 AI 服务端响应片段（原始细节保留在
/// 错误对象字段中，仅供调试日志使用）；`validation` 等业务校验信息
/// 本就是面向用户的提示，原样展示。
String friendlyErrorMessage(Object error) {
  if (error is FinditError) {
    return error.when(
      dbNotInitialized: () => '数据库尚未初始化，请重启应用后重试。',
      db: (detail) => '数据库操作失败，请重试。',
      duplicateName: (entity, name) => '已存在同名的$entity「$name」，请换个名字。',
      notFound: (entity, hint) => '找不到该$entity（$hint），可能已被删除。',
      validation: (detail) => detail,
      io: (detail) => '文件读写失败，请检查存储空间或文件权限后重试。',
      aiNotConfigured: (detail) => 'AI 未配置：$detail',
      aiUnreachable: (detail) => '无法连接 AI 服务：$detail',
      aiModelOutput: (detail) => 'AI 模型输出异常，请检查模型配置后重试。',
    );
  }
  return '出错了：$error';
}

/// 在 Scaffold 上弹出一条错误提示（底色跟随主题反色）。
void showErrorSnack(BuildContext context, Object error) {
  final messenger = ScaffoldMessenger.maybeOf(context);
  messenger?.showSnackBar(
    SnackBar(
      content: Text(friendlyErrorMessage(error)),
      behavior: SnackBarBehavior.floating,
    ),
  );
}
