//! Bounded, local-only WHDLoad slave inspection and evidence conversion.

pub mod convert;
pub mod reconcile;
pub mod slave;

pub use convert::{
    MatchedSlaveDatSource, exact_slave_match_observation, structural_slave_observation,
};
pub use reconcile::{
    WhdloadDatContext, WhdloadReconciliation, WhdloadSlaveIdentity, WhdloadSlaveMatch,
    reconcile_whdload_slaves,
};
pub use slave::{
    ParsedWHDLoadSlave, SlaveArtifact, SlaveError, SlaveHashes, inspect_whdload_slave_file,
    parse_whdload_slave,
};

#[cfg(test)]
mod tests;
