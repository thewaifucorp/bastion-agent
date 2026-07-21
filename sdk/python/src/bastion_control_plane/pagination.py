"""Mirrors ``sdk/typescript/src/pagination.ts``."""

from __future__ import annotations

from typing import Callable, Iterator, Optional


def paginate(fetch_page: Callable[[Optional[str]], dict]) -> Iterator[dict]:
    """Wrap a cursor-paginated ``/v1/*`` list endpoint in an iterator, so
    callers can ``for item in paginate(...)`` instead of hand-rolling the
    ``next_cursor`` loop. Purely a convenience over calling the list method
    directly -- see the "SDK conveniences" list in the planning doc
    (``US-external-control-plane-sdk.md``: "pagination iterator").

    ``fetch_page`` takes the current cursor (``None`` for the first page)
    and returns a dict with ``"items"`` (a list) and ``"next_cursor"``
    (``str`` or ``None``) -- exactly the shape ``TaskListResponse``/
    ``AttemptListResponse`` already have.
    """
    cursor: Optional[str] = None
    while True:
        page = fetch_page(cursor)
        for item in page["items"]:
            yield item
        if page["next_cursor"] is None:
            return
        cursor = page["next_cursor"]
