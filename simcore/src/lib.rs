//! Standalone port of the ONI cell simulation used to settle freshly
//! generated worlds. Runs natively and on wasm32 with no platform
//! dependencies.
pub mod disease;
pub mod disease_table;
pub mod driver;
pub mod element_yaml;
pub mod events;
pub mod fp;
pub mod gas;
pub mod germs;
pub mod io;
pub mod liquid;
pub mod messages;
pub mod phases;
pub mod postprocess;
pub mod radiation;
pub mod sim;
pub mod simutil;
pub mod snapshot;
pub mod state;
pub mod tables;
pub mod temperature;
pub use sim::Sim;
