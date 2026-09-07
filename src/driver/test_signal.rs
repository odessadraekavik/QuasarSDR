use super::SdrSource;
use num_complex::Complex32;
use std::f32::consts::TAU;
pub struct SimulatedSource {
    sample: u64,
    phases: [f32; 3],
    random: u32,
}

impl Default for SimulatedSource {
    fn default() -> Self {
        Self {
            sample: 0,
            phases: [0.0; 3],
            random: 0x1234abcd,
        }
    }
}

impl SimulatedSource {
    fn noise(&mut self) -> f32 {
        self.random ^= self.random << 13;
        self.random ^= self.random >> 17;
        self.random ^= self.random << 5;
        (self.random as f32 / u32::MAX as f32 - 0.5) * 0.035
    }
}

impl SdrSource for SimulatedSource {
    fn sample_rate(&self) -> u32 {
        crate::model::IQ_RATE
    }

    fn read(&mut self, output: &mut [Complex32]) -> Result<(), String> {
        let fs = self.sample_rate() as f32;
        for iq in output {
            let t = (self.sample % (self.sample_rate() as u64 * 60)) as f32 / fs;
            let mut sum = Complex32::new(self.noise(), self.noise());
            for (ch, offset) in [-45_000.0, 15_000.0, 55_000.0].iter().enumerate() {
                let active = match ch {
                    0 => t % 8.0 < 3.5,
                    1 => (t + 2.0) % 7.0 < 4.0,
                    _ => (t + 1.0) % 10.0 < 3.0,
                };
                let audio = (TAU * (600.0 + 260.0 * ch as f32) * t).sin();
                let ctcss = 0.15 * (TAU * 100.0 * t).sin();
                let deviation = if ch < 2 {
                    2000.0 * (audio + ctcss)
                } else {
                    0.0
                };
                self.phases[ch] = (self.phases[ch] + TAU * (offset + deviation) / fs) % TAU;
                if active {
                    let amp = if ch == 2 {
                        0.15 * (1.0 + 0.65 * audio)
                    } else {
                        0.23
                    };
                    sum += Complex32::from_polar(amp, self.phases[ch]);
                }
            }
            *iq = sum;
            self.sample += 1;
        }
        Ok(())
    }
}
