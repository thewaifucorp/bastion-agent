//! `bastion node …`: this installation as a **node** of the owner's primary.
//!
//! - `pair` — generate this device's identity, ask the primary to join with
//!   a pairing code, wait for the owner to approve, save the node state.
//! - `run` — dial the primary and serve until stopped (BMD-09); runs only
//!   the confined primitives ([`super::catalog`]).
//! - `promote` — turn this device's replica into a live primary (BMD-19); the
//!   owner runs it here, locally, and it never happens on its own (BMD-20).

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use bastion_mesh::devices::fence::EpochFence;
use bastion_mesh::devices::log::EventLog;
use bastion_mesh::devices::replica_store::ReplicaStore;
use bastion_mesh::devices::{transport, DeviceId, NodeAgent, NodeConfig, Platform, SessionEnd};
use bastion_mesh::identity::age_identity::AgeIdentity;
use serde::Deserialize;

use super::state::{DeviceState, LocalRole};
use super::vault;
use crate::config::DevicesConfig;

/// Pair this device with a primary at `primary_url` using `code`.
pub async fn pair(cfg: &DevicesConfig, primary_url: &str, code: &str) -> anyhow::Result<()> {
    let state = DeviceState::open(cfg.state_dir())?;
    if state.role()?.is_some() {
        anyhow::bail!("this installation is already set up as a device");
    }
    let identity = state.device_identity()?;
    let device = super::state::default_device_id();
    let device_key =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(identity.verifying_key_bytes());
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let base = primary_url.trim_end_matches('/');

    #[derive(Deserialize)]
    struct Accepted {
        request_id: String,
    }
    let accepted: Accepted = http
        .post(format!("{base}/devices/enroll"))
        .json(&serde_json::json!({
            "code": code,
            "device": device.as_str(),
            "device_key": device_key,
            "platform": Platform::current(),
            "holds_replica": true,
        }))
        .send()
        .await?
        .error_for_status()
        .map_err(|e| anyhow::anyhow!("the primary refused the pairing: {e}"))?
        .json()
        .await?;

    println!("Waiting for the owner to approve this device on the primary…");
    #[derive(Deserialize)]
    struct Status {
        status: String,
        owner_key: Option<String>,
        primary_address: Option<String>,
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(600);
    loop {
        if std::time::Instant::now() > deadline {
            anyhow::bail!("timed out waiting for approval");
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
        let status: Status = http
            .get(format!("{base}/devices/enroll/{}", accepted.request_id))
            .send()
            .await?
            .json()
            .await?;
        match status.status.as_str() {
            "approved" => {
                let owner_key = status
                    .owner_key
                    .ok_or_else(|| anyhow::anyhow!("primary approved without an owner key"))?;
                let owner_key =
                    base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(&owner_key)?;
                std::fs::write(state.path("owner.pub"), &owner_key)?;
                let address = status.primary_address.unwrap_or_else(|| base.to_string());
                std::fs::write(state.path("primary.url"), &address)?;
                state.write_json("device.id", &device)?;
                state.set_role(LocalRole::Node)?;
                println!("Approved. This device is now a node. Run `bastion node run` to serve.");
                return Ok(());
            }
            "refused" => anyhow::bail!("the owner refused this device"),
            _ => {}
        }
    }
}

/// Promote this device to primary (BMD-19): rebuild its replica into the
/// daemon's memory and take the next epoch. Run locally by the owner, never
/// automatically (BMD-20). The promoted device serves its nodes and
/// reconciles the old primary; to enroll new devices or change grants it also
/// needs the owner key placed here (the owner does that deliberately — a
/// revoked or lost primary must not carry it around).
pub async fn promote(cfg: &DevicesConfig, db_path: &str) -> anyhow::Result<()> {
    let state = DeviceState::open(cfg.state_dir())?;
    if !matches!(state.role()?, Some(LocalRole::Node)) {
        anyhow::bail!("only a node can be promoted");
    }
    let device: DeviceId = state
        .read_json("device.id")?
        .ok_or_else(|| anyhow::anyhow!("node state is missing its device id"))?;
    let node_state = state
        .read_json::<bastion_mesh::devices::node::NodeState>("node.json")?
        .ok_or_else(|| anyhow::anyhow!("this node has no saved state to promote"))?;
    let mut registry = node_state
        .registry
        .ok_or_else(|| anyhow::anyhow!("this node never received the owner's registry"))?;

    let vault = vault::default_vault(&state)?;
    let replica_key = vault::replica_key(&*vault, device.as_str())?;
    let replica = ReplicaStore::open(state.path("replica.bin"), replica_key)?;
    let last_seq = bastion_mesh::devices::ReplicaSink::last_seq(&replica)
        .await
        .unwrap_or(0);

    let epoch = registry.promote(&device, last_seq)?;
    let fence = Arc::new(EpochFence::new(epoch));
    let log = EventLog::open(db_path, device.clone(), fence)?;
    let memory = bastion_memory::sqlite::SqliteMemory::new(db_path);
    let rest = replica.materialize(&memory, &log).await?;

    state.save_registry(&registry)?;
    state.set_role(LocalRole::Primary { epoch })?;
    println!(
        "Promoted to primary at epoch {epoch}. Rebuilt the replica ({} non-belief events for the \
         host to restore). Restart the daemon here to serve as primary.",
        rest.len()
    );
    if state.owner_identity()?.is_none() {
        println!(
            "Note: the owner key is not on this device, so it cannot enroll new devices or change \
             grants until you place it here. Existing nodes and reconciliation work without it."
        );
    }
    Ok(())
}

struct Loaded {
    state: DeviceState,
    device: DeviceId,
    owner_key: [u8; 32],
    primary_url: String,
    identity: AgeIdentity,
}

fn load(cfg: &DevicesConfig) -> anyhow::Result<Loaded> {
    let state = DeviceState::open(cfg.state_dir())?;
    if !matches!(state.role()?, Some(LocalRole::Node)) {
        anyhow::bail!("this installation is not a node; run `bastion node pair` first");
    }
    let device: DeviceId = state
        .read_json("device.id")?
        .ok_or_else(|| anyhow::anyhow!("node state is missing its device id"))?;
    let owner_key: [u8; 32] = std::fs::read(state.path("owner.pub"))?
        .try_into()
        .map_err(|_| anyhow::anyhow!("stored owner key is not 32 bytes"))?;
    let primary_url = std::fs::read_to_string(state.path("primary.url"))?
        .trim()
        .to_string();
    let identity = state.device_identity()?;
    Ok(Loaded {
        state,
        device,
        owner_key,
        primary_url,
        identity,
    })
}

fn node_url(primary_url: &str) -> String {
    let base = primary_url.trim_end_matches('/');
    if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}/node")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}/node")
    } else {
        format!("{base}/node")
    }
}

fn tls_config(cfg: &DevicesConfig) -> anyhow::Result<Arc<rustls::ClientConfig>> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if let Some(ca) = &cfg.ca_file {
        let pem = std::fs::read(ca)?;
        for cert in rustls_pemfile::certs(&mut pem.as_slice()) {
            roots.add(cert?)?;
        }
    }
    Ok(Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    ))
}

/// Serve the primary until stopped. Reconnects with backoff after a drop;
/// a refusal (bad epoch, revoked, another owner) stops for good.
pub async fn run(cfg: &DevicesConfig) -> anyhow::Result<()> {
    let loaded = load(cfg)?;
    let vault = vault::default_vault(&loaded.state)?;
    let replica_key = vault::replica_key(&*vault, loaded.device.as_str())?;
    let replica = Arc::new(ReplicaStore::open(
        loaded.state.path("replica.bin"),
        replica_key,
    )?);
    // The owner id is not secret; it travels with the primary's frames and
    // the node keeps its registry copy. Use that copy when present.
    let owner = loaded
        .state
        .read_json::<bastion_mesh::devices::node::NodeState>("node.json")?
        .and_then(|s| s.registry.map(|r| r.owner().to_string()))
        .unwrap_or_default();
    let mut agent = NodeAgent::new(NodeConfig {
        owner,
        owner_key: loaded.owner_key,
        device: loaded.device.clone(),
        identity: loaded.identity,
        state_path: Some(loaded.state.path("node.json")),
    })?
    .with_replica(replica);
    for cap in super::catalog::node_capabilities() {
        agent = agent.with_capability(cap);
    }
    let node = Arc::new(agent);
    let tls = tls_config(cfg)?;
    let url = node_url(&loaded.primary_url);
    let handle = node.handle();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            handle.stop();
        }
    });

    let mut backoff = Duration::from_secs(1);
    loop {
        let tls = if url.starts_with("wss://") {
            Some(tls.clone())
        } else if cfg.allow_plain_transport {
            None
        } else {
            anyhow::bail!("refusing a plain ws:// primary; set devices.allow_plain_transport for a tailnet or loopback");
        };
        match transport::connect(&url, tls).await {
            Ok(conn) => {
                backoff = Duration::from_secs(1);
                match node.run(conn).await {
                    SessionEnd::Stopped => {
                        println!("Stopped.");
                        return Ok(());
                    }
                    SessionEnd::Revoked => {
                        anyhow::bail!("this device was revoked by the owner");
                    }
                    SessionEnd::Refused(reason) => {
                        anyhow::bail!("the primary refused this device: {reason}");
                    }
                    SessionEnd::Disconnected(reason) => {
                        tracing::warn!(event = "node_disconnected", reason = %reason);
                    }
                }
            }
            Err(e) => tracing::warn!(event = "node_connect_failed", error = %e),
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}
