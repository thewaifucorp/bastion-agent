//! This daemon as the owner's **primary**: the hub its nodes dial into, the
//! event log its memory writes go to, the replicator and the reconciliation
//! of a returning ex-primary. The HTTP surface lives in [`super::routes`].

use std::sync::Arc;

use bastion_mesh::devices::fence::EpochFence;
use bastion_mesh::devices::log::{EventLog, LoggedMemory};
use bastion_mesh::devices::reconcile::{reconcile, ConflictQueue};
use bastion_mesh::devices::{
    DeviceId, DeviceRegistry, Enrollment, HubEvent, Platform, PrimaryHub, RemoteCapability, Role,
};
use bastion_mesh::identity::age_identity::AgeIdentity;
use bastion_runtime::capability::Capability;
use tokio::sync::RwLock;

use super::catalog;
use super::pending::Pending;
use super::state::{DeviceState, LocalRole};

/// First-time setup of this installation as the owner's primary: the owner
/// key, this device's identity, and a registry with only this device at
/// epoch 1.
pub fn init_primary(
    state: &DeviceState,
    owner: &str,
    device: DeviceId,
    address: Option<String>,
) -> anyhow::Result<DeviceRegistry> {
    if state.registry()?.is_some() {
        anyhow::bail!("{} already holds a device registry", state.dir().display());
    }
    let owner_key = state.create_owner_identity()?;
    let identity = state.device_identity()?;
    let enrollment = Enrollment::new(
        owner,
        device,
        identity.verifying_key_bytes(),
        Platform::current(),
        false,
    )
    .sign(&owner_key);
    let registry = DeviceRegistry::bootstrap(owner_key.verifying_key_bytes(), enrollment, address)?;
    state.save_registry(&registry)?;
    state.set_role(LocalRole::Primary { epoch: 1 })?;
    Ok(registry)
}

pub struct PrimaryDevices {
    pub(crate) state: DeviceState,
    pub(crate) device: DeviceId,
    pub(crate) hub: PrimaryHub,
    pub(crate) log: Arc<EventLog>,
    pub(crate) fence: Arc<EpochFence>,
    pub(crate) conflicts: ConflictQueue,
    pub(crate) owner: Option<AgeIdentity>,
    pub(crate) identity: AgeIdentity,
    pub(crate) registry: Arc<RwLock<DeviceRegistry>>,
    pub(crate) pending: Pending,
    memory: std::sync::Mutex<Option<bastion_memory::SharedMemory>>,
}

impl PrimaryDevices {
    /// Load this device as primary, if the state says it is one. `db_path`
    /// is the daemon's database (the event log and conflicts live there).
    pub fn load(state: DeviceState, db_path: &str) -> anyhow::Result<Option<Arc<Self>>> {
        let Some(LocalRole::Primary { epoch }) = state.role()? else {
            return Ok(None);
        };
        let registry = state.registry()?.ok_or_else(|| {
            anyhow::anyhow!("primary without a registry in {}", state.dir().display())
        })?;
        let identity = state.device_identity()?;
        let device = registry
            .devices()
            .find(|r| r.enrollment.device_key == identity.verifying_key_bytes())
            .map(|r| r.enrollment.device.clone())
            .ok_or_else(|| anyhow::anyhow!("this device is not in its own registry"))?;
        let fence = Arc::new(EpochFence::new(epoch));
        if registry.current_epoch() > epoch {
            // The registry already knows a newer primary: never write.
            fence.observe(registry.current_epoch());
        }
        let log = Arc::new(EventLog::open(db_path, device.clone(), fence.clone())?);
        let conflicts = ConflictQueue::open(db_path)?;
        let registry = Arc::new(RwLock::new(registry));
        let hub = PrimaryHub::new(
            device.clone(),
            AgeIdentity::from_bech32(identity.age_secret_bech32())?,
            registry.clone(),
            fence.clone(),
        );
        let pending = Pending::open(&state)?;
        Ok(Some(Arc::new(Self {
            owner: state.owner_identity()?,
            state,
            device,
            hub,
            log,
            fence,
            conflicts,
            identity,
            registry,
            pending,
            memory: std::sync::Mutex::new(None),
        })))
    }

    pub fn hub(&self) -> &PrimaryHub {
        &self.hub
    }

    pub fn device(&self) -> &DeviceId {
        &self.device
    }

    pub fn fence(&self) -> &Arc<EpochFence> {
        &self.fence
    }

    /// The daemon's memory, with every belief write logged for replicas.
    pub fn wrap_memory(
        &self,
        inner: Box<dyn bastion_memory::Memory>,
    ) -> Box<dyn bastion_memory::Memory> {
        Box::new(LoggedMemory::new(inner, self.log.clone()))
    }

    /// Hand the shared memory over, for reconciliation to write into.
    pub fn attach_memory(&self, memory: bastion_memory::SharedMemory) {
        *self.memory.lock().unwrap_or_else(|p| p.into_inner()) = Some(memory);
    }

    /// A remote tool for every capability granted to every active node —
    /// registered once at startup, so the tool list (the cached prompt
    /// prefix) stays stable. A grant revoked later takes effect at once (the
    /// hub checks the grant on every call); a new grant, at the next start.
    pub async fn remote_capabilities(&self) -> Vec<Arc<dyn Capability>> {
        let known = catalog::descriptors();
        let registry = self.registry.read().await;
        let mut tools: Vec<Arc<dyn Capability>> = Vec::new();
        for record in registry.devices() {
            if record.revoked || record.enrollment.device == self.device {
                continue;
            }
            for grant in &record.enrollment.granted {
                if let Some(descriptor) = known.iter().find(|d| d.name == grant.capability) {
                    tools.push(Arc::new(RemoteCapability::new(
                        self.hub.clone(),
                        record.enrollment.device.clone(),
                        descriptor.clone(),
                        grant.clone(),
                    )));
                }
            }
        }
        tools
    }

    /// Persist the registry after a change and tell the connected nodes.
    pub(crate) async fn registry_changed(&self) -> anyhow::Result<()> {
        let snapshot = self.registry.read().await.clone();
        self.state.save_registry(&snapshot)?;
        self.hub.push_registry().await;
        Ok(())
    }

    /// Run the replicator and react to what happens on the hub: a node that
    /// saw a newer epoch closes our fence (BMD-21); an ex-primary that comes
    /// back is asked for what it wrote (§5.5).
    pub fn spawn_background(self: &Arc<Self>) {
        bastion_mesh::devices::replicate::spawn(self.hub.clone(), self.log.clone());
        let me = self.clone();
        let mut events = self.hub.subscribe();
        tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(event) => me.on_hub_event(event).await,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
        });
    }

    async fn on_hub_event(&self, event: HubEvent) {
        match event {
            HubEvent::NewerEpochSeen { device, epoch } => {
                tracing::error!(
                    event = "devices_superseded",
                    seen_by = %device,
                    epoch,
                    "epoch {epoch} exists: this device is no longer the primary and stops \
                     accepting writes; run `bastion node run` here"
                );
                if let Err(e) = self.state.set_role(LocalRole::Node) {
                    tracing::warn!(event = "devices_role_not_saved", error = %e);
                }
            }
            HubEvent::Connected { device } => self.maybe_request_proposals(&device).await,
            HubEvent::Proposals {
                device,
                epoch,
                events,
            } => self.reconcile_from(&device, epoch, events).await,
            other => tracing::debug!(event = "devices_hub_event", detail = ?other),
        }
    }

    /// An epoch whose primary was `device` and that nobody reconciled yet:
    /// ask for what it wrote after the next epoch began.
    async fn maybe_request_proposals(&self, device: &DeviceId) {
        let registry = self.registry.read().await;
        let done = self.pending.reconciled_epochs();
        let mine = registry.epochs().iter().filter(|e| e.primary == *device);
        for start in mine {
            if done.contains(&start.epoch) || start.epoch >= registry.current_epoch() {
                continue;
            }
            if let Some(next) = registry.epoch_start(start.epoch + 1) {
                self.hub
                    .request_proposals(device, start.epoch, next.after_seq)
                    .await;
            }
        }
    }

    async fn reconcile_from(
        &self,
        device: &DeviceId,
        epoch: u64,
        events: Vec<bastion_mesh::devices::replica::MemoryEvent>,
    ) {
        let memory = self
            .memory
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let Some(memory) = memory else {
            tracing::warn!(event = "devices_reconcile_no_memory");
            return;
        };
        let guard = memory.read().await;
        match reconcile(guard.as_ref(), &self.log, &self.conflicts, epoch, events).await {
            Ok(report) => {
                tracing::info!(
                    event = "devices_reconciled",
                    from = %device,
                    epoch,
                    applied = report.applied.len(),
                    conflicts = report.conflicts.len(),
                    already_known = report.already_known
                );
                if let Err(e) = self.pending.mark_reconciled(epoch) {
                    tracing::warn!(event = "devices_reconcile_not_saved", error = %e);
                }
            }
            Err(e) => {
                tracing::error!(event = "devices_reconcile_failed", from = %device, error = %e)
            }
        }
    }

    /// What the registry says about who is primary, for clients (§5.6).
    pub async fn primary_info(&self) -> PrimaryInfo {
        let registry = self.registry.read().await;
        let (record, epoch) = registry
            .primary()
            .map(|(r, e)| (Some(r.clone()), e))
            .unwrap_or((None, 0));
        PrimaryInfo {
            device: record.as_ref().map(|r| r.enrollment.device.clone()),
            address: record.and_then(|r| r.address),
            epoch,
            this_device: self.device.clone(),
            this_device_is_primary: self.fence.check().is_ok()
                && matches!(
                    registry.get(&self.device).map(|r| r.role),
                    Some(Role::Primary { .. })
                ),
        }
    }
}

/// Owner-facing operations, all guarded by the daemon token at the route.
impl PrimaryDevices {
    pub fn pending(&self) -> &Pending {
        &self.pending
    }

    pub fn conflicts(&self) -> &ConflictQueue {
        &self.conflicts
    }

    pub fn registry_handle(&self) -> &Arc<RwLock<DeviceRegistry>> {
        &self.registry
    }

    /// The owner's public key — a node needs it to verify this primary.
    pub fn owner_public_key(&self) -> anyhow::Result<[u8; 32]> {
        self.registry
            .try_read()
            .map(|r| *r.owner_key())
            .map_err(|_| anyhow::anyhow!("registry busy"))
    }

    fn owner(&self) -> anyhow::Result<&AgeIdentity> {
        self.owner.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "this device does not hold the owner key, so it cannot sign enrollments"
            )
        })
    }

    /// Admit a pending request the owner approved (BMD-08): build an
    /// owner-signed enrollment plus this primary's approval, and register it.
    pub async fn approve_request(&self, id: &str) -> anyhow::Result<()> {
        let request = self
            .pending
            .request(id)
            .ok_or_else(|| anyhow::anyhow!("no such enrollment request"))?;
        let owner = self.owner()?;
        let device_key = decode_key(&request.device_key)?;
        let enrollment = Enrollment::new(
            {
                let r = self.registry.read().await;
                r.owner().to_string()
            },
            request.device.clone(),
            device_key,
            request.platform,
            request.holds_replica,
        )
        .sign(owner);
        let approval = bastion_mesh::devices::EnrollmentApproval::sign(
            self.device.clone(),
            &self.identity,
            &enrollment,
        );
        self.registry.write().await.admit(enrollment, &approval)?;
        self.pending
            .set_status(id, super::pending::RequestStatus::Approved)?;
        self.registry_changed().await?;
        Ok(())
    }

    pub fn refuse_request(&self, id: &str) -> anyhow::Result<()> {
        self.pending
            .set_status(id, super::pending::RequestStatus::Refused)?;
        Ok(())
    }

    /// Replace a device's grants with an owner-signed newer revision
    /// (BMD-03, BMD-10). The primary may only harden `needs_approval`.
    pub async fn set_grants(
        &self,
        device: &DeviceId,
        grants: Vec<bastion_mesh::devices::CapabilityGrant>,
    ) -> anyhow::Result<()> {
        let owner = self.owner()?;
        let mut enrollment = {
            let r = self.registry.read().await;
            r.active(device)?.enrollment.clone()
        };
        enrollment.granted = grants;
        enrollment.revision += 1;
        let enrollment = enrollment.sign(owner);
        self.registry.write().await.update(enrollment)?;
        self.state
            .save_registry(&self.registry.read().await.clone())?;
        self.hub.push_grants(device).await;
        Ok(())
    }

    pub async fn set_address(
        &self,
        device: &DeviceId,
        address: Option<String>,
    ) -> anyhow::Result<()> {
        self.registry.write().await.set_address(device, address)?;
        self.registry_changed().await?;
        Ok(())
    }

    /// Revoke a device (BMD-33): it can no longer connect; the returned
    /// secret grants are what the owner must rotate. The device is told and
    /// dropped if connected.
    pub async fn revoke_device(
        &self,
        device: &DeviceId,
    ) -> anyhow::Result<Vec<bastion_mesh::devices::SecretGrant>> {
        let rotate = self.registry.write().await.revoke(device)?;
        self.registry_changed().await?;
        self.hub.notify_revoked(device).await;
        Ok(rotate)
    }

    pub async fn resolve_conflict(
        &self,
        id: i64,
        resolution: bastion_mesh::devices::reconcile::Resolution,
    ) -> anyhow::Result<()> {
        let memory = self
            .memory
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .ok_or_else(|| anyhow::anyhow!("memory not attached"))?;
        let guard = memory.read().await;
        self.conflicts
            .resolve(id, resolution, guard.as_ref(), &self.log)
            .await?;
        Ok(())
    }
}

fn decode_key(encoded: &str) -> anyhow::Result<[u8; 32]> {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|e| anyhow::anyhow!("device key: {e}"))?
        .try_into()
        .map_err(|_| anyhow::anyhow!("device key must be 32 bytes"))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PrimaryInfo {
    pub device: Option<DeviceId>,
    pub address: Option<String>,
    pub epoch: u64,
    pub this_device: DeviceId,
    pub this_device_is_primary: bool,
}
