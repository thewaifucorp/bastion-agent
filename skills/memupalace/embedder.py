"""ONNX-based text embedder for memupalace."""

from __future__ import annotations

import math
from pathlib import Path
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    import onnxruntime as ort


class ONNXEmbedder:
    """Generates text embeddings using a local ONNX model.

    The model is loaded once at initialization and reused for all subsequent
    calls — no external API calls are made.
    """

    def __init__(self, model_path: str, tokenizer_name: str | None = None) -> None:
        """Load the ONNX model from *model_path*.

        Args:
            model_path: Path to the ONNX model file.
            tokenizer_name: HuggingFace tokenizer name. If None, reads from
                ``MEMUPALACE_TOKENIZER_NAME`` env var, falling back to
                ``sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2``
                (multilingual model with pt-BR support).

        Raises:
            FileNotFoundError: If the model file does not exist at *model_path*.
            ImportError: If ``onnxruntime`` is not installed.
            ImportError: If ``transformers`` is not installed.
        """
        import os

        path = Path(model_path)
        if not path.exists():
            raise FileNotFoundError(
                f"ONNX model not found at expected path: {path.resolve()}"
            )

        try:
            import onnxruntime as ort  # noqa: PLC0415
        except ImportError as exc:
            raise ImportError(
                "onnxruntime is required for ONNXEmbedder. "
                "Install it with: pip install onnxruntime"
            ) from exc

        try:
            from transformers import AutoTokenizer  # noqa: PLC0415
        except ImportError as exc:
            raise ImportError(
                "transformers is required for ONNXEmbedder tokenization. "
                "Install it with: pip install transformers"
            ) from exc

        self._session: ort.InferenceSession = ort.InferenceSession(
            str(path),
            providers=["CPUExecutionProvider"],
        )
        _tokenizer_name = tokenizer_name or os.getenv(
            "MEMUPALACE_TOKENIZER_NAME",
            "sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2",
        )
        self._tokenizer = AutoTokenizer.from_pretrained(_tokenizer_name)

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    def embed(self, text: str) -> list[float]:
        """Return the L2-normalised embedding for *text*.

        Steps:
        1. Tokenise with padding/truncation (max_length=256).
        2. Run ONNX inference.
        3. Mean-pool over token embeddings (respecting attention mask).
        4. L2-normalise the pooled vector.
        5. Return as ``list[float]``.
        """
        return self.embed_batch([text])[0]

    def embed_batch(self, texts: list[str]) -> list[list[float]]:
        """Return L2-normalised embeddings for a batch of texts."""
        import numpy as np  # noqa: PLC0415

        encoded = self._tokenizer(
            texts,
            padding=True,
            truncation=True,
            max_length=256,
            return_tensors="np",
        )

        input_ids = encoded["input_ids"].astype(np.int64)
        attention_mask = encoded["attention_mask"].astype(np.int64)

        available: dict[str, object] = {
            "input_ids": input_ids,
            "attention_mask": attention_mask,
        }
        token_type_ids = encoded.get("token_type_ids")
        if token_type_ids is not None:
            available["token_type_ids"] = token_type_ids.astype(np.int64)

        # Feed exactly the inputs the MODEL declares, not whatever the tokenizer
        # happens to emit — the two disagree in the wild: XLM-R tokenizers (this
        # model's family) emit no token_type_ids, while some ONNX exports of the
        # same model still declare it as a REQUIRED graph input (missing input →
        # ORT "required inputs are missing" error); the reverse — feeding an
        # input the graph does not declare — is an ORT error too. A missing
        # token_type_ids is synthesized as zeros: single-sentence input is
        # segment 0 everywhere, so zeros are semantically exact, not a stub.
        feed: dict[str, object] = {}
        for model_input in self._session.get_inputs():
            name = model_input.name
            if name in available:
                feed[name] = available[name]
            elif name == "token_type_ids":
                feed[name] = np.zeros_like(input_ids)

        outputs = self._session.run(None, feed)
        # outputs[0] shape: (batch, seq_len, hidden) — last hidden state
        token_embeddings: object = outputs[0]

        # Mean pooling: average over non-padding tokens
        mask = attention_mask[:, :, np.newaxis].astype(np.float32)
        summed = np.sum(token_embeddings * mask, axis=1)  # type: ignore[operator]
        counts = np.clip(mask.sum(axis=1), a_min=1e-9, a_max=None)
        mean_pooled = summed / counts  # (batch, hidden)

        # L2 normalise each vector
        norms = np.linalg.norm(mean_pooled, axis=1, keepdims=True)
        norms = np.where(norms == 0, 1.0, norms)
        normalised = mean_pooled / norms

        return [row.tolist() for row in normalised]
