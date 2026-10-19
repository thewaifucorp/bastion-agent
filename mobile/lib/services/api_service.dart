// ApiService: Dio-based HTTP client.
// All requests inject x-bastion-token from flutter_secure_storage.
// Pairing: /auth/exchange OTC → JWT flow (Plan 01 route).

import 'package:dio/dio.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';

class ApiService {
  static const _jwtKey = 'bastion_jwt';
  static const _daemonUrlKey = 'bastion_daemon_url';

  final FlutterSecureStorage _storage = const FlutterSecureStorage();
  late final Dio _dio;

  ApiService() {
    _dio = Dio();
    _dio.interceptors.add(InterceptorsWrapper(
      onRequest: (options, handler) async {
        // Inject JWT header on every request
        final jwt = await _storage.read(key: _jwtKey);
        if (jwt != null) {
          options.headers['x-bastion-token'] = jwt;
        }
        handler.next(options);
      },
    ));
  }

  Future<String> getDaemonUrl() async {
    return await _storage.read(key: _daemonUrlKey) ?? 'http://localhost:8080';
  }

  /// Exchange one-time token from /connect-app for a JWT via POST /auth/exchange.
  /// Stores JWT in flutter_secure_storage on success.
  Future<void> pair(String daemonUrl, String otc) async {
    await _storage.write(key: _daemonUrlKey, value: daemonUrl);
    final resp = await _dio.post(
      '$daemonUrl/auth/exchange',
      data: {'otc': otc},
    );
    final jwt = resp.data['jwt'] as String;
    await _storage.write(key: _jwtKey, value: jwt);
  }

  /// Send a user message to the daemon.
  /// Contract: POST /webhook body = {'text': message}; daemon Out = {'reply': String}. (CR-05)
  Future<String> sendMessage(String message) async {
    final url = await getDaemonUrl();
    final resp = await _dio.post(
      '$url/webhook',
      data: {'text': message},
    );
    return resp.data['reply'] as String? ?? '';
  }

  /// GET /presence/snapshot. Throws [PresenceUnavailable] on 404
  /// (daemon older than presence, or presence disabled).
  Future<PresenceSnapshot> getPresenceSnapshot() async {
    final url = await getDaemonUrl();
    try {
      final resp = await _dio.get('$url/presence/snapshot');
      return PresenceSnapshot.fromJson(_asMap(resp.data));
    } on DioException catch (e) {
      throw _presenceError(e);
    }
  }

  /// POST /presence/action (approve, deny, persona, care, note).
  /// Returns the updated snapshot.
  Future<PresenceSnapshot> presenceAction(Map<String, dynamic> action) async {
    final url = await getDaemonUrl();
    try {
      final resp = await _dio.post('$url/presence/action', data: action);
      return PresenceSnapshot.fromJson(_asMap(resp.data));
    } on DioException catch (e) {
      throw _presenceError(e);
    }
  }

  static Map<String, dynamic> _asMap(dynamic data) =>
      data is Map ? Map<String, dynamic>.from(data) : <String, dynamic>{};

  static Object _presenceError(DioException e) =>
      e.response?.statusCode == 404 ? const PresenceUnavailable() : e;

  /// Check if JWT is present (app is paired).
  Future<bool> isPaired() async {
    final jwt = await _storage.read(key: _jwtKey);
    return jwt != null && jwt.isNotEmpty;
  }

  /// Clear JWT (force re-pairing).
  Future<void> clearAuth() async {
    await _storage.delete(key: _jwtKey);
  }
}

/// Raised when /presence/* answers 404: the daemon is older than presence,
/// or presence is disabled in its config.
class PresenceUnavailable implements Exception {
  const PresenceUnavailable();
  @override
  String toString() => 'presence unavailable';
}

int _asInt(dynamic v, [int fallback = 0]) =>
    v is num ? v.toInt() : (int.tryParse('${v ?? ''}') ?? fallback);

String? _asStr(dynamic v) {
  if (v == null) return null;
  final s = v.toString();
  return s.isEmpty ? null : s;
}

/// One host-agent session (st: working | waiting | idle | done | error).
class PresenceSession {
  final String id;
  final String? src;
  final String name;
  final String st;
  final String? tool;
  final String? det;
  final int? since;
  final String? model;

  const PresenceSession({
    required this.id,
    this.src,
    this.name = '',
    this.st = 'idle',
    this.tool,
    this.det,
    this.since,
    this.model,
  });

  factory PresenceSession.fromJson(Map<String, dynamic> j) => PresenceSession(
        id: _asStr(j['id']) ?? '',
        src: _asStr(j['src']),
        name: _asStr(j['name']) ?? '',
        st: _asStr(j['st']) ?? 'idle',
        tool: _asStr(j['tool']),
        det: _asStr(j['det']),
        since: j['since'] == null ? null : _asInt(j['since']),
        model: _asStr(j['model']),
      );
}

/// An alert: kind is approval | question | error. `exp` is Unix seconds.
class PresenceAlert {
  final String id;
  final String kind;
  final String? sid;
  final String? tool;
  final String? det;
  final int? exp;

  /// Approvals: the call's whole main input (owner snapshots only).
  final String? input;

  /// Approvals: whether [input] is the whole call; approve only when true.
  final bool inputComplete;

  const PresenceAlert({
    required this.id,
    required this.kind,
    this.sid,
    this.tool,
    this.det,
    this.exp,
    this.input,
    this.inputComplete = false,
  });

  factory PresenceAlert.fromJson(Map<String, dynamic> j) => PresenceAlert(
        id: _asStr(j['id']) ?? '',
        kind: _asStr(j['kind']) ?? '',
        sid: _asStr(j['sid']),
        tool: _asStr(j['tool']),
        det: _asStr(j['det']),
        exp: j['exp'] == null ? null : _asInt(j['exp']),
        input: _asStr(j['input']),
        inputComplete: j['input_complete'] == true,
      );
}

/// Companion state. Needs are 0-100 (water, food, play, rest).
class PresencePet {
  final int level;
  final int xp;
  final Map<String, int> needs;
  final String? cue;

  const PresencePet({this.level = 0, this.xp = 0, this.needs = const {}, this.cue});

  factory PresencePet.fromJson(Map<String, dynamic> j) {
    final n = j['needs'];
    return PresencePet(
      level: _asInt(j['level']),
      xp: _asInt(j['xp']),
      needs: n is Map
          ? {for (final e in n.entries) e.key.toString(): _asInt(e.value)}
          : const {},
      cue: _asStr(j['cue']),
    );
  }
}

/// GET /presence/snapshot. Omitted fields are absent (null / empty).
class PresenceSnapshot {
  final int v;
  final int seq;
  final int now;
  final String? host;
  final String? persona;
  final PresencePet? pet;
  final List<PresenceSession> sessions;
  final int more;
  final List<PresenceAlert> alerts;
  final String? note;

  const PresenceSnapshot({
    this.v = 1,
    this.seq = 0,
    this.now = 0,
    this.host,
    this.persona,
    this.pet,
    this.sessions = const [],
    this.more = 0,
    this.alerts = const [],
    this.note,
  });

  factory PresenceSnapshot.fromJson(Map<String, dynamic> j) {
    List<T> list<T>(dynamic raw, T Function(Map<String, dynamic>) f) => raw is List
        ? raw.whereType<Map>().map((m) => f(Map<String, dynamic>.from(m))).toList()
        : <T>[];
    final pet = j['pet'];
    final note = j['note'];
    return PresenceSnapshot(
      v: _asInt(j['v'], 1),
      seq: _asInt(j['seq']),
      now: _asInt(j['now']),
      host: _asStr(j['host']),
      persona: _asStr(j['persona']),
      pet: pet is Map ? PresencePet.fromJson(Map<String, dynamic>.from(pet)) : null,
      sessions: list(j['sessions'], PresenceSession.fromJson),
      more: _asInt(j['more']),
      alerts: list(j['alerts'], PresenceAlert.fromJson),
      note: note is Map ? _asStr(note['text']) : null,
    );
  }

  List<PresenceAlert> get approvals =>
      alerts.where((a) => a.kind == 'approval').toList();
}
