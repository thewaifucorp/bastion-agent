"""Tests for query_sanitizer — includes MUPL-05 recall criterion (>= 70%)."""
from __future__ import annotations

import pytest

from skills.memupalace.query_sanitizer import sanitize, SanitizeResult, SAFE_LEN

SYSTEM_PROMPT = (
    "You are a helpful personal assistant. "
    "Answer the user's questions concisely and accurately. "
    "Always be polite and professional in your responses. "
    "Be thorough and provide examples when relevant. "
    "Consider the user's context carefully before responding. "
    "Use clear and simple language at all times."
)

# Pares (raw_query_with_system_prompt, expected_keyword_in_clean)
RECALL_PAIRS = [
    (SYSTEM_PROMPT + "\nQual minha meta de 2026?",            "meta"),
    (SYSTEM_PROMPT + "\nComo está meu progresso em fitness?",  "progresso"),
    (SYSTEM_PROMPT + "\nQuais são minhas prioridades da semana?", "prioridades"),
    (SYSTEM_PROMPT + "\nO que preciso comprar hoje?",           "comprar"),
    (SYSTEM_PROMPT + "\nMeu aniversário de casamento é quando?", "casamento"),
    (SYSTEM_PROMPT + "\nQual foi o livro que li em março?",     "livro"),
    (SYSTEM_PROMPT + "\nPreciso renovar meu passaporte?",       "passaporte"),
    (SYSTEM_PROMPT + "\nQuando é a reunião com o cliente?",     "reunião"),
    (SYSTEM_PROMPT + "\nMinha meta de economia mensal é?",      "economia"),
    (SYSTEM_PROMPT + "\nComo foi minha semana produtiva?",      "semana"),
]


class TestSanitizeUnit:
    def test_short_query_passthrough(self):
        result = sanitize("Qual minha meta?")
        assert result.was_sanitized is False
        assert result.method == "passthrough"

    def test_long_query_with_question_mark(self):
        raw = SYSTEM_PROMPT + "\nQual minha meta de 2026?"
        result = sanitize(raw)
        assert result.was_sanitized is True
        assert result.method == "question_extraction"
        assert "?" in result.clean_query
        assert len(result.clean_query) <= 250

    def test_extracts_meaningful_fragment(self):
        raw = SYSTEM_PROMPT + "\nQual minha meta de 2026?"
        result = sanitize(raw)
        assert any(kw in result.clean_query.lower() for kw in ("meta", "2026"))

    def test_clean_query_max_length(self):
        very_long = "x " * 500
        result = sanitize(very_long)
        assert len(result.clean_query) <= 250

    def test_strips_quotes(self):
        result = sanitize('"Qual a meta?"')
        assert not result.clean_query.startswith('"')

    def test_passthrough_preserves_content(self):
        q = "Minha meta de fitness?"
        result = sanitize(q)
        assert result.clean_query == q.strip()


class TestRecallCriterion:
    """MUPL-05: sanitized query must contain expected keyword in >= 70% of cases."""

    def test_recall_with_system_prompt_prepended(self):
        hits = 0
        for raw, keyword in RECALL_PAIRS:
            result = sanitize(raw)
            if keyword in result.clean_query.lower():
                hits += 1
        recall = hits / len(RECALL_PAIRS)
        assert recall >= 0.70, (
            f"Recall {recall:.0%} < 70% — sanitizer failed to extract keyword "
            f"from {len(RECALL_PAIRS) - hits}/{len(RECALL_PAIRS)} queries. "
            "MUPL-05 criterion not met."
        )

    def test_all_sanitized_queries_are_shorter_than_raw(self):
        for raw, _ in RECALL_PAIRS:
            result = sanitize(raw)
            assert len(result.clean_query) < len(raw), (
                f"Expected clean_query shorter than raw for: {raw[:50]}..."
            )
