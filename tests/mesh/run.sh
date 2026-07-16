#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)"
COMPOSE_FILE="$ROOT/tests/mesh/docker-compose.yml"
PROJECT_NAME="bastion-mesh-e2e"
TMP_DIR="$(mktemp -d)"
HELPER_DIR="$TMP_DIR/mesh-helper"

cleanup() {
  docker compose -p "$PROJECT_NAME" -f "$COMPOSE_FILE" down -v >/dev/null 2>&1 || true
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT

need_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "missing required command: $1" >&2
    exit 127
  fi
}

need_cmd cargo
need_cmd curl
need_cmd docker

(cd "$ROOT" && cargo test mesh::allowlist --lib >/dev/null)

mkdir -p "$HELPER_DIR/src"
cat >"$HELPER_DIR/Cargo.toml" <<'TOML'
[package]
name = "mesh-e2e-helper"
version = "0.1.0"
edition = "2021"

[dependencies]
age = "0.11"
serde_json = "1"
TOML
cat >"$HELPER_DIR/src/main.rs" <<'RS'
use age::secrecy::ExposeSecret as _;
use std::{env, fs, path::Path};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("keygen") => {
            let identity = age::x25519::Identity::generate();
            println!("{}", identity.to_string().expose_secret());
            println!("{}", identity.to_public());
        }
        Some("envelope") => {
            let from_owner = args.next().ok_or("missing from_owner")?;
            let to_owner = args.next().ok_or("missing to_owner")?;
            let recipient_pub = args.next().ok_or("missing recipient_pub")?;
            let content = args.next().ok_or("missing content")?;
            let out = args.next().ok_or("missing out")?;
            let payload = serde_json::json!({
                "from_owner": from_owner,
                "beliefs": [{
                    "id": 1,
                    "owner_id": from_owner,
                    "persona_tag": "test",
                    "content": content,
                    "weight": 1.0,
                    "is_core": false,
                    "tier": "cloud-ok"
                }]
            });
            let recipient: age::x25519::Recipient = recipient_pub.parse()?;
            let plaintext = serde_json::to_vec(&payload)?;
            let ciphertext = age::encrypt(&recipient, &plaintext)?;
            let envelope = serde_json::json!({
                "from_owner": from_owner,
                "to_owner": to_owner,
                "ciphertext": ciphertext,
                "recipient_hint": recipient_pub
            });
            fs::write(Path::new(&out), serde_json::to_vec(&envelope)?)?;
        }
        _ => return Err("usage: keygen | envelope <from> <to> <recipient_pub> <content> <out>".into()),
    }
    Ok(())
}
RS

helper() {
  cargo run --quiet --manifest-path "$HELPER_DIR/Cargo.toml" -- "$@"
}

gen_key() {
  local name="$1"
  helper keygen >"$TMP_DIR/$name.keypair"
  sed -n '1p' "$TMP_DIR/$name.keypair" >"$TMP_DIR/$name.key"
  sed -n '2p' "$TMP_DIR/$name.keypair" >"$TMP_DIR/$name.pub"
}

gen_key alice
gen_key bob

ALICE_AGE_KEY="$(cat "$TMP_DIR/alice.key")"
BOB_AGE_KEY="$(cat "$TMP_DIR/bob.key")"
ALICE_PUB="$(cat "$TMP_DIR/alice.pub")"
BOB_PUB="$(cat "$TMP_DIR/bob.pub")"

sed "s/BOB_AGE_PUBKEY_PLACEHOLDER/$BOB_PUB/g" "$ROOT/tests/mesh/nodes/alice.toml" >"$TMP_DIR/bastion-a.toml"
sed "s/ALICE_AGE_PUBKEY_PLACEHOLDER/$ALICE_PUB/g" "$ROOT/tests/mesh/nodes/bob.toml" >"$TMP_DIR/bastion-b.toml"

export ALICE_AGE_KEY BOB_AGE_KEY
export BASTION_A_CONFIG="$TMP_DIR/bastion-a.toml"
export BASTION_B_CONFIG="$TMP_DIR/bastion-b.toml"

make_envelope() {
  local from_owner="$1"
  local to_owner="$2"
  local recipient_pub="$3"
  local content="$4"
  local out="$5"
  helper envelope "$from_owner" "$to_owner" "$recipient_pub" "$content" "$out"
}

wait_for_agent_card() {
  local url="$1"
  local name="$2"
  for _ in $(seq 1 60); do
    if curl -fsS --max-time 2 "$url/agent-card" >/dev/null 2>&1; then
      echo "OK: $name ready"
      return 0
    fi
    sleep 2
  done
  echo "timeout waiting for $name" >&2
  docker compose -p "$PROJECT_NAME" -f "$COMPOSE_FILE" logs --tail=80 >&2 || true
  exit 1
}

post_mesh() {
  local url="$1"
  local token="$2"
  local file="$3"
  curl -sS -o "$TMP_DIR/response.json" -w "%{http_code}"     -X POST "$url/mesh/ingest"     -H "x-bastion-token: $token"     -H "Content-Type: application/json"     --data-binary "@$file"
}

docker compose -p "$PROJECT_NAME" -f "$COMPOSE_FILE" up -d --build
wait_for_agent_card "http://localhost:18081" "bastion-a"
wait_for_agent_card "http://localhost:18082" "bastion-b"

echo "Test 1: bidirectional mesh ingest with real age envelopes"
make_envelope alice bob "$BOB_PUB" "alice to bob mesh belief" "$TMP_DIR/a-to-b.json"
code="$(post_mesh "http://localhost:18082" token-bob "$TMP_DIR/a-to-b.json")"
[[ "$code" == "200" ]] || { echo "A to B failed HTTP $code: $(cat "$TMP_DIR/response.json")" >&2; exit 1; }
make_envelope bob alice "$ALICE_PUB" "bob to alice mesh belief" "$TMP_DIR/b-to-a.json"
code="$(post_mesh "http://localhost:18081" token-alice "$TMP_DIR/b-to-a.json")"
[[ "$code" == "200" ]] || { echo "B to A failed HTTP $code: $(cat "$TMP_DIR/response.json")" >&2; exit 1; }
echo "OK: bidirectional ingest accepted"

echo "Test 2: cross-owner envelope rejection"
make_envelope alice eve "$BOB_PUB" "wrong owner" "$TMP_DIR/wrong-owner.json"
code="$(post_mesh "http://localhost:18082" token-bob "$TMP_DIR/wrong-owner.json")"
[[ "$code" == "403" ]] || { echo "cross-owner rejection failed HTTP $code" >&2; exit 1; }
echo "OK: cross-owner rejection works"

echo "Test 3: tag+tier filter"
(cd "$ROOT" && cargo test mesh::allowlist --lib >/dev/null)
echo "OK: tag+tier filter unit tests pass"

echo "Test 4: age key mismatch"
make_envelope alice bob "$ALICE_PUB" "wrong recipient key" "$TMP_DIR/age-mismatch.json"
code="$(post_mesh "http://localhost:18082" token-bob "$TMP_DIR/age-mismatch.json")"
[[ "$code" == "400" ]] || { echo "age mismatch expected 400, got HTTP $code" >&2; exit 1; }
echo "OK: age mismatch rejected"

echo "Test 5: peer offline does not crash local node"
docker compose -p "$PROJECT_NAME" -f "$COMPOSE_FILE" stop bastion-b >/dev/null
sleep 3
docker compose -p "$PROJECT_NAME" -f "$COMPOSE_FILE" ps bastion-a | grep -q "Up"
echo "OK: peer offline handled without crashing bastion-a"

echo "All mesh E2E tests passed"
