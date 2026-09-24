mod manifest;
#[cfg(test)]
mod tests;

pub(crate) use manifest::{prepare_selection, PackageError, PreparedSelection};
