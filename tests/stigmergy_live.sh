#!/usr/bin/env bash
set -euo pipefail

echo "=== Stigmergy Live Validation (ORCH-05) ==="
echo
echo "Automated companion:"
echo "  cargo test stigmergy_mechanism --test -- --nocapture"
echo
echo "Validated mechanisms:"
echo "  - reinforce_belief: pheromone deposit increases an owner-scoped belief weight."
echo "  - evaporate_beliefs: pheromone evaporation decays active non-core belief weights."
echo "  - retrieval bias: MemoryRagProvider ranks equal-overlap beliefs by weight."
echo
echo "Manual validation: deposit_and_evaporate"
echo "  This checkout does not currently expose src/learn/mod.rs or Reflector::deposit_and_evaporate."
echo "  When the Reflector module is restored, validate that its quality score calls"
echo "  reinforce_belief, then evaporate_beliefs, and emits OTel/log evidence."
echo
echo "post-launch refinements:"
echo "  - tune rho/L_ref/gain parameters"
echo "  - add per-turn attribution"
echo "  - add harm-floor code-side policy"
echo "  - validate with real user data"
echo
echo "ORCH-05 validation procedure documented."
