//! Sources USB physiques uniquement ; signaux synthétiques réservés aux tests.
use num_complex::Complex32;
#[cfg(all(feature = "desktop", not(target_arch = "wasm32")))]
pub mod usb;
pub trait SdrSource: Send {
    fn sample_rate(&self) -> u32;
    fn read(&mut self, output: &mut [Complex32]) -> Result<(), String>;
}
#[cfg(test)]
mod test_signal;
#[cfg(test)]
pub use test_signal::SimulatedSource;
