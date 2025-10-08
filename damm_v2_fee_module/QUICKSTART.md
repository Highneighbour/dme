# Quick Start Guide

## 🚀 Get Started in 5 Minutes

### Prerequisites Check
```bash
anchor --version  # Should be 0.30.1+
solana --version  # Should be 1.18.0+
node --version    # Should be 16+
```

### 1. Build
```bash
cd damm_v2_fee_module
anchor build
```

### 2. Test
```bash
anchor test
```

### 3. Deploy to Devnet
```bash
solana config set --url devnet
anchor deploy
```

## 📝 Basic Usage

### Initialize Honorary Position
```typescript
import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { DammV2FeeModule } from "./target/types/damm_v2_fee_module";

const program = anchor.workspace.DammV2FeeModule as Program<DammV2FeeModule>;

// Step 1: Initialize honorary position
const vaultId = new anchor.BN(1);
const tx1 = await program.methods
  .initializeHonoraryPosition(
    vaultId,
    5000,                    // 50% to investors
    null,                    // No daily cap
    new anchor.BN(1000),     // 0.001 min payout
    new anchor.BN(1_000_000_000) // 1B total allocation
  )
  .accounts({
    payer: wallet.publicKey,
    pool: poolPubkey,
    poolTokenA: poolTokenA,
    poolTokenB: poolTokenB,
    quoteMint: quoteMint,
    positionOwner: positionOwnerPDA,
    honoraryPosition: honoraryPositionPDA,
    policy: policyPDA,
    creatorQuoteAta: creatorAta,
    systemProgram: SystemProgram.programId,
  })
  .rpc();
```

### Initialize Distribution
```typescript
// Step 2: Initialize distribution accounts
const tx2 = await program.methods
  .initializeDistribution(vaultId)
  .accounts({
    payer: wallet.publicKey,
    honoraryPosition: honoraryPositionPDA,
    quoteMint: quoteMint,
    baseMint: baseMint,
    positionOwner: positionOwnerPDA,
    progress: progressPDA,
    programQuoteTreasury: quoteTreasuryPDA,
    programBaseTreasury: baseTreasuryPDA,
    tokenProgram: TOKEN_PROGRAM_ID,
    systemProgram: SystemProgram.programId,
  })
  .rpc();
```

### Run Distribution Crank
```typescript
// Step 3: Distribute fees (permissionless, anyone can call)
const investorPage = [
  {
    quoteAta: investor1Ata,
    lockedAmount: new anchor.BN(300_000_000), // 300M locked
  },
  {
    quoteAta: investor2Ata,
    lockedAmount: new anchor.BN(200_000_000), // 200M locked
  },
];

const tx3 = await program.methods
  .crankDistribute(vaultId, investorPage, true) // true = final page
  .accounts({
    cranker: wallet.publicKey,
    cpAmmProgram: METEORA_PROGRAM_ID,
    pool: poolPubkey,
    poolTokenA: poolTokenA,
    poolTokenB: poolTokenB,
    honoraryPosition: honoraryPositionPDA,
    positionOwner: positionOwnerPDA,
    honoraryPositionAccount: damm_v2_position,
    policy: policyPDA,
    progress: progressPDA,
    programQuoteTreasury: quoteTreasuryPDA,
    programBaseTreasury: baseTreasuryPDA,
    creatorQuoteAta: creatorAta,
    streamflowProgram: STREAMFLOW_PROGRAM_ID,
    tokenProgram: TOKEN_PROGRAM_ID,
    systemProgram: SystemProgram.programId,
  })
  .remainingAccounts([
    { pubkey: investor1Ata, isWritable: true, isSigner: false },
    { pubkey: investor2Ata, isWritable: true, isSigner: false },
  ])
  .rpc();
```

## 🔑 PDA Derivation Helper

```typescript
import { PublicKey } from "@solana/web3.js";

function derivePDAs(programId: PublicKey, vaultId: number) {
  const vaultIdBuffer = Buffer.alloc(8);
  vaultIdBuffer.writeBigUInt64LE(BigInt(vaultId));

  const [positionOwner] = PublicKey.findProgramAddressSync(
    [Buffer.from("investor_fee_pos_owner"), vaultIdBuffer],
    programId
  );

  const [honoraryPosition] = PublicKey.findProgramAddressSync(
    [Buffer.from("honorary_position"), vaultIdBuffer],
    programId
  );

  const [policy] = PublicKey.findProgramAddressSync(
    [Buffer.from("policy"), vaultIdBuffer],
    programId
  );

  const [progress] = PublicKey.findProgramAddressSync(
    [Buffer.from("progress"), vaultIdBuffer],
    programId
  );

  const [quoteTreasury] = PublicKey.findProgramAddressSync(
    [Buffer.from("quote_treasury"), vaultIdBuffer],
    programId
  );

  const [baseTreasury] = PublicKey.findProgramAddressSync(
    [Buffer.from("base_treasury"), vaultIdBuffer],
    programId
  );

  return {
    positionOwner,
    honoraryPosition,
    policy,
    progress,
    quoteTreasury,
    baseTreasury,
  };
}
```

## 📊 Check Distribution Status

```typescript
// Fetch progress
const progress = await program.account.progress.fetch(progressPDA);
console.log("Last distribution:", new Date(progress.lastDistributionTs * 1000));
console.log("Day state:", progress.dayState);
console.log("Today's spent:", progress.currentDaySpent.toString());
console.log("Dust carry:", progress.dustCarry.toString());

// Fetch policy
const policy = await program.account.policy.fetch(policyPDA);
console.log("Investor share:", policy.investorFeeShareBps / 100, "%");
console.log("Daily cap:", policy.dailyCapLamports?.toString() || "None");
console.log("Min payout:", policy.minPayoutLamports.toString());
```

## 🎯 Common Patterns

### Multi-Page Distribution
```typescript
async function distributeAllInvestors(investors: InvestorData[]) {
  const PAGE_SIZE = 10;
  const pages = chunk(investors, PAGE_SIZE);

  for (let i = 0; i < pages.length; i++) {
    const isLastPage = i === pages.length - 1;
    
    await program.methods
      .crankDistribute(vaultId, pages[i], isLastPage)
      .accounts({ /* ... */ })
      .remainingAccounts(
        pages[i].map(inv => ({
          pubkey: inv.quoteAta,
          isWritable: true,
          isSigner: false,
        }))
      )
      .rpc();
    
    console.log(`Processed page ${i + 1}/${pages.length}`);
  }
}
```

### Event Listening
```typescript
// Listen for distribution events
program.addEventListener("InvestorPayoutPage", (event) => {
  console.log("Page distributed:");
  console.log("- Vault ID:", event.vaultId.toString());
  console.log("- Total distributed:", event.totalDistributed.toString());
  console.log("- Dust carry:", event.dustCarry.toString());
});

program.addEventListener("CreatorPayoutDayClosed", (event) => {
  console.log("Day closed:");
  console.log("- Creator payout:", event.creatorPayout.toString());
  console.log("- Total investor payout:", event.totalInvestorPayout.toString());
});
```

### Error Handling
```typescript
try {
  await program.methods
    .crankDistribute(vaultId, investors, true)
    .accounts({ /* ... */ })
    .rpc();
} catch (err) {
  if (err.toString().includes("BaseFeeDetected")) {
    console.error("❌ Base fees detected - position not quote-only!");
  } else if (err.toString().includes("AlreadyDistributedToday")) {
    console.log("✅ Already distributed today, try tomorrow");
  } else {
    console.error("Error:", err);
  }
}
```

## 🔄 Daily Cron Setup

```typescript
// Example: Automated daily distribution
import cron from "node-cron";

// Run every day at 00:00 UTC
cron.schedule("0 0 * * *", async () => {
  console.log("Running daily distribution...");
  
  // Fetch investors from Streamflow
  const investors = await fetchStreamflowInvestors(vaultId);
  
  // Filter only investors with locked tokens
  const lockedInvestors = investors.filter(inv => inv.lockedAmount > 0);
  
  if (lockedInvestors.length === 0) {
    console.log("No locked investors, skipping...");
    return;
  }
  
  // Distribute in pages
  await distributeAllInvestors(lockedInvestors);
  
  console.log("✅ Distribution complete!");
});
```

## 📦 Package Integration

```typescript
// Add to your package.json
{
  "dependencies": {
    "@coral-xyz/anchor": "^0.30.1",
    "@solana/web3.js": "^1.95.0",
    "@solana/spl-token": "^0.4.0"
  }
}
```

## 🛠️ Troubleshooting

### Issue: Account not initialized
```typescript
// Solution: Initialize in correct order
await initializeHonoraryPosition();  // Step 1
await initializeDistribution();      // Step 2
await crankDistribute();             // Step 3
```

### Issue: Compute budget exceeded
```typescript
import { ComputeBudgetProgram } from "@solana/web3.js";

const computeIx = ComputeBudgetProgram.setComputeUnitLimit({
  units: 400_000
});

const tx = new Transaction()
  .add(computeIx)
  .add(/* your instruction */);
```

### Issue: 24h gate not passed
```typescript
// Check last distribution time
const progress = await program.account.progress.fetch(progressPDA);
const nextAllowedTime = progress.lastDistributionTs + 86400;
const now = Math.floor(Date.now() / 1000);

if (now < nextAllowedTime) {
  const waitSeconds = nextAllowedTime - now;
  console.log(`Wait ${waitSeconds} seconds before next distribution`);
}
```

## 🎓 Next Steps

1. Read [README.md](./README.md) for detailed documentation
2. Check [BUILD_AND_DEPLOYMENT.md](./BUILD_AND_DEPLOYMENT.md) for deployment guide
3. Review [IMPLEMENTATION_SUMMARY.md](./IMPLEMENTATION_SUMMARY.md) for architecture details
4. Run tests: `anchor test`
5. Deploy to devnet and test live

## 📚 Resources

- [Anchor Documentation](https://www.anchor-lang.com/)
- [Solana Cookbook](https://solanacookbook.com/)
- [DAMM v2 GitHub](https://github.com/MeteoraAg/damm-v2)
- [Streamflow Docs](https://docs.streamflow.finance/)

---

**Happy Building! 🚀**

For issues or questions, please refer to the main README or open an issue.
