"""
TOTP authentication module for Bastion v2.

Provides:
- generate_secret(): generate a new TOTP secret via pyotp
- generate_qr_uri(secret, user_name): build an otpauth:// URI for QR code display
- verify_code(secret, code): validate a 6-digit TOTP code
- SessionManager: per-user session state with TTL and attempt limiting

Configuration (read from environment, with defaults):
    BASTION_SESSION_TTL_HOURS   — session lifetime after successful auth (default: 8)
    BASTION_MAX_AUTH_ATTEMPTS   — max invalid attempts before lockout (default: 3)
    BASTION_TOTP_SECRET         — the shared TOTP secret (never stored in files)
"""

from __future__ import annotations

import logging
import os
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from typing import Protocol, runtime_checkable

import pyotp

logger = logging.getLogger(__name__)

# ---------------------------------------------------------------------------
# Environment-driven configuration
# ---------------------------------------------------------------------------

_DEFAULT_SESSION_TTL_HOURS: int = 8
_DEFAULT_MAX_AUTH_ATTEMPTS: int = 3

ISSUER_NAME: str = "Bastion"


def _get_session_ttl_hours() -> int:
    """Read BASTION_SESSION_TTL_HOURS from env, falling back to 8."""
    raw = os.environ.get("BASTION_SESSION_TTL_HOURS", "")
    try:
        return int(raw) if raw.strip() else _DEFAULT_SESSION_TTL_HOURS
    except ValueError:
        logger.warning(
            "Invalid BASTION_SESSION_TTL_HOURS=%r — using default %d",
            raw,
            _DEFAULT_SESSION_TTL_HOURS,
        )
        return _DEFAULT_SESSION_TTL_HOURS


def _get_max_auth_attempts() -> int:
    """Read BASTION_MAX_AUTH_ATTEMPTS from env, falling back to 3."""
    raw = os.environ.get("BASTION_MAX_AUTH_ATTEMPTS", "")
    try:
        return int(raw) if raw.strip() else _DEFAULT_MAX_AUTH_ATTEMPTS
    except ValueError:
        logger.warning(
            "Invalid BASTION_MAX_AUTH_ATTEMPTS=%r — using default %d",
            raw,
            _DEFAULT_MAX_AUTH_ATTEMPTS,
        )
        return _DEFAULT_MAX_AUTH_ATTEMPTS


# ---------------------------------------------------------------------------
# TOTP helpers
# ---------------------------------------------------------------------------


def generate_secret() -> str:
    """
    Generate a new random TOTP secret encoded in Base32.

    The secret is suitable for use with Authy, Google Authenticator, and any
    RFC 6238-compliant TOTP app.  It must be stored exclusively via the
    BASTION_TOTP_SECRET environment variable — never in a versioned file.

    Returns:
        A Base32-encoded secret string (e.g. "JBSWY3DPEHPK3PXP").
    """
    secret = pyotp.random_base32()
    logger.info("New TOTP secret generated (store in BASTION_TOTP_SECRET)")
    return secret


def generate_qr_uri(secret: str, user_name: str) -> str:
    """
    Build an otpauth:// URI that can be encoded as a QR code.

    The URI follows the Key URI Format used by Authy and Google Authenticator:
        otpauth://totp/{issuer}:{user_name}?secret={secret}&issuer={issuer}

    Args:
        secret:    The Base32-encoded TOTP secret.
        user_name: The account label shown in the authenticator app.

    Returns:
        A fully-formed otpauth:// URI string.
    """
    totp = pyotp.TOTP(secret)
    uri = totp.provisioning_uri(name=user_name, issuer_name=ISSUER_NAME)
    logger.debug("QR URI generated for user=%r", user_name)
    return uri


def verify_code(secret: str, code: str) -> bool:
    """
    Validate a 6-digit TOTP code against the given secret.

    Uses pyotp's built-in time-window tolerance (±1 step / ±30 s) to
    account for minor clock drift between the server and the user's device.

    Args:
        secret: The Base32-encoded TOTP secret.
        code:   The 6-digit code entered by the user.

    Returns:
        True if the code is valid for the current time window, False otherwise.
    """
    return pyotp.TOTP(secret).verify(code)


# ---------------------------------------------------------------------------
# Session state
# ---------------------------------------------------------------------------


@dataclass
class _SessionState:
    """Internal state for a single user session."""

    authenticated_until: datetime | None = None
    attempts_remaining: int = field(default_factory=_get_max_auth_attempts)
    locked: bool = False


# ---------------------------------------------------------------------------
# SessionManager
# ---------------------------------------------------------------------------


class SessionManager:
    """
    Manages per-user TOTP authentication sessions.

    State is kept in memory as a dict keyed by user_id:
        {user_id: _SessionState}

    Configuration is read from environment variables at construction time:
        BASTION_SESSION_TTL_HOURS   — how long a successful auth lasts
        BASTION_MAX_AUTH_ATTEMPTS   — max invalid attempts before lockout

    The TOTP secret is read from BASTION_TOTP_SECRET at authentication time
    so that it is never stored in a versioned file.
    """

    def __init__(self) -> None:
        self._sessions: dict[str, _SessionState] = {}
        self._ttl_hours: int = _get_session_ttl_hours()
        self._max_attempts: int = _get_max_auth_attempts()
        logger.info(
            "SessionManager initialised: ttl=%dh max_attempts=%d",
            self._ttl_hours,
            self._max_attempts,
        )

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    def start_session(self, user_id: str) -> None:
        """
        Initialise an unauthenticated session for *user_id*.

        If a session already exists it is reset to unauthenticated state so
        that a fresh TOTP challenge is required.

        Args:
            user_id: Unique identifier for the user (e.g. Telegram user ID).
        """
        self._sessions[user_id] = _SessionState(
            authenticated_until=None,
            attempts_remaining=self._max_attempts,
            locked=False,
        )
        logger.info("Session started (unauthenticated): user_id=%r", user_id)

    def authenticate(self, user_id: str, code: str) -> bool:
        """
        Attempt to authenticate *user_id* with the given TOTP *code*.

        On success:
            - Sets authenticated_until = now + SESSION_TTL_HOURS
            - Resets attempts_remaining to the configured maximum

        On failure:
            - Decrements attempts_remaining by 1
            - If attempts_remaining reaches 0, locks the session

        The TOTP secret is read from the BASTION_TOTP_SECRET environment
        variable at call time — it is never stored in this object.

        Args:
            user_id: The user whose session to authenticate.
            code:    The 6-digit TOTP code provided by the user.

        Returns:
            True if authentication succeeded, False otherwise.

        Raises:
            KeyError:    If no session exists for *user_id* (call start_session first).
            RuntimeError: If BASTION_TOTP_SECRET is not set in the environment.
            RuntimeError: If the session is locked due to too many failed attempts.
        """
        state = self._get_state(user_id)

        if state.locked:
            logger.warning("Authentication attempt on locked session: user_id=%r", user_id)
            raise RuntimeError(
                f"Session for user_id={user_id!r} is locked after too many failed attempts."
            )

        secret = os.environ.get("BASTION_TOTP_SECRET", "")
        if not secret:
            raise RuntimeError(
                "BASTION_TOTP_SECRET is not set. "
                "Generate a secret with generate_secret() and store it in .env."
            )

        if verify_code(secret, code):
            state.authenticated_until = datetime.now(tz=timezone.utc) + timedelta(
                hours=self._ttl_hours
            )
            state.attempts_remaining = self._max_attempts
            state.locked = False
            logger.info(
                "Authentication successful: user_id=%r authenticated_until=%s",
                user_id,
                state.authenticated_until.isoformat(),
            )
            return True

        # Invalid code — decrement counter
        state.attempts_remaining -= 1
        logger.warning(
            "Invalid TOTP code: user_id=%r attempts_remaining=%d",
            user_id,
            state.attempts_remaining,
        )

        if state.attempts_remaining <= 0:
            state.locked = True
            logger.warning("Session locked after max failed attempts: user_id=%r", user_id)

        return False

    def is_authenticated(self, user_id: str) -> bool:
        """
        Check whether *user_id* has an active, non-expired authenticated session.

        Returns False (rather than raising) if no session exists for the user,
        so callers can use this as a simple gate without try/except.

        Args:
            user_id: The user to check.

        Returns:
            True if the session exists and has not expired, False otherwise.
        """
        state = self._sessions.get(user_id)
        if state is None:
            return False
        if state.authenticated_until is None:
            return False
        return datetime.now(tz=timezone.utc) < state.authenticated_until

    def attempts_remaining(self, user_id: str) -> int:
        """
        Return the number of authentication attempts remaining for *user_id*.

        Args:
            user_id: The user to query.

        Returns:
            Number of attempts remaining (0 if locked or session not found).
        """
        state = self._sessions.get(user_id)
        if state is None:
            return 0
        return max(0, state.attempts_remaining)

    def is_locked(self, user_id: str) -> bool:
        """
        Return True if the session for *user_id* is locked.

        Args:
            user_id: The user to query.
        """
        state = self._sessions.get(user_id)
        return state is not None and state.locked

    # ------------------------------------------------------------------
    # Internal helpers
    # ------------------------------------------------------------------

    def _get_state(self, user_id: str) -> _SessionState:
        """Return the session state for *user_id*, raising KeyError if absent."""
        if user_id not in self._sessions:
            raise KeyError(
                f"No session found for user_id={user_id!r}. "
                "Call start_session() before authenticate()."
            )
        return self._sessions[user_id]


# ---------------------------------------------------------------------------
# CLI Interface for OpenClaw Agent
# ---------------------------------------------------------------------------
def main() -> None:
    import sys
    import argparse

    parser = argparse.ArgumentParser(description="Bastion TOTP CLI helper")
    subparsers = parser.add_subparsers(dest="command", required=True)

    # Command: generate
    subparsers.add_parser("generate", help="Generate a new TOTP secret")

    # Command: qr
    qr_parser = subparsers.add_parser("qr", help="Generate QR URI")
    qr_parser.add_argument("secret", help="The TOTP secret")
    qr_parser.add_argument("user", help="The user name")

    # Command: verify
    verify_parser = subparsers.add_parser("verify", help="Verify a TOTP code")
    verify_parser.add_argument("secret", help="The TOTP secret")
    verify_parser.add_argument("code", help="The 6-digit code")

    args = parser.parse_args()

    if args.command == "generate":
        print(generate_secret().strip())
    elif args.command == "qr":
        print(generate_qr_uri(args.secret, args.user).strip())
    elif args.command == "verify":
        if verify_code(args.secret, args.code):
            print("OK")
            sys.exit(0)
        else:
            print("FAIL")
            sys.exit(1)

if __name__ == "__main__":
    main()
