"""Parse raw Composio API responses into CalendarEvent / Task objects."""
from __future__ import annotations

import logging
from datetime import datetime, timezone

from models import CalendarEvent, CalendarSource, Task

logger = logging.getLogger(__name__)

_RECIFE_OFFSET = "+00:00"  # parsed datetimes are kept as UTC; display layer converts


def _parse_dt(value: str | dict | None) -> datetime | None:
    """Parse ISO 8601 string or Google/Outlook dateTime dict into aware datetime."""
    if value is None:
        return None
    if isinstance(value, dict):
        # Google: {"dateTime": "...", "timeZone": "..."} or {"date": "..."}
        value = value.get("dateTime") or value.get("date") or ""
    if not value:
        return None
    try:
        dt = datetime.fromisoformat(value.replace("Z", "+00:00"))
        if dt.tzinfo is None:
            dt = dt.replace(tzinfo=timezone.utc)
        return dt
    except (ValueError, TypeError):
        logger.warning("Could not parse datetime: %r", value)
        return None


# ── Google Calendar ────────────────────────────────────────────────────────────

def parse_google_events(raw: dict) -> list[CalendarEvent]:
    """Parse GOOGLECALENDAR_EVENTS_LIST* response."""
    items = raw.get("items") or raw.get("data", {}).get("items", [])
    events: list[CalendarEvent] = []
    for item in items:
        start = _parse_dt(item.get("start"))
        end = _parse_dt(item.get("end"))
        if start is None or end is None:
            continue
        events.append(CalendarEvent(
            id=item.get("id", ""),
            title=item.get("summary", "(sem título)"),
            start=start,
            end=end,
            source=CalendarSource.GOOGLE,
            location=item.get("location", ""),
            description=item.get("description", ""),
        ))
    return events


# ── Google Tasks ───────────────────────────────────────────────────────────────

def parse_google_tasks(raw: dict) -> list[Task]:
    """Parse GOOGLETASKS_LIST_ALL_TASKS / GOOGLETASKS_LIST_TASKS response."""
    items = raw.get("items") or raw.get("data", {}).get("items", [])
    tasks: list[Task] = []
    for item in items:
        if item.get("status") == "completed":
            continue
        due = _parse_dt(item.get("due"))
        tasks.append(Task(
            id=item.get("id", ""),
            title=item.get("title", "(sem título)"),
            source=CalendarSource.GOOGLE,
            due=due,
            completed=False,
            list_name=item.get("selfLink", "").split("/")[0],
        ))
    return tasks


# ── Outlook Calendar ───────────────────────────────────────────────────────────

def parse_outlook_events(raw: dict) -> list[CalendarEvent]:
    """Parse OUTLOOK_LIST_EVENTS / OUTLOOK_GET_CALENDAR_VIEW response."""
    items = raw.get("value") or raw.get("data", {}).get("value", [])
    events: list[CalendarEvent] = []
    for item in items:
        start = _parse_dt(item.get("start", {}).get("dateTime"))
        end = _parse_dt(item.get("end", {}).get("dateTime"))
        if start is None or end is None:
            continue
        events.append(CalendarEvent(
            id=item.get("id", ""),
            title=item.get("subject", "(sem título)"),
            start=start,
            end=end,
            source=CalendarSource.OUTLOOK,
            location=item.get("location", {}).get("displayName", ""),
            description=item.get("bodyPreview", ""),
        ))
    return events


# ── Outlook Tasks (To Do) ──────────────────────────────────────────────────────

def parse_outlook_tasks(raw: dict, list_name: str = "") -> list[Task]:
    """Parse OUTLOOK_LIST_TODO_TASKS response."""
    items = raw.get("value") or raw.get("data", {}).get("value", [])
    tasks: list[Task] = []
    for item in items:
        if item.get("status") == "completed":
            continue
        due_dict = item.get("dueDateTime")
        due = _parse_dt(due_dict.get("dateTime") if due_dict else None)
        tasks.append(Task(
            id=item.get("id", ""),
            title=item.get("title", "(sem título)"),
            source=CalendarSource.OUTLOOK,
            due=due,
            completed=False,
            list_name=list_name,
        ))
    return tasks
