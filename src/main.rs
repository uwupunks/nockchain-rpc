use actix_web::{post, web, App, HttpResponse, HttpServer, Responder};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::process::Command as TokioCommand;
use tokio::time::{timeout, Duration};
use env_logger;
use log;
use regex::Regex;
use dotenvy::dotenv;
use std::env;

#[derive(Deserialize, Debug)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: Option<u64>,
    method: String,
    params: Option<Params>,
}

#[derive(Deserialize, Debug)]
struct Params {
    pubkey: String,
}

#[derive(Serialize)]
struct JsonRpcResponse<T> {
    jsonrpc: &'static str,
    id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<String>,
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

        if let Some(captures) = re.captures(line) {
            if let Some(asset_str) = captures.get(1) {
                let assets: u64 = asset_str.as_str().parse().map_err(|e| format!("Failed to parse assets: {}", e))?;
                log::info!("Found assets: {}", assets);
                total_assets += assets;
                asset_count += 1;
            }
        }
    }

    log::info!("Total assets summed: {}, Number of assets found: {}", total_assets, asset_count);
    if asset_count < 9 {
        log::warn!("Expected 9 assets, found only {}", asset_count);
        return Err(format!("Incomplete output: processed {} assets, expected 9", asset_count));
    }
    Ok(total_assets)
}

#[post("/rpc/getBalance")]
async fn list_notes_by_pubkey(req: web::Json<JsonRpcRequest>) -> impl Responder {
    log::info!("Received request: {:?}", req.0);

    if req.jsonrpc != "2.0" || req.method != "getBalance" || req.params.is_none() {
        log::error!("Invalid request: {:?}", req.0);
        return HttpResponse::BadRequest().json(JsonRpcResponse::<Value> {
            jsonrpc: "2.0",
            id: req.id,
            result: None,
            error: Some(JsonRpcError {
                code: -32600,
                message: "Invalid Request".to_string(),
                data: None,
            }),
        });
    }

    let pubkey = &req.params.as_ref().unwrap().pubkey;
    log::info!("Executing command for pubkey: {}", pubkey);

    let socket_path = match env::var("NOCKCHAIN_SOCKET") {
        Ok(path) => path,
        Err(e) => {
            log::error!("Missing NOCKCHAIN_SOCKET environment variable: {}", e);
            return HttpResponse::InternalServerError().json(JsonRpcResponse::<Value> {
                jsonrpc: "2.0",
                id: req.id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32000,
                    message: "Server error".to_string(),
                    data: Some(format!("Missing NOCKCHAIN_SOCKET environment variable: {}", e)),
                }),
            });
        }
    };

    let timeout_secs = match env::var("COMMAND_TIMEOUT_SECS") {
        Ok(secs) => match secs.parse::<u64>() {
            Ok(val) => val,
            Err(e) => {
                log::error!("Invalid COMMAND_TIMEOUT_SECS: {}", e);
                return HttpResponse::InternalServerError().json(JsonRpcResponse::<Value> {
                    jsonrpc: "2.0",
                    id: req.id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32000,
                        message: "Server error".to_string(),
                        data: Some(format!("Invalid COMMAND_TIMEOUT_SECS: {}", e)),
                    }),
                });
            }
        },
        Err(_e) => {
            log::warn!("Missing COMMAND_TIMEOUT_SECS, using default: 120 seconds");
            120
        }
    };

    let output = timeout(Duration::from_secs(timeout_secs), TokioCommand::new("nockchain-wallet")
        .env("RUST_LOG", "error")
        .arg("--nockchain-socket")
        .arg(&socket_path)
        .arg("list-notes-by-pubkey")
        .arg(pubkey)
        .output())
        .await;

    match output {
        Ok(Ok(output)) => {
            log::info!("Command executed, status: {}", output.status);
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                log::error!("Command failed: stderr={}", stderr);
                return HttpResponse::InternalServerError().json(JsonRpcResponse::<Value> {
                    jsonrpc: "2.0",
                    id: req.id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32000,
                        message: "Command execution failed".to_string(),
                        data: Some(stderr),
                    }),
                });
            }

            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            log::debug!("Raw command output: {}", stdout);
            match parse_nockchain_output(&stdout) {
                Ok(total_assets) => {
                    log::info!("Total assets: {}", total_assets);
                    // Divide by 65536.0 to convert nicks to nocks as unrounded decimal value
                    let total_assets = (total_assets as f64) / 65536.0;
                    log::info!("Total assets in nocks: {}", total_assets);
                    HttpResponse::Ok().json(JsonRpcResponse {
                        jsonrpc: "2.0",
                        id: req.id,
                        result: Some(serde_json::json!(total_assets)),
                        error: None,
                    })
                }
                Err(error) => {
                    log::error!("Parsing error: {}", error);
                    HttpResponse::InternalServerError().json(JsonRpcResponse::<Value> {
                        jsonrpc: "2.0",
                        id: req.id,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32002,
                            message: "Parsing error".to_string(),
                            data: Some(error),
                        }),
                    })
                }
            }
        }
        Ok(Err(error)) => {
            log::error!("Command error: {}", error);
            HttpResponse::InternalServerError().json(JsonRpcResponse::<Value> {
                jsonrpc: "2.0",
                id: req.id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32000,
                    message: "Server error".to_string(),
                    data: Some(error.to_string()),
                }),
            })
        }
        Err(_) => {
            log::error!("Command timed out");
            HttpResponse::InternalServerError().json(JsonRpcResponse::<Value> {
                jsonrpc: "2.0",
                id: req.id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32001,
                    message: "Command timed out".to_string(),
                    data: None,
                }),
            })
        }
    }
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenv().ok(); // Load .env file, ignore if missing
    env_logger::init();
    let port = match env::var("PORT") {
        Ok(port) => port.parse::<u16>().map_err(|e| {
            log::error!("Invalid PORT: {}", e);
            std::io::Error::new(std::io::ErrorKind::InvalidData, format!("Invalid PORT: {}", e))
        })?,
        Err(_) => {
            log::warn!("Missing PORT, using default: 3000");
            3000
        }
    };
    log::info!("Starting RPC server on http://localhost:{}", port);
    HttpServer::new(|| {
        App::new().service(list_notes_by_pubkey)
    })
    .bind(("127.0.0.1", port))?
    .run()
    .await
}