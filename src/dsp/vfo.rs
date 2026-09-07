use super::{
    detector::{ActivityDetector, ToneDetector},
    filter::Fir,
};
use crate::model::{
    Mode, VfoConfig, VfoStatus, AUDIO_DECIMATION, AUDIO_RATE, CHANNEL_RATE, IQ_RATE,
};
use num_complex::Complex32;
use std::f32::consts::TAU;

pub struct Vfo {
    pub config: VfoConfig,
    oscillator: Complex32,
    step: Complex32,
    center_hz: f64,
    prefilter: Fir,
    channel_filter: Fir,
    audio_filter: Fir,
    previous: Complex32,
    dc: f32,
    deemphasis: f32,
    tick: usize,
    squelch_hold: usize,
    activity: ActivityDetector,
    tone: Option<ToneDetector>,
    voice: Option<super::voice::VoiceDenoiser>,
}

impl Vfo {
    pub fn new(config: VfoConfig, center_hz: f64) -> Self {
        let (low, high) = match config.mode {
            Mode::Usb => (200.0, 3000.0),
            Mode::Lsb => (-3000.0, -200.0),
            _ => (
                -config.mode.bandwidth() * 0.5,
                config.mode.bandwidth() * 0.5,
            ),
        };
        let intermediate = if config.mode == Mode::Wfm {
            CHANNEL_RATE
        } else {
            AUDIO_RATE
        };
        Self {
            step: Complex32::from_polar(1.0, -TAU * config.offset_hz(center_hz) / IQ_RATE as f32),
            center_hz,
            prefilter: Fir::new(IQ_RATE as f32, -100_000.0, 100_000.0, 129),
            oscillator: Complex32::new(1.0, 0.0),
            channel_filter: Fir::new(
                CHANNEL_RATE as f32,
                low,
                high,
                if matches!(config.mode, Mode::Lsb | Mode::Usb) {
                    513
                } else {
                    129
                },
            ),
            audio_filter: Fir::new(intermediate as f32, -15_000.0, 15_000.0, 65),
            previous: Complex32::new(1.0, 0.0),
            dc: 0.0,
            deemphasis: 0.0,
            tick: 0,
            squelch_hold: 0,
            activity: ActivityDetector::default(),
            tone: config.ctcss_hz.map(|f| ToneDetector::new(f, AUDIO_RATE)),
            voice: None,
            config,
        }
    }

    pub fn configure(&mut self, config: VfoConfig, center_hz: f64) {
        if self.config.mode != config.mode
            || self.config.frequency_hz != config.frequency_hz
            || self.center_hz != center_hz
            || self.config.ctcss_hz != config.ctcss_hz
        {
            *self = Self::new(config, center_hz);
        } else {
            self.config = config;
        }
    }

    pub fn clean_voice(&mut self, audio: &mut [f32], enabled: bool, strength: f32) {
        if enabled {
            self.voice
                .get_or_insert_with(Default::default)
                .process(audio, strength);
        } else {
            // Ne jamais rejouer le reliquat d'une ancienne transmission.
            self.voice = None;
        }
    }

    pub fn process(
        &mut self,
        iq: &[Complex32],
        snr_db: f32,
        level_dbfs: f32,
        sql_threshold_db: f32,
        adaptive_squelch: bool,
    ) -> (Vec<f32>, VfoStatus) {
        let wide = self.config.mode == Mode::Wfm;
        let mut audio = Vec::with_capacity(iq.len() / AUDIO_DECIMATION);
        for x in iq {
            self.prefilter.push(x * self.oscillator);
            self.oscillator *= self.step;
            self.tick += 1;
            if self.tick.is_multiple_of(1024) {
                self.oscillator /= self.oscillator.norm().max(1e-12);
            }
            // DDC multidébit : 2,4 MS/s -> 240 kS/s -> 48 kS/s.
            if !self.tick.is_multiple_of((IQ_RATE / CHANNEL_RATE) as usize) {
                continue;
            }
            self.channel_filter.push(self.prefilter.output());
            if !wide && !self.tick.is_multiple_of(AUDIO_DECIMATION) {
                continue;
            }
            let baseband = self.channel_filter.output();
            let mut sample = match self.config.mode {
                Mode::Nfm | Mode::Wfm => {
                    let product = baseband * self.previous.conj();
                    self.previous = baseband;
                    let rate = if wide { CHANNEL_RATE } else { AUDIO_RATE } as f32;
                    let deviation = if wide { 75_000.0 } else { 5000.0 };
                    product.arg() * rate / (TAU * deviation)
                }
                Mode::Am => baseband.norm(),
                Mode::Lsb | Mode::Usb => baseband.re * 2.0,
            };
            if wide {
                // Désaccentuation 50 us + anti-repliement AVANT décimation 240 -> 48 kHz.
                let alpha = 1.0 - (-1.0 / (CHANNEL_RATE as f32 * 50e-6)).exp();
                self.deemphasis += alpha * (sample - self.deemphasis);
                self.audio_filter.push(Complex32::new(self.deemphasis, 0.0));
                if !self.tick.is_multiple_of(AUDIO_DECIMATION) {
                    continue;
                }
                sample = self.audio_filter.output().re;
            }
            // Suppression DC à ~15 Hz, conserve les tonalités sub-audibles.
            self.dc += 0.002 * (sample - self.dc);
            sample -= self.dc;
            if let Some(tone) = &mut self.tone {
                tone.push(sample);
            }
            audio.push(sample);
        }
        let threshold = sql_threshold_db + self.config.squelch_db
            - if self.squelch_hold > 0 { 3.0 } else { 0.0 };
        // Une seule référence à la fois. Un seuil dBFS caché annulerait
        // l'invariance recherchée entre des bandes aux bruits différents.
        let metric = if adaptive_squelch { snr_db } else { level_dbfs };
        if metric >= threshold {
            self.squelch_hold = AUDIO_RATE as usize / 7;
        } else {
            self.squelch_hold = self.squelch_hold.saturating_sub(audio.len());
        }
        let squelch_open = self.squelch_hold > 0;
        let vad = self.activity.update(&audio, squelch_open);
        let tone_open = self.tone.as_ref().is_none_or(|t| t.detected);
        (
            audio,
            VfoStatus {
                id: self.config.id,
                snr_db,
                level_dbfs,
                squelch_open,
                vad,
                tone_open,
                audible: false,
                in_band: true,
            },
        )
    }
}
