# Personas and Cabinet

Personas give one Bastion instance distinct, reviewable perspectives for the different domains of a life: work, health, relationships, learning, finances, or a specific project. They are not separate bots and they do not bypass the runtime’s identity, capability, or privacy boundaries.

In the Compose deployment, the repository’s `personas/` directory is mounted read-only into the core container. Treat persona files as policy: review changes, keep secrets out of them, and do not allow untrusted conversation content to rewrite them.

## Cabinet

The console command below convenes named personas for the next eligible Cabinet deliberation:

```text
/cabinet <persona1> [persona2 ...]
```

Cabinet is for trade-offs, not fake consensus. It can preserve dissent while producing a synthesized recommendation, making it useful when competing priorities need to be made explicit and reconsidered.

Examples:

```text
/cabinet career health finance
/cabinet project-owner tech-lead
```

Use personas to give context a durable home. Use Cabinet when those contexts should disagree before you decide.
