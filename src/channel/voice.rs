// Local voice channel — cpal capture/playback + push-to-talk (VOICE-01).
//
// Architecture (D-12): voice is BOTH a dedicated `Channel` AND a consumer of two
// capabilities (`voice_transcribe`/`voice_speak`) that any other channel could
// also call. This file never embeds STT/TTS logic directly — it only invokes
// those capabilities through `CapabilityRegistry::invoke`, exactly like every
// other capability caller in this codebase (T-10-07-01).
//
// Privacy: every capability invocation in `handle_voice_turn` is tagged
// `PrivacyTier::LocalOnly` — this is VOICE-01's entire privacy promise ("áudio
// nunca sai para a nuvem"). It only compiles/passes correctly once the
// `voice_transcribe`/`voice_speak` adapters override `is_local() -> true`
// (Plan 10-08) — otherwise `CapabilityRegistry::invoke`'s `check_egress` call
// blocks every turn fail-closed (see `src/hooks/egress.rs`).
//
// Trigger: push-to-talk (default, always on) via `crossterm` SPACE key
// press/release. Wake-word (D-10, opt-in, default OFF) is a second, independent
// trigger that reuses the exact same `handle_voice_turn` core (Task 4).
use crate::channel::Channel;
use base64::Engine;
use bastion_memory::PrivacyTier;
use bastion_runtime::agent::handle::AgentHandle;
use bastion_runtime::capability::registry::{CapabilityRegistry, InvokeCtx};
use std::sync::{Arc, Mutex};

/// Pure capability-invocation chain for a single voice turn (VOICE-01).
///
/// record, then `voice_transcribe`, then `agent.ask`, then `voice_speak`, then the
/// caller decodes and plays back the result. Hardware-independent and fully
/// unit-testable — this is the load-bearing piece D-12 requires: the `Channel`
/// itself never embeds STT/TTS logic, it only calls through
/// `CapabilityRegistry::invoke` like any other capability caller.
///
/// SECURITY (T-10-07-01): both invocations below are tagged
/// `PrivacyTier::LocalOnly` — the entire privacy promise of this channel.
/// Neither `voice_transcribe` nor `voice_speak` sets `needs_approval()==true`
/// (SEC-01), so the approval gate is not a factor here. Returns `Err` (never
/// panics) on a malformed `voice_transcribe`/`voice_speak` response.
pub async fn handle_voice_turn(
    audio_b64_in: String,
    registry: &CapabilityRegistry,
    agent: &AgentHandle,
    owner: &str,
    voice_id: &str,
) -> anyhow::Result<String> {
    let ctx = InvokeCtx {
        owner: owner.to_string(),
        privacy_tier: Some(PrivacyTier::LocalOnly),
    };

    // Plan 11-07 (SEC-04): `.data` unwraps the raw capability output from the
    // `TaggedValue` wrapper — this is a direct capability-chain caller (not
    // dispatch_tool_loop's LLM-facing tool-result path), so no untrusted-result
    // envelope framing applies here, same as every other non-LLM-facing caller.
    let transcribe_result = registry
        .invoke(
            "voice_transcribe",
            serde_json::json!({ "audio_b64": audio_b64_in }),
            &ctx,
        )
        .await?
        .data;
    let text = transcribe_result["text"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("voice_transcribe returned no text field"))?;

    let reply = agent.ask(text.to_string(), owner.to_string()).await?;

    let speak_result = registry
        .invoke(
            "voice_speak",
            serde_json::json!({ "text": reply, "voice": voice_id }),
            &ctx,
        )
        .await?
        .data;
    let audio_b64_out = speak_result["audio_b64"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("voice_speak returned no audio_b64 field"))?;

    Ok(audio_b64_out.to_string())
}

// ─── Channel implementation ─────────────────────────────────────────────────

/// Local voice channel (VOICE-01).
///
/// Unlike every other channel in this phase, voice authenticates via local
/// mic/speaker hardware presence — there is no remote credential to check, so
/// (per 10-PATTERNS.md's Shared Patterns note) it needs no `OwnerMap`. The owner
/// is resolved once at construction via the same `BASTION_OWNER_ID`/`DEFAULT_OWNER`
/// fallback convention `daemon_loop`'s other local-only call sites already use.
pub struct VoiceChannel {
    pub(crate) owner: String,
    pub(crate) default_persona: Option<String>,
    pub(crate) voice_id: String,
    pub(crate) wake_word_enabled: bool,
    pub(crate) registry: Arc<CapabilityRegistry>,
}

impl VoiceChannel {
    /// Build a `VoiceChannel`. `voice_id` selects the TTS voice passed to
    /// `voice_speak` (e.g. Kokoro's `pf_dora`, native pt-BR). `wake_word_enabled`
    /// gates the opt-in wake-word trigger (D-10) — default OFF at the config layer
    /// (Plan 10-02's `VoiceChannelConfig`).
    pub fn new(
        registry: Arc<CapabilityRegistry>,
        voice_id: impl Into<String>,
        wake_word_enabled: bool,
    ) -> Self {
        let owner = std::env::var("BASTION_OWNER_ID")
            .unwrap_or_else(|_| bastion_runtime::agent::loop_::DEFAULT_OWNER.to_string());
        Self {
            owner,
            default_persona: None,
            voice_id: voice_id.into(),
            wake_word_enabled,
            registry,
        }
    }

    /// Set the default persona for this channel (CHAN-04).
    pub fn with_default_persona(mut self, persona: impl Into<String>) -> Self {
        self.default_persona = Some(persona.into());
        self
    }
}

#[async_trait::async_trait]
impl Channel for VoiceChannel {
    async fn run(self: Box<Self>, agent: AgentHandle) -> anyhow::Result<()> {
        let registry = self.registry.clone();
        let owner = self.owner.clone();
        let voice_id = self.voice_id.clone();

        // Wake-word (D-10, opt-in): Task 3's package-legitimacy checkpoint on
        // `rustpotter` was approved (pinned 3.0.2) — spawn the real
        // rustpotter-backed loop concurrently with push-to-talk. Both triggers
        // share the exact same `handle_voice_turn` core; only the trigger
        // differs. `wake_word_loop` itself logs a loud warning and returns (no
        // panic, no silent no-op) if the operator hasn't configured
        // `VOICE_WAKE_WORD_MODEL_PATH` or hardware/model loading fails.
        if self.wake_word_enabled {
            let registry = registry.clone();
            let agent = agent.clone();
            let owner = owner.clone();
            let voice_id = voice_id.clone();
            tokio::spawn(async move {
                if let Err(e) = wake_word_loop(registry, agent, owner, voice_id).await {
                    tracing::warn!(event = "voice_wake_word_loop_error", error = %e, "wake-word loop terminated");
                }
            });
        }
        push_to_talk_loop(registry, agent, owner, voice_id).await
    }

    fn default_persona(&self) -> Option<&str> {
        self.default_persona.as_deref()
    }
}

// ─── push-to-talk I/O loop (hardware-dependent, not unit-tested) ───────────

/// An in-progress push-to-talk (or wake-word) recording: a live `cpal` input
/// stream appending downmixed-to-mono `f32` samples into a shared buffer.
struct RecordingSession {
    stream: cpal::Stream,
    buffer: Arc<Mutex<Vec<f32>>>,
    sample_rate: u32,
}

impl RecordingSession {
    /// Stop the stream and take ownership of the recorded samples.
    fn stop(self) -> (Vec<f32>, u32) {
        let RecordingSession {
            stream,
            buffer,
            sample_rate,
        } = self;
        // Dropping the stream stops the capture thread (and, on the ALSA backend
        // used on Linux, joins it) before we read the buffer back out.
        drop(stream);
        let samples = match Arc::try_unwrap(buffer) {
            Ok(mutex) => mutex.into_inner().unwrap_or_default(),
            Err(arc) => arc.lock().map(|guard| guard.clone()).unwrap_or_default(),
        };
        (samples, sample_rate)
    }
}

/// Start recording from the default input device. Samples from all channels are
/// downmixed to mono (matches the plan's "mono" WAV encode requirement) — the
/// reported sample rate is read from the device's own default config, never
/// hardcoded (10-RESEARCH.md pitfall).
fn start_recording() -> anyhow::Result<RecordingSession> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::Sample;

    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| anyhow::anyhow!("no default audio input device"))?;
    let config = device.default_input_config()?;
    let channels = config.channels();
    let sample_rate = config.sample_rate();
    let sample_format = config.sample_format();
    let stream_config: cpal::StreamConfig = config.into();

    let buffer: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));

    let err_fn = |err: cpal::Error| {
        tracing::warn!(event = "voice_input_stream_error", error = %err);
    };

    macro_rules! build_capture_stream {
        ($sample_ty:ty) => {{
            let buffer = buffer.clone();
            device.build_input_stream(
                stream_config.clone(),
                move |data: &[$sample_ty], _: &cpal::InputCallbackInfo| {
                    let Ok(mut buf) = buffer.lock() else {
                        return;
                    };
                    if channels <= 1 {
                        buf.extend(data.iter().map(|s| f32::from_sample(*s)));
                    } else {
                        for frame in data.chunks(channels as usize) {
                            let sum: f32 = frame.iter().map(|s| f32::from_sample(*s)).sum();
                            buf.push(sum / f32::from(channels));
                        }
                    }
                },
                err_fn,
                None,
            )
        }};
    }

    let stream = match sample_format {
        cpal::SampleFormat::F32 => build_capture_stream!(f32),
        cpal::SampleFormat::I16 => build_capture_stream!(i16),
        cpal::SampleFormat::U16 => build_capture_stream!(u16),
        cpal::SampleFormat::I8 => build_capture_stream!(i8),
        other => anyhow::bail!("unsupported input sample format: {other}"),
    }?;

    stream.play()?;

    Ok(RecordingSession {
        stream,
        buffer,
        sample_rate,
    })
}

/// Encode mono `f32` samples to a WAV byte buffer, then base64-encode it (the
/// wire format `voice_transcribe` expects). Reuses the same
/// `base64::engine::general_purpose::STANDARD` encoding already used elsewhere
/// in this codebase (`webhook.rs`, `identity/age_identity.rs`) — no new crate.
fn encode_wav_b64(samples: &[f32], sample_rate: u32) -> anyhow::Result<String> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(&mut cursor, spec)?;
        for &sample in samples {
            writer.write_sample(sample)?;
        }
        writer.finalize()?;
    }
    Ok(base64::engine::general_purpose::STANDARD.encode(cursor.into_inner()))
}

/// Decode a WAV byte buffer (as returned base64-decoded from `voice_speak`,
/// e.g. Kokoro's 16-bit mono PCM) into normalized `f32` samples, and play it
/// back on the default output device. Blocking (real-time audio callbacks + a
/// sleep for playback duration) — the async caller MUST run this via
/// `tokio::task::spawn_blocking`.
fn play_wav_bytes(bytes: &[u8]) -> anyhow::Result<()> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::Sample;

    let cursor = std::io::Cursor::new(bytes);
    let mut reader = hound::WavReader::new(cursor)?;
    let spec = reader.spec();
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<Vec<_>, _>>()?,
        hound::SampleFormat::Int => match spec.bits_per_sample {
            8 => reader
                .samples::<i8>()
                .map(|s| s.map(f32::from_sample))
                .collect::<Result<Vec<_>, _>>()?,
            16 => reader
                .samples::<i16>()
                .map(|s| s.map(f32::from_sample))
                .collect::<Result<Vec<_>, _>>()?,
            32 => reader
                .samples::<i32>()
                .map(|s| s.map(f32::from_sample))
                .collect::<Result<Vec<_>, _>>()?,
            other => anyhow::bail!("unsupported WAV bit depth: {other}"),
        },
    };
    let total_samples = samples.len();

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| anyhow::anyhow!("no default audio output device"))?;
    let output_format = device.default_output_config()?.sample_format();

    let stream_config = cpal::StreamConfig {
        channels: spec.channels,
        sample_rate: spec.sample_rate,
        buffer_size: cpal::BufferSize::Default,
    };

    let samples = Arc::new(samples);
    let position = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let err_fn = |err: cpal::Error| {
        tracing::warn!(event = "voice_output_stream_error", error = %err);
    };

    macro_rules! build_playback_stream {
        ($sample_ty:ty) => {{
            let samples = samples.clone();
            let position = position.clone();
            device.build_output_stream(
                stream_config.clone(),
                move |data: &mut [$sample_ty], _: &cpal::OutputCallbackInfo| {
                    for slot in data.iter_mut() {
                        let idx = position.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        *slot = samples
                            .get(idx)
                            .copied()
                            .map(<$sample_ty>::from_sample)
                            .unwrap_or(<$sample_ty as Sample>::EQUILIBRIUM);
                    }
                },
                err_fn,
                None,
            )
        }};
    }

    let stream = match output_format {
        cpal::SampleFormat::F32 => build_playback_stream!(f32),
        cpal::SampleFormat::I16 => build_playback_stream!(i16),
        cpal::SampleFormat::U16 => build_playback_stream!(u16),
        cpal::SampleFormat::I8 => build_playback_stream!(i8),
        other => anyhow::bail!("unsupported output sample format: {other}"),
    }?;

    stream.play()?;

    // No per-sample completion signal from the real-time callback thread (would
    // need cross-thread synchronization for little benefit) — sleep for the
    // known playback duration instead, with a small safety margin.
    let denom = (spec.sample_rate as f64 * spec.channels as f64).max(1.0);
    let duration_secs = total_samples as f64 / denom + 0.2;
    std::thread::sleep(std::time::Duration::from_secs_f64(duration_secs));
    drop(stream);

    Ok(())
}

/// Run one full voice turn from already-recorded mono samples: encode, invoke
/// the capability chain (`handle_voice_turn`), decode, then play back. The
/// blocking WAV decode + playback is dispatched to a blocking-safe thread so the
/// async runtime is never stalled by `play_wav_bytes`'s `std::thread::sleep`.
async fn run_voice_turn(
    samples: Vec<f32>,
    sample_rate: u32,
    registry: &CapabilityRegistry,
    agent: &AgentHandle,
    owner: &str,
    voice_id: &str,
) -> anyhow::Result<()> {
    let audio_b64_in = encode_wav_b64(&samples, sample_rate)?;
    let audio_b64_out = handle_voice_turn(audio_b64_in, registry, agent, owner, voice_id).await?;
    let wav_bytes = base64::engine::general_purpose::STANDARD
        .decode(audio_b64_out)
        .map_err(|e| anyhow::anyhow!("failed to base64-decode voice_speak audio: {e}"))?;

    tokio::task::spawn_blocking(move || play_wav_bytes(&wav_bytes))
        .await
        .map_err(|e| anyhow::anyhow!("voice playback task panicked: {e}"))??;

    Ok(())
}

/// Push-to-talk trigger loop (VOICE-01 default): hold SPACE to record, release
/// to run a full voice turn. A failed turn is logged and never crashes the
/// channel (T-10-07-02).
///
/// `VOICE_PUSH_TO_TALK_KEY` (hardcoded to Space this phase) is documented here as
/// a future config knob.
async fn push_to_talk_loop(
    registry: Arc<CapabilityRegistry>,
    agent: AgentHandle,
    owner: String,
    voice_id: String,
) -> anyhow::Result<()> {
    use crossterm::event::{
        Event, EventStream, KeyCode, KeyEventKind, KeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    };
    use crossterm::execute;
    use crossterm::terminal::{enable_raw_mode, supports_keyboard_enhancement};
    use futures_util::StreamExt;

    enable_raw_mode()?;
    // Plain terminals only ever emit `KeyEventKind::Press` — release detection
    // needs the Kitty keyboard protocol enhancement, which not every terminal
    // supports. Best-effort: enable it when available so key-up actually stops
    // the recording; on terminals without support, push-to-talk still starts
    // recording on press (release simply won't be reported — a known,
    // terminal-capability-dependent limitation, not a Bastion bug).
    if supports_keyboard_enhancement().unwrap_or(false) {
        execute!(
            std::io::stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::REPORT_EVENT_TYPES)
        )?;
    }

    tracing::info!(
        event = "voice_push_to_talk_ready",
        "Hold SPACE to talk, release to send."
    );

    let mut events = EventStream::new();
    let mut recording: Option<RecordingSession> = None;

    while let Some(event) = events.next().await {
        let event = match event {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(event = "voice_terminal_event_error", error = %e);
                continue;
            }
        };
        let Event::Key(key) = event else { continue };
        if key.code != KeyCode::Char(' ') {
            continue;
        }
        match key.kind {
            KeyEventKind::Press => {
                if recording.is_some() {
                    continue; // already recording (key-repeat)
                }
                match start_recording() {
                    Ok(session) => recording = Some(session),
                    Err(e) => tracing::warn!(event = "voice_record_start_error", error = %e),
                }
            }
            KeyEventKind::Release => {
                let Some(session) = recording.take() else {
                    continue;
                };
                let (samples, sample_rate) = session.stop();
                if let Err(e) =
                    run_voice_turn(samples, sample_rate, &registry, &agent, &owner, &voice_id).await
                {
                    tracing::warn!(event = "voice_turn_error", error = %e);
                }
            }
            KeyEventKind::Repeat => {}
        }
    }

    Ok(())
}

// ─── wake-word trigger loop (D-10, opt-in, hardware-dependent) ────────────

/// Fixed recording window for a wake-word-triggered utterance. VAD-based
/// end-of-speech trimming is out of scope this phase (push-to-talk's
/// press/release IS the end-of-speech signal for that trigger; wake-word has
/// no equivalent signal without extra work) — a fixed window keeps the first
/// cut simple and safe. A future refinement could trim trailing silence.
const WAKE_WORD_UTTERANCE_SECS: f64 = 4.0;

/// Wake-word (D-10, opt-in) trigger loop, backed by `rustpotter` (pinned
/// `3.0.2`, approved via this plan's Task 3 human-verify checkpoint).
///
/// SUPPLY-CHAIN NOTE (T-10-07-SC): `rustpotter` ships NO bundled/default
/// wake-word model or reference — every wakeword must be a file the operator
/// supplies (either a wakeword *reference*, built from 3-8 of their own WAV
/// recordings, e.g. via `rustpotter-cli`, or a trained wakeword *model*; see
/// the crate's own README and `Rustpotter::add_wakeword_from_file`). This was
/// outside 10-RESEARCH.md's supply-chain-only audit scope and is a real,
/// expected operational requirement — not a Bastion bug. Configured via
/// `VOICE_WAKE_WORD_MODEL_PATH`; if unset, or the file fails to load, or no
/// input device is available, this loop logs a loud warning and returns
/// immediately — `VoiceChannel::run`'s push-to-talk path keeps running
/// regardless (never a silent no-op, per this task's own `<done>` criterion).
///
/// Reuses the exact same `handle_voice_turn` core as push-to-talk (via
/// `run_voice_turn`) — only the trigger differs, per D-10.
async fn wake_word_loop(
    registry: Arc<CapabilityRegistry>,
    agent: AgentHandle,
    owner: String,
    voice_id: String,
) -> anyhow::Result<()> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::Sample;

    let Ok(model_path) = std::env::var("VOICE_WAKE_WORD_MODEL_PATH") else {
        tracing::warn!(
            event = "voice_wake_word_model_missing",
            "wake_word_enabled=true but VOICE_WAKE_WORD_MODEL_PATH is unset — rustpotter ships \
             no default wakeword (a reference or trained model file must be provided by the \
             operator); wake-word trigger disabled, push-to-talk continues"
        );
        return Ok(());
    };

    let host = cpal::default_host();
    let Some(device) = host.default_input_device() else {
        tracing::warn!(
            event = "voice_wake_word_no_input_device",
            "no default audio input device — wake-word trigger disabled, push-to-talk continues"
        );
        return Ok(());
    };
    let config = match device.default_input_config() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(event = "voice_wake_word_input_config_error", error = %e, "wake-word trigger disabled, push-to-talk continues");
            return Ok(());
        }
    };
    let channels = config.channels();
    let sample_rate = config.sample_rate();
    let sample_format = config.sample_format();
    let stream_config: cpal::StreamConfig = config.into();

    let mut rustpotter_config = rustpotter::RustpotterConfig::default();
    rustpotter_config.fmt.sample_rate = sample_rate as usize;
    rustpotter_config.fmt.channels = 1;
    rustpotter_config.fmt.sample_format = rustpotter::SampleFormat::F32;

    let mut detector = match rustpotter::Rustpotter::new(&rustpotter_config) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(event = "voice_wake_word_init_error", error = %e, "failed to initialize rustpotter detector — wake-word trigger disabled, push-to-talk continues");
            return Ok(());
        }
    };
    if let Err(e) = detector.add_wakeword_from_file("wake_word", &model_path) {
        tracing::warn!(event = "voice_wake_word_model_load_error", error = %e, model_path = %model_path, "failed to load wake-word model/reference — wake-word trigger disabled, push-to-talk continues");
        return Ok(());
    }
    let frame_len = detector.get_samples_per_frame();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<f32>();
    let err_fn = |err: cpal::Error| {
        tracing::warn!(event = "voice_wake_word_input_stream_error", error = %err);
    };

    macro_rules! build_wake_word_stream {
        ($sample_ty:ty) => {{
            let tx = tx.clone();
            device.build_input_stream(
                stream_config.clone(),
                move |data: &[$sample_ty], _: &cpal::InputCallbackInfo| {
                    if channels <= 1 {
                        for s in data {
                            let _ = tx.send(f32::from_sample(*s));
                        }
                    } else {
                        for frame in data.chunks(channels as usize) {
                            let sum: f32 = frame.iter().map(|s| f32::from_sample(*s)).sum();
                            let _ = tx.send(sum / f32::from(channels));
                        }
                    }
                },
                err_fn,
                None,
            )
        }};
    }

    let stream = match sample_format {
        cpal::SampleFormat::F32 => build_wake_word_stream!(f32),
        cpal::SampleFormat::I16 => build_wake_word_stream!(i16),
        cpal::SampleFormat::U16 => build_wake_word_stream!(u16),
        cpal::SampleFormat::I8 => build_wake_word_stream!(i8),
        other => {
            tracing::warn!(event = "voice_wake_word_unsupported_format", format = ?other, "wake-word trigger disabled, push-to-talk continues");
            return Ok(());
        }
    };
    let stream = match stream {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(event = "voice_wake_word_stream_build_error", error = %e, "wake-word trigger disabled, push-to-talk continues");
            return Ok(());
        }
    };
    if let Err(e) = stream.play() {
        tracing::warn!(event = "voice_wake_word_stream_play_error", error = %e, "wake-word trigger disabled, push-to-talk continues");
        return Ok(());
    }

    tracing::info!(
        event = "voice_wake_word_ready",
        model_path = %model_path,
        "Wake-word detection active."
    );

    let mut frame_buf: Vec<f32> = Vec::with_capacity(frame_len);
    while let Some(sample) = rx.recv().await {
        frame_buf.push(sample);
        if frame_buf.len() < frame_len {
            continue;
        }
        let frame: Vec<f32> = frame_buf.drain(..frame_len).collect();
        if detector.process_samples(frame).is_none() {
            continue;
        }

        tracing::info!(
            event = "voice_wake_word_detected",
            "Wake word detected — recording utterance."
        );

        // Collect the follow-up utterance from the SAME continuous stream
        // (no second input stream needed) for a fixed window.
        let utterance_samples_needed = (sample_rate as f64 * WAKE_WORD_UTTERANCE_SECS) as usize;
        let mut utterance: Vec<f32> = Vec::with_capacity(utterance_samples_needed);
        while utterance.len() < utterance_samples_needed {
            match rx.recv().await {
                Some(s) => utterance.push(s),
                None => break,
            }
        }
        detector.reset();

        if let Err(e) =
            run_voice_turn(utterance, sample_rate, &registry, &agent, &owner, &voice_id).await
        {
            tracing::warn!(event = "voice_turn_error", error = %e);
        }
    }

    Ok(())
}

// ─── tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use base64::Engine;
    use bastion_runtime::agent::handle;
    use bastion_runtime::capability::registry::Capability;
    use serde_json::Value;
    use std::sync::{Arc, Mutex};
    use tokio::sync::mpsc;

    /// One recorded `privacy_tier` per stub invocation.
    type RecordedCalls = Arc<Mutex<Vec<Option<PrivacyTier>>>>;

    /// Stub `voice_transcribe`/`voice_speak` capability — records every `InvokeCtx`
    /// it was called with (Test 2's load-bearing LocalOnly assertion) and returns a
    /// scripted response. Mirrors `registry.rs`'s `StubCap` test pattern.
    ///
    /// `is_local() -> true` mirrors the production adapter override this whole
    /// feature depends on (Plan 10-08) — without it, `CapabilityRegistry::invoke`
    /// would block every LocalOnly-tagged call before it ever reached this stub.
    struct StubVoiceCap {
        name: String,
        schema: Value,
        response: Value,
        recorded: RecordedCalls,
    }

    #[async_trait]
    impl Capability for StubVoiceCap {
        fn name(&self) -> &str {
            &self.name
        }
        fn description(&self) -> &str {
            "stub"
        }
        fn input_schema(&self) -> &Value {
            &self.schema
        }
        async fn invoke(&self, _args: Value, ctx: &InvokeCtx) -> anyhow::Result<Value> {
            self.recorded.lock().unwrap().push(ctx.privacy_tier);
            Ok(self.response.clone())
        }
        fn is_local(&self) -> bool {
            true
        }
    }

    /// Stub consumer: replies "echo:{text}" (mirrors `telegram.rs`'s test pattern).
    fn stub_consumer(mut rx: mpsc::Receiver<handle::AgentRequest>) {
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                let _ = req.reply.send(Ok(format!("echo:{}", req.text)));
            }
        });
    }

    fn registry_with(
        transcribe_response: Value,
        speak_response: Value,
        recorded: RecordedCalls,
    ) -> CapabilityRegistry {
        let mut registry = CapabilityRegistry::new();
        registry
            .register(Arc::new(StubVoiceCap {
                name: "voice_transcribe".to_string(),
                schema: serde_json::json!({}),
                response: transcribe_response,
                recorded: recorded.clone(),
            }))
            .unwrap();
        registry
            .register(Arc::new(StubVoiceCap {
                name: "voice_speak".to_string(),
                schema: serde_json::json!({}),
                response: speak_response,
                recorded,
            }))
            .unwrap();
        registry
    }

    #[tokio::test]
    async fn handle_voice_turn_returns_decoded_speak_audio() {
        let known_bytes = b"fake-wav-bytes".to_vec();
        let audio_b64 = base64::engine::general_purpose::STANDARD.encode(&known_bytes);
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let registry = registry_with(
            serde_json::json!({ "text": "oi bastion" }),
            serde_json::json!({ "audio_b64": audio_b64, "sample_rate": 24000 }),
            recorded,
        );

        let (h, rx) = handle::channel();
        stub_consumer(rx);

        let out = handle_voice_turn(
            "fake-input-b64".to_string(),
            &registry,
            &h,
            "mario",
            "pf_dora",
        )
        .await
        .unwrap();

        let decoded = base64::engine::general_purpose::STANDARD
            .decode(out)
            .unwrap();
        assert_eq!(decoded, known_bytes);
    }

    #[tokio::test]
    async fn handle_voice_turn_tags_both_invocations_local_only() {
        let audio_b64 = base64::engine::general_purpose::STANDARD.encode(b"x");
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let registry = registry_with(
            serde_json::json!({ "text": "oi bastion" }),
            serde_json::json!({ "audio_b64": audio_b64, "sample_rate": 24000 }),
            recorded.clone(),
        );

        let (h, rx) = handle::channel();
        stub_consumer(rx);

        handle_voice_turn(
            "fake-input-b64".to_string(),
            &registry,
            &h,
            "mario",
            "pf_dora",
        )
        .await
        .unwrap();

        let calls = recorded.lock().unwrap();
        assert_eq!(
            calls.len(),
            2,
            "both voice_transcribe and voice_speak must invoke"
        );
        for tier in calls.iter() {
            assert_eq!(*tier, Some(PrivacyTier::LocalOnly));
        }
    }

    #[tokio::test]
    async fn handle_voice_turn_errors_on_missing_text_field() {
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let registry = registry_with(
            serde_json::json!({ "not_text": "oops" }),
            serde_json::json!({ "audio_b64": "", "sample_rate": 24000 }),
            recorded,
        );

        let (h, rx) = handle::channel();
        stub_consumer(rx);

        let result = handle_voice_turn(
            "fake-input-b64".to_string(),
            &registry,
            &h,
            "mario",
            "pf_dora",
        )
        .await;

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("voice_transcribe returned no text field"));
    }
}
