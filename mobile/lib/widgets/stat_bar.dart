// StatBar: segmented HP/MP-style bar with the blue→purple per-tick progression.
// On the neon skin each filled segment glows; on neuro skins the bar sits in an
// engraved groove (SystemSurface in groove mode).
import 'package:flutter/material.dart';
import '../theme/tokens.dart';
import '../theme/settings.dart';
import 'system_surface.dart';

class StatBar extends StatelessWidget {
  final double value; // 0..1
  final int segments;
  const StatBar({super.key, required this.value, this.segments = 8});

  @override
  Widget build(BuildContext context) {
    final s = SettingsScope.of(context);
    final filled = (value * segments).round().clamp(0, segments);

    final row = Row(
      children: List.generate(segments, (i) {
        final on = i < filled;
        Color color;
        if (on) {
          final t = filled <= 1 ? 0.0 : i / (filled - 1);
          color = BColors.tick(t.toDouble());
        } else {
          color = s.neuro ? Colors.transparent : BColors.track;
        }
        return Expanded(
          child: Container(
            height: 10,
            margin: EdgeInsets.only(right: i == segments - 1 ? 0 : 3),
            decoration: BoxDecoration(
              color: color,
              borderRadius: BorderRadius.circular(2),
              boxShadow: on && s.skin == ThemeSkin.systemNeon
                  ? [BoxShadow(color: color.withOpacity(.6), blurRadius: 6)]
                  : null,
            ),
          ),
        );
      }),
    );

    if (s.neuro) {
      return SystemSurface(
        mode: SurfaceMode.groove,
        cut: 7,
        padding: const EdgeInsets.symmetric(horizontal: 5, vertical: 4),
        child: row,
      );
    }
    return row;
  }
}
