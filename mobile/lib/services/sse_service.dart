// SseService: subscribes to GET /events SSE stream.
// Security: x-bastion-token header required (Pitfall 5 — same auth as /webhook).
// Reconnect: exponential backoff; 401 → triggers re-pairing callback (Pitfall 7).

import 'dart:async';
import 'dart:math';
import 'package:flutter_http_sse/client/sse_client.dart';
import 'package:flutter_http_sse/model/sse_request.dart';
import 'package:flutter_http_sse/model/sse_response.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';

typedef OnEvent = void Function(String event);
typedef OnAuthExpired = void Function();

class SseService {
  static const _jwtKey = 'bastion_jwt';
  static const _connectionId = 'bastion_events';

  final FlutterSecureStorage _storage = const FlutterSecureStorage();
  final SSEClient _sseClient = SSEClient();

  int _retryCount = 0;
  bool _disposed = false;
  Timer? _retryTimer;

  /// Start listening to /events SSE.
  /// [onEvent] receives raw JSON event strings from SEAM #4 OTel broadcast.
  /// [onAuthExpired] called on 401 — UI should show pairing screen.
  Future<void> start({
    required String daemonUrl,
    required OnEvent onEvent,
    required OnAuthExpired onAuthExpired,
  }) async {
    await _connect(daemonUrl, onEvent, onAuthExpired);
  }

  Future<void> _connect(String daemonUrl, OnEvent onEvent, OnAuthExpired onAuthExpired) async {
    if (_disposed) return;

    final jwt = await _storage.read(key: _jwtKey);
    if (jwt == null) {
      onAuthExpired();
      return;
    }

    try {
      final request = SSERequest(
        url: '$daemonUrl/events',
        headers: {
          'x-bastion-token': jwt,
          'Accept': 'text/event-stream',
        },
        onData: (SSEResponse response) {
          _retryCount = 0; // reset backoff on successful event
          final data = response.data;
          if (data != null) {
            onEvent(data is String ? data : data.toString());
          }
        },
        onError: (String error) {
          // 401 check: if error message contains 401, trigger re-pair
          if (error.contains('401')) {
            onAuthExpired();
            return;
          }
          _scheduleReconnect(daemonUrl, onEvent, onAuthExpired);
        },
        onDone: () {
          _scheduleReconnect(daemonUrl, onEvent, onAuthExpired);
        },
        retry: false, // we manage retry ourselves for 401 handling
      );

      _sseClient.connect(_connectionId, request);
    } catch (e) {
      // 401 check: if error message contains 401, trigger re-pair
      if (e.toString().contains('401')) {
        onAuthExpired();
        return;
      }
      _scheduleReconnect(daemonUrl, onEvent, onAuthExpired);
    }
  }

  void _scheduleReconnect(String daemonUrl, OnEvent onEvent, OnAuthExpired onAuthExpired) {
    if (_disposed) return;
    // Exponential backoff: 1s, 2s, 4s, 8s, max 60s
    final delay = Duration(seconds: min(1 << _retryCount, 60));
    _retryCount++;
    _retryTimer = Timer(delay, () => _connect(daemonUrl, onEvent, onAuthExpired));
  }

  void dispose() {
    _disposed = true;
    _retryTimer?.cancel();
    _sseClient.close(connectionId: _connectionId);
  }
}
