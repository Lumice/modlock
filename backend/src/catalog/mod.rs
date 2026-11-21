//! Catalog authority is independent of the native library and launch writer.
pub mod contract;
pub mod owner;
pub mod transport;
pub use owner::CatalogOwner;

pub mod provider;
pub mod preferences;
pub mod store;
pub mod query;
pub mod sync;
pub mod executor;
mod heroes;

pub mod details;
pub mod activity;
pub mod collections;

mod links;

mod rich_description;
