// Copyright (c) The Aptos Core Contributors
// SPDX-License-Identifier: Apache-2.0

use anyhow::Result;
use aptos_state_view::StateViewId;
use aptos_types::{
    account_address::AccountAddress,
    account_config::{AccountResource, AccountSequenceInfo},
    account_state::AccountState,
    on_chain_config::{AptosVersion, OnChainConfigPayload, VMConfig, VMPublishingOption},
    transaction::{SignedTransaction, VMValidatorResult},
};
use aptos_vm::AptosVM;
use executor::components::apply_chunk_output::IntoLedgerView;
use fail::fail_point;
use std::{convert::TryFrom, sync::Arc};
use storage_interface::{state_view::VerifiedStateView, DbReader};

#[cfg(test)]
#[path = "unit_tests/vm_validator_test.rs"]
mod vm_validator_test;

pub trait TransactionValidation: Send + Sync + Clone {
    type ValidationInstance: aptos_vm::VMValidator;

    /// Validate a txn from client
    fn validate_transaction(&self, _txn: SignedTransaction) -> Result<VMValidatorResult>;

    /// Restart the transaction validation instance
    fn restart(&mut self, config: OnChainConfigPayload) -> Result<()>;

    /// Notify about new commit
    fn notify_commit(&mut self);
}

fn latest_state_view(db_reader: &Arc<dyn DbReader>) -> VerifiedStateView {
    let ledger_view = db_reader
        .get_latest_tree_state()
        .expect("Should not fail.")
        .into_ledger_view(db_reader)
        .expect("Should not fail.");

    ledger_view.state_view(
        &ledger_view,
        StateViewId::TransactionValidation {
            base_version: ledger_view.version().expect("Must be bootstrapped."),
        },
        db_reader.clone(),
    )
}

pub struct VMValidator {
    db_reader: Arc<dyn DbReader>,
    cached_state_view: VerifiedStateView,
    vm: AptosVM,
}

impl Clone for VMValidator {
    fn clone(&self) -> Self {
        Self::new(self.db_reader.clone())
    }
}

impl VMValidator {
    pub fn new(db_reader: Arc<dyn DbReader>) -> Self {
        let cached_state_view = latest_state_view(&db_reader);

        let vm = AptosVM::new_for_validation(&cached_state_view);
        VMValidator {
            db_reader,
            cached_state_view,
            vm,
        }
    }
}

impl TransactionValidation for VMValidator {
    type ValidationInstance = AptosVM;

    fn validate_transaction(&self, txn: SignedTransaction) -> Result<VMValidatorResult> {
        fail_point!("vm_validator::validate_transaction", |_| {
            Err(anyhow::anyhow!(
                "Injected error in vm_validator::validate_transaction"
            ))
        });
        use aptos_vm::VMValidator;

        Ok(self.vm.validate_transaction(txn, &self.cached_state_view))
    }

    fn restart(&mut self, config: OnChainConfigPayload) -> Result<()> {
        self.notify_commit();
        let vm_config = config.get::<VMConfig>()?;
        let version = config.get::<AptosVersion>()?;
        let publishing_option = config.get::<VMPublishingOption>()?;

        self.vm = AptosVM::init_with_config(version, vm_config, publishing_option);
        Ok(())
    }

    fn notify_commit(&mut self) {
        self.cached_state_view = latest_state_view(&self.db_reader);
    }
}

/// returns account's sequence number from storage
pub fn get_account_sequence_number(
    storage: &dyn DbReader,
    address: AccountAddress,
) -> Result<AccountSequenceInfo> {
    fail_point!("vm_validator::get_account_sequence_number", |_| {
        Err(anyhow::anyhow!(
            "Injected error in get_account_sequence_number"
        ))
    });
    match storage.get_latest_account_state(address)? {
        Some(blob) => {
            if let Ok(Some(crsn)) = AccountState::try_from(&blob)?.get_crsn_resource() {
                return Ok(AccountSequenceInfo::CRSN {
                    min_nonce: crsn.min_nonce(),
                    size: crsn.size(),
                });
            }
            let seqno = AccountResource::try_from(&blob)?.sequence_number();
            Ok(AccountSequenceInfo::Sequential(seqno))
        }
        None => Ok(AccountSequenceInfo::Sequential(0)),
    }
}
