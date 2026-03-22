use std::{
    sync::{mpsc, Arc},
    time::Duration,
};

use anyhow::{anyhow, Result};
use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    BufferSize, SampleFormat, SampleRate, StreamConfig, SupportedBufferSize,
};

#[derive(Debug, Clone)]
pub struct PcmChunk(pub Arc<Vec<f32>>);

#[derive(Debug, Clone)]
pub struct InputConfig {
    pub device: Option<String>,
    pub channels: usize,
    pub sample_rate: u32,
    pub buffer: u32,
    pub timeout_secs: u64,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            device: None,
            channels: 2,
            sample_rate: 48_000,
            buffer: 1024,
            timeout_secs: 5,
        }
    }
}

pub struct AudioInput {
    rx: mpsc::Receiver<PcmChunk>,
    _stream: cpal::Stream,
    pub sample_rate: u32,
    pub channels: usize,
    pub active_channels: usize,
    pub device_name: String,
}

pub fn list_input_devices() -> Result<()> {
    let host = cpal::default_host();
    let devices = host.input_devices()?;

    for dev in devices {
        let name = dev.name().unwrap_or_else(|_| "<unknown>".to_string());
        println!("> {name}");
        for config in dev.supported_input_configs()? {
            let buffer = match config.buffer_size() {
                SupportedBufferSize::Range { min, max } => format!("{min}-{max}"),
                SupportedBufferSize::Unknown => "unknown".to_string(),
            };
            println!(
                "  + {}ch {}-{}hz buf={} ({:?})",
                config.channels(),
                config.min_sample_rate().0,
                config.max_sample_rate().0,
                buffer,
                config.sample_format()
            );
        }
    }

    Ok(())
}

/// Ported from scope-tui:
/// split an interleaved channel stream into per-channel vectors.
pub fn stream_to_matrix<I, O>(stream: impl Iterator<Item = I>, channels: usize, norm: O) -> Vec<Vec<O>>
where
    I: Copy + Into<O>,
    O: Copy + std::ops::Div<Output = O>,
{
    let mut out = vec![vec![]; channels];
    let mut channel = 0;
    for sample in stream {
        out[channel].push(sample.into() / norm);
        channel = (channel + 1) % channels;
    }
    out
}

fn matrix_to_mono(matrix: &[Vec<f32>], active_channels: usize) -> Vec<f32> {
    if matrix.is_empty() {
        return Vec::new();
    }
    let use_channels = active_channels.clamp(1, matrix.len());
    let len = matrix.iter().map(|ch| ch.len()).min().unwrap_or(0);
    if len == 0 {
        return Vec::new();
    }

    let channels = use_channels as f32;
    let mut mono = Vec::with_capacity(len);
    for i in 0..len {
        let mut sum = 0.0_f32;
        for ch in matrix.iter().take(use_channels) {
            sum += ch[i];
        }
        mono.push(sum / channels);
    }
    mono
}

fn select_input_device(host: &cpal::Host, requested: Option<&str>) -> Result<cpal::Device> {
    match requested {
        Some(name) => host
            .input_devices()?
            .find(|d| d.name().as_deref().unwrap_or_default() == name)
            .ok_or_else(|| anyhow!("input device not found: '{name}'")),
        None => host
            .default_input_device()
            .ok_or_else(|| anyhow!("no default input device available")),
    }
}

fn choose_stream_config(
    device: &cpal::Device,
    cfg: &InputConfig,
) -> Result<(StreamConfig, SampleFormat)> {
    let mut ranges: Vec<_> = device.supported_input_configs()?.collect();
    if ranges.is_empty() {
        return Err(anyhow!("device has no supported input configs"));
    }

    let req_channels = cfg.channels.max(1) as u16;
    let req_rate = cfg.sample_rate.max(8_000);
    ranges.sort_by_key(|r| {
        let ch_penalty = (r.channels() as i32 - req_channels as i32).unsigned_abs();
        let fmt_penalty = match r.sample_format() {
            SampleFormat::F32 => 0_u32,
            SampleFormat::I16 => 1,
            SampleFormat::U16 => 2,
            _ => 3,
        };
        let picked = req_rate.clamp(r.min_sample_rate().0, r.max_sample_rate().0);
        let sr_penalty = req_rate.abs_diff(picked);
        (ch_penalty, fmt_penalty, sr_penalty)
    });

    let chosen = ranges
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no usable input config found"))?;
    let sample_format = chosen.sample_format();
    let sample_rate = req_rate.clamp(chosen.min_sample_rate().0, chosen.max_sample_rate().0);
    let mut stream_cfg = chosen.with_sample_rate(SampleRate(sample_rate)).config();
    if cfg.buffer > 0 {
        stream_cfg.buffer_size = BufferSize::Fixed(cfg.buffer);
    }
    Ok((stream_cfg, sample_format))
}

impl AudioInput {
    pub fn new(cfg: &InputConfig) -> Result<Self> {
        let host = cpal::default_host();
        let device = select_input_device(&host, cfg.device.as_deref())?;
        let device_name = device.name().unwrap_or_else(|_| "<unknown>".to_string());

        let (stream_cfg, sample_format) = choose_stream_config(&device, cfg)?;
        let channels = stream_cfg.channels as usize;
        let active_channels = cfg.channels.max(1).min(channels);
        let sample_rate = stream_cfg.sample_rate.0;
        let timeout = Some(Duration::from_secs(cfg.timeout_secs.max(1)));

        let (tx, rx) = mpsc::channel();

        let stream = match sample_format {
            SampleFormat::F32 => {
                let tx = tx.clone();
                device.build_input_stream(
                    &stream_cfg,
                    move |data: &[f32], _| {
                        let matrix = stream_to_matrix(data.iter().copied(), channels, 1.0_f32);
                        let mono = matrix_to_mono(&matrix, active_channels);
                        if !mono.is_empty() {
                            let _ = tx.send(PcmChunk(Arc::new(mono)));
                        }
                    },
                    move |err| eprintln!("cpal input stream error: {err}"),
                    timeout,
                )?
            }
            SampleFormat::I16 => {
                let tx = tx.clone();
                device.build_input_stream(
                    &stream_cfg,
                    move |data: &[i16], _| {
                        let matrix = stream_to_matrix(
                            data.iter().map(|v| *v as f32),
                            channels,
                            32_768.0_f32,
                        );
                        let mono = matrix_to_mono(&matrix, active_channels);
                        if !mono.is_empty() {
                            let _ = tx.send(PcmChunk(Arc::new(mono)));
                        }
                    },
                    move |err| eprintln!("cpal input stream error: {err}"),
                    timeout,
                )?
            }
            SampleFormat::U16 => {
                let tx = tx.clone();
                device.build_input_stream(
                    &stream_cfg,
                    move |data: &[u16], _| {
                        let matrix = stream_to_matrix(
                            data.iter().map(|v| *v as f32 - 32_768.0_f32),
                            channels,
                            32_768.0_f32,
                        );
                        let mono = matrix_to_mono(&matrix, active_channels);
                        if !mono.is_empty() {
                            let _ = tx.send(PcmChunk(Arc::new(mono)));
                        }
                    },
                    move |err| eprintln!("cpal input stream error: {err}"),
                    timeout,
                )?
            }
            other => return Err(anyhow!("unsupported cpal sample format: {other:?}")),
        };

        stream.play()?;

        Ok(Self {
            rx,
            _stream: stream,
            sample_rate,
            channels,
            active_channels,
            device_name,
        })
    }

    pub fn drain_pcm_chunks(&mut self) -> Vec<PcmChunk> {
        let mut out = Vec::new();
        while let Ok(chunk) = self.rx.try_recv() {
            out.push(chunk);
        }
        out
    }
}
