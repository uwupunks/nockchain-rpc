## Nockchain gRPC Service

## Requirements
1. Synced Full Node
2. Nockchain Wallet Installed

## Install Deps
```
brew install protobuf
brew install grpcurl

```

## Running the server
Start the RPC server:

```
cargo build
cargo run
```

## Available Endpoints

### GetBalance (localhost)

```
grpcurl -plaintext -import-path protos -proto nockchain.proto \
-d '{"pubkey": "3XDSQxCvP3HVn1Q9geS7T1WBGqxAAJoWfEfSuhNQLhHvYVxyX5xJtKRLhbve2MUuX1LjowfCdM8iPo1sF14VV7Y4kGm1DqP1fCnKAViD1JecQukTSufVkcGVVTeHdfDvDs1u"}' \
127.0.0.1:3000 nockchain.NockchainService/GetBalance
```

### GetBalance (rpc.nocknames.com) 
```
grpcurl -import-path protos -proto nockchain.proto --max-time 120  \
-d '{"pubkey": "3XDSQxCvP3HVn1Q9geS7T1WBGqxAAJoWfEfSuhNQLhHvYVxyX5xJtKRLhbve2MUuX1LjowfCdM8iPo1sF14VV7Y4kGm1DqP1fCnKAViD1JecQukTSufVkcGVVTeHdfDvDs1u"}' \
rpc.nocknames.com:443 nockchain.NockchainService/GetBalance
```
