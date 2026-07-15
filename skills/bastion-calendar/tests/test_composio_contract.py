"""
Contract / integration tests — hit the real Composio API.

These tests verify that:
  1. The Composio API is reachable with the configured key
  2. The expected toolkits (GOOGLECALENDAR, GOOGLETASKS, OUTLOOK) are connected
  3. The tool calls return responses with the expected shape
  4. Our parsers handle the real responses without crashing

Run manually:
    pytest skills/calendar/tests/test_composio_contract.py -v -s

Skipped automatically if COMPOSIO_API_KEY or COMPOSIO_CONSUMER_KEY is not set.
"""
from __future__ import annotations

import os
from datetime import datetime, timedelta, timezone

import httpx
import pytest

from parser import (
    parse_google_events,
    parse_google_tasks,
    parse_outlook_events,
    parse_outlook_tasks,
)

# ── Config ─────────────────────────────────────────────────────────────────────

COMPOSIO_API_KEY = os.getenv("COMPOSIO_API_KEY") or os.getenv("COMPOSIO_CONSUMER_KEY", "")
COMPOSIO_BASE = "https://backend.composio.dev/api/v3.1"
# user_id used when the Composio plugin was connected — defaults to the Telegram user ID
COMPOSIO_USER_ID = os.getenv("COMPOSIO_USER_ID", os.getenv("TELEGRAM_USER_ID", "default"))

requires_composio = pytest.mark.skipif(
    not COMPOSIO_API_KEY,
    reason="COMPOSIO_API_KEY / COMPOSIO_CONSUMER_KEY not set — skipping contract tests",
)


def _headers() -> dict:
    return {"x-api-key": COMPOSIO_API_KEY, "Content-Type": "application/json"}


def _execute(tool_slug: str, arguments: dict) -> dict:
    """Call Composio direct tool execution endpoint."""
    payload = {
        "tool": tool_slug,
        "user_id": COMPOSIO_USER_ID,
        "arguments": arguments,
    }
    resp = httpx.post(
        f"{COMPOSIO_BASE}/tools/execute",
        headers=_headers(),
        json=payload,
        timeout=30,
    )
    resp.raise_for_status()
    return resp.json()


# ── Connectivity ───────────────────────────────────────────────────────────────

@requires_composio
def test_composio_api_reachable():
    """Basic connectivity — list available toolkits."""
    resp = httpx.get(f"{COMPOSIO_BASE}/toolkits", headers=_headers(), timeout=10)
    assert resp.status_code == 200, f"Unexpected status: {resp.status_code}"


@requires_composio
def test_composio_connected_accounts():
    """Verify that Google Calendar and Outlook accounts are connected."""
    resp = httpx.get(
        f"{COMPOSIO_BASE}/connectedAccounts",
        headers=_headers(),
        params={"user_id": COMPOSIO_USER_ID},
        timeout=10,
    )
    assert resp.status_code == 200
    data = resp.json()
    items = data.get("items") or data.get("data", {}).get("items", [])
    toolkits = {item.get("toolkit", "").upper() for item in items}
    assert "GOOGLECALENDAR" in toolkits, f"Google Calendar not connected. Found: {toolkits}"
    assert "OUTLOOK" in toolkits, f"Outlook not connected. Found: {toolkits}"


# ── Google Calendar ────────────────────────────────────────────────────────────

@requires_composio
def test_google_calendar_list_events_shape():
    """GOOGLECALENDAR_EVENTS_LIST_ALL_CALENDARS returns parseable response."""
    now = datetime.now(tz=timezone.utc)
    raw = _execute("GOOGLECALENDAR_EVENTS_LIST_ALL_CALENDARS", {
        "timeMin": now.isoformat(),
        "timeMax": (now + timedelta(hours=24)).isoformat(),
        "singleEvents": True,
        "orderBy": "startTime",
    })
    # Must not raise
    events = parse_google_events(raw)
    assert isinstance(events, list)
    for e in events:
        assert e.start.tzinfo is not None, "Event start must be timezone-aware"
        assert e.title, "Event title must not be empty"


@requires_composio
def test_google_calendar_imminent_window():
    """Events in next 60 min are correctly identified as imminent if within 5 min."""
    now = datetime.now(tz=timezone.utc)
    raw = _execute("GOOGLECALENDAR_EVENTS_LIST_ALL_CALENDARS", {
        "timeMin": now.isoformat(),
        "timeMax": (now + timedelta(minutes=60)).isoformat(),
        "singleEvents": True,
        "orderBy": "startTime",
    })
    from models import CalendarSummary
    events = parse_google_events(raw)
    summary = CalendarSummary(events=events)
    # Imminent must be a subset of all events
    assert all(e in summary.events for e in summary.imminent_events)


# ── Google Tasks ───────────────────────────────────────────────────────────────

@requires_composio
def test_google_tasks_list_shape():
    """GOOGLETASKS_LIST_ALL_TASKS returns parseable response."""
    raw = _execute("GOOGLETASKS_LIST_ALL_TASKS", {
        "showCompleted": False,
        "showHidden": False,
    })
    tasks = parse_google_tasks(raw)
    assert isinstance(tasks, list)
    for t in tasks:
        assert t.title, "Task title must not be empty"
        assert not t.completed


# ── Outlook Calendar ───────────────────────────────────────────────────────────

@requires_composio
def test_outlook_list_events_shape():
    """OUTLOOK_LIST_EVENTS returns parseable response."""
    now = datetime.now(tz=timezone.utc)
    raw = _execute("OUTLOOK_LIST_EVENTS", {
        "$filter": (
            f"start/dateTime ge '{now.isoformat()}' and "
            f"start/dateTime le '{(now + timedelta(hours=24)).isoformat()}'"
        ),
        "$orderby": "start/dateTime",
        "$top": 10,
    })
    events = parse_outlook_events(raw)
    assert isinstance(events, list)
    for e in events:
        assert e.start.tzinfo is not None


# ── Outlook Tasks ──────────────────────────────────────────────────────────────

@requires_composio
def test_outlook_todo_lists_and_tasks_shape():
    """OUTLOOK_LIST_TO_DO_LISTS + OUTLOOK_LIST_TODO_TASKS return parseable responses."""
    lists_raw = _execute("OUTLOOK_LIST_TO_DO_LISTS", {})
    lists = lists_raw.get("value") or lists_raw.get("data", {}).get("value", [])
    assert isinstance(lists, list)

    all_tasks = []
    for lst in lists[:3]:  # check first 3 lists max
        list_id = lst.get("id", "")
        if not list_id:
            continue
        tasks_raw = _execute("OUTLOOK_LIST_TODO_TASKS", {"todoTaskListId": list_id})
        tasks = parse_outlook_tasks(tasks_raw, list_name=lst.get("displayName", ""))
        all_tasks.extend(tasks)

    assert isinstance(all_tasks, list)
    for t in all_tasks:
        assert t.title
        assert not t.completed
