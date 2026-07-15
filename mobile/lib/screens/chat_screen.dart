// ChatScreen: chat UI as a body widget inside HomeShell (shell owns Scaffold +
// nav). Preserves the working data flow: ApiService.sendMessage ({text}/{reply},
// CR-05) and SseService for real-time events. Only the presentation changed.
import 'package:flutter/material.dart';
import 'dart:convert';
import '../services/api_service.dart';
import '../services/sse_service.dart';
import '../theme/tokens.dart';
import '../theme/settings.dart';
import '../widgets/system_surface.dart';
import '../widgets/hud_header.dart';
import '../widgets/stat_bar.dart';

const bool _kDemoSeed =
    bool.fromEnvironment('DEMO_SEED', defaultValue: false); // QA-only sample bubbles

enum _Sender { user, agent, event }

class ChatMessage {
  final String content;
  final _Sender sender;
  ChatMessage(this.content, this.sender);
}

class ChatScreen extends StatefulWidget {
  final ApiService api;
  final VoidCallback onAuthExpired;
  const ChatScreen({super.key, required this.api, required this.onAuthExpired});

  @override
  State<ChatScreen> createState() => _ChatScreenState();
}

class _ChatScreenState extends State<ChatScreen> {
  final _messages = <ChatMessage>[];
  final _inputCtrl = TextEditingController();
  final _scroll = ScrollController();
  final _sse = SseService();
  bool _sending = false;

  // HUD status (fetched once for the header bars — mirrors the mockup).
  double? _driftPct;
  int _metasOn = 0;
  final int _metasTotal = 5;

  @override
  void initState() {
    super.initState();
    if (_kDemoSeed) {
      _messages.addAll([
        ChatMessage('como tá meu drift hoje?', _Sender.user),
        ChatMessage(
            'drift estável (75%) — sem sinais de deriva.\n2 metas ativas, nenhuma em risco.',
            _Sender.agent),
        ChatMessage('lembrete: você pediu pra revisar o orçamento até sexta.',
            _Sender.event),
      ]);
      _driftPct = 0.75;
      _metasOn = 2;
    }
    _startSse();
    _loadStatus();
  }

  Future<void> _loadStatus() async {
    try {
      final drift = await widget.api.sendMessage('/drift');
      final goals = await widget.api.sendMessage('/goals');
      if (!mounted) return;
      final dm = RegExp(r'(\d{1,3})\s*%').firstMatch(drift);
      final gm = RegExp(r'(\d+)\s*/\s*(\d+)').firstMatch(goals);
      setState(() {
        if (dm != null) {
          _driftPct = (int.parse(dm.group(1)!).clamp(0, 100)) / 100.0;
        }
        if (gm != null) {
          _metasOn = int.parse(gm.group(1)!);
        }
      });
    } catch (_) {
      // header bars are decorative; ignore fetch errors
    }
  }

  Future<void> _startSse() async {
    final url = await widget.api.getDaemonUrl();
    _sse.start(
      daemonUrl: url,
      onEvent: (event) {
        try {
          final data = jsonDecode(event) as Map<String, dynamic>;
          final type = data['type'] as String? ?? 'event';
          if (type != 'mesh_sync') {
            setState(() => _messages.add(ChatMessage(event, _Sender.event)));
            _scrollToEnd();
          }
        } catch (_) {}
      },
      onAuthExpired: () {
        widget.api.clearAuth();
        widget.onAuthExpired();
      },
    );
  }

  void _scrollToEnd() {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (_scroll.hasClients) {
        _scroll.animateTo(_scroll.position.maxScrollExtent,
            duration: const Duration(milliseconds: 250), curve: Curves.easeOut);
      }
    });
  }

  Future<void> _send() async {
    final text = _inputCtrl.text.trim();
    if (text.isEmpty || _sending) return;
    _inputCtrl.clear();
    setState(() {
      _messages.add(ChatMessage(text, _Sender.user));
      _sending = true;
    });
    _scrollToEnd();
    try {
      final reply = await widget.api.sendMessage(text);
      setState(() => _messages.add(ChatMessage(reply, _Sender.agent)));
    } catch (e) {
      setState(() => _messages.add(ChatMessage('erro: $e', _Sender.agent)));
    } finally {
      if (mounted) {
        setState(() => _sending = false);
        _scrollToEnd();
      }
    }
  }

  @override
  void dispose() {
    _sse.dispose();
    _inputCtrl.dispose();
    _scroll.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Column(
      children: [
        HudHeader(
          tag: 'SYSTEM',
          trailing: Text('LV.7', style: BType.pixel(size: 11, color: BColors.monarch)),
          below: Column(
            children: [
              StatRow(label: 'DRIFT', bar: StatBar(value: _driftPct ?? 0.0)),
              const SizedBox(height: 4),
              StatRow(label: 'METAS', bar: _Pips(on: _metasOn, total: _metasTotal)),
            ],
          ),
        ),
        Expanded(
          child: ListView.builder(
            controller: _scroll,
            padding: const EdgeInsets.fromLTRB(14, 16, 14, 16),
            itemCount: _messages.length,
            itemBuilder: (ctx, i) => _Bubble(msg: _messages[i]),
          ),
        ),
        _InputBar(controller: _inputCtrl, sending: _sending, onSend: _send),
      ],
    );
  }
}

class _Bubble extends StatelessWidget {
  final ChatMessage msg;
  const _Bubble({required this.msg});

  @override
  Widget build(BuildContext context) {
    final isUser = msg.sender == _Sender.user;
    late final Color accent;
    late final String who;
    switch (msg.sender) {
      case _Sender.user:
        accent = BColors.system;
        who = 'YOU ◢';
        break;
      case _Sender.agent:
        accent = BColors.monarch;
        who = '◤ ${kPersonaName} · SHADOW';
        break;
      case _Sender.event:
        accent = BColors.arise;
        who = '⟢ SISTEMA';
        break;
    }
    return Padding(
      padding: const EdgeInsets.only(bottom: 16),
      child: Column(
        crossAxisAlignment:
            isUser ? CrossAxisAlignment.end : CrossAxisAlignment.start,
        children: [
          Padding(
            padding: const EdgeInsets.only(bottom: 6, left: 4, right: 4),
            child: Text(who, style: BType.pixel(size: 8, color: accent, spacing: 1.5)),
          ),
          ConstrainedBox(
            constraints: BoxConstraints(
                maxWidth: MediaQuery.of(context).size.width * 0.82),
            child: SystemSurface(
              accent: accent,
              padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 11),
              child: Text(msg.content, style: BType.mono(size: 13)),
            ),
          ),
        ],
      ),
    );
  }
}

class _InputBar extends StatelessWidget {
  final TextEditingController controller;
  final bool sending;
  final VoidCallback onSend;
  const _InputBar(
      {required this.controller, required this.sending, required this.onSend});

  @override
  Widget build(BuildContext context) {
    final neon = SettingsScope.of(context).skin == ThemeSkin.systemNeon;
    return Container(
      decoration: BoxDecoration(
        // faint purple fade-up under the input (the bottom "degradê" from the mock)
        gradient: neon
            ? LinearGradient(
                begin: Alignment.bottomCenter,
                end: Alignment.topCenter,
                colors: [BColors.monarch.withValues(alpha: .08), Colors.transparent],
              )
            : null,
      ),
      padding: const EdgeInsets.fromLTRB(12, 10, 12, 14),
      // Single groove field with the SEND button INSIDE it (HTML `.send` lives
      // inside `.field`).
      child: SystemSurface(
        mode: SurfaceMode.groove,
        cut: 10,
        padding: const EdgeInsets.fromLTRB(13, 6, 6, 6),
        child: Row(
          children: [
            Text('▸', style: BType.mono(size: 14, color: BColors.system, weight: FontWeight.w700)),
            const SizedBox(width: 8),
            Expanded(
              child: TextField(
                controller: controller,
                style: BType.mono(size: 13),
                cursorColor: BColors.system,
                decoration: InputDecoration(
                  isDense: true,
                  border: InputBorder.none,
                  hintText: 'digite uma mensagem…',
                  hintStyle: BType.mono(size: 13, color: BColors.muted),
                ),
                onSubmitted: (_) => onSend(),
              ),
            ),
            const SizedBox(width: 8),
            GestureDetector(
              onTap: sending ? null : onSend,
              child: _SendButton(sending: sending),
            ),
          ],
        ),
      ),
    );
  }
}

/// SEND button — neon: solid cyan→purple gradient pill + dark text + glow
/// (matches the mock). Neuro: raised soft chip with cyan text.
class _SendButton extends StatelessWidget {
  final bool sending;
  const _SendButton({required this.sending});

  @override
  Widget build(BuildContext context) {
    final neon = SettingsScope.of(context).skin == ThemeSkin.systemNeon;
    final label = sending
        ? const SizedBox(
            width: 12,
            height: 12,
            child: CircularProgressIndicator(strokeWidth: 2, color: BColors.voidBg))
        : Text('SEND ⟫',
            style: BType.pixel(
                size: 9, color: neon ? BColors.voidBg : BColors.system, spacing: 1));

    if (!neon) {
      return SystemSurface(
        accent: BColors.system,
        cut: 8,
        padding: const EdgeInsets.symmetric(horizontal: 13, vertical: 9),
        child: sending
            ? const SizedBox(
                width: 12,
                height: 12,
                child: CircularProgressIndicator(strokeWidth: 2, color: BColors.system))
            : Text('SEND ⟫',
                style: BType.pixel(size: 9, color: BColors.system, spacing: 1)),
      );
    }
    return Container(
      decoration: BoxDecoration(
        boxShadow: [BoxShadow(color: BColors.system.withValues(alpha: .5), blurRadius: 12)],
      ),
      child: ClipPath(
        clipper: const ChamferClipper(8),
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 9),
          decoration: const BoxDecoration(
            gradient: LinearGradient(
              begin: Alignment.topLeft,
              end: Alignment.bottomRight,
              colors: [BColors.system, BColors.monarch],
            ),
          ),
          child: label,
        ),
      ),
    );
  }
}

/// METAS pips — diamond cells (filled = monarch→arise gradient).
class _Pips extends StatelessWidget {
  final int on;
  final int total;
  const _Pips({required this.on, required this.total});

  @override
  Widget build(BuildContext context) {
    return Row(
      children: List.generate(total, (i) {
        final lit = i < on;
        return Container(
          width: 9,
          height: 9,
          margin: const EdgeInsets.only(right: 5),
          transform: Matrix4.rotationZ(0.7853981633974483), // 45°
          transformAlignment: Alignment.center,
          decoration: BoxDecoration(
            color: lit ? null : BColors.track,
            gradient: lit
                ? const LinearGradient(colors: [BColors.monarch, BColors.arise])
                : null,
            boxShadow: lit
                ? [BoxShadow(color: BColors.arise.withValues(alpha: .5), blurRadius: 6)]
                : null,
          ),
        );
      }),
    );
  }
}
