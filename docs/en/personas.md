# Personas

## What is a persona?

A persona is a behavioral profile that Bastion uses to adapt to the context of your conversation. Each persona has a name, a domain, a tone of voice, and keywords that activate it automatically.

For example: if you have a "Tech Lead" persona with the keyword "PR", every time you mention "PR" or "code review" in a message, Bastion will respond with that persona's behavior and tone — more technical, focused on code and architecture.

You can have as many personas as you want. They can be active at the same time if a message touches multiple contexts.

---

## How personas are created

During onboarding (when you send `/start` for the first time), Bastion asks which areas of your life you want it to help with. For each area you provide, it automatically creates a persona.

If you said "work", "studies", and "health", Bastion creates three personas — one for each area — and already suggests relevant tools to install for each one.

---

## How to create a new persona

Just ask Bastion in natural language:

> "I want to create a new persona for my band"

Bastion will lead a short conversation asking:

1. **Name** of the persona (e.g., "Musician")
2. **Domain** — what this persona covers (e.g., rehearsals, composition, shows)
3. **Tone of voice** — formal or informal, direct or detailed
4. **Keywords** — words that activate this persona (e.g., "rehearsal", "music", "show", "chord")
5. **Base weight** — this persona's priority relative to others (0.0 to 1.0)

After confirming, Bastion creates the persona and it's immediately available.

---

## How to edit an existing persona

Ask directly:

> "I want to edit the Tech Lead persona"
> "Add the keyword 'deploy' to my work persona"
> "Change the Student persona's tone to more informal"

Bastion makes the change and confirms what was updated.

---

## How automatic activation works

Bastion analyzes each message you send and checks:

- If any word in the message matches the keywords of any persona
- The general context of the message (even without an exact keyword match)
- The time of day, if you configured schedules for any persona

If no persona is identified, Bastion uses the persona with the highest current weight as the default.

---

## Persona weights

Each persona has a weight (from 0.0 to 1.0) that represents its priority. The weight can temporarily increase in crisis situations (see [Crisis Mode](crisis-mode.md)) and is automatically adjusted based on your usage patterns over time.

You can also adjust manually:

> "Increase the CEO persona's weight to 0.8"
> "Decrease the English persona's priority"

---

## Tips

- Create specific personas, not generic ones. "Tech Lead" is better than "Work".
- Choose keywords you actually use day-to-day — words that appear naturally in your messages.
- If Bastion is frequently using the wrong persona, add more keywords to the correct persona or remove ambiguous keywords.
