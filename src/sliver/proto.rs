//! Generated protobuf type wrappers for Sliver's gRPC protocol.
//! Compiled by tonic-build from the .proto files in the `protobuf/` directory.

#![allow(clippy::all)]
#![allow(unused_qualifications)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]

pub mod commonpb {
    tonic::include_proto!("commonpb");
}

pub mod clientpb {
    tonic::include_proto!("clientpb");
}

pub mod sliverpb {
    tonic::include_proto!("sliverpb");
}

pub mod rpcpb {
    tonic::include_proto!("rpcpb");
}
