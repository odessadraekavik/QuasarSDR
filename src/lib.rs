//! I/Q -> FFT (mesure) et DDC multi-VFO (audio), deux branches parallèles.
//! `cargo check --lib --no-default-features --target wasm32-unknown-unknown`
//! vérifie le noyau portable. Le runtime navigateur n'est pas encore fourni.
pub mod driver;
pub mod dsp;
pub mod model;

#[cfg(all(feature = "desktop", not(target_arch = "wasm32")))]
pub mod audio;
#[cfg(all(feature = "desktop", not(target_arch = "wasm32")))]
pub mod engine;
#[cfg(all(feature = "desktop", not(target_arch = "wasm32")))]
pub mod ui;
