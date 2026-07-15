// Basic smoke test for BastionApp root widget.
import 'package:flutter_test/flutter_test.dart';
import 'package:bastion_companion/main.dart';
import 'package:bastion_companion/theme/settings.dart';

void main() {
  testWidgets('BastionApp smoke test', (WidgetTester tester) async {
    await tester.pumpWidget(BastionApp(settings: AppSettings()));
    // App root renders without throwing
    expect(tester.takeException(), isNull);
  });
}
