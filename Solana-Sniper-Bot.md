# 🧭 Solana Sniper Bot [Project ID: P-667]

A Rust-based Solana token sniper that monitors new tokens via Yellowstone gRPC, executes buys on PumpFun and related DEXes, and manages automated selling with configurable price-drop, consolidation, and recovery strategies.



## 📚 Table of Contents

[About](#-about)  
[Features](#-features)  
[Tech Stack](#-tech-stack)  
[Installation](#️-installation)  
[Usage](#-usage)  
[Configuration](#-configuration)  
[Project Structure](#-project-structure)  
[Contact](#-contact)



## 🧩 About

This project provides a high-performance token sniper for the Solana ecosystem. It connects to Yellowstone gRPC for real-time transaction streaming, executes buys on PumpFun (and PumpSwap with notification-only mode), and implements multiple selling strategies based on price action, net buy volume, consolidation, and recovery signals. Key goals: low-latency entry on new tokens, configurable risk (slippage, amounts, token age), and automated exit logic (price drop recovery, big drop recovery, inactivity, booming/risky token handling).



## ✨ Features

- **Real-time monitoring** – Yellowstone gRPC streaming for new token and swap detection  
- **Multi-protocol buying** – PumpFun and PumpSwap integration (PumpSwap: notifications only)  
- **Selling strategies** – Price drop normal/immediate recovery, big drop recovery, consolidation-based buying, inactivity and low-interest exits  
- **Buying strategies** – price drop based, inactivity and low-interest exits  
- **Risk controls** – Configurable buy/sell slippage, `BUY_AMOUNT_IN_SOL`, min token age, net-buy thresholds  
- **ZeroSlot support** – Optional tip for faster inclusion via ZeroSlot  
- **Telegram notifications** – Optional bot token and chat ID for alerts  
- **Caching & batch RPC** – Improved performance via caching and batched RPC calls  
- **Transaction retry & blockhash** – Fresh blockhash handling and retry logic for reliability  



## 🧠 Tech Stack

| Category   | Technologies |
|-----------|--------------|
| **Language** | Rust (edition 2021) |
| **Blockchain** | Solana (SDK 2.1.x), Anchor client 0.31 |
| **Streaming** | Yellowstone gRPC client |
| **DEX / Tokens** | PumpFun, PumpSwap, Jupiter API, SPL Token / Token-2022 |
| **Async** | Tokio (full), tokio-tungstenite, futures |
| **Config** | dotenv, clap (CLI), serde/serde_json |
| **Other** | reqwest, teloxide (Telegram), dashmap, LRU cache |



## ⚙️ Installation

```bash
# Clone the repository
git clone https://github.com/AV1080p/Solana-Sniper-Bot

# Navigate to the project directory
cd sniper-bot

# Build (requires Rust toolchain)
cargo build --release
```

Ensure you have a [Rust](https://rustup.rs/) toolchain installed (`rustc`, `cargo`).



## 🚀 Usage

```bash
# Run with default config (loads .env)
cargo run --release

# Wrap SOL to WSOL before sniping (optional)
cargo run --release -- --wrap
```

Then the bot will connect to Yellowstone, load config from `.env`, and start monitoring and trading according to the configured strategies.



## 🧾 Configuration

Create a `.env` file in the project root (see `src/env.example` for the full list). Required and commonly used variables:

| Variable | Description |
|----------|-------------|
| `YELLOWSTONE_GRPC_HTTP` | Yellowstone gRPC endpoint URL |
| `YELLOWSTONE_GRPC_TOKEN` | Yellowstone auth token |
| `RPC_HTTP` | Solana RPC endpoint URL |
| `PRIVATE_KEY` | Base58 wallet secret key (min ~85 chars) |
| `BUY_AMOUNT_IN_SOL` | SOL per buy (e.g. `0.001`) |
| `BUY_SLIPPAGE` | Buy slippage in basis points (e.g. `700` = 7%) |
| `SELL_SLIPPAGE` | Sell slippage in basis points (e.g. `20000` = 200%) |

**Trading strategy (examples):**

- `PRICE_DROP_NORMAL_RECOVERY_MIN_PARAMETER` – Price drop + recovery conditions (e.g. `5.0:6,8.0:5,10.0:4`)
- `BIG_DROP_*` – Big drop detection and recovery thresholds
- `CONSOLIDATION_EACH_CANDLE_NET_BUY_PARAMETER` – Consolidation net buy range
- `SELL_ON_INACTIVITY_PARAMETER` – Inactivity exit (e.g. `3:-0.3:-2.5,6:0.3:-2.5`)

**Optional:**

- `TELEGRAM_BOT_TOKEN`, `TELEGRAM_CHAT_ID` – Telegram alerts  
- `ZERO_SLOT_URL`, `ZERO_SLOT_TIP_VALUE` – ZeroSlot tip  
- `UNIT_PRICE`, `UNIT_LIMIT` – Compute unit fee settings  
- `WRAP_AMOUNT` – SOL to wrap when using `--wrap` (default `0.1`)

Copy `src/env.example` to `.env` and fill in your values.



## 📁 Project Structure

| Path | Description |
|------|-------------|
| `src/main.rs` | Entry point, wallet init, wrap-SOL, sniper startup |
| `src/lib.rs` | Library root (common, core, dex, engine, error, services) |
| `src/common/` | Config, constants, logger, cache |
| `src/core/` | Token and transaction types |
| `src/dex/` | PumpFun, PumpSwap integrations |
| `src/engine/` | Sniper loop, monitor, selling strategy, swap, transaction parser/retry |
| `src/services/` | RPC client, Jupiter API, blockhash processor, cache maintenance, Telegram, ZeroSlot, health/memory/task monitors |
| `src/error/` | Error types |

See `src/env.example` for all environment variables and comments.



## 📬 Contact

- **Author:** Ronald Chan
- **Email:** ronaldchan0425+av1080p@gmail.com
- **GitHub:** @av1080p



## 🌟 Acknowledgements

- Solana and Anchor ecosystems  
- Yellowstone gRPC for streaming data  
- PumpFun and Jupiter for DEX integration  
- ZeroSlot for priority transaction support  
