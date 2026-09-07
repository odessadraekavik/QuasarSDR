//! Callback CPAL : aucune allocation, aucun verrou ni attente.
//! Le DSP pousse du mono 48 kHz dans une file bornée ; le callback adapte
//! le débit à celui du périphérique et duplique le mono sur ses canaux.
use crate::model::AUDIO_RATE;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_queue::ArrayQueue;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

pub struct AudioOutput {
    _stream: cpal::Stream,
    pub description: String,
}

pub fn open(
    queue: Arc<ArrayQueue<f32>>,
    underruns: Arc<AtomicU64>,
    errors: Arc<AtomicU64>,
) -> Result<AudioOutput, String> {
    let device = cpal::default_host()
        .default_output_device()
        .ok_or("Aucune sortie audio disponible")?;
    let supported = device.default_output_config().map_err(|e| e.to_string())?;
    let config = supported.config();
    let description = format!(
        "{} · {} Hz",
        device.name().unwrap_or_else(|_| "Sortie système".into()),
        config.sample_rate.0
    );
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => build::<f32>(&device, &config, queue, underruns, errors),
        cpal::SampleFormat::I16 => build::<i16>(&device, &config, queue, underruns, errors),
        cpal::SampleFormat::U16 => build::<u16>(&device, &config, queue, underruns, errors),
        format => return Err(format!("Format audio non géré : {format:?}")),
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(AudioOutput {
        _stream: stream,
        description,
    })
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    queue: Arc<ArrayQueue<f32>>,
    underruns: Arc<AtomicU64>,
    errors: Arc<AtomicU64>,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = config.channels as usize;
    let ratio = AUDIO_RATE as f64 / config.sample_rate.0 as f64;
    let mut phase = 0.0_f64;
    let mut left = 0.0_f32;
    let mut right = 0.0_f32;
    device.build_output_stream(
        config,
        move |data: &mut [T], _| {
            let mut missing = 0;
            for frame in data.chunks_mut(channels) {
                let x = left + (right - left) * phase as f32;
                frame.fill(T::from_sample(x));
                phase += ratio;
                while phase >= 1.0 {
                    phase -= 1.0;
                    left = right;
                    right = queue.pop().unwrap_or_else(|| {
                        missing += 1;
                        0.0
                    });
                }
            }
            underruns.fetch_add(missing, Ordering::Relaxed);
        },
        move |_| {
            errors.fetch_add(1, Ordering::Relaxed);
        },
        None,
    )
}
