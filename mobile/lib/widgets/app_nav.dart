// AppNav: persistent bottom navigation (Chat / Cockpit / Config), style-aware.
// Matches the mock: inline `glyph LABEL` per item, active = accent text + a
// top indicator line, transparent background over the screen with a neon
// hairline on top.
import 'package:flutter/material.dart';
import '../theme/tokens.dart';
import '../theme/settings.dart';

class AppNav extends StatelessWidget {
  final int index;
  final ValueChanged<int> onTap;
  const AppNav({super.key, required this.index, required this.onTap});

  static const _items = [
    ('◳', 'CHAT'),
    ('◈', 'COCKPIT'),
    ('⚙', 'CONFIG'),
  ];

  @override
  Widget build(BuildContext context) {
    final s = SettingsScope.of(context);
    return Container(
      decoration: BoxDecoration(
        color: s.screenBg,
        border: Border(
          top: BorderSide(
            color: s.skin == ThemeSkin.systemNeon
                ? BColors.system.withValues(alpha: .18)
                : Colors.white.withValues(alpha: .04),
          ),
        ),
      ),
      child: SafeArea(
        top: false,
        child: Row(
          children: List.generate(_items.length, (i) {
            final active = i == index;
            final (glyph, label) = _items[i];
            final color = active ? BColors.system : BColors.muted;
            return Expanded(
              child: GestureDetector(
                behavior: HitTestBehavior.opaque,
                onTap: () => onTap(i),
                child: Container(
                  padding: const EdgeInsets.symmetric(vertical: 15),
                  decoration: BoxDecoration(
                    border: Border(
                      top: BorderSide(
                        color: active ? BColors.system : Colors.transparent,
                        width: 2,
                      ),
                    ),
                  ),
                  child: Row(
                    mainAxisAlignment: MainAxisAlignment.center,
                    children: [
                      Text(glyph, style: TextStyle(fontSize: 12, color: color)),
                      const SizedBox(width: 7),
                      Text(label,
                          style: BType.pixel(size: 8, spacing: 1, color: color)),
                    ],
                  ),
                ),
              ),
            );
          }),
        ),
      ),
    );
  }
}
