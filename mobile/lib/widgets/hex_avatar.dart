// HexAvatar: hexagonal avatar. Glow border on neon, neuro relief on soft skins.
// Holds a generic feminine/masculine silhouette (Material icons) or a "+" for
// the future upload slot.
import 'package:flutter/material.dart';
import '../theme/tokens.dart';
import '../theme/settings.dart';

class HexAvatar extends StatelessWidget {
  final AvatarKind kind;
  final Color accent;
  final double size;
  final bool selected;
  final bool addButton;
  final VoidCallback? onTap;

  const HexAvatar({
    super.key,
    this.kind = AvatarKind.feminine,
    this.accent = BColors.monarch,
    this.size = 46,
    this.selected = true,
    this.addButton = false,
    this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    final skin = SettingsScope.of(context).skin;
    final neon = skin == ThemeSkin.systemNeon;
    // No hex frame on neuro skins — only the neon skin shows the hexagonal border.
    final iconSize = size * (neon ? 0.46 : 0.6);
    final dim = selected ? 1.0 : 0.5;
    final inner = SizedBox(
      width: size,
      height: size * 1.14,
      child: Center(
        child: addButton
            ? Text('+', style: BType.pixel(size: size * 0.32, color: accent))
            : SizedBox(
                width: iconSize,
                height: iconSize,
                child: CustomPaint(painter: _Silhouette(kind: kind, color: accent)),
              ),
      ),
    );
    return GestureDetector(
      onTap: onTap,
      child: Opacity(
        opacity: dim,
        child: neon
            ? CustomPaint(
                painter: _HexPainter(skin: skin, accent: accent, selected: selected),
                child: inner,
              )
            : inner,
      ),
    );
  }
}

Path _hexPath(Size s) => Path()
  ..moveTo(s.width / 2, 0)
  ..lineTo(s.width, s.height * .25)
  ..lineTo(s.width, s.height * .75)
  ..lineTo(s.width / 2, s.height)
  ..lineTo(0, s.height * .75)
  ..lineTo(0, s.height * .25)
  ..close();

class _HexPainter extends CustomPainter {
  final ThemeSkin skin;
  final Color accent;
  final bool selected;
  _HexPainter({required this.skin, required this.accent, required this.selected});

  @override
  void paint(Canvas canvas, Size size) {
    final path = _hexPath(size);
    if (skin == ThemeSkin.systemNeon) {
      canvas.drawPath(path, Paint()..color = BColors.deep);
      canvas.drawPath(
        path,
        Paint()
          ..style = PaintingStyle.stroke
          ..strokeWidth = 2
          ..color = accent.withOpacity(selected ? .55 : .3)
          ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 5),
      );
      canvas.drawPath(
        path,
        Paint()
          ..style = PaintingStyle.stroke
          ..strokeWidth = 1.2
          ..color = accent.withOpacity(.85),
      );
    } else {
      void shadow(Offset off, Color col) {
        canvas.save();
        canvas.translate(off.dx, off.dy);
        canvas.drawPath(
          path,
          Paint()
            ..color = col
            ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 4),
        );
        canvas.restore();
      }

      shadow(const Offset(-3, -3), BColors.nlight);
      shadow(const Offset(3, 3), BColors.ndark);
      canvas.drawPath(path, Paint()..color = BColors.nbg);
    }
  }

  @override
  bool shouldRepaint(_HexPainter o) =>
      o.skin != skin || o.accent != accent || o.selected != selected;
}

/// Generic person silhouette (head + body), matching the wireframe SVG:
/// feminine = flared/dress body, masculine = squared shoulders.
class _Silhouette extends CustomPainter {
  final AvatarKind kind;
  final Color color;
  _Silhouette({required this.kind, required this.color});

  @override
  void paint(Canvas canvas, Size size) {
    final w = size.width, h = size.height;
    final p = Paint()
      ..color = color
      ..style = PaintingStyle.fill
      ..isAntiAlias = true;
    // head
    canvas.drawCircle(Offset(w * .5, h * .30), w * .17, p);
    // body
    final body = Path();
    if (kind == AvatarKind.feminine) {
      body
        ..moveTo(w * .5, h * .47)
        ..lineTo(w * .74, h * .92)
        ..lineTo(w * .26, h * .92)
        ..close();
    } else {
      body.addRRect(RRect.fromRectAndRadius(
        Rect.fromLTWH(w * .27, h * .52, w * .46, h * .42),
        Radius.circular(w * .14),
      ));
    }
    canvas.drawPath(body, p);
  }

  @override
  bool shouldRepaint(_Silhouette o) => o.kind != kind || o.color != color;
}
