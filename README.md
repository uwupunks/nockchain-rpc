## Nockchain RPC

Start the RPC server:

```
cargo run
```

## Available Endpoints

### getBalance

curl -X POST http://localhost:3000/rpc/getBalance \
-H 'Content-Type: application/json' \
-d '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "getBalance",
    "params": { "pubkey": "3XDSQxCvP3HVn1Q9geS7T1WBGqxAAJoWfEfSuhNQLhHvYVxyX5xJtKRLhbve2MUuX1LjowfCdM8iPo1sF14VV7Y4kGm1DqP1fCnKAViD1JecQukTSufVkcGVVTeHdfDvDs1u" }
}'