---
description: Deny a pending Bastion approval by id
argument-hint: "<approval id>"
allowed-tools: Bash(bastion presence deny:*)
---
!`bastion presence deny $ARGUMENTS 2>&1`

Tell the user the result above in one line.
