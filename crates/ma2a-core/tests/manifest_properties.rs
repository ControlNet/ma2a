//! Property tests for bounded Space object decoding.

use ma2a_core::{SignedSpaceGenesisV1, SpaceChain};
use proptest::prelude::*;

proptest! {
    #[test]
    fn arbitrary_bytes_never_form_an_unverified_space(bytes in prop::collection::vec(any::<u8>(), 0..4096)) {
        let genesis = SignedSpaceGenesisV1::from_canonical_bytes(&bytes);
        if let Ok(genesis) = genesis {
            prop_assert_eq!(genesis.canonical_bytes(), bytes.as_slice());
        }
        let imported = SpaceChain::import_public(&bytes);
        if let Ok(chain) = imported {
            let exported = chain.export_public();
            prop_assert_eq!(exported.as_deref(), Ok(bytes.as_slice()));
        }
    }
}
