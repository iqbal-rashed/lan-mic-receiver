use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_queue::ArrayQueue;
use std::sync::Arc;

/// Plays mono i16 @ 48k into a selected output device.
/// Use a virtual cable (VB-Cable) if you want other apps to see it as a microphone.
pub struct AudioOutput {
    _stream: cpal::Stream,
    device_name: String,
}

impl AudioOutput {
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    pub fn start(output_device_name: Option<&str>, queue: Arc<ArrayQueue<i16>>) -> Result<Self> {
        let host = cpal::default_host();

        let device = if let Some(name) = output_device_name {
            let mut found = None;
            for d in host.output_devices()? {
                if d.name().unwrap_or_default() == name {
                    found = Some(d);
                    break;
                }
            }
            found.ok_or_else(|| anyhow!("Output device not found: {name}"))?
        } else {
            host.default_output_device()
                .ok_or_else(|| anyhow!("No default output device"))?
        };

        let device_name = device.name().unwrap_or_else(|_| "<unknown>".to_string());

        // Prefer configs that can do 48kHz, and prefer 2 channels if available.
        let mut picked: Option<cpal::SupportedStreamConfig> = None;

        if let Ok(ranges) = device.supported_output_configs() {
            let mut candidates: Vec<_> = ranges
                .filter(|r| r.min_sample_rate().0 <= 48_000 && r.max_sample_rate().0 >= 48_000)
                .collect();

            // Prefer stereo then mono; prefer f32 sample format if present.
            candidates.sort_by_key(|r| {
                // sort ascending; lower is better
                let ch_penalty = if r.channels() == 2 {
                    0
                } else if r.channels() == 1 {
                    1
                } else {
                    2
                };
                let fmt_penalty = match r.sample_format() {
                    cpal::SampleFormat::F32 => 0,
                    cpal::SampleFormat::I16 => 1,
                    cpal::SampleFormat::U16 => 2,
                    _ => 3,
                };
                (ch_penalty, fmt_penalty)
            });

            if let Some(best) = candidates.first() {
                picked = Some(best.with_sample_rate(cpal::SampleRate(48_000)));
            }
        }

        let supported = picked.unwrap_or_else(|| {
            device
                .default_output_config()
                .expect("default output config")
        });
        let sample_format = supported.sample_format();
        let config: cpal::StreamConfig = supported.clone().into();
        let channels = config.channels as usize;

        let err_fn = |err| eprintln!("cpal stream error: {err}");

        let stream = match sample_format {
            cpal::SampleFormat::F32 => device.build_output_stream(
                &config,
                move |data: &mut [f32], _| {
                    write_data_f32(data, channels, &queue);
                },
                err_fn,
                None,
            )?,
            cpal::SampleFormat::I16 => device.build_output_stream(
                &config,
                move |data: &mut [i16], _| {
                    write_data_i16(data, channels, &queue);
                },
                err_fn,
                None,
            )?,
            cpal::SampleFormat::U16 => device.build_output_stream(
                &config,
                move |data: &mut [u16], _| {
                    write_data_u16(data, channels, &queue);
                },
                err_fn,
                None,
            )?,
            other => return Err(anyhow!("Unsupported sample format: {other:?}")),
        };

        stream.play()?;

        Ok(Self {
            _stream: stream,
            device_name,
        })
    }
}

fn write_data_f32(out: &mut [f32], channels: usize, q: &Arc<ArrayQueue<i16>>) {
    for frame in out.chunks_mut(channels) {
        let s = q.pop().unwrap_or(0);
        let v = (s as f32) / 32768.0;
        for ch in frame.iter_mut() {
            *ch = v;
        }
    }
}

fn write_data_i16(out: &mut [i16], channels: usize, q: &Arc<ArrayQueue<i16>>) {
    for frame in out.chunks_mut(channels) {
        let s = q.pop().unwrap_or(0);
        for ch in frame.iter_mut() {
            *ch = s;
        }
    }
}

fn write_data_u16(out: &mut [u16], channels: usize, q: &Arc<ArrayQueue<i16>>) {
    for frame in out.chunks_mut(channels) {
        let s = q.pop().unwrap_or(0) as i32;
        let v = (s + 32768).clamp(0, 65535) as u16;
        for ch in frame.iter_mut() {
            *ch = v;
        }
    }
}
