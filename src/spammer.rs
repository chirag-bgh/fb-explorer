use alloy::{
    eips::eip2718::Encodable2718,
    network::{EthereumWallet, TransactionBuilder},
    primitives::{Address, U256, keccak256},
    providers::{Provider, ProviderBuilder},
    rpc::client::RpcClient,
    signers::local::PrivateKeySigner,
    transports::http::{
        hyper_util::{client::legacy::Client, rt::TokioExecutor},
        AuthLayer, Http, HyperClient,
    },
};
use alloy_primitives::Bytes;
use alloy_rpc_types_engine::JwtSecret;
use alloy_rpc_types_eth::{TransactionReceipt, TransactionRequest};
use eyre::{Context, Result};
use http_body_util::Full;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tower::ServiceBuilder;
use tracing::{error, info, warn};

const GAS_LIMIT: u64 = 21000;
const GAS_PRICE_BUMP: u128 = 1_000_000_000;
const SYNC_TX_TIMEOUT_MS: u64 = 6000; 

#[derive(Clone)]
pub struct TxSpammer {
    rpc_url: String,
    jwt_secret: String,
    signer: PrivateKeySigner,
    chain_id: u64,
}

impl TxSpammer {
    /// Creates a new TxSpammer instance with JWT authentication
    pub async fn new(
        rpc_url: String,
        private_key: String,
        jwt_secret: String,
    ) -> Result<Self> {
        info!("Initializing TxSpammer with RPC URL: {}", rpc_url);

        let private_key_bytes = hex::decode(&private_key)
            .context("Failed to decode private key. Ensure it's a valid hex string")?;
        let signer = PrivateKeySigner::from_slice(&private_key_bytes)
            .context("Failed to create signer from private key")?;

        info!("Spammer wallet address: {}", signer.address());

        JwtSecret::from_hex(&jwt_secret)
            .context("Failed to parse JWT secret. Ensure it's a valid hex string")?;

        let provider = ProviderBuilder::new().on_http(rpc_url.parse()?);
        let chain_id = provider
            .get_chain_id()
            .await
            .context("Failed to fetch chain ID from RPC")?;

        info!("Network chain ID: {}", chain_id);

        Ok(Self {
            rpc_url,
            jwt_secret,
            signer,
            chain_id,
        })
    }


    /// Creates a provider for making RPC calls
    fn create_provider(&self) -> alloy::providers::fillers::FillProvider<
        alloy::providers::fillers::JoinFill<
            alloy::providers::Identity,
            alloy::providers::fillers::WalletFiller<alloy::network::EthereumWallet>,
        >,
        alloy::providers::RootProvider<alloy::transports::http::Http<alloy::transports::http::Client>>,
        alloy::transports::http::Http<alloy::transports::http::Client>,
        alloy::network::Ethereum,
    > {
        let wallet = EthereumWallet::from(self.signer.clone());
        ProviderBuilder::new()
            .wallet(wallet)
            .on_http(self.rpc_url.parse().expect("Invalid RPC URL"))
    }

    pub async fn spam_transactions(&self, trigger_info: &str, num: usize) {
        info!("🚀 Triggering {} self-transfer transactions ({})", num, trigger_info);

        let from_address = self.signer.address();

        let (starting_nonce, gas_price) = match self.get_tx_params(from_address).await {
            Ok(params) => params,
            Err(e) => {
                error!("Failed to get transaction parameters: {}", e);
                return;
            }
        };

        info!("Starting nonce: {}, Gas price: {} Gwei", starting_nonce, gas_price / 1_000_000_000);

        let semaphore = Arc::new(Semaphore::new(5));
        let mut handles = Vec::new();

        for i in 0..num {
            let tx_nonce = starting_nonce + i as u64;
            let spammer = self.clone();
            let sem = Arc::clone(&semaphore);

            let handle = tokio::spawn(async move {
                let _permit = sem.acquire().await.unwrap();
                spammer.send_self_transfer(tx_nonce, gas_price, i).await
            });

            handles.push(handle);
        }

        let mut success_count = 0;
        let mut failure_count = 0;

        for (i, handle) in handles.into_iter().enumerate() {
            match handle.await {
                Ok(Ok(_)) => {
                    success_count += 1;
                }
                Ok(Err(e)) => {
                    warn!("Transaction {} failed: {}", i, e);
                    failure_count += 1;
                }
                Err(e) => {
                    error!("Task {} panicked: {}", i, e);
                    failure_count += 1;
                }
            }
        }

        info!(
            "✅ Spam batch complete: {} successful, {} failed",
            success_count, failure_count
        );
    }

    /// Gets current nonce and gas price for the signer address
    ///
    /// This uses `.pending()` to get the nonce that accounts for all pending
    /// transactions in the mempool, ensuring we don't reuse nonces from
    /// previous spam batches that may still be pending.
    async fn get_tx_params(&self, address: Address) -> Result<(u64, u128)> {
        let provider = self.create_provider();

        let nonce = provider
            .get_transaction_count(address)
            .pending()
            .await
            .context("Failed to get nonce")?;

        let gas_price = provider
            .get_gas_price()
            .await
            .context("Failed to get gas price")?;

        Ok((nonce, gas_price + GAS_PRICE_BUMP))
    }

    /// Sends a single self-transfer transaction using eth_sendRawTransactionSync
    async fn send_self_transfer(&self, nonce: u64, gas_price: u128, tx_index: usize) -> Result<()> {
        let from_address = self.signer.address();

        let mut tx_request = TransactionRequest::default()
            .with_from(from_address)
            .with_to(from_address)
            .with_nonce(nonce)
            .with_chain_id(self.chain_id)
            .with_gas_limit(GAS_LIMIT)
            .with_max_fee_per_gas(gas_price)
            .with_max_priority_fee_per_gas(gas_price / 10);

        tx_request.value = Some(U256::ZERO); // 0 ETH transfer

        let wallet = EthereumWallet::from(self.signer.clone());
        let typed_tx = tx_request
            .build(&wallet)
            .await
            .context("Failed to build transaction")?;

        let raw_tx = typed_tx.encoded_2718();

        let tx_hash = keccak256(&raw_tx);
        info!("  TX[{}] 📤 Sending tx hash: 0x{} Nonce: {}", tx_index, hex::encode(tx_hash), nonce);

        let jwt = JwtSecret::from_hex(&self.jwt_secret)
            .context("Failed to parse JWT secret")?;
        let hyper_client = Client::builder(TokioExecutor::new()).build_http::<Full<Bytes>>();
        let auth_layer = AuthLayer::new(jwt);
        let service = ServiceBuilder::new()
            .layer(auth_layer)
            .service(hyper_client);

        let layer_transport = HyperClient::with_service(service);
        let http_hyper = Http::with_client(layer_transport, self.rpc_url.parse()?);
        let rpc_client = RpcClient::new(http_hyper, true);

        match rpc_client
            .request::<_, TransactionReceipt>("eth_sendRawTransactionSync", (raw_tx, Some(SYNC_TX_TIMEOUT_MS)))
            .await
        {
            Ok(receipt) => {
                info!(
                    "  TX[{}] ✓ hash: 0x{}, block: {:?}, gas: {}, status: {}",
                    tx_index,
                    hex::encode(receipt.transaction_hash),
                    receipt.block_number,
                    receipt.gas_used,
                    if receipt.status() { "Success" } else { "Failed" }
                );
                Ok(())
            }
            Err(e) => {
                warn!("  TX[{}] ✗ hash: 0x{}, Error: {}", tx_index, hex::encode(tx_hash), e);
                Err(eyre::eyre!("Transaction failed: {}", e))
            }
        }
    }
}
