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
