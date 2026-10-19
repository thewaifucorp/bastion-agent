---
description: Show Bastion presence — agent sessions, pending approvals, active persona
allowed-tools: Bash(bastion presence status:*)
---
!`bastion presence status 2>&1`

Show the status above to the user as-is. If there are pending approvals, point out the ids and that `/bastion:approve <id>` or `/bastion:deny <id>` answers them.
