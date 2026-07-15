// SystemSurface: the style-aware "System window" container.
//  - systemNeon  → translucent fill + accent border + outer glow (chamfered)
//  - soft        → neuro relief (dual soft shadow), rounded
//  - softAngular → neuro relief on the chamfered ◤◢ silhouette
// `mode: groove` inverts the relief to look engraved (for bars / input fields).
// `skinOverride` forces a specific skin (used by the theme-picker previews).
import 'package:flutter/material.dart';
import '../theme/tokens.dart';
import '../theme/settings.dart';

enum SurfaceMode { raised, groove }

class SystemSurface extends StatelessWidget {
  final Widget child;
  final Color accent;
  final SurfaceMode mode;
  final EdgeInsetsGeometry padding;
  final double cut;
  final ThemeSkin? skinOverride;

  const SystemSurface({
    super.key,
    required this.child,
    this.accent = BColors.system,
    this.mode = SurfaceMode.raised,
    this.padding = const EdgeInsets.all(13),
    this.cut = 13,
    this.skinOverride,
  });

  @override
  Widget build(BuildContext context) {
    final skin = skinOverride ?? SettingsScope.of(context).skin;
    return CustomPaint(
      painter: _SurfacePainter(skin: skin, accent: accent, mode: mode, cut: cut),
      child: Padding(padding: padding, child: child),
    );
  }
}

Path buildSurfacePath(Size s, {required bool chamfer, required double cut}) {
  if (!chamfer) {
    return Path()
      ..addRRect(RRect.fromRectAndRadius(
          Offset.zero & s, const Radius.circular(16)));
  }
  final c = cut;
  return Path()
    ..moveTo(c, 0)
    ..lineTo(s.width, 0)
    ..lineTo(s.width, s.height - c)
    ..lineTo(s.width - c, s.height)
    ..lineTo(0, s.height)
    ..lineTo(0, c)
    ..close();
}

class _SurfacePainter extends CustomPainter {
  final ThemeSkin skin;
  final Color accent;
  final SurfaceMode mode;
  final double cut;

  _SurfacePainter({
    required this.skin,
    required this.accent,
    required this.mode,
    required this.cut,
  });

  @override
  void paint(Canvas canvas, Size size) {
    final chamfer = skin != ThemeSkin.soft;
    final path = buildSurfacePath(size, chamfer: chamfer, cut: cut);

    if (skin == ThemeSkin.systemNeon) {
      // translucent fill
      canvas.drawPath(path, Paint()..color = BColors.panel.withOpacity(.90));
      // outer glow
      canvas.drawPath(
        path,
        Paint()
          ..style = PaintingStyle.stroke
          ..strokeWidth = 2
          ..color = accent.withOpacity(.45)
          ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 6),
      );
      // crisp border
      canvas.drawPath(
        path,
        Paint()
          ..style = PaintingStyle.stroke
          ..strokeWidth = 1.2
          ..color = accent.withOpacity(.85),
      );
    } else if (skin == ThemeSkin.soft) {
      // OPT2 — neuro RELIEF (dual soft shadow), FLAT fill, faint border. No gradient.
      final raised = mode == SurfaceMode.raised;
      final lightOff = raised ? const Offset(-6, -6) : const Offset(6, 6);
      final darkOff = raised ? const Offset(6, 6) : const Offset(-6, -6);

      void shadow(Offset off, Color col, double blur) {
        canvas.save();
        canvas.translate(off.dx, off.dy);
        canvas.drawPath(
          path,
          Paint()
            ..color = col
            ..maskFilter = MaskFilter.blur(BlurStyle.normal, blur),
        );
        canvas.restore();
      }

      shadow(lightOff, BColors.nlight, 10);
      shadow(darkOff, BColors.ndark, 11);
      canvas.drawPath(
        path,
        Paint()..color = mode == SurfaceMode.groove ? BColors.groove : BColors.nbg,
      );
      if (mode == SurfaceMode.raised) {
        canvas.drawPath(
          path,
          Paint()
            ..style = PaintingStyle.stroke
            ..strokeWidth = 1
            ..color = accent.withOpacity(.16),
        );
      }
    } else {
      // OPT3 (softAngular) — semi-TRANSLUCENT box + diagonal gradient, NO relief.
      final fill = Paint();
      if (mode == SurfaceMode.groove) {
        fill.color = BColors.groove.withOpacity(.5);
      } else {
        fill.shader = LinearGradient(
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
          colors: [
            Color.lerp(BColors.nbg, accent, .22)!.withOpacity(.72),
            Color.lerp(BColors.nbg, Colors.white, .03)!.withOpacity(.40),
          ],
          stops: const [0.0, 0.6],
        ).createShader(Offset.zero & size);
      }
      canvas.drawPath(path, fill);
    }
  }

  @override
  bool shouldRepaint(_SurfacePainter o) =>
      o.skin != skin || o.accent != accent || o.mode != mode || o.cut != cut;
}

/// Clips a child to the chamfered ◤◢ silhouette (top-left + bottom-right cut).
class ChamferClipper extends CustomClipper<Path> {
  final double cut;
  const ChamferClipper(this.cut);
  @override
  Path getClip(Size size) =>
      buildSurfacePath(size, chamfer: true, cut: cut);
  @override
  bool shouldReclip(ChamferClipper o) => o.cut != cut;
}
