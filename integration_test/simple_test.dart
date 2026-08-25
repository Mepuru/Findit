import 'package:flutter_test/flutter_test.dart';
import 'package:findit/main.dart';
import 'package:findit/src/rust/frb_generated.dart';
import 'package:integration_test/integration_test.dart';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  setUpAll(() async => await RustLib.init());
  testWidgets('Can boot the app', (WidgetTester tester) async {
    await tester.pumpWidget(const FinditApp());
    expect(find.textContaining('Findit'), findsWidgets);
  });
}
