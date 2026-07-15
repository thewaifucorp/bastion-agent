"""Shared fixtures for onboarding/TOTP tests."""

from __future__ import annotations

import pyotp
import pytest

from totp import SessionManager


@pytest.fixture
def totp_secret() -> str:
    """A fresh TOTP secret for each test."""
    return pyotp.random_base32()


@pytest.fixture
def session_manager(monkeypatch: pytest.MonkeyPatch, totp_secret: str) -> SessionManager:
    """SessionManager with a known secret and default config (TTL=8h, max=3)."""
    monkeypatch.setenv("BASTION_TOTP_SECRET", totp_secret)
    monkeypatch.delenv("BASTION_SESSION_TTL_HOURS", raising=False)
    monkeypatch.delenv("BASTION_MAX_AUTH_ATTEMPTS", raising=False)
    return SessionManager()


@pytest.fixture
def valid_code(totp_secret: str) -> str:
    """The current valid TOTP code for the test secret."""
    return pyotp.TOTP(totp_secret).now()
