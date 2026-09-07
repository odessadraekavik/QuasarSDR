//! RNNoise local : PCM mono 48 kHz, modèle embarqué, état indépendant par VFO.
use nnnoiseless::DenoiseState;
use std::collections::VecDeque;

const FRAME: usize = DenoiseState::FRAME_SIZE;

pub struct VoiceDenoiser {
    state: Box<DenoiseState<'static>>,
    input: [f32; FRAME],
    previous_dry: [f32; FRAME],
    filled: usize,
    output: VecDeque<f32>,
}

impl Default for VoiceDenoiser {
    fn default() -> Self {
        Self {
            state: DenoiseState::new(),
            input: [0.0; FRAME],
            previous_dry: [0.0; FRAME],
            filled: 0,
            output: VecDeque::from(vec![0.0; FRAME]),
        }
    }
}

impl VoiceDenoiser {
    pub fn process(&mut self, audio: &mut [f32], strength: f32) {
        let wet = strength.clamp(0.0, 1.0);
        for sample in audio {
            let dry = if sample.is_finite() {
                sample.clamp(-1.0, 1.0)
            } else {
                0.0
            };
            // Taille de sortie constante, même pour les blocs DSP de 512 samples.
            *sample = self
                .output
                .pop_front()
                .expect("file audio alimentée par trames");
            self.input[self.filled] = dry * 32768.0;
            self.filled += 1;
            if self.filled == FRAME {
                let mut clean = [0.0; FRAME];
                self.state.process_frame(&mut clean, &self.input);
                for (i, value) in clean.iter().enumerate() {
                    // RNNoise retarde d'une trame ; aligner le signal original
                    // avant mélange pour éviter un écho/filtrage en peigne.
                    let mixed = self.previous_dry[i] * (1.0 - wet) + value / 32768.0 * wet;
                    self.output.push_back(if mixed.is_finite() {
                        mixed.clamp(-1.0, 1.0)
                    } else {
                        0.0
                    });
                    self.previous_dry[i] = self.input[i] / 32768.0;
                }
                self.filled = 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arbitrary_chunks_preserve_audio_and_delay_alignment() {
        let input: Vec<_> = (0..5000).map(|i| (i as f32 * 0.03).sin() * 0.2).collect();
        let mut whole = input.clone();
        VoiceDenoiser::default().process(&mut whole, 0.0);
        assert!(whole[..2 * FRAME].iter().all(|x| *x == 0.0));
        assert_eq!(&whole[2 * FRAME..], &input[..input.len() - 2 * FRAME]);
        let mut chunks = input.clone();
        let mut denoiser = VoiceDenoiser::default();
        for chunk in chunks.chunks_mut(137) {
            denoiser.process(chunk, 0.0);
        }
        assert_eq!(chunks, whole);
    }

    #[test]
    fn neural_denoising_reduces_stationary_noise_and_stays_finite() {
        let mut seed = 42_u32;
        let mut audio: Vec<_> = (0..48_000)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                (seed as f32 / u32::MAX as f32 - 0.5) * 0.1
            })
            .collect();
        let before: f32 = audio[24_000..].iter().map(|x| x * x).sum();
        let mut denoiser = VoiceDenoiser::default();
        for chunk in audio.chunks_mut(512) {
            denoiser.process(chunk, 1.0);
        }
        assert!(audio.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
        let after: f32 = audio[24_000..].iter().map(|x| x * x).sum();
        assert!(after < before * 0.5, "bruit avant={before}, après={after}");
    }
}
