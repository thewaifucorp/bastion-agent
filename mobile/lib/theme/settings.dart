// AppSettings: user-facing display preferences (theme skin, avatar, behavior
// toggles), persisted locally via shared_preferences. These are CLIENT-side
// display prefs only — no secrets, nothing the daemon needs to know.
//
// Exposed to the widget tree via SettingsScope (InheritedNotifier): any widget
// that reads SettingsScope.of(context) rebuilds automatically when a pref
// changes — so switching theme re-skins the whole app instantly.
import 'package:flutter/material.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'tokens.dart';

/// Visual skins. `systemNeon` = glass + glow (chamfered). `soft` = neuro
/// (rounded). `softAngular` = neuro relief on the chamfered silhouette.
enum ThemeSkin { systemNeon, soft, softAngular }

enum AvatarKind { feminine, masculine }

class AppSettings extends ChangeNotifier {
  ThemeSkin skin;
  AvatarKind avatar;
  bool proactive;
  bool notifications;
  bool effects;

  AppSettings({
    this.skin = ThemeSkin.systemNeon,
    this.avatar = AvatarKind.feminine,
    this.proactive = true,
    this.notifications = true,
    this.effects = true,
  });

  static const _kSkin = 'set_skin';
  static const _kAvatar = 'set_avatar';
  static const _kPro = 'set_proactive';
  static const _kNotif = 'set_notif';
  static const _kFx = 'set_fx';

  static Future<AppSettings> load() async {
    final p = await SharedPreferences.getInstance();
    final skinIdx = (p.getInt(_kSkin) ?? 0).clamp(0, ThemeSkin.values.length - 1);
    final avIdx = (p.getInt(_kAvatar) ?? 0).clamp(0, AvatarKind.values.length - 1);
    return AppSettings(
      skin: ThemeSkin.values[skinIdx],
      avatar: AvatarKind.values[avIdx],
      proactive: p.getBool(_kPro) ?? true,
      notifications: p.getBool(_kNotif) ?? true,
      effects: p.getBool(_kFx) ?? true,
    );
  }

  Future<void> _save() async {
    final p = await SharedPreferences.getInstance();
    await p.setInt(_kSkin, skin.index);
    await p.setInt(_kAvatar, avatar.index);
    await p.setBool(_kPro, proactive);
    await p.setBool(_kNotif, notifications);
    await p.setBool(_kFx, effects);
  }

  void setSkin(ThemeSkin v) {
    skin = v;
    notifyListeners();
    _save();
  }

  void setAvatar(AvatarKind v) {
    avatar = v;
    notifyListeners();
    _save();
  }

  void setProactive(bool v) {
    proactive = v;
    notifyListeners();
    _save();
  }

  void setNotifications(bool v) {
    notifications = v;
    notifyListeners();
    _save();
  }

  void setEffects(bool v) {
    effects = v;
    notifyListeners();
    _save();
  }

  /// `soft` is the only rounded skin; neon + softAngular are chamfered.
  bool get chamfered => skin != ThemeSkin.soft;

  /// neon uses glow; soft + softAngular use neuro relief.
  bool get neuro => skin != ThemeSkin.systemNeon;

  /// Scanline/glow FX only make sense on the neon skin.
  bool get fxOn => skin == ThemeSkin.systemNeon && effects;

  /// Screen background. NEURO REQUIREMENT: the scaffold must be the SAME color
  /// as the neuro surfaces (nbg) so the dual-shadow relief blends instead of
  /// bleeding onto a darker void. Neon sits on the dark void with glow.
  Color get screenBg => neuro ? BColors.nbg : BColors.voidBg;
}

class SettingsScope extends InheritedNotifier<AppSettings> {
  const SettingsScope({
    super.key,
    required AppSettings settings,
    required super.child,
  }) : super(notifier: settings);

  static AppSettings of(BuildContext context) {
    final scope =
        context.dependOnInheritedWidgetOfExactType<SettingsScope>();
    assert(scope != null, 'SettingsScope not found in widget tree');
    return scope!.notifier!;
  }
}
