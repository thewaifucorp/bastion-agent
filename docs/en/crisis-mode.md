# Crisis Mode

## What is Crisis Mode?

Crisis Mode is a Bastion feature for real emergency situations — when something important broke, a deadline is approaching, or you need to free up time immediately to solve a problem.

When activated, Bastion:

1. Increases the priority of the affected persona
2. Analyzes your schedule and identifies commitments that can be moved or cancelled
3. Frees up at least 2 hours for you to focus on the problem
4. Shows you a summary of what was reorganized

---

## How to activate

You can activate Crisis Mode in two ways:

**Direct command:**
```
/crisis Production server is down
/crisis Presentation tomorrow and I haven't finished the deck
```

**Natural urgency language:**
```
"Everything is on fire here, our main client just cancelled the contract"
"Emergency — I need to deliver this today and I have no time"
```

Bastion automatically detects when a message indicates a real crisis (with high confidence) and activates the mode without needing the `/crisis` command.

---

## What happens next

When Crisis Mode is activated, Bastion runs the **sacrifice algorithm**:

1. Identifies which persona is being affected by the crisis
2. Increases that persona's weight by +0.3 (up to a maximum of 1.0)
3. Searches your schedule for tasks that can be moved — low-priority commitments that aren't urgent
4. Cancels or reschedules those tasks to free up at least 2 hours
5. Sends you a summary: what was moved, to when, and how much time was freed

Example response:

```
🚨 Crisis Mode activated — persona: Tech Lead

Freed up 2h30 in your schedule:
• Weekly alignment meeting → moved to Thursday at 2pm
• Documentation review → moved to Friday morning

You have 2pm to 4:30pm free today. You can focus on the server.
```

---

## What if there are no tasks to move?

If Bastion can't find enough commitments to free up 2 hours, it warns you and shows the available options — without doing anything without your confirmation.

---

## Deactivating Crisis Mode

Crisis Mode has no fixed duration. When the situation is resolved, the persona's weight returns to normal automatically at the next weekly review. You can also ask manually:

> "The crisis is over, you can normalize my schedule"

---

## Tip

Crisis Mode works best when you have Google Calendar connected. Without calendar integration, Bastion can still help prioritize tasks, but can't automatically reorganize appointments.
