use nockapp::{JammedNoun, NounExt};
use rocksdb::{DB, ColumnFamilyDescriptor, Options, WriteBatch};
use std::path::Path;
use std::sync::Arc;
use tonic::{transport::Server, Request, Response, Status};
use nockchain::nockchain_service_server::{NockchainService, NockchainServiceServer};
use nockchain::{
    GetBalanceRequest, GetBalanceResponse,
    GetBlockByHeightRequest, GetBlockByHeightResponse,
    GetBlockByDigestRequest, GetBlockByDigestResponse,
    Block,
};
use tokio::process::Command as TokioCommand;
use tokio::time::{timeout, Duration};
use env_logger;
use log;
use regex::Regex;
use dotenvy::dotenv;
use std::env;
use nockvm::mem::NockStack;
use nockvm::noun::{Noun};
use hex::{decode, encode};
use crate::log::debug;

pub mod nockchain {
    tonic::include_proto!("nockchain");
}

#[derive(Debug)]
pub enum IndexerError {
    RocksDB(rocksdb::Error),
    Hex(hex::FromHexError),
    InvalidData(String),
    Memory(String),
}

impl From<rocksdb::Error> for IndexerError {
    fn from(err: rocksdb::Error) -> Self {
        IndexerError::RocksDB(err)
    }
}

impl From<hex::FromHexError> for IndexerError {
    fn from(err: hex::FromHexError) -> Self {
        IndexerError::Hex(err)
    }
}

impl From<IndexerError> for Status {
    fn from(err: IndexerError) -> Self {
        match err {
            IndexerError::RocksDB(e) => Status::internal(format!("RocksDB error: {}", e)),
            IndexerError::Hex(e) => Status::invalid_argument(format!("Hex decode error: {}", e)),
            IndexerError::InvalidData(e) => Status::invalid_argument(format!("Invalid data: {}", e)),
            IndexerError::Memory(e) => Status::resource_exhausted(format!("Memory error: {}", e)),
        }
    }
}
pub struct Page {
    digest: Noun,           // block-id
    //pow: Noun,              // unit proof
    parent: Noun,           // block-id
    tx_ids: Noun,           // z-set tx-id
    coinbase: Noun,         // coinbase-split
    timestamp: Noun,        // @
    epoch_counter: Noun,    // @ud
    target: Noun,           // bignum:bn
    accumulated_work: Noun, // bignum:bn
    height: Noun,           // @ud (direct atom)
                            // msg: Noun,           // page-msg (optional)
}
impl Page {
    pub fn get_field(&self, field: &str) -> Result<&Noun, IndexerError> {
        match field {
            "digest" => Ok(&self.digest),
            //"pow" => Ok(&self.pow),
            "parent" => Ok(&self.parent),
            "tx-ids" => Ok(&self.tx_ids),
            "coinbase" => Ok(&self.coinbase),
            "timestamp" => Ok(&self.timestamp),
            "epoch-counter" => Ok(&self.epoch_counter),
            "target" => Ok(&self.target),
            "accumulated-work" => Ok(&self.accumulated_work),
            "height" => Ok(&self.height),
            //"msg" => Ok(&self.msg),
            _ => {
                debug!("Unknown field: {}", field);
                Err(IndexerError::InvalidData(format!("Unknown field: {}", field)))
            }
        }
    }

    pub fn format_as_ud(&self, field: &str, stack: &mut NockStack) -> Result<String, IndexerError> {
        match self.get_field(field) {
            Ok(noun) => match noun.atom() {
                Some(atom) => {
                    if let Ok(value) = atom.as_u64() {
                        Ok(value.to_string())
                    } else {
                        Ok(atom.as_ubig(stack).to_string())
                    }
                }
                None => Ok(format!("invalid (not atom): {:?}", noun)),
            },
            Err(e) => Ok(format!("error: {:?}", e)),
        }
    }

    fn noun_to_bytes(&self, noun: &Noun, stack: &mut NockStack) -> Result<Vec<u8>, IndexerError> {
        let jammed = noun.jam_self(stack);
        Ok(<JammedNoun as AsRef<[u8]>>::as_ref(&jammed).to_vec())
    }

    fn to_block(&self, stack: &mut NockStack) -> Result<Block, IndexerError> {
        Ok(Block {
            digest: encode(self.noun_to_bytes(&self.digest, stack)?),
            //pow: encode(self.noun_to_bytes(&self.pow, stack)?),
            parent: encode(self.noun_to_bytes(&self.parent, stack)?),
            tx_ids: encode(self.noun_to_bytes(&self.tx_ids, stack)?),
            coinbase: encode(self.noun_to_bytes(&self.coinbase, stack)?),
            timestamp: self.format_as_ud("timestamp", stack)?,
            epoch_counter: self.format_as_ud("epoch_counter", stack)?,
            target: encode(self.noun_to_bytes(&self.target, stack)?),
            accumulated_work: encode(self.noun_to_bytes(&self.accumulated_work, stack)?),
            height: self.format_as_ud("height", stack)?,
        })
    }

    fn from_bytes(bytes: &[u8], stack: &mut NockStack) -> Result<Option<Self>, IndexerError> {
        let mut offset = 0;
        let mut nouns = Vec::with_capacity(10);

        for _ in 0..10 {
            if offset + 4 > bytes.len() {
                return Err(IndexerError::InvalidData("Incomplete data".to_string()));
            }
            let len = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;
            if offset + len > bytes.len() || len > 200_000 {
                return Err(IndexerError::InvalidData(format!("Invalid length: {}", len)));
            }
            let jammed = JammedNoun::new(bytes[offset..offset + len].to_vec().into());
            nouns.push(jammed.cue_self(stack).map_err(|e| IndexerError::Memory(format!("Cue failed: {:?}", e)))?);
            offset += len;
        }

        if nouns.len() != 10 {
            return Err(IndexerError::InvalidData("Wrong number of fields".to_string()));
        }

        Ok(Some(Page {
            digest: nouns[0],
            //pow: nouns[1],
            parent: nouns[2],
            tx_ids: nouns[3],
            coinbase: nouns[4],
            timestamp: nouns[5],
            epoch_counter: nouns[6],
            target: nouns[7],
            accumulated_work: nouns[8],
            height: nouns[9],
        }))
    }

    pub fn query_by_height(db: &DB, height: u64, stack: &mut NockStack) -> Result<Option<Self>, IndexerError> {
        let cf_height = db.cf_handle("height_to_digest").unwrap();
        let cf_pages = db.cf_handle("pages").unwrap();

        let height_key = height.to_string();
        if let Some(digest_bytes) = db.get_cf(&cf_height, height_key.as_bytes())? {
            if let Some(page_bytes) = db.get_cf(&cf_pages, &digest_bytes)? {
                return Self::from_bytes(&page_bytes, stack);
            }
        }
        Ok(None)
    }

    pub fn query_by_digest(db: &DB, digest: &str, stack: &mut NockStack) -> Result<Option<Self>, IndexerError> {
        let cf_pages = db.cf_handle("pages").unwrap();
        let digest_bytes = if digest.starts_with("0x_") {
            decode(&digest[3..])?
        } else {
            digest.as_bytes().to_vec()
        };
        if let Some(page_bytes) = db.get_cf(&cf_pages, &digest_bytes)? {
            return Self::from_bytes(&page_bytes, stack);
        }
        Ok(None)
    }
}

fn init_db(path: &str) -> Result<DB, rocksdb::Error> {
    log::info!("Initializing RocksDB at: {}", path);
    let mut cf_opts = Options::default();
    cf_opts.create_if_missing(false); // Read-only, don’t create

    let cf_names = vec![
        ColumnFamilyDescriptor::new("pages", cf_opts.clone()),
        ColumnFamilyDescriptor::new("height_to_digest", cf_opts),
    ];

    let mut db_opts = Options::default();
    db_opts.create_if_missing(false); // Read-only, don’t create
    db_opts.create_missing_column_families(false);

    DB::open_cf_descriptors_read_only(&db_opts, Path::new(path), cf_names, false)
}

fn parse_nockchain_output(output: &str) -> Result<u64, String> {
    if output.trim().is_empty() {
        log::error!("Empty command output");
        return Err("Empty command output".to_string());
    }

    log::debug!("Raw output length: {} bytes", output.len());
    let re = Regex::new(r"^- assets: (\d+)\s*$").map_err(|e| format!("Regex error: {}", e))?;
    let mut total_assets = 0;
    let mut asset_count = 0;

    for line in output.lines() {
        let line = line.trim();
        log::debug!("Processing line: {}", line);

        if line.is_empty() || line.contains("\u{001b}") {
            log::debug!("Skipped line: {}", line);
            continue;
        }

        if let Some(captures) = re.captures(line.to_lowercase().as_str()) {
            if let Some(asset_str) = captures.get(1) {
                let assets: u64 = asset_str.as_str().parse().map_err(|e| format!("Failed to parse assets: {}", e))?;
                log::info!("Found assets: {}", assets);
                total_assets += assets;
                asset_count += 1;
            }
        }
    }

    log::info!("Total assets summed: {}, Number of assets found: {}", total_assets, asset_count);
    Ok(total_assets)
}

#[derive(Debug)]
struct NockchainServiceImpl {
    db: Arc<DB>,
}

#[tonic::async_trait]
impl NockchainService for NockchainServiceImpl {
    async fn get_balance(
        &self,
        request: Request<GetBalanceRequest>,
    ) -> Result<Response<GetBalanceResponse>, Status> {
        let pubkey = request.into_inner().pubkey;
        log::info!("Received GetBalance request for pubkey: {}", pubkey);

        let socket_path = env::var("NOCKCHAIN_SOCKET").map_err(|e| {
            log::error!("Missing NOCKCHAIN_SOCKET environment variable: {}", e);
            Status::internal(format!("Missing NOCKCHAIN_SOCKET environment variable: {}", e))
        })?;

        let timeout_secs = match env::var("COMMAND_TIMEOUT_SECS") {
            Ok(secs) => secs.parse::<u64>().map_err(|e| {
                log::error!("Invalid COMMAND_TIMEOUT_SECS: {}", e);
                Status::invalid_argument(format!("Invalid COMMAND_TIMEOUT_SECS: {}", e))
            })?,
            Err(_) => {
                log::warn!("Missing COMMAND_TIMEOUT_SECS, using default: 120 seconds");
                120
            }
        };

        let output = timeout(Duration::from_secs(timeout_secs), TokioCommand::new("nockchain-wallet")
            .env("RUST_LOG", "error")
            .arg("--nockchain-socket")
            .arg(&socket_path)
            .arg("list-notes-by-pubkey")
            .arg(&pubkey)
            .output())
            .await
            .map_err(|_| Status::deadline_exceeded("Command timed out"))?;

        match output {
            Ok(output) => {
                log::info!("Command executed, status: {}", output.status);
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                    log::error!("Command failed: stderr={}", stderr);
                    return Err(Status::internal(format!("Command execution failed: {}", stderr)));
                }

                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                log::debug!("Raw command output: {}", stdout);
                match parse_nockchain_output(&stdout) {
                    Ok(total_assets) => {
                        log::info!("Total assets: {}", total_assets);
                        let balance = (total_assets as f64) / 65536.0;
                        log::info!("Total assets in nocks: {}", balance);
                        Ok(Response::new(GetBalanceResponse { balance }))
                    }
                    Err(error) => {
                        log::error!("Parsing error: {}", error);
                        Err(Status::internal(format!("Parsing error: {}", error)))
                    }
                }
            }
            Err(error) => {
                log::error!("Command error: {}", error);
                Err(Status::internal(format!("Server error: {}", error)))
            }
        }
    }

    async fn get_block_by_height(
        &self,
        request: Request<GetBlockByHeightRequest>,
    ) -> Result<Response<GetBlockByHeightResponse>, Status> {
        let height = request.into_inner().height;
        log::info!("Received GetBlockByHeight request for height: {}", height);

        let mut stack = NockStack::new(8 << 10 << 10, 64);
        match Page::query_by_height(&self.db, height, &mut stack) {
            Ok(Some(page)) => {
                let block = page.to_block(&mut stack)?;
                log::info!("Found block at height {}: {:?}", height, block);
                Ok(Response::new(GetBlockByHeightResponse { block: Some(block) }))
            }
            Ok(None) => {
                log::warn!("No block found at height {}", height);
                Ok(Response::new(GetBlockByHeightResponse { block: None }))
            }
            Err(e) => {
                log::error!("Error querying block by height {}: {:?}", height, e);
                Err(e.into())
            }
        }
    }

    async fn get_block_by_digest(
        &self,
        request: Request<GetBlockByDigestRequest>,
    ) -> Result<Response<GetBlockByDigestResponse>, Status> {
        let digest = request.into_inner().digest;
        log::info!("Received GetBlockByDigest request for digest: {}", digest);

        let mut stack = NockStack::new(8 << 10 << 10, 64); 
        match Page::query_by_digest(&self.db, &digest, &mut stack) {
            Ok(Some(page)) => {
                let block = page.to_block(&mut stack)?;
                log::info!("Found block with digest {}: {:?}", digest, block);
                Ok(Response::new(GetBlockByDigestResponse { block: Some(block) }))
            }
            Ok(None) => {
                log::warn!("No block found with digest {}", digest);
                Ok(Response::new(GetBlockByDigestResponse { block: None }))
            }
            Err(e) => {
                log::error!("Error querying block by digest {}: {:?}", digest, e);
                Err(e.into())
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();
    env_logger::init();

    let port = match env::var("PORT") {
        Ok(port) => port.parse::<u16>().map_err(|e| {
            log::error!("Invalid PORT: {}", e);
            format!("Invalid PORT: {}", e)
        })?,
        Err(_) => {
            log::warn!("Missing PORT, using default: 3000");
            3000
        }
    };

    let db_path = env::var("NOCKCHAIN_DB_PATH").unwrap_or("nockchain_index".to_string());
    log::info!("Opening RocksDB at: {:?}", std::fs::canonicalize(&db_path).unwrap_or(db_path.clone().into()));
    let db = Arc::new(init_db(&db_path)?);

    let addr = format!("127.0.0.1:{}", port).parse()?;
    log::info!("Starting gRPC server on http://{}", addr);

    Server::builder()
        .add_service(NockchainServiceServer::new(NockchainServiceImpl { db }))
        .serve(addr)
        .await?;

    Ok(())
}
