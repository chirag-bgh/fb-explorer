mod metrics;
mod payload;
mod subscriber;
mod web;

use alloy::providers::{Provider, ProviderBuilder, WsConnect};
use alloy::rpc::types::BlockTransactionsKind;
use eyre::Result;
use futures_util::StreamExt;
use serde::Serialize;
use std::fs;
use std::sync::Arc;
use std::time::Duration;
use subscriber::{FlashblocksReceiver, FlashblocksSubscriber};
use payload::FlashBlock;
use url::Url;

/// Test TCP connectivity to a host:port
async fn test_tcp_connection(host: &str, port: u16) -> Result<bool> {
    use tokio::net::TcpStream;

    match tokio::time::timeout(
        Duration::from_secs(5),
        TcpStream::connect(format!("{}:{}", host, port))
    ).await {
        Ok(Ok(_)) => Ok(true),
        Ok(Err(e)) => {
            eprintln!("TCP connection to {}:{} failed: {}", host, port, e);
            Ok(false)
        }
        Err(_) => {
            eprintln!("TCP connection to {}:{} timed out after 5 seconds", host, port);
            Ok(false)
        }
    }
}

#[derive(Serialize)]
struct BlockTransactions {
    block_number: u64,
    block_hash: String,
    extra_data: String,
    transaction_hashes: Vec<String>,
}

// Handler for flashblocks
struct FlashblockHandler;

impl FlashblocksReceiver for FlashblockHandler {
    fn on_flashblock_received(&self, flashblock: FlashBlock) {
        let block_number = flashblock.block_number();

        println!("\n✨ Flashblock Received:");
        println!("   Block Number: {}", block_number);
        println!("   Index: {}", flashblock.index);
        println!("   Payload ID: {:?}", flashblock.payload_id);
        if let Some(tx_hash) = &flashblock.metadata.transaction_hash {
            println!("   Transaction Hash: {:?}", tx_hash);
        }
        println!("   Transactions: {}", flashblock.diff.transactions.len());
        println!("   Gas Used: {}", flashblock.diff.gas_used);

        // Serialize to JSON
        match serde_json::to_string_pretty(&flashblock) {
            Ok(json) => {
                // Save to file named by block_number and index in data folder
                let filename = format!("data/flashblock_{}_{}.json", block_number, flashblock.index);
                if let Err(e) = fs::write(&filename, json) {
                    eprintln!("❌ Error writing flashblock file {}: {}", filename, e);
                } else {
                    println!("💾 Saved flashblock to {}\n", filename);
                }
            }
            Err(e) => eprintln!("❌ Error serializing flashblock data: {}\n", e),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    // Start web dashboard server
    tokio::spawn(async {
        if let Err(e) = web::start_web_server().await {
            eprintln!("Web server error: {}", e);
        }
    });

    // Start flashblocks subscription
    let flashblock_ws_url = std::env::var("FLASHBLOCK_WS_URL")
        .unwrap_or_else(|_| "ws://16.163.4.133:1111".to_string());

    println!("Starting flashblocks subscription to: {}", flashblock_ws_url);
    println!("Note: Set FLASHBLOCK_WS_URL environment variable to use a different endpoint");

    let handler = Arc::new(FlashblockHandler);
    let mut flashblocks_subscriber = FlashblocksSubscriber::new(
        handler.clone(),
        Url::parse(&flashblock_ws_url)?,
    );
    flashblocks_subscriber.start();

    // Start regular blocks subscription
    let rpc_url = std::env::var("BLOCK_WS_URL")
        .unwrap_or_else(|_| "ws://16.163.4.133:8546".to_string());

    println!("Starting blocks subscription to: {}", rpc_url);
    println!("Note: Set BLOCK_WS_URL environment variable to use a different endpoint");

    // Check if we can parse the URL before attempting connection
    let parsed_url = Url::parse(&rpc_url)?;
    let host = parsed_url.host_str().unwrap_or("unknown");
    let port = parsed_url.port().unwrap_or(8546);

    println!("Attempting to connect to {}:{}", host, port);

    // Test TCP connectivity first
    print!("Testing TCP connectivity to {}:{}... ", host, port);
    match test_tcp_connection(host, port).await {
        Ok(true) => {
            println!("✓ Success");
        }
        Ok(false) | Err(_) => {
            println!("✗ Failed");
            println!("\n❌ Cannot establish TCP connection to {}:{}", host, port);
            println!("\nPlease verify:");
            println!("  1. The server is running and accessible");
            println!("  2. Firewall allows connections to port {}", port);
            println!("  3. The correct endpoint URL is configured");
            println!("\nTo use a different endpoint:");
            println!("  BLOCK_WS_URL=ws://your-server:port cargo run");
            println!("\nThe application will continue and retry connections in the background...");
        }
    }

    let ws = WsConnect::new(rpc_url);
    let provider = ProviderBuilder::new().on_ws(ws).await.map_err(|e| {
        eprintln!("\n❌ Failed to connect to block WebSocket at {}:{}", host, port);
        eprintln!("Error: {}", e);
        eprintln!("\nTroubleshooting:");
        eprintln!("  - Check if the server is running: ping {}", host);
        eprintln!("  - Set custom endpoint: BLOCK_WS_URL=ws://your-server:port cargo run");
        e
    })?;

    // Subscribe to new blocks
    let sub = provider.subscribe_blocks().await?;
    let mut stream = sub.into_stream();

    println!("Awaiting block headers...");

    // Process blocks in the main task
    let handle = tokio::spawn(async move {
        while let Some(header) = stream.next().await {
            let block_number = header.number;
            println!("Latest block number: {}", block_number);

            // Fetch the full block with transactions
            match provider.get_block_by_number(block_number.into(), BlockTransactionsKind::Full).await {
                Ok(Some(block)) => {
                    // Extract transaction hashes
                    let tx_hashes: Vec<String> = block
                        .transactions
                        .hashes()
                        .map(|hash| format!("{:?}", hash))
                        .collect();

                    println!("Found {} transactions in block {}", tx_hashes.len(), block_number);

                    // Extract block hash and extra data
                    let block_hash = format!("{:?}", block.header.hash);
                    let extra_data = String::from_utf8(block.header.extra_data.to_vec())
                        .unwrap_or_else(|_| format!("0x{}", hex::encode(&block.header.extra_data)));

                    // Create the data structure
                    let block_data = BlockTransactions {
                        block_number,
                        block_hash,
                        extra_data,
                        transaction_hashes: tx_hashes,
                    };

                    // Serialize to JSON
                    match serde_json::to_string_pretty(&block_data) {
                        Ok(json) => {
                            // Save to file named by block number in data folder
                            let filename = format!("data/{}.json", block_number);
                            if let Err(e) = fs::write(&filename, json) {
                                eprintln!("Error writing file {}: {}", filename, e);
                            } else {
                                println!("Saved transactions to {}", filename);
                            }
                        }
                        Err(e) => eprintln!("Error serializing block data: {}", e),
                    }
                }
                Ok(None) => {
                    eprintln!("Block {} not found", block_number);
                }
                Err(e) => {
                    eprintln!("Error fetching block {}: {}", block_number, e);
                }
            }
        }
    });

    handle.await?;

    Ok(())
}
