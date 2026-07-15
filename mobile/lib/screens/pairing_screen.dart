// PairingScreen: daemon URL + one-time token (BAST-XXXX from /connect-app) →
// JWT via ApiService.pair(). Restyled to the active skin.
import 'package:flutter/material.dart';
import '../services/api_service.dart';
import '../theme/tokens.dart';
import '../theme/settings.dart';
import '../widgets/system_surface.dart';
import '../widgets/hex_avatar.dart';

class PairingScreen extends StatefulWidget {
  final ApiService api;
  final VoidCallback onPaired;
  const PairingScreen({super.key, required this.api, required this.onPaired});

  @override
  State<PairingScreen> createState() => _PairingScreenState();
}

class _PairingScreenState extends State<PairingScreen> {
  final _urlCtrl = TextEditingController(text: 'http://192.168.0.8:8787');
  final _otcCtrl = TextEditingController();
  bool _loading = false;
  String? _error;

  Future<void> _pair() async {
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      await widget.api.pair(_urlCtrl.text.trim(), _otcCtrl.text.trim());
      widget.onPaired();
    } catch (e) {
      setState(() => _error = 'Pareamento falhou: $e');
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  Widget _field(String label, TextEditingController c, String hint) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(label, style: BType.pixel(size: 8, color: BColors.muted, spacing: 1)),
        const SizedBox(height: 6),
        SystemSurface(
          mode: SurfaceMode.groove,
          cut: 9,
          padding: const EdgeInsets.symmetric(horizontal: 13, vertical: 6),
          child: TextField(
            controller: c,
            style: BType.mono(size: 13),
            cursorColor: BColors.system,
            decoration: InputDecoration(
              isDense: true,
              border: InputBorder.none,
              hintText: hint,
              hintStyle: BType.mono(size: 13, color: BColors.muted),
            ),
          ),
        ),
      ],
    );
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: SettingsScope.of(context).screenBg,
      body: SafeArea(
        child: Center(
          child: SingleChildScrollView(
            padding: const EdgeInsets.all(28),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Center(child: HexAvatar(accent: BColors.monarch, size: 64)),
                const SizedBox(height: 18),
                Center(
                  child: Text('⟦ CONECTAR AO BASTION ⟧',
                      style: BType.pixel(size: 12, color: BColors.system, spacing: 2)),
                ),
                const SizedBox(height: 10),
                Text(
                  'No Bastion, digite /connect-app para gerar seu código de uso único (BAST-XXXX).',
                  textAlign: TextAlign.center,
                  style: BType.mono(size: 12, color: BColors.muted),
                ),
                const SizedBox(height: 26),
                _field('DAEMON URL', _urlCtrl, 'http://192.168.0.8:8787'),
                const SizedBox(height: 16),
                _field('ONE-TIME TOKEN', _otcCtrl, 'BAST-XXXX-XXXX'),
                const SizedBox(height: 22),
                if (_error != null) ...[
                  Text(_error!, style: BType.mono(size: 12, color: BColors.danger)),
                  const SizedBox(height: 14),
                ],
                GestureDetector(
                  onTap: _loading ? null : _pair,
                  child: SystemSurface(
                    accent: BColors.system,
                    cut: 10,
                    padding: const EdgeInsets.all(15),
                    child: Center(
                      child: _loading
                          ? const SizedBox(
                              width: 16,
                              height: 16,
                              child: CircularProgressIndicator(
                                  strokeWidth: 2, color: BColors.system))
                          : Text('⟫ PAREAR',
                              style: BType.pixel(size: 10, color: BColors.system, spacing: 1)),
                    ),
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}
