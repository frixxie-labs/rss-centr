pub mod api;
#[cfg(any(feature = "web", feature = "server"))]
pub mod app;
#[cfg(any(feature = "web", feature = "server"))]
mod components;
pub mod logic;
pub mod models;
#[cfg(any(feature = "web", feature = "server"))]
mod pages;

#[cfg(all(feature = "web", target_arch = "wasm32"))]
mod browser;
#[cfg(feature = "server")]
pub mod server;

#[cfg(test)]
mod tests;
