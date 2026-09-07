use crate::{
    audio::{self, AudioOutput},
    driver::{
        usb::{UsbDevice, UsbSource},
        SdrSource,
    },
    dsp::Processor,
    model::*,
};
use crossbeam_channel::{bounded, Receiver, Sender};
use crossbeam_queue::ArrayQueue;
use num_complex::Complex32;
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Default)]
pub struct Counters {
    pub iq_dropped: AtomicU64,
    pub usb_dropped: Arc<AtomicU64>,
    pub audio_dropped: AtomicU64,
    pub underruns: Arc<AtomicU64>,
    pub audio_errors: Arc<AtomicU64>,
}

struct IqFrame {
    sequence: u64,
    epoch: u64,
    settings: Arc<Settings>,
    samples: Vec<Complex32>,
}

pub struct Engine {
    pub snapshots: Arc<ArrayQueue<Snapshot>>,
    pub commands: Sender<Settings>,
    pub rf_commands: Sender<f32>,
    pub rf_updates: Receiver<(f32, Vec<f32>)>,
    pub errors: Receiver<String>,
    pub counters: Arc<Counters>,
    pub audio_status: String,
    _audio: Option<AudioOutput>,
    stop: Arc<AtomicBool>,
    workers: Vec<JoinHandle<()>>,
}

impl Engine {
    pub fn start(settings: Settings, device: UsbDevice) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let counters = Arc::new(Counters::default());
        let queue = Arc::new(ArrayQueue::new(AUDIO_RATE as usize / 10));
        let (audio, audio_status) = match audio::open(
            queue.clone(),
            counters.underruns.clone(),
            counters.audio_errors.clone(),
        ) {
            Ok(a) => {
                let label = a.description.clone();
                (Some(a), label)
            }
            Err(e) => (None, format!("Audio indisponible : {e}")),
        };
        let audio_enabled = audio.is_some();
        let (iq_tx, iq_rx) = bounded::<IqFrame>(4);
        let (commands, command_rx) = bounded::<Settings>(1);
        let (rf_commands, rf_command_rx) = bounded::<f32>(1);
        let (rf_update_tx, rf_updates) = bounded(4);
        let (error_tx, errors) = bounded::<String>(4);
        let snapshots = Arc::new(ArrayQueue::new(2));
        let live_epoch = Arc::new(AtomicU64::new(0));
        let acquire_epoch = live_epoch.clone();
        let acquire_stop = stop.clone();
        let acquire_counters = counters.clone();
        let acquire_audio = queue.clone();
        let acquisition = thread::spawn(move || {
            let mut current = Arc::new(settings);
            let open = |settings: &Settings| {
                UsbSource::open(
                    &device,
                    acquire_counters.usb_dropped.clone(),
                    settings.rf_gain_db,
                    settings.center_hz,
                )
            };
            let mut source = match open(&current) {
                Ok(source) => Some(source),
                Err(error) => {
                    let _ = error_tx.try_send(error);
                    return;
                }
            };
            let initial = source.as_ref().unwrap();
            let _ = rf_update_tx.try_send((initial.applied_gain_db, initial.gain_steps.clone()));
            let mut sequence = 0;
            let mut epoch = 0;
            while !acquire_stop.load(Ordering::Relaxed) {
                if let Ok(new) = command_rx.try_recv() {
                    let retune = new.center_hz != current.center_hz;
                    current = Arc::new(new);
                    epoch += 1;
                    acquire_epoch.store(epoch, Ordering::Release);
                    if retune {
                        // Arrêter les transferts, fermer, réaccorder puis repartir sur
                        // des buffers neufs : aucun ancien I/Q étiqueté avec le nouveau centre.
                        drop(source.take());
                        while acquire_audio.pop().is_some() {}
                        source = match open(&current) {
                            Ok(source) => Some(source),
                            Err(error) => {
                                let _ = error_tx.try_send(error);
                                break;
                            }
                        };
                        let opened = source.as_ref().unwrap();
                        let _ = rf_update_tx
                            .try_send((opened.applied_gain_db, opened.gain_steps.clone()));
                    }
                }
                let receiver = source.as_mut().unwrap();
                if let Ok(gain) = rf_command_rx.try_recv() {
                    match receiver.set_gain(gain) {
                        Ok(applied) => {
                            Arc::make_mut(&mut current).rf_gain_db = applied;
                            // Nouveau gain => nouvelle référence de bruit dans le DSP.
                            epoch += 1;
                            acquire_epoch.store(epoch, Ordering::Release);
                            let _ = rf_update_tx.try_send((applied, receiver.gain_steps.clone()));
                        }
                        Err(error) => {
                            let _ = error_tx.try_send(error);
                            break;
                        }
                    }
                }
                let mut iq = vec![Complex32::default(); BLOCK_SIZE];
                if let Err(error) = receiver.read(&mut iq) {
                    let _ = error_tx.try_send(error);
                    break;
                }
                let frame = IqFrame {
                    sequence,
                    epoch,
                    settings: current.clone(),
                    samples: iq,
                };
                if iq_tx.try_send(frame).is_err() {
                    acquire_counters.iq_dropped.fetch_add(1, Ordering::Relaxed);
                }
                sequence += 1;
            }
        });

        let dsp_stop = stop.clone();
        let dsp_counters = counters.clone();
        let dsp_snapshots = snapshots.clone();
        let dsp = thread::spawn(move || {
            let mut processor: Option<Processor> = None;
            let mut previous_epoch = u64::MAX;
            let mut expected = 0;
            let mut last_display = Instant::now();
            while !dsp_stop.load(Ordering::Relaxed) {
                let frame = match iq_rx.recv_timeout(Duration::from_millis(30)) {
                    Ok(frame) => frame,
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                    Err(_) => break,
                };
                if frame.epoch != live_epoch.load(Ordering::Acquire) {
                    continue;
                }
                if frame.sequence != expected || processor.is_none() {
                    processor = Some(Processor::new((*frame.settings).clone()));
                } else if let Some(processor) = &mut processor {
                    if previous_epoch != frame.epoch {
                        processor.configure((*frame.settings).clone());
                    }
                }
                expected = frame.sequence + 1;
                previous_epoch = frame.epoch;
                let start = Instant::now();
                let (samples, mut snapshot) = processor.as_mut().unwrap().process(&frame.samples);
                snapshot.dsp_ms = start.elapsed().as_secs_f32() * 1000.0;
                if frame.epoch != live_epoch.load(Ordering::Acquire) {
                    continue;
                }
                if audio_enabled {
                    for sample in samples {
                        if queue.force_push(sample).is_some() {
                            dsp_counters.audio_dropped.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
                if last_display.elapsed() >= Duration::from_millis(30) {
                    dsp_snapshots.force_push(snapshot);
                    last_display = Instant::now();
                }
            }
        });
        Self {
            snapshots,
            commands,
            rf_commands,
            rf_updates,
            errors,
            counters,
            audio_status,
            _audio: audio,
            stop,
            workers: vec![acquisition, dsp],
        }
    }
    pub fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
        self._audio = None;
    }
}
impl Drop for Engine {
    fn drop(&mut self) {
        self.shutdown();
    }
}
