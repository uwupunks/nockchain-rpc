use tonic::{transport::Server, Request, Response, Status};
use nockchain::nockchain_service_server::{NockchainService, NockchainServiceServer};
use nockchain::{GetBalanceRequest, GetBalanceResponse};
use tokio::process::Command as TokioCommand;
use tokio::time::{timeout, Duration};
use env_logger;
use log;
use regex::Regex;
use dotenvy::dotenv;
use std::env;

pub mod nockchain {
    tonic::include_proto!("nockchain");
}

// Function to parse nockchain-wallet output and sum all assets
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

        // Skip empty lines and log messages
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
struct NockchainServiceImpl;

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
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok(); // Load .env file, ignore if missing
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

    let addr = format!("127.0.0.1:{}", port).parse()?;
    log::info!("Starting gRPC server on http://{}", addr);
    
    Server::builder()
        .add_service(NockchainServiceServer::new(NockchainServiceImpl))
        .serve(addr)
        .await?;
    
    Ok(())
}
