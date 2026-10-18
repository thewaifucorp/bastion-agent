// PresenceScreen: what Bastion is doing right now. Active persona, host-agent
// sessions, pending approvals (Approve / Deny with a countdown) and alerts.
// Data: GET /presence/snapshot, POST /presence/action; live refresh from the
// /events SSE feed (presence.updated, approval.requested, approval.resolved,
// presence.persona), debounced. 404 = daemon older than presence or disabled.
import 'dart:async';
import 'dart:convert';
import 'package:flutter/material.dart';
import '../services/api_service.dart';
import '../services/sse_service.dart';
import '../theme/tokens.dart';
import '../widgets/system_surface.dart';
import '../widgets/hud_header.dart';

class PresenceScreen extends StatefulWidget {
  final ApiService api;
  final VoidCallback onAuthExpired;
  const PresenceScreen({super.key, required this.api, required this.onAuthExpired});

  @override
  State<PresenceScreen> createState() => _PresenceScreenState();
}

class _PresenceScreenState extends State<PresenceScreen> {
  final _sse = SseService(connectionId: 'bastion_presence');
  PresenceSnapshot? _snap;
  bool _loading = false;
  bool _unavailable = false;
  String? _error;
  final _acting = <String>{};
  String? _bannerId;
  Timer? _debounce;
  Timer? _tick;
  int _clockOffset = 0; // daemon "now" minus local clock, seconds

  @override
  void initState() {
    super.initState();
    _refresh();
    _startSse();
    _tick = Timer.periodic(const Duration(seconds: 1), (_) {
      if (mounted && (_snap?.approvals.isNotEmpty ?? false)) setState(() {});
    });
  }

  @override
  void dispose() {
    _debounce?.cancel();
    _tick?.cancel();
    _sse.dispose();
    super.dispose();
  }

  int get _nowSecs =>
      DateTime.now().millisecondsSinceEpoch ~/ 1000 + _clockOffset;

  void _apply(PresenceSnapshot s) {
    if (s.now > 0) {
      _clockOffset = s.now - DateTime.now().millisecondsSinceEpoch ~/ 1000;
    }
    _snap = s;
    _unavailable = false;
    _error = null;
    // Drop a stale banner when its approval is gone.
    if (_bannerId != null && !s.approvals.any((a) => a.id == _bannerId)) {
      _clearBanner();
    }
  }

  Future<void> _refresh() async {
    if (mounted) setState(() => _loading = true);
    try {
      final s = await widget.api.getPresenceSnapshot();
      if (!mounted) return;
      setState(() => _apply(s));
    } on PresenceUnavailable {
      if (mounted) setState(() => _unavailable = true);
    } catch (e) {
      if (mounted) setState(() => _error = 'failed to load: $e');
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  void _scheduleRefresh() {
    _debounce?.cancel();
    _debounce = Timer(const Duration(milliseconds: 300), () {
      if (mounted) _refresh();
    });
  }

  Future<void> _startSse() async {
    final url = await widget.api.getDaemonUrl();
    _sse.start(
      daemonUrl: url,
      onEvent: _onEvent,
      onAuthExpired: () {
        widget.api.clearAuth();
        widget.onAuthExpired();
      },
    );
  }

  void _onEvent(String raw) {
    Map<String, dynamic> data;
    try {
      final d = jsonDecode(raw);
      if (d is! Map) return;
      data = Map<String, dynamic>.from(d);
    } catch (_) {
      return;
    }
    switch (data['event']) {
      case 'presence.updated':
      case 'presence.persona':
        _scheduleRefresh();
      case 'approval.resolved':
        final id = data['id']?.toString();
        if (id != null && id == _bannerId) _clearBanner();
        _scheduleRefresh();
      case 'approval.requested':
        // The event carries only the id (/events is shared by every owner);
        // the call itself comes from this owner's own snapshot.
        final id = data['id']?.toString();
        if (id != null) _showBannerFor(id);
        _scheduleRefresh();
    }
  }

  Future<void> _showBannerFor(String id) async {
    try {
      final s = await widget.api.getPresenceSnapshot();
      if (!mounted) return;
      setState(() => _apply(s));
      final match = s.approvals.where((a) => a.id == id);
      if (match.isNotEmpty) _showBanner(match.first);
    } catch (_) {
      // Not this owner's approval, or the daemon is gone: no banner.
    }
  }

  void _clearBanner() {
    _bannerId = null;
    if (mounted) ScaffoldMessenger.of(context).clearMaterialBanners();
  }

  void _showBanner(PresenceAlert a) {
    final m = ScaffoldMessenger.of(context);
    m.clearMaterialBanners();
    _bannerId = a.id;
    final what = [a.tool, a.det].whereType<String>().join(' · ');
    m.showMaterialBanner(MaterialBanner(
      backgroundColor: BColors.panel,
      content: Text('Approval needed${what.isEmpty ? '' : ': $what'}',
          style: BType.mono(size: 12)),
      actions: [
        TextButton(
          onPressed: () => _decide(a.id, 'deny'),
          child: Text('DENY', style: BType.pixel(size: 9, color: BColors.danger)),
        ),
        TextButton(
          onPressed: () => _decide(a.id, 'approve'),
          child: Text('APPROVE', style: BType.pixel(size: 9, color: BColors.ok)),
        ),
      ],
    ));
  }

  Future<void> _decide(String id, String action) async {
    if (_acting.contains(id)) return;
    if (id == _bannerId) _clearBanner();
    setState(() => _acting.add(id));
    try {
      final s = await widget.api.presenceAction({'action': action, 'id': id});
      if (mounted) setState(() => _apply(s));
    } on PresenceUnavailable {
      if (mounted) setState(() => _unavailable = true);
    } catch (e) {
      if (mounted) {
        ScaffoldMessenger.of(context)
            .showSnackBar(SnackBar(content: Text('Action failed: $e')));
        _scheduleRefresh();
      }
    } finally {
      if (mounted) setState(() => _acting.remove(id));
    }
  }

  @override
  Widget build(BuildContext context) {
    return Column(
      children: [
        HudHeader(
          tag: 'PRESENCE',
          trailing: GestureDetector(
            onTap: _loading ? null : _refresh,
            child: Text('⟳', style: BType.pixel(size: 14, color: BColors.system)),
          ),
        ),
        Expanded(
          child: RefreshIndicator(
            color: BColors.system,
            onRefresh: _refresh,
            child: ListView(
              physics: const AlwaysScrollableScrollPhysics(),
              padding: const EdgeInsets.fromLTRB(14, 16, 14, 16),
              children: _body(),
            ),
          ),
        ),
      ],
    );
  }

  List<Widget> _body() {
    final s = _snap;
    if (_unavailable) {
      return [
        _Panel(
          accent: BColors.danger,
          title: 'UNAVAILABLE',
          child: Text(
            'Presence is not available on this daemon. It may be an older '
            'version, or presence is disabled ([presence] enabled = false).',
            style: BType.mono(size: 12, color: BColors.muted),
          ),
        ),
      ];
    }
    if (s == null) {
      return [
        _Panel(
          accent: BColors.system,
          title: 'PRESENCE',
          child: Text(_error ?? (_loading ? 'loading…' : 'no data.'),
              style: BType.mono(size: 12, color: BColors.muted)),
        ),
      ];
    }
    final approvals = s.approvals;
    final others = s.alerts.where((a) => a.kind != 'approval').toList();
    final names = {for (final x in s.sessions) x.id: x.name};
    return [
      if (_error != null) ...[
        Text(_error!, style: BType.mono(size: 11, color: BColors.danger)),
        const SizedBox(height: 12),
      ],
      _Panel(
        accent: BColors.monarch,
        title: 'PERSONA',
        value: s.host,
        child: Text(s.persona ?? 'default',
            style: BType.pixel(size: 14, color: BColors.text)),
      ),
      for (final a in approvals) ...[
        const SizedBox(height: 16),
        _ApprovalCard(
          alert: a,
          session: names[a.sid],
          remaining: a.exp == null ? null : a.exp! - _nowSecs,
          busy: _acting.contains(a.id),
          onApprove: () => _decide(a.id, 'approve'),
          onDeny: () => _decide(a.id, 'deny'),
        ),
      ],
      for (final a in others) ...[
        const SizedBox(height: 16),
        _AlertCard(alert: a, session: names[a.sid]),
      ],
      const SizedBox(height: 16),
      _Panel(
        accent: BColors.system,
        title: 'SESSIONS',
        value: '${s.sessions.length + s.more}',
        child: s.sessions.isEmpty
            ? Text('no active sessions.',
                style: BType.mono(size: 12, color: BColors.muted))
            : Column(
                children: [
                  for (final x in s.sessions) _SessionRow(session: x),
                  if (s.more > 0)
                    Padding(
                      padding: const EdgeInsets.only(top: 6),
                      child: Text('+${s.more} more',
                          style: BType.mono(size: 11, color: BColors.muted)),
                    ),
                ],
              ),
      ),
      if (s.note != null) ...[
        const SizedBox(height: 16),
        _Panel(
          accent: BColors.arise,
          title: 'NOTE',
          child: Text(s.note!, style: BType.mono(size: 12)),
        ),
      ],
    ];
  }
}

Color _statusColor(String st) {
  switch (st) {
    case 'working':
      return BColors.system;
    case 'waiting':
      return BColors.arise;
    case 'done':
      return BColors.ok;
    case 'error':
      return BColors.danger;
    default:
      return BColors.muted; // idle / unknown
  }
}

String _fmtRemaining(int secs) {
  if (secs <= 0) return 'expired';
  final m = secs ~/ 60;
  final s = secs % 60;
  return m > 0 ? '${m}m ${s.toString().padLeft(2, '0')}s' : '${s}s';
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

class _SessionRow extends StatelessWidget {
  final PresenceSession session;
  const _SessionRow({required this.session});

  @override
  Widget build(BuildContext context) {
    final color = _statusColor(session.st);
    final detail = [session.tool, session.det].whereType<String>().join(' · ');
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 7),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Padding(
            padding: const EdgeInsets.only(top: 4, right: 10),
            child: Container(
              width: 8,
              height: 8,
              decoration: BoxDecoration(color: color, shape: BoxShape.circle),
            ),
          ),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(session.name.isEmpty ? session.id : session.name,
                    style: BType.mono(size: 12)),
                if (detail.isNotEmpty)
                  Text(detail,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: BType.mono(size: 11, color: BColors.muted)),
              ],
            ),
          ),
          const SizedBox(width: 8),
          Text(session.st.toUpperCase(),
              style: BType.pixel(size: 7, color: color, spacing: 1)),
        ],
      ),
    );
  }
}

class _ApprovalCard extends StatelessWidget {
  final PresenceAlert alert;
  final String? session;
  final int? remaining;
  final bool busy;
  final VoidCallback onApprove;
  final VoidCallback onDeny;
  const _ApprovalCard({
    required this.alert,
    required this.session,
    required this.remaining,
    required this.busy,
    required this.onApprove,
    required this.onDeny,
  });

  @override
  Widget build(BuildContext context) {
    final expired = remaining != null && remaining! <= 0;
    return _Panel(
      accent: BColors.arise,
      title: 'APPROVAL',
      value: remaining == null ? null : _fmtRemaining(remaining!),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (session != null && session!.isNotEmpty)
            Text(session!, style: BType.mono(size: 11, color: BColors.muted)),
          Text(alert.tool ?? 'tool', style: BType.pixel(size: 12)),
          if (alert.det != null) ...[
            const SizedBox(height: 6),
            Text(alert.det!, style: BType.mono(size: 13)),
          ],
          const SizedBox(height: 12),
          if (busy)
            const SizedBox(
                width: 16, height: 16, child: CircularProgressIndicator(strokeWidth: 2))
          else
            Row(
              children: [
                Expanded(
                    child: _ActionButton(
                        label: 'APPROVE',
                        color: BColors.ok,
                        onTap: expired ? null : onApprove)),
                const SizedBox(width: 10),
                Expanded(
                    child: _ActionButton(
                        label: 'DENY',
                        color: BColors.danger,
                        onTap: expired ? null : onDeny)),
              ],
            ),
        ],
      ),
    );
  }
}

class _ActionButton extends StatelessWidget {
  final String label;
  final Color color;
  final VoidCallback? onTap;
  const _ActionButton({required this.label, required this.color, required this.onTap});

  @override
  Widget build(BuildContext context) {
    final c = onTap == null ? BColors.muted : color;
    return GestureDetector(
      onTap: onTap,
      child: SystemSurface(
        accent: c,
        cut: 6,
        padding: const EdgeInsets.symmetric(vertical: 11),
        child: Center(
          child: Text(label, style: BType.pixel(size: 9, color: c, spacing: 1.5)),
        ),
      ),
    );
  }
}

class _AlertCard extends StatelessWidget {
  final PresenceAlert alert;
  final String? session;
  const _AlertCard({required this.alert, required this.session});

  @override
  Widget build(BuildContext context) {
    final isError = alert.kind == 'error';
    final accent = isError ? BColors.danger : BColors.arise;
    final title = isError ? 'ERROR' : 'WAITING AT THE COMPUTER';
    final body = isError
        ? 'A turn ended with an error.'
        : 'The agent is asking a question and waits for you at the computer.';
    return _Panel(
      accent: accent,
      title: title,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (session != null && session!.isNotEmpty)
            Text(session!, style: BType.mono(size: 11, color: BColors.muted)),
          Text(body, style: BType.mono(size: 12, color: isError ? BColors.danger : BColors.text)),
          if (alert.det != null) ...[
            const SizedBox(height: 4),
            Text(alert.det!, style: BType.mono(size: 11, color: BColors.muted)),
          ],
        ],
      ),
    );
  }
}
