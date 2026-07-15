// HomeShell: the paired-state app shell. Owns the Scaffold + bottom nav and
// keeps Chat/Cockpit/Config alive via IndexedStack (so SSE + scroll persist
// across tab switches). Renders a subtle scanline overlay when FX are on.
import 'package:flutter/material.dart';
import '../services/api_service.dart';
import '../theme/tokens.dart';
import '../theme/settings.dart';
import '../widgets/app_nav.dart';
import 'chat_screen.dart';
import 'cockpit_screen.dart';
import 'config_screen.dart';

class HomeShell extends StatefulWidget {
  final ApiService api;
  final VoidCallback onUnpair;
  const HomeShell({super.key, required this.api, required this.onUnpair});

  @override
  State<HomeShell> createState() => _HomeShellState();
}

class _HomeShellState extends State<HomeShell> {
  int _index = 0;

  @override
  Widget build(BuildContext context) {
    final s = SettingsScope.of(context);
    return Scaffold(
      backgroundColor: s.screenBg,
      body: SafeArea(
        bottom: false,
        child: Stack(
          children: [
            // Neon screen ambience — soft purple/cyan glows top, like the mock body.
            if (s.skin == ThemeSkin.systemNeon)
              const Positioned.fill(child: IgnorePointer(child: _NeonGlow())),
            IndexedStack(
              index: _index,
              children: [
                ChatScreen(api: widget.api, onAuthExpired: widget.onUnpair),
                CockpitScreen(api: widget.api),
                ConfigScreen(api: widget.api, onUnpair: widget.onUnpair),
              ],
            ),
            if (s.fxOn)
              const Positioned.fill(
                child: IgnorePointer(child: _Scanlines()),
              ),
          ],
        ),
      ),
      bottomNavigationBar:
          AppNav(index: _index, onTap: (i) => setState(() => _index = i)),
    );
  }
}

class _NeonGlow extends StatelessWidget {
  const _NeonGlow();
  @override
  Widget build(BuildContext context) {
    return DecoratedBox(
      decoration: BoxDecoration(
        gradient: RadialGradient(
          center: const Alignment(-0.7, -1.05),
          radius: 1.25,
          colors: [BColors.monarch.withValues(alpha: .20), Colors.transparent],
          stops: const [0.0, 0.55],
        ),
      ),
      child: DecoratedBox(
        decoration: BoxDecoration(
          gradient: RadialGradient(
            center: const Alignment(1.1, -0.85),
            radius: 1.1,
            colors: [BColors.system.withValues(alpha: .12), Colors.transparent],
            stops: const [0.0, 0.5],
          ),
        ),
      ),
    );
  }
}

class _Scanlines extends StatelessWidget {
  const _Scanlines();
  @override
  Widget build(BuildContext context) =>
      CustomPaint(painter: _ScanlinePainter(), size: Size.infinite);
}

class _ScanlinePainter extends CustomPainter {
  @override
  void paint(Canvas canvas, Size size) {
    final paint = Paint()..color = Colors.white.withOpacity(.015);
    for (double y = 0; y < size.height; y += 3) {
      canvas.drawRect(Rect.fromLTWH(0, y, size.width, 1), paint);
    }
  }

  @override
  bool shouldRepaint(_ScanlinePainter oldDelegate) => false;
}
