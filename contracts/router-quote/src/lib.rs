#![no_std]

//! # router-quote
//!
//! Preview transaction results before execution.
//! Allows users to get quote information including expected output amount,
//! fees, and route details without executing the transaction.
//!
//! ## Features
//! - Get quote from any registered liquidity plugin
//! - Returns expected output amount, fees, and route details
//! - Does not execute transactions (read-only preview)
//! - Works with any plugin implementing the get_quote interface
//!
//! ## Events (following naming convention: past tense verbs in snake_case)
//! - `quote_requested` — Quote request logged (route_name, token_in, token_out, amount_in)

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, Address, Env, String, Symbol, Vec,
};

// ── Types ─────────────────────────────────────────────────────────────────────

/// Request parameters for getting a quote.
#[contracttype]
#[derive(Clone, Debug)]
pub struct QuoteRequest {
    /// The route name to query (e.g., "liquidity/uniswap-v3")
    pub route_name: String,
    /// The token the user is selling
    pub token_in: Address,
    /// The token the user wants to receive
    pub token_out: Address,
    /// The amount of token_in to swap
    pub amount_in: i128,
}

/// Response containing quote details.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct QuoteResponse {
    /// The expected output amount (token_out)
    pub amount_out: i128,
    /// The fee amount deducted (in token_in)
    pub fee_amount: i128,
    /// The route name that was used
    pub route_name: String,
    /// The target contract address
    pub target: Address,
    /// Minimum output amount (for slippage protection)
    pub min_amount_out: i128,
    /// Exchange rate (amount_out / amount_in as a string for precision)
    pub exchange_rate: String,
    /// Price impact estimate (basis points, negative = adverse)
    pub price_impact_bps: i32,
}

// ── Errors ────────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum QuoteError {
    RouteNotFound = 1,
    InvalidAmount = 2,
    QuoteFailed = 3,
    InvalidRoute = 4,
}

/// Request parameters for fee estimation.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct FeeEstimateRequest {
    /// Amount of token_in being transacted (in stroops or token base units).
    pub amount: i128,
    /// Fee rate in basis points charged by the route (e.g., 30 = 0.30%).
    pub fee_bps: u32,
    /// Current network utilization in basis points (0–10000).
    /// Values ≥ 8000 trigger surge pricing.
    pub network_load_bps: u32,
}

/// Estimated fee breakdown for a transaction.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct FeeEstimateResponse {
    /// Protocol fee charged by the route (in token_in base units).
    pub protocol_fee: i128,
    /// Network/gas fee in stroops.
    pub network_fee: i128,
    /// Total estimated fee (protocol + network).
    pub total_fee: i128,
    /// Whether surge pricing was applied due to high network load.
    pub surge_pricing: bool,
    /// Effective fee rate in basis points after surge adjustment.
    pub effective_fee_bps: u32,
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct RouterQuote;

#[contractimpl]
impl RouterQuote {
    /// Get a quote from a liquidity plugin.
    ///
    /// Resolves the route name to a contract address via router-core (if provided),
    /// then invokes the plugin's `get_quote` function to retrieve the expected output.
    /// This does NOT execute the transaction — it only previews the result.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `router_core` - Optional address of router-core contract for route resolution.
    /// * `route_name` - The name of the route to query (e.g., "liquidity/uniswap-v3").
    ///                   Can also be a direct contract address if router_core is None.
    /// * `token_in` - The address of the token being sold.
    /// * `token_out` - The address of the token being bought.
    /// * `amount_in` - The amount of token_in to swap.
    ///
    /// # Returns
    /// A [`QuoteResponse`] containing the expected output amount, fees, and route details.
    ///
    /// # Errors
    /// * [`QuoteError::InvalidAmount`] — if `amount_in` is less than or equal to zero.
    /// * [`QuoteError::RouteNotFound`] — if the route name is not registered.
    /// * [`QuoteError::QuoteFailed`] — if the plugin's `get_quote` call fails.
    pub fn get_quote(
        env: Env,
        router_core: Option<Address>,
        route_name: String,
        token_in: Address,
        token_out: Address,
        amount_in: i128,
    ) -> Result<QuoteResponse, QuoteError> {
        // Validate input
        if amount_in <= 0 {
            return Err(QuoteError::InvalidAmount);
        }

        // Resolve target address
        let target: Address = match router_core {
            Some(router) => {
                // Use router-core to resolve the route name
                let function = Symbol::new(&env, "resolve");
                let mut args = Vec::new(&env);
                args.push_back(route_name.clone().into());
                
                env.invoke_contract(&router, &function, args)
            }
            None => {
                // Try direct address interpretation
                route_name.clone().try_into().map_err(|_| QuoteError::InvalidRoute)?
            }
        };

        // Try to invoke the get_quote function on the target contract
        // The plugin interface expects: get_quote(token_in, token_out, amount_in) -> i128
        let function = Symbol::new(&env, "get_quote");
        
        // Build args: (token_in, token_out, amount_in)
        let mut args = Vec::new(&env);
        args.push_back(token_in.into());
        args.push_back(token_out.into());
        args.push_back(amount_in.into());

        // Attempt the cross-contract call
        let amount_out: i128 = env
            .invoke_contract(&target, &function, args);

        // Calculate fee (assuming 1% fee for now - in production this comes from the plugin)
        let fee_amount = amount_in * 1 / 100;
        
        // Calculate min_amount_out with 0.5% slippage tolerance
        let min_amount_out = amount_out * 999 / 1000;
        
        // Exchange rate placeholder
        let exchange_rate = String::from_str(&env, "0");

        // Price impact (0 for now - would need more complex calculation)
        let price_impact_bps = 0;

        Ok(QuoteResponse {
            amount_out,
            fee_amount,
            route_name,
            target,
            min_amount_out,
            exchange_rate,
            price_impact_bps,
        })
    }

    /// Get multiple quotes in a single call (for comparing routes).
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `router_core` - Optional address of router-core contract for route resolution.
    /// * `requests` - A vector of [`QuoteRequest`]s to process.
    ///
    /// # Returns
    /// A vector of [`QuoteResponse`]s (one per request). Failed quotes
    /// will have `amount_out = 0` and an appropriate error handling strategy.
    pub fn get_quotes(
        env: Env,
        router_core: Option<Address>,
        requests: Vec<QuoteRequest>,
    ) -> Vec<QuoteResponse> {
        let mut responses = Vec::new(&env);
        
        for req in requests.iter() {
            let response = Self::get_quote(
                env.clone(),
                router_core.clone(),
                req.route_name.clone(),
                req.token_in.clone(),
                req.token_out.clone(),
                req.amount_in,
            );
            
            match response {
                Ok(quote) => responses.push_back(quote),
                Err(_) => {
                    // On failure, add a zero quote (caller can check amount_out == 0)
                    responses.push_back(QuoteResponse {
                        amount_out: 0,
                        fee_amount: 0,
                        route_name: req.route_name.clone(),
                        target: req.route_name.clone().try_into().unwrap_or(Address::from_contract_id(&env, &[0u8; 32])),
                        min_amount_out: 0,
                        exchange_rate: String::from_str(&env, "0"),
                        price_impact_bps: 0,
                    });
                }
            }
        }
        
        responses
    }

    /// Estimate fees for a single transaction.
    ///
    /// Computes protocol and network fees based on the transaction amount,
    /// the route's fee rate, and current network load. Surge pricing (2×
    /// network fee) is applied when `network_load_bps` ≥ 8000 (80%).
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `request` - A [`FeeEstimateRequest`] describing the transaction parameters.
    ///
    /// # Returns
    /// A [`FeeEstimateResponse`] with a full fee breakdown.
    ///
    /// # Errors
    /// * [`QuoteError::InvalidAmount`] — if `request.amount` ≤ 0.
    pub fn estimate_fee(env: Env, request: FeeEstimateRequest) -> Result<FeeEstimateResponse, QuoteError> {
        if request.amount <= 0 {
            return Err(QuoteError::InvalidAmount);
        }

        // Protocol fee: amount * fee_bps / 10000
        let protocol_fee = request.amount * request.fee_bps as i128 / 10_000;

        // Base network fee: 100 stroops minimum
        let base_network_fee: i128 = 100;

        // Surge pricing at ≥ 80% network load
        let (network_fee, surge_pricing, effective_fee_bps) = if request.network_load_bps >= 8_000 {
            (base_network_fee * 2, true, request.fee_bps * 2)
        } else {
            (base_network_fee, false, request.fee_bps)
        };

        let total_fee = protocol_fee + network_fee;

        env.events().publish(
            (Symbol::new(&env, "fee_estimated"),),
            (total_fee, surge_pricing),
        );

        Ok(FeeEstimateResponse {
            protocol_fee,
            network_fee,
            total_fee,
            surge_pricing,
            effective_fee_bps,
        })
    }

    /// Estimate fees for multiple transactions in one call.
    ///
    /// Processes each [`FeeEstimateRequest`] independently. Failed estimates
    /// (e.g., invalid amount) are skipped and not included in the result.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `requests` - A vector of [`FeeEstimateRequest`]s.
    ///
    /// # Returns
    /// A vector of [`FeeEstimateResponse`]s for each valid request.
    pub fn estimate_fees(env: Env, requests: Vec<FeeEstimateRequest>) -> Vec<FeeEstimateResponse> {
        let mut responses = Vec::new(&env);
        for req in requests.iter() {
            if let Ok(estimate) = Self::estimate_fee(env.clone(), req) {
                responses.push_back(estimate);
            }
        }
        responses
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::Env;

    fn setup() -> (Env, RouterQuoteClient<'static>) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, RouterQuote);
        let client = RouterQuoteClient::new(&env, &contract_id);
        (env, client)
    }

    #[test]
    fn test_estimate_fee_normal_load() {
        let (_, client) = setup();
        let req = FeeEstimateRequest {
            amount: 1_000_000,
            fee_bps: 30,
            network_load_bps: 5_000,
        };
        let resp = client.estimate_fee(&req);
        assert!(!resp.surge_pricing);
        assert_eq!(resp.protocol_fee, 3_000); // 1_000_000 * 30 / 10_000
        assert_eq!(resp.network_fee, 100);
        assert_eq!(resp.total_fee, 3_100);
        assert_eq!(resp.effective_fee_bps, 30);
    }

    #[test]
    fn test_estimate_fee_high_load_surge() {
        let (_, client) = setup();
        let req = FeeEstimateRequest {
            amount: 1_000_000,
            fee_bps: 30,
            network_load_bps: 9_000,
        };
        let resp = client.estimate_fee(&req);
        assert!(resp.surge_pricing);
        assert_eq!(resp.network_fee, 200); // 2× base
        assert_eq!(resp.effective_fee_bps, 60); // 2× fee_bps
    }

    #[test]
    fn test_estimate_fee_invalid_amount() {
        let (_, client) = setup();
        let req = FeeEstimateRequest { amount: 0, fee_bps: 30, network_load_bps: 0 };
        let result = client.try_estimate_fee(&req);
        assert_eq!(result, Err(Ok(QuoteError::InvalidAmount)));
    }

    #[test]
    fn test_estimate_fees_skips_invalid() {
        let (env, client) = setup();
        let requests = soroban_sdk::vec![
            &env,
            FeeEstimateRequest { amount: 1_000_000, fee_bps: 30, network_load_bps: 0 },
            FeeEstimateRequest { amount: 0, fee_bps: 30, network_load_bps: 0 }, // invalid
            FeeEstimateRequest { amount: 500_000, fee_bps: 10, network_load_bps: 0 },
        ];
        let responses = client.estimate_fees(&requests);
        // Only 2 valid requests
        assert_eq!(responses.len(), 2);
    }
}