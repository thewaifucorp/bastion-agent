import 'package:flutter/material.dart';
import 'services/api_service.dart';
import 'theme/tokens.dart';
import 'theme/settings.dart';
import 'screens/pairing_screen.dart';
import 'screens/home_shell.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  final settings = await AppSettings.load();
  runApp(BastionApp(settings: settings));
}

class BastionApp extends StatelessWidget {
  final AppSettings settings;
  const BastionApp({super.key, required this.settings});

  @override
  Widget build(BuildContext context) {
    // SettingsScope is an InheritedNotifier: any widget reading SettingsScope.of
    // rebuilds when a pref changes, so switching skin re-skins the app instantly.
    return SettingsScope(
      settings: settings,
      child: MaterialApp(
        title: 'Bastion',
        debugShowCheckedModeBanner: false,
        theme: ThemeData(
          useMaterial3: true,
          brightness: Brightness.dark,
          scaffoldBackgroundColor: BColors.voidBg,
          colorScheme: const ColorScheme.dark(
            primary: BColors.system,
            secondary: BColors.monarch,
            surface: BColors.panel,
          ),
        ),
        home: const AppRoot(),
      ),
    );
  }
}

// Debug-only: skip pairing to inspect the UI without a paired daemon.
// Enable with: flutter run --dart-define=BYPASS_PAIRING=true
const bool kBypassPairing =
    bool.fromEnvironment('BYPASS_PAIRING', defaultValue: false);

class AppRoot extends StatefulWidget {
  const AppRoot({super.key});

  @override
  State<AppRoot> createState() => _AppRootState();
}

class _AppRootState extends State<AppRoot> {
  final ApiService _api = ApiService();
  bool? _paired;

  @override
  void initState() {
    super.initState();
    if (kBypassPairing) {
      _paired = true;
      return;
    }
    _api.isPaired().then((p) {
      if (mounted) setState(() => _paired = p);
    });
  }

  @override
  Widget build(BuildContext context) {
    if (_paired == null) {
      return const Scaffold(
        backgroundColor: BColors.voidBg,
        body: Center(child: CircularProgressIndicator(color: BColors.system)),
      );
    }
    if (!_paired!) {
      return PairingScreen(
        api: _api,
        onPaired: () => setState(() => _paired = true),
      );
    }
    return HomeShell(
      api: _api,
      // In bypass mode there's no JWT, so SSE would immediately fire onAuthExpired
      // and bounce back to pairing — make it a no-op for the visual test.
      onUnpair: kBypassPairing ? () {} : () => setState(() => _paired = false),
    );
  }
}
