// The C2 protocol stays JSON+sealed-envelope; gRPC only carries the opaque
// blob (see proto/c2.proto), so no protocol types are duplicated here.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(true)
        .build_client(false)
        .compile_protos(&["../../proto/c2.proto"], &["../../proto"])?;
    Ok(())
}
