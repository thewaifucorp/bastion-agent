"""Unit + property tests for calendar/parser.py — no network required."""
from __future__ import annotations

from datetime import datetime, timedelta, timezone
from zoneinfo import ZoneInfo

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st

from models import CalendarSource, CalendarSummary
from parser import (
    parse_google_events,
    parse_google_tasks,
    parse_outlook_events,
    parse_outlook_tasks,
)

RECIFE = ZoneInfo("America/Recife")
NOW = datetime.now(tz=timezone.utc)


# ── Helpers ────────────────────────────────────────────────────────────────────

def _google_event(title: str, minutes_from_now: float, duration_min: int = 60) -> dict:
    start = NOW + timedelta(minutes=minutes_from_now)
    end = start + timedelta(minutes=duration_min)
    return {
        "id": "evt-1",
        "summary": title,
        "start": {"dateTime": start.isoformat()},
        "end": {"dateTime": end.isoformat()},
    }


def _outlook_event(title: str, minutes_from_now: float, duration_min: int = 60) -> dict:
    start = NOW + timedelta(minutes=minutes_from_now)
    end = start + timedelta(minutes=duration_min)
    return {
        "id": "evt-2",
        "subject": title,
        "start": {"dateTime": start.isoformat()},
        "end": {"dateTime": end.isoformat()},
        "location": {"displayName": ""},
        "bodyPreview": "",
    }


def _google_task(title: str, due_offset_min: float | None = None, completed: bool = False) -> dict:
    due = None
    if due_offset_min is not None:
        due = (NOW + timedelta(minutes=due_offset_min)).isoformat()
    return {
        "id": "task-1",
        "title": title,
        "status": "completed" if completed else "needsAction",
        "due": due,
    }


def _outlook_task(title: str, due_offset_min: float | None = None, completed: bool = False) -> dict:
    due_dict = None
    if due_offset_min is not None:
        due_dict = {"dateTime": (NOW + timedelta(minutes=due_offset_min)).isoformat()}
    return {
        "id": "task-2",
        "title": title,
        "status": "completed" if completed else "notStarted",
        "dueDateTime": due_dict,
    }


# ── Google Calendar parser ─────────────────────────────────────────────────────

class TestParseGoogleEvents:
    def test_parses_basic_event(self):
        raw = {"items": [_google_event("Standup", 10)]}
        events = parse_google_events(raw)
        assert len(events) == 1
        assert events[0].title == "Standup"
        assert events[0].source == CalendarSource.GOOGLE

    def test_empty_items(self):
        assert parse_google_events({"items": []}) == []
        assert parse_google_events({}) == []

    def test_skips_event_without_start(self):
        raw = {"items": [{"id": "x", "summary": "Bad", "end": {"dateTime": NOW.isoformat()}}]}
        assert parse_google_events(raw) == []

    def test_accepts_data_wrapper(self):
        """Composio sometimes wraps response in a 'data' key."""
        raw = {"data": {"items": [_google_event("Wrapped", 5)]}}
        events = parse_google_events(raw)
        assert len(events) == 1

    def test_z_suffix_parsed_as_utc(self):
        raw = {"items": [{
            "id": "z",
            "summary": "UTC event",
            "start": {"dateTime": "2026-04-15T10:00:00Z"},
            "end": {"dateTime": "2026-04-15T11:00:00Z"},
        }]}
        events = parse_google_events(raw)
        assert events[0].start.tzinfo is not None


# ── Outlook Calendar parser ────────────────────────────────────────────────────

class TestParseOutlookEvents:
    def test_parses_basic_event(self):
        raw = {"value": [_outlook_event("Reunião", 15)]}
        events = parse_outlook_events(raw)
        assert len(events) == 1
        assert events[0].title == "Reunião"
        assert events[0].source == CalendarSource.OUTLOOK

    def test_empty_value(self):
        assert parse_outlook_events({"value": []}) == []

    def test_accepts_data_wrapper(self):
        raw = {"data": {"value": [_outlook_event("Wrapped", 5)]}}
        events = parse_outlook_events(raw)
        assert len(events) == 1


# ── Google Tasks parser ────────────────────────────────────────────────────────

class TestParseGoogleTasks:
    def test_parses_pending_task(self):
        raw = {"items": [_google_task("Revisar PR")]}
        tasks = parse_google_tasks(raw)
        assert len(tasks) == 1
        assert tasks[0].title == "Revisar PR"

    def test_skips_completed_tasks(self):
        raw = {"items": [_google_task("Done", completed=True)]}
        assert parse_google_tasks(raw) == []

    def test_task_without_due_date(self):
        raw = {"items": [_google_task("No due")]}
        tasks = parse_google_tasks(raw)
        assert tasks[0].due is None
        assert not tasks[0].is_overdue


# ── Outlook Tasks parser ───────────────────────────────────────────────────────

class TestParseOutlookTasks:
    def test_parses_pending_task(self):
        raw = {"value": [_outlook_task("Deploy prod")]}
        tasks = parse_outlook_tasks(raw, list_name="Work")
        assert len(tasks) == 1
        assert tasks[0].list_name == "Work"

    def test_skips_completed_tasks(self):
        raw = {"value": [_outlook_task("Done", completed=True)]}
        assert parse_outlook_tasks(raw) == []


# ── CalendarSummary logic ──────────────────────────────────────────────────────

class TestCalendarSummary:
    def _make_event(self, minutes_from_now: float) -> dict:
        return _google_event("Test", minutes_from_now)

    def test_imminent_events_within_5_min(self):
        raw = {"items": [_google_event("Now!", 3), _google_event("Later", 30)]}
        events = parse_google_events(raw)
        summary = CalendarSummary(events=events)
        assert len(summary.imminent_events) == 1
        assert summary.imminent_events[0].title == "Now!"

    def test_no_imminent_events(self):
        raw = {"items": [_google_event("Later", 10)]}
        events = parse_google_events(raw)
        summary = CalendarSummary(events=events)
        assert summary.imminent_events == []

    def test_overdue_tasks(self):
        raw = {"items": [
            _google_task("Overdue", due_offset_min=-60),
            _google_task("Future", due_offset_min=60),
        ]}
        tasks = parse_google_tasks(raw)
        summary = CalendarSummary(tasks=tasks)
        assert len(summary.overdue_tasks) == 1
        assert summary.overdue_tasks[0].title == "Overdue"

    def test_past_event_not_imminent(self):
        raw = {"items": [_google_event("Past", -10)]}
        events = parse_google_events(raw)
        summary = CalendarSummary(events=events)
        assert summary.imminent_events == []


# ── Property tests ─────────────────────────────────────────────────────────────

iso_dt = st.datetimes(
    min_value=datetime(2020, 1, 1),
    max_value=datetime(2030, 12, 31),
    timezones=st.just(timezone.utc),
)


@given(
    title=st.text(min_size=1, max_size=100),
    minutes=st.floats(min_value=-1000, max_value=1000, allow_nan=False, allow_infinity=False),
)
@settings(max_examples=100)
def test_property_google_event_title_preserved(title: str, minutes: float):
    """Title is always preserved through parsing."""
    raw = {"items": [_google_event(title, minutes)]}
    events = parse_google_events(raw)
    assert len(events) == 1
    assert events[0].title == title


@given(n_events=st.integers(min_value=0, max_value=20))
@settings(max_examples=50)
def test_property_imminent_subset_of_events(n_events: int):
    """imminent_events is always a subset of all events."""
    items = [_google_event(f"evt-{i}", i * 3 - 10) for i in range(n_events)]
    events = parse_google_events({"items": items})
    summary = CalendarSummary(events=events)
    assert all(e in summary.events for e in summary.imminent_events)


@given(n_tasks=st.integers(min_value=0, max_value=20))
@settings(max_examples=50)
def test_property_overdue_subset_of_tasks(n_tasks: int):
    """overdue_tasks is always a subset of all tasks."""
    items = [_google_task(f"task-{i}", due_offset_min=i * 10 - 50) for i in range(n_tasks)]
    tasks = parse_google_tasks({"items": items})
    summary = CalendarSummary(tasks=tasks)
    assert all(t in summary.tasks for t in summary.overdue_tasks)


@given(n=st.integers(min_value=0, max_value=10))
@settings(max_examples=50)
def test_property_completed_tasks_never_overdue(n: int):
    """Completed tasks are never returned as overdue."""
    items = [_google_task(f"done-{i}", due_offset_min=-60, completed=True) for i in range(n)]
    tasks = parse_google_tasks({"items": items})
    summary = CalendarSummary(tasks=tasks)
    assert summary.overdue_tasks == []
