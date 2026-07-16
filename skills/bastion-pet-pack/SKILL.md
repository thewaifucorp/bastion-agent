---
name: bastion-pet-pack
description: Create, edit, or validate declarative animated pet packs for Bastion's terminal companion. Use when someone asks for a custom Bastion mascot, Tamagotchi, terminal pet, sprite states, RGB ASCII art, or a Provided::Ui pet asset for an extension pack.
---

# Bastion Pet Pack

Create terminal-native companion packs without executable code or ambient authority.

## Workflow

1. Scaffold a pack:

   ```bash
   python3 scripts/create_pet_pack.py publisher/name "Display Name" OUTPUT_DIR
   ```

2. Edit `OUTPUT_DIR/ui/pet.toml`. Read [references/schema.md](references/schema.md)
   before changing states, markup, dimensions, or timing.
3. Preserve all seven required states: `onboarding`, `guard`, `thinking`,
   `build`, `cabinet`, `success`, and `alert`.
4. Keep every frame at most 21 terminal columns by 6 rows. Use multiple frames
   for motion; never embed ANSI escapes, shell commands, scripts, URLs, or file
   includes.
5. Validate before handoff:

   ```bash
   python3 scripts/validate_pet_pack.py OUTPUT_DIR/ui/pet.toml
   ```

6. Preview in Bastion:

   ```bash
   BASTION_TUI_PET=OUTPUT_DIR/ui/pet.toml bastion
   ```

## Extension integration

Ship `ui/pet.toml` as a static `Provided::Ui("pet")` asset. Start from
`assets/pet-pack/extension.toml`; keep `permissions = {}`. A pet pack must not
request capabilities, filesystem, network, device, or memory authority.

Do not imply that rarity, level, or learned cosmetic abilities grant agent
permissions. Game progress is presentation state only.
