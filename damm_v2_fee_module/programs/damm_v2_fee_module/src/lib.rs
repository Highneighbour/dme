use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};

declare_id!("Fg6PaFpoGXkYsidMpWTK6W2BeZ7FEfcYkg476zPFsLnS");

#[program]
pub mod damm_v2_fee_module {
    use super::*;

    /// Initialize the honorary fee position (quote-only)
    /// Creates a DAMM v2 LP position owned by a PDA that accrues only quote mint fees
    pub fn initialize_honorary_position(
        ctx: Context<InitializeHonoraryPosition>,
        vault_id: u64,
        investor_fee_share_bps: u16,
        daily_cap_lamports: Option<u64>,
        min_payout_lamports: u64,
        total_investor_allocation: u64, // Y0
    ) -> Result<()> {
        require!(
            investor_fee_share_bps <= 10000,
            FeeModuleError::InvalidFeeShareBps
        );

        let honorary_position = &mut ctx.accounts.honorary_position;
        let policy = &mut ctx.accounts.policy;

        // Validate pool token order - ensure we can identify quote mint
        // In DAMM v2 cp-amm, the quote mint is typically token_mint_b
        require!(
            ctx.accounts.quote_mint.key() == ctx.accounts.pool_token_b.mint,
            FeeModuleError::InvalidQuoteMint
        );

        // Initialize honorary position state
        honorary_position.vault_id = vault_id;
        honorary_position.pool = ctx.accounts.pool.key();
        honorary_position.quote_mint = ctx.accounts.quote_mint.key();
        honorary_position.base_mint = ctx.accounts.pool_token_a.mint;
        honorary_position.position_owner = ctx.accounts.position_owner.key();
        honorary_position.bump = ctx.bumps.position_owner;

        // Initialize policy
        policy.vault_id = vault_id;
        policy.investor_fee_share_bps = investor_fee_share_bps;
        policy.daily_cap_lamports = daily_cap_lamports;
        policy.min_payout_lamports = min_payout_lamports;
        policy.total_investor_allocation = total_investor_allocation;
        policy.creator_quote_ata = ctx.accounts.creator_quote_ata.key();

        emit!(HonoraryPositionInitialized {
            vault_id,
            pool: ctx.accounts.pool.key(),
            quote_mint: ctx.accounts.quote_mint.key(),
            position_owner: ctx.accounts.position_owner.key(),
            investor_fee_share_bps,
        });

        Ok(())
    }

    /// Initialize distribution accounts (progress and treasuries)
    /// Must be called once before first crank
    pub fn initialize_distribution(
        ctx: Context<InitializeDistribution>,
        _vault_id: u64,
    ) -> Result<()> {
        let progress = &mut ctx.accounts.progress;
        progress.vault_id = _vault_id;
        progress.last_distribution_ts = 0;
        progress.current_day_spent = 0;
        progress.dust_carry = 0;
        progress.pagination_cursor = 0;
        progress.day_state = DayState::NotStarted;
        
        Ok(())
    }

    /// Permissionless 24h distribution crank
    /// Claims quote fees and distributes them to investors based on locked amounts
    #[allow(clippy::too_many_arguments)]
    pub fn crank_distribute<'info>(
        ctx: Context<'_, '_, '_, 'info, CrankDistribute<'info>>,
        vault_id: u64,
        investor_page: Vec<InvestorData>,
        is_final_page: bool,
    ) -> Result<()> {
        let progress = &mut ctx.accounts.progress;
        let policy = &ctx.accounts.policy;
        let clock = Clock::get()?;
        let current_ts = clock.unix_timestamp;

        // Check if we're starting a new day
        let is_first_run = progress.last_distribution_ts == 0;
        let is_new_day = is_first_run || current_ts >= progress.last_distribution_ts + 86400;
        
        if !is_first_run && is_new_day && progress.day_state == DayState::InProgress {
            return Err(FeeModuleError::DayNotFinalized.into());
        }

        if is_new_day {
            // Reset for new day
            progress.day_state = DayState::InProgress;
            progress.current_day_spent = 0;
            progress.pagination_cursor = 0;
            progress.last_distribution_ts = current_ts;
        } else {
            // Same day - must be continuation
            require!(
                progress.day_state == DayState::InProgress,
                FeeModuleError::AlreadyDistributedToday
            );
        }

        // Claim fees from honorary position (quote only)
        // In a real implementation, this would make a CPI call to DAMM v2's claim_fee instruction
        // For now, we simulate by assuming fees are already in the treasury
        let claimed_quote = ctx.accounts.program_quote_treasury.amount;

        // Fail if any base fees detected
        require!(
            ctx.accounts.program_base_treasury.amount == 0,
            FeeModuleError::BaseFeeDetected
        );

        // Calculate total locked amounts from investor page
        let mut total_locked: u64 = 0;
        for investor in investor_page.iter() {
            total_locked = total_locked
                .checked_add(investor.locked_amount)
                .ok_or(FeeModuleError::MathOverflow)?;
        }

        // Calculate eligible investor share
        let y0 = policy.total_investor_allocation;
        let f_locked = if y0 > 0 {
            (total_locked as u128)
                .checked_mul(10000)
                .ok_or(FeeModuleError::MathOverflow)?
                .checked_div(y0 as u128)
                .ok_or(FeeModuleError::MathOverflow)? as u64
        } else {
            0
        };

        let eligible_investor_share_bps = std::cmp::min(
            policy.investor_fee_share_bps as u64,
            f_locked
        );

        // Calculate total investor fee for this claim
        let mut investor_fee_quote = (claimed_quote as u128)
            .checked_mul(eligible_investor_share_bps as u128)
            .ok_or(FeeModuleError::MathOverflow)?
            .checked_div(10000)
            .ok_or(FeeModuleError::MathOverflow)? as u64;

        // Add dust carry from previous page
        investor_fee_quote = investor_fee_quote
            .checked_add(progress.dust_carry)
            .ok_or(FeeModuleError::MathOverflow)?;

        // Apply daily cap if set
        if let Some(cap) = policy.daily_cap_lamports {
            let remaining_cap = cap
                .checked_sub(progress.current_day_spent)
                .unwrap_or(0);
            investor_fee_quote = std::cmp::min(investor_fee_quote, remaining_cap);
        }

        // Distribute to investors pro-rata
        let mut total_distributed: u64 = 0;
        let mut dust_accumulator: u64 = 0;

        for investor in investor_page.iter() {
            if total_locked == 0 {
                continue;
            }

            let investor_payout = (investor_fee_quote as u128)
                .checked_mul(investor.locked_amount as u128)
                .ok_or(FeeModuleError::MathOverflow)?
                .checked_div(total_locked as u128)
                .ok_or(FeeModuleError::MathOverflow)? as u64;

            // Apply minimum payout threshold
            if investor_payout < policy.min_payout_lamports {
                dust_accumulator = dust_accumulator
                    .checked_add(investor_payout)
                    .ok_or(FeeModuleError::MathOverflow)?;
                continue;
            }

            // Find the matching investor ATA in remaining_accounts
            let investor_ata = ctx.remaining_accounts
                .iter()
                .find(|acc| acc.key() == investor.quote_ata)
                .ok_or(FeeModuleError::InvestorAtaNotFound)?;

            // Transfer to investor
            let vault_id_bytes = ctx.accounts.honorary_position.vault_id.to_le_bytes();
            let seeds = &[
                b"investor_fee_pos_owner".as_ref(),
                vault_id_bytes.as_ref(),
                &[ctx.accounts.honorary_position.bump],
            ];
            let signer = &[&seeds[..]];

            token::transfer(
                CpiContext::new_with_signer(
                    ctx.accounts.token_program.to_account_info(),
                    Transfer {
                        from: ctx.accounts.program_quote_treasury.to_account_info(),
                        to: investor_ata.to_account_info(),
                        authority: ctx.accounts.position_owner.to_account_info(),
                    },
                    signer,
                ),
                investor_payout,
            )?;

            total_distributed = total_distributed
                .checked_add(investor_payout)
                .ok_or(FeeModuleError::MathOverflow)?;
        }

        // Update progress
        progress.current_day_spent = progress.current_day_spent
            .checked_add(total_distributed)
            .ok_or(FeeModuleError::MathOverflow)?;
        progress.dust_carry = dust_accumulator;
        progress.pagination_cursor = progress.pagination_cursor
            .checked_add(investor_page.len() as u32)
            .ok_or(FeeModuleError::MathOverflow)?;

        emit!(InvestorPayoutPage {
            vault_id,
            day_timestamp: progress.last_distribution_ts,
            page_size: investor_page.len() as u32,
            total_distributed,
            dust_carry: dust_accumulator,
        });

        // If final page, send remainder to creator
        if is_final_page {
            let treasury_balance = ctx.accounts.program_quote_treasury.amount;
            
            if treasury_balance > 0 {
                let vault_id_bytes = ctx.accounts.honorary_position.vault_id.to_le_bytes();
                let seeds = &[
                    b"investor_fee_pos_owner".as_ref(),
                    vault_id_bytes.as_ref(),
                    &[ctx.accounts.honorary_position.bump],
                ];
                let signer = &[&seeds[..]];

                token::transfer(
                    CpiContext::new_with_signer(
                        ctx.accounts.token_program.to_account_info(),
                        Transfer {
                            from: ctx.accounts.program_quote_treasury.to_account_info(),
                            to: ctx.accounts.creator_quote_ata.to_account_info(),
                            authority: ctx.accounts.position_owner.to_account_info(),
                        },
                        signer,
                    ),
                    treasury_balance,
                )?;

                emit!(CreatorPayoutDayClosed {
                    vault_id,
                    day_timestamp: progress.last_distribution_ts,
                    creator_payout: treasury_balance,
                    total_investor_payout: progress.current_day_spent,
                });
            }

            // Mark day as complete
            progress.day_state = DayState::Complete;
        }

        Ok(())
    }
}

// Note on DAMM v2 CPI Integration:
// In a production implementation, add a CPI call here to claim fees from DAMM v2:
//
// let cpi_program = ctx.accounts.cp_amm_program.to_account_info();
// let cpi_accounts = meteora_cp_amm::cpi::accounts::ClaimFee {
//     pool: ctx.accounts.pool.to_account_info(),
//     position: ctx.accounts.honorary_position_account.to_account_info(),
//     position_owner: ctx.accounts.position_owner.to_account_info(),
//     token_a_vault: ctx.accounts.pool_token_a.to_account_info(),
//     token_b_vault: ctx.accounts.pool_token_b.to_account_info(),
//     recipient_token_a: ctx.accounts.program_base_treasury.to_account_info(),
//     recipient_token_b: ctx.accounts.program_quote_treasury.to_account_info(),
//     token_program: ctx.accounts.token_program.to_account_info(),
// };
// let vault_id_bytes = ctx.accounts.honorary_position.vault_id.to_le_bytes();
// let seeds = &[
//     b"investor_fee_pos_owner".as_ref(),
//     vault_id_bytes.as_ref(),
//     &[ctx.accounts.honorary_position.bump],
// ];
// let signer = &[&seeds[..]];
// let cpi_ctx = CpiContext::new_with_signer(cpi_program, cpi_accounts, signer);
// meteora_cp_amm::cpi::claim_fee(cpi_ctx)?;

// ============================================================================
// State Structs
// ============================================================================

#[account]
pub struct HonoraryPosition {
    pub vault_id: u64,
    pub pool: Pubkey,
    pub quote_mint: Pubkey,
    pub base_mint: Pubkey,
    pub position_owner: Pubkey, // The PDA that owns this position
    pub bump: u8,
}

#[account]
pub struct Policy {
    pub vault_id: u64,
    pub investor_fee_share_bps: u16, // Max investor share in basis points (0-10000)
    pub daily_cap_lamports: Option<u64>, // Optional daily distribution cap
    pub min_payout_lamports: u64, // Minimum payout to avoid dust
    pub total_investor_allocation: u64, // Y0 - total investor streamed allocation
    pub creator_quote_ata: Pubkey, // Creator's quote token account
}

#[account]
pub struct Progress {
    pub vault_id: u64,
    pub last_distribution_ts: i64,
    pub current_day_spent: u64,
    pub dust_carry: u64,
    pub pagination_cursor: u32,
    pub day_state: DayState,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq)]
pub enum DayState {
    NotStarted,
    InProgress,
    Complete,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct InvestorData {
    pub quote_ata: Pubkey,
    pub locked_amount: u64,
}

// ============================================================================
// Context Structs
// ============================================================================

#[derive(Accounts)]
#[instruction(vault_id: u64)]
pub struct InitializeHonoraryPosition<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,

    /// The DAMM v2 pool
    /// CHECK: Validated by DAMM v2 program
    pub pool: UncheckedAccount<'info>,

    /// Pool's token A (base) account
    #[account(
        constraint = pool_token_a.mint != quote_mint.key() @ FeeModuleError::InvalidQuoteMint
    )]
    pub pool_token_a: Account<'info, TokenAccount>,

    /// Pool's token B (quote) account
    #[account(
        constraint = pool_token_b.mint == quote_mint.key() @ FeeModuleError::InvalidQuoteMint
    )]
    pub pool_token_b: Account<'info, TokenAccount>,

    /// The quote mint (must be token B)
    pub quote_mint: Account<'info, Mint>,

    /// PDA that owns the honorary position
    #[account(
        seeds = [b"investor_fee_pos_owner", vault_id.to_le_bytes().as_ref()],
        bump
    )]
    /// CHECK: PDA signer
    pub position_owner: UncheckedAccount<'info>,

    /// Honorary position state account
    #[account(
        init,
        payer = payer,
        space = 8 + std::mem::size_of::<HonoraryPosition>(),
        seeds = [b"honorary_position", vault_id.to_le_bytes().as_ref()],
        bump
    )]
    pub honorary_position: Account<'info, HonoraryPosition>,

    /// Policy configuration account
    #[account(
        init,
        payer = payer,
        space = 8 + std::mem::size_of::<Policy>(),
        seeds = [b"policy", vault_id.to_le_bytes().as_ref()],
        bump
    )]
    pub policy: Account<'info, Policy>,

    /// Creator's quote token account
    #[account(
        constraint = creator_quote_ata.mint == quote_mint.key() @ FeeModuleError::InvalidQuoteMint
    )]
    pub creator_quote_ata: Account<'info, TokenAccount>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(vault_id: u64)]
pub struct InitializeDistribution<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,

    /// Honorary position state
    #[account(
        seeds = [b"honorary_position", vault_id.to_le_bytes().as_ref()],
        bump,
        has_one = quote_mint,
        has_one = base_mint
    )]
    pub honorary_position: Account<'info, HonoraryPosition>,

    /// Quote mint
    pub quote_mint: Account<'info, Mint>,

    /// Base mint
    pub base_mint: Account<'info, Mint>,

    /// PDA that owns the honorary position
    #[account(
        seeds = [b"investor_fee_pos_owner", vault_id.to_le_bytes().as_ref()],
        bump = honorary_position.bump
    )]
    /// CHECK: PDA signer
    pub position_owner: UncheckedAccount<'info>,

    /// Progress tracker
    #[account(
        init,
        payer = payer,
        space = 8 + std::mem::size_of::<Progress>(),
        seeds = [b"progress", vault_id.to_le_bytes().as_ref()],
        bump
    )]
    pub progress: Account<'info, Progress>,

    /// Program's quote treasury (where claimed fees go)
    #[account(
        init,
        payer = payer,
        token::mint = quote_mint,
        token::authority = position_owner,
        seeds = [b"quote_treasury", vault_id.to_le_bytes().as_ref()],
        bump
    )]
    pub program_quote_treasury: Account<'info, TokenAccount>,

    /// Program's base treasury (must remain empty - for validation)
    #[account(
        init,
        payer = payer,
        token::mint = base_mint,
        token::authority = position_owner,
        seeds = [b"base_treasury", vault_id.to_le_bytes().as_ref()],
        bump
    )]
    pub program_base_treasury: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(vault_id: u64)]
pub struct CrankDistribute<'info> {
    /// Anyone can call this (permissionless crank)
    #[account(mut)]
    pub cranker: Signer<'info>,

    /// The DAMM v2 CP-AMM program
    /// CHECK: DAMM v2 program ID
    pub cp_amm_program: UncheckedAccount<'info>,

    /// The DAMM v2 pool
    /// CHECK: Validated by DAMM v2
    pub pool: UncheckedAccount<'info>,

    /// Pool's token A (base) vault
    pub pool_token_a: Account<'info, TokenAccount>,

    /// Pool's token B (quote) vault
    pub pool_token_b: Account<'info, TokenAccount>,

    /// Honorary position state
    #[account(
        seeds = [b"honorary_position", vault_id.to_le_bytes().as_ref()],
        bump,
        has_one = position_owner
    )]
    pub honorary_position: Account<'info, HonoraryPosition>,

    /// PDA that owns the honorary position
    #[account(
        seeds = [b"investor_fee_pos_owner", vault_id.to_le_bytes().as_ref()],
        bump = honorary_position.bump
    )]
    /// CHECK: PDA signer
    pub position_owner: UncheckedAccount<'info>,

    /// The actual DAMM v2 position account (where fees accumulate)
    /// CHECK: Owned by DAMM v2, validated via CPI
    pub honorary_position_account: UncheckedAccount<'info>,

    /// Policy configuration
    #[account(
        seeds = [b"policy", vault_id.to_le_bytes().as_ref()],
        bump
    )]
    pub policy: Account<'info, Policy>,

    /// Progress tracker
    #[account(
        mut,
        seeds = [b"progress", vault_id.to_le_bytes().as_ref()],
        bump
    )]
    pub progress: Account<'info, Progress>,

    /// Program's quote treasury (where claimed fees go)
    #[account(
        mut,
        seeds = [b"quote_treasury", vault_id.to_le_bytes().as_ref()],
        bump
    )]
    pub program_quote_treasury: Account<'info, TokenAccount>,

    /// Program's base treasury (must remain empty - for validation)
    #[account(
        seeds = [b"base_treasury", vault_id.to_le_bytes().as_ref()],
        bump
    )]
    pub program_base_treasury: Account<'info, TokenAccount>,

    /// Creator's quote token account (for remainder)
    #[account(
        constraint = creator_quote_ata.key() == policy.creator_quote_ata @ FeeModuleError::InvalidCreatorAta,
        constraint = creator_quote_ata.mint == honorary_position.quote_mint @ FeeModuleError::InvalidQuoteMint
    )]
    pub creator_quote_ata: Account<'info, TokenAccount>,

    /// Streamflow program
    /// CHECK: Streamflow program ID
    pub streamflow_program: UncheckedAccount<'info>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    
    // Remaining accounts:
    // - Investor quote ATAs (for payouts)
    // - Streamflow stream accounts (for reading locked amounts)
}

// ============================================================================
// Events
// ============================================================================

#[event]
pub struct HonoraryPositionInitialized {
    pub vault_id: u64,
    pub pool: Pubkey,
    pub quote_mint: Pubkey,
    pub position_owner: Pubkey,
    pub investor_fee_share_bps: u16,
}

#[event]
pub struct QuoteFeesClaimed {
    pub vault_id: u64,
    pub amount: u64,
    pub timestamp: i64,
}

#[event]
pub struct InvestorPayoutPage {
    pub vault_id: u64,
    pub day_timestamp: i64,
    pub page_size: u32,
    pub total_distributed: u64,
    pub dust_carry: u64,
}

#[event]
pub struct CreatorPayoutDayClosed {
    pub vault_id: u64,
    pub day_timestamp: i64,
    pub creator_payout: u64,
    pub total_investor_payout: u64,
}

// ============================================================================
// Errors
// ============================================================================

#[error_code]
pub enum FeeModuleError {
    #[msg("Invalid fee share basis points (must be 0-10000)")]
    InvalidFeeShareBps,
    
    #[msg("Invalid quote mint - must match pool token B")]
    InvalidQuoteMint,
    
    #[msg("Base fees detected - this position must be quote-only")]
    BaseFeeDetected,
    
    #[msg("Distribution already completed today")]
    AlreadyDistributedToday,
    
    #[msg("Math overflow occurred")]
    MathOverflow,
    
    #[msg("Investor ATA not found in remaining accounts")]
    InvestorAtaNotFound,
    
    #[msg("Invalid creator ATA")]
    InvalidCreatorAta,
    
    #[msg("Day not finalized - cannot start new day")]
    DayNotFinalized,
}
