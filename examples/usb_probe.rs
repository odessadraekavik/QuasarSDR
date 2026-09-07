//! Diagnostic USB RX : `cargo run --release --example usb_probe -- --receive`.
use quasar_sdr::{
    driver::{
        usb::{RtlApi, UsbSource},
        SdrSource,
    },
    engine::Engine,
    model::{Settings, BLOCK_SIZE, CENTER_HZ},
};
use std::sync::{atomic::AtomicU64, Arc};

fn main() -> Result<(), String> {
    let api = RtlApi::load()?;
    let devices = api.devices();
    println!("{} SDR USB compatible(s)", devices.len());
    for device in &devices {
        println!("{}", device.label());
    }
    if std::env::args().any(|a| a == "--receive") {
        let device = devices.first().ok_or("Aucun SDR connecté")?;
        let mut source = UsbSource::open(device, Arc::new(AtomicU64::new(0)), 28.0, 156_800_000.0)?;
        println!("Gain RF initial : {:+.1} dB", source.applied_gain_db);
        assert!((source.applied_gain_db - 28.0).abs() < 0.11);
        println!("Gain RF modifié : {:+.1} dB", source.set_gain(20.7)?);
        println!("Gain RF restauré : {:+.1} dB", source.set_gain(28.0)?);
        let mut iq = vec![num_complex::Complex32::default(); BLOCK_SIZE];
        let mut energy = 0.0_f64;
        for _ in 0..100 {
            source.read(&mut iq)?;
            energy += iq.iter().map(|s| s.norm_sqr() as f64).sum::<f64>();
        }
        println!(
            "RX USB : {} échantillons à {} Hz ; puissance moyenne {:.6}",
            100 * BLOCK_SIZE,
            source.sample_rate(),
            energy / (100 * BLOCK_SIZE) as f64
        );
        // L'énumération doit conserver l'identité même pendant une réception.
        let mut refresh = api.devices();
        quasar_sdr::driver::usb::UsbDevice::preserve_active_label(&mut refresh, device);
        if !refresh.contains(device) {
            return Err("Identité USB instable pendant la réception".into());
        }
        drop(source);
        println!("Arrêt RX et libération USB réussis");
    }
    if std::env::args().any(|a| a == "--engine") {
        let device = devices.first().ok_or("Aucun SDR connecté")?;
        let mut settings = Settings {
            center_hz: 156_800_000.0,
            ..Default::default()
        };
        if std::env::args().any(|a| a == "--voice") {
            settings.voice_denoise = true;
            // Diagnostic CPU : forcer le SQL ouvert sur le bruit réel reçu.
            settings.adaptive_squelch = false;
            settings.sql_dbfs = -140.0;
            println!("Diagnostic Voix IA actif, SQL forcé ouvert");
        }
        for vfo in &mut settings.vfos {
            vfo.frequency_hz += settings.center_hz - CENTER_HZ;
        }
        let engine = Engine::start(settings.clone(), device.clone());
        for center in [156_800_000.0, 157_400_000.0] {
            settings.center_hz = center;
            engine
                .commands
                .send(settings.clone())
                .map_err(|e| e.to_string())?;
            let start = std::time::Instant::now();
            let mut received = 0;
            let mut max_ms = 0.0_f32;
            while start.elapsed() < std::time::Duration::from_secs(2) {
                if let Ok(error) = engine.errors.try_recv() {
                    return Err(error);
                }
                while let Some(snapshot) = engine.snapshots.pop() {
                    if snapshot.center_hz == center {
                        received += 1;
                        max_ms = max_ms.max(snapshot.dsp_ms);
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            if received == 0 {
                return Err(format!("Aucun spectre au centre {center}"));
            }
            println!(
                "Moteur RX {:.3} MHz : {} spectres, DSP max {:.2} ms / bloc 10.67 ms",
                center / 1e6,
                received,
                max_ms
            );
        }
        println!(
            "Pertes I/Q DSP : {} ; USB : {}",
            engine
                .counters
                .iq_dropped
                .load(std::sync::atomic::Ordering::Relaxed),
            engine
                .counters
                .usb_dropped
                .load(std::sync::atomic::Ordering::Relaxed)
        );
        drop(engine);
        println!("Moteur RX arrêté après réaccord");
    }
    Ok(())
}
