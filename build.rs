fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=protobuf/");
    tonic_build::configure()
        .build_client(true)
        .build_server(false)
        .compile_protos(&["protobuf/rpcpb/services.proto"], &["protobuf/"])?;
    Ok(())
}
