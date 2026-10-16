// Client side of the shared C2 carrier (proto/c2.proto). The protocol stays
// JSON+sealed-envelope; gRPC moves the opaque blob only.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(false)
        .build_client(true)
        .compile_protos(&["../../proto/c2.proto"], &["../../proto"])?;
    Ok(())
}
