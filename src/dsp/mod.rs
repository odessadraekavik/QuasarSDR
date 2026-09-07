mod detector;
pub(crate) mod filter;
mod spectrum;
mod vfo;
mod voice;

use crate::model::{
    Settings, Snapshot, VfoStatus, AUDIO_DECIMATION, BLOCK_SIZE, FFT_SIZE, MAX_VFOS,
};
use num_complex::Complex32;
use spectrum::{channel_level_dbfs, channel_snr, SpectrumAnalyzer};
use vfo::Vfo;

/// Politique déterministe : priorité, puis canal courant en cas d'égalité.
pub fn select_vfo(
    settings: &Settings,
    statuses: &[VfoStatus],
    previous: Option<u32>,
) -> Option<u32> {
    let solo = settings.vfos.iter().any(|c| c.solo && !c.muted);
    settings
        .vfos
        .iter()
        .filter(|c| !c.muted && (!solo || c.solo))
        .filter(|c| {
            statuses.iter().any(|s| {
                s.id == c.id && s.squelch_open && s.tone_open && (!settings.use_vad || s.vad)
            })
        })
        .min_by_key(|c| (c.priority, Some(c.id) != previous, c.id))
        .map(|c| c.id)
}

pub struct Processor {
    spectrum: SpectrumAnalyzer,
    vfos: Vec<Vfo>,
    settings: Settings,
    selected: Option<u32>,
    gains: Vec<f32>,
}

impl Processor {
    pub fn new(settings: Settings) -> Self {
        let mut result = Self {
            spectrum: SpectrumAnalyzer::default(),
            vfos: Vec::new(),
            settings: Settings::default(),
            selected: None,
            gains: Vec::new(),
        };
        result.configure(settings);
        result
    }
    pub fn configure(&mut self, mut settings: Settings) {
        settings.vfos.truncate(MAX_VFOS);
        if self.settings.center_hz != settings.center_hz
            || self.settings.rf_gain_db != settings.rf_gain_db
            || self.settings.noise_zero_request != settings.noise_zero_request
        {
            self.spectrum = SpectrumAnalyzer::default();
        }
        let mut previous = std::mem::take(&mut self.vfos);
        self.vfos = settings
            .vfos
            .iter()
            .map(|c| {
                if let Some(i) = previous.iter().position(|v| v.config.id == c.id) {
                    let mut v = previous.remove(i);
                    v.configure(c.clone(), settings.center_hz);
                    v
                } else {
                    Vfo::new(c.clone(), settings.center_hz)
                }
            })
            .collect();
        self.gains = vec![0.0; self.vfos.len()];
        self.settings = settings;
    }
    pub fn process(&mut self, iq: &[Complex32]) -> (Vec<f32>, Snapshot) {
        assert_eq!(iq.len(), BLOCK_SIZE);
        let spectrum = self.spectrum.process(&iq[..FFT_SIZE]);
        let mut channels = Vec::with_capacity(self.vfos.len());
        let mut statuses = Vec::with_capacity(self.vfos.len());
        for vfo in &mut self.vfos {
            if !vfo.config.in_band(self.settings.center_hz) {
                channels.push(vec![0.0; iq.len() / AUDIO_DECIMATION]);
                statuses.push(VfoStatus {
                    id: vfo.config.id,
                    level_dbfs: -140.0,
                    ..Default::default()
                });
                continue;
            }
            let offset = vfo.config.offset_hz(self.settings.center_hz);
            let snr = channel_snr(&spectrum, offset, vfo.config.mode.bandwidth());
            let level = channel_level_dbfs(&spectrum, offset, vfo.config.mode.bandwidth());
            let threshold = if self.settings.adaptive_squelch {
                self.settings.sql_relative_db
            } else {
                self.settings.sql_dbfs
            };
            let (mut audio, status) =
                vfo.process(iq, snr, level, threshold, self.settings.adaptive_squelch);
            // CTCSS, VAD et SQL voient le signal original, jamais celui de l'IA.
            vfo.clean_voice(
                &mut audio,
                self.settings.voice_denoise && status.squelch_open && status.tone_open,
                self.settings.voice_strength,
            );
            channels.push(audio);
            statuses.push(status);
        }
        self.selected = select_vfo(&self.settings, &statuses, self.selected);
        let solo = self.settings.vfos.iter().any(|c| c.solo && !c.muted);
        for (s, c) in statuses.iter_mut().zip(&self.settings.vfos) {
            s.audible = if self.settings.mix {
                !c.muted
                    && (!solo || c.solo)
                    && s.squelch_open
                    && s.tone_open
                    && (!self.settings.use_vad || s.vad)
            } else {
                Some(c.id) == self.selected
            };
        }
        let divisor = statuses.iter().filter(|s| s.audible).count().max(1) as f32;
        let mut output = vec![0.0; iq.len() / AUDIO_DECIMATION];
        for (i, (audio, status)) in channels.iter().zip(&statuses).enumerate() {
            let target = if status.audible {
                self.settings.vfos[i].gain / divisor
            } else {
                0.0
            };
            for (out, sample) in output.iter_mut().zip(audio) {
                // Rampe ~5 ms pour atténuer les clics du swapper.
                self.gains[i] += (target - self.gains[i]).clamp(-1.0 / 240.0, 1.0 / 240.0);
                *out += sample * self.gains[i] * self.settings.master_gain;
            }
        }
        for x in &mut output {
            *x = x.clamp(-0.95, 0.95);
        }
        (
            output,
            Snapshot {
                zero_progress: self.spectrum.zero_progress(),
                spectrum,
                vfos: statuses,
                selected: self.selected,
                dsp_ms: 0.0,
                center_hz: self.settings.center_hz,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        driver::{SdrSource, SimulatedSource},
        model::*,
    };
    use std::f32::consts::TAU;

    fn multichannel_settings() -> Settings {
        Settings {
            vfos: vec![
                VfoConfig::new(1, -45_000.0, Mode::Nfm),
                VfoConfig::new(2, 15_000.0, Mode::Am),
                VfoConfig::new(3, 55_000.0, Mode::Usb),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn fft_places_positive_frequency_right_of_center() {
        let iq: Vec<_> = (0..FFT_SIZE)
            .map(|i| Complex32::from_polar(1.0, TAU * 32.0 * i as f32 / FFT_SIZE as f32))
            .collect();
        let s = SpectrumAnalyzer::default().process(&iq);
        let peak = s
            .power_db
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        assert_eq!(peak, FFT_SIZE / 2 + 32);
        assert!(s.power_db[peak].abs() < 0.01);
    }

    #[test]
    fn swapper_respects_priority_mute_solo_and_tone() {
        let mut settings = multichannel_settings();
        let mut statuses: Vec<_> = settings
            .vfos
            .iter()
            .map(|c| VfoStatus {
                id: c.id,
                squelch_open: true,
                tone_open: true,
                ..Default::default()
            })
            .collect();
        assert_eq!(select_vfo(&settings, &statuses, Some(2)), Some(1));
        settings.vfos[0].muted = true;
        assert_eq!(select_vfo(&settings, &statuses, None), Some(2));
        settings.vfos[2].solo = true;
        assert_eq!(select_vfo(&settings, &statuses, None), Some(3));
        statuses[2].tone_open = false;
        assert_eq!(select_vfo(&settings, &statuses, None), None);
    }

    #[test]
    fn ctcss_accepts_target_and_rejects_other_tone() {
        for (hz, expected) in [(100.0, true), (150.0, false)] {
            let mut detector = detector::ToneDetector::new(100.0, AUDIO_RATE);
            for i in 0..9600 {
                detector.push((TAU * hz * i as f32 / AUDIO_RATE as f32).sin());
            }
            assert_eq!(detector.detected, expected);
        }
    }

    #[test]
    fn simulated_iq_produces_finite_audio_and_active_vfos() {
        let mut source = SimulatedSource::default();
        let mut processor = Processor::new(multichannel_settings());
        let mut iq = vec![Complex32::default(); BLOCK_SIZE];
        let mut energy = 0.0;
        for _ in 0..30 {
            source.read(&mut iq).unwrap();
            let (audio, snapshot) = processor.process(&iq);
            assert_eq!(audio.len(), BLOCK_SIZE / AUDIO_DECIMATION);
            assert!(audio.iter().all(|x| x.is_finite() && x.abs() <= 0.95));
            energy += audio.iter().map(|x| x * x).sum::<f32>();
            assert_eq!(snapshot.selected, Some(1));
        }
        assert!(energy > 0.1);
    }

    #[test]
    fn ssb_filter_rejects_opposite_sideband() {
        let energy = |mode, frequency| {
            let mut vfo = Vfo::new(VfoConfig::new(1, 0.0, mode), CENTER_HZ);
            let mut total = 0.0;
            for block in 0..12 {
                let iq: Vec<_> = (0..BLOCK_SIZE)
                    .map(|i| {
                        Complex32::from_polar(
                            0.3,
                            TAU * frequency * (block * BLOCK_SIZE + i) as f32 / IQ_RATE as f32,
                        )
                    })
                    .collect();
                let (audio, _) = vfo.process(&iq, 40.0, -10.0, -50.0, false);
                if block > 1 {
                    total += audio.iter().map(|x| x * x).sum::<f32>();
                }
            }
            total
        };
        for (mode, hz) in [(Mode::Usb, 1500.0), (Mode::Lsb, -1500.0)] {
            assert!(energy(mode, hz) > 100.0 * energy(mode, -hz));
        }
    }

    #[test]
    fn fm_demodulation_recovers_audio_tone_in_both_modes() {
        for (mode, deviation) in [(Mode::Nfm, 2000.0), (Mode::Wfm, 50_000.0)] {
            let mut vfo = Vfo::new(VfoConfig::new(1, 0.0, mode), CENTER_HZ);
            let mut phase = 0.0;
            let mut audio = Vec::new();
            for block in 0..16 {
                let iq: Vec<_> = (0..BLOCK_SIZE)
                    .map(|i| {
                        let t = (block * BLOCK_SIZE + i) as f32 / IQ_RATE as f32;
                        phase += TAU * deviation * (TAU * 1000.0 * t).sin() / IQ_RATE as f32;
                        Complex32::from_polar(0.4, phase)
                    })
                    .collect();
                audio.extend(vfo.process(&iq, 40.0, -10.0, -50.0, false).0);
            }
            let projection = |hz: f32| -> f32 {
                audio
                    .iter()
                    .enumerate()
                    .skip(1024)
                    .map(|(i, sample)| {
                        Complex32::from_polar(*sample, TAU * hz * i as f32 / AUDIO_RATE as f32)
                    })
                    .sum::<Complex32>()
                    .norm_sqr()
            };
            assert!(projection(1000.0) > 100.0 * projection(3000.0));
        }
    }

    #[test]
    fn global_sql_rejects_weak_signal_even_with_high_spectral_contrast() {
        for (amplitude, expected) in [(0.001, None), (0.01, Some(1))] {
            let settings = Settings {
                vfos: vec![VfoConfig::new(1, 0.0, Mode::Am)],
                adaptive_squelch: false,
                ..Default::default()
            };
            assert_eq!(settings.sql_dbfs, -50.0);
            let mut processor = Processor::new(settings);
            let iq = vec![Complex32::new(amplitude, 0.0); BLOCK_SIZE];
            let (_, snapshot) = processor.process(&iq);
            assert!((snapshot.vfos[0].level_dbfs - 20.0 * amplitude.log10()).abs() < 0.1);
            assert!(snapshot.vfos[0].snr_db > 30.0);
            assert_eq!(snapshot.selected, expected);
        }
    }

    #[test]
    fn vfos_outside_capture_band_are_silent_and_never_alias_into_audio() {
        let settings = Settings {
            center_hz: 145_500_000.0,
            ..Default::default()
        };
        let mut processor = Processor::new(settings);
        let (audio, snapshot) = processor.process(&vec![Complex32::new(0.4, 0.0); BLOCK_SIZE]);
        assert_eq!(snapshot.center_hz, 145_500_000.0);
        assert!(snapshot.vfos.iter().all(|v| !v.in_band && !v.audible));
        assert!(audio.iter().all(|x| *x == 0.0));
        assert_eq!(audio.len(), 512);
    }

    #[test]
    fn relative_sql_is_independent_of_absolute_noise_level() {
        for floor in [-120.0, -70.0] {
            for (contrast, expected) in [(3.0, false), (12.0, true)] {
                let mut spectrum = Spectrum {
                    power_db: vec![floor; FFT_SIZE],
                    floor_db: vec![floor; FFT_SIZE],
                };
                spectrum.power_db[FFT_SIZE / 2] += contrast;
                let metric = spectrum::channel_snr(&spectrum, 0.0, Mode::Nfm.bandwidth());
                let mut vfo = Vfo::new(VfoConfig::new(1, 0.0, Mode::Nfm), CENTER_HZ);
                let (_, status) = vfo.process(
                    &vec![Complex32::default(); BLOCK_SIZE],
                    metric,
                    floor + contrast,
                    10.0,
                    true,
                );
                assert_eq!(status.squelch_open, expected);
            }
        }
    }

    #[test]
    fn auto_zero_finishes_and_restarts_only_for_rf_changes_or_request() {
        let mut settings = Settings::default();
        let mut processor = Processor::new(settings.clone());
        let iq = vec![Complex32::new(0.01, 0.0); FFT_SIZE];
        for _ in 0..188 {
            processor.spectrum.process(&iq);
        }
        assert_eq!(processor.spectrum.zero_progress(), 1.0);
        settings.master_gain = 0.1;
        processor.configure(settings.clone());
        assert_eq!(processor.spectrum.zero_progress(), 1.0);
        for change in 0..3 {
            match change {
                0 => settings.noise_zero_request += 1,
                1 => settings.rf_gain_db = 20.7,
                _ => settings.center_hz += 100_000.0,
            }
            processor.configure(settings.clone());
            assert_eq!(processor.spectrum.zero_progress(), 0.0);
            for _ in 0..188 {
                processor.spectrum.process(&iq);
            }
            assert_eq!(processor.spectrum.zero_progress(), 1.0);
        }
        // Une porteuse continue n'a pas été absorbée par le bruit de référence.
        let spectrum = processor.spectrum.process(&iq);
        assert!(spectrum::channel_snr(&spectrum, 0.0, 25_000.0) > 50.0);
    }

    #[test]
    fn voice_filter_does_not_change_rf_detection_or_tone_decisions() {
        let settings = multichannel_settings();
        let mut plain = Processor::new(settings.clone());
        let mut filtered = Processor::new(Settings {
            voice_denoise: true,
            ..settings
        });
        let mut source = SimulatedSource::default();
        let mut iq = vec![Complex32::default(); BLOCK_SIZE];
        for _ in 0..8 {
            source.read(&mut iq).unwrap();
            let (_, a) = plain.process(&iq);
            let (audio, b) = filtered.process(&iq);
            assert_eq!(a.selected, b.selected);
            assert_eq!(audio.len(), BLOCK_SIZE / AUDIO_DECIMATION);
            assert!(audio.iter().all(|x| x.is_finite()));
            for (a, b) in a.vfos.iter().zip(&b.vfos) {
                assert_eq!(a.snr_db, b.snr_db);
                assert_eq!(
                    (a.squelch_open, a.vad, a.tone_open),
                    (b.squelch_open, b.vad, b.tone_open)
                );
            }
        }
    }

    #[test]
    fn noise_normalization_tracks_gain_without_changing_contrast() {
        let mut low = SpectrumAnalyzer::default();
        let mut high = SpectrumAnalyzer::default();
        let mut seed = 42_u32;
        for frame in 0..25 {
            let mut noise = || {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                (seed as f32 / u32::MAX as f32 - 0.5) * 0.006
            };
            let iq: Vec<_> = (0..FFT_SIZE)
                .map(|_| Complex32::new(noise(), noise()))
                .collect();
            let a = low.process(&iq);
            let b = high.process(&iq.iter().map(|x| x * 100.0).collect::<Vec<_>>());
            if frame == 24 {
                let mut mean = 0.0;
                for i in 0..FFT_SIZE {
                    assert!((b.floor_db[i] - a.floor_db[i] - 40.0).abs() < 0.02);
                    assert!(
                        ((a.power_db[i] - a.floor_db[i]) - (b.power_db[i] - b.floor_db[i])).abs()
                            < 0.02
                    );
                    mean += a.power_db[i] - a.floor_db[i];
                }
                assert!((mean / FFT_SIZE as f32).abs() < 2.0);
            }
        }
    }
}
