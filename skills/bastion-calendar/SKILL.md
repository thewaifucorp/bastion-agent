---
name: bastion/calendar
version: 1.0.0
description: >
  Unified calendar and tasks skill. Fetches events and tasks from Google Calendar,
  Google Tasks, and Outlook via Composio. Used by HEARTBEAT for reminders and
  available for on-demand queries from the user.
triggers:
  - scheduled via HEARTBEAT every 30 minutes (calendar-check)
  - on-demand: user asks about agenda, events, tasks, schedule
composio_toolkits:
  - GOOGLECALENDAR
  - GOOGLETASKS
  - OUTLOOK
---

# Skill: bastion/calendar

## Objective

Fetch upcoming events and pending tasks from Google Calendar, Google Tasks, and
Outlook Calendar, then surface reminders and summaries to the user via Telegram.

---

## Composio Tools Used

### Google Calendar
- `GOOGLECALENDAR_EVENTS_LIST_ALL_CALENDARS` — list events across all calendars in a time window
- `GOOGLECALENDAR_EVENTS_LIST` — list events from a specific calendar
- `GOOGLECALENDAR_CREATE_EVENT` — create a new event
- `GOOGLECALENDAR_PATCH_EVENT` — update an existing event
- `GOOGLECALENDAR_DELETE_EVENT` — delete an event (requires confirmation)

### Google Tasks
- `GOOGLETASKS_LIST_ALL_TASKS` — list all tasks across all task lists
- `GOOGLETASKS_LIST_TASKS` — list tasks from a specific list
- `GOOGLETASKS_INSERT_TASK` — create a new task
- `GOOGLETASKS_PATCH_TASK` — update a task (e.g. mark complete)
- `GOOGLETASKS_DELETE_TASK` — delete a task (requires confirmation)

### Outlook Calendar
- `OUTLOOK_LIST_EVENTS` — list calendar events in a time window
- `OUTLOOK_GET_CALENDAR_VIEW` — get calendar view for a date range
- `OUTLOOK_CALENDAR_CREATE_EVENT` — create a new Outlook event
- `OUTLOOK_UPDATE_CALENDAR_EVENT` — update an existing Outlook event
- `OUTLOOK_DELETE_CALENDAR_EVENT` — delete an Outlook event (requires confirmation)

### Outlook Tasks (To Do)
- `OUTLOOK_LIST_TO_DO_LISTS` — list all To Do task lists
- `OUTLOOK_LIST_TODO_TASKS` — list tasks from a specific To Do list
- `OUTLOOK_CREATE_TASK` — create a new To Do task

---

## HEARTBEAT: calendar-check (every 30 min)

When triggered by the heartbeat, execute this sequence:

### Step 1 — Fetch upcoming events (next 60 minutes)

Call `GOOGLECALENDAR_EVENTS_LIST_ALL_CALENDARS` with:
- `timeMin`: now (ISO 8601, UTC)
- `timeMax`: now + 60 minutes (ISO 8601, UTC)
- `singleEvents`: true
- `orderBy`: startTime

Call `OUTLOOK_LIST_EVENTS` with:
- `$filter`: `start/dateTime ge '{now}' and start/dateTime le '{now+60min}'`
- `$orderby`: `start/dateTime`
- `$top`: 10

### Step 2 — Check for imminent events (≤ 5 minutes away)

For each event from both sources:
- If `start_time - now ≤ 5 minutes`: send **immediate reminder**
- Format: `🗓️ Em [X] minutos: [título do evento] — [horário]`
- Include source tag: `(Google)` or `(Outlook)`

### Step 3 — Fetch due/overdue tasks (once per hour, on the :00 run)

Call `GOOGLETASKS_LIST_ALL_TASKS` with:
- `dueMax`: now (ISO 8601)
- `showCompleted`: false
- `showHidden`: false

Call `OUTLOOK_LIST_TO_DO_LISTS` then `OUTLOOK_LIST_TODO_TASKS` for each list with:
- filter: tasks with `dueDateTime` ≤ now and `status` != `completed`

If any overdue tasks found, send:
`📋 Tarefas vencidas: [N] tarefa(s) pendente(s) — [título1], [título2]...`

---

## On-Demand: User Queries

When the user asks about their agenda, schedule, events, or tasks, use the
appropriate tools based on the query:

### "o que tenho hoje?" / "minha agenda"
1. `GOOGLECALENDAR_EVENTS_LIST_ALL_CALENDARS` — timeMin: start of today, timeMax: end of today
2. `OUTLOOK_LIST_EVENTS` — filter for today
3. `GOOGLETASKS_LIST_ALL_TASKS` — dueMax: end of today, showCompleted: false
4. `OUTLOOK_LIST_TODO_TASKS` — tasks due today
5. Summarize all results in a single message, grouped by source

### "próxima semana" / "semana"
- Use timeMin: start of next Monday, timeMax: end of next Sunday

### "criar evento [...]"
- Parse title, date/time, and optional description from user message
- Use `GOOGLECALENDAR_CREATE_EVENT` or `OUTLOOK_CALENDAR_CREATE_EVENT` based on context
- Confirm with user before creating: `Criar evento "[título]" em [data/hora]? (sim/não)`

### "criar tarefa [...]"
- Use `GOOGLETASKS_INSERT_TASK` or `OUTLOOK_CREATE_TASK`
- No confirmation needed for task creation

### "cancelar / deletar evento [...]"
- Find the event first, show details
- Confirm: `Vou deletar "[título]" em [data/hora]. Confirmar? (sim/não)`
- Only delete after explicit "sim"

---

## Response Format

Always respond in **pt-BR** (Mário's language).

### Agenda summary format:
```
📅 Agenda de [data]

🕐 [horário] — [título] (Google/Outlook)
🕑 [horário] — [título] (Google/Outlook)

📋 Tarefas do dia:
• [tarefa 1] (Google Tasks)
• [tarefa 2] (Outlook To Do)
```

### No events format:
```
✅ Nenhum evento ou tarefa para [período].
```

---

## Timezone

Always use `America/Recife` (UTC-3) for display. Convert all UTC timestamps
from the APIs before showing to the user.

---

## Error Handling

- If a Composio tool call fails (auth error, quota, etc.): log the error, skip
  that source, and continue with the others. Never fail silently without logging.
- If both Google and Outlook fail: notify the user once per hour max.
  Format: `⚠️ Não foi possível acessar o calendário agora. Tentarei novamente em breve.`
- Never expose raw API error messages to the user.
