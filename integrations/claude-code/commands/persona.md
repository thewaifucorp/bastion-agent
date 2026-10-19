---
description: Switch the Bastion persona you act as (no name = back to the default)
argument-hint: "[persona]"
allowed-tools: Bash(bastion presence persona:*)
---
!`bastion presence persona $ARGUMENTS 2>&1`

Tell the user the result above in one line. From the next prompt on you act as that persona: Bastion sends its frame with that prompt.
