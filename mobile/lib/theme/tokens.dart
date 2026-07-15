// Design tokens for the Bastion companion app.
// Palette + typography are SHARED across all three skins; only the surface
// treatment (glow vs relief, chamfer vs round) varies — see settings.dart.
import 'package:flutter/material.dart';

class BColors {
  BColors._();

  static const voidBg = Color(0xFF06070D); // app background (near-black navy)
  static const deep = Color(0xFF0B0E1A);
  static const panel = Color(0xFF11142A); // glass panel (used translucent)
  static const nbg = Color(0xFF171B2C); // neuro base surface
  static const nlight = Color(0xFF232B49); // neuro light shadow
  static const ndark = Color(0xFF090B13); // neuro dark shadow
  static const groove = Color(0xFF10131F); // engraved channel fill

  static const system = Color(0xFF22D3EE); // cyan — Sistema / você
  static const monarch = Color(0xFF8B5CF6); // violet — agente / sombra
  static const arise = Color(0xFFC026D3); // magenta — proativo
  static const text = Color(0xFFE6EAF5);
  static const muted = Color(0xFF7A86A8);
  static const ok = Color(0xFF34E5C4);
  static const danger = Color(0xFFFB5B7A);
  static const track = Color(0xFF1C2140);

  // Per-tick progression blue -> purple (StatBar segments).
  static const ticks = <Color>[
    Color(0xFF22D3EE),
    Color(0xFF37BBF0),
    Color(0xFF4CA3F1),
    Color(0xFF618CF3),
    Color(0xFF7674F4),
    Color(0xFF8B5CF6),
  ];

  /// Interpolated tick color for t in [0,1].
  static Color tick(double t) {
    if (t <= 0) return ticks.first;
    if (t >= 1) return ticks.last;
    final p = t * (ticks.length - 1);
    final i = p.floor();
    return Color.lerp(ticks[i], ticks[i + 1], p - i)!;
  }
}

class BType {
  BType._();

  // Pixel font — HUD labels, system tags, levels, buttons.
  static TextStyle pixel({
    double size = 10,
    Color color = BColors.text,
    double spacing = 1.5,
  }) =>
      TextStyle(
          fontFamily: 'Silkscreen',
          fontSize: size,
          color: color,
          letterSpacing: spacing);

  // Mono — readable body: chat text, settings rows, daemon output.
  static TextStyle mono({
    double size = 13,
    Color color = BColors.text,
    FontWeight weight = FontWeight.w400,
    double height = 1.45,
  }) =>
      TextStyle(
          fontFamily: 'JetBrainsMono',
          fontSize: size,
          color: color,
          fontWeight: weight,
          height: height);
}
