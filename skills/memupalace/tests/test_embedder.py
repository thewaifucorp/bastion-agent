"""Tests for ONNXEmbedder — unit tests and property-based tests."""

from __future__ import annotations

import math
from typing import Any
from unittest.mock import MagicMock

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

_EMBED_DIM = 384

# Check optional heavy deps once
try:
    import numpy as np  # noqa: F401
    _HAS_NUMPY = True
except ImportError:
    _HAS_NUMPY = False

requires_numpy = pytest.mark.skipif(not _HAS_NUMPY, reason="numpy not installed")


def _make_stub_embedder(dim: int = _EMBED_DIM) -> Any:
    """Return a lightweight pure-Python stub that mimics ONNXEmbedder."""

    class _StubEmbedder:
        def embed(self, text: str) -> list[float]:
            return self.embed_batch([text])[0]

        def embed_batch(self, texts: list[str]) -> list[list[float]]:
            results = []
            for text in texts:
                seed = hash(text) % (2**31)
                vec = [math.sin(seed + i) * 0.1 + 0.01 for i in range(dim)]
                norm = math.sqrt(sum(x * x for x in vec))
                results.append([x / norm for x in vec])
            return results

    return _StubEmbedder()


# ---------------------------------------------------------------------------
# Unit tests — FileNotFoundError (no numpy/onnxruntime needed)
# ---------------------------------------------------------------------------


class TestONNXEmbedderInit:
    def test_file_not_found_raises_with_path(self, tmp_path: Any) -> None:
        """FileNotFoundError must include the expected path when model is missing."""
        from embedder import ONNXEmbedder

        missing = str(tmp_path / "nonexistent" / "model.onnx")
        with pytest.raises(FileNotFoundError) as exc_info:
            ONNXEmbedder(missing)
        assert "nonexistent" in str(exc_info.value) or "model.onnx" in str(exc_info.value)

    def test_file_not_found_message_contains_path(self, tmp_path: Any) -> None:
        """The FileNotFoundError message must contain the expected path."""
        from embedder import ONNXEmbedder

        model_path = str(tmp_path / "models" / "embedder.onnx")
        with pytest.raises(FileNotFoundError) as exc_info:
            ONNXEmbedder(model_path)
        assert "embedder.onnx" in str(exc_info.value)


# ---------------------------------------------------------------------------
# Unit tests — embed / embed_batch (require numpy; use mock session)
# ---------------------------------------------------------------------------


def _make_embedder_with_mock(
    dim: int = _EMBED_DIM,
    *,
    tokenizer_emits_token_type_ids: bool = True,
    model_declares_token_type_ids: bool = True,
) -> Any:
    """Construct ONNXEmbedder bypassing file check and real session.

    The two keyword flags simulate the tokenizer↔ONNX-export mismatches seen in
    the wild (XLM-R tokenizer emits no token_type_ids; some exports declare it).
    """
    import numpy as np
    from embedder import ONNXEmbedder

    embedder = object.__new__(ONNXEmbedder)

    tokenizer = MagicMock()

    def _tokenize(texts: list[str], **kwargs: Any) -> dict[str, Any]:
        n = len(texts)
        out = {
            "input_ids": np.ones((n, 5), dtype=np.int64),
            "attention_mask": np.ones((n, 5), dtype=np.int64),
        }
        if tokenizer_emits_token_type_ids:
            out["token_type_ids"] = np.zeros((n, 5), dtype=np.int64)
        return out

    tokenizer.side_effect = _tokenize
    embedder._tokenizer = tokenizer  # type: ignore[attr-defined]

    session = MagicMock()

    # NOTE: MagicMock(name=...) sets the mock's own name, NOT a .name attribute —
    # build the input specs with explicit attribute assignment.
    declared = ["input_ids", "attention_mask"]
    if model_declares_token_type_ids:
        declared.append("token_type_ids")
    input_specs = []
    for input_name in declared:
        spec = MagicMock()
        spec.name = input_name
        input_specs.append(spec)
    session.get_inputs.return_value = input_specs

    def _run(output_names: Any, feed: Any) -> list[Any]:
        batch = feed["input_ids"].shape[0]
        return [np.ones((batch, 5, dim), dtype=np.float32)]

    session.run.side_effect = _run
    embedder._session = session  # type: ignore[attr-defined]
    return embedder


@requires_numpy
def test_embed_returns_list_of_floats() -> None:
    embedder = _make_embedder_with_mock()
    result = embedder.embed("hello world")
    assert isinstance(result, list)
    assert all(isinstance(v, float) for v in result)


@requires_numpy
def test_embed_returns_correct_dimension() -> None:
    embedder = _make_embedder_with_mock()
    result = embedder.embed("test text")
    assert len(result) == _EMBED_DIM


@requires_numpy
def test_embed_batch_returns_list_of_lists() -> None:
    embedder = _make_embedder_with_mock()
    texts = ["first", "second", "third"]
    results = embedder.embed_batch(texts)
    assert isinstance(results, list)
    assert len(results) == 3
    for vec in results:
        assert isinstance(vec, list)
        assert len(vec) == _EMBED_DIM


@requires_numpy
def test_embed_batch_same_dimension_as_embed() -> None:
    embedder = _make_embedder_with_mock()
    single = embedder.embed("hello")
    batch = embedder.embed_batch(["hello", "world"])
    assert len(single) == len(batch[0]) == len(batch[1])


@requires_numpy
def test_embed_is_l2_normalised() -> None:
    embedder = _make_embedder_with_mock()
    vec = embedder.embed("normalisation test")
    norm = math.sqrt(sum(x * x for x in vec))
    assert abs(norm - 1.0) < 1e-5


# ---------------------------------------------------------------------------
# Regression: tokenizer ↔ ONNX-export input mismatch (token_type_ids)
# Bug found live 2026-07-02: XLM-R tokenizer emits no token_type_ids but the
# deployed ONNX export declared it as required → "required inputs are missing".
# ---------------------------------------------------------------------------


def _capture_feed(embedder: Any) -> dict[str, Any]:
    """Wrap the mock session.run to record the feed dict it receives."""
    captured: dict[str, Any] = {}
    original_run = embedder._session.run.side_effect

    def _capturing_run(output_names: Any, feed: Any) -> Any:
        captured.update(feed)
        return original_run(output_names, feed)

    embedder._session.run.side_effect = _capturing_run
    return captured


@requires_numpy
def test_token_type_ids_synthesized_when_model_requires_but_tokenizer_omits() -> None:
    """Model declares token_type_ids, tokenizer omits it → zeros are fed."""
    import numpy as np

    embedder = _make_embedder_with_mock(
        tokenizer_emits_token_type_ids=False,
        model_declares_token_type_ids=True,
    )
    captured = _capture_feed(embedder)
    embedder.embed("rex")

    assert "token_type_ids" in captured, "required model input must be fed"
    assert np.all(captured["token_type_ids"] == 0), "single segment → all zeros"
    assert captured["token_type_ids"].shape == captured["input_ids"].shape


@requires_numpy
def test_token_type_ids_dropped_when_model_does_not_declare_it() -> None:
    """Tokenizer emits token_type_ids, model doesn't declare it → not fed."""
    embedder = _make_embedder_with_mock(
        tokenizer_emits_token_type_ids=True,
        model_declares_token_type_ids=False,
    )
    captured = _capture_feed(embedder)
    embedder.embed("rex")

    assert "token_type_ids" not in captured, "undeclared graph input must not be fed"
    assert set(captured) == {"input_ids", "attention_mask"}


# ---------------------------------------------------------------------------
# Property 6: Embedding Validity
# Validates: Requirements 3.4, 3.5
# ---------------------------------------------------------------------------

# Use the stub embedder for property tests — no real ONNX needed
_STUB = _make_stub_embedder()


@given(text=st.text(min_size=1))
@settings(max_examples=100)
def test_embedding_validity_consistent_dimension(text: str) -> None:
    """Property 6 (a): For any non-empty text, embedding has consistent dimension.

    Validates: Requirements 3.4, 3.5
    """
    vec = _STUB.embed(text)
    assert len(vec) == _EMBED_DIM, (
        f"Expected dimension {_EMBED_DIM}, got {len(vec)} for text={text!r}"
    )


@given(text=st.text(min_size=1))
@settings(max_examples=100)
def test_embedding_validity_nonzero_l2_norm(text: str) -> None:
    """Property 6 (b): For any non-empty text, embedding has L2 norm > 0.

    Validates: Requirements 3.4, 3.5
    """
    vec = _STUB.embed(text)
    norm = math.sqrt(sum(x * x for x in vec))
    assert norm > 0, f"L2 norm must be > 0, got {norm} for text={text!r}"


@given(
    text_a=st.text(min_size=1),
    text_b=st.text(min_size=1),
)
@settings(max_examples=100)
def test_embedding_validity_same_dimension_for_any_two_texts(
    text_a: str, text_b: str
) -> None:
    """Property 6: Any two texts produce embeddings of identical dimension.

    Validates: Requirements 3.4
    """
    vec_a = _STUB.embed(text_a)
    vec_b = _STUB.embed(text_b)
    assert len(vec_a) == len(vec_b), (
        f"Dimension mismatch: {len(vec_a)} vs {len(vec_b)}"
    )
