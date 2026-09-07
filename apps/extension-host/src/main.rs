// SPDX-License-Identifier: MPL-2.0
//! Private launch interface: an authenticated editor process supplies invocation
//! context. The executable accepts no online source and never grants capabilities.
#[cfg(windows)]
fn run() -> Result<(), String> {
    use bareline_extensions_protocol::{Invocation, MAX_CHUNK_BYTES, decode};
    use bareline_platform_windows::extension_transport::AuthenticatedPipe;
    use sha2::{Digest, Sha256};
    use std::{
        io::{Read, Write},
        sync::{Arc, atomic::AtomicBool},
        time::Duration,
    };
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 5 {
        return Err("expected pipe, nonce, parent PID, component path, component SHA256".into());
    }
    let mut nonce = [0u8; 32];
    if args[1].len() != 64 {
        return Err("nonce length".into());
    }
    for (index, byte) in nonce.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&args[1][index * 2..index * 2 + 2], 16)
            .map_err(|_| "nonce encoding")?;
    }
    let parent = args[2].parse().map_err(|_| "parent PID")?;
    let mut pipe = AuthenticatedPipe::connect(&args[0], nonce, parent, Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    let mut prefix = [0; 4];
    pipe.read_exact(&mut prefix).map_err(|e| e.to_string())?;
    let size = u32::from_le_bytes(prefix) as usize;
    if size > MAX_CHUNK_BYTES {
        return Err("invocation size".into());
    }
    let mut invocation = vec![0; size];
    pipe.read_exact(&mut invocation)
        .map_err(|e| e.to_string())?;
    let context: Invocation = decode(&invocation).map_err(|_| "invalid invocation")?;
    if context.arguments.len() > 4096 {
        return Err("invocation arguments limit".into());
    }
    let file = std::fs::File::open(&args[3]).map_err(|e| e.to_string())?;
    let mut component = Vec::new();
    file.take(32 * 1024 * 1024 + 1)
        .read_to_end(&mut component)
        .map_err(|e| e.to_string())?;
    if component.len() > 32 * 1024 * 1024 || format!("{:x}", Sha256::digest(&component)) != args[4]
    {
        return Err("component hash or size".into());
    }
    let runtime = bareline_extension_host::Runtime::new().map_err(|e| e.to_string())?;
    runtime
        .invoke_with_broker(
            &component,
            invocation,
            Box::new(move |request| {
                // Parse bounded envelope before crossing into the parent broker.
                bareline_extensions_protocol::read_frame(&mut &request[..])
                    .map_err(|_| "malformed request".to_owned())?;
                pipe.write_all(&request).map_err(|e| e.to_string())?;
                let mut prefix = [0; 4];
                pipe.read_exact(&mut prefix).map_err(|e| e.to_string())?;
                let count = u32::from_le_bytes(prefix) as usize;
                if count > MAX_CHUNK_BYTES {
                    return Err("reply size".into());
                }
                let mut response = vec![0; count];
                pipe.read_exact(&mut response).map_err(|e| e.to_string())?;
                Ok(response)
            }),
            Arc::new(AtomicBool::new(false)),
        )
        .map_err(|e| e.to_string())
}
fn main() {
    #[cfg(windows)]
    if let Err(error) = run() {
        eprintln!("Extension host stopped: {error}");
        std::process::exit(1);
    }
    #[cfg(not(windows))]
    {
        eprintln!("Authenticated native host transport is unavailable on this platform");
        std::process::exit(1);
    }
}
