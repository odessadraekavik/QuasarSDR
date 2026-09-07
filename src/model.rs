pub const IQ_RATE: u32 = 2_400_000;
pub const CHANNEL_RATE: u32 = 240_000;
pub const AUDIO_RATE: u32 = 48_000;
pub const BLOCK_SIZE: usize = 25_600;
pub const AUDIO_DECIMATION: usize = (IQ_RATE / AUDIO_RATE) as usize;
pub const FFT_SIZE: usize = 2048;
/// Canal VHF marine international 16.
pub const CENTER_HZ: f64 = 156_800_000.0;
pub const MAX_VFOS: usize = 8;
pub const MIN_RF_HZ: f64 = 500_000.0;
pub const MAX_RF_HZ: f64 = 1_766_000_000.0;

pub fn parse_mhz(text: &str) -> Result<f64, String> {
    let hz = text
        .trim()
        .replace(',', ".")
        .parse::<f64>()
        .map_err(|_| "Fréquence invalide : saisir des MHz, par exemple 156.800".to_owned())?
        * 1e6;
    if !hz.is_finite() || !(MIN_RF_HZ..=MAX_RF_HZ).contains(&hz) {
        return Err("Fréquence hors plage : 0,500 à 1766 MHz (Blog V4)".into());
    }
    Ok(hz.round())
}

pub fn pan_center(start_hz: f64, drag_pixels: f32, width: f32) -> f64 {
    (start_hz - drag_pixels as f64 / width.max(1.0) as f64 * IQ_RATE as f64)
        .clamp(MIN_RF_HZ, MAX_RF_HZ)
        .round()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Nfm,
    Wfm,
    Am,
    Lsb,
    Usb,
}

impl Mode {
    pub const ALL: [Self; 5] = [Self::Nfm, Self::Wfm, Self::Am, Self::Lsb, Self::Usb];
    pub fn label(self) -> &'static str {
        match self {
            Self::Nfm => "NFM",
            Self::Wfm => "WFM mono",
            Self::Am => "AM",
            Self::Lsb => "LSB",
            Self::Usb => "USB",
        }
    }
    pub fn bandwidth(self) -> f32 {
        match self {
            Self::Nfm => 25_000.0,
            Self::Wfm => 160_000.0,
            Self::Am => 10_000.0,
            Self::Lsb | Self::Usb => 6_000.0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct VfoConfig {
    pub id: u32,
    pub frequency_hz: f64,
    pub mode: Mode,
    pub squelch_db: f32,
    pub gain: f32,
    pub muted: bool,
    pub solo: bool,
    /// Les petites valeurs gagnent ; égalité départagée par l'identifiant.
    pub priority: u8,
    pub ctcss_hz: Option<f32>,
}

impl VfoConfig {
    pub fn new(id: u32, offset_hz: f32, mode: Mode) -> Self {
        Self {
            id,
            frequency_hz: CENTER_HZ + offset_hz as f64,
            mode,
            squelch_db: 0.0,
            gain: 0.5,
            muted: false,
            solo: false,
            priority: id.min(255) as u8,
            ctcss_hz: None,
        }
    }
    pub fn offset_hz(&self, center_hz: f64) -> f32 {
        (self.frequency_hz - center_hz) as f32
    }
    pub fn in_band(&self, center_hz: f64) -> bool {
        // Marge de 2 % aux bords du spectre, où les filtres du tuner s'atténuent.
        (self.frequency_hz - center_hz).abs() + self.mode.bandwidth() as f64 / 2.0
            <= IQ_RATE as f64 * 0.48
    }
}

#[derive(Clone, Debug)]
pub struct Settings {
    pub vfos: Vec<VfoConfig>,
    pub mix: bool,
    pub use_vad: bool,
    pub master_gain: f32,
    /// Seuil absolu global, puissance intégrée dans le canal en dBFS.
    pub sql_dbfs: f32,
    /// Référence commune : dB au-dessus du plancher de bruit local.
    pub sql_relative_db: f32,
    pub adaptive_squelch: bool,
    /// Incrémenter pour relancer l'apprentissage du bruit sans réaccorder le SDR.
    pub noise_zero_request: u64,
    pub voice_denoise: bool,
    pub voice_strength: f32,
    pub rf_gain_db: f32,
    pub center_hz: f64,
}

impl Settings {
    pub fn tune_vfo(&mut self, id: u32, frequency_hz: f64) {
        if !frequency_hz.is_finite() {
            return;
        }
        if let Some(vfo) = self.vfos.iter_mut().find(|v| v.id == id) {
            vfo.frequency_hz = frequency_hz.clamp(MIN_RF_HZ, MAX_RF_HZ).round();
            if !vfo.in_band(self.center_hz) {
                self.center_hz = vfo.frequency_hz;
            }
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            vfos: vec![VfoConfig::new(1, 0.0, Mode::Nfm)],
            mix: false,
            use_vad: false,
            master_gain: 0.35,
            sql_dbfs: -50.0,
            sql_relative_db: 10.0,
            adaptive_squelch: true,
            noise_zero_request: 0,
            voice_denoise: false,
            voice_strength: 0.8,
            rf_gain_db: 28.0,
            center_hz: CENTER_HZ,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct VfoStatus {
    pub id: u32,
    pub snr_db: f32,
    pub level_dbfs: f32,
    pub squelch_open: bool,
    pub vad: bool,
    pub tone_open: bool,
    pub audible: bool,
    pub in_band: bool,
}

#[derive(Clone, Debug)]
pub struct Spectrum {
    /// FFT centrée : indice zéro = -Fs/2. dBFS/bin, pas une calibration RF.
    pub power_db: Vec<f32>,
    pub floor_db: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub zero_progress: f32,
    pub spectrum: Spectrum,
    pub vfos: Vec<VfoStatus>,
    pub selected: Option<u32>,
    pub dsp_ms: f32,
    pub center_hz: f64,
}

#[cfg(test)]
mod tuning_tests {
    use super::*;

    #[test]
    fn marine_frequency_parses_in_both_locales_and_rejects_invalid_input() {
        assert_eq!(parse_mhz("156.800"), Ok(156_800_000.0));
        assert_eq!(parse_mhz("156,800"), Ok(156_800_000.0));
        for bad in ["", "abc", "NaN", "inf", "-156", "2000"] {
            assert!(parse_mhz(bad).is_err());
        }
    }

    #[test]
    fn tuning_outside_window_retunes_without_moving_other_vfos() {
        let mut settings = Settings {
            center_hz: 145_500_000.0,
            ..Default::default()
        };
        let mut other_vfo = VfoConfig::new(2, 0.0, Mode::Nfm);
        other_vfo.frequency_hz = 145_515_000.0;
        settings.vfos.push(other_vfo);
        let other = settings.vfos[1].frequency_hz;
        settings.tune_vfo(1, 156_800_000.0);
        assert_eq!(settings.center_hz, 156_800_000.0);
        assert_eq!(settings.vfos[0].frequency_hz, 156_800_000.0);
        assert_eq!(settings.vfos[1].frequency_hz, other);
        assert!(!settings.vfos[1].in_band(settings.center_hz));
        settings.tune_vfo(1, 157_000_000.0);
        assert_eq!(settings.center_hz, 156_800_000.0);
    }

    #[test]
    fn panning_uses_full_capture_bandwidth_and_hardware_limits() {
        assert_eq!(pan_center(156_800_000.0, 100.0, 1000.0), 156_560_000.0);
        assert_eq!(pan_center(MIN_RF_HZ, 100.0, 1000.0), MIN_RF_HZ);
        assert_eq!(pan_center(MAX_RF_HZ, -100.0, 1000.0), MAX_RF_HZ);
    }

    #[test]
    fn defaults_receive_marine_channel_16_with_relative_sql() {
        let settings = Settings::default();
        assert_eq!(settings.center_hz, 156_800_000.0);
        assert_eq!(settings.vfos.len(), 1);
        assert_eq!(settings.vfos[0].frequency_hz, 156_800_000.0);
        assert_eq!(settings.vfos[0].mode, Mode::Nfm);
        assert_eq!(settings.vfos[0].squelch_db, 0.0);
        assert!(settings.adaptive_squelch);
        assert_eq!(settings.sql_relative_db, 10.0);
        assert_eq!(settings.rf_gain_db, 28.0);
    }
}
