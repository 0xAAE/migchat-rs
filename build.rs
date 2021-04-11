fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::compile_protos("third-party/migchat-proto/migchat.proto")?;
    Ok(())
}
