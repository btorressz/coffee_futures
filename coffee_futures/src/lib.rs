use anchor_lang::prelude::*;
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::token::{self, Mint, MintTo, Token, TokenAccount, Transfer};
use solana_program::rent::Rent;

declare_id!("programidhere");

// ------------------------- Config constants -------------------------
pub const PROGRAM_VERSION: u8 = 1;
pub const MAX_ASSETS: usize = 4;
pub const EMPTY_MERKLE_ROOT: [u8; 32] = [0u8; 32];
pub const MAX_PROOF_HASHES: usize = 16;
pub const MAX_PROOF_BYTES: usize = MAX_PROOF_HASHES * 32;
pub const SEED_PREFIX: &[u8] = b"v1"; // PDA seed versioning prefix
pub const MIN_TWAP_WINDOW: u64 = 1; // seconds minimal twap window

// Settlement price mode
#[repr(u8)]
pub enum PriceMode {
    LastPrice = 0,
    TWAP = 1,
}

// ------------------------- Program -------------------------
#[program]
pub mod coffee_futures {
    use super::*;

    // Initialize the CFT mint and its PDA authority account.
    pub fn init_cft_mint(ctx: Context<InitCftMint>, decimals: u8) -> Result<()> {
        version_guard_program()?;
        // persist the PDA bump
        ctx.accounts.cft_mint_auth.bump = ctx.bumps.cft_mint_auth;

        // use Rent sysvar to check rent-exemptness
        let rent = &ctx.accounts.rent;
        let cft_info = ctx.accounts.cft_mint.to_account_info();
        let auth_info = ctx.accounts.cft_mint_auth.to_account_info();
        require!(
            rent.is_exempt(cft_info.lamports(), cft_info.data_len()),
            CoffeeError::AccountNotRentExempt
        );
        require!(
            rent.is_exempt(auth_info.lamports(), auth_info.data_len()),
            CoffeeError::AccountNotRentExempt
        );

        // optionally verify decimals
        require!(decimals == ctx.accounts.cft_mint.decimals, CoffeeError::MintDecimalsMismatch);

        emit!(CftMintInitialized {
            cft_mint: ctx.accounts.cft_mint.key(),
            authority: ctx.accounts.payer.key(),
            decimals,
        });
        Ok(())
    }

    // Create a per-harvest market (admin)
    #[allow(clippy::too_many_arguments)]
    pub fn create_market(
        ctx: Context<CreateMarket>,
        settlement_ts: i64,
        contract_size_kg: u64,
        initial_margin_bps: u16,
        maintenance_margin_bps: u16,
        fee_bps: u16,
        farmer_fee_bps: u16,
        buyer_fee_bps: u16,
        max_notional_per_deal: u64,
        max_qty_per_deal: u64,
        max_oracle_age_sec: u64,
        twap_window_sec: u64,
        insurance_bps: u16,
        min_transfer_amount: u64,
    ) -> Result<()> {
        version_guard_program()?;

        // avoid borrow conflicts: capture the key before mut borrow
        let market_key = ctx.accounts.market.key();

        let market = &mut ctx.accounts.market;
        require!(initial_margin_bps >= maintenance_margin_bps, CoffeeError::BadMarginParams);
        require!(contract_size_kg > 0, CoffeeError::ZeroQty);
        require!(twap_window_sec >= MIN_TWAP_WINDOW, CoffeeError::InvalidTwapWindow);

        market.version = PROGRAM_VERSION;
        market.authority = ctx.accounts.authority.key();
        market.verifier = ctx.accounts.verifier.key();
        market.oracle_publisher = ctx.accounts.oracle_publisher.key();
        market.pending_oracle = Pubkey::default();
        market.pending_oracle_effective_ts = 0;
        market.cft_mint = ctx.accounts.cft_mint.key();
        market.quote_mint = ctx.accounts.quote_mint.key();
        market.settlement_ts = settlement_ts;
        market.contract_size_kg = contract_size_kg;
        market.initial_margin_bps = initial_margin_bps;
        market.maintenance_margin_bps = maintenance_margin_bps;
        market.fee_bps = fee_bps;
        market.farmer_fee_bps = farmer_fee_bps;
        market.buyer_fee_bps = buyer_fee_bps;
        market.max_notional_per_deal = max_notional_per_deal;
        market.max_qty_per_deal = max_qty_per_deal;
        market.max_oracle_age_sec = max_oracle_age_sec;
        market.twap_window_sec = twap_window_sec;
        market.insurance_bps = insurance_bps;
        market.insurance_treasury = ctx.accounts.insurance_treasury.key();
        market.min_transfer_amount = min_transfer_amount;
        market.last_price_per_kg = 0;
        market.prev_price_per_kg = 0;
        market.last_oracle_update_ts = 0;
        market.twap_acc = 0;
        market.twap_time_acc = 0;
        market.paused = false;
        market.price_mode = PriceMode::LastPrice as u8;
        market.last_price_nonce = 0;
        market.default_margin_call_grace_sec = 0;
        market.insurance_treasury_authority = Pubkey::default();
        market.program_version = PROGRAM_VERSION;

        emit!(MarketCreated {
            market: market_key,
            authority: market.authority,
            cft_mint: market.cft_mint,
            quote_mint: market.quote_mint,
            settlement_ts,
        });
        Ok(())
    }

    // Oracle publishes a price; includes nonce and performs staleness / price-band checks
    pub fn publish_price(ctx: Context<PublishPrice>, price_per_kg: u64, nonce: u64) -> Result<()> {
        version_guard_market(&ctx.accounts.market)?;
        assert_is_oracle(&ctx.accounts.market, &ctx.accounts.oracle_publisher)?;

        // replay/nonce protection
        let market = &mut ctx.accounts.market;
        require!(nonce > market.last_price_nonce, CoffeeError::ReplayOrStaleNonce);
        require!(price_per_kg > 0, CoffeeError::ZeroPrice);

        let now_ts = Clock::get()?.unix_timestamp;

        // staleness: if last update exists, ensure age <= max
        if market.last_oracle_update_ts > 0 && market.max_oracle_age_sec > 0 {
            let age_u64 = abs_i64_to_u64(now_ts - market.last_oracle_update_ts);
            require!(age_u64 <= market.max_oracle_age_sec, CoffeeError::OracleStale);
        }

        // price-band check against previous price (if present)
        if market.prev_price_per_kg > 0 {
            is_price_band_ok(market.prev_price_per_kg, price_per_kg, 2_500 /* 25% demo cap */)?;
        }

        // Update TWAP (time-weighted)
        update_twap(market, now_ts)?;

        market.prev_price_per_kg = market.last_price_per_kg;
        market.last_price_per_kg = price_per_kg;
        market.last_oracle_update_ts = now_ts;
        market.last_price_nonce = nonce;

        emit!(PricePublished {
            market: ctx.accounts.market.key(),
            price_per_kg,
            publisher: ctx.accounts.oracle_publisher.key(),
            ts: now_ts,
            nonce,
        });

        Ok(())
    }

    // Open a bilateral deal (farmer short, buyer long), both deposit initial margin
    #[allow(clippy::too_many_arguments)]
    pub fn open_deal(
        ctx: Context<OpenDeal>,
        agreed_price_per_kg: u64,
        quantity_kg: u64,
        physical_delivery: bool,
        deadline_ts: i64,
        assets: Vec<Pubkey>,        // up to MAX_ASSETS
        asset_qty: Vec<u64>,        // parallel arrays
        merkle_root: Option<[u8; 32]>,
        referrer: Option<Pubkey>,
        fee_split_bps: Option<u16>,
    ) -> Result<()> {
        version_guard_market(&ctx.accounts.market)?;
        let market = &ctx.accounts.market;
        require!(!market.paused, CoffeeError::MarketPaused);
        require!(agreed_price_per_kg > 0, CoffeeError::ZeroPrice);
        require!(quantity_kg > 0, CoffeeError::ZeroQty);
        require!(assets.len() == asset_qty.len(), CoffeeError::InvalidAssetBasket);
        require!(assets.len() <= MAX_ASSETS, CoffeeError::TooManyAssets);
        require!(quantity_kg <= market.max_qty_per_deal, CoffeeError::DealQtyExceedsLimit);

        // compute notional and check cap
        let notional = (agreed_price_per_kg as u128)
            .checked_mul(quantity_kg as u128)
            .ok_or(CoffeeError::MathOverflow)?;
        require!(notional <= market.max_notional_per_deal as u128, CoffeeError::DealNotionalExceedsLimit);

        // persist vault_auth bump
        ctx.accounts.vault_auth.bump = ctx.bumps.vault_auth;

        // avoid borrow conflict: capture deal key before mut borrow
        let deal_key = ctx.accounts.deal.key();
        let deal = &mut ctx.accounts.deal;

        deal.version = PROGRAM_VERSION;
        deal.market = market.key();
        deal.farmer = ctx.accounts.farmer.key();
        deal.buyer = ctx.accounts.buyer.key();
        deal.agreed_price_per_kg = agreed_price_per_kg;
        deal.quantity_kg = quantity_kg;
        deal.initial_margin_each = 0; // set after transfers
        deal.physical_delivery = physical_delivery;
        deal.settled = false;
        deal.settling = false;
        deal.liquidated = false;
        deal.farmer_deposited = false;
        deal.buyer_deposited = false;
        deal.deadline_ts = deadline_ts;
        deal.delivered_kg_total = 0;
        deal.margin_call_ts = 0;
        deal.margin_call_grace_sec = 0;
        deal.referrer = referrer.unwrap_or_default();
        deal.fee_split_bps = fee_split_bps.unwrap_or(0);

        deal.asset_count = assets.len() as u8;
        for i in 0..assets.len() {
            deal.assets[i] = assets[i];
            deal.asset_qty[i] = asset_qty[i];
        }
        deal.merkle_root = merkle_root.unwrap_or(EMPTY_MERKLE_ROOT);

        // compute initial margin
        let req_margin = bps_mul_u128(notional, market.initial_margin_bps)?;
        let req_margin_u64: u64 = req_margin.try_into().map_err(|_| CoffeeError::MathOverflow)?;

        // farmer -> farmer vault
        token::transfer(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.farmer_margin_from.to_account_info(),
                    to: ctx.accounts.farmer_margin_vault.to_account_info(),
                    authority: ctx.accounts.farmer.to_account_info(),
                },
            ),
            req_margin_u64,
        )?;
        deal.farmer_deposited = true;

        // buyer -> buyer vault
        token::transfer(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.buyer_margin_from.to_account_info(),
                    to: ctx.accounts.buyer_margin_vault.to_account_info(),
                    authority: ctx.accounts.buyer.to_account_info(),
                },
            ),
            req_margin_u64,
        )?;
        deal.buyer_deposited = true;

        deal.initial_margin_each = req_margin_u64;

        emit!(DealOpened {
            deal: deal_key,
            market: market.key(),
            farmer: deal.farmer,
            buyer: deal.buyer,
            agreed_price_per_kg,
            quantity_kg,
        });

        Ok(())
    }

    // Top up margin by either side
    pub fn top_up_margin(ctx: Context<TopUpMargin>, amount: u64) -> Result<()> {
        version_guard_market(&ctx.accounts.market)?;
        require!(amount > 0, CoffeeError::ZeroAmount);

        let who = ctx.accounts.who.key();
        let deal = &ctx.accounts.deal;
        assert_is_counterparty(&deal, &ctx.accounts.who)?;

        if who == deal.farmer {
            token::transfer(
                CpiContext::new(
                    ctx.accounts.token_program.to_account_info(),
                    Transfer {
                        from: ctx.accounts.from_ata.to_account_info(),
                        to: ctx.accounts.farmer_margin_vault.to_account_info(),
                        authority: ctx.accounts.who.to_account_info(),
                    },
                ),
                amount,
            )?;
        } else {
            token::transfer(
                CpiContext::new(
                    ctx.accounts.token_program.to_account_info(),
                    Transfer {
                        from: ctx.accounts.from_ata.to_account_info(),
                        to: ctx.accounts.buyer_margin_vault.to_account_info(),
                        authority: ctx.accounts.who.to_account_info(),
                    },
                ),
                amount,
            )?;
        }

        emit!(MarginToppedUp {
            deal: deal.key(),
            who,
            amount,
        });

        Ok(())
    }

    // margin_call: sets a margin call timestamp and grace period; liquidation only after grace expires
    pub fn margin_call(ctx: Context<MarginCall>, grace_sec: u64) -> Result<()> {
        version_guard_market(&ctx.accounts.market)?;
        let market = &ctx.accounts.market;
        // only market authority can invoke
        require!(ctx.accounts.authority.key() == market.authority, CoffeeError::Unauthorized);

        let deal = &mut ctx.accounts.deal;
        require!(!deal.settled, CoffeeError::DealAlreadySettled);
        let now = Clock::get()?.unix_timestamp;
        deal.margin_call_ts = now;
        deal.margin_call_grace_sec = grace_sec;

        emit!(MarginCalled {
            deal: deal.key(),
            ts: now,
            grace_sec,
        });
        Ok(())
    }

    // mark-to-market check and possible liquidation (liquidation only effective after grace)
    pub fn mark_to_market(ctx: Context<MtmCheck>) -> Result<()> {
        version_guard_market(&ctx.accounts.market)?;
        let market = &ctx.accounts.market;
        let deal = &mut ctx.accounts.deal;
        require!(!deal.settled, CoffeeError::DealAlreadySettled);

        // choose price by mode
        let price = match market.price_mode {
            0 => market.last_price_per_kg,
            1 => {
                require!(market.twap_time_acc > 0, CoffeeError::ZeroPrice);
                (market.twap_acc / (market.twap_time_acc as u128)) as u64
            }
            _ => market.last_price_per_kg,
        };
        require!(price > 0, CoffeeError::ZeroPrice);

        let notional_now = (price as u128)
            .checked_mul(deal.quantity_kg as u128)
            .ok_or(CoffeeError::MathOverflow)?;
        let maint = bps_mul_u128(notional_now, market.maintenance_margin_bps)? as u64;

        let farmer_ok = ctx.accounts.farmer_margin_vault.amount >= maint;
        let buyer_ok = ctx.accounts.buyer_margin_vault.amount >= maint;

        if !(farmer_ok && buyer_ok) {
            // check margin call grace
            if deal.margin_call_ts == 0 {
                // set margin call automatically with default grace
                deal.margin_call_ts = Clock::get()?.unix_timestamp;
                deal.margin_call_grace_sec = market.default_margin_call_grace_sec;
                emit!(MarginCalled { deal: deal.key(), ts: deal.margin_call_ts, grace_sec: deal.margin_call_grace_sec });
            } else {
                let now = Clock::get()?.unix_timestamp;
                let grace_end = deal.margin_call_ts.checked_add(deal.margin_call_grace_sec as i64).ok_or(CoffeeError::MathOverflow)?;
                if now >= grace_end {
                    deal.liquidated = true;
                    emit!(LiquidationFlagged { deal: deal.key(), ts: now });
                }
            }
        }
        Ok(())
    }

    // Cash settlement at/after expiry using market price or TWAP; supports fallback and insurance payouts
    pub fn settle_cash(ctx: Context<SettleCash>) -> Result<()> {
        version_guard_market(&ctx.accounts.market)?;
        let market = &ctx.accounts.market;
        let deal_key = ctx.accounts.deal.key();
        let deal = &mut ctx.accounts.deal;

        require!(!deal.settled, CoffeeError::DealAlreadySettled);

        // allow settlement if market settled time reached OR if post-deadline auto cash fallback
        let now = Clock::get()?.unix_timestamp;
        require!(now >= market.settlement_ts || now >= deal.deadline_ts, CoffeeError::NotYetSettleTime);

        // Reentrancy guard
        deal.start_settling();

        // choose settlement price
        let price = match market.price_mode {
            0 => market.last_price_per_kg,
            1 => {
                require!(market.twap_time_acc > 0, CoffeeError::ZeroPrice);
                (market.twap_acc / (market.twap_time_acc as u128)) as u64
            }
            _ => market.last_price_per_kg,
        };
        require!(price > 0, CoffeeError::ZeroPrice);

        // PnL calc for buyer (long)
        let pnl_long = signed_mul_diff(
            deal.agreed_price_per_kg,
            price,
            deal.quantity_kg,
            SignRole::Long,
        ).ok_or(CoffeeError::MathOverflow)?;

        // fee on notional
        let notional = (deal.agreed_price_per_kg as u128)
            .checked_mul(deal.quantity_kg as u128)
            .ok_or(CoffeeError::MathOverflow)?;
        let fee_total = bps_mul_u128(notional, market.fee_bps)? as u64;

        // split fee into farmer/buyer tiers
        let farmer_cut = bps_of_u64(fee_total, market.farmer_fee_bps)?;
        let buyer_cut = bps_of_u64(fee_total, market.buyer_fee_bps)?;
        // insurance slice
        let insurance_cut = bps_of_u64(fee_total, market.insurance_bps)?;
        let protocol_cut = fee_total
            .checked_sub(farmer_cut).and_then(|v| v.checked_sub(buyer_cut)).and_then(|v| v.checked_sub(insurance_cut))
            .ok_or(CoffeeError::MathOverflow)?;

        // collect fees (capped). For brevity we try to move protocol_cut from farmer vault; adapt if needed.
        let farmer_fee = farmer_cut.min(ctx.accounts.farmer_margin_vault.amount);
        let buyer_fee = buyer_cut.min(ctx.accounts.buyer_margin_vault.amount);

        // protocol + farmer + buyer fees -> fee_treasury (naive routing demo)
        let proto_plus_farmer = farmer_fee.saturating_add(protocol_cut);
        if proto_plus_farmer > 0 {
            transfer_from_vault_to(
                proto_plus_farmer.min(ctx.accounts.farmer_margin_vault.amount),
                &ctx.accounts.vault_auth,
                &ctx.accounts.farmer_margin_vault,
                &ctx.accounts.fee_treasury,
                &ctx.accounts.token_program,
                &deal_key,
            )?;
        }
        if buyer_fee > 0 {
            transfer_from_vault_to(
                buyer_fee.min(ctx.accounts.buyer_margin_vault.amount),
                &ctx.accounts.vault_auth,
                &ctx.accounts.buyer_margin_vault,
                &ctx.accounts.fee_treasury,
                &ctx.accounts.token_program,
                &deal_key,
            )?;
        }
        // insurance from buyer vault first, then farmer
        let insurance_from_buyer = insurance_cut.min(ctx.accounts.buyer_margin_vault.amount);
        if insurance_from_buyer > 0 {
            transfer_from_vault_to(
                insurance_from_buyer,
                &ctx.accounts.vault_auth,
                &ctx.accounts.buyer_margin_vault,
                &ctx.accounts.insurance_treasury,
                &ctx.accounts.token_program,
                &deal_key,
            )?;
        }
        let remaining_insurance = insurance_cut.saturating_sub(insurance_from_buyer);
        if remaining_insurance > 0 {
            transfer_from_vault_to(
                remaining_insurance.min(ctx.accounts.farmer_margin_vault.amount),
                &ctx.accounts.vault_auth,
                &ctx.accounts.farmer_margin_vault,
                &ctx.accounts.insurance_treasury,
                &ctx.accounts.token_program,
                &deal_key,
            )?;
        }

        // compute PnL settlement (pay winner from loser vault; use insurance shortfall if any)
        if pnl_long > 0 {
            // buyer wins
            let pnl = pnl_long as u64;
            let pay = pnl.min(ctx.accounts.farmer_margin_vault.amount);
            transfer_from_vault_to(
                pay,
                &ctx.accounts.vault_auth,
                &ctx.accounts.farmer_margin_vault,
                &ctx.accounts.buyer_receive,
                &ctx.accounts.token_program,
                &deal_key,
            )?;
            if pay < pnl {
                let shortfall = pnl - pay;
                // draw from insurance treasury directly (requires correct authority model in production)
                let draw = shortfall.min(ctx.accounts.insurance_treasury.amount);
                if draw > 0 {
                    // WARNING: placeholder safeguard
                    return err!(CoffeeError::Unauthorized);
                }
            }
        } else if pnl_long < 0 {
            // farmer wins
            let pnl = (-pnl_long) as u64;
            let pay = pnl.min(ctx.accounts.buyer_margin_vault.amount);
            transfer_from_vault_to(
                pay,
                &ctx.accounts.vault_auth,
                &ctx.accounts.buyer_margin_vault,
                &ctx.accounts.farmer_receive,
                &ctx.accounts.token_program,
                &deal_key,
            )?;
            if pay < pnl {
                let shortfall = pnl - pay;
                let draw = shortfall.min(ctx.accounts.insurance_treasury.amount);
                if draw > 0 {
                    return err!(CoffeeError::Unauthorized);
                }
            }
        }

        // return residuals (respect min_transfer_amount to avoid dust)
        let min_transfer = market.min_transfer_amount;
        if ctx.accounts.farmer_margin_vault.amount > min_transfer {
            let amt = ctx.accounts.farmer_margin_vault.amount;
            transfer_from_vault_to(
                amt,
                &ctx.accounts.vault_auth,
                &ctx.accounts.farmer_margin_vault,
                &ctx.accounts.farmer_receive,
                &ctx.accounts.token_program,
                &deal_key,
            )?;
        }
        if ctx.accounts.buyer_margin_vault.amount > min_transfer {
            let amt = ctx.accounts.buyer_margin_vault.amount;
            transfer_from_vault_to(
                amt,
                &ctx.accounts.vault_auth,
                &ctx.accounts.buyer_margin_vault,
                &ctx.accounts.buyer_receive,
                &ctx.accounts.token_program,
                &deal_key,
            )?;
        }

        deal.mark_settled();

        emit!(SettledCash {
            deal: deal.key(),
            market: market.key(),
            price,
        });

        Ok(())
    }

    // Verify physical delivery, support partial deliveries, merkle proof, minting or basket transfers
    pub fn verify_and_settle_physical(
        ctx: Context<VerifyAndSettlePhysical>,
        delivered_kg: u64,
        proof_hashes: Vec<[u8; 32]>, // capped by MAX_PROOF_HASHES
        leaf: Option<[u8; 32]>,
    ) -> Result<()> {
        version_guard_market(&ctx.accounts.market)?;
        let market = &ctx.accounts.market;
        require!(!market.paused, CoffeeError::MarketPaused);

        // cap proofs
        require!(proof_hashes.len() <= MAX_PROOF_HASHES, CoffeeError::ProofTooLarge);

        let deal_key = ctx.accounts.deal.key();
        let deal = &mut ctx.accounts.deal;
        require!(!deal.settled, CoffeeError::DealAlreadySettled);
        require!(delivered_kg > 0, CoffeeError::ZeroQty);

        // ensure verifier
        assert_is_verifier(&market, &ctx.accounts.verifier)?;

        // verify merkle if used
        if deal.merkle_root != EMPTY_MERKLE_ROOT {
            let leaf_val = leaf.ok_or(CoffeeError::MerkleProofMissing)?;
            let ok = verify_merkle_proof(leaf_val, &proof_hashes, deal.merkle_root)?;
            require!(ok, CoffeeError::MerkleProofInvalid);
        }

        // partial delivery logic
        let new_total = deal.delivered_kg_total.checked_add(delivered_kg).ok_or(CoffeeError::MathOverflow)?;
        require!(new_total <= deal.quantity_kg, CoffeeError::OverDelivery);

        // reentrancy guard
        deal.start_settling();

        // bind cft key before signer seeds
        let cft_key = ctx.accounts.cft_mint.key();
        let cft_bump = ctx.accounts.cft_mint_auth.bump;
        let signer_seeds: &[&[&[u8]]] = &[&[SEED_PREFIX, b"cft_auth", cft_key.as_ref(), &[cft_bump]]];

        // mint CFT if present in basket
        for i in 0..(deal.asset_count as usize) {
            if deal.assets[i] == market.cft_mint {
                token::mint_to(
                    CpiContext::new_with_signer(
                        ctx.accounts.token_program.to_account_info(),
                        MintTo {
                            mint: ctx.accounts.cft_mint.to_account_info(),
                            to: ctx.accounts.buyer_cft_ata.to_account_info(),
                            authority: ctx.accounts.cft_mint_auth.to_account_info(),
                        },
                        signer_seeds,
                    ),
                    delivered_kg,
                )?;
                break;
            }
        }

        // payout to farmer: agreed_price_per_kg * delivered_kg
        let pay = (deal.agreed_price_per_kg as u128)
            .checked_mul(delivered_kg as u128)
            .ok_or(CoffeeError::MathOverflow)? as u64;
        let pay_amt = pay.min(ctx.accounts.buyer_margin_vault.amount);
        transfer_from_vault_to(
            pay_amt,
            &ctx.accounts.vault_auth,
            &ctx.accounts.buyer_margin_vault,
            &ctx.accounts.farmer_receive,
            &ctx.accounts.token_program,
            &deal_key,
        )?;

        // update delivered total
        deal.delivered_kg_total = new_total;

        // return residuals on completion; else leave funds until full delivery or deadline
        if deal.delivered_kg_total == deal.quantity_kg {
            if ctx.accounts.farmer_margin_vault.amount > market.min_transfer_amount {
                let amt = ctx.accounts.farmer_margin_vault.amount;
                transfer_from_vault_to(
                    amt,
                    &ctx.accounts.vault_auth,
                    &ctx.accounts.farmer_margin_vault,
                    &ctx.accounts.farmer_receive,
                    &ctx.accounts.token_program,
                    &deal_key,
                )?;
            }
            if ctx.accounts.buyer_margin_vault.amount > market.min_transfer_amount {
                let amt = ctx.accounts.buyer_margin_vault.amount;
                transfer_from_vault_to(
                    amt,
                    &ctx.accounts.vault_auth,
                    &ctx.accounts.buyer_margin_vault,
                    &ctx.accounts.buyer_receive,
                    &ctx.accounts.token_program,
                    &deal_key,
                )?;
            }
            deal.mark_settled();
        }

        emit!(SettledPhysical {
            deal: deal.key(),
            market: market.key(),
            delivered_kg,
            total_delivered: deal.delivered_kg_total,
        });

        Ok(())
    }

    // Cancel deal before both deposited or before deadline (refunds)
    pub fn cancel_deal(ctx: Context<CancelDeal>) -> Result<()> {
        version_guard_market(&ctx.accounts.market)?;
        let deal_key = ctx.accounts.deal.key();
        let deal = &mut ctx.accounts.deal;
        require!(!deal.settled, CoffeeError::DealAlreadySettled);

        // allow cancel if not both deposited OR before deadline
        if deal.farmer_deposited && deal.buyer_deposited {
            return err!(CoffeeError::CannotCancelAfterBothDeposited);
        }
        let now = Clock::get()?.unix_timestamp;
        require!(now < deal.deadline_ts, CoffeeError::DeadlinePassed);

        // refund if any
        if ctx.accounts.farmer_margin_vault.amount > 0 {
            let amt = ctx.accounts.farmer_margin_vault.amount;
            transfer_from_vault_to(
                amt,
                &ctx.accounts.vault_auth,
                &ctx.accounts.farmer_margin_vault,
                &ctx.accounts.farmer_receive,
                &ctx.accounts.token_program,
                &deal_key,
            )?;
        }
        if ctx.accounts.buyer_margin_vault.amount > 0 {
            let amt = ctx.accounts.buyer_margin_vault.amount;
            transfer_from_vault_to(
                amt,
                &ctx.accounts.vault_auth,
                &ctx.accounts.buyer_margin_vault,
                &ctx.accounts.buyer_receive,
                &ctx.accounts.token_program,
                &deal_key,
            )?;
        }

        deal.mark_settled();
        emit!(DealCanceled { deal: deal.key(), market: ctx.accounts.market.key() });
        Ok(())
    }

    // rotate oracle publisher (propose + activate after timelock)
    pub fn propose_rotate_oracle(ctx: Context<RotateRole>, new_oracle: Pubkey, effective_after_ts: i64) -> Result<()> {
        version_guard_market(&ctx.accounts.market)?;
        let market = &mut ctx.accounts.market;
        require!(ctx.accounts.authority.key() == market.authority, CoffeeError::Unauthorized);
        market.pending_oracle = new_oracle;
        market.pending_oracle_effective_ts = effective_after_ts;
        emit!(RoleRotationProposed { market: market.key(), role: b"oracle".to_vec(), pending: new_oracle, effective_ts: effective_after_ts });
        Ok(())
    }

    pub fn activate_rotate_oracle(ctx: Context<RotateRole>) -> Result<()> {
        version_guard_market(&ctx.accounts.market)?;
        let market = &mut ctx.accounts.market;
        let now = Clock::get()?.unix_timestamp;
        require!(market.pending_oracle != Pubkey::default(), CoffeeError::NoPendingRotation);
        require!(now >= market.pending_oracle_effective_ts, CoffeeError::RotationNotEffectiveYet);
        market.oracle_publisher = market.pending_oracle;
        market.pending_oracle = Pubkey::default();
        market.pending_oracle_effective_ts = 0;
        emit!(RoleRotationActivated { market: market.key(), role: b"oracle".to_vec(), activated: market.oracle_publisher });
        Ok(())
    }

    // Close deal (account closed to receiver) - only when settled
    pub fn close_deal(ctx: Context<CloseDeal>) -> Result<()> {
        version_guard_market(&ctx.accounts.market)?;
        require!(ctx.accounts.deal.settled, CoffeeError::DealNotSettled);
        Ok(())
    }
}

// ------------------------- Accounts & State -------------------------

#[derive(Accounts)]
#[instruction(decimals: u8)]
pub struct InitCftMint<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,

    #[account(
        init,
        payer = payer,
        mint::decimals = 3, // choose alignment with decimals param if desired
        mint::authority = cft_mint_auth,
        mint::freeze_authority = cft_mint_auth,
    )]
    pub cft_mint: Account<'info, Mint>,

    #[account(
        init,
        payer = payer,
        space = 8 + CftMintAuth::SIZE,
        seeds = [SEED_PREFIX, b"cft_auth", cft_mint.key().as_ref()],
        bump
    )]
    pub cft_mint_auth: Account<'info, CftMintAuth>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[account]
pub struct CftMintAuth {
    pub bump: u8,
}
impl CftMintAuth {
    pub const SIZE: usize = 1 + 8;
}

#[derive(Accounts)]
pub struct CreateMarket<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,

    /// CHECK: multisig or authority PDA ok
    #[account(mut)]
    pub verifier: UncheckedAccount<'info>,

    /// CHECK: multisig or oracle PDA ok
    #[account(mut)]
    pub oracle_publisher: UncheckedAccount<'info>,

    pub cft_mint: Account<'info, Mint>,
    pub quote_mint: Account<'info, Mint>,

    /// Insurance treasury ATA (must be ATA for quote_mint)
    #[account(mut, constraint = insurance_treasury.mint == quote_mint.key())]
    pub insurance_treasury: Account<'info, TokenAccount>,

    #[account(
        init,
        payer = authority,
        space = 8 + Market::INIT_SPACE,
        seeds = [SEED_PREFIX, b"market", authority.key().as_ref(), cft_mint.key().as_ref(), quote_mint.key().as_ref()],
        bump
    )]
    pub market: Account<'info, Market>,

    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[account]
pub struct Market {
    pub version: u8,
    pub authority: Pubkey,
    pub verifier: Pubkey,
    pub oracle_publisher: Pubkey,

    // pending rotation fields
    pub pending_oracle: Pubkey,
    pub pending_oracle_effective_ts: i64,

    pub cft_mint: Pubkey,
    pub quote_mint: Pubkey,
    pub insurance_treasury: Pubkey,

    pub settlement_ts: i64,
    pub contract_size_kg: u64,

    // margins & fees
    pub initial_margin_bps: u16,
    pub maintenance_margin_bps: u16,
    pub fee_bps: u16,
    pub farmer_fee_bps: u16,
    pub buyer_fee_bps: u16,
    pub insurance_bps: u16,
    pub default_margin_call_grace_sec: u64,

    // exposure caps
    pub max_notional_per_deal: u64,
    pub max_qty_per_deal: u64,

    // oracle / price
    pub last_price_per_kg: u64,
    pub prev_price_per_kg: u64,
    pub last_price_nonce: u64,
    pub last_oracle_update_ts: i64,
    pub max_oracle_age_sec: u64,

    // TWAP accumulator (time-weighted)
    pub twap_acc: u128,     // sum(price * seconds)
    pub twap_time_acc: u64, // sum(seconds)
    pub twap_window_sec: u64,
    pub price_mode: u8,

    // operational
    pub paused: bool,
    pub min_transfer_amount: u64,

    // misc
    pub insurance_treasury_authority: Pubkey, // authority for insurance ATA transfers (hook for prod model)
    pub program_version: u8,
}

impl Market {
    // rough size; tune before production
    pub const INIT_SPACE: usize = 1 + 32*12 + 8*12 + 2*6 + 16 + 8 + 8 + 32;
}

#[derive(Accounts)]
pub struct PublishPrice<'info> {
    #[account(mut, has_one = oracle_publisher)]
    pub market: Account<'info, Market>,
    /// CHECK: oracle publisher signer (may be multisig PDA)
    pub oracle_publisher: Signer<'info>,
}

#[derive(Accounts)]
#[instruction(agreed_price_per_kg: u64, quantity_kg: u64)]
pub struct OpenDeal<'info> {
    #[account(mut)]
    pub farmer: Signer<'info>,
    #[account(mut)]
    pub buyer: Signer<'info>,

    #[account(mut)]
    pub market: Account<'info, Market>,

    pub quote_mint: Account<'info, Mint>,

    #[account(
        init,
        payer = buyer,
        space = 8 + Deal::INIT_SPACE,
        seeds = [SEED_PREFIX, b"deal", market.key().as_ref(), farmer.key().as_ref(), buyer.key().as_ref()],
        bump
    )]
    pub deal: Account<'info, Deal>,

    #[account(
        init,
        payer = buyer,
        space = 8 + VaultAuth::SIZE,
        seeds = [SEED_PREFIX, b"vault_auth", deal.key().as_ref()],
        bump
    )]
    pub vault_auth: Account<'info, VaultAuth>,

    #[account(
        init,
        payer = buyer,
        associated_token::mint = quote_mint,
        associated_token::authority = vault_auth,
    )]
    pub farmer_margin_vault: Account<'info, TokenAccount>,

    #[account(
        init,
        payer = buyer,
        associated_token::mint = quote_mint,
        associated_token::authority = vault_auth,
    )]
    pub buyer_margin_vault: Account<'info, TokenAccount>,

    #[account(mut, constraint = farmer_margin_from.mint == quote_mint.key())]
    pub farmer_margin_from: Account<'info, TokenAccount>,

    #[account(mut, constraint = buyer_margin_from.mint == quote_mint.key())]
    pub buyer_margin_from: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[account]
pub struct VaultAuth {
    pub bump: u8,
}
impl VaultAuth {
    pub const SIZE: usize = 1 + 8;
}

#[account]
pub struct Deal {
    pub version: u8,
    pub market: Pubkey,
    pub farmer: Pubkey,
    pub buyer: Pubkey,
    pub agreed_price_per_kg: u64,
    pub quantity_kg: u64,
    pub initial_margin_each: u64,

    // settlement & lifecycle
    pub physical_delivery: bool,
    pub delivered_kg_total: u64,
    pub liquidated: bool,
    pub settled: bool,
    pub settling: bool, // reentrancy guard
    pub farmer_deposited: bool,
    pub buyer_deposited: bool,
    pub deadline_ts: i64,
    pub margin_call_ts: i64,
    pub margin_call_grace_sec: u64,

    // optional referral & fee split
    pub referrer: Pubkey,
    pub fee_split_bps: u16,

    // multi-asset basket (fixed arrays)
    pub asset_count: u8,
    pub assets: [Pubkey; MAX_ASSETS],
    pub asset_qty: [u64; MAX_ASSETS],

    // merkle root for basket proof
    pub merkle_root: [u8; 32],
}

impl Deal {
    pub const INIT_SPACE: usize = 1 + 32*6 + 8*8 + 1*10 + (32*MAX_ASSETS) + (8*MAX_ASSETS) + 40;
    pub fn mark_settled(&mut self) {
        self.settled = true;
        self.settling = false;
    }
    pub fn start_settling(&mut self) {
        self.settling = true;
    }
}

#[derive(Accounts)]
pub struct TopUpMargin<'info> {
    #[account(mut)]
    pub who: Signer<'info>,

    pub market: Account<'info, Market>,

    #[account(mut, has_one = market)]
    pub deal: Account<'info, Deal>,

    #[account(seeds = [SEED_PREFIX, b"vault_auth", deal.key().as_ref()], bump)]
    pub vault_auth: Account<'info, VaultAuth>,

    #[account(mut, constraint = from_ata.mint == market.quote_mint)]
    pub from_ata: Account<'info, TokenAccount>,

    #[account(mut, constraint = farmer_margin_vault.mint == market.quote_mint)]
    pub farmer_margin_vault: Account<'info, TokenAccount>,

    #[account(mut, constraint = buyer_margin_vault.mint == market.quote_mint)]
    pub buyer_margin_vault: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct MarginCall<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,

    #[account(mut, has_one = market)]
    pub deal: Account<'info, Deal>,

    pub market: Account<'info, Market>,
}

#[derive(Accounts)]
pub struct MtmCheck<'info> {
    pub market: Account<'info, Market>,

    #[account(mut, has_one = market)]
    pub deal: Account<'info, Deal>,

    #[account(seeds = [SEED_PREFIX, b"vault_auth", deal.key().as_ref()], bump)]
    pub vault_auth: Account<'info, VaultAuth>,

    #[account(constraint = farmer_margin_vault.mint == market.quote_mint)]
    pub farmer_margin_vault: Account<'info, TokenAccount>,

    #[account(constraint = buyer_margin_vault.mint == market.quote_mint)]
    pub buyer_margin_vault: Account<'info, TokenAccount>,
}

#[derive(Accounts)]
pub struct SettleCash<'info> {
    pub market: Account<'info, Market>,

    #[account(mut, has_one = market)]
    pub deal: Account<'info, Deal>,

    #[account(seeds = [SEED_PREFIX, b"vault_auth", deal.key().as_ref()], bump)]
    pub vault_auth: Account<'info, VaultAuth>,

    #[account(mut, constraint = farmer_margin_vault.mint == market.quote_mint)]
    pub farmer_margin_vault: Account<'info, TokenAccount>,

    #[account(mut, constraint = buyer_margin_vault.mint == market.quote_mint)]
    pub buyer_margin_vault: Account<'info, TokenAccount>,

    #[account(mut, constraint = farmer_receive.mint == market.quote_mint)]
    pub farmer_receive: Account<'info, TokenAccount>,

    #[account(mut, constraint = buyer_receive.mint == market.quote_mint)]
    pub buyer_receive: Account<'info, TokenAccount>,

    #[account(mut, constraint = fee_treasury.mint == market.quote_mint)]
    pub fee_treasury: Account<'info, TokenAccount>,

    #[account(mut, constraint = insurance_treasury.mint == market.quote_mint)]
    pub insurance_treasury: Account<'info, TokenAccount>,

    /// CHECK: authority for insurance treasury (placeholder; wire to PDA in prod)
    pub insurance_treasury_authority: UncheckedAccount<'info>,

    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct VerifyAndSettlePhysical<'info> {
    #[account(mut, has_one = verifier, has_one = cft_mint, has_one = quote_mint)]
    pub market: Account<'info, Market>,

    #[account(mut, has_one = market)]
    pub deal: Account<'info, Deal>,

    /// CHECK: verifier may be multisig PDA
    #[account(mut)]
    pub verifier: Signer<'info>,

    #[account(mut)]
    pub cft_mint: Account<'info, Mint>,

    #[account(seeds = [SEED_PREFIX, b"cft_auth", cft_mint.key().as_ref()], bump)]
    pub cft_mint_auth: Account<'info, CftMintAuth>,

    #[account(
        init_if_needed,
        payer = verifier,
        associated_token::mint = cft_mint,
        associated_token::authority = buyer
    )]
    pub buyer_cft_ata: Account<'info, TokenAccount>,

    #[account(seeds = [SEED_PREFIX, b"vault_auth", deal.key().as_ref()], bump)]
    pub vault_auth: Account<'info, VaultAuth>,

    #[account(mut, constraint = buyer_margin_vault.mint == market.quote_mint)]
    pub buyer_margin_vault: Account<'info, TokenAccount>,

    #[account(mut, constraint = farmer_margin_vault.mint == market.quote_mint)]
    pub farmer_margin_vault: Account<'info, TokenAccount>,

    #[account(mut, constraint = farmer_receive.mint == market.quote_mint)]
    pub farmer_receive: Account<'info, TokenAccount>,

    #[account(mut, constraint = buyer_receive.mint == market.quote_mint)]
    pub buyer_receive: Account<'info, TokenAccount>,

    /// CHECK: only used as ATA authority
    pub buyer: UncheckedAccount<'info>,

    pub quote_mint: Account<'info, Mint>,

    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct CancelDeal<'info> {
    #[account(mut, has_one = market)]
    pub deal: Account<'info, Deal>,

    #[account(seeds = [SEED_PREFIX, b"vault_auth", deal.key().as_ref()], bump)]
    pub vault_auth: Account<'info, VaultAuth>,

    #[account(mut, constraint = farmer_margin_vault.mint == market.quote_mint)]
    pub farmer_margin_vault: Account<'info, TokenAccount>,

    #[account(mut, constraint = buyer_margin_vault.mint == market.quote_mint)]
    pub buyer_margin_vault: Account<'info, TokenAccount>,

    #[account(mut, constraint = farmer_receive.mint == market.quote_mint)]
    pub farmer_receive: Account<'info, TokenAccount>,

    #[account(mut, constraint = buyer_receive.mint == market.quote_mint)]
    pub buyer_receive: Account<'info, TokenAccount>,

    pub market: Account<'info, Market>,

    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct RotateRole<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,

    #[account(mut)]
    pub market: Account<'info, Market>,
}

#[derive(Accounts)]
pub struct CloseDeal<'info> {
    #[account(mut, has_one = market, close = receiver)]
    pub deal: Account<'info, Deal>,

    pub market: Account<'info, Market>,

    /// CHECK: receiver of rent lamports on close
    #[account(mut)]
    pub receiver: UncheckedAccount<'info>,
}

// ------------------------- Helpers -------------------------

fn version_guard_program() -> Result<()> {
    Ok(())
}

fn version_guard_market(market: &Account<Market>) -> Result<()> {
    require!(market.program_version == PROGRAM_VERSION, CoffeeError::VersionMismatch);
    Ok(())
}

fn assert_is_oracle(_market: &Account<Market>, _oracle: &Signer) -> Result<()> {
    // TODO: check equality with market.oracle_publisher or multisig PDA logic
    Ok(())
}
fn assert_is_verifier(_market: &Account<Market>, _verifier: &Signer) -> Result<()> {
    // TODO: check equality with market.verifier or multisig PDA logic
    Ok(())
}
fn assert_is_counterparty(deal: &Account<Deal>, signer: &Signer) -> Result<()> {
    let k = signer.key();
    require!(k == deal.farmer || k == deal.buyer, CoffeeError::InvalidCounterparty);
    Ok(())
}

// safe multiplication by bps returning u128
fn bps_mul_u128(x: u128, bps: u16) -> Result<u128> {
    x.checked_mul(bps as u128)
        .and_then(|y| y.checked_div(10_000))
        .ok_or(CoffeeError::MathOverflow.into())
}

fn bps_of_u64(x: u64, bps: u16) -> Result<u64> {
    let prod = (x as u128).checked_mul(bps as u128).ok_or(CoffeeError::MathOverflow)?;
    let out = prod.checked_div(10_000).ok_or(CoffeeError::MathOverflow)?;
    Ok(out as u64)
}

enum SignRole {
    Long,
    Short,
}

// Long PnL: (mark - agreed) * qty; Short PnL is negative of long
fn signed_mul_diff(agreed: u64, mark: u64, qty: u64, role: SignRole) -> Option<i128> {
    let agreed = agreed as i128;
    let mark = mark as i128;
    let qty = qty as i128;
    let diff = match role {
        SignRole::Long => mark.checked_sub(agreed)?,
        SignRole::Short => agreed.checked_sub(mark)?,
    };
    diff.checked_mul(qty)
}

/// Transfer amount from vault (PDA authoritiy) to `to_ata` using signer PDA
fn transfer_from_vault_to<'a>(
    amount: u64,
    vault_auth: &Account<'a, VaultAuth>,
    from_vault: &Account<'a, TokenAccount>,
    to_ata: &Account<'a, TokenAccount>,
    token_program: &Program<'a, Token>,
    deal_key: &Pubkey,
) -> Result<()> {
    if amount == 0 {
        return Ok(());
    }
    let bump = vault_auth.bump;
    let seeds: &[&[&[u8]]] = &[&[SEED_PREFIX, b"vault_auth", deal_key.as_ref(), &[bump]]];

    token::transfer(
        CpiContext::new_with_signer(
            token_program.to_account_info(),
            Transfer {
                from: from_vault.to_account_info(),
                to: to_ata.to_account_info(),
                authority: vault_auth.to_account_info(),
            },
            seeds,
        ),
        amount,
    )?;
    Ok(())
}

// Merkle verification (binary, keccak-based). Returns Result<bool, _> for easy use.
fn verify_merkle_proof(mut leaf: [u8; 32], proof: &Vec<[u8; 32]>, root: [u8; 32]) -> Result<bool> {
    for p in proof.iter() {
        // deterministic ordering by bytes
        let combined = if leaf <= *p {
            [&leaf[..], &p[..]].concat()
        } else {
            [&p[..], &leaf[..]].concat()
        };
        leaf = solana_program::keccak::hash(&combined).0;
    }
    Ok(leaf == root)
}

// Helper: absolute i64 to u64 (safe)
fn abs_i64_to_u64(v: i64) -> u64 {
    if v >= 0 { v as u64 } else { (-v) as u64 }
}

// TWAP update: incorporate previous price over elapsed time into twap_acc / twap_time_acc.
// This is a simple sliding-window approximation.
fn update_twap(market: &mut Market, now_ts: i64) -> Result<()> {
    // if no previous price/time, just set last_oracle_update_ts (no accumulation)
    if market.last_oracle_update_ts == 0 {
        market.last_oracle_update_ts = now_ts;
        return Ok(());
    }

    let dt_i64 = now_ts.checked_sub(market.last_oracle_update_ts).ok_or(CoffeeError::MathOverflow)?;
    if dt_i64 <= 0 {
        market.last_oracle_update_ts = now_ts;
        return Ok(());
    }
    let dt_u64 = dt_i64 as u64;
    let add = dt_u64.min(market.twap_window_sec);

    // add last_price contribution for elapsed seconds
    let add_val = (market.last_price_per_kg as u128)
        .checked_mul(add as u128)
        .ok_or(CoffeeError::MathOverflow)?;
    market.twap_acc = market.twap_acc.checked_add(add_val).ok_or(CoffeeError::MathOverflow)?;
    market.twap_time_acc = market.twap_time_acc.checked_add(add).ok_or(CoffeeError::MathOverflow)?;

    // if we've exceeded window, scale-down (approximate sliding window)
    if market.twap_time_acc > market.twap_window_sec {
        market.twap_acc = market.twap_acc
            .checked_mul(market.twap_window_sec as u128).ok_or(CoffeeError::MathOverflow)?
            .checked_div(market.twap_time_acc as u128).ok_or(CoffeeError::MathOverflow)?;
        market.twap_time_acc = market.twap_window_sec;
    }

    market.last_oracle_update_ts = now_ts;
    Ok(())
}

// Simple price band check helper (returns Err on violation)
fn is_price_band_ok(prev: u64, next: u64, max_delta_bps: u128) -> Result<()> {
    if prev == 0 { return Ok(()); }
    let prev_u = prev as u128;
    let next_u = next as u128;
    let delta = if next_u >= prev_u { next_u - prev_u } else { prev_u - next_u };
    let delta_bps = delta.checked_mul(10_000).ok_or(CoffeeError::MathOverflow)?.checked_div(prev_u).ok_or(CoffeeError::MathOverflow)?;
    require!(delta_bps <= max_delta_bps as u128, CoffeeError::OraclePriceBandExceeded);
    Ok(())
}

// ------------------------- Events -------------------------
#[event]
pub struct CftMintInitialized {
    pub cft_mint: Pubkey,
    pub authority: Pubkey,
    pub decimals: u8,
}

#[event]
pub struct MarketCreated {
    pub market: Pubkey,
    pub authority: Pubkey,
    pub cft_mint: Pubkey,
    pub quote_mint: Pubkey,
    pub settlement_ts: i64,
}

#[event]
pub struct PricePublished {
    pub market: Pubkey,
    pub price_per_kg: u64,
    pub publisher: Pubkey,
    pub ts: i64,
    pub nonce: u64,
}

#[event]
pub struct DealOpened {
    pub deal: Pubkey,
    pub market: Pubkey,
    pub farmer: Pubkey,
    pub buyer: Pubkey,
    pub agreed_price_per_kg: u64,
    pub quantity_kg: u64,
}

#[event]
pub struct MarginToppedUp {
    pub deal: Pubkey,
    pub who: Pubkey,
    pub amount: u64,
}

#[event]
pub struct MarginCalled {
    pub deal: Pubkey,
    pub ts: i64,
    pub grace_sec: u64,
}

#[event]
pub struct LiquidationFlagged {
    pub deal: Pubkey,
    pub ts: i64,
}

#[event]
pub struct SettledCash {
    pub deal: Pubkey,
    pub market: Pubkey,
    pub price: u64,
}

#[event]
pub struct SettledPhysical {
    pub deal: Pubkey,
    pub market: Pubkey,
    pub delivered_kg: u64,
    pub total_delivered: u64,
}

#[event]
pub struct DealCanceled {
    pub deal: Pubkey,
    pub market: Pubkey,
}

#[event]
pub struct RoleRotationProposed {
    pub market: Pubkey,
    pub role: Vec<u8>,
    pub pending: Pubkey,
    pub effective_ts: i64,
}

#[event]
pub struct RoleRotationActivated {
    pub market: Pubkey,
    pub role: Vec<u8>,
    pub activated: Pubkey,
}

// ------------------------- Errors -------------------------
#[error_code]
pub enum CoffeeError {
    #[msg("Math overflow")]
    MathOverflow,
    #[msg("Zero price")]
    ZeroPrice,
    #[msg("Zero quantity")]
    ZeroQty,
    #[msg("Zero amount")]
    ZeroAmount,
    #[msg("Initial margin must be >= maintenance margin")]
    BadMarginParams,
    #[msg("Unauthorized")]
    Unauthorized,
    #[msg("Market paused")]
    MarketPaused,
    #[msg("Not yet settlement time")]
    NotYetSettleTime,
    #[msg("Wrong settlement type")]
    WrongSettlementType,
    #[msg("Deal already settled")]
    DealAlreadySettled,
    #[msg("Deal not settled")]
    DealNotSettled,
    #[msg("Delivered kg exceeds deal quantity")]
    OverDelivery,
    #[msg("Mint decimals mismatch")]
    MintDecimalsMismatch,
    #[msg("Invalid counterparty")]
    InvalidCounterparty,
    #[msg("Invalid asset basket")]
    InvalidAssetBasket,
    #[msg("Too many assets in basket")]
    TooManyAssets,
    #[msg("Merkle proof missing")]
    MerkleProofMissing,
    #[msg("Merkle proof invalid")]
    MerkleProofInvalid,
    #[msg("Cannot cancel after both deposited")]
    CannotCancelAfterBothDeposited,
    #[msg("Deadline passed")]
    DeadlinePassed,
    #[msg("Oracle stale")]
    OracleStale,
    #[msg("Oracle price band exceeded")]
    OraclePriceBandExceeded,
    #[msg("Replay or stale nonce")]
    ReplayOrStaleNonce,
    #[msg("Proof too large")]
    ProofTooLarge,
    #[msg("Deal qty exceeds limit")]
    DealQtyExceedsLimit,
    #[msg("Deal notional exceeds limit")]
    DealNotionalExceedsLimit,
    #[msg("Version mismatch")]
    VersionMismatch,
    #[msg("Account not rent exempt")]
    AccountNotRentExempt,
    #[msg("Invalid TWAP window")]
    InvalidTwapWindow,
    #[msg("Rotation not yet effective")]
    RotationNotEffectiveYet,
    #[msg("No pending rotation")]
    NoPendingRotation,
}

// ------------------------- Unit tests -------------------------
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_price_band_ok() {
        // small change ok
        assert!(is_price_band_ok(1000, 1100, 2000).is_ok()); // 10% delta, max 20%
        // big change triggers error
        assert!(is_price_band_ok(1000, 2000, 500).is_err()); // 100% change vs 5% cap
    }

    #[test]
    fn test_update_twap_accumulates() {
        let mut m = Market {
            version: 1,
            authority: Pubkey::default(),
            verifier: Pubkey::default(),
            oracle_publisher: Pubkey::default(),
            pending_oracle: Pubkey::default(),
            pending_oracle_effective_ts: 0,
            cft_mint: Pubkey::default(),
            quote_mint: Pubkey::default(),
            insurance_treasury: Pubkey::default(),
            settlement_ts: 0,
            contract_size_kg: 0,
            initial_margin_bps: 0,
            maintenance_margin_bps: 0,
            fee_bps: 0,
            farmer_fee_bps: 0,
            buyer_fee_bps: 0,
            insurance_bps: 0,
            default_margin_call_grace_sec: 0,
            max_notional_per_deal: 0,
            max_qty_per_deal: 0,
            last_price_per_kg: 100,
            prev_price_per_kg: 0,
            last_price_nonce: 0,
            last_oracle_update_ts: 0,
            max_oracle_age_sec: 3600,
            twap_acc: 0,
            twap_time_acc: 0,
            twap_window_sec: 60,
            price_mode: PriceMode::TWAP as u8,
            paused: false,
            min_transfer_amount: 0,
            insurance_treasury_authority: Pubkey::default(),
            program_version: PROGRAM_VERSION,
        };

        // first publish: last_oracle_update_ts is 0 -> sets it only
        let now = 1_700_000_000i64;
        assert!(update_twap(&mut m, now).is_ok());
        assert_eq!(m.twap_acc, 0);
        assert_eq!(m.twap_time_acc, 0);
        // set last_price and simulate later publish with dt
        m.last_price_per_kg = 200;
        let later = now + 10;
        assert!(update_twap(&mut m, later).is_ok());
        assert!(m.twap_acc > 0);
        assert_eq!(m.twap_time_acc, 10u64);
    }

    #[test]
    fn test_rent_is_exempt_behavior() {
        // Rent::default() exists and is_exempt must return false for 0 lamports and true for huge lamports
        let rent = Rent::default();
        assert!(!rent.is_exempt(0, 10));
        assert!(rent.is_exempt(u64::MAX / 4, 10));
    }
}
