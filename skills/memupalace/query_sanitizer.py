"""Query sanitizer — removes system-prompt prefix before embedding (MUPL-05).

Vendorizado do mempalace upstream (MIT). Dependency-free.
4-step mitigation que recupera recall de ~1% para >= 70% quando system prompt
é prepended à query pelo LLM antes de chamar memory_search.
"""
from __future__ import annotations

from dataclasses import dataclass

MAX_QUERY_LEN = 250
SAFE_LEN = 200
MIN_LEN = 10


@dataclass(frozen=True)
class SanitizeResult:
    clean_query: str
    was_sanitized: bool
    method: str  # "passthrough" | "question_extraction" | "last_sentence" | "truncation"


def _strip_lone_surrogates(text: str) -> str:
    """Remove lone surrogate code points that break UTF-8 encoding."""
    return "".join(
        ch for ch in text
        if not (0xD800 <= ord(ch) <= 0xDFFF)
    )


def sanitize(raw: str) -> SanitizeResult:
    """4-step mitigation para system prompt prepended à query (MUPL-05).

    Aplica ANTES de embeddar em memory_search para recuperar recall.

    - query curta (<= SAFE_LEN chars) → passthrough (was_sanitized=False)
    - query longa com '?' → extrai segmento até a última '?' (question_extraction)
    - query longa sem '?' mas com separador → extrai última sentença (last_sentence)
    - query muito longa sem separadores → trunca últimos MAX_QUERY_LEN chars (truncation)

    Sempre aplica strip de aspas e remoção de lone surrogates UTF-8.
    """
    text = _strip_lone_surrogates(raw.strip().strip('"\''))

    # Passthrough: query curta, provavelmente sem system prompt
    if len(text) <= SAFE_LEN:
        return SanitizeResult(clean_query=text, was_sanitized=False, method="passthrough")

    # Step 1: extrai ÚLTIMA sentença que termina em '?' (inclui variantes Unicode)
    for sep in ("?", "؟", "？"):  # "?", "؟", "？"
        idx = text.rfind(sep)
        if idx != -1:
            # Look for the sentence start (last sentence-boundary before the '?')
            prefix = text[:idx]
            # Find last sentence boundary before this '?'
            sentence_start = 0
            for boundary in ("\n", ".", "!", "؟", "？"):
                boundary_idx = prefix.rfind(boundary)
                if boundary_idx != -1 and boundary_idx + 1 > sentence_start:
                    sentence_start = boundary_idx + 1
            candidate = text[sentence_start: idx + 1].strip()
            if len(candidate) >= MIN_LEN:
                return SanitizeResult(
                    clean_query=candidate,
                    was_sanitized=True,
                    method="question_extraction",
                )

    # Step 2: última sentença (split por '.', '!', '\n')
    for sep in (".", "!", "\n"):
        parts = text.rsplit(sep, 1)
        if len(parts) == 2 and len(parts[1].strip()) >= MIN_LEN:
            return SanitizeResult(
                clean_query=parts[1].strip(),
                was_sanitized=True,
                method="last_sentence",
            )

    # Step 3: truncar últimos MAX_QUERY_LEN chars
    return SanitizeResult(
        clean_query=text[-MAX_QUERY_LEN:].strip(),
        was_sanitized=True,
        method="truncation",
    )
