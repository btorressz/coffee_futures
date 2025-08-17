
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
