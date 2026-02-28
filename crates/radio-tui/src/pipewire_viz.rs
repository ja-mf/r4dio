//! PipeWire/PulseAudio monitor visualization for Linux.
//!
//! This module provides an alternative audio capture source for the VU meter
//! and oscilloscope. Instead of capturing from the stream via ffmpeg, it captures
//! directly from the PipeWire/PulseAudio monitor source, visualizing the actual
//! system audio output.
//!
//! Only available on Linux. On other platforms, this module is a no-op stub.

use std::sync::Arc;

use tracing::{debug, error, info, warn};

use crate::BroadcastMessage;

const VU_WINDOW_SAMPLES: usize = 1024;
const VU_SAMPLE_RATE: u32 = 44100;

/// Spawn a task that captures audio from PipeWire/PulseAudio monitor
/// and broadcasts PcmChunk messages.
///
/// This is the Linux-specific implementation using libpulse_binding.
#[cfg(target_os = "linux")]
pub fn spawn_pipewire_viz_task(
    device: Option<String>,
    broadcast_tx: tokio::sync::broadcast::Sender<BroadcastMessage>,
) -> tokio::task::AbortHandle {
    let handle = tokio::spawn(async move {
        info!("Starting PipeWire/PulseAudio monitor capture");
        loop {
            if let Err(e) = run_pipewire_capture(device.as_deref(), &broadcast_tx).await {
                error!("PipeWire capture error: {}", e);
            }
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    });
    handle.abort_handle()
}

/// Stub for non-Linux platforms - does nothing.
#[cfg(not(target_os = "linux"))]
pub fn spawn_pipewire_viz_task(
    _device: Option<String>,
    _broadcast_tx: tokio::sync::broadcast::Sender<BroadcastMessage>,
) -> tokio::task::AbortHandle {
    // This should never be called on non-Linux, but provide a stub just in case
    let handle = tokio::spawn(async {});
    handle.abort_handle()
}

/// Linux implementation using libpulse_binding.
#[cfg(target_os = "linux")]
async fn run_pipewire_capture(
    device: Option<&str>,
    broadcast_tx: &tokio::sync::broadcast::Sender<BroadcastMessage>,
) -> anyhow::Result<()> {
    use libpulse_binding::{
        def::BufferAttr,
        sample::{Format, Spec},
        stream::Direction,
    };
    use libpulse_simple_binding::Simple;
    use std::sync::mpsc::{channel, Receiver};
    use std::thread;

    // Create a channel to send PCM data from the PulseAudio thread to the async task
    let (pcm_tx, pcm_rx): (std::sync::mpsc::Sender<Vec<f32>>, Receiver<Vec<f32>>) = channel();

    // Spawn the PulseAudio capture in a blocking thread
    let device_owned = device.map(|s| s.to_string());
    let handle = thread::spawn(move || {
        let spec = Spec {
            format: Format::S16NE,
            channels: 1, // Mono
            rate: VU_SAMPLE_RATE,
        };

        if !spec.is_valid() {
            error!("Invalid PulseAudio spec");
            return;
        }

        // Default to monitor source if no device specified
        // PipeWire/PulseAudio monitor sources are typically named:
        // - "pipewire.monitor" for PipeWire
        // - "alsa_output.*.monitor" for ALSA devices
        // - "alsa_input.*" for input devices
        let device_name = device_owned.as_deref().or_else(|| {
            // Try to find a default monitor source
            Some("pipewire.monitor")
        });

        info!("Connecting to PulseAudio device: {:?}", device_name);

        let attrs = BufferAttr {
            maxlength: (VU_WINDOW_SAMPLES * 4) as u32,
            fragsize: (VU_WINDOW_SAMPLES * 2) as u32,
            ..Default::default()
        };

        let simple = match Simple::new(
            None,                  // Use default server
            "r4dio-viz",          // Application name
            Direction::Record,     // Record from output monitor
            device_name,           // Device (None = default)
            "visualizer",         // Stream description
            &spec,
            None,                  // Use default channel map
            Some(&attrs),
        ) {
            Ok(s) => s,
            Err(e) => {
                error!("Failed to connect to PulseAudio: {}", e);
                return;
            }
        };

        let mut buffer = vec![0i16; VU_WINDOW_SAMPLES];

        loop {
            // Convert i16 buffer to u8 slice for PulseAudio read
            let buffer_bytes = unsafe {
                std::slice::from_raw_parts_mut(
                    buffer.as_mut_ptr() as *mut u8,
                    buffer.len() * std::mem::size_of::<i16>()
                )
            };
            match simple.read(buffer_bytes) {
                Ok(()) => {
                    // Convert i16 samples to f32 (-1.0 to 1.0)
                    let pcm: Vec<f32> = buffer.iter().map(|&s| s as f32 / 32768.0).collect();
                    if pcm_tx.send(pcm).is_err() {
                        // Receiver dropped, exit thread
                        break;
                    }
                }
                Err(e) => {
                    error!("PulseAudio read error: {}", e);
                    break;
                }
            }
        }

        debug!("PulseAudio capture thread exiting");
    });

    // Receive PCM data and broadcast it
    loop {
        match pcm_rx.recv() {
            Ok(pcm) => {
                let _ = broadcast_tx.send(BroadcastMessage::PcmChunk(Arc::new(pcm)));
            }
            Err(_) => {
                // Channel closed, thread exited
                break;
            }
        }
    }

    // Clean up the thread
    drop(pcm_rx);
    let _ = handle.join();

    anyhow::bail!("PipeWire/PulseAudio capture ended")
}

#[cfg(not(target_os = "linux"))]
async fn run_pipewire_capture(
    _device: Option<&str>,
    _broadcast_tx: &tokio::sync::broadcast::Sender<BroadcastMessage>,
) -> anyhow::Result<()> {
    anyhow::bail!("PipeWire visualization is only available on Linux")
}
