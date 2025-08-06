use std::io::Result;

fn main() -> Result<()> {
    tonic_build::configure()
        .build_server(true)
        .build_client(false) // Only need server for now
        .compile_protos(&["protos/nockchain.proto"], &["protos"])?;
    Ok(())
}
