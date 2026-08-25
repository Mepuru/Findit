import 'package:flutter_test/flutter_test.dart';
import 'package:findit/src/errors.dart';
import 'package:findit/src/rust/core/error.dart';

void main() {
  group('friendlyErrorMessage', () {
    test('重名错误给出中文友好提示', () {
      const error =
          FinditError.duplicateName(entity: '存储单元', name: '客厅柜子');
      expect(
        friendlyErrorMessage(error),
        '已存在同名的存储单元「客厅柜子」，请换个名字。',
      );
    });

    test('未初始化错误给出重启提示', () {
      const error = FinditError.dbNotInitialized();
      expect(friendlyErrorMessage(error), contains('重启应用'));
    });

    test('非 FinditError 原样兜底', () {
      expect(friendlyErrorMessage(StateError('boom')), contains('boom'));
    });
  });
}
