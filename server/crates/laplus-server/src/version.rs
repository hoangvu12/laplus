//! The release identity compiled into every Rust-facing surface.

/// The effective product version.
///
/// Ordinary builds use the Cargo workspace version. Release builds may supply
/// an exact prerelease identity at compile time; it cannot be changed by the
/// environment of an installed process.
pub const PRODUCT_VERSION: &str = match option_env!("LAPLUS_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

