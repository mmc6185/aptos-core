// Copyright (c) The Aptos Core Contributors
// SPDX-License-Identifier: Apache-2.0

use crate::{
    account_address::{AccountAddress, HashAccountAddress},
    account_config::{AccountResource, BalanceResource, AptosAccountResource},
    account_state::AccountState,
    ledger_info::LedgerInfo,
    proof::{AccountStateProof, SparseMerkleRangeProof},
    transaction::Version,
};
use anyhow::{anyhow, ensure, Error, Result};
use aptos_crypto::{
    hash::{CryptoHash, CryptoHasher},
    HashValue,
};
use aptos_crypto_derive::CryptoHasher;
#[cfg(any(test, feature = "fuzzing"))]
use proptest::{arbitrary::Arbitrary, prelude::*};
#[cfg(any(test, feature = "fuzzing"))]
use proptest_derive::Arbitrary;
use serde::{Deserialize, Deserializer, Serialize};
use std::{convert::TryFrom, fmt};

#[derive(Clone, Eq, PartialEq, Serialize, CryptoHasher)]
pub struct AccountStateBlob {
    blob: Vec<u8>,
    #[serde(skip)]
    hash: HashValue,
}

impl<'de> Deserialize<'de> for AccountStateBlob {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename = "AccountStateBlob")]
        struct RawBlob {
            blob: Vec<u8>,
        }
        let blob = RawBlob::deserialize(deserializer)?;

        Ok(Self::new(blob.blob))
    }
}

impl AccountStateBlob {
    fn new(blob: Vec<u8>) -> Self {
        let mut hasher = AccountStateBlobHasher::default();
        hasher.update(&blob);
        let hash = hasher.finish();
        Self { blob, hash }
    }
}

impl fmt::Debug for AccountStateBlob {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let decoded = bcs::from_bytes(&self.blob)
            .map(|account_state: AccountState| format!("{:#?}", account_state))
            .unwrap_or_else(|_| String::from("[fail]"));

        write!(
            f,
            "AccountStateBlob {{ \n \
             Raw: 0x{} \n \
             Decoded: {} \n \
             }}",
            hex::encode(&self.blob),
            decoded,
        )
    }
}

impl AsRef<[u8]> for AccountStateBlob {
    fn as_ref(&self) -> &[u8] {
        &self.blob
    }
}

impl From<&AccountStateBlob> for Vec<u8> {
    fn from(account_state_blob: &AccountStateBlob) -> Vec<u8> {
        account_state_blob.blob.clone()
    }
}

impl From<AccountStateBlob> for Vec<u8> {
    fn from(account_state_blob: AccountStateBlob) -> Vec<u8> {
        Self::from(&account_state_blob)
    }
}

impl From<Vec<u8>> for AccountStateBlob {
    fn from(blob: Vec<u8>) -> AccountStateBlob {
        AccountStateBlob::new(blob)
    }
}

impl TryFrom<&AccountState> for AccountStateBlob {
    type Error = Error;

    fn try_from(account_state: &AccountState) -> Result<Self> {
        Ok(Self::new(bcs::to_bytes(account_state)?))
    }
}

impl TryFrom<&AccountStateBlob> for AccountState {
    type Error = Error;

    fn try_from(account_state_blob: &AccountStateBlob) -> Result<Self> {
        bcs::from_bytes(&account_state_blob.blob).map_err(Into::into)
    }
}

impl TryFrom<(&AccountResource, &AptosAccountResource, &BalanceResource)> for AccountStateBlob {
    type Error = Error;

    fn try_from(
        (account_resource, aptos_account_resource, balance_resource): (
            &AccountResource,
            &AptosAccountResource,
            &BalanceResource,
        ),
    ) -> Result<Self> {
        Self::try_from(&AccountState::try_from((
            account_resource,
            aptos_account_resource,
            balance_resource,
        ))?)
    }
}

impl TryFrom<&AccountStateBlob> for AptosAccountResource {
    type Error = Error;

    fn try_from(account_state_blob: &AccountStateBlob) -> Result<Self> {
        AccountState::try_from(account_state_blob)?
            .get_aptos_account_resource()?
            .ok_or_else(|| anyhow!("AptosAccountResource not found."))
    }
}

impl TryFrom<&AccountStateBlob> for AccountResource {
    type Error = Error;

    fn try_from(account_state_blob: &AccountStateBlob) -> Result<Self> {
        AccountState::try_from(account_state_blob)?
            .get_account_resource()?
            .ok_or_else(|| anyhow!("AccountResource not found."))
    }
}

impl CryptoHash for AccountStateBlob {
    type Hasher = AccountStateBlobHasher;

    fn hash(&self) -> HashValue {
        self.hash
    }
}

#[cfg(any(test, feature = "fuzzing"))]
prop_compose! {
    fn account_state_blob_strategy()(account_resource in any::<AccountResource>(), aptos_account_resource in any::<AptosAccountResource>(), balance_resource in any::<BalanceResource>()) -> AccountStateBlob {
        AccountStateBlob::try_from((&account_resource, &aptos_account_resource, &balance_resource)).unwrap()
    }
}

#[cfg(any(test, feature = "fuzzing"))]
impl Arbitrary for AccountStateBlob {
    type Parameters = ();
    fn arbitrary_with(_args: Self::Parameters) -> Self::Strategy {
        account_state_blob_strategy().boxed()
    }

    type Strategy = BoxedStrategy<Self>;
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "fuzzing"), derive(Arbitrary))]
pub struct AccountStateWithProof {
    /// The transaction version at which this account state is seen.
    pub version: Version,
    /// Blob value representing the account state. If this field is not set, it
    /// means the account does not exist.
    pub blob: Option<AccountStateBlob>,
    /// The proof the client can use to authenticate the value.
    pub proof: AccountStateProof,
}

impl AccountStateWithProof {
    /// Constructor.
    pub fn new(version: Version, blob: Option<AccountStateBlob>, proof: AccountStateProof) -> Self {
        Self {
            version,
            blob,
            proof,
        }
    }

    /// Verifies the the account state blob with the proof, both carried by `self`.
    ///
    /// Two things are ensured if no error is raised:
    ///   1. This account state exists in the ledger represented by `ledger_info`.
    ///   2. It belongs to account of `address` and is seen at the time the transaction at version
    /// `state_version` is just committed. To make sure this is the latest state, pass in
    /// `ledger_info.version()` as `state_version`.
    pub fn verify(
        &self,
        ledger_info: &LedgerInfo,
        version: Version,
        address: AccountAddress,
    ) -> Result<()> {
        ensure!(
            self.version == version,
            "State version ({}) is not expected ({}).",
            self.version,
            version,
        );

        self.proof
            .verify(ledger_info, version, address.hash(), self.blob.as_ref())
    }
}

/// TODO(joshlind): add a proof implementation (e.g., verify()) and unit tests
/// for these once we start supporting them.
///
/// A single chunk of all account states at a specific version.
/// Note: this is similar to `StateSnapshotChunk` but all data is included
/// in the struct itself and not behind pointers/handles to file locations.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AccountStatesChunkWithProof {
    pub first_index: u64,
    // The first account index in chunk
    pub last_index: u64,
    // The last account index in chunk
    pub first_key: HashValue,
    // The first account key in chunk
    pub last_key: HashValue,
    // The last account key in chunk
    pub account_blobs: Vec<(HashValue, AccountStateBlob)>,
    // The account blobs in the chunk
    pub proof: SparseMerkleRangeProof, // The proof to ensure the chunk is in the account states
}

#[cfg(test)]
mod tests {
    use super::{AccountStateWithProof, *};
    use bcs::test_helpers::assert_canonical_encode_decode;
    use proptest::collection::vec;

    fn hash_blob(blob: &[u8]) -> HashValue {
        let mut hasher = AccountStateBlobHasher::default();
        hasher.update(blob);
        hasher.finish()
    }

    proptest! {
        #[test]
        fn account_state_blob_hash(blob in vec(any::<u8>(), 1..100)) {
            prop_assert_eq!(hash_blob(&blob), AccountStateBlob::from(blob).hash());
        }

        #[test]
        fn account_state_blob_bcs_roundtrip(account_state_blob in any::<AccountStateBlob>()) {
            assert_canonical_encode_decode(account_state_blob);
        }

        #[test]
        fn account_state_with_proof_bcs_roundtrip(account_state_with_proof in any::<AccountStateWithProof>()) {
            assert_canonical_encode_decode(account_state_with_proof);
        }
    }

    #[test]
    fn test_debug_does_not_panic() {
        format!("{:#?}", AccountStateBlob::from(vec![1u8, 2u8, 3u8]));
    }
}
