use std::f32::consts::TAU;

/// Détection d'une tonalité CTCSS configurée, fenêtre de 200 ms.
/// Le rapport de puissance rejette les signaux énergétiques hors fréquence.
pub struct ToneDetector {
    coeff: f32,
    s1: f32,
    s2: f32,
    energy: f32,
    count: usize,
    length: usize,
    pub detected: bool,
}

impl ToneDetector {
    pub fn new(hz: f32, rate: u32) -> Self {
        Self {
            coeff: 2.0 * (TAU * hz / rate as f32).cos(),
            s1: 0.0,
            s2: 0.0,
            energy: 0.0,
            count: 0,
            length: rate as usize / 5,
            detected: false,
        }
    }
    pub fn push(&mut self, x: f32) {
        let s = x + self.coeff * self.s1 - self.s2;
        self.s2 = self.s1;
        self.s1 = s;
        self.energy += x * x;
        self.count += 1;
        if self.count == self.length {
            let power = self.s1 * self.s1 + self.s2 * self.s2 - self.coeff * self.s1 * self.s2;
            let ratio = 2.0 * power / (self.length as f32 * self.energy + 1e-12);
            self.detected = self.energy / self.length as f32 > 1e-7 && ratio > 0.008;
            self.s1 = 0.0;
            self.s2 = 0.0;
            self.energy = 0.0;
            self.count = 0;
        }
    }
}

/// VAD de départ : énergie audio + maintien, pas un classificateur de parole.
#[derive(Default)]
pub struct ActivityDetector {
    remaining: usize,
}

impl ActivityDetector {
    pub fn update(&mut self, audio: &[f32], rf_open: bool) -> bool {
        let energy = audio.iter().map(|x| x * x).sum::<f32>() / audio.len().max(1) as f32;
        if rf_open && energy > 0.0001 {
            self.remaining = 12_000;
        } else {
            self.remaining = self.remaining.saturating_sub(audio.len());
        }
        rf_open && self.remaining > 0
    }
}
