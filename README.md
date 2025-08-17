# coffee_futures

# ☕ Coffee Futures Protocol

A decentralized coffee futures trading platform built on **Solana** using the **Anchor framework**.  
This proof-of-concept was built in **Solana Playground** and currently serves as a prototype.  

The protocol enables coffee farmers and buyers to create bilateral futures contracts with both **physical** and **cash settlement** options — providing transparency, price risk management, and on-chain settlement for the global coffee trade.

---

## 🌍 Overview

The Coffee Futures Protocol addresses the critical need for **price risk management** in the coffee industry, which supports over **25 million farmers worldwide**.  
By leveraging blockchain, this protocol creates a **trustless marketplace** for futures trading with built-in safeguards against volatility and settlement disputes.  

---

## 🚀 Key Features

### 🌱 Bilateral Futures Contracts
- Direct agreements between **farmers (short)** and **buyers (long)**  
- Customizable contract terms: `price`, `quantity`, `delivery date`  
- Support for **multi-asset baskets** (different coffee grades)  

### 💰 Dual Settlement Options
- **Cash Settlement**: Uses oracle-based prices or TWAP  
- **Physical Settlement**: Minting of **Coffee Futures Token (CFT)** to represent delivery obligations  

### 🛡️ Risk Management
- Initial & maintenance **margin requirements**  
- Automated **margin calls** with grace periods  
- **Liquidation** handling for undercollateralized positions  
- **Price band checks** to prevent extreme moves  

### 📊 Oracle Integration
- Real-time coffee price feeds  
- **TWAP** calculations for smoothing volatility  
- **Nonce-based replay protection** & staleness checks  

### 🔐 Security Features
- **PDA-based vaults** for escrowed funds  
- Reentrancy protection in all critical flows  
- Merkle proof verification for **physical deliveries**  
- Role rotation with **timelocks** for governance

  ---

  ## 🏗️ Contract Architecture

- **Market** → Defines a harvest period & settlement rules  
- **Deal** → Represents an individual bilateral contract  
- **CFT (Coffee Futures Token)** → Tokenized coffee for physical settlement  
- **Oracle** → Secure market data feed (TWAP + freshness checks)  

---


---
