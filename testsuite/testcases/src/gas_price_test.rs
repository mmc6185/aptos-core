// Copyright (c) The Aptos Core Contributors
// SPDX-License-Identifier: Apache-2.0

use crate::generate_traffic;
use forge::{NetworkContext, NetworkTest, Result, Test};
use tokio::time::Duration;

pub struct NonZeroGasPrice;

impl Test for NonZeroGasPrice {
    fn name(&self) -> &'static str {
        "gas-price-unit-1"
    }
}

impl NetworkTest for NonZeroGasPrice {
    fn run<'t>(&self, ctx: &mut NetworkContext<'t>) -> Result<()> {
        let duration = Duration::from_secs(240);
        let all_validators = ctx
            .swarm()
            .validators()
            .map(|v| v.peer_id())
            .collect::<Vec<_>>();

        // Generate some traffic
        let txn_stat = generate_traffic(ctx, &all_validators, duration, 1, None)?;
        ctx.report
            .report_txn_stats(self.name().to_string(), txn_stat, duration);

        Ok(())
    }
}
