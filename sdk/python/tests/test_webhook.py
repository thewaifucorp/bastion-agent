import hashlib
import hmac

from bastion_control_plane import verify_webhook_signature


def _sign(secret: str, body: bytes) -> str:
    digest = hmac.new(secret.encode("utf-8"), body, hashlib.sha256).hexdigest()
    return f"sha256={digest}"


def test_valid_signature_verifies():
    body = b'{"event_id": "e1"}'
    header = _sign("top-secret", body)
    assert verify_webhook_signature("top-secret", body, header) is True


def test_valid_signature_verifies_for_string_body():
    body = '{"event_id": "e1"}'
    header = _sign("top-secret", body.encode("utf-8"))
    assert verify_webhook_signature("top-secret", body, header) is True


def test_wrong_secret_fails():
    body = b"payload"
    header = _sign("secret-a", body)
    assert verify_webhook_signature("secret-b", body, header) is False


def test_tampered_body_fails():
    header = _sign("top-secret", b"original")
    assert verify_webhook_signature("top-secret", b"tampered", header) is False


def test_missing_header_fails():
    assert verify_webhook_signature("top-secret", b"payload", None) is False
    assert verify_webhook_signature("top-secret", b"payload", "") is False


def test_wrong_prefix_fails():
    assert verify_webhook_signature("top-secret", b"payload", "md5=deadbeef") is False


def test_malformed_hex_fails():
    assert verify_webhook_signature("top-secret", b"payload", "sha256=not-hex!!") is False
    assert verify_webhook_signature("top-secret", b"payload", "sha256=short") is False
