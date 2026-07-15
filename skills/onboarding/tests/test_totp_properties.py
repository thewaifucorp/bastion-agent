"""
Property-based tests for the TOTP authentication module.

**Validates: Requirements 8.1, 8.3, 8.5**

Properties tested:
  - Property 19: Nova sessão sempre solicita TOTP antes de processar mensagens (Req 8.1)
  - Property 20: Código TOTP inválido decrementa contador de tentativas (Req 8.3)
  - Property 21: Sessão expirada solicita novo TOTP (Req 8.5)
"""

from __future__ import annotations

import os
from contextlib import contextmanager
from datetime import datetime, timedelta, timezone
from unittest.mock import patch

import pyotp
import pytest
from hypothesis import HealthCheck, given, settings
from hypothesis import strategies as st
from hypothesis import assume

from totp import SessionManager, generate_qr_uri, generate_secret, verify_code

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


@contextmanager
def _env(secret: str, ttl_hours: int | None = None, max_attempts: int | None = None):
    """Context manager that sets TOTP-related env vars and restores them."""
    overrides: dict[str, str] = {"BASTION_TOTP_SECRET": secret}
    if ttl_hours is not None:
        overrides["BASTION_SESSION_TTL_HOURS"] = str(ttl_hours)
    if max_attempts is not None:
        overrides["BASTION_MAX_AUTH_ATTEMPTS"] = str(max_attempts)

    old = {k: os.environ.get(k) for k in overrides}
    try:
        os.environ.update(overrides)
        yield
    finally:
        for k, v in old.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v


def _make_manager(secret: str, ttl_hours: int | None = None, max_attempts: int | None = None) -> SessionManager:
    """Create a SessionManager with the given env config."""
    with _env(secret, ttl_hours, max_attempts):
        return SessionManager()


# ---------------------------------------------------------------------------
# Hypothesis strategies
# ---------------------------------------------------------------------------

_user_id = st.text(
    alphabet="abcdefghijklmnopqrstuvwxyz0123456789_-",
    min_size=1,
    max_size=40,
)

_ttl_hours = st.integers(min_value=1, max_value=72)
_max_attempts = st.integers(min_value=1, max_value=10)


# ---------------------------------------------------------------------------
# Unit tests — generate_secret, generate_qr_uri, verify_code
# ---------------------------------------------------------------------------


def test_generate_secret_returns_valid_base32() -> None:
    """generate_secret() must return a non-empty Base32 string usable by pyotp."""
    secret = generate_secret()
    assert isinstance(secret, str)
    assert len(secret) > 0
    totp = pyotp.TOTP(secret)
    code = totp.now()
    assert len(code) == 6
    assert code.isdigit()


def test_generate_secret_produces_unique_values() -> None:
    """Each call to generate_secret() must return a different secret."""
    secrets = {generate_secret() for _ in range(20)}
    assert len(secrets) == 20, "generate_secret() returned duplicate values"


def test_generate_qr_uri_contains_required_parts() -> None:
    """generate_qr_uri() must return a valid otpauth:// URI."""
    secret = pyotp.random_base32()
    uri = generate_qr_uri(secret, "testuser")
    assert uri.startswith("otpauth://totp/")
    assert secret in uri
    assert "testuser" in uri
    assert "Bastion" in uri


def test_verify_code_accepts_current_code() -> None:
    """verify_code() must return True for the current valid TOTP code."""
    secret = pyotp.random_base32()
    code = pyotp.TOTP(secret).now()
    assert verify_code(secret, code) is True


def test_verify_code_rejects_wrong_code() -> None:
    """verify_code() must return False for a code from a different secret."""
    secret = pyotp.random_base32()
    other_secret = pyotp.random_base32()
    wrong_code = pyotp.TOTP(other_secret).now()
    current_code = pyotp.TOTP(secret).now()
    if wrong_code != current_code:
        assert verify_code(secret, wrong_code) is False


# ---------------------------------------------------------------------------
# Property 19 — Nova sessão sempre solicita TOTP antes de processar mensagens
# Validates: Requirements 8.1
# ---------------------------------------------------------------------------


@given(user_id=_user_id)
@settings(max_examples=100)
def test_property19_new_session_is_not_authenticated(user_id: str) -> None:
    """
    **Property 19: Nova sessão sempre solicita TOTP antes de processar mensagens**

    For any new session started via start_session(), is_authenticated() must
    return False — the session is unauthenticated until a valid TOTP code is
    provided.

    **Validates: Requirements 8.1**
    """
    manager = SessionManager()
    manager.start_session(user_id)

    assert manager.is_authenticated(user_id) is False, (
        f"New session for user_id={user_id!r} must not be authenticated"
    )


@given(user_id=_user_id)
@settings(max_examples=100)
def test_property19_no_session_is_not_authenticated(user_id: str) -> None:
    """
    **Property 19 (no-session variant)**

    is_authenticated() must return False for any user_id that has never had
    a session started — the system must not grant access by default.

    **Validates: Requirements 8.1**
    """
    manager = SessionManager()
    assert manager.is_authenticated(user_id) is False


@given(user_id=_user_id)
@settings(max_examples=50)
def test_property19_reset_session_requires_new_totp(user_id: str) -> None:
    """
    **Property 19 (reset variant)**

    Calling start_session() on an already-authenticated session must reset
    it to unauthenticated, requiring a new TOTP challenge.

    **Validates: Requirements 8.1**
    """
    secret = pyotp.random_base32()
    with _env(secret):
        manager = SessionManager()
        manager.start_session(user_id)
        code = pyotp.TOTP(secret).now()
        manager.authenticate(user_id, code)
        assert manager.is_authenticated(user_id) is True

        # Reset — must become unauthenticated again
        manager.start_session(user_id)
        assert manager.is_authenticated(user_id) is False, (
            "After start_session() reset, session must not be authenticated"
        )


# ---------------------------------------------------------------------------
# Property 20 — Código TOTP inválido decrementa contador de tentativas
# Validates: Requirements 8.3
# ---------------------------------------------------------------------------


@given(
    user_id=_user_id,
    max_attempts=_max_attempts,
)
@settings(max_examples=100)
def test_property20_invalid_code_decrements_attempts(
    user_id: str,
    max_attempts: int,
) -> None:
    """
    **Property 20: Código TOTP inválido decrementa contador de tentativas**

    For any invalid TOTP code provided, the system must reject the
    authentication and decrement attempts_remaining by exactly 1.

    **Validates: Requirements 8.3**
    """
    secret = pyotp.random_base32()
    other_secret = pyotp.random_base32()
    wrong_code = pyotp.TOTP(other_secret).now()
    current_valid = pyotp.TOTP(secret).now()
    assume(wrong_code != current_valid)

    with _env(secret, max_attempts=max_attempts):
        manager = SessionManager()
        manager.start_session(user_id)

        assert manager.attempts_remaining(user_id) == max_attempts

        result = manager.authenticate(user_id, wrong_code)

        assert result is False, "Invalid code must return False"
        assert manager.attempts_remaining(user_id) == max_attempts - 1, (
            f"attempts_remaining should be {max_attempts - 1} after one invalid attempt, "
            f"got {manager.attempts_remaining(user_id)}"
        )
        assert manager.is_authenticated(user_id) is False


@given(
    user_id=_user_id,
    max_attempts=st.integers(min_value=2, max_value=8),
)
@settings(max_examples=50)
def test_property20_multiple_invalid_codes_decrement_sequentially(
    user_id: str,
    max_attempts: int,
) -> None:
    """
    **Property 20 (sequential variant)**

    Each consecutive invalid code must decrement attempts_remaining by 1,
    until the session is locked at 0.

    **Validates: Requirements 8.3**
    """
    secret = pyotp.random_base32()

    with _env(secret, max_attempts=max_attempts):
        manager = SessionManager()
        manager.start_session(user_id)

        for i in range(max_attempts - 1):
            other_secret = pyotp.random_base32()
            wrong_code = pyotp.TOTP(other_secret).now()
            current_valid = pyotp.TOTP(secret).now()
            assume(wrong_code != current_valid)

            manager.authenticate(user_id, wrong_code)
            expected_remaining = max_attempts - (i + 1)
            assert manager.attempts_remaining(user_id) == expected_remaining, (
                f"After {i + 1} invalid attempts, expected {expected_remaining} remaining, "
                f"got {manager.attempts_remaining(user_id)}"
            )


@given(
    user_id=_user_id,
    max_attempts=_max_attempts,
)
@settings(max_examples=50)
def test_property20_session_locked_after_max_attempts(
    user_id: str,
    max_attempts: int,
) -> None:
    """
    **Property 20 (lockout variant)**

    After max_attempts invalid codes, the session must be locked and further
    authenticate() calls must raise RuntimeError.

    **Validates: Requirements 8.3, 8.4**
    """
    secret = pyotp.random_base32()

    with _env(secret, max_attempts=max_attempts):
        manager = SessionManager()
        manager.start_session(user_id)

        for _ in range(max_attempts):
            other_secret = pyotp.random_base32()
            wrong_code = pyotp.TOTP(other_secret).now()
            current_valid = pyotp.TOTP(secret).now()
            assume(wrong_code != current_valid)
            try:
                manager.authenticate(user_id, wrong_code)
            except RuntimeError:
                break

        assert manager.is_locked(user_id) is True, (
            "Session must be locked after exhausting all attempts"
        )
        assert manager.is_authenticated(user_id) is False

        with pytest.raises(RuntimeError):
            manager.authenticate(user_id, "123456")


# ---------------------------------------------------------------------------
# Property 21 — Sessão expirada solicita novo TOTP
# Validates: Requirements 8.5
# ---------------------------------------------------------------------------


@given(
    user_id=_user_id,
    ttl_hours=_ttl_hours,
)
@settings(max_examples=100)
def test_property21_expired_session_is_not_authenticated(
    user_id: str,
    ttl_hours: int,
) -> None:
    """
    **Property 21: Sessão expirada solicita novo TOTP**

    For any session whose TTL has expired, is_authenticated() must return
    False — the next message must trigger a new TOTP challenge.

    **Validates: Requirements 8.5**
    """
    secret = pyotp.random_base32()

    with _env(secret, ttl_hours=ttl_hours):
        manager = SessionManager()
        manager.start_session(user_id)

        code = pyotp.TOTP(secret).now()
        result = manager.authenticate(user_id, code)
        assert result is True
        assert manager.is_authenticated(user_id) is True

    # Simulate time past TTL by patching datetime.now inside the totp module
    future = datetime.now(tz=timezone.utc) + timedelta(hours=ttl_hours + 1)
    with patch("totp.datetime") as mock_dt:
        mock_dt.now.return_value = future
        assert manager.is_authenticated(user_id) is False, (
            f"Session with TTL={ttl_hours}h must not be authenticated after "
            f"{ttl_hours + 1}h have passed"
        )


@given(
    user_id=_user_id,
    ttl_hours=_ttl_hours,
)
@settings(max_examples=50)
def test_property21_session_valid_within_ttl(
    user_id: str,
    ttl_hours: int,
) -> None:
    """
    **Property 21 (within-TTL variant)**

    A session authenticated with TTL=N hours must still be valid at N-1 hours.

    **Validates: Requirements 8.5**
    """
    secret = pyotp.random_base32()

    with _env(secret, ttl_hours=ttl_hours):
        manager = SessionManager()
        manager.start_session(user_id)
        code = pyotp.TOTP(secret).now()
        manager.authenticate(user_id, code)

    near_expiry = datetime.now(tz=timezone.utc) + timedelta(hours=ttl_hours - 1)
    with patch("totp.datetime") as mock_dt:
        mock_dt.now.return_value = near_expiry
        assert manager.is_authenticated(user_id) is True, (
            f"Session must still be valid at {ttl_hours - 1}h (TTL={ttl_hours}h)"
        )


@given(user_id=_user_id)
@settings(max_examples=50)
def test_property21_successful_auth_sets_ttl(user_id: str) -> None:
    """
    **Property 21 (TTL set on auth variant)**

    After a successful authentication, is_authenticated() must return True
    immediately (the TTL window is open).

    **Validates: Requirements 8.2, 8.5**
    """
    secret = pyotp.random_base32()

    with _env(secret):
        manager = SessionManager()
        manager.start_session(user_id)

        assert manager.is_authenticated(user_id) is False

        code = pyotp.TOTP(secret).now()
        result = manager.authenticate(user_id, code)

        assert result is True
        assert manager.is_authenticated(user_id) is True
