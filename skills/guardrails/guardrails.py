"""
Guardrails — behavioral safety rules for the Bastion agent.

Implements:
  - GuardrailResult: result dataclass for all guardrail checks
  - GuardrailEngine: engine with methods for each guardrail category

Guardrails:
  1. Financial (Req 11.1): block autonomous execution of financial actions
  2. Irreversible (Req 11.2): require confirmation in exact format
  3. Anti prompt injection (Req 11.3): treat external content as data
  4. User authorization (Req 11.4): whitelist via USER.md authorized_user_ids
  5. Skill installation (Req 11.5): Verified badge + rating >= 4.0 + 50+ reviews
"""

from __future__ import annotations

import logging
import re
from dataclasses import dataclass, field
from pathlib import Path

from i18n import get_string, load_locale

logger = logging.getLogger(__name__)

# ---------------------------------------------------------------------------
# Domain models
# ---------------------------------------------------------------------------

# Keywords that indicate a financial transaction
FINANCIAL_KEYWORDS: frozenset[str] = frozenset(
    [
        "pagamento",
        "transferência",
        "transferencia",
        "pix",
        "ted",
        "doc",
        "boleto",
        "débito",
        "debito",
        "crédito",
        "credito",
        "compra",
        "venda",
        "invoice",
        "payment",
        "transfer",
        "transaction",
        "charge",
        "billing",
        "withdraw",
        "deposit",
        "wire",
        "remittance",
    ]
)

# Keywords that indicate an irreversible action
IRREVERSIBLE_KEYWORDS: frozenset[str] = frozenset(
    [
        "deletar",
        "delete",
        "remover",
        "remove",
        "excluir",
        "apagar",
        "cancelar",
        "cancel",
        "enviar email",
        "send email",
        "postar",
        "post",
        "publicar",
        "publish",
        "revogar",
        "revoke",
        "modificar configuração",
        "modify config",
        "remarcar",
        "reschedule",
    ]
)

# Patterns that indicate prompt injection attempts in external content
INJECTION_PATTERNS: list[re.Pattern[str]] = [
    re.compile(r"ignore\s+(suas|your|all|previous)\s+instru", re.IGNORECASE),
    re.compile(r"\[SYSTEM\]\s*:", re.IGNORECASE),
    re.compile(r"<!--\s*instru[çc][aã]o\s+para\s+o\s+agente", re.IGNORECASE),
    re.compile(r"a\s+partir\s+de\s+agora\s+voc[eê]\s+deve", re.IGNORECASE),
    re.compile(r"new\s+instructions?\s*:", re.IGNORECASE),
    re.compile(r"system\s+prompt\s*:", re.IGNORECASE),
    re.compile(r"forget\s+(all\s+)?previous\s+instructions?", re.IGNORECASE),
    re.compile(r"you\s+are\s+now\s+a", re.IGNORECASE),
    re.compile(r"act\s+as\s+(if\s+you\s+are|a)\s+", re.IGNORECASE),
    re.compile(r"disregard\s+(all\s+)?previous", re.IGNORECASE),
    re.compile(r"override\s+(your\s+)?instructions?", re.IGNORECASE),
    re.compile(r"jailbreak", re.IGNORECASE),
    re.compile(r"DAN\s+mode", re.IGNORECASE),
]

# Minimum skill installation criteria (Req 11.5)
SKILL_MIN_RATING: float = 4.0
SKILL_MIN_REVIEWS: int = 50


@dataclass
class GuardrailResult:
    """Result of a guardrail check."""

    allowed: bool
    reason: str
    requires_confirmation: bool = False
    confirmation_prompt: str = ""


@dataclass
class SkillMetadata:
    """Metadata for a ClawHub skill installation check."""

    name: str
    verified: bool
    rating: float
    review_count: int
    has_filesystem_access: bool = False
    has_network_access: bool = False
    family: str = ""  # e.g. "bastion" for bastion/* skills


@dataclass
class FinancialAction:
    """Represents an action that may involve a financial transaction."""

    description: str
    amount: float | None = None
    recipient: str | None = None
    keywords: list[str] = field(default_factory=list)


@dataclass
class IrreversibleAction:
    """Represents an action that cannot be undone."""

    description: str
    action_type: str = ""


# ---------------------------------------------------------------------------
# Guardrail Engine
# ---------------------------------------------------------------------------


class GuardrailEngine:
    """
    Engine that enforces all Bastion guardrails.

    Each method returns a GuardrailResult indicating whether the action
    is allowed, requires confirmation, and the reason.
    """

    def __init__(self, language: str = "en") -> None:
        self._locale = load_locale(language, skill_dir=Path(__file__).parent)

    # ------------------------------------------------------------------
    # Guardrail 1 — Financial (Req 11.1)
    # ------------------------------------------------------------------

    def check_financial_action(self, action: FinancialAction) -> GuardrailResult:
        """
        Block autonomous execution of any financial action.

        For any action involving a financial transaction, the system must
        always request explicit user confirmation and not execute without it.

        Returns:
            GuardrailResult with allowed=False and requires_confirmation=True
            for any financial action, regardless of context.
        """
        description_lower = action.description.lower()
        detected_keywords = [
            kw for kw in action.keywords if kw.lower() in FINANCIAL_KEYWORDS
        ] + [
            kw
            for kw in FINANCIAL_KEYWORDS
            if kw in description_lower
        ]

        if not detected_keywords and not action.keywords:
            # No financial indicators — check description for keywords
            has_financial = any(kw in description_lower for kw in FINANCIAL_KEYWORDS)
            if not has_financial:
                return GuardrailResult(
                    allowed=True,
                    reason="No financial transaction detected.",
                    requires_confirmation=False,
                )

        # Financial action detected — always block autonomous execution
        parts = [get_string(self._locale, "financial_detected", description=action.description)]
        if action.amount is not None:
            parts.append(get_string(self._locale, "financial_amount", amount=action.amount))
        if action.recipient:
            parts.append(get_string(self._locale, "financial_recipient", recipient=action.recipient))

        reason = " | ".join(parts)

        return GuardrailResult(
            allowed=False,
            reason=reason,
            requires_confirmation=True,
            confirmation_prompt=get_string(self._locale, "confirm_action", description=action.description),
        )

    # ------------------------------------------------------------------
    # Guardrail 2 — Irreversible actions (Req 11.2)
    # ------------------------------------------------------------------

    def check_irreversible_action(self, action: IrreversibleAction) -> GuardrailResult:
        """
        Require confirmation for any irreversible action.

        The confirmation must be in the exact format:
            "Vou [ação exata]. Confirma? (sim/não)"

        Returns:
            GuardrailResult with allowed=False and requires_confirmation=True,
            with confirmation_prompt in the required format.
        """
        return GuardrailResult(
            allowed=False,
            reason=get_string(self._locale, "irreversible_confirm", description=action.description),
            requires_confirmation=True,
            confirmation_prompt=get_string(self._locale, "confirm_action", description=action.description),
        )

    # ------------------------------------------------------------------
    # Guardrail 3 — Anti prompt injection (Req 11.3)
    # ------------------------------------------------------------------

    def check_external_content(self, content: str) -> GuardrailResult:
        """
        Detect and block prompt injection attempts in external content.

        External content (web pages, files, search results, emails) must
        always be treated as data, never as instructions.

        Returns:
            GuardrailResult with allowed=False if injection is detected,
            allowed=True if content is safe to process as data.
        """
        for pattern in INJECTION_PATTERNS:
            match = pattern.search(content)
            if match:
                detected_snippet = match.group(0)
                logger.warning(
                    "Prompt injection attempt detected: snippet=%r",
                    detected_snippet,
                )
                return GuardrailResult(
                    allowed=False,
                    reason=get_string(self._locale, "injection_detected", snippet=detected_snippet),
                    requires_confirmation=False,
                )

        return GuardrailResult(
            allowed=True,
            reason=get_string(self._locale, "content_safe"),
            requires_confirmation=False,
        )

    # ------------------------------------------------------------------
    # Guardrail 4 — User authorization (Req 11.4)
    # ------------------------------------------------------------------

    def check_user_authorized(
        self,
        user_id: str,
        authorized_ids: list[str],
    ) -> GuardrailResult:
        """
        Check if a user_id is in the authorized whitelist from USER.md.

        Messages from unauthorized user_ids must be silently ignored.

        Args:
            user_id: The ID of the user sending the message.
            authorized_ids: List of authorized user_ids from USER.md.

        Returns:
            GuardrailResult with allowed=True if authorized, False otherwise.
        """
        if user_id in authorized_ids:
            return GuardrailResult(
                allowed=True,
                reason=get_string(self._locale, "user_authorized", user_id=repr(user_id)),
                requires_confirmation=False,
            )

        return GuardrailResult(
            allowed=False,
            reason=get_string(self._locale, "user_unauthorized"),
            requires_confirmation=False,
        )

    # ------------------------------------------------------------------
    # Guardrail 5 — Skill installation (Req 11.5)
    # ------------------------------------------------------------------

    def check_skill_installation(self, skill: SkillMetadata) -> GuardrailResult:
        """
        Block installation of ClawHub skills that don't meet minimum criteria.

        Criteria (for non-bastion/* skills):
          - Badge "Verified" (required for skills with filesystem or network access)
          - Rating >= 4.0
          - 50+ reviews

        bastion/* skills are exempt from these checks.

        Returns:
            GuardrailResult with allowed=True if all criteria are met,
            False with reason if any criterion fails.
        """
        # bastion/* skills are exempt
        if skill.family == "bastion" or skill.name.startswith("bastion/"):
            return GuardrailResult(
                allowed=True,
                reason=get_string(self._locale, "skill_bastion_exempt", name=repr(skill.name)),
                requires_confirmation=False,
            )

        failures: list[str] = []

        # Check Verified badge (required for filesystem/network access)
        needs_verified = skill.has_filesystem_access or skill.has_network_access
        if needs_verified and not skill.verified:
            failures.append(get_string(self._locale, "badge_missing"))

        # Check minimum rating
        if skill.rating < SKILL_MIN_RATING:
            failures.append(
                get_string(self._locale, "rating_below_min",
                           rating=f"{skill.rating:.1f}", min_rating=f"{SKILL_MIN_RATING:.1f}")
            )

        # Check minimum review count
        if skill.review_count < SKILL_MIN_REVIEWS:
            failures.append(
                get_string(self._locale, "reviews_below_min",
                           count=skill.review_count, min_reviews=SKILL_MIN_REVIEWS)
            )

        if failures:
            reason = get_string(self._locale, "install_blocked",
                                name=repr(skill.name), reasons="; ".join(failures))
            return GuardrailResult(
                allowed=False,
                reason=reason,
                requires_confirmation=False,
            )

        return GuardrailResult(
            allowed=True,
            reason=get_string(self._locale, "install_allowed", name=repr(skill.name)),
            requires_confirmation=False,
        )


# ---------------------------------------------------------------------------
# CLI Interface for OpenClaw Agent
# ---------------------------------------------------------------------------
def main() -> None:
    import argparse
    import json
    import sys
    
    parser = argparse.ArgumentParser(description="CLI wrapper generated by refactoring")
    parser.add_argument("--action", help="Action to perform")
    parser.add_argument("--args-json", default="{}", help="Arguments as JSON string")
    
    args = parser.parse_args()
    print("Execution of stub CLI for", __file__)
    print("Action:", args.action)
    print("Args:", args.args_json)

if __name__ == "__main__":
    main()
