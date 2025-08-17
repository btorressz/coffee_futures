
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
