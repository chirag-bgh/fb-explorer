use axum::{
    extract::Path,
    response::{Html, IntoResponse, Json},
    routing::get,
    Router,
};
use serde::{Deserialize, Serialize};
use std::fs;
use tower_http::cors::CorsLayer;
use alloy_primitives::{keccak256, Bytes};

#[derive(Debug, Serialize, Deserialize)]
pub struct BlockInfo {
    pub block_number: u64,
    pub block_hash: Option<String>,
    pub extra_data: Option<String>,
    pub transaction_count: usize,
    pub flashblock_count: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BlockDetails {
    pub block_number: u64,
    pub transaction_hashes: Vec<String>,
    pub flashblocks: Vec<FlashblockSummary>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FlashblockSummary {
    pub index: u64,
    pub transaction_count: usize,
    pub transaction_hashes: Vec<String>,
}

pub async fn start_web_server() -> Result<(), Box<dyn std::error::Error>> {
    let app = Router::new()
        .route("/", get(serve_dashboard))
        .route("/api/blocks", get(get_blocks))
        .route("/api/blocks/:block_number", get(get_block_details))
        .layer(CorsLayer::permissive());

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?;
    println!("Dashboard server running on http://localhost:8080");

    axum::serve(listener, app).await?;

    Ok(())
}

async fn serve_dashboard() -> impl IntoResponse {
    Html(include_str!("dashboard.html"))
}

async fn get_blocks() -> impl IntoResponse {
    let mut blocks = Vec::new();

    // Read all block JSON files from data directory
    if let Ok(entries) = fs::read_dir("data") {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                // Match files like "123.json" (not flashblock_*.json)
                if filename.ends_with(".json") && !filename.starts_with("flashblock_") {
                    if let Some(block_num_str) = filename.strip_suffix(".json") {
                        if let Ok(block_number) = block_num_str.parse::<u64>() {
                            // Read the block file to get transaction count
                            if let Ok(content) = fs::read_to_string(&path) {
                                if let Ok(block_data) = serde_json::from_str::<serde_json::Value>(&content) {
                                    let tx_count = block_data["transaction_hashes"]
                                        .as_array()
                                        .map(|arr| arr.len())
                                        .unwrap_or(0);

                                    let block_hash = block_data["block_hash"]
                                        .as_str()
                                        .map(String::from);

                                    let extra_data = block_data["extra_data"]
                                        .as_str()
                                        .map(String::from);

                                    // Count flashblocks for this block
                                    let flashblock_count = count_flashblocks_for_block(block_number);

                                    blocks.push(BlockInfo {
                                        block_number,
                                        block_hash,
                                        extra_data,
                                        transaction_count: tx_count,
                                        flashblock_count,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Sort blocks by block number (descending)
    blocks.sort_by(|a, b| b.block_number.cmp(&a.block_number));

    // Take only the latest 50 blocks
    blocks.truncate(50);

    Json(blocks)
}

fn count_flashblocks_for_block(block_number: u64) -> usize {
    let mut count = 0;
    if let Ok(entries) = fs::read_dir("data") {
        for entry in entries.flatten() {
            if let Some(filename) = entry.file_name().to_str() {
                if filename.starts_with(&format!("flashblock_{}_", block_number)) {
                    count += 1;
                }
            }
        }
    }
    count
}

async fn get_block_details(Path(block_number): Path<u64>) -> impl IntoResponse {
    let block_file = format!("data/{}.json", block_number);

    // Read block transactions
    let transaction_hashes = match fs::read_to_string(&block_file) {
        Ok(content) => {
            serde_json::from_str::<serde_json::Value>(&content)
                .ok()
                .and_then(|data| data["transaction_hashes"].as_array().cloned())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default()
        }
        Err(_) => Vec::new(),
    };

    // Find all flashblocks for this block number
    let mut flashblocks = Vec::new();

    if let Ok(entries) = fs::read_dir("data") {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                // Match files like "flashblock_123_0.json"
                if filename.starts_with(&format!("flashblock_{}_", block_number)) {
                    if let Ok(content) = fs::read_to_string(&path) {
                        if let Ok(flashblock_data) = serde_json::from_str::<serde_json::Value>(&content) {
                            let index = flashblock_data["index"].as_u64().unwrap_or(0);

                            // Extract transaction hashes from diff.transactions
                            // The transactions field contains raw transaction bytes, not hashes
                            // We need to compute the keccak256 hash of each transaction
                            let tx_hashes = flashblock_data["diff"]["transactions"]
                                .as_array()
                                .map(|arr| {
                                    arr.iter()
                                        .filter_map(|v| {
                                            if let Some(tx_hex) = v.as_str() {
                                                // Parse hex string to bytes and compute keccak256 hash
                                                if let Ok(tx_bytes) = tx_hex.parse::<Bytes>() {
                                                    let hash = keccak256(&tx_bytes);
                                                    return Some(format!("{:?}", hash));
                                                }
                                            }
                                            None
                                        })
                                        .collect::<Vec<String>>()
                                })
                                .unwrap_or_default();

                            flashblocks.push(FlashblockSummary {
                                index,
                                transaction_count: tx_hashes.len(),
                                transaction_hashes: tx_hashes,
                            });
                        }
                    }
                }
            }
        }
    }

    // Sort flashblocks by index
    flashblocks.sort_by_key(|f| f.index);

    let details = BlockDetails {
        block_number,
        transaction_hashes,
        flashblocks,
    };

    Json(details)
}
