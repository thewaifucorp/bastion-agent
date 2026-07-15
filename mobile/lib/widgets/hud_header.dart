// HudHeader: shared top bar — hex avatar + system tag + persona name + trailing.
// Persona name is a placeholder ("ARIA") until the daemon exposes the active
// persona; swap `persona` when that endpoint lands.
import 'package:flutter/material.dart';
import '../theme/tokens.dart';
import '../theme/settings.dart';
import 'hex_avatar.dart';

const String kPersonaName = 'ARIA';

class HudHeader extends StatelessWidget {
  final String tag; // SYSTEM / STATUS / CONFIG
  final Widget trailing; // LV.7 / gear
  final Widget? below; // optional stat bars under the row
  const HudHeader({
    super.key,
    required this.tag,
    required this.trailing,
    this.below,
  });

  @override
  Widget build(BuildContext context) {
    final s = SettingsScope.of(context);
    final neon = s.skin == ThemeSkin.systemNeon;
    return Container(
      padding: const EdgeInsets.fromLTRB(16, 14, 16, 10),
      decoration: BoxDecoration(
        // faint purple fade-down (the top "degradê" from the mock)
        gradient: neon
            ? LinearGradient(
                begin: Alignment.topCenter,
                end: Alignment.bottomCenter,
                colors: [BColors.monarch.withValues(alpha: .10), Colors.transparent],
              )
            : null,
        border: Border(
          bottom: BorderSide(
            color: neon ? BColors.system.withOpacity(.18) : Colors.transparent,
          ),
        ),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              HexAvatar(
                kind: s.avatar,
                accent: BColors.monarch,
                size: 44,
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Row(
                      children: [
                        const Text('●',
                            style: TextStyle(color: BColors.ok, fontSize: 9)),
                        const SizedBox(width: 6),
                        Text(tag,
                            style: BType.pixel(
                                size: 11, color: BColors.muted, spacing: 2)),
                      ],
                    ),
                    const SizedBox(height: 4),
                    Text(kPersonaName,
                        style: BType.pixel(size: 11, color: BColors.monarch)),
                  ],
                ),
              ),
              trailing,
            ],
          ),
          if (below != null) ...[
            const SizedBox(height: 10),
            below!,
          ],
        ],
      ),
    );
  }
}

/// A small labeled stat row: `LABEL [bar]` used inside HUDs.
class StatRow extends StatelessWidget {
  final String label;
  final Widget bar;
  const StatRow({super.key, required this.label, required this.bar});

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 3),
      child: Row(
        children: [
          SizedBox(
            width: 46,
            child: Text(label,
                style: BType.pixel(size: 8, color: BColors.muted, spacing: 1)),
          ),
          const SizedBox(width: 8),
          Expanded(child: bar),
        ],
      ),
    );
  }
}
