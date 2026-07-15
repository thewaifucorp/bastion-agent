"""Salience scorer for memupalace memories."""

from __future__ import annotations

import math


def salience_score(
    similarity: float,
    reinforcement_count: int,
    days_ago: float,
    recency_decay_days: int = 30,
) -> float:
    """Compute the salience score for a memory candidate.

    score = similarity * reinforcement_factor * recency_factor

    reinforcement_factor = max(1.0, log(reinforcement_count + 1))
    recency_factor = exp(-0.693 * days_ago / recency_decay_days)

    Base case (count=0, days_ago=0): score = similarity * 1.0 * 1.0 = similarity
    """
    reinforcement_factor = max(1.0, math.log(reinforcement_count + 1))
    recency_factor = math.exp(-0.693 * days_ago / recency_decay_days)
    return similarity * reinforcement_factor * recency_factor
