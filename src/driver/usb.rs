//! ABI librtlsdr : liste fermée de fonctions RX, aucune commande TX/EEPROM/bias tee.
use super::SdrSource;
use crate::model::{BLOCK_SIZE, IQ_RATE, MAX_RF_HZ, MIN_RF_HZ};
use crossbeam_channel::{bounded, Receiver, Sender};
use libloading::Library;
use num_complex::Complex32;
use std::{
    ffi::{c_char, c_int, c_void, CStr},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

const USB_BYTES: usize = BLOCK_SIZE * 2;
type Handle = *mut c_void;
type Callback = unsafe extern "C" fn(*mut u8, u32, *mut c_void);
type DeviceCall = unsafe extern "C" fn(Handle) -> c_int;
type SetU32 = unsafe extern "C" fn(Handle, u32) -> c_int;

pub struct RtlApi {
    count: unsafe extern "C" fn() -> u32,
    name: unsafe extern "C" fn(u32) -> *const c_char,
    strings: unsafe extern "C" fn(u32, *mut c_char, *mut c_char, *mut c_char) -> c_int,
    open: unsafe extern "C" fn(*mut Handle, u32) -> c_int,
    close: DeviceCall,
    rate: SetU32,
    frequency: SetU32,
    read_frequency: unsafe extern "C" fn(Handle) -> u32,
    read_rate: unsafe extern "C" fn(Handle) -> u32,
    gain_mode: unsafe extern "C" fn(Handle, c_int) -> c_int,
    gains: unsafe extern "C" fn(Handle, *mut c_int) -> c_int,
    gain: unsafe extern "C" fn(Handle, c_int) -> c_int,
    read_gain: DeviceCall,
    reset: DeviceCall,
    read_async: unsafe extern "C" fn(Handle, Callback, *mut c_void, u32, u32) -> c_int,
    cancel: DeviceCall,
    _library: Library,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsbDevice {
    pub index: u32,
    pub name: String,
    pub serial: String,
}

impl UsbDevice {
    /// Windows peut refuser une seconde ouverture pour lire les descripteurs.
    /// Conserver le libellé du handle ACTIF seulement ; le compte USB est toujours
    /// recontrôlé et le flux surveille lui-même son éventuelle déconnexion.
    pub fn preserve_active_label(devices: &mut [Self], active: &Self) {
        if let Some(device) = devices
            .iter_mut()
            .find(|d| d.index == active.index && d.serial.is_empty())
        {
            device.name.clone_from(&active.name);
            device.serial.clone_from(&active.serial);
        }
    }

    pub fn label(&self) -> String {
        format!(
            "{} · USB {} · S/N {}",
            self.name,
            self.index + 1,
            if self.serial.is_empty() {
                "indisponible"
            } else {
                &self.serial
            }
        )
    }
}

impl RtlApi {
    pub fn load() -> Result<Arc<Self>, String> {
        let mut paths = Vec::new();
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                paths.push(dir.join("rtlsdr.dll"));
                paths.push(dir.join("drivers/rtlsdr.dll"));
            }
        }
        paths.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("drivers/rtlsdr.dll"));
        let path = paths.into_iter().find(|p| p.is_file()).ok_or(
            "Driver RTL-SDR absent : placer rtlsdr.dll x64 et ses dépendances dans drivers/ à côté du programme.")?;
        // SAFETY : signatures de rtl-sdr.h ; DLL utilisateur chargée par chemin absolu.
        #[cfg(windows)]
        let library: Library = unsafe {
            libloading::os::windows::Library::load_with_flags(&path, 0x00000100 | 0x00001000)
        }
        .map_err(|e| format!("Chargement du driver USB : {e}"))?
        .into();
        #[cfg(not(windows))]
        let library = unsafe { Library::new(&path) }.map_err(|e| e.to_string())?;
        unsafe {
            macro_rules! symbol {
                ($name:literal) => {
                    *library
                        .get(concat!($name, "\0").as_bytes())
                        .map_err(|e| format!("API RTL-SDR : {e}"))?
                };
            }
            Ok(Arc::new(Self {
                count: symbol!("rtlsdr_get_device_count"),
                name: symbol!("rtlsdr_get_device_name"),
                strings: symbol!("rtlsdr_get_device_usb_strings"),
                open: symbol!("rtlsdr_open"),
                close: symbol!("rtlsdr_close"),
                rate: symbol!("rtlsdr_set_sample_rate"),
                frequency: symbol!("rtlsdr_set_center_freq"),
                read_frequency: symbol!("rtlsdr_get_center_freq"),
                read_rate: symbol!("rtlsdr_get_sample_rate"),
                gain_mode: symbol!("rtlsdr_set_tuner_gain_mode"),
                gains: symbol!("rtlsdr_get_tuner_gains"),
                gain: symbol!("rtlsdr_set_tuner_gain"),
                read_gain: symbol!("rtlsdr_get_tuner_gain"),
                reset: symbol!("rtlsdr_reset_buffer"),
                read_async: symbol!("rtlsdr_read_async"),
                cancel: symbol!("rtlsdr_cancel_async"),
                _library: library,
            }))
        }
    }

    pub fn devices(&self) -> Vec<UsbDevice> {
        // SAFETY : buffers 256 octets exigés par l'ABI, terminaison explicite.
        unsafe {
            (0..(self.count)())
                .map(|index| {
                    let mut vendor = [0_i8; 256];
                    let mut product = [0_i8; 256];
                    let mut serial = [0_i8; 256];
                    let result = (self.strings)(
                        index,
                        vendor.as_mut_ptr(),
                        product.as_mut_ptr(),
                        serial.as_mut_ptr(),
                    );
                    vendor[255] = 0;
                    product[255] = 0;
                    serial[255] = 0;
                    let text = |b: &[i8]| {
                        CStr::from_ptr(b.as_ptr())
                            .to_string_lossy()
                            .trim()
                            .to_owned()
                    };
                    let generic = (self.name)(index);
                    let name = if result == 0 && product[0] != 0 {
                        format!("{} {}", text(&vendor), text(&product))
                            .trim()
                            .to_owned()
                    } else if !generic.is_null() {
                        CStr::from_ptr(generic).to_string_lossy().into_owned()
                    } else {
                        "RTL-SDR".into()
                    };
                    UsbDevice {
                        index,
                        name,
                        serial: if result == 0 {
                            text(&serial)
                        } else {
                            String::new()
                        },
                    }
                })
                .collect()
        }
    }
}

/// Énumération USB hors UI, rafraîchie toutes les deux secondes.
pub struct DeviceMonitor {
    pub updates: Receiver<Result<Vec<UsbDevice>, String>>,
    stop: Arc<AtomicBool>,
}

impl Default for DeviceMonitor {
    fn default() -> Self {
        let (tx, updates) = bounded(1);
        let stop = Arc::new(AtomicBool::new(false));
        let cancelled = stop.clone();
        thread::spawn(move || {
            let mut api = None;
            while !cancelled.load(Ordering::Relaxed) {
                let update = match api.get_or_insert_with(RtlApi::load) {
                    Ok(api) => Ok(api.devices()),
                    Err(e) => Err(e.clone()),
                };
                let _ = tx.try_send(update);
                if api.as_ref().is_some_and(Result::is_err) {
                    api = None;
                }
                for _ in 0..20 {
                    if cancelled.load(Ordering::Relaxed) {
                        return;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            }
        });
        Self { updates, stop }
    }
}

impl Drop for DeviceMonitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

struct CallbackState {
    tx: Sender<(u64, Vec<u8>)>,
    sequence: u64,
    dropped: Arc<AtomicU64>,
    stopping: Arc<AtomicBool>,
}

unsafe extern "C" fn receive_usb(bytes: *mut u8, len: u32, context: *mut c_void) {
    // SAFETY : contexte vivant jusqu'au retour de read_async. Copie avant restitution
    // du buffer à librtlsdr. Le callback ne bloque pas sur le DSP.
    if bytes.is_null() || context.is_null() || len as usize != USB_BYTES {
        return;
    }
    let state = unsafe { &mut *context.cast::<CallbackState>() };
    if state.stopping.load(Ordering::Relaxed) {
        return;
    }
    let data = unsafe { std::slice::from_raw_parts(bytes, len as usize) }.to_vec();
    if state.tx.try_send((state.sequence, data)).is_err() {
        state.dropped.fetch_add(1, Ordering::Relaxed);
    }
    state.sequence += 1;
}

pub struct UsbSource {
    rx: Receiver<(u64, Vec<u8>)>,
    api: Arc<RtlApi>,
    // Sérialise cancel et close ; jamais tenu pendant read_async.
    handle: Arc<Mutex<Option<usize>>>,
    worker: Option<JoinHandle<()>>,
    expected: u64,
    stopping: Arc<AtomicBool>,
    pub gain_steps: Vec<f32>,
    pub applied_gain_db: f32,
    pub center_hz: f64,
}

impl UsbSource {
    pub fn open(
        selected: &UsbDevice,
        dropped: Arc<AtomicU64>,
        gain_db: f32,
        center_hz: f64,
    ) -> Result<Self, String> {
        if !center_hz.is_finite() || !(MIN_RF_HZ..=MAX_RF_HZ).contains(&center_hz) {
            return Err("Centre RF hors plage".into());
        }
        let api = RtlApi::load()?;
        if !api.devices().contains(selected) {
            return Err(
                "Le SDR sélectionné n'est plus connecté. Sélectionnez-le à nouveau.".into(),
            );
        }
        let mut raw = std::ptr::null_mut();
        // SAFETY : pointeur de sortie valide ; propriété transférée au worker ci-dessous.
        let result = unsafe { (api.open)(&mut raw, selected.index) };
        if result != 0 || raw.is_null() {
            return Err(format!("Impossible d'ouvrir le SDR USB ({result}). Fermez les autres logiciels SDR et vérifiez WinUSB."));
        }
        let configuration = unsafe {
            [
                ("débit RX", (api.rate)(raw, IQ_RATE)),
                (
                    "fréquence RX",
                    (api.frequency)(raw, center_hz.round() as u32),
                ),
                ("gain manuel", (api.gain_mode)(raw, 1)),
                ("buffers RX", (api.reset)(raw)),
            ]
        };
        if let Some((stage, code)) = configuration.iter().find(|(_, code)| *code != 0) {
            unsafe {
                (api.close)(raw);
            }
            return Err(format!("Configuration {stage} impossible ({code})"));
        }
        let actual_center = unsafe { (api.read_frequency)(raw) } as f64;
        let actual_rate = unsafe { (api.read_rate)(raw) };
        if actual_center != center_hz.round() || actual_rate != IQ_RATE {
            unsafe {
                (api.close)(raw);
            }
            return Err(format!(
                "Réglage RF non confirmé : centre {actual_center} Hz, débit {actual_rate} S/s"
            ));
        }
        // Le nombre de gains est propre au tuner, pas un intervalle arbitraire.
        let count = unsafe { (api.gains)(raw, std::ptr::null_mut()) };
        if !(1..=256).contains(&count) {
            unsafe {
                (api.close)(raw);
            }
            return Err("Le tuner ne fournit pas de gains RF manuels compatibles".into());
        }
        let mut gain_tenths = vec![0; count as usize];
        unsafe {
            (api.gains)(raw, gain_tenths.as_mut_ptr());
        }
        let gain_steps: Vec<f32> = gain_tenths.iter().map(|g| *g as f32 / 10.0).collect();
        let selected_gain = closest_gain(&gain_steps, gain_db);
        let code = unsafe { (api.gain)(raw, (selected_gain * 10.0).round() as c_int) };
        if code != 0 {
            unsafe {
                (api.close)(raw);
            }
            return Err(format!("Réglage du gain RF impossible ({code})"));
        }
        let applied_gain_db = unsafe { (api.read_gain)(raw) } as f32 / 10.0;
        let handle = Arc::new(Mutex::new(Some(raw as usize)));
        let worker_handle = handle.clone();
        let worker_api = api.clone();
        let (tx, rx) = bounded(4);
        let stopping = Arc::new(AtomicBool::new(false));
        let worker_stopping = stopping.clone();
        let worker = thread::spawn(move || {
            let address = worker_handle.lock().unwrap().unwrap();
            let mut state = CallbackState {
                tx,
                sequence: 0,
                dropped,
                stopping: worker_stopping,
            };
            // SAFETY : le handle et le contexte restent valides jusqu'au retour.
            unsafe {
                (worker_api.read_async)(
                    address as Handle,
                    receive_usb,
                    (&mut state as *mut CallbackState).cast(),
                    8,
                    USB_BYTES as u32,
                );
            }
            let mut guard = worker_handle.lock().unwrap();
            if let Some(address) = guard.take() {
                unsafe {
                    (worker_api.close)(address as Handle);
                }
            }
        });
        Ok(Self {
            rx,
            api,
            handle,
            worker: Some(worker),
            expected: 0,
            stopping,
            gain_steps,
            applied_gain_db,
            center_hz: actual_center,
        })
    }

    pub fn set_gain(&mut self, requested_db: f32) -> Result<f32, String> {
        let gain = closest_gain(&self.gain_steps, requested_db);
        let guard = self.handle.lock().unwrap();
        let address = guard.ok_or("SDR USB déconnecté")?;
        // SAFETY : close est exclu par le mutex ; commande RX du tuner seulement.
        let code = unsafe { (self.api.gain)(address as Handle, (gain * 10.0).round() as c_int) };
        if code != 0 {
            return Err(format!("Gain RF refusé ({code})"));
        }
        self.applied_gain_db = unsafe { (self.api.read_gain)(address as Handle) } as f32 / 10.0;
        Ok(self.applied_gain_db)
    }
}

fn closest_gain(steps: &[f32], requested: f32) -> f32 {
    steps
        .iter()
        .copied()
        .min_by(|a, b| (a - requested).abs().total_cmp(&(b - requested).abs()))
        .unwrap_or(0.0)
}

impl SdrSource for UsbSource {
    fn sample_rate(&self) -> u32 {
        IQ_RATE
    }
    fn read(&mut self, output: &mut [Complex32]) -> Result<(), String> {
        if output.len() != BLOCK_SIZE {
            return Err("Taille du bloc USB invalide".into());
        }
        let (sequence, bytes) = self
            .rx
            .recv_timeout(Duration::from_secs(1))
            .map_err(|_| "SDR déconnecté ou flux USB interrompu".to_owned())?;
        if sequence != self.expected {
            return Err("Surcharge USB : réception arrêtée. Reconnectez le SDR.".into());
        }
        self.expected = sequence + 1;
        for (sample, pair) in output.iter_mut().zip(bytes.as_chunks::<2>().0) {
            *sample = Complex32::new(
                (pair[0] as f32 - 127.5) / 127.5,
                (pair[1] as f32 - 127.5) / 127.5,
            );
        }
        Ok(())
    }
}

impl Drop for UsbSource {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            // Répéter cancel couvre l'arrêt demandé juste AVANT l'entrée read_async.
            while !worker.is_finished() {
                let guard = self.handle.lock().unwrap();
                if let Some(address) = *guard {
                    unsafe {
                        (self.api.cancel)(address as Handle);
                    }
                }
                drop(guard);
                thread::sleep(Duration::from_millis(10));
            }
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_gain_selects_a_supported_hardware_step() {
        let steps = [0.0, 20.7, 25.4, 28.0, 49.6];
        assert_eq!(closest_gain(&steps, 28.0), 28.0);
        assert_eq!(closest_gain(&steps, 27.0), 28.0);
        assert_eq!(closest_gain(&steps, -5.0), 0.0);
        assert_eq!(closest_gain(&steps, 60.0), 49.6);
    }

    #[test]
    fn refresh_keeps_busy_device_label_but_never_invents_a_device() {
        let active = UsbDevice {
            index: 0,
            name: "Blog V4".into(),
            serial: "00000001".into(),
        };
        let mut busy = vec![UsbDevice {
            index: 0,
            name: "RTL2832U".into(),
            serial: String::new(),
        }];
        UsbDevice::preserve_active_label(&mut busy, &active);
        assert_eq!(busy, vec![active.clone()]);
        let mut unplugged = vec![];
        UsbDevice::preserve_active_label(&mut unplugged, &active);
        assert!(unplugged.is_empty());
        let mut replacement = vec![UsbDevice {
            index: 0,
            name: "Autre RTL-SDR".into(),
            serial: "different".into(),
        }];
        UsbDevice::preserve_active_label(&mut replacement, &active);
        assert_ne!(replacement[0], active);
    }

    #[test]
    fn usb_callback_counts_dropped_blocks_without_blocking() {
        let (tx, rx) = bounded(1);
        let dropped = Arc::new(AtomicU64::new(0));
        let mut context = CallbackState {
            tx,
            sequence: 0,
            dropped: dropped.clone(),
            stopping: Arc::new(AtomicBool::new(false)),
        };
        let mut data = vec![128_u8; USB_BYTES];
        for _ in 0..2 {
            unsafe {
                receive_usb(
                    data.as_mut_ptr(),
                    USB_BYTES as u32,
                    (&mut context as *mut CallbackState).cast(),
                );
            }
        }
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        assert_eq!(rx.recv().unwrap().0, 0);
        assert_eq!(context.sequence, 2);
        context.stopping.store(true, Ordering::Relaxed);
        unsafe {
            receive_usb(
                data.as_mut_ptr(),
                USB_BYTES as u32,
                (&mut context as *mut CallbackState).cast(),
            );
        }
        assert_eq!(context.sequence, 2);
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
    }
}
