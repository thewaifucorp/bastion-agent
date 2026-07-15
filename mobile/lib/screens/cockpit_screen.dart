// CockpitScreen: body widget inside HomeShell. Preserves the conversational
// data flow (D-06): /goals, /drift, /memories, /contest <id> over POST /webhook.
// Presentation is the Shadow Monarch HUD; drift shows a StatBar when the daemon
// response contains a percentage, else the raw text.
import 'package:flutter/material.dart';
import '../services/api_service.dart';
import '../theme/tokens.dart';
import '../widgets/system_surface.dart';
import '../widgets/stat_bar.dart';
import '../widgets/hud_header.dart';

class BeliefEntry {
  final String id;
  final String content;
  BeliefEntry({required this.id, required this.content});

  static List<BeliefEntry> parse(String response) {
    return response
        .split('\n')
        .map((l) => l.trim())
        .where((l) => l.isNotEmpty)
        .map((l) {
          final i = l.indexOf(':');
          if (i < 1) return null;
          final id = l.substring(0, i).trim();
          final content = l.substring(i + 1).trim();
          if (id.isEmpty || content.isEmpty) return null;
          return BeliefEntry(id: id, content: content);
        })
        .whereType<BeliefEntry>()
        .toList();
  }
}

const bool _kDemoSeed =
    bool.fromEnvironment('DEMO_SEED', defaultValue: false); // QA-only mock cockpit data

class CockpitScreen extends StatefulWidget {
  final ApiService api;
  const CockpitScreen({super.key, required this.api});

  @override
  State<CockpitScreen> createState() => _CockpitScreenState();
}

class _CockpitScreenState extends State<CockpitScreen> {
  String _drift = '…';
  String _goals = '…';
  List<BeliefEntry> _beliefs = [];
  bool _loading = false;
  String? _contestingId;

  @override
  void initState() {
    super.initState();
    if (_kDemoSeed) {
      _drift = 'drift estável (75%) — sem sinais de deriva.';
      _goals =
          '2/5 metas ativas\n- Lançar v1.0 no GitHub (80%)\n- Revisar orçamento mensal (30%)';
      _beliefs = BeliefEntry.parse(
          '1: Mario prefere café sem açúcar\n2: Trabalha com IA e agentes\n3: Acorda cedo pra treinar');
      return;
    }
    _refresh();
  }

  Future<void> _refresh() async {
    setState(() => _loading = true);
    try {
      final results = await Future.wait([
        widget.api.sendMessage('/drift'),
        widget.api.sendMessage('/goals'),
        widget.api.sendMessage('/memories'),
      ]);
      if (!mounted) return;
      setState(() {
        _drift = results[0].isNotEmpty ? results[0] : 'sem deriva detectada.';
        _goals = results[1].isNotEmpty ? results[1] : 'nenhuma meta ativa.';
        _beliefs = BeliefEntry.parse(results[2]);
      });
    } catch (e) {
      if (mounted) setState(() => _drift = 'erro ao carregar: $e');
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  double? _driftPercent(String text) {
    final m = RegExp(r'(\d{1,3})\s*%').firstMatch(text);
    if (m == null) return null;
    final n = int.tryParse(m.group(1)!);
    if (n == null) return null;
    return (n.clamp(0, 100)) / 100.0;
  }

  Future<void> _contest(String id) async {
    setState(() => _contestingId = id);
    try {
      final r = await widget.api.sendMessage('/contest $id');
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text(r.isNotEmpty ? r : 'Contestado.')),
        );
        await _refresh();
      }
    } catch (e) {
      if (mounted) {
        ScaffoldMessenger.of(context)
            .showSnackBar(SnackBar(content: Text('Erro ao contestar: $e')));
      }
    } finally {
      if (mounted) setState(() => _contestingId = null);
    }
  }

  @override
  Widget build(BuildContext context) {
    final pct = _driftPercent(_drift);
    return Column(
      children: [
        HudHeader(
          tag: 'STATUS',
          trailing: GestureDetector(
            onTap: _loading ? null : _refresh,
            child: Text('⟳', style: BType.pixel(size: 14, color: BColors.system)),
          ),
        ),
        Expanded(
          child: ListView(
            padding: const EdgeInsets.fromLTRB(14, 16, 14, 16),
            children: [
              // DRIFT
              _Panel(
                accent: BColors.system,
                title: 'DRIFT',
                value: pct != null ? 'ESTÁVEL' : null,
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    if (pct != null) ...[
                      Text('${(pct * 100).round()}%',
                          style: BType.pixel(size: 20, color: BColors.text)),
                      const SizedBox(height: 8),
                      StatBar(value: pct),
                      const SizedBox(height: 8),
                    ],
                    Text(_drift, style: BType.mono(size: 12, color: BColors.muted)),
                  ],
                ),
              ),
              const SizedBox(height: 16),
              // METAS
              _Panel(
                accent: BColors.monarch,
                title: 'METAS ATIVAS',
                child: Text(_goals, style: BType.mono(size: 13)),
              ),
              const SizedBox(height: 16),
              // MEMÓRIAS CONTESTÁVEIS
              _Panel(
                accent: BColors.arise,
                title: 'MEMÓRIAS CONTESTÁVEIS',
                value: '${_beliefs.length}',
                child: _beliefs.isEmpty
                    ? Text('nenhuma memória disponível.',
                        style: BType.mono(size: 12, color: BColors.muted))
                    : Column(
                        children: _beliefs
                            .map((b) => _MemoryRow(
                                  belief: b,
                                  busy: _contestingId == b.id,
                                  onContest: () => _contest(b.id),
                                ))
                            .toList(),
                      ),
              ),
              const SizedBox(height: 16),
              // MESH
              _Panel(
                accent: BColors.system,
                title: 'MESH',
                child: Text(
                  'Para conectar peers: digite /connect-peer no chat do Bastion.\n'
                  'Peers ativos e status de sync aparecem aqui quando conectados.',
                  style: BType.mono(size: 12, color: BColors.muted),
                ),
              ),
            ],
          ),
        ),
      ],
    );
  }
}

class _Panel extends StatelessWidget {
  final Color accent;
  final String title;
  final String? value;
  final Widget child;
  const _Panel(
      {required this.accent, required this.title, this.value, required this.child});

  @override
  Widget build(BuildContext context) {
    return SystemSurface(
      accent: accent,
      padding: const EdgeInsets.all(14),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            mainAxisAlignment: MainAxisAlignment.spaceBetween,
            children: [
              Text(title, style: BType.pixel(size: 9, color: accent, spacing: 1.5)),
              if (value != null)
                Text(value!, style: BType.pixel(size: 9, color: BColors.monarch)),
            ],
          ),
          const SizedBox(height: 10),
          child,
        ],
      ),
    );
  }
}

class _MemoryRow extends StatelessWidget {
  final BeliefEntry belief;
  final bool busy;
  final VoidCallback onContest;
  const _MemoryRow(
      {required this.belief, required this.busy, required this.onContest});

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 7),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text('⟢ ', style: BType.mono(size: 12, color: BColors.monarch)),
          Expanded(child: Text(belief.content, style: BType.mono(size: 12))),
          const SizedBox(width: 8),
          GestureDetector(
            onTap: busy ? null : onContest,
            child: busy
                ? const SizedBox(
                    width: 14, height: 14, child: CircularProgressIndicator(strokeWidth: 2))
                : SystemSurface(
                    accent: BColors.ok,
                    cut: 5,
                    padding: const EdgeInsets.symmetric(horizontal: 7, vertical: 5),
                    child: Text('CONTESTAR',
                        style: BType.pixel(size: 7, color: BColors.ok, spacing: 1)),
                  ),
          ),
        ],
      ),
    );
  }
}
