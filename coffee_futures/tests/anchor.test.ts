// No imports needed: web3, anchor, pg, BN, assert are globally available

describe("Coffee Futures – happy path", () => {
  it("init -> create_market -> publish_price -> open_deal -> settle_cash -> close", async () => {
    // ---------- helpers ----------
    const SEED_PREFIX = Buffer.from("v1");
    const enc = (s) => Buffer.from(s);

    // Try to find SPL-Token helpers regardless of how the playground exposes them
    const spl =
      (globalThis as any).spl ||
      (globalThis as any).splToken ||
      (anchor as any).spl ||
      (anchor as any).utils?.token;

    if (!spl || !spl.createMint || !spl.getOrCreateAssociatedTokenAccount) {
      throw new Error(
        "SPL-Token helpers not found in this playground environment. Please use a template that exposes `spl` or `splToken` globals."
      );
    }

    const ASSOCIATED_TOKEN_PROGRAM_ID =
      spl.ASSOCIATED_TOKEN_PROGRAM_ID ||
      // fallback to canonical ID
      new web3.PublicKey("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");

    const findPda = (seeds) =>
      web3.PublicKey.findProgramAddressSync(seeds, pg.program.programId)[0];

    const airdrop = async (pubkey, sol = 2) => {
      const sig = await pg.connection.requestAirdrop(
        pubkey,
        web3.LAMPORTS_PER_SOL * sol
      );
      await pg.connection.confirmTransaction(sig, "confirmed");
    };

    // Some SPL helpers require a `Keypair` payer. On many playgrounds
    // `pg.wallet.payer` exists; otherwise we fallback to `pg.wallet` (Signer).
    const getPayer = () =>
      (pg.wallet as any).payer ||
      (pg.wallet as any).keypair ||
      pg.wallet; // last resort (Signer)

    // ---------- signers / actors ----------
    const authority = pg.wallet; // program authority for market
    const oracleKp = new web3.Keypair();
    const verifierKp = new web3.Keypair();
    const farmerKp = new web3.Keypair();
    const buyerKp = new web3.Keypair();

    await Promise.all([
      airdrop(oracleKp.publicKey),
      airdrop(verifierKp.publicKey),
      airdrop(farmerKp.publicKey),
      airdrop(buyerKp.publicKey),
    ]);

    // ---------- create quote mint (USDC-like, 6 decimals) ----------
    const quoteMint = await spl.createMint(
      pg.connection,
      getPayer(),
      pg.wallet.publicKey, // mint authority
      pg.wallet.publicKey, // freeze authority
      6
    );

    // ATAs
    const farmerQuoteAta = await spl.getOrCreateAssociatedTokenAccount(
      pg.connection,
      getPayer(),
      quoteMint,
      farmerKp.publicKey
    );
    const buyerQuoteAta = await spl.getOrCreateAssociatedTokenAccount(
      pg.connection,
      getPayer(),
      quoteMint,
      buyerKp.publicKey
    );
    const feeTreasuryAta = await spl.getOrCreateAssociatedTokenAccount(
      pg.connection,
      getPayer(),
      quoteMint,
      authority.publicKey
    );
    const insuranceTreasuryAta = await spl.getOrCreateAssociatedTokenAccount(
      pg.connection,
      getPayer(),
      quoteMint,
      authority.publicKey
    );

    // Mint margin funds (use plain numbers to avoid BigInt literal errors)
    await spl.mintTo(
      pg.connection,
      getPayer(),
      quoteMint,
      farmerQuoteAta.address,
      pg.wallet.publicKey,
      1_000_000_000 // 1,000 USDC if 6 decimals
    );
    await spl.mintTo(
      pg.connection,
      getPayer(),
      quoteMint,
      buyerQuoteAta.address,
      pg.wallet.publicKey,
      1_000_000_000
    );

    // ---------- init CFT mint via program ----------
    const cftMintKp = new web3.Keypair();
    const cftAuthPda = findPda([
      SEED_PREFIX,
      enc("cft_auth"),
      cftMintKp.publicKey.toBuffer(),
    ]);

    await pg.program.methods
      .initCftMint(3)
      .accounts({
        payer: authority.publicKey,
        cftMint: cftMintKp.publicKey,
        cftMintAuth: cftAuthPda,
        tokenProgram: spl.TOKEN_PROGRAM_ID,
        systemProgram: web3.SystemProgram.programId,
        rent: web3.SYSVAR_RENT_PUBKEY,
      })
      .signers([cftMintKp])
      .rpc();

    // ---------- create market ----------
    const marketPda = findPda([
      SEED_PREFIX,
      enc("market"),
      authority.publicKey.toBuffer(),
      cftMintKp.publicKey.toBuffer(),
      quoteMint.toBuffer(),
    ]);

    const now = Math.floor(Date.now() / 1000);
    const settlementTs = new BN(now + 30); // settle soon for test
    const contractSizeKg = new BN(1);
    const initialMarginBps = 1000;
    const maintenanceMarginBps = 500;
    const feeBps = 50;
    const farmerFeeBps = 25;
    const buyerFeeBps = 25;
    const maxNotionalPerDeal = new BN(10_000_000_000);
    const maxQtyPerDeal = new BN(10_000);
    const maxOracleAgeSec = new BN(3600);
    const twapWindowSec = new BN(60);
    const insuranceBps = 100;
    const minTransferAmount = new BN(0);

    await pg.program.methods
      .createMarket(
        settlementTs,
        contractSizeKg,
        initialMarginBps,
        maintenanceMarginBps,
        feeBps,
        farmerFeeBps,
        buyerFeeBps,
        maxNotionalPerDeal,
        maxQtyPerDeal,
        maxOracleAgeSec,
        twapWindowSec,
        insuranceBps,
        minTransferAmount
      )
      .accounts({
        authority: authority.publicKey,
        verifier: verifierKp.publicKey,
        oraclePublisher: oracleKp.publicKey,
        cftMint: cftMintKp.publicKey,
        quoteMint,
        insuranceTreasury: insuranceTreasuryAta.address,
        market: marketPda,
        systemProgram: web3.SystemProgram.programId,
        rent: web3.SYSVAR_RENT_PUBKEY,
      })
      .rpc();

    // ---------- publish price ----------
    const pricePerKg = new BN(1_500);
    await pg.program.methods
      .publishPrice(pricePerKg, new BN(1))
      .accounts({
        market: marketPda,
        oraclePublisher: oracleKp.publicKey,
      })
      .signers([oracleKp])
      .rpc();

    // ---------- open deal ----------
    const dealPda = findPda([
      SEED_PREFIX,
      enc("deal"),
      marketPda.toBuffer(),
      farmerKp.publicKey.toBuffer(),
      buyerKp.publicKey.toBuffer(),
    ]);
    const vaultAuthPda = findPda([
      SEED_PREFIX,
      enc("vault_auth"),
      dealPda.toBuffer(),
    ]);

    const vaultFarmerAta = await spl.getAssociatedTokenAddress(
      quoteMint,
      vaultAuthPda,
      true,
      spl.TOKEN_PROGRAM_ID,
      ASSOCIATED_TOKEN_PROGRAM_ID
    );
    const vaultBuyerAta = await spl.getAssociatedTokenAddress(
      quoteMint,
      vaultAuthPda,
      true,
      spl.TOKEN_PROGRAM_ID,
      ASSOCIATED_TOKEN_PROGRAM_ID
    );

    const agreedPricePerKg = new BN(1_500);
    const quantityKg = new BN(10);
    const physicalDelivery = false;
    const deadlineTs = new BN(now + 300);

    await pg.program.methods
      .openDeal(
        agreedPricePerKg,
        quantityKg,
        physicalDelivery,
        deadlineTs,
        [],      // assets
        [],      // asset_qty
        null,    // merkle_root
        null,    // referrer
        null     // fee_split_bps
      )
      .accounts({
        farmer: farmerKp.publicKey,
        buyer: buyerKp.publicKey,
        market: marketPda,
        quoteMint,
        deal: dealPda,
        vaultAuth: vaultAuthPda,
        farmerMarginVault: vaultFarmerAta,
        buyerMarginVault: vaultBuyerAta,
        farmerMarginFrom: farmerQuoteAta.address,
        buyerMarginFrom: buyerQuoteAta.address,
        tokenProgram: spl.TOKEN_PROGRAM_ID,
        associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
        systemProgram: web3.SystemProgram.programId,
        rent: web3.SYSVAR_RENT_PUBKEY,
      })
      .signers([farmerKp, buyerKp])
      .rpc();

    // push mark up so buyer wins
    await pg.program.methods
      .publishPrice(new BN(1_800), new BN(2))
      .accounts({ market: marketPda, oraclePublisher: oracleKp.publicKey })
      .signers([oracleKp])
      .rpc();

    // ---------- settle cash ----------
    await pg.program.methods
      .settleCash()
      .accounts({
        market: marketPda,
        deal: dealPda,
        vaultAuth: vaultAuthPda,
        farmerMarginVault: vaultFarmerAta,
        buyerMarginVault: vaultBuyerAta,
        farmerReceive: farmerQuoteAta.address,
        buyerReceive: buyerQuoteAta.address,
        feeTreasury: feeTreasuryAta.address,
        insuranceTreasury: insuranceTreasuryAta.address,
        insuranceTreasuryAuthority: authority.publicKey,
        tokenProgram: spl.TOKEN_PROGRAM_ID,
      })
      .rpc();

    // ---------- close deal ----------
    await pg.program.methods
      .closeDeal()
      .accounts({
        deal: dealPda,
        market: marketPda,
        receiver: authority.publicKey,
      })
      .rpc();

    // ---------- assertions ----------
    const marketAcct = await pg.program.account.market.fetch(marketPda);
    assert.ok(marketAcct.cftMint.equals(cftMintKp.publicKey));
    assert.equal(marketAcct.programVersion, 1);

    const buyerBal = await pg.connection.getTokenAccountBalance(
      buyerQuoteAta.address
    );
    const farmerBal = await pg.connection.getTokenAccountBalance(
      farmerQuoteAta.address
    );
    // sanity: someone should have tokens, and balances are strings -> Number-safe here
    assert.ok(
      Number(buyerBal.value.amount) > 0 || Number(farmerBal.value.amount) > 0
    );

    console.log(
      "OK. Market:",
      marketPda.toBase58(),
      "Deal:",
      dealPda.toBase58()
    );
  });
});
