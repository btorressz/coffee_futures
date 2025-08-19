
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






___
