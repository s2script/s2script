//! Projection compatibility is independent of callback names and mutation rights.
use super::contract::Abi;

pub(crate) fn compatible(a: &Abi, b: &Abi) -> bool {
    a.fingerprint == b.fingerprint
        && a.receiver == b.receiver
        && a.parameters.len() == b.parameters.len()
        && a.parameters.iter().zip(&b.parameters).all(|(a, b)| {
            a.native == b.native
                && a.projection.id == b.projection.id
                && a.projection.version == b.projection.version
        })
        && a.returns.native == b.returns.native
        && a.returns.projection.id == b.returns.projection.id
        && a.returns.projection.version == b.returns.projection.version
}
