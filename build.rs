// The portal reuses the shared C2 carrier (proto/c2.proto): the protocol stays
// one sealed (AEAD) envelope and gRPC carries it as an opaque byte field, so no
// protocol type is duplicated here.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(true)
        .build_client(false)
        .compile_protos(&["proto/c2.proto"], &["proto"])?;
    Ok(())
}
