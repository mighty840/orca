//! Generated gRPC types from `proto/orca.proto`.

// tonic's generated service methods return `Result<_, tonic::Status>`, and
// `Status` is ~176 bytes: clippy 1.98 flags every one of them under
// `result_large_err`. Generated code cannot be reshaped, so opt out here.
#![allow(clippy::result_large_err)]

tonic::include_proto!("orca");
