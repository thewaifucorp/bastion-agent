"""Data models for the calendar skill."""
from __future__ import annotations

from dataclasses import dataclass, field
from datetime import datetime
from enum import Enum


class CalendarSource(str, Enum):
    GOOGLE = "Google"
    OUTLOOK = "Outlook"


@dataclass
class CalendarEvent:
    id: str
    title: str
    start: datetime
    end: datetime
    source: CalendarSource
    location: str = ""
    description: str = ""

    @property
    def minutes_until(self) -> float:
        """Minutes from now until event starts. Negative if already started."""
        now = datetime.now(tz=self.start.tzinfo)
        return (self.start - now).total_seconds() / 60


@dataclass
class Task:
    id: str
    title: str
    source: CalendarSource
    due: datetime | None = None
    completed: bool = False
    list_name: str = ""

    @property
    def is_overdue(self) -> bool:
        if self.due is None or self.completed:
            return False
        now = datetime.now(tz=self.due.tzinfo)
        return self.due < now


@dataclass
class CalendarSummary:
    events: list[CalendarEvent] = field(default_factory=list)
    tasks: list[Task] = field(default_factory=list)
    errors: list[str] = field(default_factory=list)

    @property
    def imminent_events(self) -> list[CalendarEvent]:
        """Events starting in <= 5 minutes."""
        return [e for e in self.events if 0 <= e.minutes_until <= 5]

    @property
    def overdue_tasks(self) -> list[Task]:
        return [t for t in self.tasks if t.is_overdue]
