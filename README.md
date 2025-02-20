# Solana-Rust-Pump.fun-Sniper
Extremely fast sniper that snipes every new pump.fun token and sells automatically upon a change in token price, or a time held in seconds.

## Config  
const MAX_CONCURRENT_TRADES: usize = 3; <-- limit the amount of tokens to actively trade  

const ABANDON_TIMEOUT: Duration = Duration::from_secs(8); <-- tokens older than 8 seconds will be sold  

const BUY_AMOUNT: u64 = 30000000000; <-- set this to the raw token amount to buy  

const SELL_AMOUNT: u64 = 30000000000; <-- set this to the raw token amount to sell   

const PROFIT_THRESHOLD: f64 = 0.04; <-- sell token after X change in profit (0.04 = 4%)  

also don't forget to add your rpc endpoint
