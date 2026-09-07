use num_complex::Complex32;
use std::f32::consts::{PI, TAU};

/// FIR passe-bande complexe, fenêtre de Blackman. L'état survit aux blocs I/Q.
pub struct Fir {
    taps: Vec<Complex32>,
    history: Vec<Complex32>,
    cursor: usize,
}

impl Fir {
    pub fn new(fs: f32, low: f32, high: f32, length: usize) -> Self {
        let cutoff = (high - low) * 0.5 / fs;
        let center = (high + low) * 0.5 / fs;
        let mut real = Vec::with_capacity(length);
        for i in 0..length {
            let n = i as f32 - (length - 1) as f32 * 0.5;
            let sinc = if n == 0.0 {
                2.0 * cutoff
            } else {
                (TAU * cutoff * n).sin() / (PI * n)
            };
            let w = TAU * i as f32 / (length - 1) as f32;
            real.push(sinc * (0.42 - 0.5 * w.cos() + 0.08 * (2.0 * w).cos()));
        }
        let sum: f32 = real.iter().sum();
        let taps = real
            .iter()
            .enumerate()
            .map(|(i, x)| {
                Complex32::from_polar(
                    x / sum,
                    TAU * center * (i as f32 - (length - 1) as f32 * 0.5),
                )
            })
            .collect();
        Self {
            taps,
            history: vec![Complex32::default(); length],
            cursor: 0,
        }
    }

    pub fn push(&mut self, x: Complex32) {
        self.history[self.cursor] = x;
        self.cursor = (self.cursor + 1) % self.history.len();
    }

    /// Appelé seulement aux instants de décimation : évite les convolutions inutiles.
    pub fn output(&self) -> Complex32 {
        // Deux tranches contiguës remplacent un modulo par tap (important à 2,4 MS/s).
        self.taps[..self.cursor]
            .iter()
            .zip(self.history[..self.cursor].iter().rev())
            .chain(
                self.taps[self.cursor..]
                    .iter()
                    .zip(self.history[self.cursor..].iter().rev()),
            )
            .map(|(tap, sample)| tap * sample)
            .sum()
    }
}
