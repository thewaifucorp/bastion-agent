// ConfigScreen: settings hub — theme skin picker (live), avatar, connection,
// behavior toggles. Theme/avatar/toggles persist via AppSettings.
import 'package:flutter/material.dart';
import '../services/api_service.dart';
import '../theme/tokens.dart';
import '../theme/settings.dart';
import '../widgets/system_surface.dart';
import '../widgets/hex_avatar.dart';
import '../widgets/hud_header.dart';

class ConfigScreen extends StatelessWidget {
  final ApiService api;
  final VoidCallback onUnpair;
  const ConfigScreen({super.key, required this.api, required this.onUnpair});

  @override
  Widget build(BuildContext context) {
    final s = SettingsScope.of(context);
    return Column(
      children: [
        HudHeader(
          tag: 'CONFIG',
          trailing: Text('⚙', style: BType.pixel(size: 14, color: BColors.system)),
        ),
        Expanded(
          child: ListView(
            padding: const EdgeInsets.fromLTRB(16, 14, 16, 16),
            children: [
              _section('APARÊNCIA / TEMA'),
              Row(
                children: [
                  _ThemeTile(
                      skin: ThemeSkin.systemNeon,
                      label: 'SYSTEM\nNEON',
                      selected: s.skin == ThemeSkin.systemNeon,
                      onTap: () => s.setSkin(ThemeSkin.systemNeon)),
                  const SizedBox(width: 11),
                  _ThemeTile(
                      skin: ThemeSkin.soft,
                      label: 'SOFT\n(NEURO)',
                      selected: s.skin == ThemeSkin.soft,
                      onTap: () => s.setSkin(ThemeSkin.soft)),
                  const SizedBox(width: 11),
                  _ThemeTile(
                      skin: ThemeSkin.softAngular,
                      label: 'SOFT\nANGULAR',
                      selected: s.skin == ThemeSkin.softAngular,
                      onTap: () => s.setSkin(ThemeSkin.softAngular)),
                ],
              ),
              _section('AVATAR'),
              Row(
                children: [
                  HexAvatar(
                      kind: AvatarKind.feminine,
                      accent: BColors.monarch,
                      size: 44,
                      selected: s.avatar == AvatarKind.feminine,
                      onTap: () => s.setAvatar(AvatarKind.feminine)),
                  const SizedBox(width: 12),
                  HexAvatar(
                      kind: AvatarKind.masculine,
                      accent: BColors.system,
                      size: 44,
                      selected: s.avatar == AvatarKind.masculine,
                      onTap: () => s.setAvatar(AvatarKind.masculine)),
                  const SizedBox(width: 12),
                  HexAvatar(
                      accent: BColors.system,
                      size: 44,
                      selected: false,
                      addButton: true,
                      onTap: () {
                        ScaffoldMessenger.of(context).showSnackBar(
                          const SnackBar(content: Text('Upload de ícone — em breve')),
                        );
                      }),
                ],
              ),
              _section('CONEXÃO'),
              FutureBuilder<String>(
                future: api.getDaemonUrl(),
                builder: (ctx, snap) => SystemSurface(
                  mode: SurfaceMode.groove,
                  cut: 9,
                  padding: const EdgeInsets.symmetric(horizontal: 13, vertical: 12),
                  child: Row(
                    children: [
                      Expanded(
                        child: Text(snap.data ?? '…',
                            style: BType.mono(size: 12)),
                      ),
                      Text('● ONLINE',
                          style: BType.pixel(size: 9, color: BColors.ok, spacing: .5)),
                    ],
                  ),
                ),
              ),
              const SizedBox(height: 10),
              GestureDetector(
                onTap: () async {
                  await api.clearAuth();
                  onUnpair();
                },
                child: SystemSurface(
                  cut: 8,
                  padding: const EdgeInsets.all(12),
                  child: Center(
                    child: Text('⟲ REPAREAR DISPOSITIVO',
                        style: BType.pixel(size: 8, color: BColors.system, spacing: 1)),
                  ),
                ),
              ),
              _section('COMPORTAMENTO'),
              _Toggle(
                  label: 'Nudges proativos',
                  hint: 'o agente fala primeiro quando relevante',
                  value: s.proactive,
                  onChanged: s.setProactive),
              _Toggle(
                  label: 'Notificações push',
                  hint: 'avisa no celular fora do app',
                  value: s.notifications,
                  onChanged: s.setNotifications),
              _Toggle(
                  label: 'Efeitos (scanline/glow)',
                  hint: 'só no tema System Neon',
                  value: s.effects,
                  onChanged: s.setEffects),
            ],
          ),
        ),
      ],
    );
  }

  Widget _section(String t) => Padding(
        padding: const EdgeInsets.fromLTRB(2, 20, 2, 11),
        child: Text(t, style: BType.pixel(size: 9, color: BColors.system, spacing: 1.5)),
      );
}

class _ThemeTile extends StatelessWidget {
  final ThemeSkin skin;
  final String label;
  final bool selected;
  final VoidCallback onTap;
  const _ThemeTile(
      {required this.skin,
      required this.label,
      required this.selected,
      required this.onTap});

  @override
  Widget build(BuildContext context) {
    return Expanded(
      child: GestureDetector(
        onTap: onTap,
        child: SystemSurface(
          skinOverride: skin,
          cut: 10,
          padding: const EdgeInsets.fromLTRB(8, 8, 8, 9),
          child: Column(
            children: [
              // mini sample bar in the tile's own skin
              SizedBox(
                height: 30,
                child: Center(
                  child: Container(
                    width: double.infinity,
                    height: 8,
                    decoration: BoxDecoration(
                      gradient: const LinearGradient(
                          colors: [BColors.system, BColors.monarch]),
                      borderRadius: BorderRadius.circular(2),
                      boxShadow: skin == ThemeSkin.systemNeon
                          ? [BoxShadow(color: BColors.system.withOpacity(.6), blurRadius: 6)]
                          : null,
                    ),
                  ),
                ),
              ),
              const SizedBox(height: 7),
              Text(label,
                  textAlign: TextAlign.center,
                  style: BType.pixel(
                      size: 6.5,
                      spacing: .5,
                      color: selected ? BColors.system : BColors.muted)),
              const SizedBox(height: 4),
              Text(selected ? '✦' : '·',
                  style: TextStyle(
                      color: selected ? BColors.ok : BColors.muted, fontSize: 11)),
            ],
          ),
        ),
      ),
    );
  }
}

class _Toggle extends StatelessWidget {
  final String label;
  final String hint;
  final bool value;
  final ValueChanged<bool> onChanged;
  const _Toggle(
      {required this.label,
      required this.hint,
      required this.value,
      required this.onChanged});

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 8),
      child: Row(
        children: [
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(label, style: BType.mono(size: 13)),
                const SizedBox(height: 2),
                Text(hint, style: BType.mono(size: 10, color: BColors.muted)),
              ],
            ),
          ),
          Switch(
            value: value,
            onChanged: onChanged,
            activeColor: Colors.white,
            activeTrackColor: BColors.monarch,
            inactiveThumbColor: BColors.muted,
            inactiveTrackColor: BColors.groove,
          ),
        ],
      ),
    );
  }
}
