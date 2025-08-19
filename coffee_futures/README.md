
# 🧱 Program Architecture (`lib.rs`)

## 📂 Accounts (State)

### **Market**
A harvest/contract venue containing:
- Settlement timestamp  
- Margin/fee bps  
- Exposure caps  
- Oracle policy  
- TWAP accumulators  
- Min-transfer (dust)  
- Pause flag  
- Rotation fields  

### **Deal**
A bilateral futures contract with:
- Farmer/buyer keys  
- Agreed price & quantity (kg)  
- Margin state  
- Partial delivery tracking  
- Optional basket (up to `MAX_ASSETS`)  
- Optional Merkle root  
- Deadlines  
- Margin-call fields  
- Flags (settled / settling / liquidated)  

### **CftMintAuth**
PDA that controls the **CFT mint** for physical settlement.  

### **VaultAuth**
PDA authority over the **margin vaults** (ATAs in quote mint) for a given deal.  

---

## 🔑 PDAs & Seeds (Versioned)

All seeds include a version prefix for future-proofing:

- `cft_mint_auth = [b"v1", "cft_auth", cft_mint]`  
- `market = [b"v1", "market", authority, cft_mint, quote_mint]`  
- `deal = [b"v1", "deal", market, farmer, buyer]`  
- `vault_auth = [b"v1", "vault_auth", deal]`  

---

## 💰 Tokens

- **Quote mint** (USDC-like) for margin & settlements.  
- **CFT (Coffee Futures Token)** mint (decimals `3` in PoC) — minted to represent delivered kg in physical settlement.  

---

## 🧭 Instructions 

1. **`init_cft_mint(decimals)`**  
   - Creates the CFT mint & PDA authority.  
   - Rent-exempt checks ✅  
   - Emits `CftMintInitialized`.

2. **`create_market(...)`**  
   - Opens a market (per harvest/spec).  
   - Sets params: margin/fee bps, caps, oracle age, TWAP window, dust, etc.  
   - Emits `MarketCreated`.

3. **`publish_price(price_per_kg, nonce)`**  
   - Oracle publishes a price.  
   - Replay & staleness guards ✅  
   - Price-band guard (±25%) ✅  
   - TWAP accumulator update ✅  
   - Emits `PricePublished`.

4. **`open_deal(...)`**  
   - Creates a bilateral futures deal.  
   - Deposits initial margin from both parties.  
   - Supports baskets, Merkle proofs, vault creation ✅  
   - Emits `DealOpened`.

5. **`top_up_margin(amount)`**  
   - Farmer/buyer adds margin.  
   - Emits `MarginToppedUp`.

6. **`margin_call(grace_sec)`**  
   - Authority sets/updates margin call.  
   - Emits `MarginCalled`.

7. **`mark_to_market()`**  
   - Checks margin vs maintenance.  
   - Flags margin call or liquidation ✅  
   - Emits `MarginCalled / LiquidationFlagged`.

8. **`settle_cash()`**  
   - Settles deal in cash at expiry.  
   - P&L transfer, fees, dust guard ✅  
   - Emits `SettledCash`.

9. **`verify_and_settle_physical(delivered_kg, proof_hashes[], leaf?)`**  
   - Verifies delivery with optional Merkle proof.  
   - Handles partial & full settlement ✅  
   - Emits `SettledPhysical`.

10. **`cancel_deal()`**  
    - Cancelable if margin not deposited or before deadline.  
    - Emits `DealCanceled`.

11. **Role Rotation (Oracle)**  
    - `propose_rotate_oracle(new_oracle, effective_after_ts)`  
    - `activate_rotate_oracle()` (after timelock).  
    - Emits `RoleRotationProposed / RoleRotationActivated`.

12. **`close_deal()`**  
    - Closes a settled deal (rent reclaimed).  

---


## 🛡️ Safety & Correctness (Code-Level)

- Checked math helpers ✅  
- Rent checks on init ✅  
- Reentrancy guard ✅  
- Oracle protections: staleness, band checks, replay ✅  
- TWAP accumulators ✅  
- PDA signer seeds (vaults & CFT mint) ✅  
- Versioned seeds (`b"v1"`) ✅  
- Events for all ops ✅  
- Clear error codes ✅  

⚠️ **PoC Limitations**  
- Insurance treasury draw blocked (returns Unauthorized).  
- Oracle/verifier multisig checks are stubbed.  
- TWAP uses compact accumulator (not ring buffer).  
- Fee treasuries not PDA-secured.  
- ATA owner checks could be stricter.  

---

## 📈 Future Enhancements (Roadmap)

### 🔒 Safety & Auth
- Ed25519 oracle sig verification (no trusted signers).  
- Per-instruction version guard + allowlist.  
- CPI guardrails (SPL Token / Token-2022 / ed25519 only).  

### 💰 Funds & Treasuries
- Dedicated PDA treasuries with governance-controlled withdrawals.  
- Multi-treasury splits (protocol / insurance / referrer).  

### 📊 Price & Risk
- Exact TWAP ring buffer.  
- Expiry median fix.  
- Circuit breaker on extreme moves.  

### 📜 Lifecycle & Margin
- Maker–taker margin tracking.  
- Liquidation hook + bounty for bots.  

### ⚖️ Rounding & Units
- Per-asset decimals.  
- Deterministic rounding policy.  

### ⚙️ Compute & Rent
- ComputeBudget hints.  
- State size optimizations.  
- Optional zero-copy Market struct.  

### 🏛 Governance & Ops
- Timelocked parameter updates.  
- Granular pause flags.  

### 🧑‍💻 Access & UX
- Strict ATA checks.  
- Rich events with reason codes.  

### 🧪 Testing
- Golden-path integration tests.  
- Fuzz Merkle proofs.  
- Deterministic TWAP rollover tests.  

### 🔧 DevEx & Product
- Cargo feature flags.  
- IDL docs & TS examples.  
- Cross-collateral margin, batch settlement, insurance fund.  
- On-chain dispute DAO, Chainlink oracle integration.  
- NFT coffee certificates.  
- UI dApp for non-technical users.

___




                         ☕ COFFEE FUTURES PROTOCOL (Anchor / Solana)

ACTORS
───────
  Farmer (short)                  Buyer (long)                    Oracle Publisher         Verifier (Physical)
      │                                │                                   │                          │
      │                                │                                   │                          │
      └───────────────┬────────────────┴───────────────────────────────────┴───────────────┬───────────┘
                      │                                                               
                      ▼                                                               

ON-CHAIN ACCOUNTS (STATE)
─────────────────────────
  [Program ID: Coffee1111...]

  ┌───────────────────────────────────────────────────────────────────────────────────────────┐
  │ Market (PDA)                                                                               │
  │  - authority, verifier, oracle_publisher                                                   │
  │  - cft_mint, quote_mint                                                                    │
  │  - settlement_ts, contract_size_kg                                                         │
  │  - initial_margin_bps, maintenance_margin_bps, fee_bps, farmer_fee_bps, buyer_fee_bps     │
  │  - insurance_bps, insurance_treasury (ATA), min_transfer_amount (dust)                    │
  │  - last_price_per_kg, prev_price_per_kg, max_oracle_age_sec                                │
  │  - TWAP accumulators (twap_acc, twap_time_acc, twap_window_sec)                            │
  │  - paused, price_mode, last_price_nonce                                                    │
  │  - rotation: pending_oracle, pending_oracle_effective_ts                                   │
  │  - exposure caps: max_notional_per_deal, max_qty_per_deal                                  │
  │  - program_version                                                                         │
  └───────────────────────────────────────────────────────────────────────────────────────────┘
               ▲
               │ seeds: [b"v1","market", authority, cft_mint, quote_mint]
               │

  ┌───────────────────────────────────────────────────────────────────────────────────────────┐
  │ Deal (PDA)                                                                                 │
  │  - market, farmer, buyer                                                                   │
  │  - agreed_price_per_kg, quantity_kg, initial_margin_each                                   │
  │  - physical_delivery (bool), delivered_kg_total                                            │
  │  - deadline_ts, margin_call_ts, margin_call_grace_sec                                      │
  │  - flags: settled, settling (reentrancy), liquidated                                       │
  │  - farmer_deposited, buyer_deposited                                                       │
  │  - basket: assets[MAX_ASSETS], asset_qty[MAX_ASSETS], asset_count                          │
  │  - merkle_root                                                                             │
  │  - referral: referrer, fee_split_bps                                                       │
  └───────────────────────────────────────────────────────────────────────────────────────────┘
               ▲
               │ seeds: [b"v1","deal", market, farmer, buyer]
               │
  ┌─────────────────────────┐         ┌─────────────────────────┐
  │ VaultAuth (PDA)         │         │ CftMintAuth (PDA)       │
  │  - bump                 │         │  - bump                 │
  └─────────────────────────┘         └─────────────────────────┘
      ▲ seeds: [b"v1",                  ▲ seeds: [b"v1",
      │         "vault_auth", deal]     │         "cft_auth", cft_mint]

MINTS & VAULTS (TOKENS)
───────────────────────
  ┌──────────────────────┐   ┌──────────────────────┐
  │ Quote Mint (e.g USDC)│   │ CFT Mint (coffee kg) │  ← init_cft_mint()
  └──────────────────────┘   └──────────────────────┘     authority = CftMintAuth PDA

  ┌──────────────────────────────┐         ┌──────────────────────────────┐
  │ Farmer Margin Vault (ATA)    │         │ Buyer Margin Vault (ATA)     │
  │ mint = Quote, owner=VaultAuth│         │ mint = Quote, owner=VaultAuth│
  └──────────────────────────────┘         └──────────────────────────────┘

  ┌──────────────────────────────┐         ┌──────────────────────────────┐
  │ Farmer Receive ATA (Quote)   │         │ Buyer Receive ATA (Quote)    │
  └──────────────────────────────┘         └──────────────────────────────┘

  ┌──────────────────────────────┐
  │ Fee Treasury ATA (Quote)     │
  └──────────────────────────────┘
  ┌──────────────────────────────┐
  │ Insurance Treasury ATA (Quote│
  └──────────────────────────────┘


LIFECYCLE (MAIN FLOWS)
──────────────────────
 1) init_cft_mint(decimals)
    - Create CFT mint & CftMintAuth PDA (mint authority), rent checks.

 2) create_market(params…)
    - Configure margins, fees, oracle rules, caps, TWAP, dust, rotation fields.

 3) publish_price(price, nonce)
    - Checks: nonce ↑ strictly, staleness (now - last_update <= max_age),
      price-band vs prev (±25% demo), update TWAP accumulators.
    - Records: last_price, prev_price, last_oracle_update_ts, last_price_nonce.

 4) open_deal(agreed_price, qty, physical, deadline, assets[], asset_qty[], merkle_root?)
    - Creates Deal PDA + VaultAuth PDA.
    - Creates margin vault ATAs owned by VaultAuth.
    - Transfers initial margin from farmer/buyer into their vaults.
    - Enforces exposure caps and basket limits.

 5) top_up_margin(amount)
    - Either side adds margin to respective vault (authority=signer).

 6) margin_call(grace_sec)  (Authority only)
    - Sets/updates margin call with grace; liquidation path enabled after grace.

 7) mark_to_market()
    - Chooses price mode (last or TWAP).
    - If under maintenance margin → set margin_call_ts or flag liquidation after grace.

 8) settle_cash()
    - Allowed at/after market.settlement_ts (or after deal.deadline_ts fallback).
    - Reentrancy guard (settling=true).
    - Computes PnL, collects fees (protocol/farmer/buyer/insurance slices),
      pays winner from loser vault; returns residuals above dust.
    - Marks settled.

 9) verify_and_settle_physical(delivered_kg, proof_hashes[], leaf?)
    - Optional Merkle verification against deal.merkle_root.
    - Partial deliveries accumulate and mint CFT to buyer via CftMintAuth PDA.
    - Pays farmer: agreed_price * delivered_kg from buyer margin vault.
    - On full delivery: return residuals; mark settled.

10) cancel_deal()
    - If not both deposited OR before deadline, refund any margin, mark settled.

11) role rotation (oracle)
    - propose_rotate_oracle(new_pubkey, ts) → activate after timelock.

12) close_deal()
    - Close Deal account (rent) only when settled.


PRICE PATH & TWAP (SIMPLIFIED)
───────────────────────────────
  publish_price:
    ┌─────────────────────────────────────────────────────────────────────┐
    │ prev_price ← last_price                                             │
    │ last_price ← new_price                                              │
    │ dt = now - last_oracle_update_ts                                    │
    │ twap_acc += last_price * min(dt, twap_window_sec)                   │
    │ twap_time_acc += min(dt, twap_window_sec)                           │
    │ if twap_time_acc > twap_window_sec: compress window proportionally  │
    │ last_oracle_update_ts ← now                                         │
    └─────────────────────────────────────────────────────────────────────┘
  settle / mtm:
    price_used = (price_mode == LAST ? last_price : twap_acc / twap_time_acc)


SECURITY GUARDS (IN-CODE)
─────────────────────────
  • Checked arithmetic (overflow-safe helpers)
  • PDA signer seeds for transfers (VaultAuth, CftMintAuth)
  • Reentrancy guard (`settling` flag)
  • Staleness, price-band, and nonce replay checks for oracle updates
  • Versioned PDA seeds prefix: b"v1"
  • Rent checks on init; explicit dust threshold for residual returns
  • Rich events: MarketCreated, PricePublished, DealOpened, MarginToppedUp,
                 MarginCalled, LiquidationFlagged, SettledCash, SettledPhysical, DealCanceled


SEED MAP (for quick reference)
──────────────────────────────
  market      = [b"v1", "market", authority, cft_mint, quote_mint]
  deal        = [b"v1", "deal", market, farmer, buyer]
  vault_auth  = [b"v1", "vault_auth", deal]
  cft_auth    = [b"v1", "cft_auth", cft_mint]



___
