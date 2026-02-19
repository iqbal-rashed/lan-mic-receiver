use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_queue::ArrayQueue;
use std::sync::Arc;

/// Plays mono i16 @ 48 kHz into a selected output device.
///
/// Use a virtual cable (e.g. VB-Cable) and select "CABLE Input" if you want
/// other apps to see it as a microphone.
pub struct AudioOutput {
    _stream: Option<cpal::Stream>,
    device_name: String,
}

impl AudioOutput {
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// Create a stopped placeholder (no active audio stream).
    /// Used as a temporary during device switching.
    pub fn stopped() -> Self {
        Self {
            _stream: None,
            device_name: "(stopped)".to_string(),
        }
    }

    /// Open the specified (or default) output device and start playing samples
    /// from `queue`. Samples are mono i16 @ 48 kHz.
    pub fn start(output_device_name: Option<&str>, queue: Arc<ArrayQueue<i16>>) -> Result<Self> {
        let host = cpal::default_host();

        let device = match output_device_name {
            Some(name) => host
                .output_devices()?
                .find(|d| d.name().unwrap_or_default() == name)
                .ok_or_else(|| anyhow!("Output device not found: {name}"))?,
            None => host
                .default_output_device()
                .ok_or_else(|| anyhow!("No default output device"))?,
        };

        let device_name = device.name().unwrap_or_else(|_| "<unknown>".to_string());

        // Pick the best config that supports 48 kHz.
        let supported = pick_output_config(&device)?;
        let sample_format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();
        let channels = config.channels as usize;

        let err_fn = |err| log::error!("cpal stream error: {err}");

        let stream = match sample_format {
            cpal::SampleFormat::F32 => device.build_output_stream(
                &config,
                move |data: &mut [f32], _| write_data_f32(data, channels, &queue),
                err_fn,
                None,
            )?,
            cpal::SampleFormat::I16 => device.build_output_stream(
                &config,
                move |data: &mut [i16], _| write_data_i16(data, channels, &queue),
                err_fn,
                None,
            )?,
            cpal::SampleFormat::U16 => device.build_output_stream(
                &config,
                move |data: &mut [u16], _| write_data_u16(data, channels, &queue),
                err_fn,
                None,
            )?,
            other => return Err(anyhow!("Unsupported sample format: {other:?}")),
        };

        stream.play()?;

        Ok(Self {
            _stream: Some(stream),
            device_name,
        })
    }
}

/// Choose the best 48 kHz-capable output config, preferring stereo + f32.
fn pick_output_config(device: &cpal::Device) -> Result<cpal::SupportedStreamConfig> {
    if let Ok(ranges) = device.supported_output_configs() {
        let mut candidates: Vec<_> = ranges
            .filter(|r| r.min_sample_rate().0 <= 48_000 && r.max_sample_rate().0 >= 48_000)
            .collect();

        // Lower penalty = better. Prefer stereo, then f32.
        candidates.sort_by_key(|r| {
            let ch = match r.channels() {
                2 => 0,
                1 => 1,
                _ => 2,
            };
            let fmt = match r.sample_format() {
                cpal::SampleFormat::F32 => 0,
                cpal::SampleFormat::I16 => 1,
                cpal::SampleFormat::U16 => 2,
                _ => 3,
            };
            (ch, fmt)
        });

        if let Some(best) = candidates.first() {
            return Ok(best.with_sample_rate(cpal::SampleRate(48_000)));
        }
    }

    device
        .default_output_config()
        .map_err(|e| anyhow!("No suitable output config: {e}"))
}

// ---------------------------------------------------------------------------
// Write callbacks — pop mono i16 samples from the queue into device frames.
// ---------------------------------------------------------------------------

fn write_data_f32(out: &mut [f32], channels: usize, q: &Arc<ArrayQueue<i16>>) {
    for frame in out.chunks_mut(channels) {
        let v = q.pop().unwrap_or(0) as f32 / 32768.0;
        frame.fill(v);
    }
}

fn write_data_i16(out: &mut [i16], channels: usize, q: &Arc<ArrayQueue<i16>>) {
    for frame in out.chunks_mut(channels) {
        let s = q.pop().unwrap_or(0);
        frame.fill(s);
    }
}

fn write_data_u16(out: &mut [u16], channels: usize, q: &Arc<ArrayQueue<i16>>) {
    for frame in out.chunks_mut(channels) {
        let v = (q.pop().unwrap_or(0) as i32 + 32768).clamp(0, 65535) as u16;
        frame.fill(v);
    }
}
