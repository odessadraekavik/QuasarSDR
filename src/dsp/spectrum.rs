use crate::model::{Spectrum, BLOCK_SIZE, FFT_SIZE, IQ_RATE};
use num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use std::{f32::consts::TAU, sync::Arc};

pub struct SpectrumAnalyzer {
    fft: Arc<dyn Fft<f32>>,
    buffer: Vec<Complex32>,
    scratch: Vec<Complex32>,
    window: Vec<f32>,
    floor: Vec<f32>,
    averaged_power: Vec<f32>,
    initialized: bool,
    frames_seen: u32,
}

impl Default for SpectrumAnalyzer {
    fn default() -> Self {
        let fft = FftPlanner::new().plan_fft_forward(FFT_SIZE);
        let scratch = vec![Complex32::default(); fft.get_inplace_scratch_len()];
        Self {
            fft,
            scratch,
            buffer: vec![Complex32::default(); FFT_SIZE],
            window: (0..FFT_SIZE)
                .map(|i| 0.5 - 0.5 * (TAU * i as f32 / FFT_SIZE as f32).cos())
                .collect(),
            floor: vec![-100.0; FFT_SIZE],
            averaged_power: vec![0.0; FFT_SIZE],
            initialized: false,
            frames_seen: 0,
        }
    }
}

impl SpectrumAnalyzer {
    pub fn zero_progress(&self) -> f32 {
        (self.frames_seen as f32 * BLOCK_SIZE as f32 / (2.0 * IQ_RATE as f32)).min(1.0)
    }

    pub fn process(&mut self, iq: &[Complex32]) -> Spectrum {
        assert_eq!(iq.len(), FFT_SIZE);
        for ((out, x), w) in self.buffer.iter_mut().zip(iq).zip(&self.window) {
            *out = x * w;
        }
        self.fft
            .process_with_scratch(&mut self.buffer, &mut self.scratch);
        let scale = (FFT_SIZE as f32 * 0.5).powi(2);
        let power_db: Vec<f32> = (0..FFT_SIZE)
            .map(|i| {
                let power = self.buffer[(i + FFT_SIZE / 2) % FFT_SIZE].norm_sqr() / scale;
                self.averaged_power[i] = if self.initialized {
                    self.averaged_power[i] + 0.2 * (power - self.averaged_power[i])
                } else {
                    power
                };
                10.0 * self.averaged_power[i].max(1e-14).log10()
            })
            .collect();
        // Quantile local robuste (25 %) puis lissage temporel asymétrique.
        // Limite : une émission occupant presque toute la fenêtre biaise l'estimation.
        for i in 0..FFT_SIZE {
            let lo = i.saturating_sub(64);
            let hi = (i + 65).min(FFT_SIZE);
            let mut values = [0.0_f32; 129];
            let window = &mut values[..hi - lo];
            window.copy_from_slice(&power_db[lo..hi]);
            let index = window.len() / 4;
            window.select_nth_unstable_by(index, f32::total_cmp);
            // Compensation approximative du quantile sur le spectre moyenné :
            // ramène la bande de bruit près de 0 dB relatif, sans calibration RF.
            let estimate = window[index] + 1.0;
            // Protéger les pics d'émission : ils ne doivent pas devenir le zéro.
            if self.frames_seen >= 32 && power_db[i] > estimate + 6.0 {
                continue;
            }
            // Au démarrage, le moyennage spectral n'est pas encore stabilisé.
            // Acquérir rapidement le bruit avant de ralentir sa remontée.
            let alpha = if self.zero_progress() < 1.0 || estimate < self.floor[i] {
                0.2
            } else {
                0.015
            };
            self.floor[i] = if self.initialized {
                self.floor[i] + alpha * (estimate - self.floor[i])
            } else {
                estimate
            };
        }
        self.initialized = true;
        self.frames_seen = self.frames_seen.saturating_add(1);
        Spectrum {
            power_db,
            floor_db: self.floor.clone(),
        }
    }
}

/// Contraste spectral maximal dans le canal, en dB au-dessus du plancher local.
/// Ce n'est pas une mesure calibrée du SNR intégré sur la bande.
pub fn channel_snr(s: &Spectrum, offset: f32, bandwidth: f32) -> f32 {
    let bin = |f: f32| {
        ((f / IQ_RATE as f32 + 0.5) * FFT_SIZE as f32).clamp(0.0, (FFT_SIZE - 1) as f32) as usize
    };
    let lo = bin(offset - bandwidth * 0.5);
    let hi = bin(offset + bandwidth * 0.5);
    (lo..=hi)
        .map(|i| s.power_db[i] - s.floor_db[i])
        .fold(0.0, f32::max)
}

/// Puissance intégrée du canal ; compensation du gain énergétique Hann (1,5).
pub fn channel_level_dbfs(s: &Spectrum, offset: f32, bandwidth: f32) -> f32 {
    let bin = |f: f32| {
        ((f / IQ_RATE as f32 + 0.5) * FFT_SIZE as f32).clamp(0.0, (FFT_SIZE - 1) as f32) as usize
    };
    let lo = bin(offset - bandwidth * 0.5);
    let hi = bin(offset + bandwidth * 0.5);
    let power: f32 = s.power_db[lo..=hi]
        .iter()
        .map(|db| 10.0_f32.powf(db / 10.0))
        .sum();
    10.0 * (power / 1.5).max(1e-14).log10()
}
